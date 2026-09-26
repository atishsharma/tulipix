// Home — eight layouts over one snapshot.
//
// Which one draws is a setting (Settings › You & Home). They read the same
// `HomeState` and differ only in what they lead with:
//
//   classic — the quiet one: counts, Continue, shelves
//   welcome — a hello, the launchers, then everything offered as cards
//   cinema  — the newest in-progress item, full bleed, with Resume on it
//   stream  — one timeline of everything that happened, newest first
//   media   — watching, listening and reading on the first screen
//   play    — a couch dashboard of big tiles for the arrow keys
//   calm    — one suggestion first, the rest a breath away
//   today   — what needs you, what is half-done, a tile per section
//
// Media and Play are made for the Sections presets of the same name, and
// Settings › Sections offers them when one is applied; any can be picked.
//
// The snapshot is taken on arrival rather than on a clock: nothing here changes
// without something else in the app having caused it, and a landing page that
// re-queries ten databases on a timer is a battery graph.

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/tokens.dart';
import '../music/music_controller.dart';
import 'home_calm.dart';
import 'home_cinema.dart';
import 'home_classic.dart';
import 'home_controller.dart';
import 'home_media.dart';
import 'home_play.dart';
import 'home_stream.dart';
import 'home_today.dart';
import 'home_welcome.dart';

class HomePage extends StatefulWidget {
  const HomePage({super.key, required this.visible});

  /// The rail keeps every section alive, so Home is told when it is looked at.
  final bool visible;

  @override
  State<HomePage> createState() => _HomePageState();
}

class _HomePageState extends State<HomePage> {
  final HomeController _c = HomeController();

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (widget.visible) _c.refresh();
      // Every layout draws the app-wide player, so Home is the first thing
      // that needs a Music snapshot when Music itself has never been opened.
      if (MusicController.instance.state == null) {
        MusicController.instance.refresh();
      }
    });
  }

  @override
  void didUpdateWidget(HomePage old) {
    super.didUpdateWidget(old);
    // Coming back to Home is exactly when its figures are stale: something was
    // played, read or downloaded in the section you just left.
    if (widget.visible && !old.visible) _c.refresh();
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        final st = _c.state;
        if (st == null) {
          return ColoredBox(
            color: t.bg,
            child: FirstLoad(error: _c.error, onRetry: _c.refresh),
          );
        }
        return ColoredBox(
          color: t.bg,
          child: switch (st.layout) {
            'welcome' => WelcomeHome(controller: _c, state: st),
            'cinema' => CinemaHome(controller: _c, state: st),
            'stream' => StreamHome(controller: _c, state: st),
            'media' => MediaHome(controller: _c, state: st),
            'play' => PlayHome(controller: _c, state: st),
            'calm' => CalmHome(controller: _c, state: st),
            'today' => TodayHome(controller: _c, state: st),
            _ => ClassicHome(controller: _c, state: st),
          },
        );
      },
    );
  }
}
