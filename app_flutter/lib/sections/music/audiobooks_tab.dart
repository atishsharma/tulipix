// Audiobooks — the shelf, and the book page with its chapters and bookmarks.
//
// A "book" is a folder whose tracks carry `is_audiobook`. Nothing is copied or
// re-indexed to make one: flagging a folder is a column update, and unflagging
// it puts the tracks straight back into My Music. That is why the shelf is
// built from `book_folders` rather than a table of its own.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_widgets.dart';

class AudiobooksTab extends StatelessWidget {
  const AudiobooksTab({super.key, required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    if (st == null) return const Center(child: CircularProgressIndicator());
    if (st.bookDetailOpen) return _BookPage(controller: controller, st: st);

    return Column(
      children: [
        SizedBox(
          height: 52,
          child: Row(
            children: [
              const SizedBox(width: 20),
              for (final tab in const [
                ('all', 'All'),
                ('progress', 'In progress'),
                ('finished', 'Finished'),
              ])
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 4),
                  child: MusicChip(
                    label: tab.$2,
                    active: st.bookTab == tab.$1,
                    tint: const Color(0xFF10B981),
                    tint2: const Color(0xFF14B8A6),
                    onTap: () =>
                        controller.send(MusicCmd.bookSetTab(name: tab.$1)),
                  ),
                ),
            ],
          ),
        ),
        Expanded(
          child: st.books.isEmpty
              ? MusicEmpty(
                  icon: Icons.auto_stories_outlined,
                  title: st.bookTab == 'all'
                      ? 'No audiobooks yet'
                      : 'Nothing in this shelf',
                  body: 'Mark a folder as an audiobook from My Music → '
                      'Folders and its chapters move here, with their own '
                      'position, speed and bookmarks.',
                  action: st.bookTab == 'all'
                      ? null
                      : (
                          'Show all',
                          () => controller
                              .send(const MusicCmd.bookSetTab(name: 'all'))
                        ),
                )
              : CardGrid(
                  min: 190,
                  children: [
                    for (final b in st.books)
                      MusicCard(
                        controller: controller,
                        title: b.title,
                        subtitle: b.author.isEmpty
                            ? '${b.chapters} chapters'
                            : b.author,
                        artKind: 'book',
                        artKey: b.folder,
                        direct: b.art,
                        fallback: Icons.menu_book,
                        progress: b.progress,
                        badge: b.finished ? 'Finished' : null,
                        onTap: () => controller
                            .send(MusicCmd.bookOpen(folder: b.folder)),
                      ),
                  ],
                ),
        ),
      ],
    );
  }
}

class _BookPage extends StatelessWidget {
  const _BookPage({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final book = st.bookDetail;
    final chapters = st.bookChapters;
    final resume = st.bookResumeIndex;

    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
          child: Row(
            children: [
              IconButton(
                icon: const Icon(Icons.arrow_back),
                tooltip: 'Back to the shelf',
                onPressed: () => controller.send(const MusicCmd.bookBack()),
              ),
              const Spacer(),
              TextButton.icon(
                icon: Icon(
                  book?.finished ?? false
                      ? Icons.check_circle
                      : Icons.check_circle_outline,
                  size: 16,
                ),
                label: Text(
                    book?.finished ?? false ? 'Finished' : 'Mark finished'),
                onPressed: () => controller.send(
                  MusicCmd.bookSetFinished(
                      finished: !(book?.finished ?? false)),
                ),
              ),
            ],
          ),
        ),
        if (book != null)
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 8, 24, 16),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: [
                MusicArt(
                  controller: controller,
                  kind: 'book',
                  artKey: book.folder,
                  direct: book.art,
                  size: 148,
                  radius: 10,
                  fallback: Icons.menu_book,
                ),
                const SizedBox(width: 20),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(book.title,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 24,
                              fontWeight: FontWeight.w700,
                              color: t.nInk)),
                      Text(
                        [
                          if (book.author.isNotEmpty) book.author,
                          '${book.chapters} chapters',
                          if (book.totalS > 0) fmtClock(book.totalS),
                        ].join(' · '),
                        style: TextStyle(fontSize: 12, color: t.nInk2),
                      ),
                      const SizedBox(height: 12),
                      Row(
                        children: [
                          FilledButton.icon(
                            icon: const Icon(Icons.play_arrow, size: 18),
                            label: Text(resume >= 0 ? 'Resume' : 'Start'),
                            onPressed: chapters.isEmpty
                                ? null
                                : () => controller.send(
                                      MusicCmd.bookPlay(
                                        // Resume means the chapter that was
                                        // left part-heard; with none, the book
                                        // starts at the beginning.
                                        itemId: chapters[resume >= 0 &&
                                                    resume < chapters.length
                                                ? resume
                                                : 0]
                                            .itemId,
                                      ),
                                    ),
                          ),
                          const SizedBox(width: 8),
                          OutlinedButton.icon(
                            icon: const Icon(Icons.bookmark_add_outlined,
                                size: 18),
                            label: const Text('Bookmark here'),
                            onPressed: controller.now?.mode == 'book'
                                ? () => _addBookmark(context, controller)
                                : null,
                          ),
                        ],
                      ),
                      if (book.progress > 0) ...[
                        const SizedBox(height: 12),
                        SizedBox(
                          width: 320,
                          child: LinearProgressIndicator(
                            value: book.progress.clamp(0.0, 1.0),
                            minHeight: 4,
                            backgroundColor: t.nHair,
                            valueColor:
                                const AlwaysStoppedAnimation(Color(0xFF10B981)),
                          ),
                        ),
                      ],
                    ],
                  ),
                ),
              ],
            ),
          ),
        Expanded(
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                flex: 3,
                child: ListView.builder(
                  itemCount: chapters.length,
                  itemBuilder: (_, i) => _ChapterRow(
                    controller: controller,
                    chapter: chapters[i],
                    index: i,
                  ),
                ),
              ),
              Container(width: 1, color: t.nHair),
              SizedBox(
                width: 280,
                child: _Bookmarks(controller: controller, st: st),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

class _ChapterRow extends StatelessWidget {
  const _ChapterRow({
    required this.controller,
    required this.chapter,
    required this.index,
  });

  final MusicController controller;
  final Chapter chapter;
  final int index;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = chapter;
    final playing =
        controller.now?.mode == 'book' && controller.now?.itemId == c.itemId;
    final part = c.durationS > 0 ? (c.positionS / c.durationS) : 0.0;
    return ListTile(
      dense: true,
      leading: SizedBox(
        width: 28,
        child: playing
            ? const Icon(Icons.equalizer, size: 16, color: Color(0xFF10B981))
            : c.finished
                ? const Icon(Icons.check, size: 16, color: Tokens.ok)
                : Text('${index + 1}',
                    textAlign: TextAlign.center,
                    style: TextStyle(fontSize: 12, color: t.nInk2)),
      ),
      title: Text(
        c.title,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: TextStyle(
          fontSize: 13,
          fontWeight: playing ? FontWeight.w600 : FontWeight.w400,
          color: playing ? const Color(0xFF10B981) : t.nInk,
        ),
      ),
      subtitle: part > 0.01 && part < 0.99
          ? LinearProgressIndicator(
              value: part,
              minHeight: 2,
              backgroundColor: t.nHair,
              valueColor: const AlwaysStoppedAnimation(Color(0xFF10B981)),
            )
          : null,
      trailing: Text(fmtClock(c.durationS),
          style: TextStyle(fontSize: 12, color: t.nInk2)),
      onTap: () => controller.send(MusicCmd.bookPlay(itemId: c.itemId)),
    );
  }
}

