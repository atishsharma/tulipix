// The Books shell: the hero, the filters, the shelf, and — when one is open —
// the reader over the top of all of it.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/books.dart';
import 'book_detail.dart';
import 'book_reader.dart';
import 'books_controller.dart';
import 'books_stats.dart';
import 'books_widgets.dart';

class BooksPage extends StatefulWidget {
  const BooksPage({super.key});

  @override
  State<BooksPage> createState() => _BooksPageState();
}

class _BooksPageState extends State<BooksPage> {
  final BooksController _c = BooksController();
  final TextEditingController _search = TextEditingController();

  @override
  void initState() {
    super.initState();
    _c.refresh();
  }

  @override
  void dispose() {
    _search.dispose();
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        final st = _c.state;
        // The reader takes the whole section. It is not a dialog: you are in
        // the book until you leave it.
        if (_c.reader != null) return BookReader(controller: _c);

        return ColoredBox(
          color: t.nCanvas,
          child: Column(
            children: [
              _Header(controller: _c, search: _search),
              if (_c.progress != null) _ScanBar(controller: _c),
              if (_c.error != null) _ErrorBanner(controller: _c),
              if (st != null && st.status.isNotEmpty)
                _StatusBanner(message: st.status),
              Expanded(
                child: st == null
                    ? const Center(child: CircularProgressIndicator())
                    : _Body(controller: _c, state: st),
              ),
            ],
          ),
        );
      },
    );
  }
}

class _Header extends StatelessWidget {
  const _Header({required this.controller, required this.search});

  final BooksController controller;
  final TextEditingController search;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = controller.state;
    final trash = st?.view == 'trash';

    return Container(
      padding: const EdgeInsets.fromLTRB(20, 14, 20, 12),
      decoration: BoxDecoration(
        color: t.panel,
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Column(
        children: [
          Row(
            children: [
              const Icon(Icons.menu_book, color: Tokens.secBooks, size: 22),
              const SizedBox(width: 10),
              Text('Books',
                  style: TextStyle(
                      fontSize: 20,
                      fontWeight: FontWeight.w800,
                      color: t.nInk)),
              const SizedBox(width: 12),
              if (st != null)
                Text(
                  '${st.filteredTotal} of ${st.total}',
                  style: TextStyle(fontSize: 12, color: t.nInk2),
                ),
              const Spacer(),
              SizedBox(
                width: 300,
                child: TextField(
                  controller: search,
                  decoration: InputDecoration(
                    isDense: true,
                    hintText: (st?.searchContents ?? false)
                        ? 'Search inside books…'
                        : 'Search titles, authors, series…',
                    prefixIcon: const Icon(Icons.search, size: 18),
                    suffixIcon: IconButton(
                      iconSize: 16,
                      tooltip: (st?.searchContents ?? false)
                          ? 'Searching contents'
                          : 'Search contents instead',
                      icon: Icon(
                        Icons.manage_search,
                        color: (st?.searchContents ?? false)
                            ? Tokens.secBooks
                            : t.nInk3,
                      ),
                      onPressed: () => controller.send(
                        BooksCmd.setSearchContents(
                            on_: !(st?.searchContents ?? false)),
                      ),
                    ),
                    border: const OutlineInputBorder(),
                  ),
                  onSubmitted: (v) =>
                      controller.send(BooksCmd.search(text: v.trim())),
                ),
              ),
              const SizedBox(width: 8),
              _SortButton(controller: controller),
              IconButton(
                tooltip: st?.viewMode == 'list' ? 'Grid' : 'List',
                icon: Icon(
                    st?.viewMode == 'list' ? Icons.grid_view : Icons.view_list),
                onPressed: () => controller.send(BooksCmd.setViewMode(
                    mode: st?.viewMode == 'list' ? 'grid' : 'list')),
              ),
              IconButton(
                tooltip: 'Reading statistics',
                icon: const Icon(Icons.insights_outlined),
                onPressed: () => controller.send(const BooksCmd.openStats()),
              ),
              IconButton(
                tooltip: trash ? 'Back to the library' : 'Trash',
                icon: Icon(trash ? Icons.arrow_back : Icons.delete_outline),
                onPressed: () => controller
                    .send(BooksCmd.setView(view: trash ? 'library' : 'trash')),
              ),
              _LibraryMenu(controller: controller),
            ],
          ),
          const SizedBox(height: 10),
          if (!trash) _Filters(controller: controller),
        ],
      ),
    );
  }
}

class _SortButton extends StatelessWidget {
  const _SortButton({required this.controller});

  final BooksController controller;

