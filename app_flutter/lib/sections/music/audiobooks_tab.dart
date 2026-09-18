// Audiobooks — Home, the shelf, authors and series, and the book page.
//
// A "book" is a folder whose tracks carry `is_audiobook`. Nothing is copied or
// re-indexed to make one: flagging a folder is a column update, and unflagging
// it puts the tracks straight back into My Music. That is why the shelf is
// built from `book_folders` rather than a table of its own.
//
// The shape is docs/listen-deck.html. Every figure on this tab is time LEFT at
// the speed the book is actually heard at, not a percentage: "4 h 12 min left
// at 1.25×" is an answer, "46%" is a fact about a bar. Bookmarks, the stats
// card and the per-listener switches live in the docked panel (side_panel.dart)
// beside whatever page is open.

import 'package:flutter/material.dart';

import '../../platform/pick.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_widgets.dart';

/// The emerald→teal pair ui/page_music.slint gives the audiobook sub-tabs.
const Color kBookTint = Color(0xFF10B981);
const Color kBookTint2 = Color(0xFF14B8A6);

/// Seconds still to hear, at the speed this book is played at. The one figure
/// every surface on this tab quotes, so it is computed in one place.
double bookLeftS(BookCard b) {
  final speed = b.speed > 0 ? b.speed : 1.0;
  return (b.totalS * (1 - b.progress.clamp(0.0, 1.0))) / speed;
}

class AudiobooksTab extends StatelessWidget {
  const AudiobooksTab({super.key, required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    if (st == null) return const Center(child: CircularProgressIndicator());

    return Column(
      children: [
        _SubTabs(controller: controller, st: st),
        Expanded(
          // A search is a question about the whole shelf, so it outranks the
          // tab and the open book: answering it under the book page would put
          // the answers where nobody could see them.
          child: st.bookQuery.isNotEmpty
              ? _Hits(controller: controller, st: st)
              : st.bookDetailOpen
              ? _BookPage(
                  key: ValueKey(st.bookDetail?.folder ?? ''),
                  controller: controller,
                  st: st,
                )
              : st.books.isEmpty
                  ? _Empty(controller: controller, st: st)
                  : switch (st.bookTab) {
                      'series' => _Series(controller: controller, st: st),
                      'folders' =>
                        _FolderList(controller: controller, books: st.books),
                      'home' => _Home(controller: controller, st: st),
                      _ => _Shelf(controller: controller, st: st),
                    },
        ),
      ],
    );
  }
}

class _SubTabs extends StatelessWidget {
  const _SubTabs({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final going =
        st.books.where((b) => b.progress > 0 && !b.finished).length;
    final done = st.books.where((b) => b.finished).length;
    return SizedBox(
      height: 52,
      child: Row(
        children: [
          const SizedBox(width: 20),
          for (final tab in [
            ('home', 'Home', Icons.home_outlined, null),
            (
              'all',
              'All',
              Icons.menu_book_outlined,
              st.books.isEmpty ? null : '${st.books.length}'
            ),
            (
              'progress',
              'In progress',
              Icons.play_circle_outline,
              going > 0 ? '$going' : null
            ),
            ('finished', 'Finished', Icons.check_circle_outline,
                done > 0 ? '$done' : null),
            ('series', 'Authors & series', Icons.people_outline, null),
            ('folders', 'Folders', Icons.folder_open, null),
          ])
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 4),
              child: MusicChip(
                label: tab.$2,
                icon: tab.$3,
                active: st.bookTab == tab.$1 && !st.bookDetailOpen,
                tint: kBookTint,
                tint2: kBookTint2,
                badge: tab.$4,
                // One command: picking a tab leaves the book page on the
                // bridge's side. Two sends raced and the loser's snapshot won.
                onTap: () =>
                    controller.send(MusicCmd.bookSetTab(name: tab.$1)),
              ),
            ),
          const Spacer(),
          TextButton.icon(
            icon: const Icon(Icons.create_new_folder_outlined, size: 16),
            label: const Text('Add a shelf'),
            onPressed: controller.addFolder,
          ),
          IconButton(
            iconSize: 18,
            tooltip: 'Bookmarks and stats (Q)',
            isSelected: controller.panel == 'queue',
            icon: const Icon(Icons.bookmarks_outlined),
            onPressed: () => controller.setPanel('queue'),
          ),
          const SizedBox(width: 12),
          Container(width: 1, height: 24, color: t.outline),
          const SizedBox(width: 12),
        ],
      ),
    );
  }
}

