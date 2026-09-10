// The Music section's layout and its shared vocabulary.
//
// Music is the largest thing in the app — 28 files, 23k lines — and until now
// the only section with no test at all, while Books, Home, the reader,
// Transfer and the video player all had one. That asymmetry is the reason this
// exists: every fix landed here was landing blind.
//
// Nothing here calls the bridge. `MusicController`'s constructor guards its own
// startup, and every widget below takes plain values, so this runs without the
// native library.
//
// What it pins, in order of how quietly each would break:
//
//  - The Songs grid's column count. It snaps to the divisors of the page size,
//    and the whole reason it is not free is that a page of 32 laid out seven
//    across ends in a ragged row. A change to `colChoices` that broke that
//    would look completely fine until someone counted.
//  - The lyric timestamp format. It is written by Rust and echoed by Dart, and
//    the two spellings have to agree or a hand-timed file is timed wrongly.
//  - The empty states, which are the ones nobody looks at while developing.
//  - The accessibility labels, which have no visible symptom at all.

import 'dart:typed_data' show Float64List;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:tulipix/design/tokens.dart';
import 'package:tulipix/sections/music/lyrics_timer.dart' show fmtStamp;
import 'package:tulipix/sections/music/music_controller.dart';
import 'package:tulipix/sections/music/music_viz.dart' show visStyleNames;
import 'package:tulipix/sections/music/music_widgets.dart';
import 'package:tulipix/sections/music/my_music_tab.dart';
import 'package:tulipix/src/rust/api/music.dart';

final Float64List _noBands = Float64List(0);

const NowPlaying _nothingPlaying = NowPlaying(
  mode: 'music',
  itemId: 0,
  key: '',
  title: '',
  artist: '',
  album: '',
  art: '',
  playing: false,
  loaded: false,
  pos: 0,
  dur: 0,
  volume: 80,
  muted: false,
  loved: false,
  stars: 0,
  streamTitle: '',
);

