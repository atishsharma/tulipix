// Cinema — the one that leads with what you were doing.
//
// One hero at full bleed for whatever you were last watching, a shelf along the
// bottom carrying the resume rail and its kind tabs, and a glass stack on the
// right for the player and the one hub card that takes its turn under it.
//
// The rail is the hero's tab strip: picking a tab swaps the backdrop and the
// Resume button, it does not open anything. Opening is what the hero's own
// button is for.

import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../../design/app_mark.dart';
import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/home.dart';
import '../music/music_controller.dart';
import 'home_controller.dart';
import 'home_player.dart';
import 'home_shared.dart';

/// Ten seconds a card — the clock the hero slideshow and the hub carousel both
/// keep.
const Duration kCineRotate = Duration(seconds: 10);

class CinemaHome extends StatefulWidget {
  const CinemaHome({
    super.key,
    required this.controller,
    required this.state,
  });

  final HomeController controller;
  final HomeState state;

  @override
  State<CinemaHome> createState() => _CinemaHomeState();
}

class _CinemaHomeState extends State<CinemaHome> {
  int _sel = 0;
  int _hub = 0;
  Timer? _slide;
  Timer? _hubSlide;

  @override
  void initState() {
    super.initState();
    _armSlide();
    _armHub();
  }

  /// Restarted, not merely reset, on a manual step — so stepping a card by hand
  /// buys its full ten seconds instead of being overwritten half a second later.
  void _armHub() {
    _hubSlide?.cancel();
    _hubSlide = Timer.periodic(kCineRotate, (_) {
      if (mounted) setState(() => _hub += 1);
    });
  }

  void _armSlide() {
    _slide?.cancel();
    if (widget.state.continueRows.length < 2) return;
    _slide = Timer.periodic(kCineRotate, (_) {
      if (mounted) {
        setState(() =>
            _sel = (_sel + 1) % math.max(1, widget.state.continueRows.length));
      }
    });
  }

  @override
  void didUpdateWidget(CinemaHome old) {
    super.didUpdateWidget(old);
    if (old.state.continueRows.length != widget.state.continueRows.length) {
      _sel = 0;
      _armSlide();
    }
  }

  @override
  void dispose() {
    _slide?.cancel();
    _hubSlide?.cancel();
    super.dispose();
  }

  bool _on(String card) => widget.state.cards.contains(card);

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.state;
    final rows = st.continueRows;

