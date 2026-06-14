//! No-op stand-in for the embedded libmpv player (`src/mpv.rs`), compiled when
//! the `embedded-mpv` feature is OFF — e.g. a Windows build that links no
//! libmpv. Mirrors the public API so every `mpv::` call site keeps compiling.
//!
//! Effect: the in-app **Videos** player is inert (renders nothing). All audio —
//! local music, podcasts, radio, audiobooks, YouTube — is unaffected: those use
//! the out-of-process `mpv.exe` IPC path (`src/mpv_ipc.rs`), not libmpv.
#![allow(dead_code, unused_variables)]

use crate::MainWindow;
use std::ffi::CStr;

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

#[derive(Clone)]
pub struct Track {
    pub id: i64,
    pub kind: String,
    pub label: String,
    pub selected: bool,
}

#[derive(Clone)]
pub struct Chapter {
    pub index: i64,
    pub title: String,
    pub time: f64,
}

pub fn set_window(_w: slint::Weak<MainWindow>) {}
pub fn init() -> bool { false }
pub fn open(_path: &str) {}
pub fn command(_args: &[&str]) {}
pub fn set_flag(_name: &str, _on: bool) {}
pub fn set_double(_name: &str, _val: f64) {}
pub fn set_string(_name: &str, _val: &str) {}
pub fn get_prop(_name: &str) -> Option<String> { None }
pub fn stop() {}

pub fn render_frame(
    _get_proc: &dyn Fn(&CStr) -> *const core::ffi::c_void,
    _w: i32,
    _h: i32,
) -> Option<slint::Image> {
    None
}

pub fn teardown() {}

pub fn on_update(_cb: impl Fn(Update) + Send + Sync + 'static) {}
pub fn on_tracks(_cb: impl Fn(Vec<Track>, Vec<Chapter>, String) + Send + Sync + 'static) {}