/// A `MusicState` with nothing in it, and named overrides for the fields these
/// tests actually vary.
///
/// 191 fields, so it is generated rather than typed: every future music test
/// wants one of these, and the alternative is each of them building its own
/// two-hundred-line literal and getting a different set of empties.
///
/// The last six parameters are fields this session added to `MusicState`; they
/// are named here so the fixture is complete the moment `gen` catches up.
///
/// `int` rather than `PlatformInt64` throughout: the alias is `int` on every
/// target this runs on, and a fixture nobody can call without an import of
/// frb's internals is a fixture nobody calls.
MusicState _state({
  String view = '',
  String query = '',
  String status = '',
  NowPlaying now = _nothingPlaying,
  int sleepMin = 0,
  List<Track> queue = const [],
  List<LyricLine> lyrics = const [],
  String lyricsPlain = '',
  String eqPreset = '',
  Float64List? eqBands,
  bool eqOn = false,
  String libTab = '',
  int trackCount = 0,
  List<Track> songs = const [],
  int songTotal = 0,
  int songPage = 0,
  int songPages = 0,
  List<BrowseCard> cards = const [],
  int browsePages = 0,
  List<BrowseCard> roots = const [],
  List<BrowseCard> playlists = const [],
  bool mgrOpen = false,
  String mgrMode = '',
  String mgrTitle = '',
  bool mgrDirty = false,
  bool detailOpen = false,
  String podTab = '',
  bool podBusy = false,
  double podFrac = 0.0,
  String podStatus = '',
  int podDlId = 0,
  double podDlFrac = 0.0,
  String podDlTitle = '',
  String ytTab = '',
  List<YtVideo> ytChannelVideos = const [],
  bool ytBusy = false,
  int ytChannelPage = 0,
  bool ytChannelHasNext = false,
  int vizStyle = 5,
  bool vizOn = true,
  List<String> castDevices = const [],
  String castTarget = '',
  bool castActive = false,
  bool castBusy = false,
}) =>
    MusicState(
      vizStyle: vizStyle,
      vizOn: vizOn,
      castDevices: castDevices,
      castTarget: castTarget,
      castActive: castActive,
      castBusy: castBusy,
      view: view,
      query: query,
      status: status,
      now: now,
      shuffle: false,
      repeat: '',
      sleepMin: sleepMin,
      queue: queue,
      queueSuggested: 0,
      lyrics: lyrics,
      lyricsPlain: lyricsPlain,
      lyricsOffsetMs: 0,
      eqPreset: eqPreset,
      eqBands: eqBands ?? _noBands,
      eqOn: eqOn,
      devices: const [],
      device: '',
      gapless: false,
      crossfade: 0.0,
      replaygain: '',
      preampDb: 0.0,
      libTab: libTab,
      trackCount: trackCount,
      songs: songs,
      songTotal: songTotal,
      songPage: songPage,
      songPages: songPages,
      songSort: '',
      songDir: '',
      cards: cards,
      cardTotal: 0,
      browsePage: 0,
      browsePages: browsePages,
      browseSort: '',
      browseDir: '',
      stats: const Stats(week: '0m', total: '0m', genre: '', streak: ''),
      railRecent: const [],
      railMost: const [],
      railLoved: const [],
      railFresh: const [],
      railAlbums: const [],
      railArtists: const [],
      roots: roots,
      playlists: playlists,
      mgrOpen: mgrOpen,
      mgrTab: '',
      mgrMode: mgrMode,
      mgrFilter: '',
      mgrPage: 0,
      mgrPages: 0,
      mgrTotal: 0,
      mgrOk: 0,
      mgrMissing: 0,
      mgrRows: const [],
      mgrItemId: 0,
      mgrTitle: mgrTitle,
      mgrQName: '',
      mgrQArtist: '',
      mgrQAlbum: '',
      mgrSearching: false,
      mgrResults: const [],
      mgrViewRows: const [],
      mgrViewPlain: '',
      mgrDirty: mgrDirty,
      mgrStatus: '',
      mgrProgress: 0.0,
      mgrBusy: false,
      detailOpen: detailOpen,
      detailKind: '',
      detailId: 0,
      detailKey: '',
      detailTitle: '',
      detailSubtitle: '',
      detailArt: '',
      detailTracks: const [],
      detailAlbums: const [],
      detailLoved: false,
      detailStars: 0,
      detailArtistId: 0,
      detailInfo: '',
      detailFacts: const [],
      detailMbid: '',
      detailIsSmart: false,
      detailMeta: const [],
      detailShelves: const [],
      detailBars: const [],
      detailCollage: const [],
      detailNote: '',
      podTab: podTab,
      podShows: const [],
      podTotal: 0,
      podPage: 0,
      podPages: 0,
      podCategories: const [],
      podCat: '',
      podLatest: const [],
      podDownloads: const [],
      podDetailOpen: false,
      podDetail: null,
      podEpisodes: const [],
      podEpPage: 0,
      podEpPages: 0,
      podEpSort: '',
      podSpeed: 0.0,
      podQueue: const [],
      podHome: const [],
      podHomePage: 0,
      podHomePages: 0,
      podHomeSort: '',
      podDlSort: '',
      podTrends: const [],
      podTrendsLoading: false,
      podTrendsSort: '',
      podTrendsPage: 0,
      podTrendsPages: 0,
      podInfo: null,
      podTranscriptTitle: '',
      podTranscriptText: '',
      podBusy: podBusy,
      podFrac: podFrac,
      podStatus: podStatus,
      podQuery: '',
      podDlId: podDlId,
      podDlFrac: podDlFrac,
      podDlTitle: podDlTitle,
      bookTab: '',
      books: const [],
      bookDetailOpen: false,
      bookDetail: null,
      bookChapters: const [],
      bookBookmarks: const [],
      bookSpeed: 0.0,
      bookResumeIndex: 0,
      radioTab: '',
      radioCategories: const [],
      radioCatOpen: false,
      radioCatTitle: '',
      radioStations: const [],
      radioTotal: 0,
      radioPage: 0,
      radioPages: 0,
      radioSort: '',
      ytTab: ytTab,
      ytResults: const [],
      ytRecent: const [],
      ytCached: const [],
      ytDownloads: const [],
      ytDlPage: 0,
      ytDlPages: 0,
      ytDlTotal: 0,
      ytSubs: const [],
      ytSubCount: 0,
      ytSubsPage: 0,
      ytSubsPages: 0,
      ytPlaylists: const [],
      ytChannelOpen: false,
      ytChannelId: '',
      ytChannelTitle: '',
      ytChannelAvatar: '',
      ytChannelSubscribed: false,
      ytChannelMode: '',
      ytChannelVideos: ytChannelVideos,
      ytPlaylistOpen: false,
      ytPlaylistId: 0,
      ytPlaylistTitle: '',
      ytPlaylistVideos: const [],
      ytStatus: '',
      ytRecommended: const [],
      ytResultsMore: false,
      ytDlSort: '',
      ytSubsSort: '',
      ytSubsDir: '',
      ytSubsFilter: '',
      ytPlaylistSort: '',
      ytChannelSub: '',
      ytJobs: const [],
      ytBusy: ytBusy,
      ytDefaultRes: 0,
      ytHomeChannels: const [],
      ytHomeSubs: const [],
      ytFetcher: '',
      ytFetchBusy: false,
      ytFetchFrac: 0.0,
      ytFetchMsg: '',
      ytChannelQuery: '',
      ytChannelResults: const [],
      ytChannelPage: ytChannelPage,
      ytChannelHasNext: ytChannelHasNext,
      ytWatching: false,
      ytPlayingPlId: 0,
      ytHomeConnect: false,
    );