    return LayoutBuilder(
      builder: (context, box) {
        // ── Geometry, from ui/page_home_cinema.slint ───────────────────────
        const pad = 34.0;
        const stackW = 300.0;
        final railOn = _on('continue') && rows.isNotEmpty;
        // The Continue row is 70% of the page — at half, four tabs were
        // narrower than their own titles.
        final railW = (box.maxWidth - 2 * pad) * 0.7;
        final railH = railOn ? 132.0 : 0.0;
        const railHeadH = 30.0;
        const shelfTop = 22.0;
        const shelfBottom = 28.0;
        final shelfH =
            railH > 0 ? shelfTop + railHeadH + 9 + railH + shelfBottom : 0.0;
        // Capped: filtered to one kind the strip can be a single tab, and one
        // tab half a page wide is a banner, not a tab.
        final tabW = rows.isEmpty
            ? 0.0
            : math.min(320.0,
                math.max(0.0, (railW - (rows.length - 1) * 8) / rows.length));

        final colAvail = math.max(0.0, box.maxHeight - shelfH - 96 - 20);
        // The hub band takes its slice off the TOP of the budget, not off
        // whatever the art happens to leave: the art is elastic, the tile is not.
        final hubCards = homeHubCards(st);
        final hubBand = (_on('hub') && colAvail >= 430) ? 152.0 : 0.0;
        final plAvail =
            math.max(0.0, colAvail - (hubBand > 0 ? hubBand + 16 : 0));
        final plArt =
            math.max(0.0, math.min(stackW - 26, plAvail - kCinemaChrome));
        final playerH = plArt + kCinemaChrome;

        // Wide enough for a long title to break over TWO lines rather than
        // elide at the first fifteen characters.
        final heroW = math.max(
            320.0,
            math.min(_on('player') ? 880.0 : 1080.0,
                box.maxWidth - 2 * pad - (_on('player') ? stackW + 24 : 0)));
        final heroY = (box.maxHeight - shelfH) * 0.42;

        // ── The subject ────────────────────────────────────────────────────
        final sel = rows.isEmpty ? null : rows[_sel.clamp(0, rows.length - 1)];
        final kind = sel?.kind ?? st.hero.kind;
        final title = sel?.title ?? st.hero.title;
        final kicker = sel == null
            ? st.hero.kicker
            : switch (sel.kind) {
                'video' => 'CONTINUE WATCHING',
                'book' => 'CONTINUE READING',
                _ => 'CONTINUE LISTENING',
              };
        final meta = sel == null
            ? st.hero.meta
            : (sel.author.isEmpty ? sel.sub : '${sel.author} · ${sel.sub}');
        final frac = sel?.frac ?? st.hero.frac;
        final rem = sel?.sub ?? st.hero.meta;
        final accent = kind.isEmpty ? Tokens.brand : kindColor(kind);
        final artId = sel?.id ?? st.hero.id;
        final artSection = kindSection(kind);

        return Stack(
          children: [
            // ── Backdrop ──────────────────────────────────────────────────
            if (_on('hero') && title.isNotEmpty && artId >= 0)
              Positioned.fill(
                child: LazyCover(
                  section: artSection,
                  id: artId,
                  tint: accent,
                  fit: BoxFit.cover,
                ),
              )
            else
              // No art: the app gradient stands in, so the page is never a flat
              // rectangle with text on it.
              Positioned.fill(
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    gradient: LinearGradient(
                      begin: Alignment.topLeft,
                      end: Alignment.bottomRight,
                      colors: [
                        Tokens.brand.withValues(alpha: 0.45),
                        Tokens.secVideos.withValues(alpha: 0.28),
                        t.bg,
                      ],
                    ),
                  ),
                ),
              ),
            // Side and bottom scrims. Two flat gradients rather than a blur.
            Positioned.fill(
              child: DecoratedBox(
                decoration: BoxDecoration(
                  gradient: LinearGradient(
                    begin: Alignment.centerLeft,
                    end: Alignment.centerRight,
                    colors: [
                      t.bg,
                      t.bg.withValues(alpha: 0.86),
                      t.bg.withValues(alpha: 0.18),
                      t.bg.withValues(alpha: 0.62),
                    ],
                    stops: const [0, 0.34, 0.64, 1],
                  ),
                ),
              ),
            ),
            Positioned.fill(
              child: DecoratedBox(
                decoration: BoxDecoration(
                  gradient: LinearGradient(
                    begin: Alignment.bottomCenter,
                    end: Alignment.topCenter,
                    colors: [t.bg, t.bg.withValues(alpha: 0.08)],
                    stops: const [0.02, 0.48],
                  ),
                ),
              ),
            ),

            // ── Top bar ───────────────────────────────────────────────────
            Positioned(
              left: pad,
              top: 20,
              width: box.maxWidth - 2 * pad,
              height: 34,
              child: Row(
                children: [
                  AppMark(
                      choice:
                          ShellController.instance.state?.logoChoice ?? 0),
                  const SizedBox(width: 8),
                  Text('Tulipix',
                      style: TextStyle(
                          fontSize: 15,
                          fontWeight: FontWeight.w700,
                          color: t.text)),
                  const SizedBox(width: 10),
                  // One solid pill: greeting and name are one line, and the
                  // date said nothing the rest of the page did not.
                  Container(
                    height: 26,
                    padding: const EdgeInsets.symmetric(horizontal: 13),
                    alignment: Alignment.center,
                    decoration: BoxDecoration(
                      color: kLiveTv,
                      borderRadius: BorderRadius.circular(13),
                    ),
                    child: Text(st.greeting,
                        style: const TextStyle(
                            fontSize: 11.5,
                            fontWeight: FontWeight.w700,
                            color: Colors.white)),
                  ),
                  const SizedBox(width: 10),
                  if (_on('quick')) const Expanded(child: LauncherPills()),
                ],
              ),
            ),

            // ── Hero ──────────────────────────────────────────────────────
            if (_on('hero'))
              Positioned(
                left: pad + 6,
                top: heroY,
                width: heroW,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    // The kicker says what the button will do, in the button's
                    // words; the badge carries the category in its own colour.
                    SizedBox(
                      height: 22,
                      child: Row(
                        children: [
                          AnimatedContainer(
                            duration: const Duration(milliseconds: 200),
                            height: 21,
                            padding: const EdgeInsets.symmetric(horizontal: 10),
                            alignment: Alignment.center,
                            decoration: BoxDecoration(
                              color: accent,
                              borderRadius: BorderRadius.circular(6),
                            ),
                            child: Text(
                              kind.isEmpty ? 'START' : kindLabel(kind),
                              style: const TextStyle(
                                  fontSize: 9,
                                  fontWeight: FontWeight.w800,
                                  letterSpacing: 0.8,
                                  color: Colors.white),
                            ),
                          ),
                          const SizedBox(width: 9),
                          Expanded(
                            child: Text(
                              title.isEmpty ? 'NOTHING IN PROGRESS' : kicker,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                  fontSize: 10,
                                  fontWeight: FontWeight.w600,
                                  letterSpacing: 1.4,
                                  color: t.textDim),
                            ),
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(height: 14),
                    Text(
                      title.isEmpty ? 'Your library' : title,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 44,
                          fontWeight: FontWeight.w700,
                          letterSpacing: -1.7,
                          height: 1.1,
                          color: t.text),
                    ),
                    const SizedBox(height: 12),
                    Text(
                      title.isEmpty
                          ? 'Add a folder and the newest thing you were '
                              'watching lands here.'
                          : meta,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 12.5, color: t.textDim),
                    ),
                    const SizedBox(height: 16),
                    if (frac >= 0)
                      SizedBox(
                        width: 330,
                        child:
                            ProgressPill(sub: rem, frac: frac, accent: accent),
                      ),
                    const SizedBox(height: 20),
                    Row(
                      children: [
                        HeroBtn(
                          label: title.isEmpty ? 'Browse Photos' : 'Resume',
                          icon: title.isEmpty
                              ? Icons.image_outlined
                              : Icons.play_arrow,
                          primary: true,
                          fill: accent,
                          // The hero opens what the RAIL selected; with nothing
                          // in progress it is the Browse Photos door instead.
                          onTap: () => sel == null
                              ? ShellController.instance.go(
                                  title.isEmpty ? Section.photos : artSection)
                              : continueOpen(sel),
                        ),
                        // Details is a BOOK affordance: only Books has a
                        // details panel to open. A film or an episode resumed
                        // straight into its player.
                        if (title.isNotEmpty && kind == 'book') ...[
                          const SizedBox(width: 10),
                          HeroBtn(
                            label: 'Details',
                            icon: Icons.info_outline,
                            // The panel, not the reader — and for the title the
                            // rail selected, not the backend's own hero.
                            onTap: () => ShellController.instance.goOpen(
                                Section.books, 'detail', '${sel?.id ?? st.hero.id}'),
                          ),
                        ],
                      ],
                    ),
                  ],
                ),
              ),

            // ── Right glass stack — the player ────────────────────────────
            if (_on('player'))
              Positioned(
                left: box.maxWidth - pad - stackW,
                top: 96,
                width: stackW,
                height: playerH,
                child: CinemaPlayer(height: playerH),
              ),

            // ── My Hub, in the gap under the player ───────────────────────
            // Anchored to the BOTTOM of the column, not hung off the player:
            // the art is elastic, so hanging it there would make the band drift
            // with every window resize.
            if (hubBand > 0)
              Positioned(
                left: box.maxWidth - pad - stackW,
                top: box.maxHeight - shelfH - 20 - hubBand,
                width: stackW,
                height: hubBand,
                child: _HubCarousel(
                  cards: hubCards,
                  index: _hub,
                  onStep: (d) {
                    setState(() => _hub += d);
                    _armHub();
                  },
                ),
              ),

            // ── Shelf ─────────────────────────────────────────────────────
            if (shelfH > 0)
              Positioned(
                left: 0,
                top: box.maxHeight - shelfH,
                width: box.maxWidth,
                height: shelfH,
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    gradient: LinearGradient(
                      begin: Alignment.bottomCenter,
                      end: Alignment.topCenter,
                      colors: [
                        t.bg,
                        t.bg.withValues(alpha: 0.72),
                        t.bg.withValues(alpha: 0),
                      ],
                      stops: const [0, 0.62, 1],
                    ),
                  ),
                  child: Stack(
                    children: [
                      Positioned(
                        left: pad,
                        top: shelfTop,
                        width: railW,
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.stretch,
                          children: [
                            SizedBox(
                              height: railHeadH,
                              child: Row(
                                children: [
                                  Expanded(
                                    child: Text('Continue',
                                        style: TextStyle(
                                            fontSize: 12.5,
                                            fontWeight: FontWeight.w700,
                                            color: t.text)),
                                  ),
                                  ContinueTabs(
                                    active: st.continueFilter,
                                    onPick: (id) {
                                      setState(() => _sel = 0);
                                      widget.controller.send(
                                          HomeCmd.setContinueFilter(
                                              filter: id));
                                    },
                                  ),
                                ],
                              ),
                            ),
                            const SizedBox(height: 9),
                            // 70% of the page and NOT scrollable: the tabs
                            // divide the row between them, so however many
                            // there are they all fit.
                            SizedBox(
                              height: railH,
                              child: Row(
                                children: [
                                  for (var i = 0; i < rows.length; i++) ...[
                                    if (i > 0) const SizedBox(width: 8),
                                    SizedBox(
                                      width: tabW,
                                      child: RailTile(
                                        data: rows[i],
                                        selected:
                                            i == _sel.clamp(0, rows.length - 1),
                                        // Selects, never opens — the hero
                                        // above carries Resume.
                                        onTap: () {
                                          setState(() => _sel = i);
                                          _armSlide();
                                        },
                                      ),
                                    ),
                                  ],
                                ],
                              ),
                            ),
                          ],
                        ),
                      ),
                      // Three lyric lines in the space the 70% strip leaves —
                      // no plate, no outline, no title: just the words.
                      Positioned(
                        left: pad + railW + 28,
                        top: shelfTop + railHeadH + 9,
                        right: pad,
                        height: railH,
                        child: AnimatedBuilder(
                          animation: MusicController.instance,
                          builder: (context, _) => HomeLyrics(
                            controller: MusicController.instance,
                            accent: MusicController.instance.accent,
                            big: true,
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
          ],
        );
      },
    );
  }
}

