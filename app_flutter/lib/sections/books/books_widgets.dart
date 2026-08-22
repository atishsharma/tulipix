// The pieces the library is built from: a cover, a tile, a row, a chip, stars.

import 'dart:io';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/books.dart';
import 'books_controller.dart';

/// A cover, or a coloured spine with the title on it when there is none. Books
/// without art are the normal case in a fresh library, and a grid of grey
/// rectangles is unreadable.
class BookCover extends StatelessWidget {
  const BookCover({
    super.key,
    required this.controller,
    required this.book,
    required this.width,
    this.radius = 8,
  });

  final BooksController controller;
  final Book book;
  final double width;
  final double radius;

  @override
  Widget build(BuildContext context) {
    // Books are taller than they are wide; 2:3 is the paperback proportion.
    final height = width * 1.5;
    final path = controller.coverFor(book);
    final hue = coverHue(book.title);

    return ClipRRect(
      borderRadius: BorderRadius.circular(radius),
      child: SizedBox(
        width: width,
        height: height,
        child: path == null
            ? _Blank(book: book, hue: hue, width: width)
            : Image.file(
                File(path),
                fit: BoxFit.cover,
                errorBuilder: (_, __, ___) =>
                    _Blank(book: book, hue: hue, width: width),
              ),
      ),
    );
  }
}

class _Blank extends StatelessWidget {
  const _Blank({required this.book, required this.hue, required this.width});

  final Book book;
  final Color hue;
  final double width;

  @override
  Widget build(BuildContext context) => DecoratedBox(
        decoration: BoxDecoration(
          gradient: LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [hue, Color.lerp(hue, Colors.black, 0.45)!],
          ),
        ),
        child: Padding(
          padding: EdgeInsets.all(width * 0.10),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                book.title,
                maxLines: 4,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: width * 0.11,
                  height: 1.2,
                  fontWeight: FontWeight.w700,
                  color: Colors.white,
                ),
              ),
              const Spacer(),
              Text(
                book.author,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: width * 0.085,
                  color: Colors.white70,
                ),
              ),
            ],
          ),
        ),
      );
}

/// A tile in the grid: cover, progress ring, title, author, and the menu.
class BookTile extends StatefulWidget {
  const BookTile({
    super.key,
    required this.controller,
    required this.book,
    required this.onOpen,
    required this.onDetails,
    this.width = 150,
  });

  final BooksController controller;
  final Book book;
  final VoidCallback onOpen;
  final VoidCallback onDetails;
  final double width;

  @override
  State<BookTile> createState() => _BookTileState();
}

class _BookTileState extends State<BookTile> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final b = widget.book;
    final c = widget.controller;

    return MouseRegion(
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: b.missing ? widget.onDetails : widget.onOpen,
        onSecondaryTap: widget.onDetails,
        child: SizedBox(
          width: widget.width,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Stack(
                children: [
                  AnimatedScale(
                    scale: _hover ? 1.03 : 1.0,
                    duration: const Duration(milliseconds: 140),
                    child: BookCover(
                      controller: c,
                      book: b,
                      width: widget.width,
                    ),
                  ),
                  if (b.percent > 0.001 && b.percent < 0.999)
                    Positioned(
                      left: 0,
                      right: 0,
                      bottom: 0,
                      child: _ProgressStripe(percent: b.percent),
                    ),
                  if (b.finished)
                    const Positioned(
                      right: 6,
                      top: 6,
                      child: _Badge(
                        icon: Icons.check,
                        colour: Color(0xFF2FBF71),
                        tip: 'Finished',
                      ),
                    ),
                  if (b.missing)
                    const Positioned(
                      left: 6,
                      top: 6,
                      child: _Badge(
                        icon: Icons.link_off,
                        colour: Color(0xFFEF4444),
                        tip: 'The file has moved',
                      ),
                    ),
                  Positioned(
                    left: 6,
                    bottom: 10,
                    child: _FormatPill(format: b.format),
                  ),
                  if (_hover)
                    Positioned(
                      right: 4,
                      bottom: 8,
                      child: _TileMenu(
                          controller: c, book: b, onDetails: widget.onDetails),
                    ),
                  Positioned(
                    right: 4,
                    top: b.finished ? 34 : 6,
                    child: IconButton(
                      iconSize: 16,
                      visualDensity: VisualDensity.compact,
                      tooltip: b.favorite ? 'Unfavourite' : 'Favourite',
                      icon: Icon(
                        b.favorite ? Icons.favorite : Icons.favorite_border,
                        color: b.favorite
                            ? const Color(0xFFE0518F)
                            : Colors.white70,
                        shadows: const [
                          Shadow(color: Colors.black54, blurRadius: 4)
                        ],
                      ),
                      onPressed: () =>
                          c.send(BooksCmd.toggleFavorite(id: b.id)),
                    ),
                  ),
                ],
              ),
              const SizedBox(height: 8),
              Text(
                b.title,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                  color: t.nInk,
                  height: 1.25,
                ),
              ),
              Text(
                b.author,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 11, color: t.nInk2),
              ),
              if (b.rating > 0) Stars(value: b.rating, size: 12),
            ],
          ),
        ),
      ),
    );
  }
}

