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
import 'window.dart';

/// How often the badge and the health lamp are re-counted. The Slint build runs
/// the same slow tick for exactly these two figures.
const Duration kShellTick = Duration(seconds: 30);

class ShellController extends ChangeNotifier {
  static final ShellController instance = ShellController._();

  ShellController._();

  ShellState? state;
  Section section = Section.home;
  bool collapsed = true;

  /// App health, worst-of across every section. Its own poll, slower than the
  /// Status page's — the lamp needs to be right within half a minute, not
  /// within two seconds.
  String statusLevel = 'unknown';
  String statusNote = 'Starting…';

  Timer? _tick;

  /// The poll runs while something is drawing the badge and the lamp, and not
  /// otherwise. It used to start in the constructor — but this is a singleton
  /// that is constructed the moment any page reaches for it, so a headless
  /// context (a layout test, say) inherited a 30-second timer nothing could
  /// ever cancel.
  @override
  void addListener(VoidCallback listener) {
    super.addListener(listener);
    _tick ??= Timer.periodic(kShellTick, (_) => refresh());
  }

  @override
  void removeListener(VoidCallback listener) {
    super.removeListener(listener);
    if (!hasListeners) {
      _tick?.cancel();
      _tick = null;
    }
  }

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

  /// How each section answers a deep link — "open Videos *on Live TV*".
  ///
  /// Home's six launchers are doors past a section's front page, and the tab
  /// they want lives in that section's own controller, which the launcher
  /// cannot reach. Pages register here in `initState`; every page is built at
  /// launch (the shell is an IndexedStack), so by the time anything is
  /// clickable they all have.
  final Map<Section, void Function(String)> _openers = {};

  void onOpen(Section s, void Function(String) f) => _openers[s] = f;

  /// Go to [s] and ask it to open [tab]. Fires the opener even when the
  /// section is already up: "Live TV" from Videos still means Live TV.
  void goTab(Section s, String tab) {
    go(s);
    _openers[s]?.call(tab);
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

  // --- logo-fullscreen -------------------------------------------------------
  //
  // `app-fullscreen` in ui/main.slint: the sidebar's mark takes the app
  // borderless-fullscreen, the caption row goes away with the frame, and a 3px
  // strip at the top peeks a minimal bar back with the way out on it. One
  // control opens it; the strip's bar and the same mark both close it.

  bool appFullscreen = false;

  void toggleAppFullscreen() => setAppFullscreen(!appFullscreen);

  void setAppFullscreen(bool on) {
    if (appFullscreen == on) return;
    appFullscreen = on;
    setWindowFullscreen(on);
    notifyListeners();
  }
}
