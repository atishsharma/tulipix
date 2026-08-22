// The pieces every Music tab is built from: artwork that resolves itself, the
// card, the track row, the chip, the pager, the empty state.
//
// One file rather than one per widget because they are each twenty lines and
// they are used by all five tabs — the alternative is nine imports at the top
// of every page for no gain.

import 'dart:io';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';

/// Artwork with a fallback glyph. `kind`/`key` are what `music_ensure_art`
/// takes; a null answer paints the placeholder rather than a broken image.
///
/// The remote case matters for Podcasts and YouTube, whose art starts life as
/// a URL: the bridge caches it to disk on first ask, so this only ever draws
/// from a file and the two builds share one cache.
class MusicArt extends StatelessWidget {
  const MusicArt({
    super.key,
    required this.controller,
    required this.kind,
    required this.artKey,
    required this.size,
    this.direct,
    this.radius = 8,
    this.fallback = Icons.music_note,
  });

  final MusicController controller;
  final String kind;
  final String artKey;

  /// A path the snapshot already knew, which skips the resolver entirely.
  final String? direct;
  final double size;
  final double radius;
  final IconData fallback;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    String? path = direct != null && direct!.isNotEmpty ? direct : null;
    path ??= controller.artFor(kind, artKey);
    return ClipRRect(
      borderRadius: BorderRadius.circular(radius),
      child: SizedBox(
        width: size,
        height: size,
        child: path == null
            ? ColoredBox(
                color: t.nTile,
                child: Icon(fallback, size: size * 0.4, color: t.nInk2),
              )
            : Image.file(
                File(path),
                fit: BoxFit.cover,
                // A cover that was deleted under us is a missing picture, not
                // a crashed grid.
                errorBuilder: (_, __, ___) => ColoredBox(
                  color: t.nTile,
                  child: Icon(fallback, size: size * 0.4, color: t.nInk2),
                ),
              ),
      ),
    );
  }
}

/// A pill in any of the section's chip rows. `tint`/`tint2` give it the
/// two-stop gradient the Slint chips carry when active.
class MusicChip extends StatelessWidget {
  const MusicChip({
    super.key,
    required this.label,
    required this.active,
    required this.onTap,
    this.icon,
    this.tint,
    this.tint2,
    this.badge,
  });

