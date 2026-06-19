//! The generated Slint UI for Tulipix — `MainWindow` plus every exported
//! struct/global (`ToolField`, `QueueJob`, `ToolStatus`, `CloudEntry`, …).
//!
//! `slint::include_modules!()` emits all of these as public items at this
//! crate's root, so downstream crates use them as `tulipix_ui::MainWindow`,
//! `tulipix_ui::ToolField`, etc. Splitting the codegen here keeps the binary
//! crate's rustc unit small.

slint::include_modules!();
