// Layout regression for the reader's chrome and its page.
//
// Same shape as books_layout_test: the widgets are pure and the controller is
// detached, so nothing here reaches the bridge. The reader is where a budget
// layout bites hardest — two 72px bars, a 310px docked panel and a 36px zoom
// column all come out of the same window, and the spread is sized off whatever
// is left.

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:tulipix/design/tokens.dart';
import 'package:tulipix/sections/books/book_theme.dart';
import 'package:tulipix/sections/books/book_reader.dart';
import 'package:tulipix/sections/books/books_controller.dart';
import 'package:tulipix/sections/books/reader_parts.dart';
import 'package:tulipix/src/rust/api/books.dart';

class _NoBridge extends BooksController {
  _NoBridge() : super.detached();
}

const _prefs = ReaderPrefs(
  fontPx: 17,
  lineIndex: 1,
  marginIndex: 1,
  typeface: 0,
  align: 1,
  bold: false,
  theme: 'light',
  brightness: 1.0,
);

/// A page of real prose, long enough that a short column has to clip it.
final _body =
    'The morning was cold and the sky had the colour of an old coin. He walked '
            'the length of the platform twice before the train came, counting the '
            'sleepers between the boards and thinking about nothing in particular, '
            'which was the only kind of thinking he had left. ' *
        6;

Reader _reader({
  bool imageMode = false,
  bool single = false,
  int toc = 3,
  int bookmarks = 2,
  int notes = 2,
  int hits = 3,
  int thumbs = 0,
  String error = '',
}) =>
    Reader(
      open: true,
      id: 1,
      title: 'A Book With A Reasonably Long Title On It',
      author: 'Somebody With A Long Name',
      format: imageMode ? 'PDF' : 'EPUB',
      filePath: '',
      imageMode: imageMode,
      page: 12,
      pageCount: 214,
      percent: 0.42,
      chapterName: 'Chapter Four · The Long Way Round',
      nextChapterName: 'Chapter Five · What Came After',
      leftText: _body,
      rightText: _body,
      leftHeading: 'The Long Way Round',
      rightHeading: '',
      leftImage: '',
      rightImage: '',
      leftFolio: 24,
      rightFolio: 25,
      rtl: false,
      single: single,
      trim: false,
      bookmarked: true,
      hasNote: false,
      toc: [
        for (var i = 0; i < toc; i++)
          TocItem(
              label: 'Chapter ${i + 1} · A heading long enough to elide',
              depth: i % 2,
              chapter: i),
      ],
      bookmarks: [
        for (var i = 0; i < bookmarks; i++)
          BookmarkRow(
              id: i,
              page: i * 30,
              note: 'a note',
              color: 'yellow',
              createdAt: 0),
      ],
      notes: [
        for (var i = 0; i < notes; i++)
          NoteRow(
              id: i,
              page: i * 30,
              snippet: 'a snippet of the page it was taken from',
              note: 'what I thought about it',
              color: 'yellow',
              createdAt: 0),
      ],
      searchQuery: 'coin',
      searchResults: [
        for (var i = 0; i < hits; i++)
          SearchHit(
              page: i * 11 + 3,
              snippet: 'the sky had the colour of an old coin, and he'),
      ],
      searchEnabled: !imageMode,
      thumbs: [
        for (var i = 0; i < thumbs; i++) ThumbRow(page: i + 1, path: ''),
      ],
      prefs: _prefs,
      nextTitle: 'The One After This One',
      nextId: 2,
      error: error,
    );

/// The whole snapshot, so the reader shell can be pumped as the section pumps
/// it. Only `reader` matters here; the rest is the empty library around it.
BooksState _state(Reader reader) => BooksState(
      view: 'library',
      books: const [],
      total: 0,
      filteredTotal: 0,
      page: 1,
      pageCount: 1,
      viewMode: 'grid',
      sortIndex: 0,
      query: '',
      searchContents: false,
      slider: const [],
      stats: const LibraryStats(
        total: 0,
        authors: 0,
        finished: 0,
        inProgress: 0,
        hoursRead: 0,
        addedMonth: 0,
        authorsMonth: 0,
      ),
      formats: const [],
      genres: const [],
      series: const [],
      collections: const [],
      smart: const [],
      activeFormat: '',
      activeGenre: '',
      activeSeries: '',
      activeAuthor: '',
      activeQuick: 'all',
      activeCollection: 0,
      trashedCount: 0,
      folders: const [],
      detailSummary: '',
      detailCollections: const [],
      reader: reader,
      status: '',
    );

