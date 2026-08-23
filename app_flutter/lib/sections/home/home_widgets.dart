// The pieces every Home layout is built from.
//
// Four layouts draw the same facts differently, so what they share is here:
// the kind chip, the Continue card, the section tile, the now-playing card and
// the lazy cover. What differs — where those go, and what a layout leads with —
// stays in its own file.

import 'dart:io';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../shell/sidebar.dart';
import '../../src/rust/api/books.dart';
import '../../src/rust/api/home.dart';
import '../../src/rust/api/music.dart';
import '../../src/rust/api/videos.dart';
import '../music/music_controller.dart';
import 'home_controller.dart';

/// The palette Continue uses, kind for kind. Podcasts and audiobooks get their
/// own hues rather than borrowing Music's: on a strip of four cards, three the
/// same colour is three cards you cannot tell apart.
Color kindColor(String kind) => switch (kind) {
      'video' => Tokens.secVideos,
      'book' => Tokens.secBooks,
      'podcast' => const Color(0xFF8B5CF6),
      'audiobook' => const Color(0xFF3B82F6),
      _ => Tokens.secMusic,
    };

IconData kindIcon(String kind) => switch (kind) {
      'video' => Icons.movie_outlined,
      'book' => Icons.menu_book_outlined,
      'podcast' => Icons.podcasts,
      _ => Icons.headphones,
    };

/// Which section a Continue row belongs to.
Section kindSection(String kind) => switch (kind) {
      'video' => Section.videos,
      'book' => Section.books,
      _ => Section.music,
    };

/// A 24px pill. One size everywhere: the Continue kind tabs, the Recently
/// Added tabs and Stream's feed key all wear it.
class HomeChip extends StatelessWidget {
  const HomeChip({
    super.key,
    required this.label,
    required this.on,
    required this.onTap,
    this.accent = Tokens.brand,
    this.icon,
  });

  final String label;
  final bool on;
  final VoidCallback onTap;
  final Color accent;
  final IconData? icon;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: on ? accent.withValues(alpha: 0.18) : t.glass,
      borderRadius: BorderRadius.circular(13),
      child: InkWell(
        borderRadius: BorderRadius.circular(13),
        onTap: onTap,
        child: Container(
          height: 26,
          padding: EdgeInsets.symmetric(horizontal: icon == null ? 11 : 9),
          alignment: Alignment.center,
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (icon != null) ...[
                Icon(icon, size: 13, color: on ? accent : t.textDim),
                const SizedBox(width: 5),
              ],
              Text(label,
                  style: TextStyle(
                      fontSize: 11,
                      fontWeight: FontWeight.w700,
                      color: on ? accent : t.textDim)),
            ],
          ),
        ),
      ),
    );
  }
}

/// A caption above a block. Every layout uses the same one so the pages read as
/// one app in four moods rather than four apps.
class HomeCaption extends StatelessWidget {
  const HomeCaption(this.text, {super.key, this.trailing});

  final String text;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: Row(
        children: [
          Text(text,
              style: TextStyle(
                  fontSize: 11,
                  fontWeight: FontWeight.w800,
                  letterSpacing: 1.1,
                  color: t.textDim)),
          if (trailing != null) ...[const Spacer(), trailing!],
        ],
      ),
    );
  }
}

/// The Continue strip's kind tabs, in the order the Slint page lists them.
class ContinueTabs extends StatelessWidget {
  const ContinueTabs({
    super.key,
    required this.controller,
    required this.active,
  });

  final HomeController controller;
  final String active;

  @override
  Widget build(BuildContext context) => Wrap(
        spacing: 7,
        runSpacing: 7,
        children: [
          for (final k in kContinueKinds)
            HomeChip(
              label: k.label,
              on: active == k.id,
              accent: k.id == 'all' ? Tokens.secMusic : kindColor(k.id),
              onTap: () =>
                  controller.send(HomeCmd.setContinueFilter(filter: k.id)),
            ),
        ],
      );
}

/// Art for one library item, fetched by that section's own lazy resolver as the
/// tile appears. Photos has no standalone resolver, so its tiles keep a plate.
class LazyCover extends StatefulWidget {
  const LazyCover({
    super.key,
    required this.section,
    required this.id,
    required this.tint,
    this.icon,
    this.fit = BoxFit.cover,
  });

  final Section section;
  final int id;
  final Color tint;
  final IconData? icon;
  final BoxFit fit;

  @override
  State<LazyCover> createState() => _LazyCoverState();
}

class _LazyCoverState extends State<LazyCover> {
  String? _path;

  @override
  void initState() {
    super.initState();
    _ask();
  }

