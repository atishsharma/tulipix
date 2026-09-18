// The timeline grid: justified rows, day headings in the flow, and a date
// scrubber down the right edge.
//
// The grid was a `SliverGrid` with a max cross-axis extent, which crops every
// photo to a square. A portrait and a panorama are not the same photo and the
// grid was the one place that refused to say so. Rows are justified instead:
// each row is filled greedily until it would be shorter than the target height,
// then scaled so it fills the width exactly — the layout Google Photos, Lightroom
// and every contact sheet before them use.
//
// It stays lazy. The layout pass is arithmetic over `tiles` — no widgets, no
// measurement — and its output is a flat list of rows that `SliverList` builds
// on demand. Twenty-four thousand photos cost one pass and a screen of widgets.

import 'package:flutter/material.dart';

import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/photos.dart';
import 'photo_tile.dart';
import 'photos_controller.dart';

/// One laid-out row: either a heading or a run of tiles that fills the width.
///
/// `height` is what the row occupies including nothing else — the caller adds
/// the gap. A heading row carries `label` and no tiles.
class PhotoRow {
  const PhotoRow.head(this.label, this.height)
      : tiles = const [],
        widths = const [];
  const PhotoRow.tiles(this.tiles, this.widths, this.height) : label = null;

  final String? label;

  /// Indices into `PhotosState.tiles`.
  final List<int> tiles;

  /// Each tile's width at this row's height, in the same order.
  final List<double> widths;
  final double height;

  bool get isHead => label != null;
}

/// A photo's shape, falling back to 3:2 when the dimensions were never read.
///
/// A zero here is not a broken photo, it is a photo whose EXIF has not been
/// read yet — which is the first thing the AI tab's Read EXIF pass fixes. Until
/// then the grid has to guess, and guessing landscape beats collapsing the row.
double aspectOf(PhotoTile t) {
  if (t.width <= 0 || t.height <= 0) return 1.5;
  return (t.width / t.height).clamp(0.4, 3.2);
}

/// Lay `indices` out into rows that each fill `width` exactly.
///
/// `target` is the height a row aims for; rows come out near it, never wildly
/// over. Pure arithmetic, so the whole thing is one unit test away from being
/// pinned.
List<PhotoRow> justifyRows(
  List<PhotoTile> tiles,
  List<int> indices,
  double width,
  double target,
  double gap,
) {
  final rows = <PhotoRow>[];
  if (width <= 0 || indices.isEmpty) return rows;
  var i = 0;
  while (i < indices.length) {
    final run = <int>[];
    var sumAr = 0.0;
    var height = target;
    while (i < indices.length) {
      run.add(indices[i]);
      sumAr += aspectOf(tiles[indices[i]]);
      i++;
      height = (width - gap * (run.length - 1)) / sumAr;
      if (height <= target) break;
    }
    // The last row of a group rarely fills the width. Scaling it up to do so
    // would make three holiday photos twice the height of everything above
    // them, so it keeps the target instead and simply ends early.
    if (height > target * 1.35) height = target;
    final widths = [for (final k in run) height * aspectOf(tiles[k])];
    rows.add(PhotoRow.tiles(run, widths, height));
  }
  return rows;
}

/// Heading height, so the scrubber and the layout agree on one number.
const double kHeadHeight = 44;
const double kRowGap = 3;

/// The three row heights the density button cycles.
const List<double> kDensities = [130, 190, 260];

/// Build the whole timeline: headings interleaved with justified rows, and the
/// y-offset of every heading for the scrubber.
({List<PhotoRow> rows, Map<int, double> headTops}) buildTimeline({
  required PhotosState state,
  required double width,
  required double target,
}) {
  final rows = <PhotoRow>[];
  final headTops = <int, double>{};
  var y = 0.0;
  // Groups are contiguous runs over `tiles`; an ungrouped view is one implicit
  // group over the whole list.
  final groups = state.groups.isEmpty
      ? [(null, List<int>.generate(state.tiles.length, (i) => i))]
      : [
          for (final g in state.groups)
            (g.label, g.tiles.map((i) => i).toList())
        ];

  for (final (label, indices) in groups) {
    if (label != null) {
      headTops[rows.length] = y;
      rows.add(PhotoRow.head(label, kHeadHeight));
      y += kHeadHeight;
    }
    for (final r in justifyRows(state.tiles, indices, width, target, kRowGap)) {
      rows.add(r);
      y += r.height + kRowGap;
    }
  }
  return (rows: rows, headTops: headTops);
}

