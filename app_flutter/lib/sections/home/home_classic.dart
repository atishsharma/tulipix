// Classic — the bento dashboard, and the default landing page.
//
// Two regions: a fixed music RAIL down the right (the real player, its five
// source discs, and Quick Launch under it), and everything else stacked on the
// LEFT — the header, the Photos and Videos coverflow cards, the Books, Cloud
// and Tools row cards, and the Continue strip along the bottom.
//
// Nothing here scrolls. Every height comes out of the page budget, which is why
// the arithmetic at the top of `build` reads like ui/page_home.slint's: it is
// that arithmetic.

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

class ClassicHome extends StatelessWidget {
  const ClassicHome({
    super.key,
    required this.controller,
    required this.state,
  });

  final HomeController controller;
  final HomeState state;

  bool _on(String card) => state.cards.contains(card);

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, box) {
        // ── Geometry, from ui/page_home.slint ─────────────────────────────
        final railH = box.maxHeight - 32;
        const qaH = 250.0;
        // Scale the mini so its content fits the slot with the disc row clear
        // beneath it: the real content runs ~590px at scale 1, so reserve the
        // chrome and never upscale past 1.25.
        final playerScale =
            ((railH - qaH - 10 - 155) / 505).clamp(0.9, 1.25).toDouble();
        final musicW = (300 * playerScale + 40) * 1.05;
        // The 30px gutter holds the divider.
        final leftW = box.maxWidth - 32 - musicW - 30;
        final contH = _on('continue') ? 250.0 : 0.0;

        return Stack(
          children: [
            // ── Left region ───────────────────────────────────────────────
            Positioned(
              left: 16,
              top: 16,
              width: math.max(0, leftW),
              height: railH,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  SizedBox(height: 84, child: _Header(state: state)),
                  const SizedBox(height: 10),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        // Photos / Videos
                        if (_on('photos') || _on('videos'))
                          Expanded(child: _MediaRow(state: state, on: _on)),
                        if ((_on('photos') || _on('videos')) &&
                            (_on('books') || _on('cloud') || _on('tools')))
                          const SizedBox(height: 10),
                        // Books / Cloud / Tools
                        if (_on('books') || _on('cloud') || _on('tools'))
                          Expanded(child: _ShelfRow(state: state, on: _on)),
                        if (contH > 0) ...[
                          const SizedBox(height: 10),
                          SizedBox(
                            height: contH,
                            child: ContinueStrip(
                                controller: controller, state: state),
                          ),
                        ],
                      ],
                    ),
                  ),
                ],
              ),
            ),
            // ── The bento divider, in the gutter ──────────────────────────
            // Light-blue in light, light-pink in dark, with a soft glow.
            Positioned(
              left: box.maxWidth - musicW - 32,
              top: 16,
              width: 2,
              height: railH,
              child: Builder(
                builder: (context) {
                  final c = context.tokens.dark
                      ? const Color(0xFFF9A8D4)
                      : const Color(0xFF93C5FD);
                  return DecoratedBox(
                    decoration: BoxDecoration(
                      color: c,
                      borderRadius: BorderRadius.circular(1),
                      boxShadow: [
                        BoxShadow(
                            color: c.withValues(alpha: 0.4), blurRadius: 8),
                      ],
                    ),
                  );
                },
              ),
            ),
            // ── Right rail ────────────────────────────────────────────────
            Positioned(
              left: box.maxWidth - musicW - 16,
              top: 16,
              width: musicW,
              height: railH,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  if (_on('player'))
                    Expanded(
                      child: ClassicMusicCard(playerScale: playerScale),
                    )
                  else
                    const Spacer(),
                  if (_on('quick')) ...[
                    const SizedBox(height: 10),
                    const SizedBox(height: qaH, child: _QuickLaunch()),
                  ],
                ],
              ),
            ),
          ],
        );
      },
    );
  }
}

// ── Header ──────────────────────────────────────────────────────────────────

class _Header extends StatefulWidget {
  const _Header({required this.state});

  final HomeState state;

  @override
  State<_Header> createState() => _HeaderState();
}

class _HeaderState extends State<_Header> {
  /// The header spectrum is OFF by default — the mini's own visualizer is the
  /// always-on one.
  bool _viz = false;
  bool _lyrics = true;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.state;
    final c = MusicController.instance;
    return Row(
      children: [
        const AppMark(size: 84, radius: 20),
        const SizedBox(width: 16),
        Flexible(
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(st.greeting,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 30,
                      fontWeight: FontWeight.w800,
                      color: t.text)),
              const SizedBox(height: 5),
              Text(st.dateLine,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 14, color: t.textDim)),
            ],
          ),
        ),
        const SizedBox(width: 16),
        // The spectrum spans the header gap — borderless, so it reads as part
        // of the header rather than a boxed card.
        Expanded(
          child: AnimatedBuilder(
            animation: c,
            builder: (context, _) =>
                HeaderViz(controller: c, on: _viz, lyrics: _lyrics),
          ),
        ),
        const SizedBox(width: 16),
        // The waveform chip: five styles, Off, and the lyric toggle.
        AnimatedBuilder(
          animation: c,
          builder: (context, _) {
            final mode = c.now?.mode ?? 'idle';
            if (!c.tickPlaying || (mode != 'music' && mode != 'radio')) {
              return const SizedBox.shrink();
            }
            return _VizMenu(
              on: _viz,
              lyrics: _lyrics,
              lyricsAllowed: mode == 'music',
              style: c.visStyle,
              onStyle: (i) {
                setState(() => _viz = true);
                c.setVisStyle(i);
              },
              onOff: () => setState(() => _viz = false),
              onLyrics: () => setState(() => _lyrics = !_lyrics),
            );
          },
        ),
        const SizedBox(width: 12),
        const HomeAvatar(size: 56, dot: true),
      ],
    );
  }
}

