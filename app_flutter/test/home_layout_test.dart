// Layout regression for the four Home layouts.
//
// Every widget exercised here is pure: nothing calls the bridge, so this runs
// without the native library. What it catches is the class of bug that only
// shows at run time — a Column asking for more height than it was given, a
// Wrap in a Row spilling out of the header — which is otherwise found by a
// person looking at a red-and-yellow screen.
//
// The four pages are budget layouts: every band's height is derived from the
// window, so the cruel sizes below are the point. A 1280 x 720 window is where
// the Classic rail and the Welcome page budget break first.

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'dart:typed_data' show Float64List;
// frb generates its own Int64List (a BigInt-strict wrapper), not the one in
// dart:typed_data — the feed's thumb ids travel in that.
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart' show Int64List;

import 'package:tulipix/design/tokens.dart';
import 'package:tulipix/sections/home/home_controller.dart';
import 'package:tulipix/sections/home/home_cinema.dart';
import 'package:tulipix/sections/home/home_classic.dart';
import 'package:tulipix/sections/home/home_stream.dart';
import 'package:tulipix/sections/home/home_welcome.dart';
import 'package:tulipix/sections/home/home_shared.dart';
import 'package:tulipix/src/rust/api/home.dart';

HomeState _state({
  int continueRows = 4,
  int tiles = 10,
  int remotes = 3,
  int events = 12,
  String spent = '1240.50',
  List<String>? cards,
}) {
  HomeTile tile(int i) => HomeTile(
        id: i,
        label: 'a-rather-long-file-name-$i.mkv',
        sub: '1h 42m',
      );
  return HomeState(
    greeting: 'Good afternoon, Alexandra',
    dateLine: 'Sunday, 30 August 2026',
    libraryLine: '12,304 items · 41 GB',
    quote: const HomeQuote(
      text: 'The best code is the code never written, and the second best is '
          'the code somebody else already wrote.',
      author: 'Nobody in particular',
    ),
    // The shelf the hero walks a minute at a time.
    quotes: const [
      HomeQuote(
        text: 'The best code is the code never written, and the second best is '
            'the code somebody else already wrote.',
        author: 'Nobody in particular',
      ),
      HomeQuote(text: 'Simplicity is a feature.', author: 'Also nobody'),
    ],
    counts: const HomeCounts(
      photos: 12304,
      photosAlbums: 41,
      videos: 812,
      videosShows: 19,
      songs: 9421,
      podcasts: 37,
      audiobooks: 12,
      radio: 220,
      books: 341,
      booksReading: 4,
      cloudRemotes: 3,
      toolsRunning: 1,
      toolsQueued: 2,
      financesDue: 3,
    ),
    continueFilter: 'all',
    continueRows: [
      for (var i = 0; i < continueRows; i++)
        HomeContinue(
          kind: const ['book', 'video', 'podcast', 'audiobook'][0 + i % 4],
          title: 'A Title That Is Deliberately Far Too Long To Fit $i',
          author: 'Some Author With A Long Name',
          sub: '2h 14m left',
          frac: i.isEven ? 0.42 : -1,
          id: i,
          path: '/tmp/x$i',
        ),
    ],
    recentPhotos: [for (var i = 0; i < tiles; i++) tile(i)],
    recentVideos: [for (var i = 0; i < tiles; i++) tile(i)],
    recentBooks: [for (var i = 0; i < tiles; i++) tile(i)],
    remotes: [
      for (var i = 0; i < remotes; i++)
        HomeRemote(
          name: 'remote-$i',
          backend: const ['drive', 'dropbox', 's3'][i % 3],
          usage: '11 GB / 15 GB',
        ),
    ],
    recentSongs: [for (var i = 0; i < tiles; i++) tile(i)],
    hero: const HomeHero(
      kind: 'video',
      title: 'The Hero',
      kicker: 'CONTINUE WATCHING',
      meta: '41m left',
      frac: 0.6,
      id: 1,
      path: '',
    ),
    layout: 'classic',
    cards: cards ??
        const [
          'hero',
          'continue',
          'player',
          'quick',
          'photos',
          'videos',
          'music',
          'books',
          'cloud',
          'tools',
          'transfer',
          'finances',
        ],
    feedFilter: 'all',
    feedNote: 'Nothing today — showing the last three days',
    events: [
      for (var i = 0; i < events; i++)
        HomeEvent(
          at: '19:4${i % 10}',
          day: '11-08-26',
          group: const ['NOW', 'TODAY', 'YESTERDAY', 'EARLIER'][i % 4],
          head: i % 4 == 0,
          section: const [
            'photos',
            'videos',
            'music',
            'books',
            'cloud',
            'tools',
            'transfer',
            'finances',
            'library',
          ][i % 9],
          kind: 'added',
          title: '34 photos added from a folder with a very long name',
          sub: 'Somewhere deep inside ~/Pictures/2026/August/holiday',
          action: i.isEven ? 'Mark paid' : '',
          alarm: i % 5 == 0,
          id: i,
          path: '/tmp/e$i',
          // A folded run keeps up to three; every third row here is one.
          ids: Int64List.fromList(
              i % 3 == 0 ? [i, i + 1, i + 2] : [i]),
        ),
    ],
    toolCount: 23,
    transferInbox: '~/Downloads',
    finMonth: 'August',
    finSpent: spent,
    finMonths: Float64List.fromList(
        [0.1, 0.3, 0.2, 0.9, 0.4, 0.5, 1.0, 0.7, 0.3, 0.6, 0.8, 0.5]),
    finMonthSpends: const [
      '410.00',
      '980.10',
      '655.40',
      '2910.00',
      '1290.75',
      '1610.20',
      '3200.00',
      '2240.90',
      '980.00',
      '1930.60',
      '2580.30',
      '1240.50',
    ],
    finMonthLabels: const [
      'September',
      'October',
      'November',
      'December',
      'January',
      'February',
      'March',
      'April',
      'May',
      'June',
      'July',
      'August',
    ],
    finDues: const [
      HomeDue(name: 'Rent', amount: '1200.00', due: '01-09-26', late_: false),
      HomeDue(
          name: 'Electricity', amount: '84.20', due: '28-08-26', late_: true),
    ],
    busy: false,
  );
}

