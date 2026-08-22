// The detail sheet, and the three small dialogs the section needs.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/books.dart';
import 'books_controller.dart';
import 'books_widgets.dart';

/// One book, everything known about it, and everything that can be done to it.
class BookDetailPanel extends StatelessWidget {
  const BookDetailPanel({
    super.key,
    required this.controller,
    required this.book,
  });

  final BooksController controller;
  final Book book;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = controller.state!;

    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 18, 24, 40),
      children: [
        Row(
          children: [
            TextButton.icon(
              icon: const Icon(Icons.arrow_back, size: 18),
              label: const Text('Back to the shelf'),
              onPressed: () => controller.send(const BooksCmd.closeDetail()),
            ),
            const Spacer(),
            if (!book.missing)
              FilledButton.icon(
                style: FilledButton.styleFrom(backgroundColor: Tokens.secBooks),
                icon: Icon(book.percent > 0
                    ? Icons.play_arrow
                    : Icons.auto_stories_outlined),
                label: Text(book.percent > 0 ? 'Resume' : 'Start reading'),
                onPressed: () =>
                    controller.send(BooksCmd.openBook(id: book.id)),
              ),
          ],
        ),
        const SizedBox(height: 16),
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            BookCover(
                controller: controller, book: book, width: 170, radius: 12),
            const SizedBox(width: 24),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(book.title,
                      style: TextStyle(
                          fontSize: 26,
                          fontWeight: FontWeight.w800,
                          color: t.nInk)),
                  const SizedBox(height: 4),
                  InkWell(
                    onTap: () async {
                      await controller
                          .send(BooksCmd.setAuthor(author: book.author));
                      await controller.send(const BooksCmd.closeDetail());
                    },
                    child: Text(book.author,
                        style: const TextStyle(
                            fontSize: 15, color: Tokens.secBooks)),
                  ),
                  const SizedBox(height: 12),
                  Wrap(
                    spacing: 8,
                    runSpacing: 6,
                    children: [
                      if (book.series.isNotEmpty)
                        _Fact(label: 'Series', value: book.series),
                      if (book.genre.isNotEmpty)
                        _Fact(label: 'Genre', value: book.genre),
                      if (book.published.isNotEmpty)
                        _Fact(label: 'Published', value: book.published),
                      _Fact(label: 'Format', value: book.format.toUpperCase()),
                      if (book.sizeBytes > 0)
                        _Fact(label: 'Size', value: fmtSize(book.sizeBytes)),
                      if (book.addedAt > 0)
                        _Fact(label: 'Added', value: fmtDate(book.addedAt)),
                      if (book.lastRead > 0)
                        _Fact(
                            label: 'Last read', value: fmtDate(book.lastRead)),
                      if (book.timeRead > 0)
                        _Fact(
                            label: 'Time read',
                            value: fmtReadTime(book.timeRead)),
                    ],
                  ),
                  const SizedBox(height: 16),
                  Row(
                    children: [
                      Text('Your rating',
                          style: TextStyle(fontSize: 12, color: t.nInk2)),
                      const SizedBox(width: 10),
                      Stars(
                        value: book.rating,
                        size: 22,
                        onRate: (v) => controller
                            .send(BooksCmd.setRating(id: book.id, rating: v)),
                      ),
                      if (book.netRating > 0) ...[
                        const SizedBox(width: 20),
                        Text('Readers',
                            style: TextStyle(fontSize: 12, color: t.nInk2)),
                        const SizedBox(width: 8),
                        Stars(value: book.netRating, size: 16),
                        const SizedBox(width: 6),
                        Text(book.netRating.toStringAsFixed(1),
                            style: TextStyle(fontSize: 12, color: t.nInk3)),
                      ],
                    ],
                  ),
                  const SizedBox(height: 16),
                  if (book.percent > 0) ...[
                    ClipRRect(
                      borderRadius: BorderRadius.circular(3),
                      child: LinearProgressIndicator(
                        value: book.percent.clamp(0.0, 1.0),
                        minHeight: 6,
                        backgroundColor: t.nHover,
                        valueColor:
                            const AlwaysStoppedAnimation(Tokens.secBooks),
                      ),
                    ),
                    const SizedBox(height: 4),
                    Text('${(book.percent * 100).round()}% read',
                        style: TextStyle(fontSize: 11, color: t.nInk2)),
                    const SizedBox(height: 16),
                  ],
                  Wrap(
                    spacing: 8,
                    runSpacing: 8,
                    children: [
                      OutlinedButton.icon(
                        icon: Icon(
                          book.favorite
                              ? Icons.favorite
                              : Icons.favorite_border,
                          size: 16,
                        ),
                        label: Text(
                            book.favorite ? 'Favourited' : 'Add to favourites'),
                        onPressed: () => controller
                            .send(BooksCmd.toggleFavorite(id: book.id)),
                      ),
                      OutlinedButton.icon(
                        icon:
                            const Icon(Icons.cloud_download_outlined, size: 16),
                        label: const Text('Fetch description'),
                        onPressed: () =>
                            controller.send(BooksCmd.fetchSummary(id: book.id)),
                      ),
                      if (book.missing)
                        OutlinedButton.icon(
                          icon: const Icon(Icons.link, size: 16),
                          label: const Text('Relink the file'),
                          onPressed: () async {
                            final p = await promptText(
                              context,
                              title: 'Where is it now?',
                              label: 'New absolute path',
                              confirm: 'Relink',
                            );
                            if (p != null) {
                              await controller
                                  .send(BooksCmd.relink(id: book.id, path: p));
                            }
                          },
                        ),
                      OutlinedButton.icon(
                        icon: Icon(
                            book.trashed
                                ? Icons.restore_from_trash
                                : Icons.delete_outline,
                            size: 16),
                        label: Text(book.trashed ? 'Restore' : 'Move to trash'),
                        onPressed: () => controller.send(book.trashed
                            ? BooksCmd.restore(id: book.id)
                            : BooksCmd.trash(id: book.id)),
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ],
        ),
        const SizedBox(height: 26),
        if (st.detailSummary.isNotEmpty) ...[
          Text('Description',
              style: TextStyle(
                  fontSize: 14, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(height: 8),
          Text(st.detailSummary,
              style: TextStyle(fontSize: 13, height: 1.55, color: t.nInk2)),
          const SizedBox(height: 26),
        ],
        Row(
          children: [
            Text('Collections',
                style: TextStyle(
                    fontSize: 14, fontWeight: FontWeight.w700, color: t.nInk)),
            const SizedBox(width: 12),
            TextButton.icon(
              icon: const Icon(Icons.add, size: 16),
              label: const Text('New'),
              onPressed: () async {
                final name = await promptText(
                  context,
                  title: 'New collection',
                  label: 'Name',
                  confirm: 'Create',
                );
                if (name != null) {
                  await controller.send(BooksCmd.collectionCreate(name: name));
                }
              },
            ),
          ],
        ),
        const SizedBox(height: 8),
        if (st.detailCollections.isEmpty)
          Text('No collections yet.',
              style: TextStyle(fontSize: 12, color: t.nInk2))
        else
          Wrap(
            children: [
              for (final c in st.detailCollections)
                BookChipView(
                  label: c.name,
                  count: c.count,
                  active: c.member,
                  onTap: () => controller.send(
                      BooksCmd.collectionToggle(id: c.id, bookId: book.id)),
                ),
            ],
          ),
      ],
    );
  }
}

class _Fact extends StatelessWidget {
  const _Fact({required this.label, required this.value});

  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(6),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text('$label ', style: TextStyle(fontSize: 10, color: t.nInk3)),
          Text(value,
              style: TextStyle(
                  fontSize: 11, fontWeight: FontWeight.w600, color: t.nInk)),
        ],
      ),
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