class _VizMenu extends StatelessWidget {
  const _VizMenu({
    required this.on,
    required this.lyrics,
    required this.lyricsAllowed,
    required this.style,
    required this.onStyle,
    required this.onOff,
    required this.onLyrics,
  });

  final bool on;
  final bool lyrics;
  final bool lyricsAllowed;
  final int style;
  final ValueChanged<int> onStyle;
  final VoidCallback onOff;
  final VoidCallback onLyrics;

  static const List<String> _names = [
    'Bars',
    'Mirror',
    'Dots',
    'Levels',
    'Line'
  ];

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return PopupMenuButton<String>(
      tooltip: 'Visualizer',
      position: PopupMenuPosition.under,
      color: t.panel,
      onSelected: (v) {
        if (v == 'off') return onOff();
        if (v == 'lyrics') return onLyrics();
        onStyle(int.parse(v));
      },
      itemBuilder: (context) => [
        for (var i = 0; i < _names.length; i++)
          PopupMenuItem(
            value: '$i',
            height: 26,
            child: Text(_names[i],
                style: TextStyle(
                    fontSize: 12,
                    fontWeight:
                        on && style == i ? FontWeight.w700 : FontWeight.w500,
                    color: on && style == i ? Tokens.brand : t.text)),
          ),
        const PopupMenuDivider(),
        PopupMenuItem(
          value: 'off',
          height: 26,
          child: Text('Off Viz',
              style: TextStyle(
                  fontSize: 12,
                  fontWeight: on ? FontWeight.w500 : FontWeight.w700,
                  color: on ? t.text : Tokens.brand)),
        ),
        if (lyricsAllowed)
          PopupMenuItem(
            value: 'lyrics',
            height: 26,
            child: Text(lyrics ? 'Lyrics · on' : 'Lyrics · off',
                style: TextStyle(
                    fontSize: 12,
                    fontWeight: lyrics ? FontWeight.w700 : FontWeight.w500,
                    color: lyrics ? Tokens.brand : t.text)),
          ),
      ],
      child: Container(
        width: 30,
        height: 30,
        decoration: BoxDecoration(
          color: on ? Tokens.brand : t.panel2,
          shape: BoxShape.circle,
          border: Border.all(color: on ? Tokens.brand : t.outline),
        ),
        child:
            Icon(Icons.graphic_eq, size: 15, color: on ? Colors.white : t.text),
      ),
    );
  }
}

// ── Photos / Videos ─────────────────────────────────────────────────────────

class _MediaRow extends StatelessWidget {
  const _MediaRow({required this.state, required this.on});

  final HomeState state;
  final bool Function(String) on;

  @override
  Widget build(BuildContext context) {
    final c = state.counts;
    // Stretch: these cards are Stacks of positioned children, so the row's
    // height is the only height they have.
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (on('photos'))
          Expanded(
            child: PhotoSlideshow(
              icon: Icons.image_outlined,
              accent: Tokens.secPhotos,
              name: 'Photos',
              stat: '${c.photos} items · ${c.photosAlbums} albums',
              tiles: state.recentPhotos,
              section: Section.photos,
              emptyIcon: Icons.image_not_supported_outlined,
              topCrop: true,
              cycle: const Duration(seconds: 4),
            ),
          ),
        if (on('photos') && on('videos')) const SizedBox(width: 10),
        if (on('videos'))
          Expanded(
            child: PhotoSlideshow(
              icon: Icons.movie_outlined,
              accent: Tokens.secVideos,
              name: 'Videos',
              stat: '${c.videos} items · ${c.videosShows} shows',
              tiles: state.recentVideos,
              section: Section.videos,
              emptyIcon: Icons.videocam_off_outlined,
              showPlay: true,
              tileAspect: 1.6,
              // Offset from Photos so the two cards never step together.
              cycle: const Duration(seconds: 3),
            ),
          ),
      ],
    );
  }
}

/// A stacked fan of the ten most-recent items that holds each one ~2s then
/// eases one step to the next. `PhotoSlideshow` in ui/page_home.slint.
class PhotoSlideshow extends StatefulWidget {
  const PhotoSlideshow({
    super.key,
    required this.icon,
    required this.accent,
    required this.name,
    required this.stat,
    required this.tiles,
    required this.section,
    required this.emptyIcon,
    this.showPlay = false,
    this.tileAspect = 1.0,
    this.topCrop = false,
    this.cycle = const Duration(seconds: 4),
  });

