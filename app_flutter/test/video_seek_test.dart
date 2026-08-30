// The seek bar's geometry and the arithmetic behind a held arrow key.
//
// Both are here because both went wrong in ways nothing would have reported:
// the played fill grew out of the middle of the bar in both directions, and a
// held key can only be got right by clamping at both ends of the file.

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:tulipix/playback/video_controls.dart';

void main() {
  group('the track fills from the left', () {
    // The real tree puts the track inside a `Center`, to sit it in the middle
    // of a taller hit lane. That hands it *loose* width — which is what the
    // bug was about, so a test that pumps it under a tight SizedBox proves
    // nothing.
    Future<void> pump(WidgetTester tester, double played, double buffered) {
      return tester.pumpWidget(
        Directionality(
          textDirection: TextDirection.ltr,
          child: Align(
            alignment: Alignment.topLeft,
            child: SizedBox(
              width: 200,
              height: 40,
              child: Center(
                child: SeekTrack(
                  height: 8,
                  played: played,
                  buffered: buffered,
                  accent: const Color(0xFFFF0000),
                  knob: false,
                ),
              ),
            ),
          ),
        ),
      );
    }

    testWidgets('the lane spans the full width whatever the progress',
        (tester) async {
      for (final played in const [0.0, 0.25, 0.5, 1.0]) {
        await pump(tester, played, played);
        final lane = tester.getRect(find.byType(SeekTrack));
        // Without an explicit width the container shrink-wrapped the Stack,
        // the Stack took the width of its widest child — the fill — and the
        // Center then centred the lot. That is a bar that grows out of the
        // middle in both directions as it plays.
        expect(lane.left, 0, reason: 'played=$played');
        expect(lane.width, 200, reason: 'played=$played');
      }
    });

    testWidgets('half played starts at the left edge, not the middle',
        (tester) async {
      await pump(tester, 0.5, 0.75);
      final fill = tester.getRect(find.byKey(const ValueKey('seek-played')));
      expect(fill.left, 0);
      expect(fill.width, closeTo(100, 0.01));
    });

    testWidgets('the buffer bar starts at the left edge too', (tester) async {
      await pump(tester, 0.5, 0.75);
      final buffer = tester.getRect(find.byKey(const ValueKey('seek-buffered')));
      expect(buffer.left, 0);
      expect(buffer.width, closeTo(150, 0.01));
    });

    testWidgets('nothing played paints nothing', (tester) async {
      await pump(tester, 0, 0);
      expect(tester.getRect(find.byKey(const ValueKey('seek-played'))).width, 0);
    });

    testWidgets('fully played fills the lane', (tester) async {
      await pump(tester, 1, 1);
      final fill = tester.getRect(find.byKey(const ValueKey('seek-played')));
      expect(fill.left, 0);
      expect(fill.width, closeTo(200, 0.01));
    });
  });

  group('seek target', () {
    const film = Duration(hours: 2);

    test('adds and subtracts', () {
      expect(seekTargetFor(const Duration(minutes: 10), 5, film),
          const Duration(minutes: 10, seconds: 5));
      expect(seekTargetFor(const Duration(minutes: 10), -10, film),
          const Duration(minutes: 9, seconds: 50));
    });

    test('never goes negative — mpv errors on that rather than seeking to 0', () {
      expect(seekTargetFor(const Duration(seconds: 3), -10, film), Duration.zero);
      expect(seekTargetFor(Duration.zero, -5, film), Duration.zero);
    });

    test('stops a second short of the end, so holding the key cannot end it', () {
      final at = seekTargetFor(const Duration(hours: 1, minutes: 59, seconds: 58), 10, film);
      expect(at, film - const Duration(seconds: 1));
      expect(at, lessThan(film));
    });

    test('an unknown duration still clamps the low end', () {
      expect(seekTargetFor(const Duration(seconds: 2), -10, Duration.zero),
          Duration.zero);
      // …and does not invent an upper one for a live stream.
      expect(seekTargetFor(const Duration(hours: 9), 10, Duration.zero),
          const Duration(hours: 9, seconds: 10));
    });

    test('a run of presses accumulates rather than repeating one jump', () {
      // What a held key does: each repeat counts from the last target, so a
      // second of holding covers a second of presses, not one jump repeated.
      var at = const Duration(minutes: 5);
      for (var i = 0; i < 6; i++) {
        at = seekTargetFor(at, 5, film);
      }
      expect(at, const Duration(minutes: 5, seconds: 30));
    });
  });
}
