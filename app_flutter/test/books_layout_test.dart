// Layout regression for the Books shelf and the Genesis grid.
//
// Every widget here is pure: the controller is detached from the bridge and its
// cover lookup is stubbed, so this runs without the native library. What it
// catches is the class of bug that only shows at run time — an info column
// asking for more height than its 60/40 slot, a toolbar row spilling past the
// card, a grid cell computed negative on a short window.
//
// Books is a budget layout like Home: the grid is three across and two down,
// cut from whatever is left of the window, so the cruel sizes below are the
// point. A 1280 × 720 window is where the tile's bottom block breaks first.

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:tulipix/design/tokens.dart';
import 'package:tulipix/sections/books/book_mockup.dart';
import 'package:tulipix/sections/books/book_theme.dart';
import 'package:tulipix/sections/books/books_controller.dart';
import 'package:tulipix/sections/books/books_widgets.dart';
import 'package:tulipix/sections/genesis/genesis_page.dart';
import 'package:tulipix/src/rust/api/books.dart';
import 'package:tulipix/src/rust/api/genesis.dart';

/// A controller that never reaches the bridge, and never asks it for a cover.
class _NoBridge extends BooksController {
  _NoBridge() : super.detached();

  @override
  String? coverFor(Book book) => null;
}

Book _book({
  String title = 'The Master and Margarita',
  String author = 'Mikhail Bulgakov',
  double rating = 3.5,
  double netRating = 4.2,
  bool favorite = true,
}) =>
    Book(
      id: 1,
      title: title,
      author: author,
      series: 'A series with a long enough name to push a row',
      genre: 'Fiction',
      format: 'EPUB',
      cover: '',
      published: '1967',
      percent: 0.42,
      favorite: favorite,
      finished: false,
      missing: false,
      trashed: false,
      magazine: false,
      rtl: false,
      rating: rating,
      netRating: netRating,
      sizeBytes: 1024 * 900,
      addedAt: 1700000000,
      lastRead: 1700000000,
      timeRead: 7200,
    );

const _collections = [
  CollectionRow(id: 1, name: 'To read', count: 12, member: false),
  CollectionRow(id: 2, name: 'Favourites', count: 3, member: true),
];

