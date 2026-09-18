// The justified grid's arithmetic. Pins the two things that make a contact
// sheet a contact sheet: rows fill the width exactly, and a photo keeps its
// shape. Both are silent failures — a row that is 4px short still looks like a
// grid, and a squashed panorama still looks like a photo.

import 'package:flutter_test/flutter_test.dart';
import 'package:tulipix/sections/photos/photos_grid.dart';
import 'package:tulipix/src/rust/api/photos.dart';

PhotoTile tile(int id, int w, int h) => PhotoTile(
      itemId: id,
      path: '/p/$id.jpg',
      thumb: '',
      label: '$id',
      takenAt: 0,
      starred: false,
      archived: false,
      trashed: false,
      width: w,
      height: h,
      stackSize: 0,
      stackId: 0,
      live: false,
    );

void main() {
  const width = 1000.0;
  const target = 190.0;
  const gap = 3.0;

  test('every full row fills the width exactly', () {
    // Twenty landscape photos: enough for several full rows plus a remainder.
    final tiles = [for (var i = 0; i < 20; i++) tile(i, 3000, 2000)];
    final rows = justifyRows(
      tiles,
      List<int>.generate(tiles.length, (i) => i),
      width,
      target,
      gap,
    );
    expect(rows.length, greaterThan(1));
    // All but the last: the last row of a group ends where the photos end.
    for (final r in rows.take(rows.length - 1)) {
      final used =
          r.widths.fold<double>(0, (a, w) => a + w) + gap * (r.tiles.length - 1);
      expect(used, closeTo(width, 0.5), reason: 'row of ${r.tiles.length}');
    }
  });

  test('a photo keeps its shape', () {
    // A portrait, a square and a panorama in one row: each tile's width must
    // be its own aspect times the shared row height, not a square crop.
    final tiles = [tile(0, 2000, 3000), tile(1, 2000, 2000), tile(2, 6000, 2000)];
    final rows = justifyRows(tiles, [0, 1, 2], width, target, gap);
    final r = rows.first;
    for (var k = 0; k < r.tiles.length; k++) {
      expect(
        r.widths[k] / r.height,
        closeTo(aspectOf(tiles[r.tiles[k]]), 0.001),
      );
    }
  });

  test('a short last row keeps the target height', () {
    // One photo cannot fill 1000px at 190px tall, and scaling it up would make
    // a lone holiday snap three times the height of the row above it.
    final tiles = [tile(0, 3000, 2000)];
    final rows = justifyRows(tiles, [0], width, target, gap);
    expect(rows.single.height, target);
  });

  test('dimensions that were never read do not collapse the row', () {
    // width/height are 0 until the EXIF pass has been over the photo. A 0 here
    // would divide by nothing and take the whole row with it.
    final tiles = [for (var i = 0; i < 6; i++) tile(i, 0, 0)];
    final rows = justifyRows(
      tiles,
      List<int>.generate(6, (i) => i),
      width,
      target,
      gap,
    );
    expect(rows, isNotEmpty);
    for (final r in rows) {
      expect(r.height, greaterThan(0));
      for (final w in r.widths) {
        expect(w, greaterThan(0));
      }
    }
  });

  test('every photo is placed exactly once', () {
    // The greedy fill advances `i` inside the inner loop; an off-by-one there
    // drops a photo or shows it twice, and neither is visible at a glance.
    final tiles = [
      for (var i = 0; i < 37; i++) tile(i, 2000 + (i % 5) * 700, 2000),
    ];
    final rows = justifyRows(
      tiles,
      List<int>.generate(tiles.length, (i) => i),
      width,
      target,
      gap,
    );
    final placed = [for (final r in rows) ...r.tiles];
    expect(placed, List<int>.generate(tiles.length, (i) => i));
  });

  test('zero width lays nothing out rather than dividing by it', () {
    // A LayoutBuilder hands out a zero width for one frame during a route
    // change; an unguarded divide produces Infinity row heights and takes
    // SliverList down with it.
    expect(justifyRows([tile(0, 3000, 2000)], [0], 0, target, gap), isEmpty);
  });
}
