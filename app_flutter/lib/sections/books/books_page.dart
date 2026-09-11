// The Books shell — the same page ui/page_books.slint draws, in the same
// order: header row · hero row (Continue card + the 2 × 3 stat block) · main
// row (toolbar, grid, pagination | the 300 px right rail). The detail sheet and
// the reading-stats panel are modals over all of it, and the reader — like
// Genesis — takes the whole section.
//
// Numbers here are that file's numbers. The grid is a fixed 3 × 2 of cells cut
// from the viewport, not a scrolling wrap: a page is six books and the pager
// lives in the toolbar, which is why PAGE_SIZE in the bridge is six.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../design/skin.dart';

import '../../design/first_load.dart';
import '../../design/pick.dart';
import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/books.dart';
import '../genesis/genesis_page.dart';
import 'book_detail.dart';
import 'book_mockup.dart';
import 'book_reader.dart';
import 'book_theme.dart';
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

  /// Genesis is a sub-page of this section, like the reader: it takes the whole
  /// area until you come back out of it.
  bool _genesis = false;

  /// Which main panel is showing — library | collections | series. Local, the
  /// way `panel` is a plain property on the Slint page: nothing on the Rust
  /// side needs to know which tab is up.
  String _panel = 'library';
  String _collectionName = '';

  /// Which in-progress book the Continue card is showing. The bridge has no
  /// hero-slide command — the Slint page keeps this on the component too — so
  /// it lives here and indexes `state.slider`.
  int _slide = 0;
  Timer? _carousel;

  /// The status line under the header clears itself, the way a toast does: a
  /// note about what the last command did has no business sitting there for the
  /// rest of the session.
  String _statusShown = '';
  bool _statusOpen = false;
  Timer? _statusTimer;

  @override
  void initState() {
    super.initState();
    _c.refresh();
    // Home's Genesis launcher opens the sub-page, not the shelf.
    ShellController.instance.onOpen(Section.books, (tab) {
      if (tab == 'genesis' && mounted) setState(() => _genesis = true);
    });
  }

  @override
  void dispose() {
    _carousel?.cancel();
    _statusTimer?.cancel();
    _search.dispose();
    _c.dispose();
    super.dispose();
  }

  /// Auto-advance every five seconds while more than one book is in progress —
  /// the Slint page's own Timer, at its interval.
  void _carouselFor(int slides) {
    if (slides > 1) {
      _carousel ??=
          Timer.periodic(const Duration(seconds: 5), (_) => _nextSlide());
    } else {
      _carousel?.cancel();
      _carousel = null;
    }
  }

  void _nextSlide() {
    final slides = _c.state?.slider.length ?? 0;
    if (slides < 2 || !mounted) return;
    setState(() => _slide = (_slide + 1) % slides);
  }

  /// A new status line shows for five seconds, then goes. Returns whether
  /// anything changed, so the caller can avoid a setState that would rebuild
  /// into another call of this.
  bool _statusFor(String message) {
    if (message == _statusShown) return false;
    _statusShown = message;
    _statusTimer?.cancel();
    _statusOpen = message.isNotEmpty;
    if (message.isNotEmpty) {
      _statusTimer = Timer(const Duration(seconds: 5), () {
        if (mounted) setState(() => _statusOpen = false);
      });
    }
    return true;
  }

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        final st = _c.state;
        _carouselFor(st?.slider.length ?? 0);
        // The status line's timer is armed from the build rather than the
        // command, because a snapshot is the only place the note appears.
        if (_statusShown != (st?.status ?? '')) {
          WidgetsBinding.instance.addPostFrameCallback((_) {
            if (mounted && _statusFor(st?.status ?? '')) setState(() {});
          });
        }
        // The reader takes the whole section. It is not a dialog: you are in
        // the book until you leave it.
        if (_c.reader != null) return BookReader(controller: _c);

        if (_genesis) {
          return GenesisPage(
            // Coming back re-scans nothing: a download already asked the books
            // library to rescan, and the snapshot behind this page is fresh.
            onBack: () => setState(() => _genesis = false),
          );
        }

        return ColoredBox(
          color: b.canvas,
          child: Stack(
            children: [
              Padding(
                padding: const EdgeInsets.all(24),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    _Header(
                      controller: _c,
                      search: _search,
                      onGenesis: () => setState(() => _genesis = true),
                    ),
                    if (_c.progress != null) ...[
                      const SizedBox(height: 12),
                      _ScanBar(controller: _c),
                    ],
                    if (_c.error != null) ...[
                      const SizedBox(height: 12),
                      _ErrorBanner(controller: _c),
                    ],
                    if (_statusOpen && _statusShown.isNotEmpty) ...[
                      const SizedBox(height: 12),
                      _StatusBanner(
                        message: _statusShown,
                        onClose: () => setState(() => _statusOpen = false),
                      ),
                    ],
                    const SizedBox(height: 20),
                    if (st == null)
                      Expanded(
                          child:
                              FirstLoad(error: _c.error, onRetry: _c.refresh))
                    else ...[
                      _HeroRow(
                        controller: _c,
                        state: st,
                        slide: _slide,
                        onSlide: _nextSlide,
                      ),
                      const SizedBox(height: 20),
                      Expanded(
                        child: _MainRow(
                          controller: _c,
                          state: st,
                          panel: _panel,
                          collectionName: _collectionName,
                          onPanel: (p) => setState(() => _panel = p),
                          onCollection: (name) =>
                              setState(() => _collectionName = name),
                        ),
                      ),
                    ],
                  ],
                ),
              ),
              // The detail sheet and the stats panel are popups over the page,
              // not places you navigate to.
              if (st?.detail != null)
                _Modal(
                  width: 768,
                  height: 506,
                  hue: coverHue(st!.detail!.title),
                  onClose: () => _c.send(const BooksCmd.closeDetail()),
                  child: BookDetailPanel(controller: _c, book: st.detail!),
                ),
              if (st?.readingStats != null)
                _Modal(
                  width: 620,
                  height: 500,
                  onClose: () => _c.send(const BooksCmd.closeStats()),
                  child: ReadingStatsPanel(
                      controller: _c, stats: st!.readingStats!),
                ),
            ],
          ),
        );
      },
    );
  }
}

/// The frosted scrim plus a fixed-size card in the middle of the page. Flutter
/// can blur where Slint could not, so the wash the Slint file fakes is a real
/// one here.
class _Modal extends StatelessWidget {
  const _Modal({
    required this.width,
    required this.height,
    required this.child,
    required this.onClose,
    this.hue,
  });