  @override
  void didUpdateWidget(LazyCover old) {
    super.didUpdateWidget(old);
    if (old.id != widget.id || old.section != widget.section) {
      _path = null;
      _ask();
    }
  }

  Future<void> _ask() async {
    final id = widget.id;
    try {
      final path = switch (widget.section) {
        Section.videos => await videosEnsureThumb(itemId: id),
        Section.books => await booksEnsureCover(id: id),
        _ => null,
      };
      if (mounted && path != null && widget.id == id) {
        setState(() => _path = path);
      }
    } catch (_) {
      // No art is a plate, not an error worth a banner over a landing page.
    }
  }

  @override
  Widget build(BuildContext context) {
    final plate = ColoredBox(
      color: widget.tint.withValues(alpha: 0.16),
      child: widget.icon == null
          ? null
          : Center(child: Icon(widget.icon, size: 20, color: widget.tint)),
    );
    if (_path == null) return plate;
    return Image.file(File(_path!),
        fit: widget.fit, errorBuilder: (_, __, ___) => plate);
  }
}

/// One in-progress item. The same card in all four layouts; only its width
/// changes.
class ContinueCard extends StatelessWidget {
  const ContinueCard({
    super.key,
    required this.controller,
    required this.row,
    this.dense = false,
  });

  final HomeController controller;
  final HomeContinue row;
  final bool dense;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = kindColor(row.kind);
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: () => ShellController.instance.go(kindSection(row.kind)),
        child: Container(
          padding: const EdgeInsets.all(10),
          decoration: BoxDecoration(
            color: t.panel,
            borderRadius: BorderRadius.circular(Tokens.radiusMd),
            border: Border.all(color: t.outline),
          ),
          child: Row(
            children: [
              ClipRRect(
                borderRadius: BorderRadius.circular(8),
                child: SizedBox(
                  width: dense ? 38 : 46,
                  height: dense ? 52 : 62,
                  child: LazyCover(
                    section: kindSection(row.kind),
                    id: row.id,
                    tint: tint,
                    icon: kindIcon(row.kind),
                  ),
                ),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(row.title,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: dense ? 11.5 : 12.5,
                            fontWeight: FontWeight.w700,
                            color: t.text)),
                    const SizedBox(height: 2),
                    Text(row.sub,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11, color: t.textDim)),
                    // A negative fraction is unknown, and the bar is hidden
                    // rather than drawn at zero under something clearly
                    // part-way through.
                    if (row.frac >= 0) ...[
                      const SizedBox(height: 6),
                      ClipRRect(
                        borderRadius: BorderRadius.circular(3),
                        child: LinearProgressIndicator(
                          value: row.frac,
                          minHeight: 4,
                          backgroundColor: t.outline,
                          color: tint,
                        ),
                      ),
                    ],
                  ],
                ),
              ),
              IconButton(
                tooltip: 'Not now',
                iconSize: 15,
                icon: Icon(Icons.close, color: t.textDim),
                onPressed: () => controller.send(HomeCmd.dismissContinue(
                    kind: row.kind, id: row.id, path: row.path)),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// The eight section tiles, with each section's real counts. Shared because
/// three of the four layouts show them; only their arrangement differs.
List<({Section section, String value, String sub})> hubTiles(HomeState st) {
  final c = st.counts;
  return [
    (
      section: Section.photos,
      value: '${c.photos}',
      sub: '${c.photosAlbums} albums'
    ),
    (
      section: Section.videos,
      value: '${c.videos}',
      sub: '${c.videosShows} shows'
    ),
    (
      section: Section.music,
      value: '${c.songs}',
      sub: '${c.podcasts} podcasts · ${c.radio} stations'
    ),
    (
      section: Section.books,
      value: '${c.books}',
      sub: '${c.booksReading} in progress'
    ),
    (
      section: Section.cloud,
      value: '${c.cloudRemotes}',
      sub: c.cloudRemotes == 1 ? 'remote' : 'remotes'
    ),
    (
      section: Section.tools,
      value: '${st.toolCount}',
      sub: '${c.toolsRunning} running · ${c.toolsQueued} queued'
    ),
    (
      section: Section.transfer,
      value: st.transferInbox.isEmpty ? '—' : '✓',
      sub: st.transferInbox.isEmpty ? 'send and receive' : 'inbox set'
    ),
    (
      section: Section.finances,
      value: '${c.financesDue}',
      sub: 'due or upcoming'
    ),
  ];
}

/// One hub tile. `tall` is the two-line form Welcome and Cinema use, where the
/// glyph rides the top and the label sits on the floor.
class HubTile extends StatefulWidget {
  const HubTile({
    super.key,
    required this.section,
    required this.value,
    required this.sub,
    this.tall = false,
  });

  final Section section;
  final String value;
  final String sub;
  final bool tall;

  @override
  State<HubTile> createState() => _HubTileState();
}

class _HubTileState extends State<HubTile> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final accent = accentFor(widget.section);
    final meta = kSectionMeta[widget.section]!;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: () => ShellController.instance.go(widget.section),
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 130),
          padding: const EdgeInsets.all(14),
          decoration: BoxDecoration(
            color: accent.withValues(alpha: _hover ? 0.26 : 0.14),
            borderRadius: BorderRadius.circular(16),
            border: Border.all(
                color: _hover
                    ? accent.withValues(alpha: 0.45)
                    : Colors.transparent),
          ),
          child: widget.tall
              ? Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Icon(meta.icon, size: 24, color: accent),
                    const Spacer(),
                    Text(meta.label,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 13,
                            fontWeight: FontWeight.w700,
                            color: t.text)),
                    Text('${widget.value} · ${widget.sub}',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 10.5, color: t.textDim)),
                  ],
                )
              : Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Icon(meta.icon, size: 17, color: accent),
                        const SizedBox(width: 8),
                        Text(meta.label,
                            style: TextStyle(
                                fontSize: 12.5,
                                fontWeight: FontWeight.w700,
                                color: t.text)),
                      ],
                    ),
                    const Spacer(),
                    Text(widget.value,
                        style: TextStyle(
                            fontSize: 22,
                            fontWeight: FontWeight.w800,
                            color: t.text)),
                    Text(widget.sub,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11, color: t.textDim)),
                  ],
                ),
        ),
      ),
    );
  }
}

