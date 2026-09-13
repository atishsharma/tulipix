//! The Dart-facing surface. Three to four exported symbols per section.

pub mod books;
pub mod chat;
pub mod cloud;
pub mod dialog;
pub mod editor;
pub mod finances;
pub mod genesis;
pub mod home;
pub mod lock;
pub mod mdl;
pub mod music;
pub mod photos;
pub mod scrobble;
pub mod settings;
pub mod shell;
pub mod status;
pub mod tools;
pub mod transfer;
pub mod videos;

/// Runs once, before any other bridge call, from `RustLib.init()` on the Dart
/// side. Wires Rust panics and `println!` through to the Dart console.
#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();

    // The domain crates diagnose themselves through `tracing` and say useful
    // things -- which inbox could not be written to, which upload could not be
    // finalised, whether the server fell back to plain HTTP. The Slint binary
    // installs a subscriber in `main()` (crates/tulipix-app/src/main.rs); a
    // cdylib has no `main`, so until this was here every one of those went
    // nowhere and the port looked mute where the shipping build is talkative.
    //
    // `try_init` because a global default can only be set once and a hot
    // restart re-enters this: failing to install a second logger is not worth
    // taking the app down for.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    // yt-dlp goes stale on its own schedule: sites change, and a binary a few
    // weeks old starts answering 403 on downloads that worked yesterday. This
    // is the weekly check — background thread, at most one network call a week,
    // and every failure is a log line rather than something in the user's way.
    tulipix_core::updater::spawn_ytdlp_update();
}