  final double width;
  final double height;
  final Widget child;
  final VoidCallback onClose;
  final Color? hue;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return Stack(
      children: [
        Positioned.fill(
          child: GestureDetector(
            onTap: onClose,
            child: ColoredBox(
              color: b.dark ? const Color(0x99141420) : const Color(0x99F4F2FA),
            ),
          ),
        ),
        Center(
          child: LayoutBuilder(
            builder: (context, box) => SizedBox(
              width: width.clamp(0.0, box.maxWidth - 32),
              height: height.clamp(0.0, box.maxHeight - 32),
              child: Container(
                clipBehavior: Clip.antiAlias,
                decoration: context.skin
                        .surface(SurfaceRole.card, radius: 20) ??
                    BoxDecoration(
                  color: b.card,
                  borderRadius: BorderRadius.circular(20),
                  border: Border.all(
                      color: (hue ?? b.hairline).withValues(alpha: 0.5),
                      width: 1.5),
                  boxShadow: const [
                    BoxShadow(
                        color: Color(0x40000000),
                        blurRadius: 40,
                        offset: Offset(0, 10)),
                  ],
                ),
                child: child,
              ),
            ),
          ),
        ),
      ],
    );
  }
}

// ── header row ───────────────────────────────────────────────────────────────

class _Header extends StatelessWidget {
  const _Header({
    required this.controller,
    required this.search,
    required this.onGenesis,
  });

  final BooksController controller;
  final TextEditingController search;
  final VoidCallback onGenesis;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final st = controller.state;

    return Row(
      children: [
        Container(
          width: 40,
          height: 40,
          alignment: Alignment.center,
          decoration: context.skin.control(
                active: true,
                tint: BookTheme.accent,
                radius: 12,
              ) ??
              BoxDecoration(
                color: b.tintViolet,
                borderRadius: BorderRadius.circular(12),
              ),
          child: Icon(context.skin.icon(Icons.menu_book_outlined),
              size: 22, color: BookTheme.accent),
        ),
        const SizedBox(width: 16),
        // Not Flexible: a flexing title would split the free space with the
        // Spacer and leave the search box floating in the middle of the row
        // instead of on the right edge.
        Text('My Library',
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
                fontSize: 26, fontWeight: FontWeight.w800, color: b.ink)),
        const Spacer(),
        _SearchBox(controller: controller, search: search),
        const SizedBox(width: 16),
        // Genesis — search and download. Filled red, so the one control that
        // leaves the library for the open internet does not look like the ones
        // that do not.
        PillButton(
          label: 'Genesis',
          icon: Icons.download,
          filled: true,
          compact: true,
          accent: BookTheme.genesisRed,
          onTap: onGenesis,
        ),
        const SizedBox(width: 16),
        PillButton(
          label: 'Add books',
          filled: true,
          compact: true,
          onTap: () async {
            final path = await pickDirectory();
            if (path != null) {
              await controller.send(BooksCmd.addFolder(path: path));
            }
          },
        ),
        const SizedBox(width: 8),
        // Folder management, rescans and the index passes have no home on the
        // Slint page — they are its `add-books` callback's other half — so they
        // live behind one dot rather than as five more buttons.
        _LibraryMenu(controller: controller, trashed: st?.trashedCount ?? 0),
      ],
    );
  }
}

class _SearchBox extends StatelessWidget {
  const _SearchBox({required this.controller, required this.search});

  final BooksController controller;
  final TextEditingController search;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final st = controller.state;
    final inside = st?.searchContents ?? false;
    // Search holds a value, so a skin sinks it into its well — black glass
    // under Unibody, which is why the ink comes from the well.
    final skin = context.skin;
    return SizedBox(
      width: 340,
      height: 42,
      child: Container(
        padding: const EdgeInsets.fromLTRB(16, 0, 10, 0),
        decoration: skin.surface(SurfaceRole.well, radius: 21) ??
            BoxDecoration(
              color: b.card,
              borderRadius: BorderRadius.circular(21),
              border: Border.all(color: b.hairline, width: 1.5),
            ),
        child: Row(
          children: [
            Icon(skin.icon(Icons.search),
                size: 18, color: skin.wellInkDim ?? b.inkDim),
            const SizedBox(width: 10),
            Expanded(
              child: TextField(
                controller: search,
                style: TextStyle(fontSize: 14, color: skin.wellInk ?? b.ink),
                decoration: InputDecoration(
                  border: InputBorder.none,
                  isCollapsed: true,
                  // The theme fills fields under a skin; this one sits in its
                  // own well already.
                  filled: false,
                  hintText: inside
                      ? 'Search inside books…'
                      : 'Search by title, author, genre…',
                  hintStyle: TextStyle(
                      fontSize: 14, color: skin.wellInkDim ?? b.inkDim),
                ),
                onSubmitted: (v) =>
                    controller.send(BooksCmd.search(text: v.trim())),
              ),
            ),
            IconButton(
              iconSize: 16,
              visualDensity: VisualDensity.compact,
              tooltip:
                  inside ? 'Searching contents' : 'Search contents instead',
              icon: Icon(Icons.manage_search,
                  color: inside ? BookTheme.accent : b.inkDim),
              onPressed: () =>
                  controller.send(BooksCmd.setSearchContents(on_: !inside)),
            ),
          ],
        ),
      ),
    );
  }
}

class _LibraryMenu extends StatelessWidget {
  const _LibraryMenu({required this.controller, required this.trashed});

  final BooksController controller;
  final int trashed;

  @override
  Widget build(BuildContext context) {
    return PopupMenuButton<String>(
      tooltip: 'Library',
      icon: Icon(Icons.more_vert, color: context.book.inkDim),
      onSelected: (v) async {
        switch (v) {
          case 'add':
            final path = await pickDirectory();
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
          case 'empty':
            if (!context.mounted) return;
            final ok = await confirmAction(
              context,
              title: 'Empty the trash?',
              body: 'Removes $trashed books from the library. The files '
                  'themselves are left alone.',
              action: 'Empty',
            );
            if (ok) await controller.send(const BooksCmd.emptyTrash());
        }
      },
      itemBuilder: (_) => [
        const PopupMenuItem(value: 'add', child: Text('Add a books folder…')),
        const PopupMenuItem(value: 'folders', child: Text('Watched folders…')),
        const PopupMenuDivider(),
        const PopupMenuItem(value: 'scan', child: Text('Rescan the library')),
        const PopupMenuItem(
            value: 'art', child: Text('Build missing cover art')),
        const PopupMenuItem(
            value: 'index', child: Text('Index contents for search')),
        if (trashed > 0) ...[
          const PopupMenuDivider(),
          const PopupMenuItem(value: 'empty', child: Text('Empty the trash')),
        ],
      ],
    );
  }
}

// ── hero row ─────────────────────────────────────────────────────────────────

class _HeroRow extends StatelessWidget {
  const _HeroRow({
    required this.controller,
    required this.state,
    required this.slide,
    required this.onSlide,
  });

