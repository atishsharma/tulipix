// The detail popup, and the three small dialogs the section needs.
//
// The popup is ui/page_books.slint's: a 768 × 506 card split 40 / 60 — the
// book on the left, everything known about it on the right, in one meta pill,
// a progress pill, the collection chips, a scrolling summary and the actions.
// The page owns the scrim and the card; this is what goes inside it.

import 'package:flutter/material.dart';

import '../../design/pick.dart';
import '../../src/rust/api/books.dart';
import 'book_theme.dart';
import 'books_controller.dart';
import 'books_widgets.dart';

/// One book, everything known about it, and everything that can be done to it.
class BookDetailPanel extends StatefulWidget {
  const BookDetailPanel({
    super.key,
    required this.controller,
    required this.book,
  });

  final BooksController controller;
  final Book book;

  @override
  State<BookDetailPanel> createState() => _BookDetailPanelState();
}

class _BookDetailPanelState extends State<BookDetailPanel> {
  /// The delete confirmation replaces the action row rather than opening a
  /// second dialog on top of this one.
  bool _confirmDelete = false;

  @override
  void initState() {
    super.initState();
    widget.controller.ensureSummary(widget.book);
  }

  @override
  void didUpdateWidget(BookDetailPanel old) {
    super.didUpdateWidget(old);
    if (old.book.id != widget.book.id) {
      widget.controller.ensureSummary(widget.book);
    }
  }

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final book = widget.book;
    final st = widget.controller.state!;
    final c = widget.controller;