  @override
  Widget build(BuildContext context) {
    final idx = controller.state?.sortIndex ?? 0;
    return PopupMenuButton<int>(
      tooltip: 'Sort',
      icon: const Icon(Icons.sort),
      onSelected: (v) => controller.send(BooksCmd.setSort(index: v)),
      itemBuilder: (_) => [
        for (var i = 0; i < sortOptions.length; i++)
          CheckedPopupMenuItem(
            value: i,
            checked: idx == i,
            child: Text(sortOptions[i]),
          ),
      ],
    );
  }
}

class _LibraryMenu extends StatelessWidget {
  const _LibraryMenu({required this.controller});

  final BooksController controller;

  @override
  Widget build(BuildContext context) {
    return PopupMenuButton<String>(
      tooltip: 'Library',
      icon: const Icon(Icons.more_vert),
      onSelected: (v) async {
        switch (v) {
          case 'add':
            final path = await promptText(
              context,
              title: 'Add a books folder',
              label: 'Absolute path',
              hint: '/home/you/Books',
              confirm: 'Add and scan',
            );
            if (path != null) {
              await controller.send(BooksCmd.addFolder(path: path));
            }
          case 'scan':
            await controller.send(const BooksCmd.scan());
          case 'art':
            await controller.send(const BooksCmd.buildArt());
          case 'index':
            await controller.send(const BooksCmd.indexContents());
          case 'folders':
            if (context.mounted) await showFolders(context, controller);
        }
      },
      itemBuilder: (_) => const [
        PopupMenuItem(value: 'add', child: Text('Add a books folder…')),
        PopupMenuItem(value: 'folders', child: Text('Watched folders…')),
        PopupMenuDivider(),
        PopupMenuItem(value: 'scan', child: Text('Rescan the library')),
        PopupMenuItem(value: 'art', child: Text('Build missing cover art')),
        PopupMenuItem(value: 'index', child: Text('Index contents for search')),
      ],
    );
  }
}

/// Quick filters, then whichever facet rows have anything in them.
class _Filters extends StatelessWidget {
  const _Filters({required this.controller});

  final BooksController controller;

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    if (st == null) return const SizedBox.shrink();
    final t = context.tokens;
    final filtered = st.activeFormat.isNotEmpty ||
        st.activeGenre.isNotEmpty ||
        st.activeSeries.isNotEmpty ||
        st.activeAuthor.isNotEmpty ||
        st.activeCollection != 0 ||
        st.activeQuick != 'all' ||
        st.query.isNotEmpty;

    return Column(
      children: [
        SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          child: Row(
            children: [
              for (final q in quickFilters)
                BookChipView(
                  label: q.label,
                  icon: q.icon,
                  active: st.activeQuick == q.id,
                  onTap: () => controller.send(BooksCmd.setQuick(quick: q.id)),
                ),
              if (st.formats.isNotEmpty) ...[
                const _Divider(),
                for (final f in st.formats)
                  BookChipView(
                    label: f.label.toUpperCase(),
                    count: f.count,
                    active: st.activeFormat == f.key,
                    onTap: () =>
                        controller.send(BooksCmd.setFormat(format: f.key)),
                  ),
              ],
              if (st.collections.isNotEmpty) ...[
                const _Divider(),
                for (final c in st.collections)
                  BookChipView(
                    label: c.name,
                    count: c.count,
                    active: st.activeCollection == c.id,
                    onTap: () =>
                        controller.send(BooksCmd.setCollection(id: c.id)),
                  ),
              ],
              if (st.smart.isNotEmpty) ...[
                const _Divider(),
                for (final c in st.smart)
                  BookChipView(
                    label: '✦ ${c.name}',
                    active: false,
                    onTap: () => controller.send(BooksCmd.smartApply(id: c.id)),
                  ),
              ],
              if (filtered) ...[
                const _Divider(),
                BookChipView(
                  label: 'Clear',
                  icon: Icons.close,
                  active: false,
                  onTap: () => controller.send(const BooksCmd.clearFilters()),
                ),
                BookChipView(
                  label: 'Save as smart',
                  icon: Icons.bookmark_add_outlined,
                  active: false,
                  onTap: () async {
                    final name = await promptText(
                      context,
                      title: 'Save these filters',
                      label: 'Name',
                      confirm: 'Save',
                    );
                    if (name != null) {
                      await controller
                          .send(BooksCmd.smartCreateFromFilters(name: name));
                    }
                  },
                ),
              ],
            ],
          ),
        ),
        if (st.genres.isNotEmpty || st.series.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(top: 6),
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: Row(
                children: [
                  Text('Genres',
                      style: TextStyle(fontSize: 11, color: t.nInk3)),
                  const SizedBox(width: 8),
                  for (final g in st.genres.take(14))
                    BookChipView(
                      label: g.label,
                      count: g.count,
                      active: st.activeGenre == g.key,
                      onTap: () =>
                          controller.send(BooksCmd.setGenre(genre: g.key)),
                    ),
                  if (st.series.isNotEmpty) ...[
                    const _Divider(),
                    Text('Series',
                        style: TextStyle(fontSize: 11, color: t.nInk3)),
                    const SizedBox(width: 8),
                    for (final s in st.series.take(10))
                      BookChipView(
                        label: s.label,
                        count: s.count,
                        active: st.activeSeries == s.key,
                        onTap: () =>
                            controller.send(BooksCmd.setSeries(series: s.key)),
                      ),
                  ],
                ],
              ),
            ),
          ),
      ],
    );
  }
}