  final IconData icon;
  final Color accent;
  final String name;
  final String stat;
  final List<HomeTile> tiles;
  final Section section;
  final IconData emptyIcon;

  /// Videos get a play badge on the centre card.
  final bool showPlay;

  /// 1 = square (photos), >1 landscape (video).
  final double tileAspect;
  final bool topCrop;
  final Duration cycle;

  @override
  State<PhotoSlideshow> createState() => _PhotoSlideshowState();
}

class _PhotoSlideshowState extends State<PhotoSlideshow>
    with SingleTickerProviderStateMixin {
  /// The head. The timer bumps it by one; the controller eases the slow ~1.4s
  /// wheel roll, so each item sits centred for the ~2.6s remainder.
  int _head = 0;
  Timer? _timer;
  late final AnimationController _roll = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 1400),
  );

  @override
  void initState() {
    super.initState();
    _arm();
  }

  @override
  void didUpdateWidget(PhotoSlideshow old) {
    super.didUpdateWidget(old);
    if (old.cycle != widget.cycle || old.tiles.length != widget.tiles.length) {
      _arm();
    }
  }

  void _arm() {
    _timer?.cancel();
    if (widget.tiles.length < 2) return;
    _timer = Timer.periodic(widget.cycle, (_) {
      if (!mounted) return;
      _roll.forward(from: 0).whenComplete(() {
        if (mounted) setState(() => _head += 1);
      });
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    _roll.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final n = widget.tiles.length;
    return Hover(
      onTap: () => ShellController.instance.go(widget.section),
      builder: (context, hov) => Container(
        decoration: BoxDecoration(
          color: plate(widget.accent),
          borderRadius: BorderRadius.circular(kCardRadius),
          border: Border.all(
              color: plateBorder(widget.accent, hov),
              width: plateBorderW(false)),
        ),
        clipBehavior: Clip.antiAlias,
        child: Stack(
          children: [
            // The fan strip, raised off the bottom edge so the sliding tiles
            // never sit underneath the title pill.
            Positioned(
              left: 0,
              top: 0,
              right: 0,
              bottom: 34,
              child: ClipRect(
                child: LayoutBuilder(
                  builder: (context, box) {
                    if (n == 0) {
                      return Center(
                        child: Icon(widget.emptyIcon,
                            size: math.min(box.maxHeight * 0.5, 96),
                            color: widget.accent.withValues(alpha: 0.55)),
                      );
                    }
                    return AnimatedBuilder(
                      animation: _roll,
                      builder: (context, _) {
                        final pos =
                            _head + Curves.easeInOut.transform(_roll.value);
                        return Stack(
                          clipBehavior: Clip.none,
                          children: [
                            // Declared far→near so the centre paints on top.
                            for (final k in const [3, -2, 2, -1, 1, 0])
                              _slot(k, pos, box),
                            if (widget.showPlay)
                              Positioned(
                                left: box.maxWidth / 2 - 28,
                                top: box.maxHeight / 2 - 28,
                                child: Container(
                                  width: 56,
                                  height: 56,
                                  decoration: BoxDecoration(
                                    color: const Color(0xB0000000),
                                    shape: BoxShape.circle,
                                    border: Border.all(
                                        color: const Color(0xE0FFFFFF),
                                        width: 2),
                                  ),
                                  child: const Icon(Icons.play_arrow,
                                      size: 24, color: Colors.white),
                                ),
                              ),
                          ],
                        );
                      },
                    );
                  },
                ),
              ),
            ),
            Positioned(
              left: 14,
              bottom: 14,
              child: TitlePill(
                icon: widget.icon,
                accent: widget.accent,
                name: widget.name,
                stat: widget.stat,
              ),
            ),
          ],
        ),
      ),
    );
  }

  /// One fan slot at fixed offset [k] from the centre. All the geometry derives
  /// from the shared float `pos`, which is what gives the heavy overlap and the
  /// stacked-deck look.
  Widget _slot(int k, double pos, BoxConstraints box) {
    final n = widget.tiles.length;
    final frac = pos - pos.floorToDouble();
    final rel = k - frac;
    // Sharp falloff (÷2.1) so only five cards ever carry opacity — the sixth
    // buffer card stays hidden until it becomes the incoming edge card.
    final prox = math.max(0.0, 1 - rel.abs() / 2.1);
    // Sub-linear horizontal offset, so the third layer hugs the second for a
    // stacked read while the fan still reaches both card edges.
    final off = rel * (1 - 0.05 * rel.abs());
    final step = box.maxWidth * 0.2;
    // Big tiles — all five near-full-height so they fill the card edge to edge.
    final h = box.maxHeight * (0.63 + 0.12 * prox);
    final w = h * widget.tileAspect;
    // `+ n` before the mod: k is negative for the left half of the fan, and at
    // pos 0 that would index backwards.
    final int idx = (pos.floor() + k + n) % math.max<int>(n, 1);
    final opacity = prox > 0 ? 0.35 + 0.65 * prox : 0.0;
    if (opacity <= 0) return const SizedBox.shrink();
    return Positioned(
      left: box.maxWidth / 2 + off * step - w / 2,
      top: (box.maxHeight - h) / 2,
      width: w,
      height: h,
      child: Opacity(
        opacity: opacity.clamp(0.0, 1.0),
        child: GestureDetector(
          // Only the centre tile opens the item; anywhere else on the card is
          // the section, which the Hover above already handles.
          onTap:
              k != 0 ? null : () => ShellController.instance.go(widget.section),
          child: Container(
            decoration: BoxDecoration(
              color: const Color(0xFF14161F),
              borderRadius: BorderRadius.circular(12),
              border: Border.all(color: const Color(0x5CFFFFFF), width: 1.5),
              boxShadow: const [
                BoxShadow(
                    color: Color(0x66000000),
                    blurRadius: 18,
                    offset: Offset(0, 6)),
              ],
            ),
            clipBehavior: Clip.antiAlias,
            child: LazyCover(
              section: widget.section,
              id: widget.tiles[idx].id,
              tint: widget.accent,
              icon: widget.icon,
              fit: BoxFit.cover,
            ),
          ),
        ),
      ),
    );
  }
}

// ── Books / Cloud / Tools ───────────────────────────────────────────────────

class _ShelfRow extends StatelessWidget {
  const _ShelfRow({required this.state, required this.on});

  final HomeState state;
  final bool Function(String) on;

  @override
  Widget build(BuildContext context) {
    final c = state.counts;
    final cards = <Widget>[
      if (on('books'))
        BookRow(
          total: c.books,
          tiles: state.recentBooks,
          stat: '${c.books} books'
              '${c.booksReading > 0 ? ' · ${c.booksReading} reading' : ''}',
        ),
      if (on('cloud')) CloudRow(remotes: state.remotes, total: c.cloudRemotes),
      if (on('tools')) ToolRow(total: state.toolCount),
    ];
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (var i = 0; i < cards.length; i++) ...[
          if (i > 0) const SizedBox(width: 10),
          Expanded(child: cards[i]),
        ],
      ],
    );
  }
}

