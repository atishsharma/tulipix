// Native file and folder pickers.
//
// Every "Add folder", every import and every export in this app used to be a
// text field you typed an absolute path into — accepted differences #2, #18,
// #21 and #24, all of them the same difference. The shell was supposed to own
// dialogs and the bridge's three `rfd` calls were the only ones that existed.
//
// `file_selector` is maintained by the Flutter team and does this natively on
// every platform, so the dialogs belong here now and `rfd` — with its
// xdg-portal backend — is out of the bridge entirely. What the bridge receives
// is a path, which is what it wanted in the first place.
//
// Cancelling returns null everywhere. None of these throw: a picker the desktop
// portal refuses to open should leave the button looking unpressed, not put a
// stack trace on screen.

import 'package:file_selector/file_selector.dart';
import 'package:flutter/foundation.dart' show debugPrint;

/// One folder.
Future<String?> pickDirectory({String? title, String? initial}) async {
  try {
    return await getDirectoryPath(
        confirmButtonText: 'Choose', initialDirectory: initial);
  } catch (e) {
    debugPrint('picker: $e');
    return null;
  }
}

/// One file, optionally narrowed to a set of extensions.
///
/// Extensions are given without the dot, which is what `XTypeGroup` wants and
/// also what the Rust side's filter lists already hold.
Future<String?> pickFile({
  required String label,
  List<String> extensions = const [],
  String? initial,
}) async {
  try {
    final file = await openFile(
      initialDirectory: initial,
      acceptedTypeGroups: [
        if (extensions.isNotEmpty)
          XTypeGroup(label: label, extensions: extensions),
      ],
    );
    return file?.path;
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
    final files = await openFiles(
      initialDirectory: initial,
      acceptedTypeGroups: [
        if (extensions.isNotEmpty)
          XTypeGroup(label: label, extensions: extensions),
      ],
    );
    return [for (final f in files) f.path];
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
    final location = await getSaveLocation(
      suggestedName: suggestedName,
      acceptedTypeGroups: [
        if (extensions.isNotEmpty)
          XTypeGroup(label: label, extensions: extensions),
      ],
    );
    return location?.path;
  } catch (e) {
    debugPrint('picker: $e');
    return null;
  }
}
