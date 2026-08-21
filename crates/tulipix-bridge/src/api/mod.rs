//! The Dart-facing surface. Three to four exported symbols per section.

pub mod photos;

/// Runs once, before any other bridge call, from `RustLib.init()` on the Dart
/// side. Wires Rust panics and `println!` through to the Dart console.
#[flutter_rust_bridge::frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}
