// The pieces the library is built from — one Dart class per Slint component in
// ui/page_books.slint: Card, PillButton, FilterChip, EntryCard, Cover, Book3D,
// StatCard, RatingPill, GridTile, RailBlock, RailRow, GenreChip.
//
// The geometry here is the Slint file's, not a re-design of it: where a number
// looks arbitrary (0.40 of the tile for the cover, 60/40 for the info column,
// 0.831 × the frame for the inset cover) it is the number that page uses.

import 'dart:io';

import 'package:flutter/material.dart';

import '../../src/rust/api/books.dart';
import 'book_mockup.dart';
import 'book_theme.dart';
import 'books_controller.dart';

/// Slint's `alignment: start/end` plus `clip: true`, which is what every tight
/// block on that page uses. A Column that is a pixel taller than its slot must
/// lose the overflowing pixel, not paint a stripe over it: the scroll view
/// gives the child unbounded height and clips, and the physics keep it from
/// actually scrolling.
class Clipped extends StatelessWidget {
  const Clipped({
    super.key,
    required this.child,
    this.fromBottom = false,
    this.axis = Axis.vertical,
  });

  final Widget child;
  final bool fromBottom;
  final Axis axis;

  @override
  Widget build(BuildContext context) => SingleChildScrollView(
        scrollDirection: axis,
        reverse: fromBottom,
        physics: const NeverScrollableScrollPhysics(),
        child: child,
      );
}

/// White rounded card with the section's hairline shadow. `Card` in Slint.
class PanelCard extends StatelessWidget {
  const PanelCard({
    super.key,
    required this.child,
    this.padding,
    this.height,
    this.width,
    this.radius = BookTheme.radius,
    this.border,
    this.clip = false,
  });

  final Widget child;
  final EdgeInsetsGeometry? padding;
  final double? height;
  final double? width;
  final double radius;
  final BoxBorder? border;
  final bool clip;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return Container(
      height: height,
      width: width,
      padding: padding,
      clipBehavior: clip ? Clip.antiAlias : Clip.none,
      decoration: BoxDecoration(
        color: b.card,
        borderRadius: BorderRadius.circular(radius),
        border: border,
        boxShadow: b.cardShadow,
      ),
      child: child,
    );
  }
}

/// Rounded pill button. `filled` → solid accent + white; else outline.
class PillButton extends StatefulWidget {
  const PillButton({
    super.key,
    required this.label,
    required this.onTap,
    this.icon,
    this.filled = false,
    this.compact = false,
    this.accent = BookTheme.accent,
  });

  final String label;
  final VoidCallback onTap;
  final IconData? icon;
  final bool filled;
  final bool compact;
  final Color accent;

  @override
  State<PillButton> createState() => _PillButtonState();
}

class _PillButtonState extends State<PillButton> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final h = widget.compact ? 38.0 : 40.0;
    final ink = widget.filled ? Colors.white : widget.accent;
    final bg = widget.filled
        ? (_hover
            ? Color.lerp(widget.accent, Colors.black, 0.08)!
            : widget.accent)
        : (_hover ? b.pillBg : b.card);
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 120),
          height: h,
          padding: EdgeInsets.fromLTRB(
              widget.compact ? 14 : 16, 0, widget.compact ? 15 : 18, 0),
          decoration: BoxDecoration(
            color: bg,
            borderRadius: BorderRadius.circular(h / 2),
            border: widget.filled
                ? null
                : Border.all(
                    color: widget.accent.withValues(alpha: 0.35), width: 1.5),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (widget.icon != null) ...[
                Icon(widget.icon, size: widget.compact ? 16 : 18, color: ink),
                SizedBox(width: widget.compact ? 7 : 8),
              ],
              Text(widget.label,
                  style: TextStyle(
                      fontSize: widget.compact ? 13 : 14,
                      fontWeight: FontWeight.w700,
                      color: ink)),
            ],
          ),
        ),
      ),
    );
  }
}

/// Rounded filter chip — per-chip hue: soft fill + outline, solid when active.
class BookFilterChip extends StatelessWidget {
  const BookFilterChip({
    super.key,
    required this.label,
    required this.onTap,
    this.count = -1,
    this.active = false,
    this.hue = BookTheme.accent,
  });