class JustifiedGrid extends StatefulWidget {
  const JustifiedGrid({
    super.key,
    required this.controller,
    required this.state,
    required this.onOpen,
    required this.onStackMenu,
    this.footer,
  });

  final PhotosController controller;
  final PhotosState state;
  final void Function(PhotoTile) onOpen;
  final void Function(PhotoTile, TapDownDetails) onStackMenu;
  final Widget? footer;

  @override
  State<JustifiedGrid> createState() => _JustifiedGridState();
}

class _JustifiedGridState extends State<JustifiedGrid> {
  final _scroll = ScrollController();

  @override
  void dispose() {
    _scroll.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    return LayoutBuilder(
      builder: (context, box) {
        // The scrubber sits over the right edge rather than beside it: taking
        // 54px out of the grid for a column of month labels costs a tile per
        // row on a laptop.
        const scrubW = 54.0;
        final width = box.maxWidth - 40 - scrubW;
        final built = buildTimeline(
          state: widget.state,
          width: width,
          target: c.density,
        );
        final rows = built.rows;
        return Stack(
          children: [
            CustomScrollView(
              controller: _scroll,
              slivers: [
                SliverPadding(
                  padding: const EdgeInsets.fromLTRB(20, 4, 20 + scrubW, 0),
                  sliver: SliverList.builder(
                    itemCount: rows.length,
                    itemBuilder: (context, i) => _Row(
                      row: rows[i],
                      state: widget.state,
                      controller: c,
                      onOpen: widget.onOpen,
                      onStackMenu: widget.onStackMenu,
                    ),
                  ),
                ),
                if (widget.footer != null)
                  SliverToBoxAdapter(child: widget.footer),
                const SliverToBoxAdapter(child: SizedBox(height: 24)),
              ],
            ),
            if (built.headTops.length > 1)
              Positioned(
                top: 0,
                right: 0,
                bottom: 0,
                width: scrubW,
                child: _Scrubber(
                  rows: rows,
                  headTops: built.headTops,
                  scroll: _scroll,
                ),
              ),
          ],
        );
      },
    );
  }
}

class _Row extends StatelessWidget {
  const _Row({
    required this.row,
    required this.state,
    required this.controller,
    required this.onOpen,
    required this.onStackMenu,
  });

  final PhotoRow row;
  final PhotosState state;
  final PhotosController controller;
  final void Function(PhotoTile) onOpen;
  final void Function(PhotoTile, TapDownDetails) onStackMenu;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (row.isHead) {
      return SizedBox(
        height: row.height,
        child: Align(
          alignment: Alignment.bottomLeft,
          child: Padding(
            padding: const EdgeInsets.only(bottom: 8, left: 4),
            child: Text(
              row.label!,
              style: TextStyle(
                fontFamily: context.skin.fontFamily ?? Tokens.fontFamily,
                fontSize: 15,
                fontWeight: FontWeight.w600,
                color: t.nInk,
              ),
            ),
          ),
        ),
      );
    }
    return Padding(
      padding: const EdgeInsets.only(bottom: kRowGap),
      child: SizedBox(
        height: row.height,
        child: Row(
          children: [
            for (var k = 0; k < row.tiles.length; k++) ...[
              if (k > 0) const SizedBox(width: kRowGap),
              SizedBox(
                width: row.widths[k],
                child: _tile(context, state.tiles[row.tiles[k]]),
              ),
            ],
          ],
        ),
      ),
    );
  }

  Widget _tile(BuildContext context, PhotoTile tile) => PhotoTileView(
        // Keyed on the item, not the position: a row re-justifies when the
        // window moves and a tile that keeps its key keeps its loaded thumb.
        key: ValueKey(tile.itemId),
        tile: tile,
        controller: controller,
        selected: controller.selected.contains(tile.itemId),
        onTap: () {
          if (controller.selecting) {
            controller.toggleSelect(tile.itemId);
          } else {
            onOpen(tile);
          }
        },
        onStackMenu: tile.stackSize > 1 ? (e) => onStackMenu(tile, e) : null,
        onToggleSelect: () => controller.toggleSelect(tile.itemId),
      );
}