class _Empty extends StatelessWidget {
  const _Empty({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) => MusicEmpty(
        icon: Icons.auto_stories_outlined,
        title: st.bookTab == 'all' || st.bookTab == 'home'
            ? 'No audiobooks yet'
            : 'Nothing on this shelf',
        body: st.bookTab == 'all' || st.bookTab == 'home'
            ? 'Add a folder here and everything under it becomes a shelf — '
                'one book per sub-folder, each with its own position, speed '
                'and bookmarks, and none of it in My Music. A folder already '
                'in My Music can be moved across from its tile menu instead.'
            : 'Nothing has reached this shelf yet.',
        action: st.bookTab == 'all' || st.bookTab == 'home'
            ? ('Add a folder', controller.addFolder)
            : (
                'Show all',
                () => controller.send(const MusicCmd.bookSetTab(name: 'all'))
              ),
      );
}

/// What the header box found: books, their chapters, and every bookmark whose
/// label or note matches. Three kinds in one list because they are one
/// question — you do not know in advance whether "lighthouse" is a book, a
/// chapter or the note you left on one.
class _Hits extends StatelessWidget {
  const _Hits({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final hits = st.bookHits;
    if (hits.isEmpty) {
      return MusicEmpty(
        icon: Icons.search_off,
        title: 'Nothing matches “${st.bookQuery}”',
        body: 'The box reaches book titles, authors, narrators, series, '
            'chapter names and what you wrote on a bookmark.',
        action: (
          'Clear the search',
          () => controller.send(const MusicCmd.bookSearch(query: ''))
        ),
      );
    }
    const kinds = [
      ('book', 'Books', Icons.menu_book_outlined),
      ('chapter', 'Chapters', Icons.list_alt_outlined),
      ('bookmark', 'Bookmarks', Icons.bookmark_outline),
    ];
    return ListView(
      children: [
        for (final k in kinds)
          if (hits.any((h) => h.kind == k.$1)) ...[
            _Head(
              title: k.$2,
              hint: '${hits.where((h) => h.kind == k.$1).length}',
            ),
            for (final h in hits.where((h) => h.kind == k.$1))
              ListTile(
                dense: true,
                leading: Icon(k.$3, size: 18, color: t.nInk3),
                title: Text(h.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 13, color: t.nInk)),
                subtitle: Text(h.subtitle,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11, color: t.nInk2)),
                onTap: () => _openHit(controller, h),
              ),
          ],
        const SizedBox(height: 24),
      ],
    );
  }
}

/// A hit opens the book it belongs to; a chapter or a bookmark plays from it.
/// The search closes either way — it has been answered.
void _openHit(MusicController c, BookHit hit) {
  c.send(const MusicCmd.bookSearch(query: ''));
  c.send(MusicCmd.bookOpen(folder: hit.folder));
  if (hit.itemId == 0) return;
  c.send(MusicCmd.bookPlay(itemId: hit.itemId));
  if (hit.positionS >= 0) c.send(MusicCmd.seek(secs: hit.positionS));
}

// ------------------------------------------------------------------- Home --

class _Home extends StatelessWidget {
  const _Home({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // In progress, the one touched most recently first. A book never started
    // has `lastPlayed == 0`, which is why it cannot be in this list at all.
    final going = st.books.where((b) => b.progress > 0 && !b.finished).toList()
      ..sort((a, b) => b.lastPlayed.compareTo(a.lastPlayed));
    final hero = going.isEmpty ? null : going.first;
    // The next unstarted book of a series you are part-way through. Read off
    // the shelf rather than asked for: "next" means the lowest number nobody
    // has opened, in a series that already has a finished book in it.
    final next = nextInSeries(st.books);
    final fresh = st.books.toList()
      ..sort((a, b) => b.added.compareTo(a.added));

    return ListView(
      children: [
        if (hero != null)
          _Resume(controller: controller, book: hero)
        else
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 24, 24, 0),
            child: Text(
              'Nothing started yet. Open a book and the shelf remembers where '
              'you stopped, chapter by chapter.',
              style: TextStyle(fontSize: 12.5, color: t.nInk2),
            ),
          ),
        if (going.length > 1) ...[
          _Head(
            title: 'Continue',
            hint: '${going.length - 1} more in progress',
            action: (
              'In progress',
              () => controller.send(const MusicCmd.bookSetTab(name: 'progress'))
            ),
          ),
          SizedBox(
            height: 88,
            child: ListView.separated(
              scrollDirection: Axis.horizontal,
              padding: const EdgeInsets.symmetric(horizontal: 24),
              itemCount: going.length - 1,
              separatorBuilder: (_, __) => const SizedBox(width: 12),
              itemBuilder: (_, i) =>
                  _ContinueCard(controller: controller, book: going[i + 1]),
            ),
          ),
        ],
        if (next != null) ...[
          _Head(
            title: 'Next in ${next.series}',
            hint: 'you have finished the ones before it',
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 0, 24, 0),
            child: _BookRow(controller: controller, book: next),
          ),
        ],
        _Head(
          title: 'Recently added',
          action: (
            'All books',
            () => controller.send(const MusicCmd.bookSetTab(name: 'all'))
          ),
        ),
        CardGrid(
          inList: true,
          padding: const EdgeInsets.fromLTRB(24, 8, 24, 24),
          min: 190,
          children: [
            for (final b in fresh.take(12))
              _BookCardTile(controller: controller, book: b),
          ],
        ),
      ],
    );
  }
}

/// The next unstarted book of a series with finished books in it, lowest
/// number first. Null when no series on the shelf is part-way through.
BookCard? nextInSeries(List<BookCard> books) {
  final done = <String>{
    for (final b in books)
      if (b.series.isNotEmpty && b.finished) b.series,
  };
  final candidates = books
      .where((b) => done.contains(b.series) && !b.finished && b.progress == 0)
      .toList()
    ..sort((a, b) => a.seriesNo.compareTo(b.seriesNo));
  return candidates.isEmpty ? null : candidates.first;
}

/// Home's hero: the book you were last in, with everything needed to decide
/// whether to carry on now — where you are, how much is left at your speed,
/// and how much of the open chapter is left.
class _Resume extends StatelessWidget {
  const _Resume({required this.controller, required this.book});