/// The six launchers every layout carries: the doors that skip a section's
/// front page and land on the thing you actually wanted.
const List<({String label, IconData icon, Section section})> kLaunchers = [
  (label: 'Stream', icon: Icons.live_tv_outlined, section: Section.videos),
  (label: 'Live TV', icon: Icons.tv_outlined, section: Section.videos),
  (label: 'YouTube', icon: Icons.play_circle_outline, section: Section.music),
  (label: 'Radio', icon: Icons.radio_outlined, section: Section.music),
  (label: 'Genesis', icon: Icons.auto_awesome_outlined, section: Section.books),
  (label: 'Transfer', icon: Icons.share_outlined, section: Section.transfer),
];

class LaunchBar extends StatelessWidget {
  const LaunchBar({super.key, this.solid = false});

  /// Cinema's top bar fills its pills; the rest outline them.
  final bool solid;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Wrap(
      spacing: 8,
      runSpacing: 8,
      children: [
        for (final l in kLaunchers)
          Material(
            color: solid ? t.glassStrong : t.glass,
            borderRadius: BorderRadius.circular(18),
            child: InkWell(
              borderRadius: BorderRadius.circular(18),
              onTap: () => ShellController.instance.go(l.section),
              child: Container(
                height: 36,
                padding: const EdgeInsets.symmetric(horizontal: 14),
                alignment: Alignment.center,
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(l.icon, size: 15, color: accentFor(l.section)),
                    const SizedBox(width: 7),
                    Text(l.label,
                        style: TextStyle(
                            fontSize: 12,
                            fontWeight: FontWeight.w600,
                            color: t.text)),
                  ],
                ),
              ),
            ),
          ),
      ],
    );
  }
}

/// The now-playing card. Reads `MusicController.instance` — the same controller
/// the floating mini and the Music section's own bar read — rather than a
/// second player fed by its own snapshot. Two players that can disagree about
/// what is playing is the bug this avoids.
class NowPlayingCard extends StatelessWidget {
  const NowPlayingCard({
    super.key,
    required this.state,
    this.tall = false,
  });

  final HomeState state;