/// The shell the three row cards share: full-bleed content below a title pill
/// overlaid top-left.
class RowCard extends StatelessWidget {
  const RowCard({
    super.key,
    required this.icon,
    required this.accent,
    required this.name,
    required this.onTap,
    required this.child,
    this.stat = '',
  });

  final IconData icon;
  final Color accent;
  final String name;
  final String stat;
  final VoidCallback onTap;
  final Widget child;

  @override
  Widget build(BuildContext context) => Hover(
        onTap: onTap,
        builder: (context, hov) => Container(
          decoration: BoxDecoration(
            color: plate(accent),
            borderRadius: BorderRadius.circular(kCardRadius),
            border: Border.all(
                color: plateBorder(accent, true), width: plateBorderW(true)),
          ),
          clipBehavior: Clip.antiAlias,
          child: Stack(
            children: [
              // Content sits below the single-line pill so tiles never underlap.
              Positioned(
                left: 16,
                top: 62,
                right: 16,
                bottom: 16,
                child: child,
              ),
              Positioned(
                left: 14,
                top: 14,
                child: TitlePill(
                    icon: icon, accent: accent, name: name, stat: stat),
              ),
            ],
          ),
        ),
      );
}

/// Books — three covers plus a "+N" box, as 9:16 portrait tiles sized so
/// exactly four fit the row. The hovered cover lifts, bookshelf-pull style.
class BookRow extends StatelessWidget {
  const BookRow({
    super.key,
    required this.total,
    required this.tiles,
    required this.stat,
  });

  final int total;
  final List<HomeTile> tiles;
  final String stat;

  @override
  Widget build(BuildContext context) {
    final shown = math.min(3, tiles.length);
    return RowCard(
      icon: Icons.menu_book_outlined,
      accent: Tokens.secBooks,
      name: 'Books',
      stat: stat,
      onTap: () => ShellController.instance.go(Section.books),
      child: LayoutBuilder(
        builder: (context, box) {
          if (tiles.isEmpty) {
            return Center(
              child: Icon(Icons.menu_book_outlined,
                  size: math.min(box.maxHeight * 0.6, 72),
                  color: Tokens.secBooks.withValues(alpha: 0.55)),
            );
          }
          // As tall as fits while four across still fit.
          final tileH =
              math.min(box.maxHeight, ((box.maxWidth - 24) / 4) * 16 / 9);
          final tileW = tileH * 9 / 16;
          final slots = shown + (total - shown > 0 ? 1 : 0);
          final gap =
              slots > 1 ? (box.maxWidth - slots * tileW) / (slots - 1) : 0.0;
          return Stack(
            children: [
              for (var i = 0; i < shown; i++)
                _tile(
                  x: i * (tileW + gap),
                  w: tileW,
                  h: tileH,
                  boxH: box.maxHeight,
                  child: LazyCover(
                    section: Section.books,
                    id: tiles[i].id,
                    tint: Tokens.secBooks,
                    icon: Icons.menu_book_outlined,
                  ),
                ),
              if (total - shown > 0)
                _tile(
                  x: shown * (tileW + gap),
                  w: tileW,
                  h: tileH,
                  boxH: box.maxHeight,
                  child: Center(
                    child: Text('+${total - shown}',
                        style: TextStyle(
                            fontSize: 15,
                            fontWeight: FontWeight.w700,
                            color: context.tokens.textDim)),
                  ),
                ),
            ],
          );
        },
      ),
    );
  }