  final MusicController controller;
  final BookCard book;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final b = book;
    final chapterS = b.chapters > 0 ? b.totalS / b.chapters : 0.0;
    final speed = b.speed > 0 ? b.speed : 1.0;
    return Container(
      margin: const EdgeInsets.fromLTRB(24, 16, 24, 0),
      padding: const EdgeInsets.all(18),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(Tokens.radiusLg),
        border: Border.all(color: t.nHair),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          MusicArt(
            controller: controller,
            kind: 'book',
            artKey: b.folder,
            direct: b.art,
            size: 132,
            radius: 12,
            fallback: Icons.menu_book,
          ),
          const SizedBox(width: 20),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  'PICK UP WHERE YOU STOPPED'
                  '${b.lastPlayed > 0 ? '  ·  ${fmtAgo(b.lastPlayed).toUpperCase()}' : ''}',
                  style: TextStyle(
                      fontSize: 10,
                      letterSpacing: 1.1,
                      fontWeight: FontWeight.w700,
                      color: t.nInk3),
                ),
                const SizedBox(height: 4),
                Text(b.title,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 22,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
                Text(
                  [
                    if (b.author.isNotEmpty) b.author,
                    if (b.narrator.isNotEmpty) 'read by ${b.narrator}',
                  ].join(' · '),
                  style: TextStyle(fontSize: 12.5, color: t.nInk2),
                ),
                const SizedBox(height: 12),
                Wrap(
                  spacing: 22,
                  runSpacing: 8,
                  children: [
                    _Stat(
                        value: '${b.chapterNow} / ${b.chapters}',
                        label: 'chapter'),
                    _Stat(
                        value: '${(b.progress * 100).round()}%',
                        label: 'through'),
                    _Stat(
                        value: fmtMins(bookLeftS(b)), label: 'left at $speed×'),
                    if (chapterS > 0)
                      _Stat(
                        value: fmtClock(chapterS / speed),
                        label: 'a chapter',
                      ),
                  ],
                ),
                const SizedBox(height: 14),
                Wrap(
                  spacing: 8,
                  runSpacing: 6,
                  children: [
                    FilledButton.icon(
                      icon: const Icon(Icons.play_arrow, size: 18),
                      label: Text('Resume chapter ${b.chapterNow}'),
                      onPressed: () =>
                          controller.send(MusicCmd.bookOpen(folder: b.folder)),
                    ),
                    OutlinedButton.icon(
                      icon: const Icon(Icons.list_alt_outlined, size: 16),
                      label: const Text('Chapters & bookmarks'),
                      onPressed: () =>
                          controller.send(MusicCmd.bookOpen(folder: b.folder)),
                    ),
                    TextButton.icon(
                      icon: const Icon(Icons.bedtime_outlined, size: 16),
                      label: const Text('Sleep at chapter end'),
                      // -1 is the bridge's "after this track", which for a
                      // book is this chapter.
                      onPressed: () =>
                          controller.send(const MusicCmd.setSleep(minutes: -1)),
                    ),
                  ],
                ),
                const SizedBox(height: 12),
                ClipRRect(
                  borderRadius: BorderRadius.circular(3),
                  child: LinearProgressIndicator(
                    value: b.progress.clamp(0.0, 1.0),
                    minHeight: 5,
                    backgroundColor: t.nTile,
                    valueColor: const AlwaysStoppedAnimation(kBookTint),
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _Stat extends StatelessWidget {
  const _Stat({required this.value, required this.label});

  final String value;
  final String label;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(value,
            style: TextStyle(
                fontSize: 14, fontWeight: FontWeight.w700, color: t.nInk)),
        Text(label, style: TextStyle(fontSize: 10.5, color: t.nInk3)),
      ],
    );
  }
}

class _Head extends StatelessWidget {
  const _Head({required this.title, this.hint = '', this.action});

  final String title;
  final String hint;
  final (String, VoidCallback)? action;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 6),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.baseline,
        textBaseline: TextBaseline.alphabetic,
        children: [
          Text(title,
              style: TextStyle(
                  fontSize: 15, fontWeight: FontWeight.w700, color: t.nInk)),
          if (hint.isNotEmpty) ...[
            const SizedBox(width: 10),
            Text(hint, style: TextStyle(fontSize: 11.5, color: t.nInk3)),
          ],
          const Spacer(),
          if (action != null)
            TextButton(onPressed: action!.$2, child: Text(action!.$1)),
        ],
      ),
    );
  }
}

class _ContinueCard extends StatelessWidget {
  const _ContinueCard({required this.controller, required this.book});