    return Padding(
      padding: const EdgeInsets.all(22),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // 40 %: the book itself, which lays down when you point at it.
          Expanded(
            flex: 40,
            child: _DetailCover(controller: c, book: book),
          ),
          const SizedBox(width: 20),
          // 60 %: the info column.
          Expanded(
            flex: 60,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(book.title,
                              maxLines: 2,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                  fontSize: 20,
                                  fontWeight: FontWeight.w800,
                                  color: b.ink)),
                          const SizedBox(height: 2),
                          GestureDetector(
                            onTap: () async {
                              await c.send(const BooksCmd.closeDetail());
                              await c.send(
                                  BooksCmd.setAuthor(author: book.author));
                            },
                            child: Text(book.author,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: const TextStyle(
                                    fontSize: 13,
                                    fontWeight: FontWeight.w600,
                                    color: BookTheme.accent)),
                          ),
                        ],
                      ),
                    ),
                    IconButton(
                      iconSize: 15,
                      visualDensity: VisualDensity.compact,
                      icon: Icon(Icons.close, color: b.inkDim),
                      onPressed: () => c.send(const BooksCmd.closeDetail()),
                    ),
                  ],
                ),
                const SizedBox(height: 10),
                // File type · genre · size · added · time read · publication ·
                // series · rating are ONE container: the outer spacing must
                // never get between those lines.
                _MetaPill(controller: c, book: book),
                if (book.percent > 0) ...[
                  const SizedBox(height: 10),
                  _ProgressPill(percent: book.percent),
                ],
                if (st.detailCollections.isNotEmpty) ...[
                  const SizedBox(height: 10),
                  Wrap(
                    spacing: 6,
                    runSpacing: 6,
                    children: [
                      for (final col in st.detailCollections)
                        _CollectionToggle(
                          row: col,
                          onTap: () => c.send(BooksCmd.collectionToggle(
                              id: col.id, bookId: book.id)),
                        ),
                    ],
                  ),
                ],
                const SizedBox(height: 10),
                // The summary absorbs the leftover height — it scrolls, so
                // capping it would push the slack into the meta pill instead.
                Expanded(child: _Summary(controller: c, book: book)),
                const SizedBox(height: 10),
                if (_confirmDelete)
                  _ConfirmDelete(
                    title: book.title,
                    onCancel: () => setState(() => _confirmDelete = false),
                    onRemove: () async {
                      setState(() => _confirmDelete = false);
                      await c.send(BooksCmd.trash(id: book.id));
                      await c.send(const BooksCmd.closeDetail());
                    },
                  )
                else
                  _Actions(
                    controller: c,
                    book: book,
                    onAskDelete: () => setState(() => _confirmDelete = true),
                  ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// The cover column. It shows the book on its mockup, and swaps to the flat
/// cover while the pointer is on it — the Slint popup does the same thing with
/// its `detail-book-hero` rendition, so you can read the art without the
/// hardcover's tilt across it. Flat means filling the column, not letterboxed
/// inside it: the whole point of the swap is to see the cover.
class _DetailCover extends StatefulWidget {
  const _DetailCover({required this.controller, required this.book});

  final BooksController controller;
  final Book book;

  @override
  State<_DetailCover> createState() => _DetailCoverState();
}

class _DetailCoverState extends State<_DetailCover> {
  bool _flat = false;

  @override
  Widget build(BuildContext context) => MouseRegion(
        onEnter: (_) => setState(() => _flat = true),
        onExit: (_) => setState(() => _flat = false),
        child: AnimatedSwitcher(
          duration: const Duration(milliseconds: 160),
          child: _flat
              ? LayoutBuilder(
                  key: const ValueKey('flat'),
                  builder: (context, box) => Center(
                    child: BookCover(
                      controller: widget.controller,
                      book: widget.book,
                      width: box.maxWidth,
                      height: box.maxHeight,
                      radius: 10,
                      showProgress: false,
                    ),
                  ),
                )
              : Book3D(
                  key: const ValueKey('mockup'),
                  controller: widget.controller,
                  book: widget.book,
                ),
        ),
      );
}

class _MetaPill extends StatelessWidget {
  const _MetaPill({required this.controller, required this.book});

  final BooksController controller;
  final Book book;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final facts = [
      book.format.toUpperCase(),
      if (book.genre.isNotEmpty) book.genre,
      if (book.sizeBytes > 0) fmtSize(book.sizeBytes),
      if (book.addedAt > 0) fmtDate(book.addedAt),
    ].join('  ·  ');
    final timeRead = fmtReadTime(book.timeRead);

    return Container(
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: b.pillBg.withValues(alpha: 0.6),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: BookTheme.accent.withValues(alpha: 0.3)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(facts, style: TextStyle(fontSize: 12, color: b.inkDim)),
          if (timeRead.isNotEmpty) ...[
            const SizedBox(height: 4),
            Text('Time read: $timeRead',
                style: const TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w700,
                    color: BookTheme.accent)),
          ],
          const SizedBox(height: 4),
          Text(
              'Publication Date : '
              '${book.published.isEmpty ? "—" : book.published}',
              style: TextStyle(fontSize: 12, color: b.inkDim)),
          if (book.series.isNotEmpty) ...[
            const SizedBox(height: 4),
            GestureDetector(
              onTap: () async {
                await controller.send(const BooksCmd.closeDetail());
                await controller.send(BooksCmd.setSeries(series: book.series));
              },
              child: Text('Series: ${book.series}',
                  style: const TextStyle(
                      fontSize: 12,
                      fontWeight: FontWeight.w600,
                      color: BookTheme.accent)),
            ),
          ],
          const SizedBox(height: 6),
          Row(
            children: [
              RatingPill(
                rating: book.rating,
                onRate: (v) =>
                    controller.send(BooksCmd.setRating(id: book.id, rating: v)),
              ),
              const SizedBox(width: 10),
              FavouriteDot(
                on: book.favorite,
                side: 26,
                onTap: () =>
                    controller.send(BooksCmd.toggleFavorite(id: book.id)),
              ),
              if (book.format.toUpperCase() == 'PDF') ...[
                const SizedBox(width: 10),
                _Toggle(
                  label: book.magazine ? '✓ Magazine' : 'Magazine',
                  on: book.magazine,
                  hue: BookTheme.amber,
                  onTap: () =>
                      controller.send(BooksCmd.toggleMagazine(id: book.id)),
                ),
              ],
              if (book.format.toUpperCase() == 'CBZ' ||
                  book.format.toUpperCase() == 'CBR') ...[
                const SizedBox(width: 10),
                _Toggle(
                  label: book.rtl ? '✓ RTL' : 'RTL',
                  on: book.rtl,
                  hue: BookTheme.pink,
                  onTap: () => controller.send(BooksCmd.toggleRtl(id: book.id)),
                ),
              ],
              if (book.netRating > 0) ...[
                const SizedBox(width: 10),
                Container(
                  height: 22,
                  padding: const EdgeInsets.symmetric(horizontal: 9),
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: BookTheme.amber.withValues(alpha: 0.16),
                    borderRadius: BorderRadius.circular(11),
                  ),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      const Icon(Icons.star, size: 12, color: BookTheme.amber),
                      const SizedBox(width: 4),
                      Text('${(book.netRating * 10).round() / 10} avg',
                          style: const TextStyle(
                              fontSize: 11,
                              fontWeight: FontWeight.w700,
                              color: Color(0xFFCF8A12))),
                    ],
                  ),
                ),
              ],
            ],
          ),
        ],
      ),
    );
  }
}