  /// Cinema's right column stands it up: big art, transport underneath.
  final bool tall;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = MusicController.instance;
    return AnimatedBuilder(
      animation: c,
      builder: (context, _) {
        final now = c.now;
        final live = now != null && now.title.isNotEmpty;
        final art = live && now.art.isNotEmpty
            ? Image.file(File(now.art),
                fit: BoxFit.cover,
                errorBuilder: (_, __, ___) => const _ArtPlate())
            : const _ArtPlate();
        final transport = Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            IconButton(
              tooltip: 'Previous',
              icon: const Icon(Icons.skip_previous, size: 20),
              onPressed: live ? () => c.send(const MusicCmd.prev()) : null,
            ),
            IconButton(
              tooltip: live ? 'Play / pause' : 'Shuffle everything',
              icon: Icon(
                  live && now.playing
                      ? Icons.pause_circle_filled
                      : Icons.play_circle_fill,
                  size: 34,
                  color: Tokens.secMusic),
              // With nothing loaded there is no shuffle-all on the bridge, so
              // this is a door into Music rather than a button that does
              // nothing.
              onPressed: () => live
                  ? c.send(const MusicCmd.playPause())
                  : ShellController.instance.go(Section.music),
            ),
            IconButton(
              tooltip: 'Next',
              icon: const Icon(Icons.skip_next, size: 20),
              onPressed: live ? () => c.send(const MusicCmd.next()) : null,
            ),
          ],
        );
        final title = Column(
          crossAxisAlignment:
              tall ? CrossAxisAlignment.center : CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(live ? now.title : 'Nothing playing',
                textAlign: tall ? TextAlign.center : TextAlign.start,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 14, fontWeight: FontWeight.w700, color: t.text)),
            const SizedBox(height: 2),
            Text(
                live
                    ? now.artist
                    : '${state.counts.songs} songs · '
                        '${state.counts.podcasts} podcasts · '
                        '${state.counts.radio} stations',
                textAlign: tall ? TextAlign.center : TextAlign.start,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 11.5, color: t.textDim)),
          ],
        );
        return Container(
          padding: const EdgeInsets.all(14),
          decoration: BoxDecoration(
            color: t.panel,
            borderRadius: BorderRadius.circular(Tokens.radiusMd),
            border: Border.all(color: t.outline),
          ),
          child: tall
              ? Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    AspectRatio(
                      aspectRatio: 1,
                      child: ClipRRect(
                        borderRadius: BorderRadius.circular(12),
                        child: art,
                      ),
                    ),
                    const SizedBox(height: 12),
                    title,
                    const SizedBox(height: 6),
                    transport,
                  ],
                )
              : Row(
                  children: [
                    ClipRRect(
                      borderRadius: BorderRadius.circular(10),
                      child: SizedBox(width: 56, height: 56, child: art),
                    ),
                    const SizedBox(width: 12),
                    Expanded(child: title),
                    transport,
                  ],
                ),
        );
      },
    );
  }
}

class _ArtPlate extends StatelessWidget {
  const _ArtPlate();

  @override
  Widget build(BuildContext context) => ColoredBox(
        color: Tokens.secMusic.withValues(alpha: 0.14),
        child:
            const Center(child: Icon(Icons.music_note, color: Tokens.secMusic)),
      );
}

/// One a day, from a shelf shuffled once per run. The same quote all day is a
/// fixture you can come to like; a new one per repaint is noise.
class QuoteBlock extends StatelessWidget {
  const QuoteBlock({super.key, required this.quote});

  final HomeQuote quote;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (quote.text.isEmpty) return const SizedBox.shrink();
    return Container(
      padding: const EdgeInsets.fromLTRB(16, 13, 16, 13),
      decoration: BoxDecoration(
        color: Tokens.brand.withValues(alpha: 0.07),
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        border: Border(
            left: BorderSide(
                color: Tokens.brand.withValues(alpha: 0.5), width: 3)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(quote.text,
              style: TextStyle(
                  fontSize: 13.5, fontStyle: FontStyle.italic, color: t.text)),
          if (quote.author.isNotEmpty) ...[
            const SizedBox(height: 4),
            Text('— ${quote.author}',
                style: TextStyle(fontSize: 11.5, color: t.textDim)),
          ],
        ],
      ),
    );
  }
}

/// A horizontal shelf of recently-added things.
class Shelf extends StatelessWidget {
  const Shelf({
    super.key,
    required this.title,
    required this.section,
    required this.tiles,
  });

  final String title;
  final Section section;
  final List<HomeTile> tiles;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(bottom: 20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          HomeCaption(
            title,
            trailing: TextButton(
              onPressed: () => ShellController.instance.go(section),
              child: const Text('Open', style: TextStyle(fontSize: 11.5)),
            ),
          ),
          SizedBox(
            height: 128,
            child: ListView.separated(
              scrollDirection: Axis.horizontal,
              itemCount: tiles.length,
              separatorBuilder: (_, __) => const SizedBox(width: 10),
              itemBuilder: (context, i) => MouseRegion(
                cursor: SystemMouseCursors.click,
                child: GestureDetector(
                  onTap: () => ShellController.instance.go(section),
                  child: SizedBox(
                    width: 96,
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Expanded(
                          child: ClipRRect(
                            borderRadius: BorderRadius.circular(10),
                            child: SizedBox(
                              width: 96,
                              child: LazyCover(
                                section: section,
                                id: tiles[i].id,
                                tint: accentFor(section),
                                icon: kSectionMeta[section]!.icon,
                              ),
                            ),
                          ),
                        ),
                        const SizedBox(height: 5),
                        Text(tiles[i].label,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 11, color: t.text)),
                        if (tiles[i].sub.isNotEmpty)
                          Text(tiles[i].sub,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(fontSize: 10, color: t.textDim)),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}