  final MusicController controller;
  final BookCard book;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final b = book;
    return SizedBox(
      width: 300,
      child: Material(
        color: t.nCard,
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        child: InkWell(
          borderRadius: BorderRadius.circular(Tokens.radiusMd),
          onTap: () => controller.send(MusicCmd.bookOpen(folder: b.folder)),
          child: Padding(
            padding: const EdgeInsets.all(10),
            child: Row(
              children: [
                MusicArt(
                  controller: controller,
                  kind: 'book',
                  artKey: b.folder,
                  direct: b.art,
                  size: 52,
                  radius: 8,
                  fallback: Icons.menu_book,
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(b.title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 12.5,
                              fontWeight: FontWeight.w600,
                              color: t.nInk)),
                      const SizedBox(height: 2),
                      Text(
                        'Ch ${b.chapterNow} of ${b.chapters} · '
                        '${fmtMins(bookLeftS(b))} left',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11, color: t.nInk2),
                      ),
                      const SizedBox(height: 8),
                      ClipRRect(
                        borderRadius: BorderRadius.circular(2),
                        child: LinearProgressIndicator(
                          value: b.progress.clamp(0.0, 1.0),
                          minHeight: 3,
                          backgroundColor: t.nHair,
                          valueColor: const AlwaysStoppedAnimation(kBookTint),
                        ),
                      ),
                    ],
                  ),
                ),
                const Icon(Icons.play_arrow, size: 20, color: kBookTint),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// A book as a wide row: Home's "next in series", and the rows under an author.
class _BookRow extends StatelessWidget {
  const _BookRow({required this.controller, required this.book});

  final MusicController controller;
  final BookCard book;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final b = book;
    return Material(
      color: t.nCard,
      borderRadius: BorderRadius.circular(Tokens.radiusMd),
      child: InkWell(
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        onTap: () => controller.send(MusicCmd.bookOpen(folder: b.folder)),
        child: Padding(
          padding: const EdgeInsets.all(12),
          child: Row(
            children: [
              MusicArt(
                controller: controller,
                kind: 'book',
                artKey: b.folder,
                direct: b.art,
                size: 56,
                radius: 8,
                fallback: Icons.menu_book,
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(b.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 13,
                            fontWeight: FontWeight.w600,
                            color: t.nInk)),
                    Text(
                      [
                        if (b.series.isNotEmpty) '#${b.seriesNo}',
                        '${b.chapters} chapters',
                        if (b.totalS > 0) fmtMins(b.totalS),
                      ].join(' · '),
                      style: TextStyle(fontSize: 11, color: t.nInk2),
                    ),
                  ],
                ),
              ),
              FilledButton.icon(
                icon: const Icon(Icons.play_arrow, size: 16),
                label: Text(b.progress > 0 ? 'Resume' : 'Start'),
                onPressed: () =>
                    controller.send(MusicCmd.bookOpen(folder: b.folder)),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

// ------------------------------------------------------------------ shelf --

class _Shelf extends StatelessWidget {
  const _Shelf({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  static const List<(String, String)> sorts = [
    ('recent', 'Recently played'),
    ('added', 'Recently added'),
    ('title', 'Title'),
    ('author', 'Author'),
    ('left', 'Shortest left'),
  ];

  static const List<(String, String)> groups = [
    ('none', 'None'),
    ('author', 'Author'),
    ('series', 'Series'),
  ];

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // `books` is the whole shelf whatever the tab: the chips' counts, Home and
    // the search all read across it. The tab narrows it here, where narrowing
    // it is what the page is for.
    final shelf = switch (st.bookTab) {
      'progress' =>
        st.books.where((b) => b.progress > 0 && !b.finished).toList(),
      'finished' => st.books.where((b) => b.finished).toList(),
      _ => st.books,
    };
    if (shelf.isEmpty) return _Empty(controller: controller, st: st);
    final list = sortBooks(shelf, st.bookSort);
    final total = list.fold<double>(0, (n, b) => n + b.totalS);
    final groups = groupBooks(list, st.bookGroup);
    return ListView(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 12, 24, 4),
          child: Wrap(
            spacing: 6,
            runSpacing: 6,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              Text('Sort', style: TextStyle(fontSize: 11.5, color: t.nInk3)),
              for (final s in sorts)
                SortChip(
                  label: s.$2,
                  active: st.bookSort == s.$1,
                  onTap: () => controller.send(MusicCmd.bookSetSort(mode: s.$1)),
                ),
              const SizedBox(width: 10),
              Text('Group', style: TextStyle(fontSize: 11.5, color: t.nInk3)),
              for (final g in _Shelf.groups)
                SortChip(
                  label: g.$2,
                  active: st.bookGroup == g.$1,
                  onTap: () =>
                      controller.send(MusicCmd.bookSetGroup(mode: g.$1)),
                ),
              const SizedBox(width: 10),
              Text('${list.length} books · ${fmtMins(total)}',
                  style: TextStyle(fontSize: 11.5, color: t.nInk3)),
            ],
          ),
        ),
        for (final g in groups) ...[
          if (g.$1.isNotEmpty)
            _Head(title: g.$1, hint: '${g.$2.length}'),
          CardGrid(
            inList: true,
            padding: EdgeInsets.fromLTRB(24, g.$1.isEmpty ? 8 : 0, 24, 8),
            min: 190,
            children: [
              for (final b in g.$2)
                _BookCardTile(controller: controller, book: b),
            ],
          ),
        ],
        const SizedBox(height: 24),
      ],
    );
  }
}

/// The shelf's order. Never-played and never-added sort last rather than first:
/// a 0 in `lastPlayed` means "never", not "in 1970".
List<BookCard> sortBooks(List<BookCard> books, String mode) {
  final out = books.toList();
  switch (mode) {
    case 'added':
      out.sort((a, b) => b.added.compareTo(a.added));
    case 'title':
      out.sort((a, b) => a.title.toLowerCase().compareTo(b.title.toLowerCase()));
    case 'author':
      out.sort((a, b) =>
          a.author.toLowerCase().compareTo(b.author.toLowerCase()));
    case 'left':
      out.sort((a, b) => bookLeftS(a).compareTo(bookLeftS(b)));
    default:
      out.sort((a, b) => b.lastPlayed.compareTo(a.lastPlayed));
  }
  return out;
}

/// `(heading, books)` pairs in the order they are drawn. One pair with an
/// empty heading when nothing is grouped, so the caller has one shape to draw.
List<(String, List<BookCard>)> groupBooks(List<BookCard> books, String mode) {
  if (mode != 'author' && mode != 'series') return [('', books)];
  final map = <String, List<BookCard>>{};
  for (final b in books) {
    final key = mode == 'author'
        ? (b.author.isEmpty ? 'Unknown author' : b.author)
        : (b.series.isEmpty ? 'Standalone' : b.series);
    (map[key] ??= []).add(b);
  }
  final keys = map.keys.toList()
    ..sort((a, b) => a.toLowerCase().compareTo(b.toLowerCase()));
  return [for (final k in keys) (k, map[k]!)];
}

class _BookCardTile extends StatelessWidget {
  const _BookCardTile({required this.controller, required this.book});

  final MusicController controller;
  final BookCard book;

  @override
  Widget build(BuildContext context) {
    final b = book;
    final going = b.progress > 0 && !b.finished;
    return MusicCard(
      controller: controller,
      title: b.title,
      subtitle: [
        if (b.author.isNotEmpty) b.author,
        if (b.series.isNotEmpty) '#${b.seriesNo}',
        if (going) '${fmtMins(bookLeftS(b))} left',
        if (!going && b.author.isEmpty) '${b.chapters} chapters',
      ].join(' · '),
      artKind: 'book',
      artKey: b.folder,
      direct: b.art,
      fallback: Icons.menu_book,
      progress: going ? b.progress : null,
      badge: b.finished ? 'Finished' : null,
      onTap: () => controller.send(MusicCmd.bookOpen(folder: b.folder)),
      onMenu: () => bookMenu(context, controller, b),
    );
  }
}

// ------------------------------------------------------- authors & series --

class _Series extends StatelessWidget {
  const _Series({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final byAuthor = <String, List<BookCard>>{};
    for (final b in st.books) {
      (byAuthor[b.author.isEmpty ? 'Unknown author' : b.author] ??= []).add(b);
    }
    final authors = byAuthor.keys.toList()
      ..sort((a, b) => a.toLowerCase().compareTo(b.toLowerCase()));
    final series = st.books
        .where((b) => b.series.isNotEmpty)
        .map((b) => b.series)
        .toSet();
    return ListView(
      children: [
        _Head(
          title: 'Authors & series',
          hint: '${authors.length} authors · ${series.length} series',
        ),
        for (final a in authors)
          _AuthorRow(
            controller: controller,
            author: a,
            books: byAuthor[a]!
              ..sort((x, y) => x.seriesNo != y.seriesNo
                  ? x.seriesNo.compareTo(y.seriesNo)
                  : x.title.compareTo(y.title)),
          ),
        if (authors.isEmpty)
          Padding(
            padding: const EdgeInsets.all(24),
            child: Text(
              'Nothing on the shelf has an author yet. The lookup fills them '
              'in; “Edit details” on a book page is the way to correct one.',
              style: TextStyle(fontSize: 12.5, color: t.nInk2),
            ),
          ),
        const SizedBox(height: 24),
      ],
    );
  }
}

class _AuthorRow extends StatelessWidget {
  const _AuthorRow({
    required this.controller,
    required this.author,
    required this.books,
  });

  final MusicController controller;
  final String author;
  final List<BookCard> books;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final done = books.where((b) => b.finished).length;
    final total = books.fold<double>(0, (n, b) => n + b.totalS);
    return Container(
      margin: const EdgeInsets.fromLTRB(24, 0, 24, 8),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        border: Border.all(color: t.nHair),
      ),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(author,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
                Text(
                  '${books.length} book${books.length == 1 ? '' : 's'} · '
                  '$done finished · ${fmtMins(total)}',
                  style: TextStyle(fontSize: 11, color: t.nInk2),
                ),
              ],
            ),
          ),
          for (final b in books.take(6))
            Padding(
              padding: const EdgeInsets.only(left: 6),
              child: Tooltip(
                message: b.series.isEmpty
                    ? b.title
                    : '${b.title} · ${b.series} #${b.seriesNo}',
                child: InkWell(
                  borderRadius: BorderRadius.circular(7),
                  onTap: () =>
                      controller.send(MusicCmd.bookOpen(folder: b.folder)),
                  child: MusicArt(
                    controller: controller,
                    kind: 'book',
                    artKey: b.folder,
                    direct: b.art,
                    size: 40,
                    radius: 7,
                    fallback: Icons.menu_book,
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// The Folders sub-tab: one row per book folder, `name · N chapters — path`,
/// which is the same line the Slint build draws. Useful when a shelf points at
/// a parent directory and you want to see what the split into books actually
/// produced.
class _FolderList extends StatelessWidget {
  const _FolderList({required this.controller, required this.books});

  final MusicController controller;
  final List<BookCard> books;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView.builder(
      padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 8),
      itemCount: books.length,
      itemBuilder: (_, i) {
        final b = books[i];
        return ListTile(
          dense: true,
          leading: const Icon(Icons.folder_open, size: 18),
          title: Text('${b.title}   ·   ${b.chapters} chapters',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(fontSize: 13, color: t.nInk)),
          subtitle: Text(b.folder,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(fontSize: 11, color: t.nInk2)),
          onTap: () => controller.send(MusicCmd.bookOpen(folder: b.folder)),
          trailing: IconButton(
            iconSize: 16,
            icon: const Icon(Icons.more_horiz),
            tooltip: 'More',
            onPressed: () => bookMenu(context, controller, b),
          ),
        );
      },
    );
  }
}

// -------------------------------------------------------------- book page --

class _BookPage extends StatefulWidget {
  const _BookPage({
    super.key,
    required this.controller,
    required this.st,
  });

  final MusicController controller;
  final MusicState st;

  @override
  State<_BookPage> createState() => _BookPageState();
}

class _BookPageState extends State<_BookPage> {
  final TextEditingController _find = TextEditingController();

  @override
  void dispose() {
    _find.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    final st = widget.st;
    final t = context.tokens;
    final book = st.bookDetail;
    final resume = st.bookResumeIndex;
    final needle = _find.text.trim().toLowerCase();
    // Filtering keeps the original index: a chapter's number is its place in
    // the book, not its place in your search results.
    final chapters = [
      for (var i = 0; i < st.bookChapters.length; i++)
        if (needle.isEmpty ||
            st.bookChapters[i].title.toLowerCase().contains(needle))
          (i, st.bookChapters[i]),
    ];
    final done = st.bookChapters.where((c) => c.finished).length;

    return ListView(
      padding: EdgeInsets.zero,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
          child: TextButton.icon(
            icon: const Icon(Icons.arrow_back, size: 18),
            label: const Text('Back to the shelf'),
            onPressed: () => controller.send(const MusicCmd.bookBack()),
          ),
        ),
        if (book != null)
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 8, 24, 12),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                // The hero is the cover control, exactly as it is in Slint:
                // click it to pick your own art for this book.
                Tooltip(
                  message: 'Cover, and where this folder lives',
                  child: InkWell(
                    borderRadius: BorderRadius.circular(12),
                    onTap: () => bookMenu(context, controller, book),
                    child: MusicArt(
                      controller: controller,
                      kind: 'book',
                      artKey: book.folder,
                      direct: book.art,
                      size: 148,
                      radius: 12,
                      fallback: Icons.menu_book,
                    ),
                  ),
                ),
                const SizedBox(width: 20),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        book.series.isEmpty
                            ? 'AUDIOBOOK'
                            : '${book.series.toUpperCase()} · BOOK ${book.seriesNo}',
                        style: TextStyle(
                            fontSize: 10,
                            letterSpacing: 1.1,
                            fontWeight: FontWeight.w700,
                            color: t.nInk3),
                      ),
                      const SizedBox(height: 2),
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
                          if (book.narrator.isNotEmpty)
                            'read by ${book.narrator}',
                        ].join(' · '),
                        style: TextStyle(fontSize: 12.5, color: t.nInk2),
                      ),
                      const SizedBox(height: 10),
                      Wrap(
                        spacing: 22,
                        runSpacing: 8,
                        children: [
                          _Stat(value: '${book.chapters}', label: 'chapters'),
                          if (book.totalS > 0)
                            _Stat(
                                value: fmtMins(book.totalS), label: 'total'),
                          if (!book.finished && book.totalS > 0)
                            _Stat(
                              value: fmtMins(bookLeftS(book)),
                              label: 'left at '
                                  '${book.speed > 0 ? book.speed : st.bookSpeed}×',
                            ),
                        ],
                      ),
                      const SizedBox(height: 14),
                      Wrap(
                        spacing: 8,
                        runSpacing: 6,
                        crossAxisAlignment: WrapCrossAlignment.center,
                        children: [
                          FilledButton.icon(
                            icon: const Icon(Icons.play_arrow, size: 18),
                            label: Text(resume >= 0
                                ? 'Resume chapter ${book.chapterNow}'
                                : 'Start'),
                            onPressed: st.bookChapters.isEmpty
                                ? null
                                : () => controller.send(
                                      MusicCmd.bookPlay(
                                        // Resume means the chapter that was
                                        // left part-heard; with none, the book
                                        // starts at the beginning.
                                        itemId: st.bookChapters[
                                                resume >= 0 &&
                                                        resume <
                                                            st.bookChapters
                                                                .length
                                                    ? resume
                                                    : 0]
                                            .itemId,
                                      ),
                                    ),
                          ),
                          OutlinedButton.icon(
                            icon: const Icon(Icons.bookmark_add_outlined,
                                size: 16),
                            label: const Text('Bookmark here'),
                            onPressed: controller.now?.mode == 'book'
                                ? () => addBookmark(context, controller)
                                : null,
                          ),
                          TextButton.icon(
                            icon: Icon(
                                book.finished
                                    ? Icons.check_circle
                                    : Icons.check_circle_outline,
                                size: 16),
                            label: Text(
                                book.finished ? 'Finished' : 'Mark finished'),
                            onPressed: () => controller.send(
                              MusicCmd.bookSetFinished(
                                  finished: !book.finished),
                            ),
                          ),
                          TextButton.icon(
                            icon: const Icon(Icons.edit_outlined, size: 16),
                            label: const Text('Edit details'),
                            onPressed: () =>
                                _editDetails(context, controller, book),
                          ),
                          TextButton.icon(
                            icon: const Icon(Icons.restart_alt, size: 16),
                            label: const Text('Start over'),
                            onPressed: () =>
                                _startOver(context, controller, book),
                          ),
                        ],
                      ),
                      if (book.progress > 0) ...[
                        const SizedBox(height: 12),
                        ClipRRect(
                          borderRadius: BorderRadius.circular(3),
                          child: LinearProgressIndicator(
                            value: book.progress.clamp(0.0, 1.0),
                            minHeight: 5,
                            backgroundColor: t.nTile,
                            valueColor:
                                const AlwaysStoppedAnimation(kBookTint),
                          ),
                        ),
                      ],
                    ],
                  ),
                ),
              ],
            ),
          ),
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 6, 24, 6),
          child: Row(
            children: [
              SizedBox(
                width: 260,
                height: 34,
                child: TextField(
                  controller: _find,
                  style: const TextStyle(fontSize: 12.5),
                  decoration: InputDecoration(
                    isDense: true,
                    contentPadding: const EdgeInsets.symmetric(vertical: 8),
                    prefixIcon: const Icon(Icons.search, size: 16),
                    prefixIconConstraints:
                        const BoxConstraints(minWidth: 32, minHeight: 32),
                    hintText: 'Find a chapter',
                    border: const OutlineInputBorder(),
                    suffixIcon: _find.text.isEmpty
                        ? null
                        : IconButton(
                            icon: const Icon(Icons.close, size: 14),
                            tooltip: 'Clear',
                            onPressed: () {
                              _find.clear();
                              setState(() {});
                            },
                          ),
                  ),
                  onChanged: (_) => setState(() {}),
                ),
              ),
              const Spacer(),
              Text(
                book == null
                    ? ''
                    : 'Chapter ${book.chapterNow} of ${book.chapters} · '
                        '$done done',
                style: TextStyle(fontSize: 11.5, color: t.nInk3),
              ),
            ],
          ),
        ),
        if (chapters.isEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 24, 24, 24),
            child: Text(
              needle.isEmpty
                  ? 'This folder has no chapters in the library yet.'
                  : 'No chapter matches “${_find.text.trim()}”.',
              style: TextStyle(fontSize: 12.5, color: t.nInk2),
            ),
          )
        else
          for (final c in chapters)
            _ChapterRow(
              controller: controller,
              chapter: c.$2,
              index: c.$1,
              current: c.$1 == resume,
            ),
        const SizedBox(height: 24),
      ],
    );
  }
}

