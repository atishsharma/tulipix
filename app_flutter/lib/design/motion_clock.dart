// One beat for everything on screen that moves by itself.
//
// Each of these used to keep its own time: the visualizer (and a bouncing
// title, since dropped) on 90 ms timers, the seek pill on a timer of its own, the
// scrolling title, the Expressive wave, the bubble and the breathing rings on
// Tickers at the display's rate. Uncoordinated, each asked for frames at its
// own moments -- on My Music the gaps between frames ran anywhere from 15 to
// 117 ms -- and on this renderer every one of those frames draws the whole
// window. On one clock, whatever moves in a beat moves in the same frame, and
// frames arrive on the beat.
//
// The beat is not a frame rate. A frame follows a beat only when something on
// it changed what it draws: a subscriber that snaps to whole device pixels and
// has not crossed one asks for nothing. The clock runs only while something is
// subscribed, and nothing subscribes while the deck is paused or its widget is
// behind another section.

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/widgets.dart';

class MotionClock {
  MotionClock._();

  static final MotionClock instance = MotionClock._();

  /// Thirty a second: the most any subscriber can move, and often enough that
  /// a pixel-at-a-time move reads as motion rather than as steps.
  static const Duration beat = Duration(microseconds: 33333);

  final Stopwatch _elapsed = Stopwatch()..start();
  final Set<VoidCallback> _subscribers = {};
  final Set<VoidCallback> _once = {};
  Timer? _timer;
  int _count = 0;

  /// Seconds on the clock, for shapes that are a function of time.
  double get seconds => _elapsed.elapsedMicroseconds / 1e6;

  /// Beats so far, for a subscriber that moves on every nth.
  int get count => _count;

  bool get running => _timer != null;

  void join(VoidCallback onBeat) {
    _subscribers.add(onBeat);
    _timer ??= Timer.periodic(beat, _tick);
  }

  void leave(VoidCallback onBeat) {
    _subscribers.remove(onBeat);
    if (_subscribers.isNotEmpty) return;
    _timer?.cancel();
    _timer = null;
    // Not now: a leave is often a widget stopping mid-build, and what waits
    // here is a notify that would set state under it.
    if (_once.isNotEmpty) scheduleMicrotask(_flushOnce);
  }

  /// Runs [f] on the next beat, so a change that arrives between beats -- the
  /// once-a-second playback tick -- lands in the frame the motion is already
  /// drawing rather than in one of its own. At once when nothing moves.
  void onNextBeat(VoidCallback f) {
    if (_timer == null) {
      f();
    } else {
      _once.add(f);
    }
  }

  void _tick(Timer _) {
    _count++;
    for (final f in _subscribers.toList()) {
      f();
    }
    _flushOnce();
  }

  void _flushOnce() {
    if (_once.isEmpty) return;
    final run = _once.toList();
    _once.clear();
    for (final f in run) {
      f();
    }
  }
}

/// [v] logical pixels, rounded to the nearest device pixel. What a subscriber
/// compares against its last value: equal means nothing visible moved, and a
/// [ValueNotifier] given an equal value notifies nobody.
double snapToPixel(double v, double dpr) => (v * dpr).roundToDouble() / dpr;

/// 0 up to 1 and back over [period], on the beat, while [sync] says so; 0 at
/// rest. The breathing rings on Home and a busy device's chip in Transfer — a
/// repeating AnimationController did the same at the display's rate.
class Breath extends ValueNotifier<double> {
  Breath({this.period = const Duration(milliseconds: 2480)}) : super(0);

  final Duration period;
  bool _on = false;

  void sync(bool on) {
    if (on == _on) return;
    _on = on;
    if (on) {
      MotionClock.instance.join(_beat);
    } else {
      MotionClock.instance.leave(_beat);
      value = 0;
    }
  }

  void _beat() {
    final half = period.inMicroseconds / 2e6;
    final p = MotionClock.instance.seconds % (2 * half) / half;
    value = p <= 1 ? p : 2 - p;
  }

  @override
  void dispose() {
    if (_on) MotionClock.instance.leave(_beat);
    super.dispose();
  }
}

/// Paints [child] moved by [shift], and does nothing else when it changes.
///
/// `Transform.translate` draws the same thing, but a new offset also re-walks
/// semantics and repaints up to the nearest boundary — for the title in the
/// music header, the whole page, around 580 render objects a beat. This is a boundary
/// of its own and repaints alone. Hit testing follows the shift; the semantics
/// rectangle keeps the one from the last full update, a few pixels out at most.
class ShiftedPaint extends SingleChildRenderObjectWidget {
  const ShiftedPaint({super.key, required this.shift, super.child});

  final ValueListenable<Offset> shift;

  @override
  RenderObject createRenderObject(BuildContext context) =>
      _RenderShiftedPaint(shift);

  @override
  void updateRenderObject(BuildContext context, RenderObject renderObject) =>
      (renderObject as _RenderShiftedPaint).shift = shift;
}

class _RenderShiftedPaint extends RenderProxyBox {
  _RenderShiftedPaint(this._shift);

  ValueListenable<Offset> _shift;
  set shift(ValueListenable<Offset> v) {
    if (identical(v, _shift)) return;
    if (attached) {
      _shift.removeListener(markNeedsPaint);
      v.addListener(markNeedsPaint);
    }
    _shift = v;
    markNeedsPaint();
  }

  @override
  bool get isRepaintBoundary => true;

  @override
  void attach(PipelineOwner owner) {
    super.attach(owner);
    _shift.addListener(markNeedsPaint);
  }

  @override
  void detach() {
    _shift.removeListener(markNeedsPaint);
    super.detach();
  }

  @override
  void paint(PaintingContext context, Offset offset) {
    final c = child;
    if (c != null) context.paintChild(c, offset + _shift.value);
  }

  @override
  bool hitTestChildren(BoxHitTestResult result, {required Offset position}) {
    final c = child;
    if (c == null) return false;
    return result.addWithPaintOffset(
      offset: _shift.value,
      position: position,
      hitTest: (result, at) => c.hitTest(result, position: at),
    );
  }

  @override
  void applyPaintTransform(RenderBox child, Matrix4 transform) =>
      transform.translateByDouble(_shift.value.dx, _shift.value.dy, 0, 1);
}