  final BooksController controller;
  final BooksState state;
  final int slide;
  final VoidCallback onSlide;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: 210,
      child: LayoutBuilder(
        builder: (context, box) {
          // The Slint block is a flat 680; a window narrower than that plus a
          // usable hero gets a proportional one rather than an overflow.
          final statsW = (box.maxWidth - 340).clamp(260.0, 680.0);
          return Row(
            children: [
              Expanded(
                  child: _ContinueCard(
                      controller: controller,
                      state: state,
                      slide: slide,
                      onSlide: onSlide)),
              const SizedBox(width: 20),
              SizedBox(
                  width: statsW,
                  child: _StatBlock(controller: controller, state: state)),
            ],
          );
        },
      ),
    );
  }
}

class _ContinueCard extends StatelessWidget {
  const _ContinueCard({
    required this.controller,
    required this.state,
    required this.slide,
    required this.onSlide,
  });

  final BooksController controller;
  final BooksState state;
  final int slide;
  final VoidCallback onSlide;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final hero = state.slider.isEmpty
        ? state.hero
        : state.slider[slide % state.slider.length];
    return PanelCard(
      clip: true,
      child: hero == null
          ? Center(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(Icons.menu_book_outlined, size: 40, color: b.inkDim),
                  const SizedBox(height: 6),
                  Text('Nothing in progress',
                      style: TextStyle(
                          fontSize: 16,
                          fontWeight: FontWeight.w700,
                          color: b.ink)),
                  const SizedBox(height: 6),
                  Text('Open a book to start reading',
                      style: TextStyle(fontSize: 13, color: b.inkDim)),
                ],
              ),
            )
          : LayoutBuilder(
              builder: (context, box) {
                // Mockup 195, its gutter 18, the illustration 220 and its 12,
                // and 32 of padding — the text column needs what is left, so
                // the art only appears when there is enough for both.
                final room = box.maxWidth >= 195 + 18 + 220 + 12 + 32 + 210;
                return Stack(
                  children: [
                    Padding(
                      padding: const EdgeInsets.all(16),
                      child: Row(
                        children: [
                          // The tilted hardcover, framed by the book's own hue —
                          // the Slint card lays the book down here rather than
                          // standing it up like a grid tile.
                          Container(
                            width: 195,
                            height: 178,
                            decoration: BoxDecoration(
                              borderRadius: BorderRadius.circular(14),
                              border: Border.all(
                                  color: coverHue(hero.title)
                                      .withValues(alpha: 0.5),
                                  width: 1.5),
                            ),
                            child: Book3D(
                              controller: controller,
                              book: _stand(hero),
                              mockup: Mockup.hero,
                            ),
                          ),
                          const SizedBox(width: 18),
                          Expanded(
                            child: Clipped(
                              child:
                                  _HeroText(controller: controller, hero: hero),
                            ),
                          ),
                          // The decorative illustration, theme-matched. It is the
                          // one thing on this card that is only decoration, so it
                          // is also the first thing to go when the card is narrow.
                          if (room)
                            Padding(
                              padding: const EdgeInsets.only(left: 12),
                              child: SizedBox(
                                width: 220,
                                height: 170,
                                child: Image.asset(
                                  heroArtAsset(slide, hero.id.toInt(), b.dark),
                                  fit: BoxFit.contain,
                                  errorBuilder: (_, __, ___) =>
                                      const SizedBox.shrink(),
                                ),
                              ),
                            ),
                        ],
                      ),
                    ),
                    // → slider arrow, only when more than one book is in progress.
                    // It advances the slide; Resume is what opens a book.
                    if (state.slider.length > 1)
                      Positioned(
                        right: 8,
                        top: 0,
                        bottom: 0,
                        child: Center(
                          child: _RoundIcon(
                              icon: Icons.chevron_right, onTap: onSlide),
                        ),
                      ),
                  ],
                );
              },
            ),
    );
  }

  /// The unDraw pack the Slint build embeds, decoded out of `hero_art.rs`.
  /// Index 0 is the reading-nook scene and belongs to the first slide; the rest
  /// are picked by the book's own id, so a given book always draws the same one.
  static String heroArtAsset(int slide, int id, bool dark) {
    final ix = slide == 0 ? 0 : 1 + (id.abs() % 5);
    return 'assets/bookhero/heroart-${dark ? "dark" : "light"}-$ix.png';
  }

  /// The hero carries its own cover path, so a synthetic Book is enough to
  /// reuse the cover widget rather than duplicating its fallback.
  static Book _stand(HeroBook h) => Book(
        id: h.id,
        title: h.title,
        author: h.author,
        series: '',
        genre: '',
        format: '',
        cover: h.cover,
        published: '',
        percent: h.percent,
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
}

class _HeroText extends StatelessWidget {
  const _HeroText({required this.controller, required this.hero});

  final BooksController controller;
  final HeroBook hero;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final pct = (hero.percent.clamp(0.0, 1.0) * 100);
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text('CONTINUE READING',
            style: TextStyle(
                fontSize: 11,
                height: 1.3,
                fontWeight: FontWeight.w700,
                letterSpacing: 1,
                color: BookTheme.accent)),
        const SizedBox(height: 6),
        SizedBox(
          height: 32,
          child: Align(
            alignment: Alignment.centerLeft,
            child: Text(hero.title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 24,
                    height: 1.15,
                    fontWeight: FontWeight.w800,
                    color: b.ink)),
          ),
        ),
        const SizedBox(height: 6),
        Text(hero.author,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
                fontSize: 14,
                height: 1.2,
                fontWeight: FontWeight.w500,
                color: b.inkDim)),
        const SizedBox(height: 6),
        // A thick bar whose track fades out towards the right edge.
        SizedBox(
          height: 16,
          child: Stack(
            children: [
              Positioned.fill(
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(8),
                    gradient: LinearGradient(
                      colors: [b.track, b.track, b.track.withValues(alpha: 0)],
                      stops: const [0, 0.7, 1],
                    ),
                  ),
                ),
              ),
              // Positioned.fill, because a non-positioned child of a Stack is
              // laid out loose — and a DecoratedBox with no child takes the
              // smallest height it is offered, which is none of it. That is
              // why the gradient was not showing.
              Positioned.fill(
                child: FractionallySizedBox(
                  alignment: Alignment.centerLeft,
                  widthFactor: hero.percent.clamp(0.0, 1.0),
                  heightFactor: 1,
                  child: const DecoratedBox(
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.all(Radius.circular(8)),
                      gradient: LinearGradient(
                          colors: [Color(0xFF6C4DF6), Color(0xFFB9B0FF)]),
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: 6),
        Text(
          'Page ${hero.page + 1} of ${hero.total} • ${pct.round()}%'
          '${hero.timeLeft.isEmpty ? "" : "   ${hero.timeLeft}"}',
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(fontSize: 12, height: 1.2, color: b.inkDim),
        ),
        const SizedBox(height: 12),
        Row(
          children: [
            PillButton(
              label: 'Resume Reading',
              icon: Icons.menu_book_outlined,
              filled: true,
              onTap: () => controller.send(BooksCmd.openBook(id: hero.id)),
            ),
            const SizedBox(width: 10),
            PillButton(
              label: 'Details',
              onTap: () => controller.send(BooksCmd.openDetail(id: hero.id)),
            ),
          ],
        ),
      ],
    );
  }
}