class _Divider extends StatelessWidget {
  const _Divider();

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.symmetric(horizontal: 8),
        child: Container(width: 1, height: 18, color: context.tokens.nHover),
      );
}

class _Body extends StatelessWidget {
  const _Body({required this.controller, required this.state});

  final BooksController controller;
  final BooksState state;

  @override
  Widget build(BuildContext context) {
    final st = state;
    if (st.readingStats != null) {
      return ReadingStatsPanel(controller: controller, stats: st.readingStats!);
    }
    if (st.detail != null) {
      return BookDetailPanel(controller: controller, book: st.detail!);
    }
    if (st.books.isEmpty) {
      return _Empty(controller: controller, trash: st.view == 'trash');
    }

    void open(Book b) => controller.send(BooksCmd.openBook(id: b.id));
    void details(Book b) => controller.send(BooksCmd.openDetail(id: b.id));

    return ListView(
      padding: const EdgeInsets.fromLTRB(20, 16, 20, 32),
      children: [
        if (st.view != 'trash' && st.hero != null) ...[
          _HeroCard(controller: controller, hero: st.hero!, slider: st.slider),
          const SizedBox(height: 20),
        ],
        if (st.view != 'trash') ...[
          _StatsStrip(stats: st.stats),
          const SizedBox(height: 20),
        ],
        if (st.view == 'trash')
          Padding(
            padding: const EdgeInsets.only(bottom: 12),
            child: Row(
              children: [
                Text('${st.trashedCount} in the trash',
                    style: TextStyle(color: context.tokens.nInk2)),
                const Spacer(),
                if (st.trashedCount > 0)
                  TextButton.icon(
                    icon: const Icon(Icons.delete_forever, size: 16),
                    label: const Text('Empty the trash'),
                    onPressed: () async {
                      final ok = await confirmAction(
                        context,
                        title: 'Empty the trash?',
                        body:
                            'Removes ${st.trashedCount} books from the library. '
                            'The files themselves are left alone.',
                        action: 'Empty',
                      );
                      if (ok) {
                        await controller.send(const BooksCmd.emptyTrash());
                      }
                    },
                  ),
              ],
            ),
          ),
        if (st.viewMode == 'list')
          for (final b in st.books)
            BookListRow(
              controller: controller,
              book: b,
              onOpen: () => open(b),
              onDetails: () => details(b),
            )
        else
          Wrap(
            spacing: 18,
            runSpacing: 22,
            children: [
              for (final b in st.books)
                BookTile(
                  controller: controller,
                  book: b,
                  onOpen: () => open(b),
                  onDetails: () => details(b),
                ),
            ],
          ),
        const SizedBox(height: 20),
        BookPager(
          page: st.page,
          pages: st.pageCount,
          onGo: (p) => controller.send(BooksCmd.setPage(page: p)),
        ),
      ],
    );
  }
}

/// What you are part-way through, with the rest of the shelf beside it.
class _HeroCard extends StatelessWidget {
  const _HeroCard({
    required this.controller,
    required this.hero,
    required this.slider,
  });