  Widget _tile({
    required double x,
    required double w,
    required double h,
    required double boxH,
    required Widget child,
  }) =>
      _LiftTile(
        x: x,
        width: w,
        height: h,
        boxHeight: boxH,
        accent: Tokens.secBooks,
        onTap: () => ShellController.instance.go(Section.books),
        child: child,
      );
}

/// A tile that lifts 12px under the cursor. The pull is what makes a row of
/// covers read as a shelf rather than as four buttons.
class _LiftTile extends StatelessWidget {
  const _LiftTile({
    required this.x,
    required this.width,
    required this.height,
    required this.boxHeight,
    required this.accent,
    required this.onTap,
    required this.child,
  });

  final double x;
  final double width;
  final double height;
  final double boxHeight;
  final Color accent;
  final VoidCallback onTap;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // The slot keeps 12px of headroom above the tile's rest position and the
    // padding animates away, so the lift is a paint change rather than a
    // reflow of everything beside it.
    return Positioned(
      left: x,
      top: (boxHeight - height) / 2 - 12,
      width: width,
      height: height + 12,
      child: Hover(
        onTap: onTap,
        builder: (context, hov) => AnimatedPadding(
          duration: const Duration(milliseconds: 160),
          curve: Curves.easeOut,
          padding: EdgeInsets.only(top: hov ? 0 : 12),
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 130),
            decoration: BoxDecoration(
              color: hov ? t.panel : t.panel2,
              borderRadius: BorderRadius.circular(10),
              border: Border.all(
                  color: hov ? accent : t.outline.withValues(alpha: 0.9),
                  width: hov ? 2 : 1.5),
              boxShadow: hov
                  ? [
                      BoxShadow(
                          color: t.dark
                              ? const Color(0x99000000)
                              : const Color(0x33000000),
                          blurRadius: 16,
                          offset: const Offset(0, 5)),
                    ]
                  : null,
            ),
            clipBehavior: Clip.antiAlias,
            child: child,
          ),
        ),
      ),
    );
  }
}

/// Cloud — one tile per remote: the provider's own glyph, then the name, the
/// backend and the usage, each on its own coloured pill.
class CloudRow extends StatelessWidget {
  const CloudRow({super.key, required this.remotes, required this.total});

  final List<HomeRemote> remotes;
  final int total;

  /// The provider colours ui/page_home.slint's `CloudGlyph` tints by.
  static Color glyphColour(String backend) => switch (backend.toLowerCase()) {
        'onedrive' => const Color(0xFF2B7CD3),
        'drive' ||
        'googlecloudstorage' ||
        'google cloud storage' =>
          const Color(0xFF1A9F57),
        'dropbox' => const Color(0xFF0061FF),
        'box' => const Color(0xFF0061D5),
        's3' || 'amazon s3' => const Color(0xFFE77600),
        'mega' => const Color(0xFFD9272E),
        'pcloud' => const Color(0xFF21A0ED),
        'yandex' => const Color(0xFFFF3333),
        'webdav' || 'sftp' || 'ftp' => kPodcast,
        _ => Tokens.secCloud,
      };

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final shown = remotes.length;
    return RowCard(
      icon: Icons.cloud_outlined,
      accent: Tokens.secCloud,
      name: 'Cloud',
      onTap: () => ShellController.instance.go(Section.cloud),
      child: LayoutBuilder(
        builder: (context, box) {
          if (remotes.isEmpty) {
            return Center(
              child: Icon(Icons.cloud_off_outlined,
                  size: math.min(box.maxHeight * 0.6, 72),
                  color: Tokens.secCloud.withValues(alpha: 0.55)),
            );
          }
          Widget pill(String text, Color colour, double size) => Container(
                height: size + 7,
                padding: const EdgeInsets.symmetric(horizontal: 8),
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: colour.withValues(alpha: t.dark ? 0.30 : 0.18),
                  borderRadius: BorderRadius.circular((size + 7) / 2),
                ),
                child: Text(text,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: size,
                        fontWeight: FontWeight.w800,
                        color: colour)),
              );
          return Row(
            children: [
              for (final r in remotes) ...[
                Expanded(
                  child: Hover(
                    onTap: () => ShellController.instance.go(Section.cloud),
                    builder: (context, hov) => AnimatedContainer(
                      duration: const Duration(milliseconds: 130),
                      padding: const EdgeInsets.all(8),
                      decoration: BoxDecoration(
                        color: hov ? t.panel : t.panel2,
                        borderRadius: BorderRadius.circular(10),
                        border: Border.all(
                            color: hov
                                ? Tokens.secCloud
                                : t.outline.withValues(alpha: 0.9),
                            width: hov ? 2 : 1.5),
                      ),
                      child: FittedBox(
                        fit: BoxFit.scaleDown,
                        child: Column(
                          mainAxisAlignment: MainAxisAlignment.center,
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            Icon(Icons.cloud,
                                size: 46, color: glyphColour(r.backend)),
                            const SizedBox(height: 5),
                            pill(r.name, Tokens.secCloud, 11),
                            const SizedBox(height: 5),
                            pill(r.backend, kPodcast, 9),
                            if (r.usage.isNotEmpty) ...[
                              const SizedBox(height: 5),
                              pill(r.usage, Tokens.secCloud, 9),
                            ],
                          ],
                        ),
                      ),
                    ),
                  ),
                ),
                const SizedBox(width: 8),
              ],
              if (total - shown > 0)
                Expanded(
                  child: Hover(
                    onTap: () => ShellController.instance.go(Section.cloud),
                    builder: (context, hov) => Container(
                      alignment: Alignment.center,
                      decoration: BoxDecoration(
                        color: hov ? t.panel : t.panel2,
                        borderRadius: BorderRadius.circular(10),
                        border: Border.all(
                            color: hov
                                ? Tokens.secCloud
                                : t.outline.withValues(alpha: 0.9),
                            width: hov ? 2 : 1.5),
                      ),
                      child: Text('+${total - shown}',
                          style: TextStyle(
                              fontSize: 15,
                              fontWeight: FontWeight.w700,
                              color: t.textDim)),
                    ),
                  ),
                ),
            ],
          );
        },
      ),
    );
  }
}

