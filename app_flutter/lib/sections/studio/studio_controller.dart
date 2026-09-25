// The Studio section's state.
//
// The bridge keeps the session — tab and the project open — and renders in the
// background, so this holds the last snapshot and asks again while something
// renders, so the progress moves.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/studio.dart';

export '../kitchen/kitchen_controller.dart' show plainError, plural;

typedef StudioTab = ({String id, String label});

/// The header's tabs, in the order it draws them. Also Settings → Sections'
/// list of what can be switched off.
const List<StudioTab> studioTabs = [
  (id: 'create', label: 'Create'),
  (id: 'projects', label: 'Projects'),
];

const Color kStudio = Tokens.secStudio;
const Color kStudio2 = Tokens.secStudio2;

class StudioController extends ChangeNotifier {
  StudioState? state;
  Object? error;
  bool busy = false;

  String notice = '';
  Timer? _noticeTimer;
  Timer? _again;

  Future<StudioState?> send(StudioCmd cmd, {bool quiet = false}) async {
    if (!quiet) {
      busy = true;
      error = null;
      notifyListeners();
    }
    try {
      final st = await studioDispatch(cmd: cmd);
      state = st;
      if (st.notice.isNotEmpty) say(st.notice);
      _again?.cancel();
      if (st.busy) {
        _again = Timer(
            const Duration(milliseconds: 1500), () => refresh(quiet: true));
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
      send(const StudioCmd.refresh(), quiet: quiet);

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