  final BooksController controller;
  final HeroBook hero;
  final List<HeroBook> slider;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(18),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: t.nHair),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          _HeroCover(controller: controller, hero: hero),
          const SizedBox(width: 18),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text('Continue reading',
                    style: TextStyle(
                        fontSize: 11,
                        letterSpacing: 1.4,
                        fontWeight: FontWeight.w700,
                        color: Tokens.secBooks)),
                const SizedBox(height: 6),
                Text(hero.title,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 24,
                        fontWeight: FontWeight.w800,
                        color: t.nInk)),
                Text(hero.author,
                    style: TextStyle(fontSize: 13, color: t.nInk2)),
                const SizedBox(height: 14),
                ClipRRect(
                  borderRadius: BorderRadius.circular(3),
                  child: LinearProgressIndicator(
                    value: hero.percent.clamp(0.0, 1.0),
                    minHeight: 6,
                    backgroundColor: t.nHover,
                    valueColor: const AlwaysStoppedAnimation(Tokens.secBooks),
                  ),
                ),
                const SizedBox(height: 6),
                Text(
                  [
                    'page ${hero.page + 1} of ${hero.total}',
                    '${(hero.percent * 100).round()}%',
                    if (hero.timeLeft.isNotEmpty) hero.timeLeft,
                  ].join('  ·  '),
                  style: TextStyle(fontSize: 12, color: t.nInk2),
                ),
                const SizedBox(height: 14),
                Row(
                  children: [
                    FilledButton.icon(
                      style: FilledButton.styleFrom(
                          backgroundColor: Tokens.secBooks),
                      icon: const Icon(Icons.play_arrow, size: 18),
                      label: const Text('Resume'),
                      onPressed: () =>
                          controller.send(BooksCmd.openBook(id: hero.id)),
                    ),
                    const SizedBox(width: 8),
                    OutlinedButton(
                      onPressed: () =>
                          controller.send(BooksCmd.openDetail(id: hero.id)),
                      child: const Text('Details'),
                    ),
                  ],
                ),
                if (slider.length > 1) ...[
                  const SizedBox(height: 16),
                  SizedBox(
                    height: 92,
                    child: ListView.separated(
                      scrollDirection: Axis.horizontal,
                      itemCount: slider.length,
                      separatorBuilder: (_, __) => const SizedBox(width: 10),
                      itemBuilder: (_, i) => _SliderChip(
                        controller: controller,
                        hero: slider[i],
                        current: slider[i].id == hero.id,
                      ),
                    ),
                  ),
                ],
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _HeroCover extends StatelessWidget {
  const _HeroCover({required this.controller, required this.hero});

  final BooksController controller;
  final HeroBook hero;

  @override
  Widget build(BuildContext context) {
    // The hero carries its own cover path, so a synthetic Book is enough to
    // reuse the cover widget rather than duplicating its fallback.
    final stand = Book(
      id: hero.id,
      title: hero.title,
      author: hero.author,
      series: '',
      genre: '',
      format: '',
      cover: hero.cover,
      published: '',
      percent: hero.percent,
      favorite: false,
      finished: false,
      missing: false,
      trashed: false,
      magazine: false,
      rtl: false,
      rating: 0,
      netRating: 0,
      sizeBytes: 0,
      addedAt: 0,
      lastRead: 0,
      timeRead: 0,
    );
    return BookCover(
        controller: controller, book: stand, width: 120, radius: 10);
  }
}

class _SliderChip extends StatelessWidget {
  const _SliderChip({
    required this.controller,
    required this.hero,
    required this.current,
  });

  final BooksController controller;
  final HeroBook hero;
  final bool current;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: () => controller.send(BooksCmd.openBook(id: hero.id)),
      borderRadius: BorderRadius.circular(8),
      child: Container(
        width: 190,
        padding: const EdgeInsets.all(8),
        decoration: BoxDecoration(
          color: current ? Tokens.secBooks.withValues(alpha: 0.12) : t.nTile,
          borderRadius: BorderRadius.circular(8),
          border: Border.all(color: current ? Tokens.secBooks : t.nHair),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(hero.title,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 12, fontWeight: FontWeight.w600, color: t.nInk)),
            const Spacer(),
            LinearProgressIndicator(
              value: hero.percent.clamp(0.0, 1.0),
              minHeight: 3,
              backgroundColor: t.nHover,
              valueColor: const AlwaysStoppedAnimation(Tokens.secBooks),
            ),
            const SizedBox(height: 4),
            Text('${(hero.percent * 100).round()}%',
                style: TextStyle(fontSize: 10, color: t.nInk2)),
          ],
        ),
      ),
    );
  }
}

class _StatsStrip extends StatelessWidget {
  const _StatsStrip({required this.stats});

  final LibraryStats stats;

  @override
  Widget build(BuildContext context) {
    final cards = <({String label, String value, String sub, IconData icon})>[
      (
        label: 'Books',
        value: '${stats.total}',
        sub: stats.addedMonth > 0 ? '+${stats.addedMonth} this month' : '',
        icon: Icons.menu_book_outlined,
      ),
      (
        label: 'Authors',
        value: '${stats.authors}',
        sub: stats.authorsMonth > 0 ? '+${stats.authorsMonth} new' : '',
        icon: Icons.person_outline,
      ),
      (
        label: 'Finished',
        value: '${stats.finished}',
        sub: '${stats.inProgress} in progress',
        icon: Icons.check_circle_outline,
      ),
      (
        label: 'Hours read',
        value: stats.hoursRead.toStringAsFixed(1),
        sub: '',
        icon: Icons.schedule,
      ),
    ];
    return Row(
      children: [
        for (final c in cards)
          Expanded(
              child: _StatCard(
                  label: c.label, value: c.value, sub: c.sub, icon: c.icon)),
      ],
    );
  }
}