/// One representative tool per category, laid out 3×2, plus a "+N" key for
/// everything else in the catalog.
class ToolRow extends StatelessWidget {
  const ToolRow({super.key, required this.total});

  final int total;

  static const List<({IconData icon, String label})> _tools = [
    (icon: Icons.folder_outlined, label: 'File ops'),
    (icon: Icons.movie_outlined, label: 'Video'),
    (icon: Icons.music_note_outlined, label: 'Audio'),
    (icon: Icons.image_outlined, label: 'Photo'),
    (icon: Icons.chat_bubble_outline, label: 'Subtitles'),
  ];

  @override
  Widget build(BuildContext context) => RowCard(
        icon: Icons.build_outlined,
        accent: Tokens.secTools,
        name: 'Tools',
        onTap: () => ShellController.instance.go(Section.tools),
        child: ActionGrid(
          columns: 3,
          children: [
            for (final t in _tools)
              CircleAction(
                icon: t.icon,
                label: t.label,
                tint: Tokens.secTools,
                disc: 70,
                mono: true,
                onTap: () => ShellController.instance.go(Section.tools),
              ),
            CircleAction(
              icon: Icons.add,
              label: '+${math.max(0, total - 5)} more',
              tint: Tokens.secTools,
              disc: 70,
              mono: true,
              onTap: () => ShellController.instance.go(Section.tools),
            ),
          ],
        ),
      );
}

/// A tinted icon disc with a pill label under it. `mono` is the Tools style:
/// white fill in light, a faint tint wash in dark, a hard outline in both.
class CircleAction extends StatelessWidget {
  const CircleAction({
    super.key,
    required this.icon,
    required this.label,
    required this.tint,
    required this.onTap,
    this.disc = 54,
    this.mono = false,
  });

  final IconData icon;
  final String label;
  final Color tint;
  final VoidCallback onTap;
  final double disc;
  final bool mono;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final monoFill = t.dark ? tint.withValues(alpha: 0.15) : Colors.white;
    final monoOutline = t.dark ? Colors.white : Colors.black;
    final monoIcon = t.dark ? Colors.white : tint;
    return Hover(
      onTap: onTap,
      builder: (context, hov) => Column(
        mainAxisAlignment: MainAxisAlignment.center,
        mainAxisSize: MainAxisSize.min,
        children: [
          AnimatedContainer(
            duration: const Duration(milliseconds: 120),
            curve: Curves.easeOut,
            width: disc,
            height: disc,
            decoration: BoxDecoration(
              color:
                  hov ? tint : (mono && !t.dark ? monoFill : wash(tint, 0.15)),
              shape: BoxShape.circle,
              border: Border.all(
                  color: mono ? monoOutline : tint.withValues(alpha: 0.40)),
            ),
            child: Icon(icon,
                size: disc * 0.44,
                color: hov ? Colors.white : (mono ? monoIcon : tint)),
          ),
          const SizedBox(height: 7),
          Container(
            height: 22,
            padding: const EdgeInsets.symmetric(horizontal: 11),
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color:
                  mono && !t.dark ? monoFill : wash(tint, mono ? 0.15 : 0.12),
              borderRadius: BorderRadius.circular(11),
              border: Border.all(
                  color: mono ? monoOutline : tint.withValues(alpha: 0.30)),
            ),
            child: Text(label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 10, fontWeight: FontWeight.w700, color: t.text)),
          ),
        ],
      ),
    );
  }
}

// ── Continue ────────────────────────────────────────────────────────────────

/// Four fixed pill slots, filled left→right with only as many items as exist
/// and never stretched — Rust caps the model at four.
class ContinueStrip extends StatelessWidget {
  const ContinueStrip({
    super.key,
    required this.controller,
    required this.state,
  });