class _ChapterRow extends StatelessWidget {
  const _ChapterRow({
    required this.controller,
    required this.chapter,
    required this.index,
    required this.current,
  });

  final MusicController controller;
  final Chapter chapter;
  final int index;
  final bool current;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = chapter;
    final playing =
        controller.now?.mode == 'book' && controller.now?.itemId == c.itemId;
    final part = c.durationS > 0 ? (c.positionS / c.durationS) : 0.0;
    final live = playing || current;
    return Material(
      color: Colors.transparent,
      child: InkWell(
        onTap: () => controller.send(MusicCmd.bookPlay(itemId: c.itemId)),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(24, 8, 24, 8),
          child: Row(
            children: [
              SizedBox(
                width: 30,
                child: live
                    ? const Icon(Icons.play_arrow, size: 16, color: kBookTint)
                    : Text('${index + 1}',
                        textAlign: TextAlign.center,
                        style: TextStyle(fontSize: 12, color: t.nInk3)),
              ),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      c.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: 13,
                        fontWeight: live ? FontWeight.w600 : FontWeight.w400,
                        color: live
                            ? kBookTint
                            : (c.finished ? t.nInk2 : t.nInk),
                      ),
                    ),
                    // A bar on the chapter you are in; ticks are for the ones
                    // behind you, and a bar at zero on every other row is noise.
                    if (part > 0.01 && part < 0.99)
                      Padding(
                        padding: const EdgeInsets.only(top: 5, right: 40),
                        child: ClipRRect(
                          borderRadius: BorderRadius.circular(2),
                          child: LinearProgressIndicator(
                            value: part,
                            minHeight: 3,
                            backgroundColor: t.nHair,
                            valueColor:
                                const AlwaysStoppedAnimation(kBookTint),
                          ),
                        ),
                      ),
                  ],
                ),
              ),
              const SizedBox(width: 12),
              Text(fmtClock(c.durationS),
                  style: TextStyle(fontSize: 12, color: t.nInk3)),
              SizedBox(
                width: 28,
                child: c.finished
                    ? const Icon(Icons.check, size: 15, color: Tokens.ok)
                    : null,
              ),
            ],
          ),
        ),
      ),
    );
  }
}

