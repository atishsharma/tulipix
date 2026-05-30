//! Embedded libmpv player (np.p3.player.*).
//!
//! libmpv renders video frames into an OpenGL FBO/texture via its render API
//! (`render_gl.h`); that texture is handed to Slint as a `BorrowedOpenGLTexture`
//! so the video paints *inside* the app window — no external window, no webview.
//!
//! Threading:
//!   * GL + the render context live on the Slint UI thread, driven from the
//!     window's rendering notifier (the only place the GL context is current).
//!   * mpv's command/property API is thread-safe, so a background event thread
//!     pumps `mpv_wait_event` and pushes property changes back to the UI via
//!     `slint::invoke_from_event_loop`.
//!   * mpv's "update" callback (frame ready) runs on an arbitrary thread; it
//!     only flips a flag + requests a redraw.

#![allow(non_camel_case_types, non_upper_case_globals)]

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::MainWindow;
use slint::ComponentHandle;

// ─────────────────────────── libmpv FFI ───────────────────────────

enum mpv_handle {}
enum mpv_render_context {}

const MPV_FORMAT_STRING: c_int = 1;
const MPV_FORMAT_FLAG: c_int = 3;
const MPV_FORMAT_INT64: c_int = 4;
const MPV_FORMAT_DOUBLE: c_int = 5;

const MPV_EVENT_SHUTDOWN: c_int = 1;
const MPV_EVENT_FILE_LOADED: c_int = 8;
const MPV_EVENT_END_FILE: c_int = 7;
const MPV_EVENT_PROPERTY_CHANGE: c_int = 22;

const MPV_RENDER_PARAM_INVALID: c_int = 0;
const MPV_RENDER_PARAM_API_TYPE: c_int = 1;
const MPV_RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
const MPV_RENDER_PARAM_OPENGL_FBO: c_int = 3;
const MPV_RENDER_PARAM_FLIP_Y: c_int = 4;