Widget _host(Widget child) => MaterialApp(
      theme: tulipixTheme(Tokens.dark()),
      home: Scaffold(body: child),
    );

Future<void> _at(WidgetTester tester, Size size, Widget child) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(_host(child));
  await tester.pump();
}

Track _track(int id, {String title = 'A song', int stars = 0, bool loved = false}) =>
    Track(
      itemId: id,
      title: title,
      artist: 'An artist',
      album: 'An album',
      durationS: 212,
      path: '/m/$id.flac',
      art: '',
      loved: loved,
      stars: stars,
      playCount: 0,
      trackNo: 0,
      discNo: 0,
      bpm: 0,
      musicKey: '',
      drScore: 0,
      year: 0,
      genre: '',
      lyrics: '',
      hasCredits: false,
    );

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  group('the Songs grid keeps whole pages', () {
    // SONGS_PAGE is 32. Four, eight and sixteen divide it; five, seven and
    // eleven do not, and a page laid out in one of those ends in a ragged row
    // that reads as the library running out rather than the page ending.
    const page = 32;

    for (final (width, expected) in const [
      (600.0, 4),
      (900.0, 8),
      (1400.0, 8),
      (2600.0, 16),
    ]) {
      testWidgets('$width wide lays out $expected across', (tester) async {
        var cols = 0;
        await _at(
          tester,
          Size(width, 900),
          MusicGrid(
            count: page,
            minCols: 4,
            maxCols: 16,
            colChoices: const [4, 8, 16],
            builder: (_, __) => const SizedBox.shrink(),
          ),
        );
        // Count the tiles in the first row by their laid-out width.
        final rows = tester.widgetList<Row>(find.byType(Row)).toList();
        cols = rows.isEmpty ? 0 : rows.first.children.length;
        expect(cols, expected);
        expect(page % cols, 0, reason: 'a page must fill whole rows');
        expect(tester.takeException(), isNull);
      });
    }

    testWidgets('a grid with no choices takes whatever fits', (tester) async {
      await _at(
        tester,
        const Size(1000, 800),
        MusicGrid(
          count: 12,
          target: 150,
          maxCols: 12,
          builder: (_, __) => const SizedBox.shrink(),
        ),
      );
      final rows = tester.widgetList<Row>(find.byType(Row)).toList();
      // 1000 / 150 = 6.67 → 6. Unchanged from before `colChoices` existed,
      // which is the point: every other grid in the section reads this path.
      expect(rows.first.children.length, 6);
      expect(tester.takeException(), isNull);
    });
  });

  group('lyric timestamps', () {
    // `lyrics_sync::fmt_ts` writes these and Dart echoes them. Two spellings
    // of the same format is how a hand-timed file ends up timed wrongly.
    test('are [mm:ss.xx], zero-padded, in centiseconds', () {
      expect(fmtStamp(0), '00:00.00');
      expect(fmtStamp(1000), '00:01.00');
      expect(fmtStamp(3450), '00:03.45');
      expect(fmtStamp(83450), '01:23.45');
    });

    test('do not roll the minutes into an hour', () {
      // LRC has no hour field: a 75-minute set is 75 minutes.
      expect(fmtStamp(75 * 60 * 1000), '75:00.00');
    });

    test('truncate rather than round, as centiseconds do', () {
      expect(fmtStamp(1999), '00:01.99');
    });
  });

  group('the pieces every tab is built from', () {
    testWidgets('a card with no art paints a placeholder, not a hole',
        (tester) async {
      await _at(
        tester,
        const Size(400, 400),
        Center(
          child: SizedBox(
            width: 160,
            height: 200,
            child: MusicCard(
              controller: MusicController.instance,
              title: 'A song',
              subtitle: 'An artist',
              artKind: 'track',
              artKey: '1',
              direct: '',
              fallback: Icons.music_note,
              onTap: () {},
            ),
          ),
        ),
      );
      expect(find.byIcon(Icons.music_note), findsOneWidget);
      expect(tester.takeException(), isNull);
    });

    testWidgets('an empty state says what to do about it', (tester) async {
      var tapped = false;
      await _at(
        tester,
        const Size(800, 600),
        MusicEmpty(
          icon: Icons.library_music_outlined,
          title: 'No music yet',
          body: 'Add a folder of audio files.',
          action: ('Add a folder', () => tapped = true),
        ),
      );
      expect(find.text('No music yet'), findsOneWidget);
      await tester.tap(find.text('Add a folder'));
      expect(tapped, isTrue);
      expect(tester.takeException(), isNull);
    });

    testWidgets('the pager reads as a page, not as a fraction',
        (tester) async {
      await _at(
        tester,
        const Size(600, 200),
        Pager(page: 0, pages: 12, onGo: (_) {}),
      );
      // Visible: "1 / 12". Spoken: a sentence.
      expect(find.text('1 / 12'), findsOneWidget);
      expect(
        tester.getSemantics(find.text('1 / 12')).label,
        'Page 1 of 12',
      );
      expect(tester.takeException(), isNull);
    });

    testWidgets('the pager will not step off either end', (tester) async {
      final gone = <int>[];
      await _at(
        tester,
        const Size(600, 200),
        Pager(page: 0, pages: 3, onGo: gone.add),
      );
      await tester.tap(find.byTooltip('Previous page'));
      await tester.pump();
      expect(gone, isEmpty, reason: 'there is no page before the first');
      await tester.tap(find.byTooltip('Next page'));
      await tester.pump();
      expect(gone, [1]);
    });
  });

  group('what a screen reader is told', () {
    testWidgets('a loved cover says which way pressing it goes',
        (tester) async {
      final handle = tester.ensureSemantics();
      await _at(
        tester,
        const Size(400, 400),
        Center(
          child: SizedBox(
            width: 160,
            height: 200,
            child: MusicCard(
              controller: MusicController.instance,
              title: 'Roads',
              subtitle: 'Portishead',
              artKind: 'track',
              artKey: '1',
              direct: '',
              fallback: Icons.music_note,
              loved: true,
              onFav: () {},
              onTap: () {},
            ),
          ),
        ),
      );
      // The heart is visible because the track is loved, hover or not.
      expect(find.byTooltip('Remove Roads from favourites'), findsOneWidget);
      handle.dispose();
      expect(tester.takeException(), isNull);
    });
  });

  group('My Music, driven off a snapshot', () {
    final c = MusicController.instance;

    tearDown(() => c.state = null);

    testWidgets('an empty library beats every sub-tab', (tester) async {
      // None of the nine can show anything and the fix is the same from all of
      // them, so the tab short-circuits before it picks one.
      c.state = _state(trackCount: 0, roots: const [], libTab: 'songs');
      await _at(tester, const Size(1400, 900), MyMusicTab(controller: c));
      expect(find.text('No music yet'), findsOneWidget);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    });

    testWidgets('a library with folders but no tracks does not', (tester) async {
      // A scan that has found the folder and not yet read it is a different
      // state, and saying "No music yet" there is wrong twice over.
      c.state = _state(
        trackCount: 0,
        roots: const [
          BrowseCard(id: 1, key: '/m', title: 'Music', subtitle: '', art: '',
              count: 0, loved: false, stars: 0),
        ],
        libTab: 'songs',
      );
      await _at(tester, const Size(1400, 900), MyMusicTab(controller: c));
      expect(find.text('No music yet'), findsNothing);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    });

    testWidgets('the sub-tab row carries all ten, and no scrollbar fight',
        (tester) async {
      c.state = _state(trackCount: 40, libTab: 'home');
      await _at(tester, const Size(1400, 900), MyMusicTab(controller: c));
      for (final tab in libTabs) {
        expect(find.text(tab.label), findsWidgets, reason: tab.id);
      }
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    });

    for (final size in const [Size(1920, 1080), Size(1280, 720)]) {
      testWidgets('a full page of songs fits at ${size.width}x${size.height}',
          (tester) async {
        // The cruel size is the point: 1280x720 is where a grid of 32 tiles
        // and two rows of chrome runs out of height first.
        c.state = _state(
          trackCount: 400,
          libTab: 'songs',
          songs: [for (var i = 0; i < 32; i++) _track(i, title: 'Song $i')],
          songTotal: 400,
          songPages: 13,
        );
        await _at(tester, size, MyMusicTab(controller: c));
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox());
      });
    }

    testWidgets('a search that matches nothing says so in the search\'s words',
        (tester) async {
      c.state = _state(
        trackCount: 40,
        libTab: 'songs',
        songs: const [],
        query: 'zzzz',
      );
      await _at(tester, const Size(1400, 900), MyMusicTab(controller: c));
      expect(find.textContaining('zzzz'), findsWidgets);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    });
  });

  group('the controller before the bridge has answered', () {
    final c = MusicController.instance;

    test('the visualizer falls back to spectrum, not to bars', () {
      // Bars was the old default and zen forced spectrum on the way in, which
      // is why a chosen style never survived. Spectrum is the default now, and
      // nothing overrides it.
      expect(c.state, isNull, reason: 'no bridge in a unit test');
      expect(c.visStyle, visStyleNames.length - 1);
      expect(c.visOn, isTrue);
    });

    test('there are ten library sub-tabs and Downloader is one of them', () {
      expect(libTabs.length, 10);
      expect(libTabs.map((t) => t.id), contains('downloader'));
      // Every tab carries its own hue: the row is ten places rather than one
      // selected thing and nine greys.
      expect(libTabs.map((t) => t.tint).toSet().length, libTabs.length);
    });
  });
}
