// The shell's own state: who you are, which section is up, and the two live
// figures the sidebar carries.
//
// A singleton, like MusicController: the sidebar is drawn beside every page and
// the Settings page writes the same profile it reads, so one owner beats
// handing the same object down two trees.

import 'dart:async';

import 'package:flutter/material.dart';

import '../design/design_language.dart';
import '../design/tokens.dart';

import '../src/rust/api/shell.dart';
import '../src/rust/api/status.dart';
import 'section_tabs.dart';
import 'window.dart';

/// How often the badge and the health lamp are re-counted. The Slint build runs
/// the same slow tick for exactly these two figures.
const Duration kShellTick = Duration(seconds: 30);

/// The saved id list as sections, in order.
///
/// Null or empty means the first snapshot has not landed: everything, because a
/// shell that starts short and fills in would drop every page's State the
/// moment it did. Unknown ids are dropped — a settings file from a build with a
/// section this one does not have must not put a hole in the stack.
List<Section> sectionsFrom(List<String>? ids) {
  if (ids == null || ids.isEmpty) return Section.values;
  final out = [
    for (final id in ids)
      if (ShellController.byId[id] != null) ShellController.byId[id]!
  ];
  // Settings is the way back and is never off, but a settings file written by
  // a build that did not know that must not be able to strand anyone.
  if (!out.contains(Section.settings)) out.add(Section.settings);
  return out;
}

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

  /// Bumped when Save rewrites the profile photo or cover. The files keep
  /// their names, so anything drawing them keys on this to load them again.
  int pictureEpoch = 0;

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
      // Not latched like the section list: a tab row can change under a live
      // page without costing anybody their scroll position.
      applyTabsOff(state?.tabsOff ?? const []);
      _takeLanding();
      _keepOnScreen();
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
    // A launcher pointing at a section that is not in the sidebar: showing it
    // would put the shell on a page it never built. Nothing happens instead.
    if (!sections.contains(s)) return;
    if (section == s) return;
    section = s;
    notifyListeners();
  }

  /// Section ids → the enum, for the list the bridge sends.
  static const Map<String, Section> byId = {
    'home': Section.home,
    'photos': Section.photos,
    'videos': Section.videos,
    'music': Section.music,
    'books': Section.books,
    'cloud': Section.cloud,
    'tools': Section.tools,
    'transfer': Section.transfer,
    'finances': Section.finances,
    'feeds': Section.feeds,
    'journal': Section.journal,
    'kitchen': Section.kitchen,
    'papers': Section.papers,
    'settings': Section.settings,
  };

  /// The sections in the sidebar, in the saved order, live.
  ///
  /// This was latched for the session, because the shell's IndexedStack is
  /// built from it and Flutter matches a multi-child list by position: a list
  /// that changed length mid-session re-matched everything after the change and
  /// threw away its State — the page you were on, its scroll, its controllers.
  /// The fix was a key per child (see `main.dart`), not a latch: keyed, the
  /// survivors are recognised as the same child that moved. So the panel takes
  /// effect where you can see it, and a section switched back on is simply
  /// built then.
  List<Section> get sections => sectionsFrom(state?.sections);

  /// Where the shell's IndexedStack should be. -1 is impossible by
  /// construction — [go] is only reachable from what [sections] drew — but a
  /// deep link into a hidden section would be, so it falls back rather than
  /// throwing during layout.
  int get stackIndex {
    final i = sections.indexOf(section);
    return i < 0 ? 0 : i;
  }

  /// Open on whatever Settings → Sections chose, once, when the first snapshot
  /// arrives. Before that there is nothing to obey.
  bool _landed = false;
  void _takeLanding() {
    if (_landed) return;
    final ids = state?.sections;
    if (ids == null || ids.isEmpty) return;
    _landed = true;
    final s = byId[state?.landing ?? ''];
    if (s != null && sections.contains(s) && section != s) section = s;
  }

  /// Nobody is left standing on a section that has just been hidden.
  ///
  /// `stackIndex` would fall back to 0, which draws the first page while the
  /// sidebar highlights nothing — the shell quietly showing one thing and
  /// saying another.
  void _keepOnScreen() {
    final list = sections;
    if (list.isEmpty || list.contains(section)) return;
    final land = byId[state?.landing ?? ''];
    section = land != null && list.contains(land) ? land : list.first;
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
    if (!sections.contains(s)) return;
    go(s);
    _openers[s]?.call(tab);
  }

  /// Go to [s] and ask it to open one THING — a book by id, a remote by name,
  /// a tool category. Home's cards all hold an id and a kind; what to do with
  /// them belongs to the section's own controller, which Home cannot reach
  /// (every page owns its own, and none is a singleton), so it travels as a
  /// verb and a payload through the same registry the tab links use.
  void goOpen(Section s, String verb, [String arg = '']) =>
      goTab(s, arg.isEmpty ? verb : '$verb$kOpenSep$arg');

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

  /// Which material the Music section is drawn in, as last stored. Standard
  /// until the first snapshot lands, and for any name this build does not know.
  DesignLanguage get designLanguage =>
      DesignLanguage.fromId(state?.designLanguage);

  /// The desktop's accent while "Follow system accent" is on; null when off,
  /// or when the desktop reports none.
  Color? get systemAccent {
    final hex = state?.systemAccent ?? '';
    if (hex.length != 7 || !hex.startsWith('#')) return null;
    final v = int.tryParse(hex.substring(1), radix: 16);
    return v == null ? null : Color(0xFF000000 | v);
  }

  /// "Honour OS font scale". On until the first snapshot says otherwise.
  bool get followOsFontScale => state?.followOsFontScale ?? true;

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

/// The separator an opener argument is split on. One nul, because a path may
/// contain anything else — the grammar `tools_page.dart` already carried.
const String kOpenSep = '\u0000';

/// The two halves of an opener argument, for the section on the receiving end.
/// A bare tab name ("livetv", "genesis") comes back as the verb, with no arg.
({String verb, String arg}) openArg(String raw) {
  final cut = raw.indexOf(kOpenSep);
  return cut < 0
      ? (verb: raw, arg: '')
      : (verb: raw.substring(0, cut), arg: raw.substring(cut + 1));
}
