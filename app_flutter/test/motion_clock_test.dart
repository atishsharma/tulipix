// The shared beat: whatever moves steps on it together, it runs only while
// something is on it, and a painted shift moves its child and nothing else.

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:tulipix/design/motion_clock.dart';

class _CountingPainter extends CustomPainter {
  int paints = 0;

  @override
  void paint(Canvas canvas, Size size) => paints++;

  @override
  bool shouldRepaint(_CountingPainter old) => false;
}

void main() {
  final clock = MotionClock.instance;

  testWidgets('everything on the clock moves on the same beat', (tester) async {
    final seen = <String>[];
    void a() => seen.add('a');
    void b() => seen.add('b');
    clock.join(a);
    clock.join(b);
    await tester.pump(MotionClock.beat);
    // One timer, both movers, one frame's worth of change.
    expect(seen, ['a', 'b']);
    clock.leave(a);
    expect(clock.running, isTrue);
    clock.leave(b);
    expect(clock.running, isFalse);
  });

  testWidgets('a change between beats waits for the next one', (tester) async {
    var notified = 0;
    void notify() => notified++;
    // Nothing moving: at once.
    clock.onNextBeat(notify);
    expect(notified, 1);
    void mover() {}
    clock.join(mover);
    clock.onNextBeat(notify);
    clock.onNextBeat(notify);
    expect(notified, 1);
    await tester.pump(MotionClock.beat);
    // On the beat, and twice between beats is still once.
    expect(notified, 2);
    clock.leave(mover);
  });

  testWidgets('leaving with a change waiting runs it after, not during',
      (tester) async {
    // The last mover often leaves mid-build, and what waits is a notify.
    var notified = 0;
    void notify() => notified++;
    void mover() {}
    clock.join(mover);
    clock.onNextBeat(notify);
    clock.leave(mover);
    expect(notified, 0);
    await tester.pump();
    expect(notified, 1);
  });

  testWidgets('a painted shift moves its child and repaints nothing else',
      (tester) async {
    // Transform.translate here would repaint the sibling on every step: both
    // sit under one boundary, and that is the whole music page in the app.
    final shift = ValueNotifier<Offset>(Offset.zero);
    addTearDown(shift.dispose);
    final sibling = _CountingPainter();
    await tester.pumpWidget(Directionality(
      textDirection: TextDirection.ltr,
      child: Column(children: [
        CustomPaint(painter: sibling, size: const Size(100, 20)),
        ShiftedPaint(shift: shift, child: const Text('Music')),
      ]),
    ));
    final before = tester.getTopLeft(find.text('Music'));
    final paints = sibling.paints;
    shift.value = const Offset(0, -5);
    await tester.pump();
    expect(tester.getTopLeft(find.text('Music')), before + const Offset(0, -5));
    expect(sibling.paints, paints);
  });

  test('a pixel is a device pixel', () {
    expect(snapToPixel(2.4, 1), 2);
    expect(snapToPixel(2.4, 2), 2.5);
  });
}