// --------------------------------------------------------------- dialogs ----

/// Per-book actions that have nowhere else to live: the cover, and putting the
/// folder back in My Music. Both write what the Slint build writes, so a change
/// made in either shows in both.
Future<void> bookMenu(
  BuildContext context,
  MusicController c,
  BookCard book,
) async {
  final choice = await showDialog<String>(
    context: context,
    builder: (ctx) => SimpleDialog(
      title: Text(book.title, maxLines: 2, overflow: TextOverflow.ellipsis),
      children: [
        SimpleDialogOption(
          onPressed: () => Navigator.pop(ctx, 'cover'),
          child: const ListTile(
            dense: true,
            leading: Icon(Icons.image_outlined, size: 18),
            title: Text('Choose cover…'),
          ),
        ),
        SimpleDialogOption(
          onPressed: () => Navigator.pop(ctx, 'edit'),
          child: const ListTile(
            dense: true,
            leading: Icon(Icons.edit_outlined, size: 18),
            title: Text('Edit details…'),
            subtitle: Text('Title, author, narrator, series'),
          ),
        ),
        SimpleDialogOption(
          onPressed: () => Navigator.pop(ctx, 'reset'),
          child: const ListTile(
            dense: true,
            leading: Icon(Icons.restart_alt, size: 18),
            title: Text('Start over'),
            subtitle: Text('Forget where you are in it'),
          ),
        ),
        SimpleDialogOption(
          onPressed: () => Navigator.pop(ctx, 'unbook'),
          child: const ListTile(
            dense: true,
            leading: Icon(Icons.library_music_outlined, size: 18),
            title: Text('Move back to My Music'),
            subtitle: Text('Its chapters become songs again'),
          ),
        ),
      ],
    ),
  );
  if (choice == null || !context.mounted) return;
  switch (choice) {
    case 'cover':
      final path = await pickFile(
        label: 'Images',
        extensions: const ['png', 'jpg', 'jpeg', 'webp', 'bmp'],
      );
      if (path == null) return;
      await c.send(
          MusicCmd.setCardArt(kind: 'book', key: book.folder, path: path));
    case 'edit':
      await _editDetails(context, c, book);
    case 'reset':
      await _startOver(context, c, book);
    case 'unbook':
      await c.send(MusicCmd.bookFlagFolder(folder: book.folder, on_: false));
  }
}