  final HomeController controller;
  final HomeState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = state;
    return Container(
      decoration: BoxDecoration(
        color: plateQuiet(Tokens.secMusic),
        borderRadius: BorderRadius.circular(kCardRadius),
        border: Border.all(
            color: plateBorder(Tokens.secMusic, false),
            width: plateBorderW(false)),
      ),
      padding: const EdgeInsets.all(14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const TitlePill(
                icon: Icons.rotate_left,
                accent: Tokens.secMusic,
                name: 'Continue',
              ),
              const Spacer(),
              ContinueTabs(
                active: st.continueFilter,
                onPick: (id) =>
                    controller.send(HomeCmd.setContinueFilter(filter: id)),
              ),
            ],
          ),
          const SizedBox(height: 10),
          Expanded(
            child: st.continueRows.isEmpty
                ? Align(
                    alignment: Alignment.centerLeft,
                    child: Text(
                        'Nothing in progress — resume points from books, '
                        'podcasts and audiobooks show up here.',
                        style: TextStyle(fontSize: 10, color: t.textDim)),
                  )
                // FOUR fixed slots, filled left→right and never stretched —
                // Rust caps the model at four. Measured here rather than handed
                // in: the caller's width did not know about this card's own
                // padding and hairline, and the row came out two pixels wide.
                : LayoutBuilder(
                    builder: (context, box) {
                      final slot = math.max(0.0, (box.maxWidth - 3 * 14) / 4);
                      // Full-height 3:4 portrait.
                      final thumb = box.maxHeight * 3 / 5;
                      return Row(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          for (var i = 0; i < st.continueRows.length; i++) ...[
                            if (i > 0) const SizedBox(width: 14),
                            SizedBox(
                              width: slot,
                              child: ContinueCard(
                                controller: controller,
                                row: st.continueRows[i],
                                thumbWidth: thumb,
                              ),
                            ),
                          ],
                        ],
                      );
                    },
                  ),
          ),
        ],
      ),
    );
  }
}

/// One resume card: the cover, the title over the author, and the progress pill
/// pinned at the bottom. The kind tag and the ⋮ ride the top-right corner.
class ContinueCard extends StatelessWidget {
  const ContinueCard({
    super.key,
    required this.controller,
    required this.row,
    required this.thumbWidth,
  });

  final HomeController controller;
  final HomeContinue row;
  final double thumbWidth;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final accent = kindColor(row.kind);
    return Hover(
      onTap: () => ShellController.instance.go(kindSection(row.kind)),
      builder: (context, hov) => AnimatedContainer(
        duration: const Duration(milliseconds: 120),
        decoration: BoxDecoration(
          color: hov ? t.panel : t.panel.withValues(alpha: 0.6),
          borderRadius: BorderRadius.circular(10),
          border: Border.all(
              color: hov ? accent.withValues(alpha: 0.6) : t.outline),
        ),
        child: Stack(
          // Expand, or the row inside takes the height of its own shortest
          // child and the progress pill is squeezed off the bottom.
          fit: StackFit.expand,
          children: [
            Padding(
              padding: const EdgeInsets.all(10),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  // The accent ring sits 1px off the thumb: outer rect draws
                  // the border, inner rect holds the clipped image.
                  Container(
                    width: thumbWidth,
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(9),
                      border: Border.all(color: accent),
                    ),
                    padding: const EdgeInsets.all(2),
                    child: ClipRRect(
                      borderRadius: BorderRadius.circular(7),
                      child: LazyCover(
                        section: kindSection(row.kind),
                        id: row.id,
                        tint: accent,
                        icon: kindIcon(row.kind),
                        iconSize: 22,
                      ),
                    ),
                  ),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Padding(
                      // Keep the title clear of the ⋮.
                      padding: const EdgeInsets.only(right: 12),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          // Drops the title below the tag and the ⋮ so long
                          // first lines never collide with them.
                          const SizedBox(height: 14),
                          // Slint gives the title a max-height of 72 and the
                          // author 31, then lets the pill sit on the floor.
                          // `maxLines` alone caps the lines, not the box, so
                          // the two share what the pill leaves in that ratio.
                          Expanded(
                            child: Column(
                              crossAxisAlignment: CrossAxisAlignment.stretch,
                              children: [
                                Flexible(
                                  flex: 72,
                                  child: Text(row.title,
                                      maxLines: 4,
                                      overflow: TextOverflow.ellipsis,
                                      style: TextStyle(
                                          fontSize: 14,
                                          fontWeight: FontWeight.w800,
                                          color: t.text)),
                                ),
                                if (row.author.isNotEmpty) ...[
                                  const SizedBox(height: 3),
                                  Flexible(
                                    flex: 31,
                                    child: Text(row.author,
                                        maxLines: 2,
                                        overflow: TextOverflow.ellipsis,
                                        style: TextStyle(
                                            fontSize: 12, color: t.textDim)),
                                  ),
                                ],
                              ],
                            ),
                          ),
                          ProgressPill(
                            sub: row.sub,
                            frac: row.frac,
                            accent: accent,
                            compact: true,
                          ),
                        ],
                      ),
                    ),
                  ),
                ],
              ),
            ),
            // The kind tag, left of the ⋮ — every pill says where it comes from.
            Positioned(
              right: 28,
              top: 4,
              child: KindTag(kind: row.kind, accent: accent),
            ),
            Positioned(
              right: 2,
              top: 0,
              child: _RemoveMenu(controller: controller, row: row),
            ),
          ],
        ),
      ),
    );
  }
}