// ── One Continue tab ────────────────────────────────────────────────────────

/// A tab, not a launcher: clicking it puts that item in the hero, where the big
/// Resume button is.
class RailTile extends StatelessWidget {
  const RailTile({
    super.key,
    required this.data,
    required this.selected,
    required this.onTap,
  });

  final HomeContinue data;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final accent = kindColor(data.kind);
    return Hover(
      onTap: onTap,
      builder: (context, hov) => AnimatedContainer(
        duration: const Duration(milliseconds: 120),
        padding: const EdgeInsets.all(10),
        decoration: BoxDecoration(
          color: selected
              ? accent.withValues(alpha: 0.22)
              : (hov ? t.glassStrong : t.glass),
          borderRadius: BorderRadius.circular(11),
          border: Border.all(color: selected ? accent : t.glassBorder),
        ),
        clipBehavior: Clip.antiAlias,
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // The cover keeps a 3:4 portrait footprint whatever the medium.
            AspectRatio(
              aspectRatio: 0.72,
              child: ClipRRect(
                borderRadius: BorderRadius.circular(7),
                child: LazyCover(
                  section: kindSection(data.kind),
                  id: data.id,
                  tint: accent,
                  icon: kindIcon(data.kind),
                  iconSize: 18,
                ),
              ),
            ),
            const SizedBox(width: 11),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Align(
                    alignment: Alignment.centerLeft,
                    child: KindTag(
                        kind: data.kind,
                        accent: accent,
                        filled: true,
                        height: 17),
                  ),
                  const SizedBox(height: 5),
                  Text(data.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 13,
                          fontWeight: FontWeight.w800,
                          color: t.text)),
                  Text(data.author.isEmpty ? data.sub : data.author,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 10.5, color: t.textDim)),
                  const Spacer(),
                  ProgressPill(sub: data.sub, frac: data.frac, accent: accent),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ── My Hub, one card at a time ──────────────────────────────────────────────

/// Welcome shows all eight side by side; this column is 300px wide, so here
/// they take turns — ten seconds each, arrows to step by hand.
class _HubCarousel extends StatelessWidget {
  const _HubCarousel({
    required this.cards,
    required this.index,
    required this.onStep,
  });