  final String label;
  final VoidCallback onTap;
  final int count;
  final bool active;
  final Color hue;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: onTap,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        child: Container(
          height: 34,
          padding: const EdgeInsets.symmetric(horizontal: 14),
          decoration: BoxDecoration(
            color: active ? hue : hue.withValues(alpha: 0.10),
            borderRadius: BorderRadius.circular(17),
            border: Border.all(
                color: hue.withValues(alpha: active ? 0 : 0.45), width: 1.5),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(label,
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w600,
                      color: active ? Colors.white : hue)),
              if (count >= 0) ...[
                const SizedBox(width: 6),
                Text('$count',
                    style: TextStyle(
                        fontSize: 12,
                        fontWeight: FontWeight.w600,
                        color: active
                            ? Colors.white.withValues(alpha: 0.8)
                            : hue.withValues(alpha: 0.75))),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// A row in the collections / series panels: icon · name · count · chevron.
class EntryCard extends StatefulWidget {
  const EntryCard({
    super.key,
    required this.label,
    required this.count,
    required this.icon,
    required this.onTap,
    this.subtitle = 'Saved view',
    this.onDelete,
  });

  final String label;
  final int count;
  final IconData icon;
  final VoidCallback onTap;
  final String subtitle;
  final VoidCallback? onDelete;

  @override
  State<EntryCard> createState() => _EntryCardState();
}

class _EntryCardState extends State<EntryCard> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: Container(
          height: 54,
          padding: const EdgeInsets.fromLTRB(14, 0, 12, 0),
          decoration: BoxDecoration(
            color: _hover ? b.pillBg : b.card,
            borderRadius: BorderRadius.circular(12),
            border: Border.all(color: b.hairline),
          ),
          child: Row(
            children: [
              Container(
                width: 34,
                height: 34,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: b.tintViolet,
                  borderRadius: BorderRadius.circular(10),
                ),
                child: Icon(widget.icon, size: 17, color: BookTheme.accent),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(widget.label,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 14,
                            fontWeight: FontWeight.w700,
                            color: b.ink)),
                    // A smart collection's membership is a live query, so
                    // there is no number to show without running it.
                    Text(
                        widget.count < 0
                            ? widget.subtitle
                            : '${widget.count} '
                                '${widget.count == 1 ? "book" : "books"}',
                        style: TextStyle(fontSize: 11, color: b.inkDim)),
                  ],
                ),
              ),
              if (widget.onDelete != null)
                IconButton(
                  iconSize: 15,
                  visualDensity: VisualDensity.compact,
                  icon: const Icon(Icons.delete_outline,
                      color: Color(0xFFD6336C)),
                  onPressed: widget.onDelete,
                ),
              Icon(Icons.chevron_right, size: 16, color: b.inkDim),
            ],
          ),
        ),
      ),
    );
  }
}

/// The cover, or a generated card drawn from the title and author when there
/// is none. `Cover` in Slint: hairline frame, format badge, serif title,
/// divider, italic author, progress along the bottom edge.
class BookCover extends StatelessWidget {
  const BookCover({
    super.key,
    required this.controller,
    required this.book,
    required this.width,
    this.height,
    this.radius = 8,
    this.fitContain = false,
    this.showProgress = true,
  });

  final BooksController controller;
  final Book book;
  final double width;
  final double? height;
  final double radius;
  final bool fitContain;
  final bool showProgress;