/// The four fields the online lookup guesses. It is right often enough to be
/// worth running and wrong often enough to need a way back.
Future<void> _editDetails(
  BuildContext context,
  MusicController c,
  BookCard book,
) async {
  final title = TextEditingController(text: book.title);
  final author = TextEditingController(text: book.author);
  final narrator = TextEditingController(text: book.narrator);
  final series = TextEditingController(text: book.series);
  final no = TextEditingController(
      text: book.seriesNo > 0 ? '${book.seriesNo}' : '');
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Edit details'),
      content: SizedBox(
        width: 420,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            for (final f in [
              (title, 'Title'),
              (author, 'Author'),
              (narrator, 'Narrator'),
              (series, 'Series'),
            ])
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: TextField(
                  controller: f.$1,
                  decoration: InputDecoration(labelText: f.$2),
                ),
              ),
            TextField(
              controller: no,
              keyboardType: TextInputType.number,
              decoration: const InputDecoration(
                labelText: 'Number in the series',
                hintText: 'blank for a standalone book',
              ),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Cancel')),
        FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('Save')),
      ],
    ),
  );
  if (ok ?? false) {
    await c.send(MusicCmd.bookEditDetails(
      folder: book.folder,
      title: title.text.trim(),
      author: author.text.trim(),
      narrator: narrator.text.trim(),
      series: series.text.trim(),
      seriesNo: int.tryParse(no.text.trim()) ?? 0,
    ));
  }
  for (final f in [title, author, narrator, series, no]) {
    f.dispose();
  }
}