class _RoundIcon extends StatefulWidget {
  const _RoundIcon({required this.icon, required this.onTap});

  final IconData icon;
  final VoidCallback onTap;

  @override
  State<_RoundIcon> createState() => _RoundIconState();
}

class _RoundIconState extends State<_RoundIcon> {
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
          width: 34,
          height: 34,
          alignment: Alignment.center,
          decoration: context.skin
                  .control(active: false, hovered: _hover, radius: 17) ??
              BoxDecoration(
                color: _hover ? b.pillBg : b.card,
                shape: BoxShape.circle,
                border: Border.all(color: b.hairline),
              ),
          child: Icon(context.skin.icon(widget.icon), size: 18, color: b.ink),
        ),
      ),
    );
  }
}

/// Six fixed identities in two rows of three; only the values flow in. The
/// third column — Time Read and Streak — opens the reading-stats panel.
class _StatBlock extends StatelessWidget {
  const _StatBlock({required this.controller, required this.state});

  final BooksController controller;
  final BooksState state;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final s = state.stats;
    final streak = controller.streakDays;

    Widget card({
      required String label,
      required String value,
      required String sub,
      required Color tint,
      required Color accent,
      required IconData icon,
      required VoidCallback onTap,
    }) =>
        Expanded(
          child: StatCard(
            label: label,
            value: value,
            sub: sub,
            tint: tint,
            accent: accent,
            icon: icon,
            onTap: onTap,
          ),
        );

    void quick(String q) => controller.send(BooksCmd.setQuick(quick: q));
    void stats() => controller.send(const BooksCmd.openStats());

    return Column(
      children: [
        Expanded(
          child: Row(
            children: [
              card(
                label: 'Total Books',
                value: '${s.total}',
                sub: s.addedMonth > 0 ? '+${s.addedMonth} this month' : '',
                tint: b.tintViolet,
                accent: BookTheme.violet,
                icon: Icons.menu_book_outlined,
                onTap: () => quick('all'),
              ),
              const SizedBox(width: 12),
              card(
                label: 'Authors',
                value: '${s.authors}',
                sub: s.authorsMonth > 0 ? '+${s.authorsMonth} new' : '',
                tint: b.tintPink,
                accent: BookTheme.pink,
                icon: Icons.person_outline,
                onTap: stats,
              ),
              const SizedBox(width: 12),
              card(
                label: 'Time Read',
                value: '${s.hoursRead.toStringAsFixed(1)}h',
                sub: '',
                tint: b.tintBlue,
                accent: BookTheme.blue,
                icon: Icons.schedule,
                onTap: stats,
              ),
            ],
          ),
        ),
        const SizedBox(height: 12),
        Expanded(
          child: Row(
            children: [
              card(
                label: 'In Progress',
                value: '${s.inProgress}',
                sub: '',
                tint: b.tintOrange,
                accent: BookTheme.amber,
                icon: Icons.circle_outlined,
                onTap: () => quick('reading'),
              ),
              const SizedBox(width: 12),
              card(
                label: 'Completed',
                value: '${s.finished}',
                sub: '',
                tint: b.tintGreen,
                accent: BookTheme.green,
                icon: Icons.check_circle_outline,
                onTap: () => quick('finished'),
              ),
              const SizedBox(width: 12),
              card(
                label: 'Streak',
                // The bridge's LibraryStats has no streak — ReadingStats does,
                // so the number appears once that panel has been opened and
                // stays. Until then the card says so rather than inventing one.
                value: streak == null ? '—' : '${streak}d',
                sub: '',
                tint: b.tintViolet,
                accent: BookTheme.pink,
                icon: Icons.local_fire_department_outlined,
                onTap: stats,
              ),
            ],
          ),
        ),
      ],
    );
  }
}

// ── main row ─────────────────────────────────────────────────────────────────

class _MainRow extends StatelessWidget {
  const _MainRow({
    required this.controller,
    required this.state,
    required this.panel,
    required this.collectionName,
    required this.onPanel,
    required this.onCollection,
  });

  final BooksController controller;
  final BooksState state;
  final String panel;
  final String collectionName;
  final ValueChanged<String> onPanel;
  final ValueChanged<String> onCollection;

  @override
  Widget build(BuildContext context) {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              _Toolbar(
                controller: controller,
                state: state,
                panel: panel,
                collectionName: collectionName,
                onPanel: onPanel,
              ),
              const SizedBox(height: 16),
              Expanded(child: _panelBody(context)),
            ],
          ),
        ),
        const SizedBox(width: 20),
        SizedBox(
          width: 300,
          child: _Rail(controller: controller, state: state),
        ),
      ],
    );
  }

  Widget _panelBody(BuildContext context) {
    switch (panel) {
      case 'collections':
        return _CollectionsPanel(
          controller: controller,
          state: state,
          onOpen: (name) {
            onCollection(name);
            onPanel('library');
          },
        );
      case 'series':
        return _SeriesPanel(
          controller: controller,
          state: state,
          onOpen: () => onPanel('library'),
        );
      default:
        return _Grid(controller: controller, state: state);
    }
  }
}

/// The filters the rail can set, as pills at the end of the tab strip — each
/// one says what is on and clears it.
List<Widget> activeFilters(
  BooksController controller,
  BooksState state,
  String panel,
  String collectionName,
) {
  final out = <Widget>[];
  void pill(IconData icon, Color hue, String label, VoidCallback clear) {
    out.add(const SizedBox(width: 8));
    out.add(_FilterPill(
      icon: icon,
      hue: hue,
      label: label,
      count: state.filteredTotal.toInt(),
      onClear: clear,
    ));
  }

  void clear() => controller.send(const BooksCmd.clearFilters());

  if (state.activeAuthor.isNotEmpty) {
    pill(Icons.person_outline, BookTheme.pink, state.activeAuthor, clear);
  }
  // A file-type filter has no banner in the Slint page, because there it lives
  // in a dropdown you just opened. Here it can also be set from the rail and
  // then forgotten, and a forgotten format filter is indistinguishable from a
  // library that has only one format in it.
  if (state.activeFormat.isNotEmpty) {
    pill(
        Icons.description_outlined,
        BookTheme.blue,
        state.activeFormat.toUpperCase(),
        () => controller.send(BooksCmd.setFormat(format: state.activeFormat)));
  }
  if (panel == 'library' && state.activeSeries.isNotEmpty) {
    pill(Icons.menu_book_outlined, BookTheme.accent, state.activeSeries, clear);
  }
  if (panel == 'library' && state.activeCollection != 0) {
    pill(Icons.bookmark_outline, BookTheme.accent, collectionName, clear);
  }
  return out;
}