  @override
  Widget build(BuildContext context) {
    // Books are taller than they are wide; 2:3 is the paperback proportion.
    final h = height ?? width * 1.5;
    final path = controller.coverFor(book);
    final hue = coverHue(book.title);
    final big = width > 100;

    return ClipRRect(
      borderRadius: BorderRadius.circular(radius),
      child: SizedBox(
        width: width,
        height: h,
        child: Stack(
          fit: StackFit.expand,
          children: [
            if (path == null)
              _Generated(book: book, hue: hue, big: big)
            else
              Image.file(
                File(path),
                fit: fitContain ? BoxFit.contain : BoxFit.cover,
                errorBuilder: (_, __, ___) =>
                    _Generated(book: book, hue: hue, big: big),
              ),
            if (showProgress && book.percent > 0)
              Positioned(
                left: 0,
                right: 0,
                bottom: 0,
                child: Container(
                  height: 4,
                  color: const Color(0x30000000),
                  alignment: Alignment.centerLeft,
                  child: FractionallySizedBox(
                    widthFactor: book.percent.clamp(0.0, 1.0),
                    child: Container(color: BookTheme.accent),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

class _Generated extends StatelessWidget {
  const _Generated({required this.book, required this.hue, required this.big});

  final Book book;
  final Color hue;
  final bool big;

  @override
  Widget build(BuildContext context) => DecoratedBox(
        decoration: BoxDecoration(
          gradient: LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [hue, Color.lerp(hue, Colors.black, 0.35)!],
          ),
        ),
        child: LayoutBuilder(
          builder: (context, box) {
            // Below this the card is a thumbnail of a thumbnail — the list
            // row's cover in a short window is a dozen pixels tall. Nothing on
            // it would be legible, so it stays a coloured spine.
            if (box.maxHeight < 64 || box.maxWidth < 44) {
              return const SizedBox.expand();
            }
            return _plate(context, box);
          },
        ),
      );

  Widget _plate(BuildContext context, BoxConstraints box) => Stack(
        children: [
          // Inner hairline frame.
          Positioned.fill(
            left: 6,
            top: 6,
            right: 6,
            bottom: 6,
            child: DecoratedBox(
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(6),
                border: Border.all(color: const Color(0x55FFFFFF), width: 1.5),
              ),
            ),
          ),
          Padding(
            padding: EdgeInsets.all(big ? 16 : 8),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                if (book.format.isNotEmpty)
                  Container(
                    height: big ? 20 : 15,
                    padding: const EdgeInsets.symmetric(horizontal: 6),
                    alignment: Alignment.center,
                    decoration: BoxDecoration(
                      color: const Color(0x33FFFFFF),
                      borderRadius: BorderRadius.circular(5),
                    ),
                    child: Text(book.format.toUpperCase(),
                        style: TextStyle(
                            fontSize: big ? 10 : 8,
                            fontWeight: FontWeight.w700,
                            color: Colors.white)),
                  ),
                Expanded(
                  child: Column(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      Flexible(
                        child: Text(
                          book.title,
                          textAlign: TextAlign.center,
                          maxLines: big ? 4 : 3,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            fontFamily: 'serif',
                            fontSize: big ? 20 : 11,
                            height: 1.2,
                            fontWeight: FontWeight.w700,
                            color: Colors.white,
                          ),
                        ),
                      ),
                      SizedBox(height: big ? 12 : 6),
                      Container(
                          width: big ? 46 : 24,
                          height: 1,
                          color: const Color(0x77FFFFFF)),
                      SizedBox(height: big ? 12 : 6),
                      Flexible(
                        child: Text(
                          book.author,
                          textAlign: TextAlign.center,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            fontFamily: 'serif',
                            fontSize: big ? 14 : 9,
                            fontStyle: FontStyle.italic,
                            color: const Color(0xDDFFFFFF),
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ],
      );
}

/// The "physical book" rendition: the cover warped onto the hardcover mockup's
/// face. See book_mockup.dart — the warp is what the Rust baker does, and what
/// the Slint file's own fallback (a contain-fitted cover floating in the face's
/// bounding box) fails to do.
class Book3D extends StatelessWidget {
  const Book3D({
    super.key,
    required this.controller,
    required this.book,
    this.mockup = Mockup.tile,
  });

  final BooksController controller;
  final Book book;
  final Mockup mockup;

  @override
  Widget build(BuildContext context) => BookMockup(
        controller: controller,
        book: book,
        mockup: mockup,
        dark: context.book.dark,
      );
}

/// One of the six cards in the hero row's 2 × 3 block.
class StatCard extends StatefulWidget {
  const StatCard({
    super.key,
    required this.label,
    required this.value,
    required this.sub,
    required this.tint,
    required this.accent,
    required this.icon,
    required this.onTap,
  });

  final String label;
  final String value;
  final String sub;
  final Color tint;
  final Color accent;
  final IconData icon;
  final VoidCallback onTap;

  @override
  State<StatCard> createState() => _StatCardState();
}

class _StatCardState extends State<StatCard> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: Container(
          padding: const EdgeInsets.all(16),
          decoration: BoxDecoration(
            color: _hover ? b.pillBg : b.card,
            borderRadius: BorderRadius.circular(BookTheme.radius),
            border: Border.all(
                color: widget.accent.withValues(alpha: 0.45), width: 1.5),
            boxShadow: b.cardShadow,
          ),
          child: Row(
            children: [
              Container(
                width: 46,
                height: 46,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: widget.tint,
                  borderRadius: BorderRadius.circular(13),
                ),
                child: Icon(widget.icon, size: 22, color: widget.accent),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(widget.value,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 24,
                            height: 1.15,
                            fontWeight: FontWeight.w800,
                            color: b.ink)),
                    Text(widget.label,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 12,
                            height: 1.2,
                            fontWeight: FontWeight.w500,
                            color: b.inkDim)),
                    if (widget.sub.isNotEmpty)
                      Text(widget.sub,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 10,
                              height: 1.2,
                              fontWeight: FontWeight.w600,
                              color: widget.accent)),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Live half-star rating pill. Stars fill under the cursor; the left half of a
/// star is a ½ step. Shared by the grid card, the list row and the detail
/// popup, exactly as `RatingPill` is in Slint.
class RatingPill extends StatefulWidget {
  const RatingPill({
    super.key,
    required this.rating,
    required this.onRate,
    this.star = 18,
  });

  final double rating;
  final ValueChanged<double> onRate;
  final double star;

  @override
  State<RatingPill> createState() => _RatingPillState();
}

class _RatingPillState extends State<RatingPill> {
  double _hover = -1;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final shown = _hover >= 0 ? _hover : widget.rating;
    final lit = widget.rating > 0 || _hover > 0;
    return Container(
      height: widget.star + 8,
      padding: EdgeInsets.symmetric(horizontal: widget.star * 0.7),
      decoration: BoxDecoration(
        color: BookTheme.amber.withValues(alpha: lit ? 0.16 : 0.07),
        borderRadius: BorderRadius.circular((widget.star + 8) / 2),
        border: Border.all(color: BookTheme.amber.withValues(alpha: 0.45)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (var i = 1; i <= 5; i++)
            SizedBox(
              width: widget.star + 4,
              child: Stack(
                alignment: Alignment.center,
                children: [
                  Icon(
                    shown >= i
                        ? Icons.star
                        : (shown >= i - 0.5
                            ? Icons.star_half
                            : Icons.star_border),
                    size: widget.star,
                    color: shown >= i - 0.5
                        ? BookTheme.amber
                        : b.inkDim.withValues(alpha: 0.55),
                  ),
                  // Left half = i − 0.5 stars, right half = i stars.
                  Row(
                    children: [
                      for (final v in [i - 0.5, i.toDouble()])
                        Expanded(
                          child: MouseRegion(
                            cursor: SystemMouseCursors.click,
                            onEnter: (_) => setState(() => _hover = v),
                            onExit: (_) => setState(
                                () => _hover = _hover == v ? -1 : _hover),
                            child: GestureDetector(
                              behavior: HitTestBehavior.opaque,
                              onTap: () =>
                                  widget.onRate(widget.rating == v ? 0 : v),
                              child: SizedBox(height: widget.star + 8),
                            ),
                          ),
                        ),
                    ],
                  ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// Five read-only stars, for the places that report a number rather than take
/// one (the online average).
class Stars extends StatelessWidget {
  const Stars({super.key, required this.value, this.size = 16, this.onRate});

  final double value;
  final double size;
  final ValueChanged<double>? onRate;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (var i = 1; i <= 5; i++)
          GestureDetector(
            // Tapping the star that is already the rating clears it.
            onTap: onRate == null
                ? null
                : () => onRate!(value.round() == i ? 0 : i.toDouble()),
            child: Icon(
              value >= i
                  ? Icons.star
                  : (value >= i - 0.5 ? Icons.star_half : Icons.star_border),
              size: size,
              color: value >= i - 0.5
                  ? BookTheme.amber
                  : b.inkDim.withValues(alpha: 0.55),
            ),
          ),
      ],
    );
  }
}

/// A titled rail block (Quick filters / File types / Genres).
class RailBlock extends StatelessWidget {
  const RailBlock({super.key, required this.title, required this.children});

  final String title;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(title,
            style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w700,
                letterSpacing: 0.6,
                color: b.inkDim)),
        for (final c in children) ...[const SizedBox(height: 6), c],
      ],
    );
  }
}

/// One rail list row: leading icon · label · trailing count.
class RailRow extends StatefulWidget {
  const RailRow({
    super.key,
    required this.icon,
    required this.label,
    required this.onTap,
    this.iconTint = BookTheme.accent,
    this.count = -1,
    this.active = false,
  });

  final IconData icon;
  final String label;
  final VoidCallback onTap;
  final Color iconTint;
  final int count;
  final bool active;

  @override
  State<RailRow> createState() => _RailRowState();
}

class _RailRowState extends State<RailRow> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 100),
          height: 36,
          padding: const EdgeInsets.fromLTRB(10, 0, 12, 0),
          decoration: BoxDecoration(
            color: widget.active
                ? b.tintViolet
                : (_hover ? b.pillBg : Colors.transparent),
            borderRadius: BorderRadius.circular(10),
          ),
          child: Row(
            children: [
              Icon(widget.icon,
                  size: 16,
                  color: widget.active ? BookTheme.accent : widget.iconTint),
              const SizedBox(width: 10),
              Expanded(
                child: Text(widget.label,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight:
                            widget.active ? FontWeight.w700 : FontWeight.w500,
                        color: widget.active ? BookTheme.accent : b.ink)),
              ),
              if (widget.count >= 0)
                Text('${widget.count}',
                    style: TextStyle(
                        fontSize: 12,
                        fontWeight: FontWeight.w600,
                        color: b.inkDim)),
            ],
          ),
        ),
      ),
    );
  }
}

/// A coloured genre chip with a trailing count (the rail's Genres section).
class GenreChip extends StatelessWidget {
  const GenreChip({
    super.key,
    required this.label,
    required this.onTap,
    this.count = -1,
    this.hue = BookTheme.accent,
    this.active = false,
  });

  final String label;
  final VoidCallback onTap;
  final int count;
  final Color hue;
  final bool active;

  @override
  Widget build(BuildContext context) => GestureDetector(
        onTap: onTap,
        child: MouseRegion(
          cursor: SystemMouseCursors.click,
          child: Container(
            height: 30,
            padding: const EdgeInsets.symmetric(horizontal: 12),
            decoration: BoxDecoration(
              color: hue.withValues(alpha: active ? 0.9 : 0.12),
              borderRadius: BorderRadius.circular(15),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Flexible(
                  child: Text(label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 12,
                          fontWeight: FontWeight.w600,
                          color: active ? Colors.white : hue)),
                ),
                if (count >= 0) ...[
                  const SizedBox(width: 8),
                  Text('$count',
                      style: TextStyle(
                          fontSize: 11,
                          fontWeight: FontWeight.w600,
                          color: active
                              ? Colors.white.withValues(alpha: 0.8)
                              : hue.withValues(alpha: 0.7))),
                ],
              ],
            ),
          ),
        ),
      );
}

/// The big grid card: cover on the left at 40 % of the width, the info column
/// on the right split 60 : 40 vertically. `GridTile` in Slint.
class BookGridTile extends StatefulWidget {
  const BookGridTile({
    super.key,
    required this.controller,
    required this.book,
    required this.collections,
    required this.onOpen,
    required this.onAuthor,
    required this.onRate,
    required this.onFavorite,
    required this.onAddCollection,
    required this.onMenu,
  });

  final BooksController controller;
  final Book book;
  final List<CollectionRow> collections;
  final VoidCallback onOpen;
  final VoidCallback onAuthor;
  final ValueChanged<double> onRate;
  final VoidCallback onFavorite;
  final ValueChanged<CollectionRow> onAddCollection;
  final ValueChanged<Offset> onMenu;

  @override
  State<BookGridTile> createState() => _BookGridTileState();
}

class _BookGridTileState extends State<BookGridTile> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final tile = widget.book;
    final hue = coverHue(tile.title);
    final date = tile.published.isNotEmpty
        ? tile.published
        : fmtDate(tile.addedAt.toInt());

    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onOpen,
        child: Container(
          clipBehavior: Clip.antiAlias,
          decoration: BoxDecoration(
            color: b.card,
            borderRadius: BorderRadius.circular(BookTheme.radius),
            border: Border.all(
                color: hue.withValues(alpha: _hover ? 0.85 : 0.45), width: 1.5),
            boxShadow: b.cardShadow,
          ),
          child: LayoutBuilder(
            builder: (context, box) => Padding(
              padding: const EdgeInsets.all(16),
              child: Row(
                children: [
                  // 40 %: the cover, as a composed physical book.
                  SizedBox(
                    width: (box.maxWidth - 46) * 0.40,
                    child: Book3D(controller: widget.controller, book: tile),
                  ),
                  const SizedBox(width: 14),
                  // 60 %: the info column, split 60 : 40.
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        Expanded(
                          flex: 60,
                          child:
                              _TileHead(book: tile, onAuthor: widget.onAuthor),
                        ),
                        Expanded(
                          flex: 40,
                          child: _TileFoot(
                            book: tile,
                            date: date,
                            collections: widget.collections,
                            onRate: widget.onRate,
                            onFavorite: widget.onFavorite,
                            onAddCollection: widget.onAddCollection,
                            onMenu: widget.onMenu,
                          ),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _TileHead extends StatefulWidget {
  const _TileHead({required this.book, required this.onAuthor});

  final Book book;
  final VoidCallback onAuthor;

  @override
  State<_TileHead> createState() => _TileHeadState();
}

class _TileHeadState extends State<_TileHead> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final tile = widget.book;
    return Clipped(
      child: Padding(
        padding: const EdgeInsets.only(top: 8),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(tile.title,
                maxLines: 3,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 20,
                    height: 1.15,
                    fontWeight: FontWeight.w800,
                    color: b.ink)),
            const SizedBox(height: 4),
            // The author is a link to that author's page.
            MouseRegion(
              cursor: SystemMouseCursors.click,
              onEnter: (_) => setState(() => _hover = true),
              onExit: (_) => setState(() => _hover = false),
              child: GestureDetector(
                onTap: widget.onAuthor,
                child: Text(
                  tile.author,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                    color: _hover ? BookTheme.accent : b.inkDim,
                    decoration: TextDecoration.underline,
                    decorationColor: _hover ? BookTheme.accent : b.inkDim,
                  ),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _TileFoot extends StatelessWidget {
  const _TileFoot({
    required this.book,
    required this.date,
    required this.collections,
    required this.onRate,
    required this.onFavorite,
    required this.onAddCollection,
    required this.onMenu,
  });

  final Book book;
  final String date;
  final List<CollectionRow> collections;
  final ValueChanged<double> onRate;
  final VoidCallback onFavorite;
  final ValueChanged<CollectionRow> onAddCollection;
  final ValueChanged<Offset> onMenu;

  @override
  Widget build(BuildContext context) {
    return Clipped(
      fromBottom: true,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Align(
            alignment: Alignment.centerLeft,
            child: _DatePill(date: date),
          ),
          const SizedBox(height: 6),
          // Your five stars, with the online average to their right. Neither
          // pill shrinks, and a three-across cell is 150 px of info column at
          // 1280 — so the row clips its tail rather than overflowing, the same
          // way every other block in this tile handles a short cell.
          Clipped(
            axis: Axis.horizontal,
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                RatingPill(rating: book.rating, onRate: onRate, star: 15),
                if (book.netRating > 0) ...[
                  const SizedBox(width: 6),
                  NetRatingPill(rating: book.netRating),
                ],
              ],
            ),
          ),
          const SizedBox(height: 6),
          Row(
            children: [
              Flexible(child: FormatBadge(format: book.format)),
              const Spacer(),
              AddToCollection(
                  collections: collections, onPick: onAddCollection),
              const SizedBox(width: 8),
              FavouriteDot(on: book.favorite, onTap: onFavorite, side: 28),
              const SizedBox(width: 8),
              MenuDot(onMenu: onMenu, side: 28),
            ],
          ),
        ],
      ),
    );
  }
}

/// The average rating fetched from online metadata — read-only, and always
/// beside the five stars that are yours.
class NetRatingPill extends StatelessWidget {
  const NetRatingPill({super.key, required this.rating});

  final double rating;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return Container(
      height: 22,
      padding: const EdgeInsets.symmetric(horizontal: 9),
      decoration: BoxDecoration(
        color: BookTheme.amber.withValues(alpha: 0.16),
        borderRadius: BorderRadius.circular(11),
        border: Border.all(color: BookTheme.amber.withValues(alpha: 0.45)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          const Icon(Icons.star, size: 12, color: BookTheme.amber),
          const SizedBox(width: 4),
          Flexible(
            child: Text('${(rating * 10).round() / 10} avg online',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w700,
                    color:
                        b.dark ? BookTheme.amber : const Color(0xFFCF8A12))),
          ),
        ],
      ),
    );
  }
}

class _DatePill extends StatelessWidget {
  const _DatePill({required this.date});

  final String date;

  @override
  Widget build(BuildContext context) => Container(
        height: 24,
        padding: const EdgeInsets.symmetric(horizontal: 10),
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: BookTheme.blue.withValues(alpha: 0.10),
          borderRadius: BorderRadius.circular(12),
          border: Border.all(color: BookTheme.blue.withValues(alpha: 0.4)),
        ),
        child: Text('Publication Date : ${date.isEmpty ? "—" : date}',
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w600,
                color: BookTheme.blue)),
      );
}

/// The violet file-type badge every card carries.
class FormatBadge extends StatelessWidget {
  const FormatBadge({super.key, required this.format});

  final String format;

  @override
  Widget build(BuildContext context) => Container(
        height: 20,
        padding: const EdgeInsets.symmetric(horizontal: 8),
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: BookTheme.accent.withValues(alpha: 0.10),
          borderRadius: BorderRadius.circular(6),
          border: Border.all(color: BookTheme.accent.withValues(alpha: 0.4)),
        ),
        child: Text(format.toUpperCase(),
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(
                fontSize: 10,
                fontWeight: FontWeight.w700,
                color: BookTheme.accent)),
      );
}

/// ⊕ — the green add-to-collection dot and its little menu.
class AddToCollection extends StatelessWidget {
  const AddToCollection({
    super.key,
    required this.collections,
    required this.onPick,
    this.side = 28,
  });

  final List<CollectionRow> collections;
  final ValueChanged<CollectionRow> onPick;
  final double side;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return PopupMenuButton<CollectionRow>(
      tooltip: 'Add to collection',
      position: PopupMenuPosition.under,
      // The dot is already the hit target; the default 8 all round makes the
      // controls row 16 wider than the tile's bottom block has to give.
      padding: EdgeInsets.zero,
      onSelected: onPick,
      itemBuilder: (_) => collections.isEmpty
          ? [
              PopupMenuItem(
                enabled: false,
                child: Text(
                    'No collections yet — create one\nin '
                    'the Collections tab.',
                    style: TextStyle(fontSize: 11, color: b.inkDim)),
              ),
            ]
          : [
              for (final c in collections)
                PopupMenuItem(
                    value: c,
                    height: 34,
                    child: Text(c.name, style: const TextStyle(fontSize: 13))),
            ],
      child: _Dot(
        side: side,
        hue: BookTheme.green,
        child: Icon(Icons.add, size: side * 0.57, color: BookTheme.green),
      ),
    );
  }
}

/// ♥ — the pink favourite dot.
class FavouriteDot extends StatelessWidget {
  const FavouriteDot({
    super.key,
    required this.on,
    required this.onTap,
    this.side = 28,
  });

  final bool on;
  final VoidCallback onTap;
  final double side;

  @override
  Widget build(BuildContext context) => GestureDetector(
        onTap: onTap,
        child: MouseRegion(
          cursor: SystemMouseCursors.click,
          child: _Dot(
            side: side,
            hue: BookTheme.pink,
            child: Icon(on ? Icons.favorite : Icons.favorite_border,
                size: side * 0.57, color: BookTheme.pink),
          ),
        ),
      );
}

/// ⋮ — the violet overflow dot. Reports where it is so the page can put its
/// one shared menu overlay there.
class MenuDot extends StatelessWidget {
  const MenuDot({super.key, required this.onMenu, this.side = 28});

  final ValueChanged<Offset> onMenu;
  final double side;

  @override
  Widget build(BuildContext context) => Builder(
        builder: (context) => GestureDetector(
          onTap: () {
            final box = context.findRenderObject() as RenderBox?;
            if (box == null) return;
            onMenu(box.localToGlobal(Offset.zero));
          },
          child: MouseRegion(
            cursor: SystemMouseCursors.click,
            child: _Dot(
              side: side,
              hue: BookTheme.accent,
              child: Icon(Icons.more_vert,
                  size: side * 0.57, color: BookTheme.accent),
            ),
          ),
        ),
      );
}

class _Dot extends StatelessWidget {
  const _Dot({required this.side, required this.hue, required this.child});

  final double side;
  final Color hue;
  final Widget child;

  @override
  Widget build(BuildContext context) => Container(
        width: side,
        height: side,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: hue.withValues(alpha: 0.10),
          borderRadius: BorderRadius.circular(side / 2),
          border: Border.all(color: hue.withValues(alpha: 0.4)),
        ),
        child: child,
      );
}

/// List view: the same book as a 15 / 55 / 30 row — cover · title, author,
/// date · rating and controls.
class BookListRow extends StatefulWidget {
  const BookListRow({
    super.key,
    required this.controller,
    required this.book,
    required this.collections,
    required this.onOpen,
    required this.onAuthor,
    required this.onRate,
    required this.onFavorite,
    required this.onAddCollection,
    required this.onMenu,
  });

