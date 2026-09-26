// Home · Media — for the Media preset (Photos, Videos, Music, Books).
//
// docs/mockups/NewSections/home-media-play-deck.html. What you were watching,
// what is playing and the book you are in, all on the first screen; then one
// row of what is next and a band per section. Blocks follow the card switches
// in Settings › You & Home, and a section that is off has no door or band.

import 'package:flutter/material.dart';

import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/home.dart';
import 'home_controller.dart';
import 'home_modern.dart';
import 'home_shared.dart';

class MediaHome extends StatelessWidget {
  const MediaHome({super.key, required this.controller, required this.state});

  final HomeController controller;
  final HomeState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = state;
    final name = firstName(st);
    final rows = cardOn(st, 'continue') ? st.continueRows : <HomeContinue>[];
    final reading = rows.where((r) => r.kind == 'book').firstOrNull;
    final upNext = rows.skip(st.hero.kind.isEmpty ? 0 : 1).toList();
    final photos = sectionOn(Section.photos) && cardOn(st, 'photos');
    final music = sectionOn(Section.music) && cardOn(st, 'music');
    final books = sectionOn(Section.books) && cardOn(st, 'books');
    final videos = sectionOn(Section.videos) && cardOn(st, 'videos');

    return LayoutBuilder(builder: (context, box) {
      final wide = box.maxWidth >= 1000;
      final pad = box.maxWidth >= 700 ? 30.0 : 18.0;
      final bands = [
        if (photos && st.recentPhotos.isNotEmpty) _PhotoBand(st: st),
        if (music && st.recentSongs.isNotEmpty) _SongBand(st: st),
        if (books && st.recentBooks.isNotEmpty) _BookBand(st: st),
      ];
      return SingleChildScrollView(
        padding: EdgeInsets.fromLTRB(pad, 26, pad, 36),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (cardOn(st, 'hero')) ...[
              Text(st.dateLine.toUpperCase(),
                  style: TextStyle(
                      fontSize: 10.5,
                      fontWeight: FontWeight.w700,
                      letterSpacing: 1.3,
                      color: t.textDim)),
              const SizedBox(height: 2),
              Text(
                  name.isEmpty
                      ? 'Good ${dayPartName(dayPart()).toLowerCase()}'
                      : st.greeting,
                  style: TextStyle(
                      fontSize: 24,
                      fontWeight: FontWeight.w800,
                      letterSpacing: -0.4,
                      color: t.text)),
              const SizedBox(height: 18),
            ],
            _Doors(st: st, wide: wide),
            const SizedBox(height: 18),
            _heroRow(context, wide, reading),
            if (upNext.isNotEmpty) ...[
              const SizedBox(height: 26),
              const ModernHead('Up next', note: 'across your library'),
              ModernRail(
                height: 196,
                children: [for (final r in upNext) _UpNextCard(row: r)],
              ),
            ],
            if (bands.isNotEmpty) ...[
              const SizedBox(height: 26),
              wide
                  ? IntrinsicHeight(
                      child: Row(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          for (var i = 0; i < bands.length; i++) ...[
                            if (i > 0) const SizedBox(width: 14),
                            Expanded(flex: i == 0 ? 13 : 10, child: bands[i]),
                          ],
                        ],
                      ),
                    )
                  : Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        for (var i = 0; i < bands.length; i++) ...[
                          if (i > 0) const SizedBox(height: 14),
                          bands[i],
                        ],
                      ],
                    ),
            ],
            if (videos && st.recentVideos.isNotEmpty) ...[
              const SizedBox(height: 26),
              ModernHead('New in Videos',
                  link: 'Videos',
                  onLink: () => ShellController.instance.go(Section.videos)),
              ModernRail(
                height: 238,
                children: [
                  for (final v in st.recentVideos) _Poster(tile: v),
                ],
              ),
            ],
          ],
        ),
      );
    });
  }

  Widget _heroRow(BuildContext context, bool wide, HomeContinue? reading) {
    final st = state;
    final film = _FilmCard(st: st);
    final side = <Widget>[
      if (cardOn(st, 'player') && sectionOn(Section.music))
        const PanelCard(child: NowPlayingCard()),
      if (reading != null) _ReadingCard(row: reading),
    ];
    if (!wide || side.isEmpty) {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          film,
          for (final s in side) ...[const SizedBox(height: 14), s],
        ],
      );
    }
    return IntrinsicHeight(
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(flex: 7, child: film),
          const SizedBox(width: 14),
          Expanded(
            flex: 4,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (var i = 0; i < side.length; i++) ...[
                  if (i > 0) const SizedBox(height: 14),
                  side[i],
                ],
              ],
            ),
          ),
        ],
      ),
    );
  }
}