class _Toolbar extends StatelessWidget {
  const _Toolbar({
    required this.controller,
    required this.state,
    required this.panel,
    required this.collectionName,
    required this.onPanel,
  });

  final BooksController controller;
  final BooksState state;
  final String panel;
  final String collectionName;
  final ValueChanged<String> onPanel;

  @override
  Widget build(BuildContext context) {
    final st = state;
    final library = panel == 'library' && st.view != 'trash';

    void quick(String q) {
      onPanel('library');
      if (st.view == 'trash') {
        controller.send(const BooksCmd.setView(view: 'library'));
      }
      controller.send(BooksCmd.setQuick(quick: q));
    }

    // The tabs scroll; the view toggle, Filters and the pager stay pinned to
    // the right edge, which is what the Slint row's stretching spacer does.
    return PanelCard(
      height: 60,
      padding: const EdgeInsets.symmetric(horizontal: 14),
      child: Row(
        children: [
          // Expanded, not Flexible: a Flexible scroll view shrink-wraps its
          // content, so the toolbar's right-hand cluster ended up beside the
          // tabs in the middle of the row instead of on the right edge.
          Expanded(
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: Row(
                children: [
                  // Tabs are mutually exclusive: "All" only lights up when no
                  // series, collection or author filter is on.
                  BookFilterChip(
                    label: 'All',
                    hue: BookTheme.violet,
                    active: library &&
                        (st.activeQuick == 'all' || st.activeQuick.isEmpty) &&
                        st.activeSeries.isEmpty &&
                        st.activeCollection == 0 &&
                        st.activeAuthor.isEmpty,
                    onTap: () => quick('all'),
                  ),
                  const SizedBox(width: 8),
                  BookFilterChip(
                    label: 'Reading',
                    hue: BookTheme.blue,
                    active: library && st.activeQuick == 'reading',
                    onTap: () => quick('reading'),
                  ),
                  const SizedBox(width: 8),
                  BookFilterChip(
                    label: 'Unread',
                    hue: BookTheme.amber,
                    active: library && st.activeQuick == 'unread',
                    onTap: () => quick('unread'),
                  ),
                  const SizedBox(width: 8),
                  BookFilterChip(
                    label: 'Collections',
                    hue: BookTheme.teal,
                    count: st.collections.length,
                    active: panel == 'collections',
                    onTap: () => onPanel('collections'),
                  ),
                  const SizedBox(width: 8),
                  BookFilterChip(
                    label: 'Favorites',
                    hue: BookTheme.pink,
                    active: library && st.activeQuick == 'favorite',
                    onTap: () => quick('favorite'),
                  ),
                  if (st.series.isNotEmpty) ...[
                    const SizedBox(width: 8),
                    BookFilterChip(
                      label: 'Series',
                      hue: BookTheme.green,
                      count: st.series.length,
                      active: panel == 'series',
                      onTap: () => onPanel('series'),
                    ),
                  ],
                  const SizedBox(width: 8),
                  BookFilterChip(
                    label: 'Trash',
                    hue: BookTheme.danger,
                    count: st.trashedCount.toInt(),
                    active: st.view == 'trash',
                    onTap: () {
                      onPanel('library');
                      controller.send(BooksCmd.setView(
                          view: st.view == 'trash' ? 'library' : 'trash'));
                    },
                  ),
                  // What the rail last set, and the way back out of it. The
                  // Slint page gives this its own band under the toolbar, but
                  // the grid here is a fixed 3 x 2 cut from what is left of the
                  // window: an extra band takes 56 px off every cell and the
                  // tiles lose their bottom block. Riding along at the end of
                  // the tab strip costs the grid nothing.
                  ...activeFilters(controller, state, panel, collectionName),
                ],
              ),
            ),
          ),
          const SizedBox(width: 24),
          _ViewToggle(controller: controller, state: st),
          const SizedBox(width: 8),
          _FiltersButton(controller: controller, state: st),
          if (st.pageCount > 1) ...[
            const SizedBox(width: 8),
            _Pager(controller: controller, state: st),
          ],
        ],
      ),
    );
  }
}

class _ViewToggle extends StatelessWidget {
  const _ViewToggle({required this.controller, required this.state});

  final BooksController controller;
  final BooksState state;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    Widget half(String mode, IconData icon) {
      final on = state.viewMode == mode;
      return GestureDetector(
        onTap: () => controller.send(BooksCmd.setViewMode(mode: mode)),
        child: MouseRegion(
          cursor: SystemMouseCursors.click,
          child: Container(
            width: 32,
            height: 28,
            alignment: Alignment.center,
            decoration: (on
                    ? context.skin.control(
                        active: true, tint: BookTheme.accent, radius: 14)
                    : null) ??
                BoxDecoration(
                  color: on ? b.tintViolet : Colors.transparent,
                  borderRadius: BorderRadius.circular(14),
                ),
            child: Icon(context.skin.icon(icon),
                size: 16, color: on ? BookTheme.accent : b.inkDim),
          ),
        ),
      );
    }

    return Container(
      width: 74,
      height: 36,
      padding: const EdgeInsets.all(4),
      decoration: context.skin.surface(SurfaceRole.card, radius: 18) ??
          BoxDecoration(
            color: b.card,
            borderRadius: BorderRadius.circular(18),
            border: Border.all(color: b.hairline),
          ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          half('grid', Icons.grid_view),
          const SizedBox(width: 2),
          half('list', Icons.view_list),
        ],
      ),
    );
  }
}

/// File type and Sort, in one popover — the toolbar's "Filters" button.
class _FiltersButton extends StatelessWidget {
  const _FiltersButton({required this.controller, required this.state});