  final String label;
  final bool active;
  final VoidCallback onTap;
  final IconData? icon;
  final Color? tint;
  final Color? tint2;
  final String? badge;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final a = tint ?? Tokens.secMusic;
    final b = tint2 ?? Tokens.brand;
    const r = 18.0;
    return Material(
      color: active ? Colors.transparent : t.nChip,
      borderRadius: BorderRadius.circular(r),
      child: Ink(
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(r),
          gradient: active
              ? LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: [a, b],
                )
              : null,
        ),
        child: InkWell(
          borderRadius: BorderRadius.circular(r),
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 8),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (icon != null) ...[
                  Icon(icon, size: 16, color: active ? Colors.white : t.nInk2),
                  const SizedBox(width: 6),
                ],
                Text(
                  label,
                  style: TextStyle(
                    fontSize: 13,
                    fontWeight: active ? FontWeight.w600 : FontWeight.w400,
                    color: active ? Colors.white : t.nInk,
                  ),
                ),
                if (badge != null) ...[
                  const SizedBox(width: 6),
                  Text(
                    badge!,
                    style: TextStyle(
                      fontSize: 11,
                      color: active ? Colors.white70 : t.nInk2,
                    ),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// A square tile: album, artist, genre, playlist, folder, podcast, book.
class MusicCard extends StatefulWidget {
  const MusicCard({
    super.key,
    required this.controller,
    required this.title,
    required this.subtitle,
    required this.artKind,
    required this.artKey,
    this.direct,
    required this.onTap,
    this.onPlay,
    this.onMenu,
    this.round = false,
    this.badge,
    this.progress,
    this.fallback = Icons.album_outlined,
  });

  final MusicController controller;
  final String title;
  final String subtitle;
  final String artKind;
  final String artKey;
  final String? direct;
  final VoidCallback onTap;
  final VoidCallback? onPlay;
  final VoidCallback? onMenu;

  /// Artists read as people, so their tile is a circle. Everything else is a
  /// rounded square.
  final bool round;
  final String? badge;

  /// 0..1, drawn as a bar across the bottom of the art. Books and part-watched
  /// videos have one; an album does not.
  final double? progress;
  final IconData fallback;

  @override
  State<MusicCard> createState() => _MusicCardState();
}

class _MusicCardState extends State<MusicCard> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: widget.onTap,
        onSecondaryTap: widget.onMenu,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            AspectRatio(
              aspectRatio: 1,
              child: LayoutBuilder(
                builder: (context, box) => Stack(
                  fit: StackFit.expand,
                  children: [
                    MusicArt(
                      controller: widget.controller,
                      kind: widget.artKind,
                      artKey: widget.artKey,
                      direct: widget.direct,
                      size: box.maxWidth,
                      radius: widget.round ? box.maxWidth / 2 : 10,
                      fallback: widget.fallback,
                    ),
                    if (widget.badge != null)
                      Positioned(
                        top: 6,
                        left: 6,
                        child: _Badge(text: widget.badge!),
                      ),
                    if (widget.progress != null && widget.progress! > 0)
                      Positioned(
                        left: 0,
                        right: 0,
                        bottom: 0,
                        child: LinearProgressIndicator(
                          value: widget.progress!.clamp(0.0, 1.0),
                          minHeight: 3,
                          backgroundColor: Colors.black26,
                          valueColor:
                              const AlwaysStoppedAnimation(Tokens.secMusic),
                        ),
                      ),
                    if (_hovered && widget.onPlay != null)
                      Positioned(
                        right: 8,
                        bottom: 8,
                        child: _PlayFab(onPressed: widget.onPlay!),
                      ),
                  ],
                ),
              ),
            ),
            const SizedBox(height: 8),
            Text(
              widget.title,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontSize: 13,
                fontWeight: FontWeight.w600,
                color: t.nInk,
              ),
            ),
            if (widget.subtitle.isNotEmpty)
              Text(
                widget.subtitle,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 11, color: t.nInk2),
              ),
          ],
        ),
      ),
    );
  }
}

class _Badge extends StatelessWidget {
  const _Badge({required this.text});
  final String text;

  @override
  Widget build(BuildContext context) => DecoratedBox(
        decoration: BoxDecoration(
          color: Colors.black.withValues(alpha: 0.62),
          borderRadius: BorderRadius.circular(6),
        ),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
          child: Text(
            text,
            style: const TextStyle(
                fontSize: 11, color: Colors.white, fontWeight: FontWeight.w600),
          ),
        ),
      );
}

class _PlayFab extends StatelessWidget {
  const _PlayFab({required this.onPressed});
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) => Material(
        color: Tokens.secMusic,
        shape: const CircleBorder(),
        elevation: 4,
        child: InkWell(
          customBorder: const CircleBorder(),
          onTap: onPressed,
          child: const Padding(
            padding: EdgeInsets.all(8),
            child: Icon(Icons.play_arrow, size: 20, color: Colors.white),
          ),
        ),
      );
}

/// One row of any track list. The star, the heart and the lyrics glyph are the
/// three per-row controls the Slint `SongCard` carries; the rest of its
/// seventeen properties were hover states Flutter derives.
class TrackRow extends StatefulWidget {
  const TrackRow({
    super.key,
    required this.controller,
    required this.track,
    required this.index,
    required this.onPlay,
    this.onQueue,
    this.onRemove,
    this.showArt = true,
    this.dense = false,
  });

  final MusicController controller;
  final Track track;
  final int index;
  final VoidCallback onPlay;
  final VoidCallback? onQueue;
  final VoidCallback? onRemove;
  final bool showArt;
  final bool dense;

  @override
  State<TrackRow> createState() => _TrackRowState();
}