class _StatCard extends StatelessWidget {
  const _StatCard({
    required this.label,
    required this.value,
    required this.sub,
    required this.icon,
  });

  final String label;
  final String value;
  final String sub;
  final IconData icon;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      margin: const EdgeInsets.only(right: 12),
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: t.nHair),
      ),
      child: Row(
        children: [
          Icon(icon, size: 20, color: Tokens.secBooks),
          const SizedBox(width: 12),
          Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(value,
                  style: TextStyle(
                      fontSize: 20,
                      fontWeight: FontWeight.w800,
                      color: t.nInk)),
              Text(label, style: TextStyle(fontSize: 11, color: t.nInk2)),
              if (sub.isNotEmpty)
                Text(sub, style: TextStyle(fontSize: 10, color: t.nInk3)),
            ],
          ),
        ],
      ),
    );
  }
}

class _Empty extends StatelessWidget {
  const _Empty({required this.controller, required this.trash});

  final BooksController controller;
  final bool trash;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 96,
            height: 96,
            decoration: BoxDecoration(
              color: Tokens.secBooks.withValues(alpha: 0.14),
              shape: BoxShape.circle,
            ),
            child: Icon(trash ? Icons.delete_outline : Icons.menu_book,
                size: 40, color: Tokens.secBooks),
          ),
          const SizedBox(height: 16),
          Text(trash ? 'The trash is empty' : 'No books here yet',
              style: TextStyle(
                  fontSize: 22, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(height: 6),
          Text(
            trash
                ? 'Books you move to the trash appear here.'
                : 'Add a folder of EPUB, PDF, MOBI or CBZ files to fill the shelf.',
            style: TextStyle(fontSize: 13, color: t.nInk2),
          ),
          if (!trash) ...[
            const SizedBox(height: 18),
            FilledButton.icon(
              style: FilledButton.styleFrom(backgroundColor: Tokens.secBooks),
              icon: const Icon(Icons.add),
              label: const Text('Add a books folder'),
              onPressed: () async {
                final path = await promptText(
                  context,
                  title: 'Add a books folder',
                  label: 'Absolute path',
                  hint: '/home/you/Books',
                  confirm: 'Add and scan',
                );
                if (path != null) {
                  await controller.send(BooksCmd.addFolder(path: path));
                }
              },
            ),
          ],
        ],
      ),
    );
  }
}

class _ScanBar extends StatelessWidget {
  const _ScanBar({required this.controller});

  final BooksController controller;

  @override
  Widget build(BuildContext context) {
    final p = controller.progress!;
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 8),
      color: Tokens.secBooks.withValues(alpha: 0.10),
      child: Row(
        children: [
          SizedBox(
            width: 160,
            child: LinearProgressIndicator(
              value: p.total > 0 ? p.done / p.total : null,
              minHeight: 4,
              backgroundColor: t.nHover,
              valueColor: const AlwaysStoppedAnimation(Tokens.secBooks),
            ),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Text('Scanning ${p.done} / ${p.total}  ·  ${p.name}',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12, color: t.nInk2)),
          ),
        ],
      ),
    );
  }
}

class _ErrorBanner extends StatelessWidget {
  const _ErrorBanner({required this.controller});

  final BooksController controller;

  @override
  Widget build(BuildContext context) => Container(
        width: double.infinity,
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
        color: Tokens.error.withValues(alpha: 0.14),
        child: Row(
          children: [
            const Icon(Icons.error_outline, size: 18, color: Tokens.error),
            const SizedBox(width: 10),
            Expanded(
              child: Text('${controller.error}',
                  style: const TextStyle(fontSize: 12, color: Tokens.error)),
            ),
            IconButton(
              iconSize: 16,
              icon: const Icon(Icons.close),
              onPressed: controller.clearError,
            ),
          ],
        ),
      );
}

class _StatusBanner extends StatelessWidget {
  const _StatusBanner({required this.message});

  final String message;

  @override
  Widget build(BuildContext context) => Container(
        width: double.infinity,
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 8),
        color: context.tokens.nChip,
        child: Text(message,
            style: TextStyle(fontSize: 12, color: context.tokens.nInk2)),
      );
}
