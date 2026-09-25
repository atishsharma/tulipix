// The desktop widget's geometry and its state machine.
//
// Nothing here calls the bridge: `MusicController`'s constructor guards its own
// startup so it can be touched without the native library.
//
// What this pins is the half most likely to drift silently — the numbers. Every
// one of them is a constant in crates/tulipix-music/src/mini_player.rs, and a
// port whose pill is 40px narrower than the Slint one looks fine on its own and
// wrong beside it.
//
// It also pins the SEPARATION. The mini player (a 300x470 card inside the app,
// opened from the player) and the mini widget (three size classes that take the
// window, opened from the title bar) are two objects with two sets of state.
// One shared flag is what made pressing the caption button turn an open card
// into a widget.

import 'dart:ui' show Size;

import 'package:flutter_test/flutter_test.dart';
import 'package:tulipix/sections/music/mini_player.dart' show kMiniSize;
import 'package:tulipix/sections/music/mini_widget.dart';
import 'package:tulipix/sections/music/music_controller.dart';

void main() {
  // The controller's constructor reaches for the audio deck and the event
  // stream. Both are guarded, but they touch the binding on the way to failing.
  TestWidgetsFlutterBinding.ensureInitialized();

  final c = MusicController.instance;

  setUp(() {
    // The controller is a singleton, so each test puts it back.
    c.widgetStyle = MiniStyle.bar;
    c.widgetScale = 1.0;
    c.widgetOpen = false;
    c.miniOpen = false;
    c.miniBubble = false;
    c.miniScale = 1.0;
    c.pillOpen = false;
    c.pillLyrics = false;
  });

  test('base sizes match MiniStyle::base_size in mini_player.rs', () {
    expect(MiniStyle.bar.base.width, 441);
    expect(MiniStyle.bar.base.height, 212);
    expect(MiniStyle.square.base.width, 280);
    expect(MiniStyle.square.base.height, 496);
    expect(MiniStyle.pill.base.width, 300);
    expect(MiniStyle.pill.base.height, 80);
  });

  test('there is no card style — the card is a different object', () {
    expect(MiniStyle.values.length, 3);
    // `music-mini-w` / `music-mini-h` in ui/main.slint, and it lives with the
    // card rather than in this enum.
    expect(kMiniSize, const Size(300, 470));
  });

  test('the pill extras are PILL_CLUSTER and PILL_LYRICS', () {
    expect(kPillCluster, 148);
    expect(kPillLyrics, 32);
  });

  test('cycle runs bar -> square -> pill -> bar', () {
    expect(c.widgetStyle, MiniStyle.bar);
    c.cycleWidgetStyle();
    expect(c.widgetStyle, MiniStyle.square);
    c.cycleWidgetStyle();
    expect(c.widgetStyle, MiniStyle.pill);
    c.cycleWidgetStyle();
    expect(c.widgetStyle, MiniStyle.bar);
  });

  test('the window is the base size times the widget scale', () {
    expect(c.widgetWindow.width, 441);
    expect(c.widgetWindow.height, 212);
    c.scaleWidget(1.2); // DEFAULT_SCALE — what the widget opens at
    expect(c.widgetWindow.width, closeTo(529.2, 0.01));
    expect(c.widgetWindow.height, closeTo(254.4, 0.01));
  });

  test('the open pill is 148 wider, and only the pill is', () {
    c.widgetStyle = MiniStyle.pill;
    expect(c.widgetWindow.width, 300);
    c.togglePillOpen();
    expect(c.widgetWindow.width, 448);
    // The extra is added at the current scale but is not part of the ratio —
    // `resize_locked_with_extra`, which is why the cluster does not stretch.
    c.scaleWidget(2.0);
    expect(c.widgetWindow.width, 896);
    expect(c.widgetWindow.height, 160);

    // A bar carrying the pill's extras would just be a bar 148px too wide.
    c.widgetStyle = MiniStyle.bar;
    expect(c.widgetWindow.width, 882);
  });

  test('the lyrics row needs lyrics to exist', () {
    c.widgetStyle = MiniStyle.pill;
    c.pillLyrics = true;
    // No snapshot in this test, so no words — and a row that reveals an empty
    // box must not cost the window 32px.
    expect(c.hasLyrics, isFalse);
    expect(c.widgetWindow.height, 80);
  });

  test('changing style drops the pill extras', () {
    c.widgetStyle = MiniStyle.pill;
    c.pillOpen = true;
    c.pillLyrics = true;
    c.cycleWidgetStyle();
    expect(c.pillOpen, isFalse);
    expect(c.pillLyrics, isFalse);
    expect(c.widgetWindow.width, MiniStyle.bar.base.width);
  });

  test('the card floors at 1.0 and the widget at 0.7', () {
    c.scaleMini(0.5);
    expect(c.miniScale, 1.0);
    c.scaleMini(9);
    expect(c.miniScale, 3.0);

    c.scaleWidget(0.5);
    expect(c.widgetScale, 0.7); // MIN_SCALE
    c.scaleWidget(9);
    expect(c.widgetScale, 3.0); // MAX_SCALE
  });

  test('the two scales are two numbers', () {
    c.scaleWidget(0.7);
    c.scaleMini(2.0);
    expect(c.widgetScale, 0.7, reason: 'the card must not move the widget');
    expect(c.miniWindow.width, 600);
    expect(c.widgetWindow.width, closeTo(308.7, 0.01));
  });

  test('opening one does not open the other', () {
    c.toggleWidget();
    expect(c.widgetOpen, isTrue);
    expect(c.miniOpen, isFalse, reason: 'the widget is not the mini player');

    c.toggleMini();
    expect(c.miniOpen, isTrue);
    expect(c.widgetOpen, isTrue,
        reason: 'and the mini player is not it either');

    c.closeWidget();
    expect(c.widgetOpen, isFalse);
    expect(c.miniOpen, isTrue);
  });

  test('zen stands both of them down', () {
    c.toggleWidget();
    c.toggleMini();
    c.openZen();
    expect(c.widgetOpen, isFalse);
    expect(c.miniOpen, isFalse);
    c.closeZen();
  });
}