GenRow _row({String format = 'EPUB', bool have = false}) => GenRow(
      md5: 'abc123',
      title: 'A title long enough to need both of its two lines and then some',
      author: 'Somebody With A Long Name',
      year: '2019',
      pages: '480',
      language: 'English',
      size: '4.2 MB',
      format: format,
      cover: '',
      have: have,
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

/// The geometry the page hands the grid, from the window size: 24 padding all
/// round, the 300 px rail and its 20 gutter on the right, and above the grid
/// the header, the 210 hero band, the 60 toolbar and their three 20/16 gaps.
({double w, double h}) _gridArea(Size window) => (
      w: window.width - 48 - 300 - 20,
      h: window.height - 48 - 42 - 20 - 210 - 20 - 60 - 16,
    );

void main() {
  // The two shapes that matter: a laptop, and the smallest window anyone
  // actually runs this in.
  const sizes = [Size(1920, 1080), Size(1440, 900), Size(1280, 720)];

  group('the grid tile', () {
    for (final size in sizes) {
      testWidgets('fits its 3 × 2 cell at ${size.width}x${size.height}',
          (tester) async {
        final area = _gridArea(size);
        await _at(
          tester,
          size,
          Center(
            child: SizedBox(
              width: (area.w - 2 * 16) / 3,
              height: (area.h - 16) / 2,
              child: BookGridTile(
                controller: _NoBridge(),
                book: _book(),
                collections: _collections,
                onOpen: () {},
                onAuthor: () {},
                onRate: (_) {},
                onFavorite: () {},
                onAddCollection: (_) {},
                onMenu: (_) {},
              ),
            ),
          ),
        );
        expect(tester.takeException(), isNull);
      });
    }

    testWidgets('a title with no author still draws its three blocks',
        (tester) async {
      final area = _gridArea(const Size(1440, 900));
      await _at(
        tester,
        const Size(1440, 900),
        Center(
          child: SizedBox(
            width: (area.w - 2 * 16) / 3,
            height: (area.h - 16) / 2,
            child: BookGridTile(
              controller: _NoBridge(),
              book: _book(author: '', netRating: 0, rating: 0),
              collections: const [],
              onOpen: () {},
              onAuthor: () {},
              onRate: (_) {},
              onFavorite: () {},
              onAddCollection: (_) {},
              onMenu: (_) {},
            ),
          ),
        ),
      );
      // Once: the drawn cover is painted onto the mockup's face rather than
      // built out of widgets, so the only Text saying EPUB is the badge in the
      // controls row.
      expect(find.text('EPUB'), findsOneWidget);
      expect(find.textContaining('Publication Date'), findsOneWidget);
      expect(tester.takeException(), isNull);
    });

    testWidgets('the online average sits beside the five stars', (tester) async {
      final area = _gridArea(const Size(1920, 1080));
      await _at(
        tester,
        const Size(1920, 1080),
        Center(
          child: SizedBox(
            width: (area.w - 2 * 16) / 3,
            height: (area.h - 16) / 2,
            child: BookGridTile(
              controller: _NoBridge(),
              book: _book(),
              collections: _collections,
              onOpen: () {},
              onAuthor: () {},
              onRate: (_) {},
              onFavorite: () {},
              onAddCollection: (_) {},
              onMenu: (_) {},
            ),
          ),
        ),
      );
      final stars = tester.getRect(find.byType(RatingPill));
      final net = tester.getRect(find.byType(NetRatingPill));
      // To the right of them, on the same line — not above, under the author.
      expect(net.left, greaterThanOrEqualTo(stars.right));
      expect((net.center.dy - stars.center.dy).abs(), lessThan(2));
      expect(tester.takeException(), isNull);
    });
  });

  group('the list row', () {
    for (final size in sizes) {
      testWidgets('fits one of six at ${size.width}x${size.height}',
          (tester) async {
        final area = _gridArea(size);
        await _at(
          tester,
          size,
          Center(
            child: SizedBox(
              // List mode is a half-width card, six down.
              width: area.w * 0.5,
              height: (area.h - 5 * 16) / 6,
              child: BookListRow(
                controller: _NoBridge(),
                book: _book(),
                collections: _collections,
                onOpen: () {},
                onAuthor: () {},
                onRate: (_) {},
                onFavorite: () {},
                onAddCollection: (_) {},
                onMenu: (_) {},
              ),
            ),
          ),
        );
        expect(tester.takeException(), isNull);
      });
    }
  });

  group('the hero row', () {
    for (final size in sizes) {
      testWidgets('six stat cards fit the 210 band at ${size.width}',
          (tester) async {
        // Two rows of three inside the 680-wide block, 12 between.
        await _at(
          tester,
          size,
          Center(
            child: SizedBox(
              width: (size.width - 340).clamp(260.0, 680.0) / 3 - 8,
              height: (210 - 12) / 2,
              child: StatCard(
                label: 'Total Books',
                value: '12,481',
                sub: '+128 this month',
                tint: const BookTheme(dark: false, oled: false).tintViolet,
                accent: BookTheme.violet,
                icon: Icons.menu_book_outlined,
                onTap: () {},
              ),
            ),
          ),
        );
        expect(tester.takeException(), isNull);
      });
    }
  });

  group('the rating pill', () {
    testWidgets('reports half a star from the left of one', (tester) async {
      double? got;
      await _at(
        tester,
        const Size(1440, 900),
        Center(
          child: SizedBox(
            width: 200,
            child: RatingPill(rating: 0, onRate: (v) => got = v),
          ),
        ),
      );
      final star = tester.getRect(find.byIcon(Icons.star_border).first);
      await tester.tapAt(Offset(star.left + 2, star.center.dy));
      await tester.pump();
      expect(got, 0.5);
      expect(tester.takeException(), isNull);
    });

    testWidgets('tapping the star that is already the rating clears it',
        (tester) async {
      double? got;
      await _at(
        tester,
        const Size(1440, 900),
        Center(
          child: SizedBox(
            width: 200,
            child: RatingPill(rating: 1, onRate: (v) => got = v),
          ),
        ),
      );
      final star = tester.getRect(find.byIcon(Icons.star).first);
      await tester.tapAt(Offset(star.right - 2, star.center.dy));
      await tester.pump();
      expect(got, 0);
    });
  });

  group('the Genesis card', () {
    for (final size in sizes) {
      testWidgets('fits its computed cell at ${size.width}x${size.height}',
          (tester) async {
        // The page is 24 padded, the body card 1 bordered and 14 padded.
        final inner = size.width - 48 - 2 - 28;
        final cols = ((size.width - 50) / 232).floor().clamp(2, 12);
        final cell = (inner - (cols - 1) * 14) / cols;
        await _at(
          tester,
          size,
          Center(
            child: SizedBox(
              width: cell,
              // Cover is 1.18 × the cell, and the text block under it is a
              // fixed 143.
              height: cell * 1.18 + 143,
              child: BookCard(
                row: _row(),
                canDownload: true,
                busy: false,
                onDownload: () {},
                onOpen: () {},
              ),
            ),
          ),
        );
        expect(tester.takeException(), isNull);
      });
    }

    testWidgets('one already in the library says so instead of offering it',
        (tester) async {
      await _at(
        tester,
        const Size(1440, 900),
        Center(
          child: SizedBox(
            width: 218,
            height: 218 * 1.18 + 143,
            child: BookCard(
              row: _row(have: true),
              canDownload: true,
              busy: false,
              onDownload: () {},
              onOpen: () {},
            ),
          ),
        ),
      );
      expect(find.text('In library'), findsOneWidget);
      expect(find.text('Download'), findsNothing);
      expect(tester.takeException(), isNull);
    });
  });

  group('the book mockup', () {
    test('both quads sit inside their own frame', () {
      for (final m in const [Mockup.tile, Mockup.hero]) {
        expect(m.quad.length, 4);
        for (final p in m.quad) {
          expect(p.dx, inInclusiveRange(0, m.frame.width));
          expect(p.dy, inInclusiveRange(0, m.frame.height));
        }
      }
    });

    test('the face reaches across the frame it is warped onto', () {
      // A quad that had drifted small is exactly the bug this replaces: the
      // cover has to reach the mockup's edges, not float inside them. The
      // upright face is nearly the whole frame; the tilted one is foreshortened
      // and sits in a square canvas, so it is judged on its own span.
      for (final (m, least) in const [(Mockup.tile, 0.8), (Mockup.hero, 0.5)]) {
        var l = double.infinity, t = double.infinity, r = 0.0, b = 0.0;
        for (final p in m.quad) {
          l = p.dx < l ? p.dx : l;
          t = p.dy < t ? p.dy : t;
          r = p.dx > r ? p.dx : r;
          b = p.dy > b ? p.dy : b;
        }
        final span = (r - l) * (b - t) / (m.frame.width * m.frame.height);
        expect(span, greaterThan(least),
            reason: '${m.asset} face spans only '
                '${(span * 100).round()}% of its frame');
      }
    });

    testWidgets('a book paints on its mockup at both scales', (tester) async {
      for (final m in const [Mockup.tile, Mockup.hero]) {
        await _at(
          tester,
          const Size(1440, 900),
          Center(
            child: SizedBox(
              width: 195,
              height: 178,
              child: BookMockup(
                  controller: _NoBridge(), book: _book(), mockup: m),
            ),
          ),
        );
        // The frame and the drawn cover both resolve off the main thread.
        await tester.pumpAndSettle();
        expect(tester.takeException(), isNull);
      }
    });
  });
}