class _RemoveMenu extends StatelessWidget {
  const _RemoveMenu({required this.controller, required this.row});

  final HomeController controller;
  final HomeContinue row;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return PopupMenuButton<void>(
      tooltip: 'More',
      padding: EdgeInsets.zero,
      iconSize: 13,
      color: t.panel,
      position: PopupMenuPosition.under,
      itemBuilder: (context) => [
        PopupMenuItem(
          height: 32,
          onTap: () => _confirm(context),
          child: const Row(
            children: [
              Icon(Icons.delete_outline, size: 13, color: Tokens.error),
              SizedBox(width: 7),
              Text('Remove',
                  style: TextStyle(
                      fontSize: 11,
                      fontWeight: FontWeight.w700,
                      color: Tokens.error)),
            ],
          ),
        ),
      ],
      child: SizedBox(
        width: 20,
        height: 20,
        child: Icon(Icons.more_vert, size: 13, color: t.textDim),
      ),
    );
  }

  /// Enter removes, Escape cancels — the same two keys the Slint dialog binds.
  void _confirm(BuildContext context) {
    final t = context.tokens;
    showDialog<void>(
      context: context,
      builder: (context) => AlertDialog(
        backgroundColor: t.panel,
        title: const Text('Remove from Continue?',
            style: TextStyle(fontSize: 14, fontWeight: FontWeight.w800)),
        content:
            Text(row.title, style: TextStyle(fontSize: 11, color: t.textDim)),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('Cancel'),
          ),
          FilledButton(
            autofocus: true,
            style: FilledButton.styleFrom(backgroundColor: Tokens.error),
            onPressed: () {
              controller.send(HomeCmd.dismissContinue(
                  kind: row.kind, id: row.id, path: row.path));
              Navigator.of(context).pop();
            },
            child: const Text('Remove'),
          ),
        ],
      ),
    );
  }
}

// ── Quick Launch ────────────────────────────────────────────────────────────

class _QuickLaunch extends StatelessWidget {
  const _QuickLaunch();

  @override
  Widget build(BuildContext context) => Container(
        decoration: BoxDecoration(
          // The same pink splash as the player card above it.
          color: plateQuiet(Tokens.secMusic),
          borderRadius: BorderRadius.circular(kCardRadius),
          border: Border.all(
              color: plateBorder(Tokens.secVideos, false),
              width: plateBorderW(false)),
        ),
        padding: const EdgeInsets.all(14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const TitlePill(
              icon: Icons.bolt,
              accent: Tokens.secVideos,
              name: 'Quick Launch',
            ),
            const SizedBox(height: 10),
            Expanded(
              child: ActionGrid(
                columns: 2,
                children: [
                  CircleAction(
                    icon: Icons.movie_outlined,
                    label: 'Stream',
                    tint: Tokens.secVideos,
                    onTap: () => launch('stream'),
                  ),
                  CircleAction(
                    icon: Icons.tv_outlined,
                    label: 'Live TV',
                    tint: kLiveTv,
                    onTap: () => launch('livetv'),
                  ),
                  CircleAction(
                    icon: Icons.download_outlined,
                    label: 'Music Downloader',
                    tint: Tokens.secMusic,
                    onTap: () => launch('download'),
                  ),
                  CircleAction(
                    icon: Icons.radio_outlined,
                    label: 'Random Radio',
                    tint: kRadio,
                    onTap: () => launch('radio'),
                  ),
                ],
              ),
            ),
          ],
        ),
      );
}

/// A fixed grid of [CircleAction]s that divides the height it is given.
///
/// `GridLayout` in Slint distributes the band between its rows; `GridView`
/// derives a row height from an aspect ratio and scrolls the rest away, which
/// on a short window silently cut the bottom row off. This divides, and scales
/// a cell's contents down when even the divided row is shorter than a disc and
/// its label.
class ActionGrid extends StatelessWidget {
  const ActionGrid({
    super.key,
    required this.columns,
    required this.children,
    this.spacing = 10,
  });

  final int columns;
  final List<Widget> children;
  final double spacing;

  @override
  Widget build(BuildContext context) {
    final rows = (children.length / columns).ceil();
    return Column(
      children: [
        for (var r = 0; r < rows; r++) ...[
          if (r > 0) SizedBox(height: spacing),
          Expanded(
            child: Row(
              children: [
                for (var c = 0; c < columns; c++) ...[
                  if (c > 0) SizedBox(width: spacing),
                  Expanded(
                    child: r * columns + c < children.length
                        ? FittedBox(
                            fit: BoxFit.scaleDown,
                            child: children[r * columns + c])
                        : const SizedBox.shrink(),
                  ),
                ],
              ],
            ),
          ),
        ],
      ],
    );
  }
}