class _TrackRowState extends State<TrackRow> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tr = widget.track;
    final playing = widget.controller.now?.itemId == tr.itemId &&
        (widget.controller.now?.loaded ?? false);
    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: InkWell(
        onTap: widget.onPlay,
        child: Container(
          height: widget.dense ? 44 : 56,
          padding: const EdgeInsets.symmetric(horizontal: 12),
          color: _hovered ? t.nHover : Colors.transparent,
          child: Row(
            children: [
              SizedBox(
                width: 28,
                child: playing
                    ? const Icon(Icons.equalizer,
                        size: 16, color: Tokens.secMusic)
                    : Text(
                        '${widget.index + 1}',
                        textAlign: TextAlign.center,
                        style: TextStyle(fontSize: 12, color: t.nInk2),
                      ),
              ),
              if (widget.showArt) ...[
                MusicArt(
                  controller: widget.controller,
                  kind: 'track',
                  artKey: '${tr.itemId}',
                  direct: tr.art,
                  size: widget.dense ? 32 : 40,
                  radius: 6,
                ),
                const SizedBox(width: 12),
              ],
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    Text(
                      tr.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: 13,
                        fontWeight: playing ? FontWeight.w600 : FontWeight.w400,
                        color: playing ? Tokens.secMusic : t.nInk,
                      ),
                    ),
                    if (tr.artist.isNotEmpty || tr.album.isNotEmpty)
                      Text(
                        [tr.artist, tr.album]
                            .where((s) => s.isNotEmpty)
                            .join(' · '),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11, color: t.nInk2),
                      ),
                  ],
                ),
              ),
              if (tr.lyrics.isNotEmpty)
                Tooltip(
                  message:
                      tr.lyrics == 'synced' ? 'Synced lyrics' : 'Lyrics stored',
                  child: Icon(
                    Icons.lyrics_outlined,
                    size: 15,
                    color: tr.lyrics == 'synced' ? Tokens.secMusic : t.nInk2,
                  ),
                ),
              const SizedBox(width: 8),
              IconButton(
                iconSize: 17,
                visualDensity: VisualDensity.compact,
                tooltip: tr.loved ? 'Remove from favourites' : 'Favourite',
                icon: Icon(
                  tr.loved ? Icons.favorite : Icons.favorite_border,
                  color: tr.loved ? Tokens.secMusic : t.nInk2,
                ),
                onPressed: () =>
                    widget.controller.send(MusicCmd.love(itemId: tr.itemId)),
              ),
              _Stars(
                stars: tr.stars,
                onSet: (n) => widget.controller
                    .send(MusicCmd.rate(itemId: tr.itemId, stars: n)),
              ),
              const SizedBox(width: 8),
              SizedBox(
                width: 48,
                child: Text(
                  fmtClock(tr.durationS),
                  textAlign: TextAlign.right,
                  style: TextStyle(fontSize: 12, color: t.nInk2),
                ),
              ),
              PopupMenuButton<String>(
                iconSize: 18,
                tooltip: 'More',
                onSelected: (v) => _menu(context, v),
                itemBuilder: (_) => [
                  if (widget.onQueue != null)
                    const PopupMenuItem(
                        value: 'queue', child: Text('Add to queue')),
                  const PopupMenuItem(
                      value: 'playlist', child: Text('Add to playlist…')),
                  const PopupMenuItem(value: 'tags', child: Text('Edit tags…')),
                  if (widget.onRemove != null)
                    const PopupMenuItem(
                        value: 'remove', child: Text('Remove from this list')),
                  const PopupMenuItem(
                      value: 'delete', child: Text('Delete from disk…')),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }

  /// The row's context menu. Everything here needs a dialog, a controller or
  /// both, which is why it is a method on the state rather than callbacks the
  /// six call sites would each have to wire.
  Future<void> _menu(BuildContext context, String choice) async {
    final tr = widget.track;
    switch (choice) {
      case 'queue':
        widget.onQueue?.call();
      case 'remove':
        widget.onRemove?.call();
      case 'playlist':
        await addToPlaylist(context, widget.controller, [tr]);
      case 'tags':
        await editTags(context, widget.controller, tr);
      case 'delete':
        final ok = await confirm(
          context,
          title: 'Delete this file?',
          body: '“${tr.title}” is removed from disk as well as from '
              'the library. There is no trash for audio — this cannot be '
              'undone.',
        );
        if (ok) {
          await widget.controller.send(MusicCmd.deleteTrack(itemId: tr.itemId));
        }
    }
  }
}

