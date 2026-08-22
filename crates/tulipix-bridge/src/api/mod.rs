//! The Dart-facing surface. Three to four exported symbols per section.

pub mod editor;
pub mod music;
pub mod photos;
pub mod transfer;

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
}
