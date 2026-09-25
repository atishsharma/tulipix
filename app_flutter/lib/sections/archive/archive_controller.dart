// The Archive section's state.
//
// The bridge keeps the session — tab, kind, search, the zip open — and runs
// scans and checksums in the background, so this holds the last snapshot and
// asks again while one runs, so its count moves.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/archive.dart';

export '../kitchen/kitchen_controller.dart' show plainError, plural;

typedef ArchiveTab = ({String id, String label});

/// The header's tabs, in the order it draws them. Also Settings → Sections'
/// list of what can be switched off.
const List<ArchiveTab> archiveTabs = [
  (id: 'library', label: 'Library'),
  (id: 'dupes', label: 'Duplicates'),
  (id: 'health', label: 'Health'),
];

const Color kArchive = Tokens.secArchive;
const Color kArchive2 = Tokens.secArchive2;

class ArchiveController extends ChangeNotifier {
  ArchiveState? state;
  Object? error;
  bool busy = false;

  String notice = '';
  Timer? _noticeTimer;
  Timer? _again;

  Future<ArchiveState?> send(ArchiveCmd cmd, {bool quiet = false}) async {
    if (!quiet) {
      busy = true;
      error = null;
      notifyListeners();
    }
    try {
      final st = await archiveDispatch(cmd: cmd);
      state = st;
      if (st.notice.isNotEmpty) say(st.notice);
      _again?.cancel();
      // A job just queued has not reported yet: look again soon either way
      // after a command, then keep looking while one runs.
      if (st.job != null || !quiet) {
        _again = Timer(const Duration(milliseconds: 1200),
            () => refresh(quiet: true));
      }
      return st;
    } catch (e) {
      error = e;
      return null;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  Future<void> refresh({bool quiet = false}) =>
      send(const ArchiveCmd.refresh(), quiet: quiet);

  void pause() => _again?.cancel();

  void say(String text) {
    notice = text;
    _noticeTimer?.cancel();
    _noticeTimer = Timer(const Duration(seconds: 6), dismissNotice);
    notifyListeners();
  }

  void dismissNotice() {
    notice = '';
    notifyListeners();
  }

  void clearError() {
    error = null;
    notifyListeners();
  }

  @override
  void dispose() {
    _noticeTimer?.cancel();
    _again?.cancel();
    super.dispose();
  }
}
