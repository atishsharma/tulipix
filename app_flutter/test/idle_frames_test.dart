// Nothing on screen moves, so nothing may ask for a frame.
//
// A running Ticker requests a frame at every vsync whatever its callback does,
// and each of those frames renders the whole window — under a design language,
// every blurred shadow on it. `pumpAndSettle` only returns once no frame is
// scheduled, so each test here times out if its widget ticks at rest.
//
// Nothing calls the bridge: `MusicController`'s constructor guards its own
// startup, and with no snapshot the deck reads as idle.

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:tulipix/design/motion_clock.dart';
import 'package:tulipix/design/skin.dart';
import 'package:tulipix/design/tokens.dart';
import 'package:tulipix/sections/home/home_player.dart';
import 'package:tulipix/sections/music/mini_player.dart';
import 'package:tulipix/sections/music/music_controller.dart';
import 'package:tulipix/sections/music/player_widgets.dart';
import 'package:tulipix/src/rust/api/music.dart';

Widget _host(Widget child) => MaterialApp(
      theme: ThemeData(extensions: [Tokens.dark(), const StandardSkin()]),
      home: Scaffold(body: Center(child: child)),
    );

void main() {
  setUp(() => MusicController.instance.tickPlaying = false);

  testWidgets('the smooth seek pill never asks for a frame per vsync',
      (tester) async {
    // It used to run a Ticker while playing: a whole-window frame at the
    // display's rate, on My Music alone, to move a playhead two pixels a
    // second. Now it moves on the shared motion clock, a device pixel at a
    // time, and a frame follows a move, not the beat. The deck stands still
    // here (pos 0 is where it reads), so nothing moves and nothing may be
    // scheduled.
    Widget pill(bool playing) => _host(SizedBox(
          width: 400,
          child: SeekPill(
            pos: 0,
            dur: 100,
            smooth: true,
            playing: playing,
            onSeek: (_) {},
          ),
        ));
    await tester.pumpWidget(pill(false));
    await tester.pumpAndSettle();
    await tester.pumpWidget(pill(true));
    await tester.pumpAndSettle();
    await tester.pump(const Duration(seconds: 2));
    expect(tester.binding.hasScheduledFrame, isFalse);
    // Pausing takes it off the clock, and the clock stops with nothing on it.
    await tester.pumpWidget(pill(false));
    await tester.pumpAndSettle();
    expect(MotionClock.instance.running, isFalse);
  });

  testWidgets("Home's three players rest while nothing plays", (tester) async {
    tester.view.physicalSize = const Size(1920, 1080);
    tester.view.devicePixelRatio = 1.0;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(_host(Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        const SizedBox(
          width: 420,
          height: 640,
          child: CinemaPlayer(height: 640),
        ),
        SizedBox(
          width: 396,
          height: 900,
          child: ListView(children: const [StreamRailPlayer(railWidth: 396)]),
        ),
        const SizedBox(width: 900, child: WelcomePlayerBar(width: 900)),
      ],
    )));
    await tester.pumpAndSettle();
    expect(tester.takeException(), isNull);
  });

  testWidgets('the minimised mini rests while nothing plays', (tester) async {
    await tester.pumpWidget(
        _host(MiniBubble(controller: MusicController.instance)));
    await tester.pumpAndSettle();
  });

  testWidgets('a tick that only moves the position rebuilds only its readers',
      (tester) async {
    // Once a second while playing. It used to fire the whole controller, and
    // the Music page, Home and the shell all rebuilt to move one clock.
    final c = MusicController.instance;
    var all = 0, pos = 0;
    void onAll() => all++;
    void onPos() => pos++;
    c.addListener(onAll);
    c.ticks.addListener(onPos);
    addTearDown(() {
      c.removeListener(onAll);
      c.ticks.removeListener(onPos);
    });
    // Starting to play is news to everyone.
    c.debugEvent(const MusicEvent.tick(pos: 1, dur: 100, playing: true));
    expect((all, pos), (1, 0));
    // The next second is news only to the position.
    c.debugEvent(const MusicEvent.tick(pos: 2, dur: 100, playing: true));
    expect((all, pos), (1, 1));
    // Pausing is news to everyone again.
    c.debugEvent(const MusicEvent.tick(pos: 2, dur: 100, playing: false));
    expect((all, pos), (2, 1));
  });
}