class _Bookmarks extends StatelessWidget {
  const _Bookmarks({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final marks = st.bookBookmarks;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 16, 16, 8),
          child: Text('Bookmarks',
              style: TextStyle(
                  fontSize: 14, fontWeight: FontWeight.w700, color: t.nInk)),
        ),
        if (marks.isEmpty)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16),
            child: Text(
              'Nothing marked yet. While the book is playing, “Bookmark here” '
              'saves the exact second.',
              style: TextStyle(fontSize: 12, color: t.nInk2),
            ),
          ),
        Expanded(
          child: ListView.builder(
            itemCount: marks.length,
            itemBuilder: (_, i) => ListTile(
              dense: true,
              leading: const Icon(Icons.bookmark, size: 16),
              title: Text(
                marks[i].label.isEmpty ? marks[i].when : marks[i].label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(fontSize: 12),
              ),
              subtitle: Text(marks[i].when,
                  style: TextStyle(fontSize: 11, color: t.nInk2)),
              onTap: () => controller.send(MusicCmd.bookmarkJump(index: i)),
              onLongPress: () =>
                  _renameBookmark(context, controller, i, marks[i].label),
              trailing: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  IconButton(
                    iconSize: 16,
                    icon: const Icon(Icons.edit_outlined),
                    tooltip: 'Rename',
                    onPressed: () =>
                        _renameBookmark(context, controller, i, marks[i].label),
                  ),
                  IconButton(
                    iconSize: 16,
                    icon: const Icon(Icons.close),
                    tooltip: 'Remove',
                    onPressed: () =>
                        controller.send(MusicCmd.bookmarkRemove(index: i)),
                  ),
                ],
              ),
            ),
          ),
        ),
      ],
    );
  }
}

Future<void> _renameBookmark(
  BuildContext context,
  MusicController c,
  int index,
  String current,
) async {
  final text = TextEditingController(text: current);
  final label = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Rename bookmark'),
      content: TextField(
        controller: text,
        autofocus: true,
        decoration: const InputDecoration(labelText: 'Label'),
        onSubmitted: (v) => Navigator.pop(ctx, v),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx), child: const Text('Cancel')),
        FilledButton(
          onPressed: () => Navigator.pop(ctx, text.text),
          child: const Text('Rename'),
        ),
      ],
    ),
  );
  if (label != null) {
    await c.send(MusicCmd.bookmarkRename(index: index, label: label.trim()));
  }
}

Future<void> _addBookmark(BuildContext context, MusicController c) async {
  final text = TextEditingController();
  final label = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Bookmark this moment'),
      content: TextField(
        controller: text,
        autofocus: true,
        decoration: const InputDecoration(
          labelText: 'Label (optional)',
          hintText: 'the bit about the lighthouse',
        ),
        onSubmitted: (v) => Navigator.pop(ctx, v),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx), child: const Text('Cancel')),
        FilledButton(
          onPressed: () => Navigator.pop(ctx, text.text),
          child: const Text('Save'),
        ),
      ],
    ),
  );
  if (label != null) await c.send(MusicCmd.bookmarkAdd(label: label.trim()));
}
