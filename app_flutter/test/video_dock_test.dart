// The docked video player's geometry, which is the only part of the layer that
// can be checked without libmpv: constructing a `Player` needs the native
// library, so the stage itself is not testable here. What is testable is the
// rectangle it gets handed, and that ending playback forgets the dock.

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:tulipix/playback/video_layer.dart';

void main() {
  const window = Size(1920, 1080);

  test('undocked fills the window', () {
    expect(videoStageRect(window, false), Offset.zero & window);
  });

  test('docked is a 16:9 card in the bottom-right, on the gutter', () {
    final r = videoStageRect(window, true);
    expect(r.width, kVideoDockW);
    expect(r.height, closeTo(kVideoDockW * 9 / 16, 0.001));
    expect(window.width - r.right, kVideoDockGap);
    expect(window.height - r.bottom, kVideoDockGap);
  });

  test('a window too narrow for the card shrinks it rather than clipping it',
      () {
    const narrow = Size(320, 240);
    final r = videoStageRect(narrow, true);
    expect(r.width, narrow.width - kVideoDockGap * 2);
    expect(r.left, greaterThanOrEqualTo(0));
    expect(r.right, lessThanOrEqualTo(narrow.width));
    expect(r.bottom, lessThanOrEqualTo(narrow.height));
  });

  test('a window narrower than the gutters still yields a drawable rect', () {
    final r = videoStageRect(const Size(20, 20), true);
    expect(r.width, greaterThanOrEqualTo(0));
    expect(r.height, greaterThanOrEqualTo(0));
    expect(r.left, greaterThanOrEqualTo(0));
    expect(r.top, greaterThanOrEqualTo(0));
  });

  test('pinned left sits on the left gutter', () {
    final r =
        videoStageRect(window, true, const VideoDockSpot(right: false, y: 0));
    expect(r.left, kVideoDockGap);
    expect(r.top, kVideoDockGap);
  });

  test('y is a fraction of the free travel, not of the window', () {
    final r =
        videoStageRect(window, true, const VideoDockSpot(right: true, y: 0.5));
    final travel = window.height - r.height - kVideoDockGap * 2;
    expect(r.top, closeTo(kVideoDockGap + travel / 2, 0.001));
    // Half way down the travel is not half way down the window.
    expect(r.top, isNot(closeTo(window.height / 2, 0.001)));
  });

  test('the side is decided by the card centre, not its left edge', () {
    // Top-left is in the left half; the card's own middle is not.
    final w = dockCardWidth(window);
    final at = Offset(window.width / 2 - w * 0.4, 100);
    expect(at.dx, lessThan(window.width / 2));
    expect(dockSpotFor(at, window).right, isTrue);
  });

  test('a drop outside the gutters clamps rather than parking off-screen', () {
    expect(dockSpotFor(const Offset(0, -900), window).y, 0);
    expect(dockSpotFor(const Offset(0, 9000), window).y, 1);
  });

  test('a rect round-trips back to the spot that made it', () {
    for (final spot in const [
      VideoDockSpot(right: true, y: 1),
      VideoDockSpot(right: false, y: 0),
      VideoDockSpot(right: false, y: 0.25),
    ]) {
      final back =
          dockSpotFor(videoStageRect(window, true, spot).topLeft, window);
      expect(back.right, spot.right);
      expect(back.y, closeTo(spot.y, 0.001));
    }
  });

  test('a window with no room to move gives a finite spot, not a NaN', () {
    const squat = Size(400, 40);
    final spot = dockSpotFor(const Offset(10, 10), squat);
    expect(spot.y, 0);
    expect(videoStageRect(squat, true, spot).top.isFinite, isTrue);
  });

  test('clearing playback forgets the dock, so the next film opens full', () {
    videoRequest.value = const VideoRequest(
      token: 7,
      src: '/tmp/film.mkv',
      startAt: 0,
      props: [],
    );
    videoDocked.value = true;
    videoDockDrag.value = const Offset(40, 40);

    clearVideo();

    expect(videoRequest.value, isNull);
    expect(videoDocked.value, isFalse);
    expect(videoDockDrag.value, isNull);
  });

  testWidgets('the layer draws only its child until something plays',
      (tester) async {
    await tester.pumpWidget(
      const MaterialApp(home: VideoLayer(child: Text('app'))),
    );
    expect(find.text('app'), findsOneWidget);
    expect(find.byType(AnimatedPositioned), findsNothing);
  });
}
