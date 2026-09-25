// The Kitchen section's state, and cook mode's timers.
//
// The bridge keeps the session — which tab, which recipe, how many servings,
// which step — so this holds the last snapshot and the notice it left, and
// the one thing the bridge does not: timers counting down, which live where
// the second hand is.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart' show AnyhowException;

import '../../design/tokens.dart';
import '../../src/rust/api/kitchen.dart';

typedef KitchenTab = ({String id, String label});

/// The header's tabs, in the order it draws them. Also Settings → Sections'
/// list of what can be switched off.
const List<KitchenTab> kitchenTabs = [
  (id: 'recipes', label: 'Recipes'),
  (id: 'recipe', label: 'Recipe'),
  (id: 'cook', label: 'Cook'),
  (id: 'plan', label: 'Plan'),
  (id: 'shop', label: 'Shopping'),
  (id: 'pantry', label: 'Pantry'),
];

const Color kKitchen = Tokens.secKitchen;
const Color kKitchen2 = Tokens.secKitchen2;

/// One countdown in cook mode, keyed by recipe, step and position so it keeps
/// running while you move between steps.
class CookTimer {
  CookTimer(this.label, this.total) : left = total;

  final String label;
  final int total;
  int left;
  bool running = false;

  bool get done => left <= 0;
  double get fraction => total == 0 ? 1 : 1 - left / total;
}

class KitchenController extends ChangeNotifier {
  KitchenState? state;
  Object? error;
  bool busy = false;

  String notice = '';
  Timer? _noticeTimer;

  final Map<String, CookTimer> timers = {};
  Timer? _tick;

  Future<KitchenState?> send(KitchenCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      final st = await kitchenDispatch(cmd: cmd);
      state = st;
      if (st.notice.isNotEmpty) say(st.notice);
      if (!st.cooking) _dropTimers();
      return st;
    } catch (e) {
      error = e;
      return null;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  Future<void> refresh() => send(const KitchenCmd.refresh());

  /// A snapshot without the progress line, for polling: the phone's ticks.
  Future<void> quiet() async {
    if (busy) return;
    try {
      state = await kitchenDispatch(cmd: const KitchenCmd.refresh());
      notifyListeners();
    } catch (_) {
      // The next poll tries again; a strip every five seconds would not help.
    }
  }

  /// Run a command whose failure belongs in a dialog rather than the strip;
  /// answers the reason, or null when it worked.
  Future<String?> attempt(KitchenCmd cmd) async {
    await send(cmd);
    final e = error;
    if (e == null) return null;
    error = null;
    notifyListeners();
    return plainError(e);
  }

  CookTimer timerFor(int recipe, int step, int i, TimerSpec spec) =>
      timers.putIfAbsent(
          '$recipe:$step:$i', () => CookTimer(spec.label, spec.secs));

  void toggle(CookTimer t) {
    if (t.done) {
      t.left = t.total;
      t.running = true;
    } else {
      t.running = !t.running;
    }
    _tick ??= Timer.periodic(const Duration(seconds: 1), (_) => _step());
    notifyListeners();
  }

  void _step() {
    var any = false;
    for (final t in timers.values) {
      if (!t.running) continue;
      t.left--;
      if (t.left <= 0) {
        t.left = 0;
        t.running = false;
        say('${t.label} is done');
        SystemSound.play(SystemSoundType.alert);
      } else {
        any = true;
      }
    }
    if (!any) {
      _tick?.cancel();
      _tick = null;
    }
    notifyListeners();
  }

  void _dropTimers() {
    if (timers.isEmpty) return;
    timers.clear();
    _tick?.cancel();
    _tick = null;
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
    _tick?.cancel();
    super.dispose();
  }
}

// ------------------------------------------------------------ formatting ---

/// A bridge error as its message, without the type wrapped around it.
String plainError(Object e) => (e is AnyhowException ? e.message : '$e')
    .split('\n\nStack backtrace')
    .first
    .trim();

/// "6:12"; "1:05:00" past the hour.
String countdown(int secs) {
  final h = secs ~/ 3600, m = (secs % 3600) ~/ 60, s = secs % 60;
  final ss = s.toString().padLeft(2, '0');
  return h > 0 ? '$h:${m.toString().padLeft(2, '0')}:$ss' : '$m:$ss';
}

/// "8 min", "1 h 20 min", "45 s".
String span(int secs) {
  if (secs < 60) return '$secs s';
  final h = secs ~/ 3600, m = (secs % 3600) ~/ 60;
  if (h == 0) return '$m min';
  return m == 0 ? '$h h' : '$h h $m min';
}

String plural(int n, String one, [String? many]) =>
    n == 1 ? '1 $one' : '$n ${many ?? '${one}s'}';

/// A recipe's placeholder colours, fixed by its id so a card keeps them.
const List<(Color, Color)> _plates = [
  (Color(0xFF7F1D1D), Color(0xFFDC2626)),
  (Color(0xFF1E3A8A), Color(0xFFF97316)),
  (Color(0xFF7C2D12), Color(0xFFFB923C)),
  (Color(0xFF14532D), Color(0xFF65A30D)),
  (Color(0xFF451A03), Color(0xFFB45309)),
  (Color(0xFF3F3F46), Color(0xFFA8A29E)),
  (Color(0xFF713F12), Color(0xFFFACC15)),
];

(Color, Color) plateColours(int seed) => _plates[seed.abs() % _plates.length];