class _Toggle extends StatelessWidget {
  const _Toggle({
    required this.label,
    required this.on,
    required this.hue,
    required this.onTap,
  });

  final String label;
  final bool on;
  final Color hue;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => GestureDetector(
        onTap: onTap,
        child: MouseRegion(
          cursor: SystemMouseCursors.click,
          child: Container(
            height: 26,
            padding: const EdgeInsets.symmetric(horizontal: 10),
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: on ? hue : hue.withValues(alpha: 0.12),
              borderRadius: BorderRadius.circular(13),
              border: Border.all(color: hue.withValues(alpha: 0.5)),
            ),
            child: Text(label,
                style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w700,
                    color: on ? Colors.white : hue)),
          ),
        ),
      );
}

/// Full-width bar with the percentage at its right edge. The DB stores 0‥1
/// here, so nothing gets multiplied twice.
class _ProgressPill extends StatelessWidget {
  const _ProgressPill({required this.percent});

  final double percent;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return Container(
      height: 28,
      padding: const EdgeInsets.symmetric(horizontal: 12),
      decoration: BoxDecoration(
        color: BookTheme.accent.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: BookTheme.accent.withValues(alpha: 0.4)),
      ),
      child: Row(
        children: [
          Expanded(
            child: SizedBox(
              height: 8,
              child: Stack(
                children: [
                  Positioned.fill(
                    child: DecoratedBox(
                      decoration: BoxDecoration(
                        color: b.track,
                        borderRadius: BorderRadius.circular(4),
                      ),
                    ),
                  ),
                  // See the same fix in the hero card: loose constraints give
                  // a childless DecoratedBox zero height.
                  Positioned.fill(
                    child: FractionallySizedBox(
                      alignment: Alignment.centerLeft,
                      widthFactor: percent.clamp(0.0, 1.0),
                      heightFactor: 1,
                      child: const DecoratedBox(
                        decoration: BoxDecoration(
                          borderRadius: BorderRadius.all(Radius.circular(4)),
                          gradient: LinearGradient(
                              colors: [Color(0xFF6C4DF6), Color(0xFFB9B0FF)]),
                        ),
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
          const SizedBox(width: 10),
          Text('${(percent.clamp(0.0, 1.0) * 100).round()}% read',
              style: const TextStyle(
                  fontSize: 11,
                  fontWeight: FontWeight.w700,
                  color: BookTheme.accent)),
        ],
      ),
    );
  }
}

class _CollectionToggle extends StatelessWidget {
  const _CollectionToggle({required this.row, required this.onTap});

  final CollectionRow row;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return GestureDetector(
      onTap: onTap,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        child: Container(
          height: 26,
          constraints: const BoxConstraints(minWidth: 44),
          padding: const EdgeInsets.symmetric(horizontal: 10),
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: row.member ? BookTheme.accent : b.pillBg,
            borderRadius: BorderRadius.circular(13),
          ),
          child: Text('${row.member ? "✓ " : "+ "}${row.name}',
              style: TextStyle(
                  fontSize: 11,
                  fontWeight: FontWeight.w600,
                  color: row.member ? Colors.white : b.ink)),
        ),
      ),
    );
  }
}

class _Summary extends StatelessWidget {
  const _Summary({required this.controller, required this.book});

  final BooksController controller;
  final Book book;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final summary = controller.state?.detailSummary ?? '';
    final loading = controller.fetching == book.id;
    return Container(
      decoration: BoxDecoration(
        color: b.pillBg.withValues(alpha: 0.5),
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: BookTheme.accent.withValues(alpha: 0.35)),
      ),
      child: ListView(
        padding: const EdgeInsets.all(10),
        children: [
          if (loading)
            Row(
              children: [
                const SizedBox(
                  width: 12,
                  height: 12,
                  child: CircularProgressIndicator(
                      strokeWidth: 1.6, color: BookTheme.accent),
                ),
                const SizedBox(width: 8),
                Text('Looking this one up…',
                    style: TextStyle(fontSize: 12, color: b.inkDim)),
              ],
            )
          else ...[
            Text(summary.isEmpty ? 'No summary yet.' : summary,
                style: TextStyle(fontSize: 12, height: 1.5, color: b.ink)),
            if (summary.isEmpty) ...[
              const SizedBox(height: 6),
              MouseRegion(
                cursor: SystemMouseCursors.click,
                child: GestureDetector(
                  onTap: () => controller.fetchSummary(book.id),
                  child: const Text('Fetch from online →',
                      style: TextStyle(
                          fontSize: 12,
                          fontWeight: FontWeight.w700,
                          color: BookTheme.accent)),
                ),
              ),
            ],
          ],
        ],
      ),
    );
  }
}