/// Month ticks down the right edge, with a bubble that follows the scroll.
///
/// The offsets are exact because the layout pass already computed them — this
/// is the one thing a justified grid gives you that a lazy uniform grid cannot,
/// since it knows every row's height before any of them is built.
class _Scrubber extends StatefulWidget {
  const _Scrubber({
    required this.rows,
    required this.headTops,
    required this.scroll,
  });

  final List<PhotoRow> rows;
  final Map<int, double> headTops;
  final ScrollController scroll;

  @override
  State<_Scrubber> createState() => _ScrubberState();
}

class _ScrubberState extends State<_Scrubber> {
  double _at = 0;

  @override
  void initState() {
    super.initState();
    widget.scroll.addListener(_onScroll);
  }

  @override
  void dispose() {
    widget.scroll.removeListener(_onScroll);
    super.dispose();
  }

  void _onScroll() {
    if (!widget.scroll.hasClients) return;
    final max = widget.scroll.position.maxScrollExtent;
    final f = max <= 0 ? 0.0 : (widget.scroll.offset / max).clamp(0.0, 1.0);
    if ((f - _at).abs() > 0.002) setState(() => _at = f);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final entries = widget.headTops.entries.toList();
    if (entries.isEmpty) return const SizedBox.shrink();
    final total = entries.last.value;
    return LayoutBuilder(
      builder: (context, box) {
        final h = box.maxHeight;
        // One tick per ~34px of track. A five-year library has sixty months
        // and room for a dozen labels; the rest would be a smear.
        final step = (entries.length / ((h - 40) / 34)).ceil().clamp(1, 999);
        return Stack(
          children: [
            for (var i = 0; i < entries.length; i += step)
              Positioned(
                top: total <= 0
                    ? 10
                    : (entries[i].value / total) * (h - 34) + 12,
                right: 8,
                child: _Tick(
                  label: _short(widget.rows[entries[i].key].label ?? ''),
                  onTap: () => widget.scroll.animateTo(
                    entries[i].value,
                    duration: const Duration(milliseconds: 260),
                    curve: Curves.easeOutCubic,
                  ),
                ),
              ),
            Positioned(
              top: (_at * (h - 40) + 14).clamp(0.0, h - 26),
              right: 6,
              child: IgnorePointer(
                child: AnimatedOpacity(
                  opacity: widget.scroll.hasClients &&
                          widget.scroll.position.isScrollingNotifier.value
                      ? 1
                      : 0.55,
                  duration: const Duration(milliseconds: 140),
                  child: Container(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
                    decoration: BoxDecoration(
                      color: t.nInk,
                      borderRadius: BorderRadius.circular(999),
                    ),
                    child: Text(
                      _short(_labelAt(_at)),
                      style: TextStyle(
                        fontSize: 10.5,
                        fontWeight: FontWeight.w700,
                        color: t.nCanvas,
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ],
        );
      },
    );
  }

  String _labelAt(double f) {
    final entries = widget.headTops.entries.toList();
    if (entries.isEmpty) return '';
    final want = f * entries.last.value;
    var hit = entries.first;
    for (final e in entries) {
      if (e.value > want) break;
      hit = e;
    }
    return widget.rows[hit.key].label ?? '';
  }

  /// "May 2026" → "May ’26". The column is 54px wide.
  static String _short(String label) {
    final parts = label.split(' ');
    if (parts.length != 2 || parts[1].length != 4) return label;
    return '${parts[0].substring(0, parts[0].length.clamp(0, 3))} ’${parts[1].substring(2)}';
  }
}

class _Tick extends StatelessWidget {
  const _Tick({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(6),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 3),
        child: Text(
          label,
          style: TextStyle(
            fontSize: 10,
            fontWeight: FontWeight.w600,
            color: t.nInk3,
          ),
        ),
      ),
    );
  }
}