#[repr(C)]
struct mpv_render_param {
    type_: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct mpv_opengl_init_params {
    get_proc_address: Option<extern "C" fn(*mut c_void, *const c_char) -> *mut c_void>,
    get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
struct mpv_opengl_fbo {
    fbo: c_int,
    w: c_int,
    h: c_int,
    internal_format: c_int,
}

#[repr(C)]
struct mpv_event {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct mpv_event_property {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

#[link(name = "mpv")]
unsafe extern "C" {
    fn mpv_create() -> *mut mpv_handle;
    fn mpv_initialize(ctx: *mut mpv_handle) -> c_int;
    fn mpv_terminate_destroy(ctx: *mut mpv_handle);
    fn mpv_set_option_string(ctx: *mut mpv_handle, name: *const c_char, data: *const c_char) -> c_int;
    fn mpv_set_property_string(ctx: *mut mpv_handle, name: *const c_char, data: *const c_char) -> c_int;
    fn mpv_set_property(ctx: *mut mpv_handle, name: *const c_char, format: c_int, data: *mut c_void) -> c_int;
    fn mpv_get_property_string(ctx: *mut mpv_handle, name: *const c_char) -> *mut c_char;
    fn mpv_command(ctx: *mut mpv_handle, args: *mut *const c_char) -> c_int;
    fn mpv_observe_property(ctx: *mut mpv_handle, reply: u64, name: *const c_char, format: c_int) -> c_int;
    fn mpv_wait_event(ctx: *mut mpv_handle, timeout: f64) -> *mut mpv_event;
    fn mpv_free(data: *mut c_void);

    fn mpv_render_context_create(out: *mut *mut mpv_render_context, ctx: *mut mpv_handle, params: *mut mpv_render_param) -> c_int;
    fn mpv_render_context_render(rctx: *mut mpv_render_context, params: *mut mpv_render_param) -> c_int;
    fn mpv_render_context_set_update_callback(rctx: *mut mpv_render_context, cb: extern "C" fn(*mut c_void), ctx: *mut c_void);
    fn mpv_render_context_report_swap(rctx: *mut mpv_render_context);
    fn mpv_render_context_free(rctx: *mut mpv_render_context);
}

// ─────────────────────────── minimal GL ───────────────────────────

type GLenum = u32;
type GLuint = u32;
type GLint = i32;
type GLsizei = i32;

const GL_TEXTURE_2D: GLenum = 0x0DE1;
const GL_RGBA: GLenum = 0x1908;
const GL_RGBA8: GLenum = 0x8058;
const GL_UNSIGNED_BYTE: GLenum = 0x1401;
const GL_FRAMEBUFFER: GLenum = 0x8D40;
const GL_FRAMEBUFFER_BINDING: GLenum = 0x8CA6;
const GL_COLOR_ATTACHMENT0: GLenum = 0x8CE0;
const GL_TEXTURE_MIN_FILTER: GLenum = 0x2801;
const GL_TEXTURE_MAG_FILTER: GLenum = 0x2800;
const GL_TEXTURE_WRAP_S: GLenum = 0x2802;
const GL_TEXTURE_WRAP_T: GLenum = 0x2803;
const GL_LINEAR: GLint = 0x2601;
const GL_CLAMP_TO_EDGE: GLint = 0x812F;
const GL_VIEWPORT: GLenum = 0x0BA2;
const GL_COLOR_BUFFER_BIT: GLenum = 0x0000_4000;
const GL_SCISSOR_TEST: GLenum = 0x0C11;

struct GlFns {
    gen_textures: extern "C" fn(GLsizei, *mut GLuint),
    bind_texture: extern "C" fn(GLenum, GLuint),
    tex_image_2d: extern "C" fn(GLenum, GLint, GLint, GLsizei, GLsizei, GLint, GLenum, GLenum, *const c_void),
    tex_parameteri: extern "C" fn(GLenum, GLenum, GLint),
    gen_framebuffers: extern "C" fn(GLsizei, *mut GLuint),
    bind_framebuffer: extern "C" fn(GLenum, GLuint),
    framebuffer_texture_2d: extern "C" fn(GLenum, GLenum, GLenum, GLuint, GLint),
    delete_textures: extern "C" fn(GLsizei, *const GLuint),
    delete_framebuffers: extern "C" fn(GLsizei, *const GLuint),
    get_integerv: extern "C" fn(GLenum, *mut GLint),
    viewport: extern "C" fn(GLint, GLint, GLsizei, GLsizei),
    clear_color: extern "C" fn(f32, f32, f32, f32),
    clear: extern "C" fn(GLenum),
    disable: extern "C" fn(GLenum),
    is_enabled: extern "C" fn(GLenum) -> u8,
    enable: extern "C" fn(GLenum),
}

impl GlFns {
    fn load(get_proc: &dyn Fn(&CStr) -> *const c_void) -> Option<GlFns> {
        macro_rules! sym {
            ($n:literal) => {{
                let c = CString::new($n).ok()?;
                let p = get_proc(&c);
                if p.is_null() { return None; }
                unsafe { std::mem::transmute(p) }
            }};
        }
        Some(GlFns {
            gen_textures: sym!("glGenTextures"),
            bind_texture: sym!("glBindTexture"),
            tex_image_2d: sym!("glTexImage2D"),
            tex_parameteri: sym!("glTexParameteri"),
            gen_framebuffers: sym!("glGenFramebuffers"),
            bind_framebuffer: sym!("glBindFramebuffer"),
            framebuffer_texture_2d: sym!("glFramebufferTexture2D"),
            delete_textures: sym!("glDeleteTextures"),
            delete_framebuffers: sym!("glDeleteFramebuffers"),
            get_integerv: sym!("glGetIntegerv"),
            viewport: sym!("glViewport"),
            clear_color: sym!("glClearColor"),
            clear: sym!("glClear"),
            disable: sym!("glDisable"),
            is_enabled: sym!("glIsEnabled"),
            enable: sym!("glEnable"),
        })
    }
}

// ─────────────────────────── globals ───────────────────────────

// mpv handle: Send-safe (the C API is thread-safe). Render context is only
// touched on the UI thread.
static CTX: AtomicPtr<mpv_handle> = AtomicPtr::new(ptr::null_mut());
static RENDER: AtomicPtr<mpv_render_context> = AtomicPtr::new(ptr::null_mut());
static FRAME_READY: AtomicBool = AtomicBool::new(false);
static REDRAW_PENDING: AtomicBool = AtomicBool::new(false);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static WIN: OnceLock<Mutex<Option<slint::Weak<MainWindow>>>> = OnceLock::new();

fn win_slot() -> &'static Mutex<Option<slint::Weak<MainWindow>>> {
    WIN.get_or_init(|| Mutex::new(None))
}

// GL render state lives on the UI thread only.
thread_local! {
    static GL: std::cell::RefCell<Option<GlFns>> = const { std::cell::RefCell::new(None) };
    static TEX: std::cell::Cell<GLuint> = const { std::cell::Cell::new(0) };
    static FBO: std::cell::Cell<GLuint> = const { std::cell::Cell::new(0) };
    static TEX_W: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
    static TEX_H: std::cell::Cell<i32> = const { std::cell::Cell::new(0) };
    // True once a real mpv frame has been rendered into the current texture, so
    // we never present an uninitialised (garbage/black) texture.
    static RENDERED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    // Set for the duration of an mpv_render_context_create call so the
    // trampoline can reach the live get_proc_address closure.
    static CUR_GET_PROC: std::cell::Cell<*const c_void> = const { std::cell::Cell::new(ptr::null()) };
}

fn ctx() -> *mut mpv_handle { CTX.load(Ordering::Acquire) }

/// One-time setup: store the window handle for redraw requests.
pub fn set_window(w: slint::Weak<MainWindow>) {
    *win_slot().lock().unwrap() = Some(w);
}

fn cstr(s: &str) -> CString { CString::new(s).unwrap_or_default() }

// ─────────────────────────── lifecycle ───────────────────────────

/// Create + initialise the mpv handle (idempotent). Returns false if libmpv
/// could not be created. The render context is created lazily in `render`.
pub fn init() -> bool {
    if !ctx().is_null() { return true; }
    let h = unsafe { mpv_create() };
    if h.is_null() { tracing::error!("mpv_create failed"); return false; }
    unsafe {
        // No built-in OSC/window; we drive everything from Slint.
        let set = |k: &str, v: &str| { mpv_set_option_string(h, cstr(k).as_ptr(), cstr(v).as_ptr()); };
        set("vo", "libmpv");
        set("hwdec", "auto-safe");      // np.p3.player.hwdec
        set("keep-open", "yes");
        set("force-window", "no");
        set("ytdl", "no");
        set("osc", "no");
        set("input-default-bindings", "no");
        if mpv_initialize(h) < 0 {
            tracing::error!("mpv_initialize failed");
            mpv_terminate_destroy(h);
            return false;
        }
        // Observe playback state for the controls overlay.
        let obs = |name: &str, fmt: c_int| { mpv_observe_property(h, 0, cstr(name).as_ptr(), fmt); };
        obs("time-pos", MPV_FORMAT_DOUBLE);
        obs("duration", MPV_FORMAT_DOUBLE);
        obs("pause", MPV_FORMAT_FLAG);
        obs("volume", MPV_FORMAT_DOUBLE);
        obs("speed", MPV_FORMAT_DOUBLE);
        obs("mute", MPV_FORMAT_FLAG);
        obs("chapter", MPV_FORMAT_INT64);
        obs("core-idle", MPV_FORMAT_FLAG);
    }
    CTX.store(h, Ordering::Release);
    spawn_event_thread();
    true
}

/// Load a file and start the embedded player.
pub fn open(path: &str) {
    if !init() { return; }
    ACTIVE.store(true, Ordering::Release);
    RENDERED.with(|c| c.set(false)); // wait for this file's first frame before presenting
    let h = ctx();
    unsafe {
        let a0 = cstr("loadfile");
        let a1 = cstr(path);
        let mut args = [a0.as_ptr(), a1.as_ptr(), ptr::null()];
        mpv_command(h, args.as_mut_ptr());
        let f: c_int = 0;
        let mut pause = f;
        mpv_set_property(h, cstr("pause").as_ptr(), MPV_FORMAT_FLAG, &mut pause as *mut _ as *mut c_void);
    }
}

pub fn command(args: &[&str]) {
    let h = ctx();
    if h.is_null() { return; }
    let cstrs: Vec<CString> = args.iter().map(|s| cstr(s)).collect();
    let mut ptrs: Vec<*const c_char> = cstrs.iter().map(|c| c.as_ptr()).collect();
    ptrs.push(ptr::null());
    unsafe { mpv_command(h, ptrs.as_mut_ptr()); }
}

pub fn set_flag(name: &str, on: bool) {
    let h = ctx();
    if h.is_null() { return; }
    let mut v: c_int = if on { 1 } else { 0 };
    unsafe { mpv_set_property(h, cstr(name).as_ptr(), MPV_FORMAT_FLAG, &mut v as *mut _ as *mut c_void); }
}

pub fn set_double(name: &str, val: f64) {
    let h = ctx();
    if h.is_null() { return; }
    let mut v = val;
    unsafe { mpv_set_property(h, cstr(name).as_ptr(), MPV_FORMAT_DOUBLE, &mut v as *mut _ as *mut c_void); }
}

pub fn set_string(name: &str, val: &str) {
    let h = ctx();
    if h.is_null() { return; }
    unsafe { mpv_set_property_string(h, cstr(name).as_ptr(), cstr(val).as_ptr()); }
}

/// Read an mpv property as a string (e.g. "video-params/gamma"). None if unset.
pub fn get_prop(name: &str) -> Option<String> { get_string(name) }

fn get_string(name: &str) -> Option<String> {
    let h = ctx();
    if h.is_null() { return None; }
    unsafe {
        let p = mpv_get_property_string(h, cstr(name).as_ptr());
        if p.is_null() { return None; }
        let s = CStr::from_ptr(p).to_string_lossy().into_owned();
        mpv_free(p as *mut c_void);
        Some(s)
    }
}

/// Stop playback + hide the player. The handle is kept alive for reuse.
pub fn stop() {
    ACTIVE.store(false, Ordering::Release);
    command(&["stop"]);
}

// ─────────────────────────── render ───────────────────────────

extern "C" fn get_proc_trampoline(_ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    let stored = CUR_GET_PROC.with(|c| c.get());
    if stored.is_null() || name.is_null() { return ptr::null_mut(); }
    // stored points at the live `&dyn Fn(&CStr) -> *const c_void`.
    let f = unsafe { &*(stored as *const &dyn Fn(&CStr) -> *const c_void) };
    let cname = unsafe { CStr::from_ptr(name) };
    f(cname) as *mut c_void
}

extern "C" fn on_mpv_update(_ctx: *mut c_void) {
    FRAME_READY.store(true, Ordering::Release);
    // Coalesce redraw requests so a fast frame source doesn't flood the loop.
    if REDRAW_PENDING.swap(true, Ordering::AcqRel) { return; }
    let weak = win_slot().lock().ok().and_then(|g| g.clone());
    let _ = slint::invoke_from_event_loop(move || {
        REDRAW_PENDING.store(false, Ordering::Release);
        if let Some(w) = weak.and_then(|w| w.upgrade()) { w.window().request_redraw(); }
    });
}

/// Render the current mpv frame into our FBO and return the Slint image that
/// samples it. Called from the rendering notifier (GL context current).
pub fn render_frame(get_proc: &dyn Fn(&CStr) -> *const c_void, w: i32, h: i32) -> Option<slint::Image> {
    if !ACTIVE.load(Ordering::Acquire) || w <= 0 || h <= 0 { return None; }
    if ctx().is_null() { return None; }

    // Lazily create the render context (needs get_proc for GL symbol loading).
    if RENDER.load(Ordering::Acquire).is_null() {
        let gp_ref: &dyn Fn(&CStr) -> *const c_void = get_proc;
        let gp_ptr = &gp_ref as *const &dyn Fn(&CStr) -> *const c_void as *const c_void;
        CUR_GET_PROC.with(|c| c.set(gp_ptr));
        let mut init = mpv_opengl_init_params {
            get_proc_address: Some(get_proc_trampoline),
            get_proc_address_ctx: ptr::null_mut(),
        };
        let api = cstr("opengl");
        let mut params = [
            mpv_render_param { type_: MPV_RENDER_PARAM_API_TYPE, data: api.as_ptr() as *mut c_void },
            mpv_render_param { type_: MPV_RENDER_PARAM_OPENGL_INIT_PARAMS, data: &mut init as *mut _ as *mut c_void },
            mpv_render_param { type_: MPV_RENDER_PARAM_INVALID, data: ptr::null_mut() },
        ];
        let mut rctx: *mut mpv_render_context = ptr::null_mut();
        let rc = unsafe { mpv_render_context_create(&mut rctx, ctx(), params.as_mut_ptr()) };
        CUR_GET_PROC.with(|c| c.set(ptr::null()));
        if rc < 0 || rctx.is_null() {
            tracing::error!(rc, "mpv_render_context_create failed");
            return None;
        }
        unsafe { mpv_render_context_set_update_callback(rctx, on_mpv_update, ptr::null_mut()); }
        RENDER.store(rctx, Ordering::Release);
    }

    // Load GL fns once.
    GL.with(|g| {
        if g.borrow().is_none() { *g.borrow_mut() = GlFns::load(get_proc); }
    });
    let have_gl = GL.with(|g| g.borrow().is_some());
    if !have_gl { return None; }

    // (Re)allocate the texture + FBO when the target size changes.
    let (tw, th) = (TEX_W.with(|c| c.get()), TEX_H.with(|c| c.get()));
    if tw != w || th != h {
        GL.with(|g| {
            let gl = g.borrow();
            let gl = gl.as_ref().unwrap();
            // Drop the previous allocation.
            let old_tex = TEX.with(|c| c.get());
            let old_fbo = FBO.with(|c| c.get());
            if old_tex != 0 { (gl.delete_textures)(1, &old_tex); }
            if old_fbo != 0 { (gl.delete_framebuffers)(1, &old_fbo); }
            let mut tex: GLuint = 0;
            (gl.gen_textures)(1, &mut tex);
            (gl.bind_texture)(GL_TEXTURE_2D, tex);
            (gl.tex_image_2d)(GL_TEXTURE_2D, 0, GL_RGBA8 as GLint, w, h, 0, GL_RGBA, GL_UNSIGNED_BYTE, ptr::null());
            (gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MIN_FILTER, GL_LINEAR);
            (gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_MAG_FILTER, GL_LINEAR);
            (gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE);
            (gl.tex_parameteri)(GL_TEXTURE_2D, GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE);
            let mut fbo: GLuint = 0;
            (gl.gen_framebuffers)(1, &mut fbo);
            (gl.bind_framebuffer)(GL_FRAMEBUFFER, fbo);
            (gl.framebuffer_texture_2d)(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, tex, 0);
            (gl.bind_texture)(GL_TEXTURE_2D, 0);
            TEX.with(|c| c.set(tex));
            FBO.with(|c| c.set(fbo));
        });
        TEX_W.with(|c| c.set(w));
        TEX_H.with(|c| c.set(h));
        RENDERED.with(|c| c.set(false)); // new texture needs a fresh frame
        FRAME_READY.store(true, Ordering::Release);
    }

    let rctx = RENDER.load(Ordering::Acquire);
    if rctx.is_null() { return None; }
    let fbo = FBO.with(|c| c.get());
    let tex = TEX.with(|c| c.get());
    if fbo == 0 || tex == 0 { return None; }

    // Only re-render mpv into our texture when a new frame is actually ready.
    // Rendering on every Slint repaint (UI hover, etc.) would draw black/last
    // frames in alternation against the real ones — the whole-screen flicker.
    if FRAME_READY.swap(false, Ordering::AcqRel) {
        // Save the GL state mpv clobbers: the bound framebuffer (Slint's) and
        // the viewport. Skia draws the whole UI afterwards and assumes both are
        // unchanged — not restoring the viewport corrupts the entire frame.
        let mut prev_fbo: GLint = 0;
        let mut vp: [GLint; 4] = [0; 4];
        let mut scissor_on = 0u8;
        GL.with(|g| {
            let gl = g.borrow();
            let gl = gl.as_ref().unwrap();
            (gl.get_integerv)(GL_FRAMEBUFFER_BINDING, &mut prev_fbo);
            (gl.get_integerv)(GL_VIEWPORT, vp.as_mut_ptr());
            scissor_on = (gl.is_enabled)(GL_SCISSOR_TEST);
            // Clear our FBO to OPAQUE black so letterbox / not-yet-decoded areas
            // are never transparent (otherwise the video appears to float over
            // the desktop). Scissor off so the whole texture is cleared.
            (gl.bind_framebuffer)(GL_FRAMEBUFFER, fbo);
            if scissor_on != 0 { (gl.disable)(GL_SCISSOR_TEST); }
            (gl.viewport)(0, 0, w, h);
            (gl.clear_color)(0.0, 0.0, 0.0, 1.0);
            (gl.clear)(GL_COLOR_BUFFER_BIT);
            if scissor_on != 0 { (gl.enable)(GL_SCISSOR_TEST); }
        });

        let mut fbo_param = mpv_opengl_fbo { fbo: fbo as c_int, w, h, internal_format: 0 };
        let mut flip: c_int = 1; // top-left origin to match Slint's default
        let mut params = [
            mpv_render_param { type_: MPV_RENDER_PARAM_OPENGL_FBO, data: &mut fbo_param as *mut _ as *mut c_void },
            mpv_render_param { type_: MPV_RENDER_PARAM_FLIP_Y, data: &mut flip as *mut _ as *mut c_void },
            mpv_render_param { type_: MPV_RENDER_PARAM_INVALID, data: ptr::null_mut() },
        ];
        unsafe { mpv_render_context_render(rctx, params.as_mut_ptr()); }
        unsafe { mpv_render_context_report_swap(rctx); }

        GL.with(|g| {
            let gl = g.borrow();
            let gl = gl.as_ref().unwrap();
            (gl.bind_framebuffer)(GL_FRAMEBUFFER, prev_fbo as GLuint);
            (gl.viewport)(vp[0], vp[1], vp[2], vp[3]);
        });
        RENDERED.with(|c| c.set(true));
    }

    // Don't present until we have a real frame (avoids a black flash).
    if !RENDERED.with(|c| c.get()) { return None; }
    let texture_id = std::num::NonZeroU32::new(tex)?;
    let img = unsafe {
        slint::BorrowedOpenGLTextureBuilder::new_gl_2d_rgba_texture(
            texture_id, [w as u32, h as u32].into()).build()
    };
    Some(img)
}

/// Free GL + render resources on window teardown.
pub fn teardown() {
    let rctx = RENDER.swap(ptr::null_mut(), Ordering::AcqRel);
    if !rctx.is_null() { unsafe { mpv_render_context_free(rctx); } }
    GL.with(|g| {
        if let Some(gl) = g.borrow().as_ref() {
            let tex = TEX.with(|c| c.get());
            let fbo = FBO.with(|c| c.get());
            if tex != 0 { (gl.delete_textures)(1, &tex); }
            if fbo != 0 { (gl.delete_framebuffers)(1, &fbo); }
        }
    });
}

// ─────────────────────────── event pump ───────────────────────────

#[derive(Clone)]
pub enum Update {
    TimePos(f64),
    Duration(f64),
    Pause(bool),
    Volume(f64),
    Speed(f64),
    Mute(bool),
    Chapter(i64),
    Idle(bool),
    FileLoaded,
    EndFile,
}

fn spawn_event_thread() {
    let h_addr = ctx() as usize;
    std::thread::Builder::new().name("mpv-events".into()).spawn(move || {
        let h = h_addr as *mut mpv_handle;
        loop {
            let ev = unsafe { mpv_wait_event(h, 1.0) };
            if ev.is_null() { continue; }
            let id = unsafe { (*ev).event_id };
            match id {
                MPV_EVENT_SHUTDOWN => break,
                MPV_EVENT_FILE_LOADED => {
                    push(Update::FileLoaded);
                    refresh_tracks_and_chapters();
                }
                MPV_EVENT_END_FILE => push(Update::EndFile),
                MPV_EVENT_PROPERTY_CHANGE => {
                    let p = unsafe { &*((*ev).data as *const mpv_event_property) };
                    if p.name.is_null() || p.data.is_null() { continue; }
                    let name = unsafe { CStr::from_ptr(p.name) }.to_string_lossy();
                    match (name.as_ref(), p.format) {
                        ("time-pos", MPV_FORMAT_DOUBLE) => push(Update::TimePos(unsafe { *(p.data as *const f64) })),
                        ("duration", MPV_FORMAT_DOUBLE) => push(Update::Duration(unsafe { *(p.data as *const f64) })),
                        ("volume", MPV_FORMAT_DOUBLE) => push(Update::Volume(unsafe { *(p.data as *const f64) })),
                        ("speed", MPV_FORMAT_DOUBLE) => push(Update::Speed(unsafe { *(p.data as *const f64) })),
                        ("pause", MPV_FORMAT_FLAG) => push(Update::Pause(unsafe { *(p.data as *const c_int) } != 0)),
                        ("mute", MPV_FORMAT_FLAG) => push(Update::Mute(unsafe { *(p.data as *const c_int) } != 0)),
                        ("core-idle", MPV_FORMAT_FLAG) => push(Update::Idle(unsafe { *(p.data as *const c_int) } != 0)),
                        ("chapter", MPV_FORMAT_INT64) => push(Update::Chapter(unsafe { *(p.data as *const i64) })),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }).ok();
}

/// Track + chapter rows, read off the mpv thread on file-load and pushed to UI.
#[derive(Clone)]
pub struct Track { pub id: i64, pub kind: String, pub label: String, pub selected: bool }
#[derive(Clone)]
pub struct Chapter { pub index: i64, pub title: String, pub time: f64 }

type TracksCb = std::sync::Arc<dyn Fn(Vec<Track>, Vec<Chapter>, String) + Send + Sync>;
static TRACKS_CB: OnceLock<Mutex<Option<TracksCb>>> = OnceLock::new();
pub fn on_tracks(cb: impl Fn(Vec<Track>, Vec<Chapter>, String) + Send + Sync + 'static) {
    *TRACKS_CB.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(std::sync::Arc::new(cb));
}

fn refresh_tracks_and_chapters() {
    let n: i64 = get_string("track-list/count").and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut tracks = Vec::new();
    for i in 0..n {
        let kind = get_string(&format!("track-list/{i}/type")).unwrap_or_default();
        let id: i64 = get_string(&format!("track-list/{i}/id")).and_then(|s| s.parse().ok()).unwrap_or(0);
        let title = get_string(&format!("track-list/{i}/title")).unwrap_or_default();
        let lang = get_string(&format!("track-list/{i}/lang")).unwrap_or_default();
        let selected = get_string(&format!("track-list/{i}/selected")).map(|s| s == "yes").unwrap_or(false);
        let label = match (title.is_empty(), lang.is_empty()) {
            (false, false) => format!("{title} ({lang})"),
            (false, true) => title,
            (true, false) => lang,
            (true, true) => format!("Track {id}"),
        };
        tracks.push(Track { id, kind, label, selected });
    }
    let cn: i64 = get_string("chapter-list/count").and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut chapters = Vec::new();
    for i in 0..cn {
        let title = get_string(&format!("chapter-list/{i}/title"))
            .filter(|s| !s.is_empty()).unwrap_or_else(|| format!("Chapter {}", i + 1));
        let time: f64 = get_string(&format!("chapter-list/{i}/time")).and_then(|s| s.parse().ok()).unwrap_or(0.0);
        chapters.push(Chapter { index: i, title, time });
    }
    let hwdec = get_string("hwdec-current").filter(|s| !s.is_empty() && s != "no")
        .unwrap_or_else(|| "software".into());
    if let Some(cb) = TRACKS_CB.get().and_then(|m| m.lock().ok()).and_then(|g| g.clone()) {
        cb(tracks, chapters, hwdec);
    }
}

static UPDATE_CB: OnceLock<Mutex<Option<std::sync::Arc<dyn Fn(Update) + Send + Sync>>>> = OnceLock::new();
pub fn on_update(cb: impl Fn(Update) + Send + Sync + 'static) {
    *UPDATE_CB.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(std::sync::Arc::new(cb));
}
fn push(u: Update) {
    if let Some(cb) = UPDATE_CB.get().and_then(|m| m.lock().ok()).and_then(|g| g.clone()) {
        cb(u);
    }
}