Future<void> _startOver(
  BuildContext context,
  MusicController c,
  BookCard book,
) async {
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Start this book over?'),
      content: Text(
        'Every chapter of “${book.title}” goes back to the beginning and the '
        'book leaves the In progress shelf. Your bookmarks stay — they are '
        'notes about the text, not about the progress.',
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Cancel')),
        FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('Start over')),
      ],
    ),
  );
  if (ok ?? false) {
    await c.send(MusicCmd.bookResetProgress(folder: book.folder));
  }
}

/// Mark the exact second being heard. Shared with the docked panel and the B
/// key, which is why it is not private to this file.
Future<void> addBookmark(BuildContext context, MusicController c) async {
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
  text.dispose();
  if (label != null) await c.send(MusicCmd.bookmarkAdd(label: label.trim()));
}

/// Rename a bookmark and write the sentence behind it. One dialog, because the
/// label and the note are one thought and two dialogs would be two trips.
Future<void> editBookmark(
  BuildContext context,
  MusicController c,
  int index,
  Bookmark mark,
) async {
  final label = TextEditingController(text: mark.label);
  final note = TextEditingController(text: mark.note);
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(mark.when),
      content: SizedBox(
        width: 380,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: label,
              autofocus: true,
              decoration: const InputDecoration(labelText: 'Label'),
            ),
            const SizedBox(height: 8),
            TextField(
              controller: note,
              minLines: 2,
              maxLines: 4,
              decoration: const InputDecoration(
                labelText: 'Note',
                hintText: 'why this bit is worth coming back to',
              ),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Cancel')),
        FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('Save')),
      ],
    ),
  );
  if (ok ?? false) {
    await c.send(
        MusicCmd.bookmarkRename(index: index, label: label.text.trim()));
    await c.send(
        MusicCmd.bookmarkSetNote(index: index, note: note.text.trim()));
  }
  label.dispose();
  note.dispose();
}