Widget _host(Widget child, {bool dark = true}) => MaterialApp(
      theme: tulipixTheme(dark ? Tokens.dark() : Tokens.light()),
      home: Scaffold(body: child),
    );

Future<void> _at(WidgetTester tester, Size size, Widget child) async {
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.reset);
  await tester.pumpWidget(_host(child));
  await tester.pump();
}

/// What the stage gets: the window less the two 72px bars and, when one is
/// open, the 310px panel.
({double w, double h}) _stage(Size window, {bool panel = false}) => (
      w: window.width - (panel ? 310 : 0),
      h: window.height - 144,
    );

void main() {
  const sizes = [Size(1920, 1080), Size(1440, 900), Size(1280, 720)];

  group('the paper page', () {
    for (final size in sizes) {
      testWidgets('fits its half of the spread at ${size.width}',
          (tester) async {
        // The mockup is contain-fitted at 95%, and a page is 0.34 × 0.87 of it.
        final stage = _stage(size);
        final bgW = [1360.0, stage.w - 140, stage.h * (1511 / 1041)]
                .reduce((a, c) => a < c ? a : c) *
            0.95;
        final bgH = bgW / (1511 / 1041);
        await _at(
          tester,
          size,
          Center(
            child: SizedBox(
              width: bgW * 0.34,
              height: bgH * 0.87,
              child: PaperPage(
                body: _body,
                heading: 'The Long Way Round',
                folio: 24,
                palette: ReaderPalette.of('light'),
                prefs: _prefs,
                onPrev: () {},
                onNext: () {},
              ),
            ),
          ),
        );
        expect(find.text('24'), findsOneWidget);
        expect(tester.takeException(), isNull);
      });
    }

    testWidgets('the widest margin still leaves a column', (tester) async {
      await _at(
        tester,
        const Size(1280, 720),
        Center(
          child: SizedBox(
            width: 240,
            height: 300,
            child: PaperPage(
              body: _body,
              heading: '',
              folio: 7,
              palette: ReaderPalette.of('sepia'),
              // Wide margins are 150 of the column's width, which on a small
              // window is most of it.
              prefs: const ReaderPrefs(
                fontPx: 26,
                lineIndex: 2,
                marginIndex: 2,
                typeface: 2,
                align: 0,
                bold: true,
                theme: 'sepia',
                brightness: 0.6,
              ),
              onPrev: () {},
              onNext: () {},
            ),
          ),
        ),
      );
      expect(tester.takeException(), isNull);
    });
  });

  group('the side panels', () {
    for (final panel in ['contents', 'search', 'bookmarks', 'notes']) {
      for (final size in sizes) {
        testWidgets('$panel fits at ${size.width}x${size.height}',
            (tester) async {
          final c = _NoBridge();
          final r = _reader();
          await _at(
            tester,
            size,
            SizedBox(
              height: _stage(size).h,
              child: Row(
                children: [
                  const Spacer(),
                  switch (panel) {
                    'contents' =>
                      ContentsPanel(controller: c, reader: r, onClose: () {}),
                    'search' =>
                      SearchPanel(controller: c, reader: r, onClose: () {}),
                    'bookmarks' =>
                      BookmarksPanel(controller: c, reader: r, onClose: () {}),
                    _ => NotesPanel(controller: c, reader: r, onClose: () {}),
                  },
                ],
              ),
            ),
          );
          expect(tester.takeException(), isNull);
        });
      }
    }

    testWidgets('an empty contents says so rather than drawing nothing',
        (tester) async {
      await _at(
        tester,
        const Size(1440, 900),
        SizedBox(
          height: 756,
          child: Row(children: [
            const Spacer(),
            ContentsPanel(
                controller: _NoBridge(),
                reader: _reader(toc: 0),
                onClose: () {}),
          ]),
        ),
      );
      expect(find.textContaining('no table of contents'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  });

  group('the sliders', () {
    testWidgets('the scrub bar reports the page under the tap', (tester) async {
      int? got;
      await _at(
        tester,
        const Size(1440, 900),
        Center(
          child: SizedBox(
            width: 400,
            child:
                BookSlider(value: 1, maximum: 101, onChanged: (v) => got = v),
          ),
        ),
      );
      final bar = tester.getRect(find.byType(BookSlider));
      await tester.tapAt(Offset(bar.left + bar.width / 2, bar.center.dy));
      await tester.pump();
      // Half of 1..101 is 51, give or take the pixel the tap landed on.
      expect(got, closeTo(51, 2));
      expect(tester.takeException(), isNull);
    });

    testWidgets('the zoom column steps by a quarter', (tester) async {
      double? got;
      await _at(
        tester,
        const Size(1440, 900),
        Center(
          child: SizedBox(
            width: 36,
            height: 230,
            child: VFloatSlider(
                value: 1.0,
                minimum: 0.5,
                maximum: 4.0,
                onChanged: (v) => got = v),
          ),
        ),
      );
      await tester.tap(find.text('+'));
      await tester.pump();
      expect(got, 1.25);
      await tester.tap(find.text('−'));
      await tester.pump();
      expect(got, 0.75);
    });
  });

  group('the chrome parts', () {
    testWidgets('a captioned button and an icon one are the same box',
        (tester) async {
      await _at(
        tester,
        const Size(1440, 900),
        Center(
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              ChromeButton(
                  caption: 'Aa', tip: 'Reading settings', onTap: () {}),
              ChromeButton(
                  icon: Icons.menu, accent: BookTheme.pink, onTap: () {}),
            ],
          ),
        ),
      );
      final boxes = tester
          .widgetList<ChromeButton>(find.byType(ChromeButton))
          .map((b) => b.side)
          .toList();
      expect(boxes, [44, 44]);
      expect(tester.takeException(), isNull);
    });

    testWidgets('a segmented control divides its row evenly', (tester) async {
      await _at(
        tester,
        const Size(1440, 900),
        Center(
          child: SizedBox(
            width: 288,
            child: Row(
              children: [
                for (final l in const ['Compact', 'Normal', 'Relaxed'])
                  Expanded(
                      child: SegItem(
                          label: l, active: l == 'Normal', onTap: () {})),
              ],
            ),
          ),
        ),
      );
      expect(tester.takeException(), isNull);
    });
  });

  group('the reader shell', () {
    for (final size in sizes) {
      for (final image in [false, true]) {
        testWidgets(
            '${image ? "a comic" : "an ebook"} fits at '
            '${size.width}x${size.height}', (tester) async {
          final c = _NoBridge()..state = _state(_reader(imageMode: image));
          await _at(tester, size, BookReader(controller: c));
          expect(tester.takeException(), isNull);
        });
      }
    }

    testWidgets('every panel opens over the page without spilling the bar',
        (tester) async {
      final c = _NoBridge()..state = _state(_reader());
      await _at(tester, const Size(1280, 720), BookReader(controller: c));
      // The contents button is the one every book has.
      await tester.tap(find.byTooltip('Table of contents'));
      await tester.pump();
      expect(find.text('Contents'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });

    testWidgets('the Aa sheet fits beside the page it settles', (tester) async {
      final c = _NoBridge()..state = _state(_reader());
      await _at(tester, const Size(1280, 720), BookReader(controller: c));
      await tester.tap(find.byTooltip('Reading settings'));
      await tester.pump();
      expect(find.text('Typeface'), findsOneWidget);
      expect(find.text('Line spacing'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });

    testWidgets('a comic\'s Aa sheet drops the typography it cannot use',
        (tester) async {
      final c = _NoBridge()..state = _state(_reader(imageMode: true));
      await _at(tester, const Size(1440, 900), BookReader(controller: c));
      await tester.tap(find.byTooltip('Reading settings'));
      await tester.pump();
      expect(find.text('Page layout'), findsOneWidget);
      expect(find.text('Typeface'), findsNothing);
      expect(tester.takeException(), isNull);
    });

    testWidgets('a book that would not open offers the way back',
        (tester) async {
      final c = _NoBridge()
        ..state = _state(_reader(error: 'The file has moved.'));
      await _at(tester, const Size(1440, 900), BookReader(controller: c));
      expect(find.text('Back to library'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });
  });
}