/// A page under a fixed window, with the tokens the widgets read.
Widget _host(Widget child, {bool dark = true}) => MaterialApp(
      theme: tulipixTheme(dark ? Tokens.dark() : Tokens.light()),
      home: Scaffold(body: child),
    );

Future<void> _at(WidgetTester tester, Size size, Widget page) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(_host(page));
  await tester.pump();
}

/// Take the tree down so the pages' own clocks are disposed.
///
/// Three of the four keep them — the emoji, the quote, the Recently Added loop,
/// the hero slideshow — and the binding checks for pending timers as soon as
/// the test body returns, which is before any `addTearDown` runs.
Future<void> _unmount(WidgetTester tester) =>
    tester.pumpWidget(const SizedBox());

void main() {
  // The two shapes that matter: a laptop, and the smallest window anyone
  // actually runs this in.
  const sizes = [Size(1920, 1080), Size(1440, 900), Size(1280, 720)];

  group('Continue', () {
    for (final size in sizes) {
      testWidgets('the strip fits at ${size.width}x${size.height}',
          (tester) async {
        await _at(
          tester,
          size,
          Center(
            child: SizedBox(
              // Classic's own budget: the strip is 250 tall and the left column
              // is what is left after the music rail and its gutter.
              width: size.width - 32 - 355 - 30,
              height: 250,
              child: ContinueStrip(
                controller: _NullController(),
                state: _state(),
              ),
            ),
          ),
        );
        expect(tester.takeException(), isNull);
      });
    }

    testWidgets('an empty strip says so rather than drawing four holes',
        (tester) async {
      await _at(
        tester,
        const Size(1440, 900),
        Center(
          child: SizedBox(
            width: 900,
            height: 250,
            child: ContinueStrip(
              controller: _NullController(),
              state: _state(continueRows: 0),
            ),
          ),
        ),
      );
      expect(find.textContaining('Nothing in progress'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  });

  group('Hub tiles', () {
    testWidgets('eight across a Welcome shelf, at its shortest',
        (tester) async {
      final cards = homeHubCards(_state());
      // `hubH` bottoms out at 156, and the disc takes what the two text lines
      // leave — the band is where a tall tile overflows first.
      await _at(
        tester,
        const Size(1280, 720),
        Center(
          child: SizedBox(
            width: 1280 - 52,
            height: 156,
            child: Padding(
              padding: const EdgeInsets.all(14),
              child: Row(
                children: [
                  for (final c in cards)
                    Expanded(
                      child: HubTile(
                        name: c.name,
                        count: c.count,
                        icon: c.icon,
                        accent: c.accent,
                        disc: (156 - 28 - 74).clamp(28.0, 62.0).toDouble(),
                        onTap: () {},
                      ),
                    ),
                ],
              ),
            ),
          ),
        ),
      );
      expect(find.text('Photos'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });

    testWidgets('one alone in Cinema\'s 300px column', (tester) async {
      final card = homeHubCards(_state()).first;
      await _at(
        tester,
        const Size(1440, 900),
        Center(
          child: SizedBox(
            width: 300 - 52,
            height: 152,
            child: HubTile(
              name: card.name,
              count: card.count,
              icon: card.icon,
              accent: card.accent,
              disc: 56,
              textScale: 1.35,
              washScale: 1.4,
              onTap: () {},
            ),
          ),
        ),
      );
      expect(tester.takeException(), isNull);
    });
  });

  group('Progress pill', () {
    testWidgets('an unknown fraction hides the bar rather than drawing zero',
        (tester) async {
      await _at(
        tester,
        const Size(1440, 900),
        const Center(
          child: SizedBox(
            width: 172,
            child: ProgressPill(
                sub: 'Never opened', frac: -1, accent: Tokens.secBooks),
          ),
        ),
      );
      expect(find.byType(LinearProgressIndicator), findsNothing);
      expect(tester.takeException(), isNull);
    });

    testWidgets('a known one draws it', (tester) async {
      await _at(
        tester,
        const Size(1440, 900),
        const Center(
          child: SizedBox(
            width: 172,
            child: ProgressPill(
                sub: '2h 14m left', frac: 0.42, accent: Tokens.secBooks),
          ),
        ),
      );
      expect(find.byType(LinearProgressIndicator), findsOneWidget);
      expect(find.text('42%'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  });

  group('Stream · month spend', () {
    test('the newest month is the figure the bridge sent', () {
      final st = _state();
      expect(monthSpend(st, 11), '1240.50');
    });

    test('every other month falls out of the peak the newest one implies', () {
      // months[11] is 0.5 and is worth 1240.50, so the peak is 2481.00 and
      // months[6] at 1.0 is the whole of it.
      final st = _state();
      expect(monthSpend(st, 6), '2481.00');
      expect(monthSpend(st, 0), '248.10');
    });

    test('a figure that will not parse gives no answer, not a wrong one', () {
      expect(monthSpend(_state(spent: ''), 6), isNull);
      expect(monthSpend(_state(spent: 'n/a'), 6), isNull);
    });

    test('out of range is null', () {
      expect(monthSpend(_state(), -1), isNull);
      expect(monthSpend(_state(), 99), isNull);
    });
  });

  group('Classic cards', () {
    // The media row and the shelf row each take half of what the header and the
    // Continue strip leave — the shallowest band is where they break.
    const bands = [(1920.0, 300.0), (1440.0, 240.0), (1280.0, 170.0)];

    for (final (w, h) in bands) {
      testWidgets('the coverflow cards fit a ${h}px band', (tester) async {
        await _at(
          tester,
          Size(w, 1080),
          Center(
            child: SizedBox(
              width: w - 32 - 355 - 30,
              height: h,
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Expanded(
                    child: PhotoSlideshow(
                      icon: Icons.image_outlined,
                      accent: Tokens.secPhotos,
                      name: 'Photos',
                      stat: '12,304 items · 41 albums',
                      tiles: _state().recentPhotos,
                      section: Section.photos,
                      emptyIcon: Icons.image_not_supported_outlined,
                      topCrop: true,
                    ),
                  ),
                  const SizedBox(width: 10),
                  Expanded(
                    child: PhotoSlideshow(
                      icon: Icons.movie_outlined,
                      accent: Tokens.secVideos,
                      name: 'Videos',
                      stat: '812 items · 19 shows',
                      tiles: _state().recentVideos,
                      section: Section.videos,
                      emptyIcon: Icons.videocam_off_outlined,
                      showPlay: true,
                      tileAspect: 1.6,
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
        expect(tester.takeException(), isNull);
      });

      testWidgets('the Books / Cloud / Tools row fits a ${h}px band',
          (tester) async {
        final st = _state();
        await _at(
          tester,
          Size(w, 1080),
          Center(
            child: SizedBox(
              width: w - 32 - 355 - 30,
              height: h,
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Expanded(
                    child: BookRow(
                      total: 341,
                      tiles: st.recentBooks,
                      stat: '341 books · 4 reading',
                    ),
                  ),
                  const SizedBox(width: 10),
                  Expanded(child: CloudRow(remotes: st.remotes, total: 5)),
                  const SizedBox(width: 10),
                  const Expanded(child: ToolRow(total: 23)),
                ],
              ),
            ),
          ),
        );
        expect(tester.takeException(), isNull);
      });
    }

    testWidgets('an empty library draws its off-glyph, not a hole',
        (tester) async {
      final st = _state(tiles: 0, remotes: 0);
      await _at(
        tester,
        const Size(1440, 900),
        Center(
          child: SizedBox(
            width: 900,
            height: 240,
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Expanded(
                  child: PhotoSlideshow(
                    icon: Icons.image_outlined,
                    accent: Tokens.secPhotos,
                    name: 'Photos',
                    stat: '0 items',
                    tiles: st.recentPhotos,
                    section: Section.photos,
                    emptyIcon: Icons.image_not_supported_outlined,
                  ),
                ),
                const SizedBox(width: 10),
                Expanded(child: CloudRow(remotes: st.remotes, total: 0)),
              ],
            ),
          ),
        ),
      );
      expect(find.byIcon(Icons.image_not_supported_outlined), findsOneWidget);
      expect(find.byIcon(Icons.cloud_off_outlined), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  });

  group('Stream', () {
    // Everything but the player, which is the one block that needs the native
    // library to say what is playing.
    const noPlayer = [
      'hero',
      'continue',
      'quick',
      'photos',
      'videos',
      'music',
      'books',
      'cloud',
      'tools',
      'transfer',
      'finances',
      'library',
    ];

    for (final size in sizes) {
      testWidgets('the page fits at ${size.width}x${size.height}',
          (tester) async {
        await _at(
          tester,
          size,
          StreamHome(
            controller: _NullController(),
            state: _state(cards: noPlayer),
          ),
        );
        await tester.pump();
        expect(tester.takeException(), isNull);
        await _unmount(tester);
      });
    }

    testWidgets('an empty feed says what would fill it', (tester) async {
      await _at(
        tester,
        const Size(1440, 900),
        StreamHome(
          controller: _NullController(),
          state: _state(events: 0, cards: noPlayer),
        ),
      );
      expect(find.text('Nothing recorded yet'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  });

  // The other three pages draw the app-wide player, so they touch
  // `MusicController.instance`. Its constructor degrades rather than throwing
  // when the bridge is not up, which is what lets these run here: the player
  // draws its idle state and the rest of the page is the real thing.
  group('Classic', () {
    for (final size in sizes) {
      testWidgets('the bento fits at ${size.width}x${size.height}',
          (tester) async {
        await _at(
          tester,
          size,
          ClassicHome(controller: _NullController(), state: _state()),
        );
        await tester.pump();
        expect(tester.takeException(), isNull);
        await _unmount(tester);
      });
    }

    testWidgets('an empty library still lays out', (tester) async {
      await _at(
        tester,
        const Size(1440, 900),
        ClassicHome(
          controller: _NullController(),
          state: _state(continueRows: 0, tiles: 0, remotes: 0),
        ),
      );
      await tester.pump();
      expect(tester.takeException(), isNull);
      await _unmount(tester);
    });
  });

  group('Welcome', () {
    for (final size in sizes) {
      testWidgets('the page budget balances at ${size.width}x${size.height}',
          (tester) async {
        await _at(
          tester,
          size,
          WelcomeHome(controller: _NullController(), state: _state()),
        );
        await tester.pump();
        expect(tester.takeException(), isNull);
        await _unmount(tester);
      });
    }

    testWidgets(
        'the wells keep Continue four rows tall when nothing is in '
        'progress', (tester) async {
      await _at(
        tester,
        const Size(1440, 900),
        WelcomeHome(
          controller: _NullController(),
          state: _state(continueRows: 0),
        ),
      );
      await tester.pump();
      expect(find.textContaining('Nothing half-finished yet'), findsOneWidget);
      expect(tester.takeException(), isNull);
      await _unmount(tester);
    });
  });

  group('Cinema', () {
    for (final size in sizes) {
      testWidgets('the shelf and the stack fit at ${size.width}x${size.height}',
          (tester) async {
        await _at(
          tester,
          size,
          CinemaHome(controller: _NullController(), state: _state()),
        );
        await tester.pump();
        expect(tester.takeException(), isNull);
        await _unmount(tester);
      });
    }

    testWidgets('nothing in progress falls back to the snapshot hero',
        (tester) async {
      await _at(
        tester,
        const Size(1440, 900),
        CinemaHome(
          controller: _NullController(),
          state: _state(continueRows: 0),
        ),
      );
      await tester.pump();
      expect(find.text('The Hero'), findsOneWidget);
      expect(tester.takeException(), isNull);
      await _unmount(tester);
    });
  });
}

/// The Continue strip only ever dispatches; nothing in these tests presses a
/// chip, so the controller never has to reach the bridge.
class _NullController extends HomeController {}