  final BooksController controller;
  final BooksState state;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return GestureDetector(
      onTap: () => _open(context),
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        child: Container(
          height: 36,
          padding: const EdgeInsets.symmetric(horizontal: 14),
          decoration: context.skin.control(
                active: false,
                tint: BookTheme.accent,
                radius: 18,
              ) ??
              BoxDecoration(
                color: b.card,
                borderRadius: BorderRadius.circular(18),
                border: Border.all(
                    color: BookTheme.accent.withValues(alpha: 0.35),
                    width: 1.5),
              ),
          child: const Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(Icons.tune, size: 15, color: BookTheme.accent),
              SizedBox(width: 6),
              Text('Filters',
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      color: BookTheme.accent)),
            ],
          ),
        ),
      ),
    );
  }

  Future<void> _open(BuildContext context) async {
    final b = context.book;
    final box = context.findRenderObject() as RenderBox?;
    final overlay =
        Overlay.of(context).context.findRenderObject() as RenderBox?;
    if (box == null || overlay == null) return;
    final topLeft =
        box.localToGlobal(Offset(0, box.size.height + 6), ancestor: overlay);
    await showDialog<void>(
      context: context,
      barrierColor: Colors.transparent,
      builder: (dialogContext) => Stack(
        children: [
          Positioned(
            left: (topLeft.dx - 260 + box.size.width)
                .clamp(8.0, overlay.size.width - 268),
            top: topLeft.dy.clamp(8.0, overlay.size.height - 220),
            width: 260,
            child: Material(
              color: b.card,
              borderRadius: BorderRadius.circular(12),
              elevation: 8,
              child: Padding(
                padding: const EdgeInsets.all(14),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Text('File type',
                        style: TextStyle(
                            fontSize: 12,
                            fontWeight: FontWeight.w600,
                            color: b.inkDim)),
                    const SizedBox(height: 8),
                    DropdownButton<String>(
                      isExpanded: true,
                      value:
                          state.activeFormat.isEmpty ? '' : state.activeFormat,
                      items: [
                        const DropdownMenuItem(value: '', child: Text('All')),
                        for (final f in state.formats)
                          DropdownMenuItem(
                              value: f.key, child: Text(f.label.toUpperCase())),
                      ],
                      onChanged: (v) {
                        controller.send(BooksCmd.setFormat(format: v ?? ''));
                        Navigator.of(dialogContext).pop();
                      },
                    ),
                    const SizedBox(height: 8),
                    Text('Sort by',
                        style: TextStyle(
                            fontSize: 12,
                            fontWeight: FontWeight.w600,
                            color: b.inkDim)),
                    const SizedBox(height: 8),
                    DropdownButton<int>(
                      isExpanded: true,
                      value: state.sortIndex.toInt(),
                      items: [
                        for (var i = 0; i < sortOptions.length; i++)
                          DropdownMenuItem(
                              value: i, child: Text(sortOptions[i])),
                      ],
                      onChanged: (v) {
                        controller.send(BooksCmd.setSort(index: v ?? 0));
                        Navigator.of(dialogContext).pop();
                      },
                    ),
                    const SizedBox(height: 10),
                    FilledButton(
                      style: FilledButton.styleFrom(
                          backgroundColor: BookTheme.accent),
                      onPressed: () => Navigator.of(dialogContext).pop(),
                      child: const Text('Apply'),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _Pager extends StatelessWidget {
  const _Pager({required this.controller, required this.state});

  final BooksController controller;
  final BooksState state;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final page = state.page.toInt();
    final pages = state.pageCount.toInt();

    Widget step(IconData icon, bool live, int to) => GestureDetector(
          onTap:
              live ? () => controller.send(BooksCmd.setPage(page: to)) : null,
          child: MouseRegion(
            cursor: live ? SystemMouseCursors.click : SystemMouseCursors.basic,
            child: Container(
              width: 32,
              height: 32,
              alignment: Alignment.center,
              decoration: (live
                      ? context.skin.control(
                          active: false, tint: BookTheme.accent, radius: 16)
                      : null) ??
                  BoxDecoration(
                    color: live ? b.card : b.pillBg,
                    shape: BoxShape.circle,
                    border: Border.all(color: b.hairline),
                  ),
              child: Icon(context.skin.icon(icon), size: 16, color: b.ink),
            ),
          ),
        );

    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        step(Icons.chevron_left, page > 1, page - 1),
        const SizedBox(width: 6),
        Text('$page / $pages',
            style: TextStyle(
                fontSize: 13, fontWeight: FontWeight.w700, color: b.ink)),
        const SizedBox(width: 6),
        step(Icons.chevron_right, page < pages, page + 1),
      ],
    );
  }
}

class _FilterPill extends StatelessWidget {
  const _FilterPill({
    required this.icon,
    required this.hue,
    required this.label,
    required this.count,
    required this.onClear,
  });

  final IconData icon;
  final Color hue;
  final String label;
  final int count;
  final VoidCallback onClear;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onClear,
        child: Container(
          height: 34,
          constraints: const BoxConstraints(maxWidth: 280),
          padding: const EdgeInsets.symmetric(horizontal: 12),
          // An applied filter is a latched key in its own hue.
          decoration: context.skin
                  .control(active: true, tint: hue, radius: 17) ??
              BoxDecoration(
                color: hue.withValues(alpha: 0.12),
                borderRadius: BorderRadius.circular(17),
                border: Border.all(color: hue.withValues(alpha: 0.45)),
              ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(context.skin.icon(icon), size: 14, color: hue),
              const SizedBox(width: 6),
              Flexible(
                child: Text('$label  ·  $count',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 12,
                        fontWeight: FontWeight.w700,
                        color: b.ink)),
              ),
              const SizedBox(width: 6),
              Icon(Icons.close, size: 13, color: b.inkDim),
            ],
          ),
        ),
      ),
    );
  }
}

// ── the grid ─────────────────────────────────────────────────────────────────

/// Full-bleed: grid mode is 3 × 2 big horizontal cards cut from the viewport,
/// list mode is six half-width rows centred in the column. Six either way,
/// which is the bridge's page size.
class _Grid extends StatelessWidget {
  const _Grid({required this.controller, required this.state});

  final BooksController controller;
  final BooksState state;

  static const double _gap = 16;

  @override
  Widget build(BuildContext context) {
    final st = state;
    if (st.books.isEmpty) return _Empty(controller: controller, state: st);

    final list = st.viewMode == 'list';
    final cols = list ? 1 : 3;
    final rows = list ? 6 : 2;

    return LayoutBuilder(
      builder: (context, box) {
        final cellW = (list
                ? box.maxWidth * 0.5
                : (box.maxWidth - (cols - 1) * _gap) / cols)
            .clamp(1.0, double.infinity);
        final cellH = ((box.maxHeight - (rows - 1) * _gap) / rows)
            .clamp(1.0, double.infinity);
        return Stack(
          children: [
            for (var i = 0; i < st.books.length && i < cols * rows; i++)
              Positioned(
                left: list
                    ? (box.maxWidth - cellW) / 2
                    : (i % cols) * (cellW + _gap),
                top: (i ~/ cols) * (cellH + _gap),
                width: cellW,
                height: cellH,
                child: _tile(context, st.books[i], list),
              ),
          ],
        );
      },
    );
  }

  Widget _tile(BuildContext context, Book b, bool list) {
    // A card opens the detail popup, the same as the Slint tile's `open`.
    // Reading a book is what Resume, and the popup's own Open, are for.
    void open() => controller.send(BooksCmd.openDetail(id: b.id));
    void author() => controller.send(BooksCmd.setAuthor(author: b.author));
    void rate(double v) =>
        controller.send(BooksCmd.setRating(id: b.id, rating: v));
    void favourite() => controller.send(BooksCmd.toggleFavorite(id: b.id));
    void addCollection(CollectionRow c) =>
        controller.send(BooksCmd.collectionToggle(id: c.id, bookId: b.id));
    void menu(Offset at) => _bookMenu(context, controller, b, at);

    return list
        ? BookListRow(
            controller: controller,
            book: b,
            collections: state.collections,
            onOpen: open,
            onAuthor: author,
            onRate: rate,
            onFavorite: favourite,
            onAddCollection: addCollection,
            onMenu: menu,
          )
        : BookGridTile(
            controller: controller,
            book: b,
            collections: state.collections,
            onOpen: open,
            onAuthor: author,
            onRate: rate,
            onFavorite: favourite,
            onAddCollection: addCollection,
            onMenu: menu,
          );
  }
}