class _Stars extends StatelessWidget {
  const _Stars({required this.stars, required this.onSet});
  final int stars;
  final void Function(int) onSet;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (var i = 1; i <= 5; i++)
          GestureDetector(
            // Clicking the star that is already lit clears the rating, which
            // is the only way to get back to "unrated" from five.
            onTap: () => onSet(stars == i ? 0 : i),
            child: Icon(
              i <= stars ? Icons.star : Icons.star_border,
              size: 13,
              color: i <= stars ? Tokens.warn : t.nInk2,
            ),
          ),
      ],
    );
  }
}

/// Previous / page-of / next. Every paged list in the section uses it.
class Pager extends StatelessWidget {
  const Pager({
    super.key,
    required this.page,
    required this.pages,
    required this.onGo,
  });

  final int page;
  final int pages;
  final void Function(int) onGo;

  @override
  Widget build(BuildContext context) {
    if (pages <= 1) return const SizedBox.shrink();
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 16),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          IconButton(
            icon: const Icon(Icons.chevron_left),
            onPressed: page > 0 ? () => onGo(page - 1) : null,
          ),
          Text('${page + 1} / $pages',
              style: TextStyle(fontSize: 13, color: t.nInk2)),
          IconButton(
            icon: const Icon(Icons.chevron_right),
            onPressed: page + 1 < pages ? () => onGo(page + 1) : null,
          ),
        ],
      ),
    );
  }
}

/// The section's one empty state. A tab with nothing in it should say why and
/// offer the thing that fixes it — a blank pane reads as a bug.
class MusicEmpty extends StatelessWidget {
  const MusicEmpty({
    super.key,
    required this.icon,
    required this.title,
    required this.body,
    this.action,
  });

  final IconData icon;
  final String title;
  final String body;
  final (String, VoidCallback)? action;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 48, color: t.nInk2),
          const SizedBox(height: 16),
          Text(title,
              style: TextStyle(
                  fontSize: 17, fontWeight: FontWeight.w600, color: t.nInk)),
          const SizedBox(height: 6),
          SizedBox(
            width: 380,
            child: Text(
              body,
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 13, color: t.nInk2),
            ),
          ),
          if (action != null) ...[
            const SizedBox(height: 18),
            FilledButton(onPressed: action!.$2, child: Text(action!.$1)),
          ],
        ],
      ),
    );
  }
}

/// A titled horizontal rail. My Music's home and YouTube's home are both
/// stacks of these.
class Rail extends StatelessWidget {
  const Rail({
    super.key,
    required this.title,
    required this.height,
    required this.children,
    this.action,
  });

  final String title;
  final double height;
  final List<Widget> children;
  final (String, VoidCallback)? action;

  @override
  Widget build(BuildContext context) {
    if (children.isEmpty) return const SizedBox.shrink();
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 20, 24, 10),
          child: Row(
            children: [
              Text(title,
                  style: TextStyle(
                      fontSize: 15,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
              const Spacer(),
              if (action != null)
                TextButton(onPressed: action!.$2, child: Text(action!.$1)),
            ],
          ),
        ),
        SizedBox(
          height: height,
          child: ListView.separated(
            scrollDirection: Axis.horizontal,
            padding: const EdgeInsets.symmetric(horizontal: 24),
            itemCount: children.length,
            separatorBuilder: (_, __) => const SizedBox(width: 14),
            itemBuilder: (_, i) => SizedBox(
              // Square art plus two lines of label. Fixed rather than
              // intrinsic: a rail of intrinsically-sized children re-measures
              // every one of them on every scroll frame.
              width: height - 42,
              child: children[i],
            ),
          ),
        ),
      ],
    );
  }
}

/// The responsive grid every card page uses. `min` is the smallest a tile may
/// get before the grid drops a column.
class CardGrid extends StatelessWidget {
  const CardGrid({
    super.key,
    required this.children,
    this.min = 168,
    this.padding = const EdgeInsets.fromLTRB(24, 16, 24, 24),
  });

  final List<Widget> children;
  final double min;
  final EdgeInsets padding;

  @override
  Widget build(BuildContext context) => GridView.builder(
        padding: padding,
        gridDelegate: SliverGridDelegateWithMaxCrossAxisExtent(
          maxCrossAxisExtent: min,
          mainAxisSpacing: 18,
          crossAxisSpacing: 14,
          // Square art plus the two label lines under it.
          childAspectRatio: 0.78,
        ),
        itemCount: children.length,
        itemBuilder: (_, i) => children[i],
      );
}