class _ProgressStripe extends StatelessWidget {
  const _ProgressStripe({required this.percent});

  final double percent;

  @override
  Widget build(BuildContext context) => Container(
        height: 4,
        color: Colors.black38,
        alignment: Alignment.centerLeft,
        child: FractionallySizedBox(
          widthFactor: percent.clamp(0.0, 1.0),
          child: Container(color: Tokens.secBooks),
        ),
      );
}

class _Badge extends StatelessWidget {
  const _Badge({required this.icon, required this.colour, required this.tip});

  final IconData icon;
  final Color colour;
  final String tip;

  @override
  Widget build(BuildContext context) => Tooltip(
        message: tip,
        child: Container(
          width: 22,
          height: 22,
          decoration: BoxDecoration(color: colour, shape: BoxShape.circle),
          child: Icon(icon, size: 13, color: Colors.white),
        ),
      );
}

class _FormatPill extends StatelessWidget {
  const _FormatPill({required this.format});

  final String format;

  @override
  Widget build(BuildContext context) => Container(
        padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
        decoration: BoxDecoration(
          color: Colors.black54,
          borderRadius: BorderRadius.circular(4),
        ),
        child: Text(
          format.toUpperCase(),
          style: const TextStyle(
            fontSize: 8,
            fontWeight: FontWeight.w800,
            letterSpacing: 0.6,
            color: Colors.white,
          ),
        ),
      );
}

class _TileMenu extends StatelessWidget {
  const _TileMenu({
    required this.controller,
    required this.book,
    required this.onDetails,
  });

  final BooksController controller;
  final Book book;
  final VoidCallback onDetails;

  @override
  Widget build(BuildContext context) {
    return PopupMenuButton<String>(
      tooltip: 'More',
      iconSize: 18,
      icon: const Icon(Icons.more_vert,
          color: Colors.white,
          shadows: [Shadow(color: Colors.black54, blurRadius: 4)]),
      onSelected: (v) {
        switch (v) {
          case 'details':
            onDetails();
          case 'magazine':
            controller.send(BooksCmd.toggleMagazine(id: book.id));
          case 'rtl':
            controller.send(BooksCmd.toggleRtl(id: book.id));
          case 'trash':
            controller.send(BooksCmd.trash(id: book.id));
          case 'restore':
            controller.send(BooksCmd.restore(id: book.id));
          case 'delete':
            controller.send(BooksCmd.deletePerm(id: book.id));
        }
      },
      itemBuilder: (_) => [
        const PopupMenuItem(value: 'details', child: Text('Details')),
        PopupMenuItem(
          value: 'magazine',
          child: Text(book.magazine ? 'Not a magazine' : 'Treat as a magazine'),
        ),
        PopupMenuItem(
          value: 'rtl',
          child: Text(book.rtl ? 'Left-to-right pages' : 'Right-to-left pages'),
        ),
        const PopupMenuDivider(),
        if (book.trashed) ...[
          const PopupMenuItem(value: 'restore', child: Text('Restore')),
          const PopupMenuItem(
              value: 'delete', child: Text('Delete permanently')),
        ] else
          const PopupMenuItem(value: 'trash', child: Text('Move to trash')),
      ],
    );
  }
}

/// A row in list mode: the same book, denser, with the columns a list can show
/// that a tile cannot.
class BookListRow extends StatelessWidget {
  const BookListRow({
    super.key,
    required this.controller,
    required this.book,
    required this.onOpen,
    required this.onDetails,
  });