  final BooksController controller;
  final Book book;
  final List<CollectionRow> collections;
  final VoidCallback onOpen;
  final VoidCallback onAuthor;
  final ValueChanged<double> onRate;
  final VoidCallback onFavorite;
  final ValueChanged<CollectionRow> onAddCollection;
  final ValueChanged<Offset> onMenu;

  @override
  State<BookListRow> createState() => _BookListRowState();
}

class _BookListRowState extends State<BookListRow> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final tile = widget.book;
    final hue = coverHue(tile.title);
    final date = tile.published.isNotEmpty
        ? tile.published
        : fmtDate(tile.addedAt.toInt());

    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onOpen,
        child: Container(
          clipBehavior: Clip.antiAlias,
          decoration: BoxDecoration(
            color: b.card,
            borderRadius: BorderRadius.circular(BookTheme.radius),
            border: Border.all(
                color: hue.withValues(alpha: _hover ? 0.85 : 0.45), width: 1.5),
            boxShadow: b.cardShadow,
          ),
          child: LayoutBuilder(
            builder: (context, box) => Padding(
              padding: const EdgeInsets.all(10),
              child: Row(
                children: [
                  SizedBox(
                    width: (box.maxWidth - 44) * 0.15,
                    child: Book3D(controller: widget.controller, book: tile),
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Clipped(
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(tile.title,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: const TextStyle(
                                  fontSize: 16,
                                  fontWeight: FontWeight.w800,
                                  color: BookTheme.accent)),
                          const SizedBox(height: 4),
                          GestureDetector(
                            onTap: widget.onAuthor,
                            child: Text(tile.author,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: const TextStyle(
                                  fontSize: 13,
                                  fontWeight: FontWeight.w700,
                                  color: BookTheme.blue,
                                  decoration: TextDecoration.underline,
                                  decorationColor: BookTheme.blue,
                                )),
                          ),
                          const SizedBox(height: 4),
                          Text('Publication : ${date.isEmpty ? "—" : date}',
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: const TextStyle(
                                  fontSize: 12,
                                  fontWeight: FontWeight.w600,
                                  color: BookTheme.amber)),
                        ],
                      ),
                    ),
                  ),
                  const SizedBox(width: 12),
                  SizedBox(
                    width: (box.maxWidth - 44) * 0.30,
                    child: Clipped(
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          RatingPill(
                              rating: tile.rating,
                              star: 13,
                              onRate: widget.onRate),
                          const SizedBox(height: 6),
                          Row(
                            mainAxisAlignment: MainAxisAlignment.center,
                            children: [
                              Flexible(child: FormatBadge(format: tile.format)),
                              const SizedBox(width: 8),
                              AddToCollection(
                                  collections: widget.collections,
                                  onPick: widget.onAddCollection,
                                  side: 26),
                              const SizedBox(width: 8),
                              FavouriteDot(
                                  on: tile.favorite,
                                  onTap: widget.onFavorite,
                                  side: 26),
                              const SizedBox(width: 8),
                              MenuDot(onMenu: widget.onMenu, side: 26),
                            ],
                          ),
                        ],
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}