class _Actions extends StatelessWidget {
  const _Actions({
    required this.controller,
    required this.book,
    required this.onAskDelete,
  });

  final BooksController controller;
  final Book book;
  final VoidCallback onAskDelete;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return Row(
      children: [
        Expanded(
          child: SizedBox(
            height: 42,
            child: FilledButton.icon(
              style: FilledButton.styleFrom(
                backgroundColor: BookTheme.accent,
                shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(11)),
              ),
              icon: Icon(book.missing ? Icons.link : Icons.menu_book_outlined,
                  size: 16),
              label: Text(book.missing ? 'Relink' : 'Open'),
              onPressed: () async {
                if (!book.missing) {
                  await controller.send(BooksCmd.openBook(id: book.id));
                  return;
                }
                final p = await pickFile(
                  label: 'Book',
                  extensions: const [
                    'epub',
                    'pdf',
                    'djvu',
                    'cbz',
                    'cbr',
                    'cb7',
                    'cbt',
                    'fb2',
                    'mobi',
                    'azw3',
                  ],
                );
                if (p != null) {
                  await controller.send(BooksCmd.relink(id: book.id, path: p));
                }
              },
            ),
          ),
        ),
        const SizedBox(width: 10),
        _SquareBtn(
          icon: Icons.refresh,
          tint: BookTheme.accent,
          onTap: () => controller.fetchSummary(book.id),
        ),
        const SizedBox(width: 10),
        if (book.trashed)
          _SquareBtn(
            icon: Icons.restore_from_trash,
            tint: BookTheme.green,
            onTap: () => controller.send(BooksCmd.restore(id: book.id)),
          )
        else
          _SquareBtn(
            icon: Icons.delete_outline,
            tint: BookTheme.danger,
            fill: b.card,
            onTap: onAskDelete,
          ),
      ],
    );
  }
}

class _SquareBtn extends StatelessWidget {
  const _SquareBtn({
    required this.icon,
    required this.tint,
    required this.onTap,
    this.fill,
  });

