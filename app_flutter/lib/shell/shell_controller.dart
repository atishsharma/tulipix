// The shell's own state: who you are, which section is up, and the two live
// figures the sidebar carries.
//
// A singleton, like MusicController: the sidebar is drawn beside every page and
// the Settings page writes the same profile it reads, so one owner beats
// handing the same object down two trees.

import 'dart:async';

import 'package:flutter/material.dart';

import '../design/tokens.dart';

import '../src/rust/api/shell.dart';
import '../src/rust/api/status.dart';

/// How often the badge and the health lamp are re-counted. The Slint build runs
/// the same slow tick for exactly these two figures.
const Duration kShellTick = Duration(seconds: 30);

class ShellController extends ChangeNotifier {
  static final ShellController instance = ShellController._();

  ShellController._() {
    _tick = Timer.periodic(kShellTick, (_) => refresh());
  }

  ShellState? state;
  Section section = Section.home;
  bool collapsed = true;

  /// App health, worst-of across every section. Its own poll, slower than the
  /// Status page's — the lamp needs to be right within half a minute, not
  /// within two seconds.
  String statusLevel = 'unknown';
  String statusNote = 'Starting…';

  Timer? _tick;

  @override
  void dispose() {
    _tick?.cancel();
    super.dispose();
  }

  Future<void> send(ShellCmd cmd) async {
    try {
      state = await shellDispatch(cmd: cmd);
      notifyListeners();
    } catch (_) {
      // The sidebar has a sensible empty state and a failed count is not worth
      // a banner over the whole app.
    }
  }

  Future<void> refresh() async {
    await send(const ShellCmd.refresh());
    await _lamp();
  }

  Future<void> _lamp() async {
    try {
      final st = await statusDispatch(cmd: const StatusCmd.refresh());
      statusLevel = st.level;
      statusNote = st.headline;
      notifyListeners();
    } catch (_) {
      // Leave the lamp where it was rather than reporting "unknown" for a
      // transient database lock.
    }
  }

  void go(Section s) {
    if (section == s) return;
    section = s;
    notifyListeners();
  }

  /// light → dark → extra-dark → light, and stored.
  ///
  /// Two buttons cycle the app theme — the one in the sidebar's footer dock and
  /// the one at the top right of the zen player — and they are the same three
  /// stops in the same order, so the rule lives here rather than in whichever
  /// widget was written first.
  void cycleTheme() {
    final theme = state?.theme ?? 'system';
    final dark = theme != 'light';
    final oled = theme == 'extra-dark';
    send(ShellCmd.setTheme(
      theme: !dark
          ? 'dark'
          : !oled
              ? 'extra-dark'
              : 'light',
    ));
  }

  /// `light` | `dark` | `extra-dark`, whatever the shell last stored.
  String get theme => state?.theme ?? 'system';

  void toggleCollapsed() {
    collapsed = !collapsed;
    notifyListeners();
  }
}
