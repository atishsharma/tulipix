// The Papers section's state.
//
// The bridge keeps the session — tab, filters, the paper open, whether the
// vault is open — so this holds the last snapshot and the notice it left. What
// it adds is asking again: while papers are still being read, and while the
// vault's five minutes run, so the grid fills in and the vault shuts on screen
// without a click.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/papers.dart';
import '../kitchen/kitchen_controller.dart' show plainError;

export '../kitchen/kitchen_controller.dart' show plainError, plural;

typedef PapersTab = ({String id, String label});

/// The header's tabs, in the order it draws them. Also Settings → Sections'
/// list of what can be switched off.
const List<PapersTab> papersTabs = [
  (id: 'inbox', label: 'Inbox'),
  (id: 'all', label: 'All papers'),
  (id: 'expiring', label: 'Expiring'),
  (id: 'vault', label: 'Vault'),
];

const Color kPapers = Tokens.secPapers;
const Color kPapers2 = Tokens.secPapers2;

/// Each kind's colour, as the deck draws its dots and page tops.
Color kindColour(String kind) => switch (kind) {
      'receipt' => const Color(0xFF14B8A6),
      'bill' => const Color(0xFFF59E0B),
      'warranty' => const Color(0xFF6366F1),
      'id' => const Color(0xFFEC4899),
      'contract' => const Color(0xFF8B5CF6),
      'medical' => const Color(0xFFEF4444),
      'tax' => const Color(0xFF0EA5E9),
      _ => const Color(0xFF64748B),
    };

/// What a date's ring is drawn in: red inside six weeks, amber inside three
/// months, the section's teal beyond.
Color ringColour(int days) => days <= 45
    ? const Color(0xFFEF4444)
    : days <= 92
        ? const Color(0xFFF59E0B)
        : kPapers;

class PapersController extends ChangeNotifier {
  PapersState? state;
  Object? error;
  bool busy = false;

  /// All papers as rows rather than a grid.
  bool list = false;

  String notice = '';
  Timer? _noticeTimer;
  Timer? _again;

  Future<PapersState?> send(PapersCmd cmd, {bool quiet = false}) async {
    if (!quiet) {
      busy = true;
      error = null;
      notifyListeners();
    }
    try {
      final st = await papersDispatch(cmd: cmd);
      state = st;
      if (st.notice.isNotEmpty) say(st.notice);
      _askAgain(st);
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
      send(const PapersCmd.refresh(), quiet: quiet);

  void _askAgain(PapersState st) {
    _again?.cancel();
    final wait = st.reading > 0
        ? const Duration(milliseconds: 1500)
        : st.hasPin && st.vaultOpen
            ? const Duration(seconds: 20)
            : null;
    if (wait != null) _again = Timer(wait, () => refresh(quiet: true));
  }

  /// Run a command whose failure belongs in a dialog rather than the strip;
  /// answers the reason, or null when it worked.
  Future<String?> attempt(PapersCmd cmd) async {
    await send(cmd);
    final e = error;
    if (e == null) return null;
    error = null;
    notifyListeners();
    return plainError(e);
  }

  String kindLabel(String id) {
    for (final k in state?.kinds ?? const <KindCount>[]) {
      if (k.id == id) return k.label;
    }
    return id == 'id' ? 'ID' : '${id[0].toUpperCase()}${id.substring(1)}';
  }

  void toggleList() {
    list = !list;
    notifyListeners();
  }

  /// A line under the header that goes away on its own.
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
