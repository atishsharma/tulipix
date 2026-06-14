fn main() {
    slint_build::compile("../../ui/main.slint").expect("Slint compile failed");

    // Embed the Tulipix icon as a Win32 resource. winit uses the executable's
    // first icon resource as the default window/taskbar icon, so this fixes the
    // missing icon without any runtime code. Host-gated: only runs on a native
    // Windows build (where the embed-resource build-dep is present).
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed=app.rc");
        println!("cargo:rerun-if-changed=icon.ico");
        embed_resource::compile("app.rc", embed_resource::NONE);
    }

    // Trends feed list is include_str!'d from podc.md — rebuild when it changes.
    println!("cargo:rerun-if-changed=../../podc.md");
    // Embedded player links libmpv directly (render API). On Linux the system
    // mpv package ships libmpv.so on the default search path, so no hint is
    // needed. On Windows/macOS (or a Linux install in a non-standard prefix)
    // point the linker at the dir holding the import lib / dylib via env var:
    //
    //   Windows : set TULIPIX_MPV_LIB_DIR=C:\path\to\mpv-dev\lib   (has mpv.lib)
    //   macOS   : export TULIPIX_MPV_LIB_DIR=/opt/homebrew/opt/mpv/lib
    //   Linux   : usually unnecessary; set it for a custom prefix.
    //
    // `MPV_LIB_DIR` is accepted as a fallback alias.
    println!("cargo:rerun-if-env-changed=TULIPIX_MPV_LIB_DIR");
    println!("cargo:rerun-if-env-changed=MPV_LIB_DIR");

    // Only link libmpv when the embedded Videos player is compiled in. A build
    // with `--no-default-features` (no `embedded-mpv`) skips the link entirely,
    // so Windows needs no mpv.lib / mpv-2.dll / TULIPIX_MPV_LIB_DIR.
    if std::env::var_os("CARGO_FEATURE_EMBEDDED_MPV").is_some() {
        if let Some(dir) = std::env::var_os("TULIPIX_MPV_LIB_DIR")
            .or_else(|| std::env::var_os("MPV_LIB_DIR"))
        {
            println!("cargo:rustc-link-search=native={}", dir.to_string_lossy());
        }
        // Link libmpv. Basename is `mpv` on every OS:
        //   Linux/macOS -> libmpv.so / libmpv.dylib
        //   Windows     -> mpv.lib (import lib for mpv-2.dll)
        println!("cargo:rustc-link-lib=mpv");
    }
}