/// The card ⋮ menu. One menu for every card, opened where the dot is — the
/// Slint page keeps it at page root for the same reason: a grid refresh that
/// drops the row must not be able to tear down the open menu's parent.
Future<void> _bookMenu(
  BuildContext context,
  BooksController controller,
  Book book,
  Offset at,
) async {
  final overlay = Overlay.of(context).context.findRenderObject() as RenderBox?;
  if (overlay == null) return;
  final choice = await showMenu<String>(
    context: context,
    position: RelativeRect.fromLTRB(
      at.dx - 170,
      at.dy + 32,
      overlay.size.width - at.dx,
      0,
    ),
    items: [
      if (!book.trashed) ...[
        const PopupMenuItem(value: 'details', child: Text('Details')),
        PopupMenuItem(
          value: 'favorite',
          child: Text(book.favorite ? 'Remove Favorite' : 'Add to Favorites'),
        ),
        PopupMenuItem(
          value: 'magazine',
          child: Text(book.magazine ? 'Not a magazine' : 'Treat as a magazine'),
        ),
        PopupMenuItem(
          value: 'rtl',
          child: Text(book.rtl ? 'Left-to-right pages' : 'Right-to-left pages'),
        ),
        const PopupMenuDivider(),
        const PopupMenuItem(value: 'trash', child: Text('Move to Trash')),
      ] else ...[
        const PopupMenuItem(
            value: 'restore', child: Text('Restore to Library')),
        const PopupMenuItem(value: 'delete', child: Text('Delete Permanently')),
      ],
    ],
  );
  switch (choice) {
    case 'details':
      await controller.send(BooksCmd.openDetail(id: book.id));
    case 'favorite':
      await controller.send(BooksCmd.toggleFavorite(id: book.id));
    case 'magazine':
      await controller.send(BooksCmd.toggleMagazine(id: book.id));
    case 'rtl':
      await controller.send(BooksCmd.toggleRtl(id: book.id));
    case 'trash':
      await controller.send(BooksCmd.trash(id: book.id));
    case 'restore':
      await controller.send(BooksCmd.restore(id: book.id));
    case 'delete':
      await controller.send(BooksCmd.deletePerm(id: book.id));
  }
}

class _Empty extends StatelessWidget {
  const _Empty({required this.controller, required this.state});

