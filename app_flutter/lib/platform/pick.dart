// Native file and folder pickers.
//
// Every "Add folder", every import and every export in this app used to be a
// text field you typed an absolute path into — accepted differences #2, #18,
// #21 and #24, all of them the same difference. Every dialog goes through
// here, so the four functions below are the whole surface.
//
// They open through the bridge (api/dialog.rs): `rfd` over the desktop portal,
// exactly as the Slint build does. `file_selector` did this for a while, but
// on Linux it drives GTK's chooser, which only reaches the portal inside a
// sandbox — so on KDE it showed GTK's plain dialog, not KDE's picker with its
// previews and Places.
//
// Cancelling returns null everywhere. None of these throw: a picker the desktop
// portal refuses to open should leave the button looking unpressed, not put a
// stack trace on screen.
//
// Not in lib/design, where it started. `lib/design` is handed what it draws and
// reaches into nothing (design_layering_test enforces that), because every
// section imports skin.dart and a bridge import there hands the bridge to all
// of them. This file is a native shim, not a drawing file, and the sections
// that need a picker import it directly.

import 'package:flutter/foundation.dart' show debugPrint;

import '../src/rust/api/dialog.dart';

/// One folder.
Future<String?> pickDirectory({String? title, String? initial}) async {
  try {
    return await dialogPickFolder(title: title ?? '', initial: initial ?? '');
  } catch (e) {
    debugPrint('picker: $e');
    return null;
  }
}

/// One file, optionally narrowed to a set of extensions.
///
/// Extensions are given without the dot, which is also what the Rust side's
/// filter lists already hold.
Future<String?> pickFile({
  required String label,
  List<String> extensions = const [],
  String? initial,
}) async {
  try {
    return await dialogPickFile(
      title: '',
      initial: initial ?? '',
      label: label,
      extensions: extensions,
    );
  } catch (e) {
    debugPrint('picker: $e');
    return null;
  }
}

/// Several files at once. Empty when cancelled, never null, because every
/// caller is about to iterate it.
Future<List<String>> pickFiles({
  String label = 'Any file',
  List<String> extensions = const [],
  String? initial,
}) async {
  try {
    return await dialogPickFiles(
      title: '',
      initial: initial ?? '',
      label: label,
      extensions: extensions,
    );
  } catch (e) {
    debugPrint('picker: $e');
    return const [];
  }
}

/// Where to write something that does not exist yet.
///
/// The one case a folder picker cannot cover: an export names a file, and the
/// name is part of what the user is choosing.
Future<String?> pickSaveLocation({
  required String suggestedName,
  String label = 'File',
  List<String> extensions = const [],
}) async {
  try {
    return await dialogSaveFile(
      title: '',
      fileName: suggestedName,
      label: label,
      extensions: extensions,
    );
  } catch (e) {
    debugPrint('picker: $e');
    return null;
  }
}
