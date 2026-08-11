fn main() {
    // The Slint UI is compiled by the tulipix-ui crate (its own rustc unit);
    // this crate just links against it. Rebuild if any .slint changes.
    println!("cargo:rerun-if-changed=../../ui");

    // Embed the Tulipix icon as a Win32 resource. winit uses the executable's
    // first icon resource as the default window/taskbar icon, so this fixes the
    // missing icon without any runtime code. Host-gated: only runs on a native
    // Windows build (where the embed-resource build-dep is present).
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed=app.rc");
        println!("cargo:rerun-if-changed=icon.ico");
        let _ = embed_resource::compile("app.rc", embed_resource::NONE);
    }

    // Trends feed list is include_str!'d by tulipix-sec-music — rebuild when it
    // changes (cargo does not track include_str! targets across crates).
    println!("cargo:rerun-if-changed=../../resources/podcast-feeds.txt");
    // Nothing links libmpv any more. The embedded player was the only caller of
    // the render API; playback runs as an external mpv process found on PATH, so
    // there is no mpv.lib / mpv-2.dll / TULIPIX_MPV_LIB_DIR to arrange at build
    // time on any platform.
}