  final BooksController controller;
  final Book book;
  final VoidCallback onOpen;
  final VoidCallback onDetails;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: book.missing ? onDetails : onOpen,
      onSecondaryTap: onDetails,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        child: Row(
          children: [
            BookCover(controller: controller, book: book, width: 34, radius: 4),
            const SizedBox(width: 12),
            Expanded(
              flex: 4,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(book.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 13,
                          fontWeight: FontWeight.w600,
                          color: t.nInk)),
                  Text(
                    [book.author, if (book.series.isNotEmpty) book.series]
                        .where((s) => s.isNotEmpty)
                        .join('  ·  '),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11, color: t.nInk2),
                  ),
                ],
              ),
            ),
            Expanded(
              flex: 2,
              child: Text(book.genre,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11, color: t.nInk2)),
            ),
            SizedBox(width: 90, child: Stars(value: book.rating, size: 13)),
            SizedBox(
              width: 60,
              child: Text(book.format.toUpperCase(),
                  style: TextStyle(fontSize: 10, color: t.nInk3)),
            ),
            SizedBox(
              width: 70,
              child: Text(fmtSize(book.sizeBytes),
                  style: TextStyle(fontSize: 11, color: t.nInk2)),
            ),
            SizedBox(
              width: 54,
              child: book.percent > 0
                  ? Text('${(book.percent * 100).round()}%',
                      style:
                          const TextStyle(fontSize: 11, color: Tokens.secBooks))
                  : const SizedBox.shrink(),
            ),
            _TileMenu(controller: controller, book: book, onDetails: onDetails),
          ],
        ),
      ),
    );
  }
}

/// Five stars, tappable when `onRate` is given. Half stars are shown but not
/// settable — a half star is what Open Library reports, not something a reader
/// chooses.
class Stars extends StatelessWidget {
  const Stars({
    super.key,
    required this.value,
    this.size = 16,
    this.onRate,
  });

  final double value;
  final double size;
  final ValueChanged<double>? onRate;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (var i = 1; i <= 5; i++)
          GestureDetector(
            onTap: onRate == null
                ? null
                // Tapping the star that is already the rating clears it.
                : () => onRate!(value.round() == i ? 0 : i.toDouble()),
            child: Icon(
              value >= i
                  ? Icons.star
                  : (value >= i - 0.5 ? Icons.star_half : Icons.star_border),
              size: size,
              color: value >= i - 0.5 ? const Color(0xFFF5A623) : t.nInk3,
            ),
          ),
      ],
    );
  }
}

/// A filter chip with a count.
class BookChipView extends StatelessWidget {
  const BookChipView({
    super.key,
    required this.label,
    required this.active,
    required this.onTap,
    this.count,
    this.icon,
  });

  final String label;
  final bool active;
  final VoidCallback onTap;
  final int? count;
  final IconData? icon;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(right: 6),
      child: Material(
        color: active ? Tokens.secBooks.withValues(alpha: 0.18) : t.nChip,
        borderRadius: BorderRadius.circular(16),
        child: InkWell(
          borderRadius: BorderRadius.circular(16),
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (icon != null) ...[
                  Icon(icon,
                      size: 14, color: active ? Tokens.secBooks : t.nInk2),
                  const SizedBox(width: 6),
                ],
                Text(
                  label,
                  style: TextStyle(
                    fontSize: 12,
                    fontWeight: active ? FontWeight.w700 : FontWeight.w500,
                    color: active ? Tokens.secBooks : t.nInk,
                  ),
                ),
                if (count != null && count! >= 0) ...[
                  const SizedBox(width: 6),
                  Text('$count',
                      style: TextStyle(fontSize: 11, color: t.nInk3)),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Previous / page n of m / next.
class BookPager extends StatelessWidget {
  const BookPager({
    super.key,
    required this.page,
    required this.pages,
    required this.onGo,
  });

  final int page;
  final int pages;
  final ValueChanged<int> onGo;

  @override
  Widget build(BuildContext context) {
    if (pages <= 1) return const SizedBox.shrink();
    final t = context.tokens;
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        IconButton(
          icon: const Icon(Icons.chevron_left),
          onPressed: page > 1 ? () => onGo(page - 1) : null,
        ),
        Text('$page of $pages', style: TextStyle(fontSize: 12, color: t.nInk2)),
        IconButton(
          icon: const Icon(Icons.chevron_right),
          onPressed: page < pages ? () => onGo(page + 1) : null,
        ),
      ],
    );
  }
}
