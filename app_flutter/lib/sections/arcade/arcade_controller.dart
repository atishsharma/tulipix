// The Arcade section's state.
//
// The bridge keeps the session — tab, filter, search, the game open — and
// reads the launchers, so this holds the last snapshot, the notice, and
// whether couch mode is up (a view, not worth a round trip).

import 'dart:async';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/arcade.dart';

export '../kitchen/kitchen_controller.dart' show plainError, plural;

typedef ArcadeTab = ({String id, String label});

/// The header's tabs, in the order it draws them. Also Settings → Sections'
/// list of what can be switched off.
const List<ArcadeTab> arcadeTabs = [
  (id: 'library', label: 'Library'),
  (id: 'stats', label: 'Stats'),
  (id: 'sources', label: 'Sources'),
];

const Color kArcade = Tokens.secArcade;
const Color kArcade2 = Tokens.secArcade2;

/// ProtonDB's tier colours, as the site draws them.
Color tierColour(String t) => switch (t) {
      'platinum' => const Color(0xFFB4C7DC),
      'gold' => const Color(0xFFCFB53B),
      'silver' => const Color(0xFFA6A6A6),
      'bronze' => const Color(0xFFCD7F32),
      'borked' => const Color(0xFFFF1919),
      'native' => kArcade,
      _ => Colors.grey,
    };

class ArcadeController extends ChangeNotifier {
  ArcadeState? state;
  Object? error;
  bool busy = false;
  bool couch = false;

  String notice = '';
  Timer? _noticeTimer;

  Future<ArcadeState?> send(ArcadeCmd cmd, {bool quiet = false}) async {
    if (!quiet) {
      busy = true;
      error = null;
      notifyListeners();
    }
    try {
      final st = await arcadeDispatch(cmd: cmd);
      state = st;
      if (st.notice.isNotEmpty) say(st.notice);
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
      send(const ArcadeCmd.refresh(), quiet: quiet);

  void setCouch(bool on) {
    couch = on;
    notifyListeners();
  }

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
    super.dispose();
  }
}