  final IconData icon;
  final Color tint;
  final VoidCallback onTap;
  final Color? fill;

  @override
  Widget build(BuildContext context) => GestureDetector(
        onTap: onTap,
        child: MouseRegion(
          cursor: SystemMouseCursors.click,
          child: Container(
            width: 46,
            height: 42,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: fill ?? tint.withValues(alpha: 0.12),
              borderRadius: BorderRadius.circular(11),
              border: Border.all(color: tint.withValues(alpha: 0.4)),
            ),
            child: Icon(icon, size: 16, color: tint),
          ),
        ),
      );
}

class _ConfirmDelete extends StatelessWidget {
  const _ConfirmDelete({
    required this.title,
    required this.onCancel,
    required this.onRemove,
  });

  final String title;
  final VoidCallback onCancel;
  final VoidCallback onRemove;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text('Move “$title” to the Trash tab? The file on disk is kept.',
            style: TextStyle(fontSize: 12, color: b.ink)),
        const SizedBox(height: 8),
        Row(
          children: [
            Expanded(
              child: SizedBox(
                height: 40,
                child: OutlinedButton(
                  onPressed: onCancel,
                  child: const Text('Cancel'),
                ),
              ),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: SizedBox(
                height: 40,
                child: FilledButton(
                  style:
                      FilledButton.styleFrom(backgroundColor: BookTheme.danger),
                  onPressed: onRemove,
                  child: const Text('Remove'),
                ),
              ),
            ),
          ],
        ),
      ],
    );
  }
}

/// One line of text, with a title and a hint. Returns the trimmed value, or
/// null when cancelled or left empty.
///
/// A typed path rather than a native chooser, for the same reason Photos and
/// Music do it: the shell (phase 04) owns file dialogs for every section.
Future<String?> promptText(
  BuildContext context, {
  required String title,
  required String label,
  String hint = '',
  String confirm = 'OK',
  String initial = '',
}) async {
  final text = TextEditingController(text: initial);
  final value = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(title),
      content: SizedBox(
        width: 460,
        child: TextField(
          controller: text,
          autofocus: true,
          decoration: InputDecoration(labelText: label, hintText: hint),
          onSubmitted: (v) => Navigator.pop(ctx, v),
        ),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx), child: const Text('Cancel')),
        FilledButton(
            onPressed: () => Navigator.pop(ctx, text.text),
            child: Text(confirm)),
      ],
    ),
  );
  final trimmed = value?.trim() ?? '';
  return trimmed.isEmpty ? null : trimmed;
}

Future<bool> confirmAction(
  BuildContext context, {
  required String title,
  required String body,
  String action = 'Delete',
}) async {
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(title),
      content: SizedBox(width: 420, child: Text(body)),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Cancel')),
        FilledButton(
            onPressed: () => Navigator.pop(ctx, true), child: Text(action)),
      ],
    ),
  );
  return ok ?? false;
}

/// The watched folders, and a way to stop watching one.
Future<void> showFolders(
    BuildContext context, BooksController controller) async {
  await showDialog<void>(
    context: context,
    builder: (ctx) => AnimatedBuilder(
      animation: controller,
      builder: (ctx, _) {
        final folders = controller.state?.folders ?? const <String>[];
        return AlertDialog(
          title: const Text('Watched folders'),
          content: SizedBox(
            width: 520,
            child: folders.isEmpty
                ? const Text('No folders are being watched yet.')
                : Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      for (final f in folders)
                        ListTile(
                          dense: true,
                          title: Text(f,
                              maxLines: 1, overflow: TextOverflow.ellipsis),
                          trailing: IconButton(
                            icon: const Icon(Icons.close, size: 16),
                            tooltip: 'Stop watching',
                            onPressed: () =>
                                controller.send(BooksCmd.removeFolder(path: f)),
                          ),
                        ),
                    ],
                  ),
          ),
          actions: [
            FilledButton(
                onPressed: () => Navigator.pop(ctx), child: const Text('Done')),
          ],
        );
      },
    ),
  );
}