// ── the four doors ──────────────────────────────────────────────────────────

class _Doors extends StatelessWidget {
  const _Doors({required this.st, required this.wide});

  final HomeState st;
  final bool wide;

  @override
  Widget build(BuildContext context) {
    final c = st.counts;
    final doors = [
      if (sectionOn(Section.videos))
        (
          s: Section.videos,
          label: 'Watch',
          icon: Icons.movie_outlined,
          line: c.videosShows > 0
              ? '${plural(c.videos, 'video')} · ${plural(c.videosShows, 'show')}'
              : plural(c.videos, 'video'),
        ),
      if (sectionOn(Section.music))
        (
          s: Section.music,
          label: 'Listen',
          icon: Icons.music_note_outlined,
          line: plural(c.songs, 'song'),
        ),
      if (sectionOn(Section.books))
        (
          s: Section.books,
          label: 'Read',
          icon: Icons.menu_book_outlined,
          line: plural(c.books, 'book'),
        ),
      if (sectionOn(Section.photos))
        (
          s: Section.photos,
          label: 'Look',
          icon: Icons.image_outlined,
          line: plural(c.photos, 'photo'),
        ),
    ];
    if (doors.isEmpty) return const SizedBox.shrink();
    final cols = wide ? doors.length : 2;
    final t = context.tokens;
    return LayoutBuilder(builder: (context, box) {
      final w = (box.maxWidth - 10 * (cols - 1)) / cols;
      return Wrap(
        spacing: 10,
        runSpacing: 10,
        children: [
          for (final d in doors)
            SizedBox(
              width: w,
              child: PanelCard(
                padding: 12,
                onTap: () => ShellController.instance.go(d.s),
                child: Row(
                  children: [
                    Container(
                      width: 36,
                      height: 36,
                      decoration: BoxDecoration(
                        color: _accent(d.s).withValues(alpha: 0.18),
                        borderRadius: BorderRadius.circular(11),
                      ),
                      child: Icon(d.icon, size: 18, color: _accent(d.s)),
                    ),
                    const SizedBox(width: 12),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(d.label,
                              style: TextStyle(
                                  fontSize: 13.5,
                                  fontWeight: FontWeight.w700,
                                  color: t.text)),
                          Text(d.line,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style:
                                  TextStyle(fontSize: 11.5, color: t.textDim)),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
            ),
        ],
      );
    });
  }
}

Color _accent(Section s) => Tokens.accentOf(s);

// ── the film ────────────────────────────────────────────────────────────────

class _FilmCard extends StatelessWidget {
  const _FilmCard({required this.st});

  final HomeState st;

  @override
  Widget build(BuildContext context) {
    final h = st.hero;
    final none = h.kind.isEmpty || !cardOn(st, 'continue');
    // Nothing half-watched: the newest film stands in, so the first screen is
    // never an empty frame.
    final fresh = none ? st.recentVideos.firstOrNull : null;
    final kind = none ? 'video' : h.kind;
    final id = none ? (fresh?.id ?? -1) : h.id;
    final kicker = none
        ? (fresh == null ? 'NOTHING IN PROGRESS' : 'NEW IN VIDEOS')
        : h.kicker;
    final title = none ? (fresh?.label ?? 'Pick something to watch') : h.title;
    final meta = none ? (fresh?.sub ?? '') : h.meta;
    return Tap(
      onTap: none
          ? () => fresh == null
              ? ShellController.instance.go(Section.videos)
              : openVideo(fresh.id)
          : () => resumeHero(h),
      child: ClipRRect(
        borderRadius: BorderRadius.circular(20),
        child: ConstrainedBox(
          constraints: const BoxConstraints(minHeight: 330),
          child: Stack(
            children: [
              Positioned.fill(
                child: LazyCover(
                  section: kindSection(kind),
                  id: id,
                  art: none ? null : heroArt(h),
                  tint: kindColor(kind),
                  icon: kindIcon(kind),
                  iconSize: 48,
                ),
              ),
              const Positioned.fill(
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    gradient: LinearGradient(
                      begin: Alignment.bottomCenter,
                      end: Alignment.topCenter,
                      colors: [
                        Color(0xEB050614),
                        Color(0x59050614),
                        Color(0x00050614),
                      ],
                      stops: [0, 0.55, 1],
                    ),
                  ),
                ),
              ),
              Padding(
                padding: const EdgeInsets.all(24),
                child: ConstrainedBox(
                  constraints: const BoxConstraints(minHeight: 282),
                  child: Column(
                    mainAxisAlignment: MainAxisAlignment.end,
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(kicker,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                              fontSize: 10.5,
                              fontWeight: FontWeight.w700,
                              letterSpacing: 1.3,
                              color: Color(0xB3FFFFFF))),
                      const SizedBox(height: 6),
                      Text(title,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                              fontSize: 32,
                              height: 1.08,
                              fontWeight: FontWeight.w800,
                              letterSpacing: -0.6,
                              color: Colors.white)),
                      if (meta.isNotEmpty) ...[
                        const SizedBox(height: 6),
                        Text(meta,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: const TextStyle(
                                fontSize: 12.5, color: Color(0xBFFFFFFF))),
                      ],
                      if (!none && h.frac >= 0) ...[
                        const SizedBox(height: 14),
                        SizedBox(
                          width: 360,
                          child: ThinBar(
                            frac: h.frac,
                            color: Colors.white,
                            track: const Color(0x38FFFFFF),
                          ),
                        ),
                      ],
                      const SizedBox(height: 16),
                      Wrap(
                        spacing: 10,
                        runSpacing: 10,
                        children: [
                          PillBtn(
                            label: none ? 'Open' : 'Resume',
                            icon: Icons.play_arrow_rounded,
                            onTap: none
                                ? () => fresh == null
                                    ? ShellController.instance
                                        .go(Section.videos)
                                    : openVideo(fresh.id)
                                : () => resumeHero(h),
                          ),
                          PillBtn(
                            label: 'Open ${_sectionName(kindSection(kind))}',
                            solid: false,
                            onTap: () =>
                                ShellController.instance.go(kindSection(kind)),
                          ),
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
    );
  }
}

String _sectionName(Section s) => switch (s) {
      Section.videos => 'Videos',
      Section.books => 'Books',
      Section.photos => 'Photos',
      _ => 'Music',
    };

// ── the book ────────────────────────────────────────────────────────────────

class _ReadingCard extends StatelessWidget {
  const _ReadingCard({required this.row});

  final HomeContinue row;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return PanelCard(
      onTap: () => resumeRow(row),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text('READING',
              style: TextStyle(
                  fontSize: 10.5,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 1.3,
                  color: Tokens.secBooks)),
          const SizedBox(height: 10),
          Row(
            children: [
              ClipRRect(
                borderRadius: BorderRadius.circular(8),
                child:
                    SizedBox(width: 62, height: 90, child: RowCover(row: row)),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(row.title,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 14.5,
                            fontWeight: FontWeight.w700,
                            color: t.text)),
                    Text(
                        [row.author, row.sub]
                            .where((s) => s.isNotEmpty)
                            .join(' · '),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 12, color: t.textDim)),
                    const SizedBox(height: 10),
                    ThinBar(frac: row.frac, color: Tokens.secBooks),
                    if (row.frac >= 0) ...[
                      const SizedBox(height: 6),
                      Text('${(row.frac * 100).round()}%',
                          style: TextStyle(fontSize: 11.5, color: t.textDim)),
                    ],
                  ],
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

// ── up next ─────────────────────────────────────────────────────────────────

class _UpNextCard extends StatelessWidget {
  const _UpNextCard({required this.row});

  final HomeContinue row;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = kindColor(row.kind);
    return SizedBox(
      width: 250,
      child: Tap(
        onTap: () => resumeRow(row),
        child: Container(
          clipBehavior: Clip.antiAlias,
          decoration: context.skin.surface(SurfaceRole.card, radius: 16) ??
              BoxDecoration(
                color: t.panel,
                borderRadius: BorderRadius.circular(16),
                border: Border.all(color: t.outline),
              ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              SizedBox(height: 120, child: RowCover(row: row)),
              Padding(
                padding: const EdgeInsets.fromLTRB(12, 10, 12, 12),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    KindTag(kind: row.kind, accent: c),
                    const SizedBox(height: 6),
                    Text(row.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 13.5,
                            fontWeight: FontWeight.w700,
                            color: t.text)),
                    const SizedBox(height: 8),
                    row.frac >= 0
                        ? ThinBar(frac: row.frac, color: c)
                        : Text(row.sub,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 11.5, color: t.textDim)),
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

// ── the bands ───────────────────────────────────────────────────────────────

class _PhotoBand extends StatelessWidget {
  const _PhotoBand({required this.st});

  final HomeState st;

  @override
  Widget build(BuildContext context) {
    final p = st.recentPhotos.take(5).toList();
    Widget tile(int i) => i < p.length
        ? ClipRRect(
            borderRadius: BorderRadius.circular(12),
            child: Tap(
              onTap: () => ShellController.instance.go(Section.photos),
              child: LazyCover(
                section: Section.photos,
                id: p[i].id,
                tint: Tokens.secPhotos,
                alignment: Alignment.topCenter,
              ),
            ),
          )
        : const SizedBox.shrink();
    return PanelCard(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          ModernHead('New photos',
              link: 'Photos',
              onLink: () => ShellController.instance.go(Section.photos)),
          SizedBox(
            height: 200,
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Expanded(flex: 2, child: tile(0)),
                const SizedBox(width: 8),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      Expanded(child: tile(1)),
                      const SizedBox(height: 8),
                      Expanded(child: tile(3)),
                    ],
                  ),
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      Expanded(child: tile(2)),
                      const SizedBox(height: 8),
                      Expanded(child: tile(4)),
                    ],
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

class _SongBand extends StatelessWidget {
  const _SongBand({required this.st});

  final HomeState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = st.recentSongs.take(6).toList();
    return PanelCard(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          ModernHead('Recent songs',
              link: 'Music',
              onLink: () => ShellController.instance.go(Section.music)),
          for (var r = 0; r < s.length; r += 3) ...[
            if (r > 0) const SizedBox(height: 10),
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                for (var i = r; i < r + 3; i++) ...[
                  if (i > r) const SizedBox(width: 10),
                  Expanded(
                    child: i >= s.length
                        ? const SizedBox.shrink()
                        : Column(
                            crossAxisAlignment: CrossAxisAlignment.stretch,
                            children: [
                              AspectRatio(
                                aspectRatio: 1,
                                child: ClipRRect(
                                  borderRadius: BorderRadius.circular(12),
                                  child: LazyCover(
                                    section: Section.music,
                                    id: s[i].id,
                                    tint: Tokens.secMusic,
                                    icon: Icons.music_note,
                                  ),
                                ),
                              ),
                              const SizedBox(height: 5),
                              Text(s[i].label,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: TextStyle(
                                      fontSize: 11.5, color: t.textDim)),
                            ],
                          ),
                  ),
                ],
              ],
            ),
          ],
        ],
      ),
    );
  }
}

class _BookBand extends StatelessWidget {
  const _BookBand({required this.st});

  final HomeState st;

  @override
  Widget build(BuildContext context) {
    final b = st.recentBooks.take(4).toList();
    return PanelCard(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          ModernHead('Your shelf',
              link: 'Books',
              onLink: () => ShellController.instance.go(Section.books)),
          SizedBox(
            height: 180,
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: [
                for (var i = 0; i < b.length; i++) ...[
                  if (i > 0) const SizedBox(width: 8),
                  Expanded(
                    child: Tap(
                      onTap: () => openBook(b[i].id),
                      child: ClipRRect(
                        borderRadius: BorderRadius.circular(6),
                        child: AspectRatio(
                          aspectRatio: 2 / 3,
                          child: LazyCover(
                            section: Section.books,
                            id: b[i].id,
                            tint: Tokens.secBooks,
                            icon: Icons.menu_book_outlined,
                          ),
                        ),
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

class _Poster extends StatelessWidget {
  const _Poster({required this.tile});

  final HomeTile tile;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      width: 132,
      child: Tap(
        onTap: () => openVideo(tile.id),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            ClipRRect(
              borderRadius: BorderRadius.circular(12),
              child: SizedBox(
                height: 198,
                width: 132,
                child: LazyCover(
                  section: Section.videos,
                  id: tile.id,
                  tint: Tokens.secVideos,
                  icon: Icons.movie_outlined,
                ),
              ),
            ),
            const SizedBox(height: 7),
            Text(tile.label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 12, fontWeight: FontWeight.w600, color: t.text)),
            if (tile.sub.isNotEmpty)
              Text(tile.sub,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11, color: t.textDim)),
          ],
        ),
      ),
    );
  }
}
