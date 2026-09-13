//! Native file and folder dialogs, opened the way the Slint build opens them:
//! `rfd` over the desktop portal. On KDE that is KDE's own picker (previews,
//! Places, split view); Flutter's `file_selector` went through GTK's chooser,
//! which only reaches the portal inside a sandbox, so it showed GTK's plainer
//! dialog instead.
//!
//! Plain blocking functions on purpose: frb runs a non-async function on its
//! worker pool, so the dialog waits there and Dart just awaits a Future. An
//! empty string means "not given" for every text argument; cancelling returns
//! None or an empty list.

use std::path::PathBuf;

use rfd::FileDialog;

fn dialog(title: &str, initial: &str, label: &str, extensions: &[String]) -> FileDialog {
    let mut d = FileDialog::new();
    if !title.is_empty() {
        d = d.set_title(title);
    }
    if !initial.is_empty() {
        d = d.set_directory(initial);
    }
    if !extensions.is_empty() {
        d = d.add_filter(if label.is_empty() { "Files" } else { label }, extensions);
    }
    d
}

fn text(p: PathBuf) -> String {
    p.to_string_lossy().into_owned()
}

/// One folder.
pub fn dialog_pick_folder(title: String, initial: String) -> Option<String> {
    dialog(&title, &initial, "", &[]).pick_folder().map(text)
}

/// One file, optionally narrowed to extensions given without the dot.
pub fn dialog_pick_file(
    title: String,
    initial: String,
    label: String,
    extensions: Vec<String>,
) -> Option<String> {
    dialog(&title, &initial, &label, &extensions).pick_file().map(text)
}

/// Several files at once.
pub fn dialog_pick_files(
    title: String,
    initial: String,
    label: String,
    extensions: Vec<String>,
) -> Vec<String> {
    dialog(&title, &initial, &label, &extensions)
        .pick_files()
        .unwrap_or_default()
        .into_iter()
        .map(text)
        .collect()
}

/// Where to write a file that does not exist yet; `file_name` is the
/// suggestion the dialog opens with.
pub fn dialog_save_file(
    title: String,
    file_name: String,
    label: String,
    extensions: Vec<String>,
) -> Option<String> {
    let mut d = dialog(&title, "", &label, &extensions);
    if !file_name.is_empty() {
        d = d.set_file_name(file_name);
    }
    d.save_file().map(text)
}