  final BooksController controller;
  final BooksState state;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final trash = state.view == 'trash';
    final filtered = state.activeFormat.isNotEmpty ||
        state.activeGenre.isNotEmpty ||
        state.activeSeries.isNotEmpty ||
        state.activeAuthor.isNotEmpty ||
        state.activeCollection != 0 ||
        (state.activeQuick != 'all' && state.activeQuick.isNotEmpty) ||
        state.query.isNotEmpty;

    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            trash
                ? Icons.delete_outline
                : (filtered ? Icons.search : Icons.menu_book_outlined),
            size: 48,
            color: b.inkDim,
          ),
          const SizedBox(height: 8),
          Text(
            trash
                ? 'The trash is empty'
                : (filtered ? 'No books match your filters' : 'No books here'),
            style: TextStyle(
                fontSize: 16, fontWeight: FontWeight.w700, color: b.ink),
          ),
          const SizedBox(height: 6),
          Text(
            trash
                ? 'Books you move to the trash appear here.'
                : (filtered
                    ? 'Try clearing a filter or search'
                    : 'Add a folder to start your library'),
            style: TextStyle(fontSize: 13, color: b.inkDim),
          ),
          if (!trash && !filtered) ...[
            const SizedBox(height: 18),
            PillButton(
              label: 'Add a books folder',
              icon: Icons.add,
              filled: true,
              onTap: () async {
                final path = await pickDirectory();
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

// ── the collections and series panels ────────────────────────────────────────

class _CollectionsPanel extends StatefulWidget {
  const _CollectionsPanel({
    required this.controller,
    required this.state,
    required this.onOpen,
  });

  final BooksController controller;
  final BooksState state;
  final ValueChanged<String> onOpen;

  @override
  State<_CollectionsPanel> createState() => _CollectionsPanelState();
}

class _CollectionsPanelState extends State<_CollectionsPanel> {
  final TextEditingController _name = TextEditingController();
  final TextEditingController _smart = TextEditingController();

  @override
  void dispose() {
    _name.dispose();
    _smart.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final st = widget.state;
    return ListView(
      padding: EdgeInsets.zero,
      children: [
        _NewRow(
          field: _name,
          hint: 'New collection…',
          label: 'Create',
          icon: Icons.add,
          onSubmit: (v) {
            if (v.trim().isEmpty) return;
            widget.controller.send(BooksCmd.collectionCreate(name: v.trim()));
            _name.clear();
          },
        ),
        const SizedBox(height: 12),
        for (final c in st.collections) ...[
          EntryCard(
            label: c.name,
            count: c.count.toInt(),
            icon: Icons.bookmark_outline,
            onTap: () {
              widget.controller.send(BooksCmd.setCollection(id: c.id));
              widget.onOpen(c.name);
            },
            onDelete: () =>
                widget.controller.send(BooksCmd.collectionDelete(id: c.id)),
          ),
          const SizedBox(height: 8),
        ],
        if (st.collections.isEmpty)
          Text('No collections yet — create one above.',
              style: TextStyle(fontSize: 13, color: b.inkDim)),
        const SizedBox(height: 16),
        // A smart collection is a saved *filter*, so new books that match join
        // it on their own.
        Text('Smart collections',
            style: TextStyle(
                fontSize: 15, fontWeight: FontWeight.w700, color: b.ink)),
        const SizedBox(height: 4),
        Text(
            'Saves the current filters and search, not a fixed list — matching '
            'books are added automatically.',
            style: TextStyle(fontSize: 12, color: b.inkDim)),
        const SizedBox(height: 8),
        _NewRow(
          field: _smart,
          hint: 'Name this view…',
          label: 'Save',
          icon: Icons.bookmark_add_outlined,
          onSubmit: (v) {
            if (v.trim().isEmpty) return;
            widget.controller
                .send(BooksCmd.smartCreateFromFilters(name: v.trim()));
            _smart.clear();
          },
        ),
        const SizedBox(height: 12),
        for (final s in st.smart) ...[
          EntryCard(
            label: s.name,
            count: -1,
            icon: Icons.search,
            onTap: () {
              widget.controller.send(BooksCmd.smartApply(id: s.id));
              widget.onOpen('');
            },
            onDelete: () =>
                widget.controller.send(BooksCmd.smartDelete(id: s.id)),
          ),
          const SizedBox(height: 8),
        ],
        if (st.smart.isEmpty)
          Text(
              'No smart collections yet — filter the library, then save the '
              'view.',
              style: TextStyle(fontSize: 13, color: b.inkDim)),
      ],
    );
  }
}

class _NewRow extends StatelessWidget {
  const _NewRow({
    required this.field,
    required this.hint,
    required this.label,
    required this.icon,
    required this.onSubmit,
  });

  final TextEditingController field;
  final String hint;
  final String label;
  final IconData icon;
  final ValueChanged<String> onSubmit;

  @override
  Widget build(BuildContext context) => Row(
        children: [
          Expanded(
            child: SizedBox(
              height: 36,
              child: TextField(
                controller: field,
                decoration: InputDecoration(
                  isDense: true,
                  hintText: hint,
                  border: const OutlineInputBorder(),
                ),
                onSubmitted: onSubmit,
              ),
            ),
          ),
          const SizedBox(width: 8),
          SizedBox(
            width: 96,
            height: 36,
            child: FilledButton.icon(
              style: FilledButton.styleFrom(
                backgroundColor: BookTheme.accent,
                padding: EdgeInsets.zero,
                shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(10)),
              ),
              icon: Icon(icon, size: 15),
              label: Text(label, style: const TextStyle(fontSize: 13)),
              onPressed: () => onSubmit(field.text),
            ),
          ),
        ],
      );
}

class _SeriesPanel extends StatelessWidget {
  const _SeriesPanel({
    required this.controller,
    required this.state,
    required this.onOpen,
  });

  final BooksController controller;
  final BooksState state;
  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    if (state.series.isEmpty) {
      return Text('No series in your library.',
          style: TextStyle(fontSize: 13, color: b.inkDim));
    }
    return ListView.separated(
      padding: EdgeInsets.zero,
      itemCount: state.series.length,
      separatorBuilder: (_, __) => const SizedBox(height: 8),
      itemBuilder: (_, i) {
        final s = state.series[i];
        return EntryCard(
          label: s.label,
          count: s.count.toInt(),
          icon: Icons.menu_book_outlined,
          onTap: () {
            controller.send(BooksCmd.setSeries(series: s.key));
            onOpen();
          },
        );
      },
    );
  }
}

// ── the right rail ───────────────────────────────────────────────────────────

class _Rail extends StatelessWidget {
  const _Rail({required this.controller, required this.state});

  final BooksController controller;
  final BooksState state;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final st = state;
    return PanelCard(
      child: ListView(
        padding: const EdgeInsets.all(18),
        children: [
          RailBlock(
            title: 'QUICK FILTERS',
            children: [
              for (final q in quickFilters)
                RailRow(
                  icon: q.icon,
                  label: q.label,
                  active: st.activeQuick == q.id && st.view != 'trash',
                  iconTint: switch (q.id) {
                    'reading' => BookTheme.blue,
                    'finished' => BookTheme.green,
                    'favorite' => BookTheme.amber,
                    _ => BookTheme.accent,
                  },
                  // A quick filter clicked from the trash view has to leave
                  // it, or nothing appears to happen.
                  onTap: () async {
                    if (st.view == 'trash') {
                      await controller
                          .send(const BooksCmd.setView(view: 'library'));
                    }
                    await controller.send(BooksCmd.setQuick(quick: q.id));
                  },
                ),
            ],
          ),
          const SizedBox(height: 20),
          if (st.formats.isNotEmpty) ...[
            RailBlock(
              title: 'FILE TYPES',
              children: [
                for (final f in st.formats)
                  RailRow(
                    icon: Icons.menu_book_outlined,
                    label: f.label.toUpperCase(),
                    count: f.count.toInt(),
                    active: st.activeFormat == f.key,
                    onTap: () =>
                        controller.send(BooksCmd.setFormat(format: f.key)),
                  ),
              ],
            ),
            const SizedBox(height: 20),
          ],
          if (st.genres.isNotEmpty) ...[
            Row(
              children: [
                Expanded(
                  child: Text('GENRES',
                      style: TextStyle(
                          fontSize: 11,
                          fontWeight: FontWeight.w700,
                          letterSpacing: 0.6,
                          color: b.inkDim)),
                ),
                GestureDetector(
                  onTap: () =>
                      controller.send(const BooksCmd.setGenre(genre: '')),
                  child: const Text('View all',
                      style: TextStyle(
                          fontSize: 11,
                          fontWeight: FontWeight.w700,
                          color: BookTheme.accent)),
                ),
              ],
            ),
            const SizedBox(height: 6),
            Wrap(
              spacing: 6,
              runSpacing: 6,
              children: [
                for (var i = 0; i < st.genres.length; i++)
                  GenreChip(
                    label: st.genres[i].label,
                    count: st.genres[i].count.toInt(),
                    active: st.activeGenre == st.genres[i].key,
                    hue: BookTheme.genreHues[i % BookTheme.genreHues.length],
                    onTap: () => controller
                        .send(BooksCmd.setGenre(genre: st.genres[i].key)),
                  ),
              ],
            ),
          ],
          const SizedBox(height: 4),
        ],
      ),
    );
  }
}

// ── the three banners ────────────────────────────────────────────────────────

class _ScanBar extends StatelessWidget {
  const _ScanBar({required this.controller});

  final BooksController controller;

  @override
  Widget build(BuildContext context) {
    final p = controller.progress!;
    final b = context.book;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 8),
      decoration: BoxDecoration(
        color: BookTheme.accent.withValues(alpha: 0.10),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        children: [
          SizedBox(
            width: 160,
            child: LinearProgressIndicator(
              value: p.total > 0 ? p.done / p.total : null,
              minHeight: 4,
              backgroundColor: b.track,
              valueColor: const AlwaysStoppedAnimation(BookTheme.accent),
            ),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Text('Scanning ${p.done} / ${p.total}  ·  ${p.name}',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12, color: b.inkDim)),
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
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 4),
        decoration: BoxDecoration(
          color: Tokens.error.withValues(alpha: 0.14),
          borderRadius: BorderRadius.circular(12),
        ),
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
  const _StatusBanner({required this.message, required this.onClose});

  final String message;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(14, 2, 4, 2),
      decoration: BoxDecoration(
        color: b.pillBg,
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        children: [
          Expanded(
            child: Text(message,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12, color: b.inkDim)),
          ),
          IconButton(
            iconSize: 14,
            visualDensity: VisualDensity.compact,
            tooltip: 'Dismiss',
            icon: Icon(Icons.close, color: b.inkDim),
            onPressed: onClose,
          ),
        ],
      ),
    );
  }
}