  final List<HubCard> cards;
  final int index;
  final ValueChanged<int> onStep;

  @override
  Widget build(BuildContext context) {
    if (cards.isEmpty) return const SizedBox.shrink();
    final now = cards[index % cards.length];
    return Row(
      children: [
        _HubArrow(
          icon: Icons.chevron_left,
          accent: now.accent,
          onTap: () => onStep(-1),
        ),
        const SizedBox(width: 2),
        Expanded(
          child: HubTile(
            name: now.name,
            count: now.count,
            icon: now.icon,
            accent: now.accent,
            // Sized to FILL the 152px band, so the tile has no dead margin over
            // and under its own contents.
            disc: 56,
            textScale: 1.35,
            // One tile alone in a column needs more paint than eight in a row.
            washScale: 1.4,
            onTap: () => ShellController.instance.go(now.section),
          ),
        ),
        const SizedBox(width: 2),
        _HubArrow(
          icon: Icons.chevron_right,
          accent: now.accent,
          onTap: () => onStep(1),
        ),
      ],
    );
  }
}

class _HubArrow extends StatelessWidget {
  const _HubArrow({
    required this.icon,
    required this.accent,
    required this.onTap,
  });

  final IconData icon;
  final Color accent;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => Hover(
        onTap: onTap,
        builder: (context, hov) => AnimatedContainer(
          duration: const Duration(milliseconds: 120),
          width: 24,
          height: 24,
          decoration: BoxDecoration(
            color: hov ? accent : accent.withValues(alpha: 0.16),
            shape: BoxShape.circle,
          ),
          child: Icon(icon, size: 13, color: hov ? Colors.white : accent),
        ),
      );
}
