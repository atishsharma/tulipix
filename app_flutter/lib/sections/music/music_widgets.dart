// The pieces every Music tab is built from: artwork that resolves itself, the
// card, the track row, the chip, the pager, the empty state.
//
// One file rather than one per widget because they are each twenty lines and
// they are used by all five tabs — the alternative is nine imports at the top
// of every page for no gain.

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';
import 'music_motion.dart';
import 'song_menu.dart';

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

/// Rows per page on the Songs tab. `SONGS_PAGE` in
/// crates/tulipix-bridge/src/api/music.rs — eight columns by four rows. The
/// page index has to be multiplied by it to turn a row's place on the page
/// into its place in the library.
const int kSongsPerPage = 32;

/// Loved and History are lists, not grids — `LIST_ROWS` in the bridge.
const int kListRows = 25;

/// How wide a collapsed [MusicChip] is.
///
/// Slint's `min-width` for a collapsed tab is 44, which is square at the 36px
/// chip height — a circle, in other words, and at the port's scale a row of
/// them read as a row of icon buttons rather than as tabs. 56 is a pill: wide
/// enough that the shape says "tab" before the label appears on hover. A
/// deliberate divergence, asked for.
const double kChipCollapsed = 56;

/// A pill in any of the section's chip rows. `tint`/`tint2` give it the
/// two-stop gradient the Slint chips carry when active.
/// A header tab, ported from `HdrChip` with `has-grad` in ui/section_header.slint.
///
/// The point of the colour is that a row of these reads as five (or ten)
/// distinct things at a glance, not as one selected thing and a queue of grey.
/// So the tint is on the chip at rest too — a wash of it behind, an outline of
/// it around, and the label inked in it — and going active only turns the wash
/// into the full gradient.
///
/// [collapsible] is how ten of them fit on one row: icon only until the pointer
/// is on it or it is the open tab.
class MusicChip extends StatefulWidget {
  const MusicChip({
    super.key,
    required this.label,
    required this.active,
    required this.onTap,
    this.icon,
    this.tint,
    this.tint2,
    this.badge,
    this.collapsible = false,
    this.minWidth = 104,
    this.expand = false,
  });

  final String label;
  final bool active;
  final VoidCallback onTap;
  final IconData? icon;
  final Color? tint;
  final Color? tint2;
  final String? badge;

  /// Icon-only at rest, label revealed on hover or when active.
  final bool collapsible;

  /// The expanded width. A collapsed chip is [kChipCollapsed] wide.
  final double minWidth;

  /// Fill whatever the parent gives it, rather than hugging its label. For the
  /// two chips at the head of the docked panel, which are its tabs.
  final bool expand;

  @override
  State<MusicChip> createState() => _MusicChipState();
}

class _MusicChipState extends State<MusicChip> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final a = widget.tint ?? Tokens.secMusic;
    final b = widget.tint2 ?? Tokens.brand;
    final active = widget.active;
    final shown = !widget.collapsible || active || _hovered;
    final skin = context.skin;
    // A skin draws the chip in its own material — pressed in when active — and
    // inks it in its accent, in place of the tint wash and the gradient.
    final skinned =
        skin.control(active: active, hovered: _hovered, radius: 18);

    // `tint.mix(#000000, 0.72)` / `tint.mix(#ffffff, 0.5)` — Slint's mix is
    // factor * self + (1 - factor) * other, so these are lerps *from* the
    // second colour. A raw tint on a pale canvas is unreadable at 13px.
    final ink = skinned != null
        ? (active ? skin.accent! : skin.inkDim!)
        : active
            ? Colors.white
            : (t.dark
                ? Color.lerp(Colors.white, a, 0.5)!
                : Color.lerp(Colors.black, a, 0.72)!);

    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: t.reduceMotion
              ? Duration.zero
              : const Duration(milliseconds: 140),
          curve: Curves.easeOut,
          height: 36,
          width:
              widget.expand ? double.infinity : (shown ? null : kChipCollapsed),
          constraints: shown
              ? BoxConstraints(minWidth: widget.minWidth)
              : const BoxConstraints(),
          padding: const EdgeInsets.symmetric(horizontal: 4),
          decoration: skinned ??
              BoxDecoration(
            borderRadius: BorderRadius.circular(18),
            gradient: active
                ? LinearGradient(
                    // Slint's 120deg, which runs down-and-right rather than
                    // corner to corner.
                    begin: const Alignment(-1, -0.58),
                    end: const Alignment(1, 0.58),
                    colors: [a, b],
                  )
                : null,
            color: active
                ? null
                : a.withValues(
                    alpha: _hovered
                        ? (t.dark ? 0.30 : 0.34)
                        : (t.dark ? 0.15 : 0.20),
                  ),
            border: active
                ? null
                : Border.all(
                    color: a.withValues(alpha: t.dark ? 0.55 : 0.85),
                  ),
          ),
          clipBehavior: Clip.antiAlias,
          child: Row(
            mainAxisAlignment: MainAxisAlignment.center,
            mainAxisSize: widget.expand ? MainAxisSize.max : MainAxisSize.min,
            children: [
              if (widget.icon != null)
                Icon(skin.icon(widget.icon!), size: 15, color: ink),
              if (shown && widget.label.isNotEmpty) ...[
                if (widget.icon != null) const SizedBox(width: 7),
                // Flexible, because the label appears the frame `shown` flips
                // while the width is still animating up from `kChipCollapsed`.
                // For those few frames the text is wider than the chip, and an
                // unflexed Text in a Row is a `RenderFlex overflowed` every
                // time a sub-tab is hovered or collapsed.
                Flexible(
                  child: Text(
                    widget.label,
                    maxLines: 1,
                    overflow: TextOverflow.clip,
                    softWrap: false,
                    style: TextStyle(
                      fontFamily: skin.fontFamily ?? Tokens.fontFamily,
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      color: ink,
                    ),
                  ),
                ),
              ],
              if (shown && widget.badge != null) ...[
                const SizedBox(width: 6),
                Flexible(
                  child: Text(
                    widget.badge!,
                    maxLines: 1,
                    softWrap: false,
                    overflow: TextOverflow.clip,
                    style: TextStyle(
                      fontFamily: Tokens.fontFamily,
                      fontSize: 10.5,
                      fontWeight: FontWeight.w800,
                      color: ink.withValues(alpha: 0.6),
                    ),
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// The chrome under a tile's artwork, as a fixed reservation.
///
/// Guessing the height of text is the bug these replace. 13px is a 19px line in
/// one font and a 21px line in another, and a shelf that reserved "30 for a gap
/// and a label" was a pixel over in one locale and six over in the next — which
/// is a `RenderFlex overflowed` under every tile on the page. The lines are
/// drawn in boxes of exactly these heights now, so a shelf reserving the same
/// numbers is doing arithmetic rather than taking a measurement.
const double kCardGap = 8;
const double kCardTitleLine = 20;
const double kCardSubLine = 17;

/// What a shelf reserves under a tile that carries a title only.
const double kCardLabelOne = kCardGap + kCardTitleLine;

/// ...and under one that carries a subtitle as well. Nearly every grid in the
/// section does: an album needs its artist, an artist needs its track count.
const double kCardLabelTwo = kCardLabelOne + kCardSubLine;

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
    this.count = 0,
    this.loved,
    this.stars,
    this.onFav,
    this.onRate,
    this.centred = false,
    this.lyrics = '',
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

  /// How many tracks are behind this tile. Drawn top-right in a gradient disc,
  /// always visible, because it is the one number that tells you whether an
  /// album is an album or a single stray file.
  final int count;

  /// The heart, top-left. Null hides it entirely — Slint's `fav-enabled`.
  final bool? loved;

  /// 0..5, on the bottom strip. Null hides it, as above.
  final int? stars;
  final VoidCallback? onFav;
  final void Function(int)? onRate;

  /// Centre the label under the art. Circular tiles want it; a grid of squares
  /// reads better ranged left.
  final bool centred;

  /// "" | plain | synced. A song tile wears a mark in the top-right corner when
  /// its words are stored, filled when they are timed — the one thing about a
  /// track you cannot tell from its cover, and the reason the Tags & Lyrics
  /// manager exists.
  final String lyrics;

  @override
  State<MusicCard> createState() => _MusicCardState();
}

class _MusicCardState extends State<MusicCard> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final w = widget;
    final showFav = w.loved != null;
    final showRate = w.stars != null;
    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: w.onTap,
        onSecondaryTap: w.onMenu,
        child: Column(
          crossAxisAlignment:
              w.centred ? CrossAxisAlignment.center : CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            // Flexible, not a bare AspectRatio: every shelf that draws these
            // sizes its tile as `art + 42`, and the two text lines under the
            // art are 43 -- 8 gap + 19 title + 16 subtitle -- so all 45 cards
            // on screen overflowed their column by exactly one pixel. Guessing
            // the height of text is the bug, not the constant: the same sum
            // moves again with the font, the locale, or the reader's text
            // scale. Loose fit, so the square gives up the pixel instead. With
            // an unbounded height (no shelf caps it) RenderFlex lays a loose
            // flex child out as an ordinary one, so this stays width x width.
            Flexible(
              child: AspectRatio(
                aspectRatio: 1,
                child: LayoutBuilder(
                  builder: (context, box) {
                    final side = box.maxWidth;
                    final radius = w.round ? side / 2 : 10.0;
                    // A skin sets the art in a mat of its own material, in
                    // place of the hover ring and glow.
                    final mat = context.skin
                        .surface(SurfaceRole.art, radius: radius + 5);
                    return Container(
                      padding: EdgeInsets.all(mat == null ? 0 : 5),
                      // Hover elevation: a 2px accent ring and a coloured glow,
                      // drawn outside the clip so neither eats into the art.
                      decoration: mat ?? BoxDecoration(
                        borderRadius: BorderRadius.circular(radius),
                        border: _hovered
                            ? Border.all(color: Tokens.secMusic, width: 2)
                            : null,
                        boxShadow: _hovered
                            ? const [
                                BoxShadow(
                                  color: Color(0xAAEC4899),
                                  blurRadius: 28,
                                )
                              ]
                            : null,
                      ),
                      child: ClipRRect(
                        borderRadius: BorderRadius.circular(radius),
                        child: Stack(
                          fit: StackFit.expand,
                          children: [
                            MusicArt(
                              controller: w.controller,
                              kind: w.artKind,
                              artKey: w.artKey,
                              direct: w.direct,
                              size: side,
                              radius: 0,
                              fallback: w.fallback,
                            ),
                            if (_hovered)
                              ColoredBox(
                                color: const Color(0x55000000),
                                child: Center(
                                  child: _PlayFab(
                                    onPressed: w.onPlay ?? w.onTap,
                                  ),
                                ),
                              ),
                            if (w.badge != null)
                              Positioned(
                                top: 6,
                                left: 6,
                                child: _Badge(text: w.badge!),
                              ),
                            // The heart sits on the art rather than beside the
                            // label, so a wall of tiles shows what is loved
                            // without a second row of chrome under each one.
                            if (showFav && (_hovered || w.loved!))
                              Positioned(
                                top: 8,
                                left: 8,
                                child: _Disc(
                                  icon: w.loved!
                                      ? Icons.favorite
                                      : Icons.favorite_border,
                                  colour:
                                      w.loved! ? Tokens.secMusic : Colors.white,
                                  label: w.loved!
                                      ? 'Remove ${w.title} from favourites'
                                      : 'Add ${w.title} to favourites',
                                  onTap: w.onFav,
                                ),
                              ),
                            if (w.count > 0)
                              Positioned(
                                top: 8,
                                right: 8,
                                child: _CountDisc(count: w.count),
                              ),
                            if (w.lyrics.isNotEmpty)
                              Positioned(
                                top: 8,
                                right: 8,
                                child: _LyricDisc(synced: w.lyrics == 'synced'),
                              ),
                            if (showRate && (_hovered || w.stars! > 0))
                              Positioned(
                                left: 0,
                                right: 0,
                                bottom: 2,
                                child: _RateStrip(
                                  stars: w.stars!,
                                  onSet: w.onRate,
                                ),
                              ),
                            if (w.progress != null && w.progress! > 0)
                              Positioned(
                                left: 0,
                                right: 0,
                                bottom: 0,
                                child: LinearProgressIndicator(
                                  value: w.progress!.clamp(0.0, 1.0),
                                  minHeight: 3,
                                  backgroundColor: Colors.black26,
                                  valueColor: const AlwaysStoppedAnimation(
                                      Tokens.secMusic),
                                ),
                              ),
                          ],
                        ),
                      ),
                    );
                  },
                ),
              ),
            ),
            const SizedBox(height: kCardGap),
            SizedBox(
              height: kCardTitleLine,
              child: Align(
                alignment: w.centred ? Alignment.center : Alignment.centerLeft,
                child: Text(
                  w.title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  textAlign: w.centred ? TextAlign.center : TextAlign.start,
                  style: TextStyle(
                    fontSize: 13,
                    fontWeight: FontWeight.w600,
                    color: t.nInk,
                  ),
                ),
              ),
            ),
            if (w.subtitle.isNotEmpty)
              SizedBox(
                height: kCardSubLine,
                child: Align(
                  alignment:
                      w.centred ? Alignment.center : Alignment.centerLeft,
                  child: Text(
                    w.subtitle,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    textAlign: w.centred ? TextAlign.center : TextAlign.start,
                    style: TextStyle(fontSize: 11, color: t.nInk2),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// The lyric mark in a song tile's top-right corner. Filled in the section's
/// pink when the words are timed to the music, outlined when they are only
/// stored — which is the difference between a sheet you can sing along to and
/// a block of text.
class _LyricDisc extends StatelessWidget {
  const _LyricDisc({required this.synced});

  final bool synced;

  @override
  Widget build(BuildContext context) => Tooltip(
        message: synced ? 'Synced lyrics' : 'Lyrics stored',
        child: Container(
          width: 26,
          height: 26,
          alignment: Alignment.center,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: synced ? Tokens.secMusic : const Color(0xAA000000),
          ),
          child: Icon(
            synced ? Icons.lyrics : Icons.lyrics_outlined,
            size: 14,
            color: Colors.white,
          ),
        ),
      );
}

/// A 30px black disc with a glyph in it — the tile's fav corner.
class _Disc extends StatelessWidget {
  const _Disc({
    required this.icon,
    required this.colour,
    this.onTap,
    this.label,
  });

  final IconData icon;
  final Color colour;
  final VoidCallback? onTap;

  /// What this disc is, for anyone not looking at it. A heart over a cover is
  /// unambiguous by sight and completely silent otherwise.
  final String? label;

  @override
  Widget build(BuildContext context) {
    final disc = SizedBox(
      width: 30,
      height: 30,
      child: Material(
        color: const Color(0xAA000000),
        shape: const CircleBorder(),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onTap,
          child: Icon(icon, size: 16, color: colour),
        ),
      ),
    );
    return label == null ? disc : Tooltip(message: label!, child: disc);
  }
}

/// The track count, top-right, in the section gradient. Always on: it is the
/// difference between an album and one stray file that happens to have art.
class _CountDisc extends StatelessWidget {
  const _CountDisc({required this.count});

  final int count;

  @override
  Widget build(BuildContext context) => Container(
        width: 28,
        height: 28,
        alignment: Alignment.center,
        decoration: const BoxDecoration(
          shape: BoxShape.circle,
          gradient: LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [Color(0xFFEC4899), Color(0xFF8B5CF6)],
          ),
          boxShadow: [BoxShadow(color: Color(0x88000000), blurRadius: 8)],
        ),
        child: Text(
          '$count',
          style: TextStyle(
            fontFamily: Tokens.fontFamily,
            fontSize: count > 99 ? 10 : 12,
            fontWeight: FontWeight.w800,
            color: Colors.white,
          ),
        ),
      );
}

/// Five stars across the foot of a tile. Clicking star n rates n; clicking the
/// star that is already the rating clears it.
class _RateStrip extends StatelessWidget {
  const _RateStrip({required this.stars, this.onSet});

  final int stars;
  final void Function(int)? onSet;

  @override
  Widget build(BuildContext context) => Container(
        height: 28,
        color: const Color(0xAA000000),
        child: Row(
          children: [
            for (var n = 1; n <= 5; n++)
              Expanded(
                child: Semantics(
                  button: true,
                  // Five stars in a strip are five identical hit targets.
                  // Naming the action rather than the star: pressing the
                  // current rating is how it gets cleared.
                  label: stars == n
                      ? 'Clear the rating'
                      : (n == 1 ? 'Rate 1 star' : 'Rate $n stars'),
                  child: GestureDetector(
                    behavior: HitTestBehavior.opaque,
                    onTap:
                        onSet == null ? null : () => onSet!(stars == n ? 0 : n),
                    child: Icon(
                      stars >= n ? Icons.star : Icons.star_border,
                      size: 13,
                      color: stars >= n
                          ? const Color(0xFFE5A00D)
                          : const Color(0xCCFFFFFF),
                    ),
                  ),
                ),
              ),
          ],
        ),
      );
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
  Widget build(BuildContext context) => SizedBox(
        width: 44,
        height: 44,
        child: Material(
          color: Tokens.secMusic,
          shape: const CircleBorder(),
          clipBehavior: Clip.antiAlias,
          child: InkWell(
            onTap: onPressed,
            child: const Icon(Icons.play_arrow, size: 20, color: Colors.white),
          ),
        ),
      );
}

/// One row of any track list. The star, the heart and the lyrics glyph are the
/// three per-row controls the Slint `SongCard` carries; the rest of its
/// seventeen properties were hover states Flutter derives.

/// Who made a track, under the row it belongs to.
///
/// Every name is a link that searches the library for it, which is how you find
/// out the same producer is on four of your records. Splitting a credit into
/// names is a guess: a composer tag holds "Yorke, Greenwood" or
/// "Yorke/Greenwood" or one name with a comma in it, and nothing normalises
/// that -- so the whole value stays readable as written and the split only
/// decides where the tap targets are.
class _CreditsPanel extends StatelessWidget {
  const _CreditsPanel({
    required this.controller,
    required this.credits,
    required this.indent,
  });

  final MusicController controller;
  final List<MetaRow>? credits;
  final double indent;

  static final RegExp _split = RegExp(r'\s*[,;/]\s*|\s+&\s+|\s+feat\.?\s+',
      caseSensitive: false);

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final rows = credits;
    return Padding(
      padding: EdgeInsets.fromLTRB(indent, 2, 12, 8),
      child: rows == null
          ? SizedBox(
              height: 14,
              child: Align(
                alignment: Alignment.centerLeft,
                child: SizedBox(
                  width: 14,
                  height: 14,
                  child: CircularProgressIndicator(
                      strokeWidth: 1.6, color: t.nInk3),
                ),
              ),
            )
          : rows.isEmpty
              ? Text('No credits in this file.',
                  style: TextStyle(fontSize: 11.5, color: t.nInk3))
              : Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    for (final row in rows)
                      Padding(
                        padding: const EdgeInsets.only(bottom: 3),
                        child: Wrap(
                          crossAxisAlignment: WrapCrossAlignment.center,
                          spacing: 6,
                          children: [
                            Text(
                              row.label,
                              style: TextStyle(
                                fontFamily: Tokens.fontFamily,
                                fontSize: 11,
                                fontWeight: FontWeight.w700,
                                letterSpacing: 0.3,
                                color: t.nInk3,
                              ),
                            ),
                            for (final name in row.value
                                .split(_split)
                                .map((n) => n.trim())
                                .where((n) => n.isNotEmpty))
                              InkWell(
                                borderRadius: BorderRadius.circular(4),
                                onTap: () => controller
                                    .send(MusicCmd.search(query: name)),
                                child: Text(
                                  name,
                                  style: TextStyle(
                                    fontSize: 12,
                                    color: t.nInk,
                                    decoration: TextDecoration.underline,
                                    decorationColor: t.nHair,
                                  ),
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
    this.compact = false,
    this.draggable = false,
    this.selected = false,
    this.onSelect,
  });

  final MusicController controller;
  final Track track;
  final int index;
  final VoidCallback onPlay;
  final VoidCallback? onQueue;
  final VoidCallback? onRemove;
  final bool showArt;
  final bool dense;

  /// The dashboard shape: artwork, two lines, a duration, and nothing else.
  /// The full row's number column, lyric badge, heart, stars and overflow menu
  /// belong to a list you are working in, not to a rail you are glancing at.
  final bool compact;

  /// This row lives in a `ReorderableListView`, which paints its own drag
  /// handle over the trailing edge. The handle is drawn *on top* of the row
  /// rather than beside it, so without a gap reserved for it the grip sits on
  /// the duration -- two things in the same 24 pixels, and the one you can read
  /// is the one you cannot grab.
  final bool draggable;

  /// Drawn as part of a selection.
  final bool selected;

  /// Ctrl/Cmd-click adds this row to a selection; shift-click extends to it.
  /// Null in every list that has no bulk actions, which is most of them, and
  /// then a modifier-click is just a click.
  final void Function({required bool range})? onSelect;

  @override
  State<TrackRow> createState() => _TrackRowState();
}

class _TrackRowState extends State<TrackRow> {
  bool _hovered = false;

  /// Who made this track, fetched the first time the row is expanded and kept
  /// after that. `Track.hasCredits` already says whether there is anything to
  /// fetch, so a row without credits never asks.
  List<MetaRow>? _credits;
  bool _showCredits = false;

  Future<void> _toggleCredits() async {
    final open = !_showCredits;
    setState(() => _showCredits = open);
    if (!open || _credits != null) return;
    List<MetaRow> got;
    try {
      got = await musicTrackCredits(itemId: widget.track.itemId);
    } catch (_) {
      got = const [];
    }
    if (mounted) setState(() => _credits = got);
  }

  /// This row's artwork, so a copy of it can be measured and flown to the
  /// player bar when the row is played.
  final GlobalKey _artKey = GlobalKey();

  /// Play this row, and send its cover to the player on the way.
  ///
  /// Wrapped here rather than at each of the call sites that pass `onPlay`,
  /// because every list in the section funnels through this one row -- the
  /// queue, the album page, Favourites, History and the rails all get the
  /// flight from this single edit.
  void _play() {
    // A modifier turns the click into a selection rather than a play. Checked
    // here, in the one place every list's row click funnels through.
    final keys = HardwareKeyboard.instance;
    if (widget.onSelect != null) {
      if (keys.isShiftPressed) {
        widget.onSelect!(range: true);
        return;
      }
      if (keys.isControlPressed || keys.isMetaPressed) {
        widget.onSelect!(range: false);
        return;
      }
    }
    if (widget.showArt) {
      ArtFlight.toPlayer(
        context,
        fromKey: _artKey,
        art: MusicArt(
          controller: widget.controller,
          kind: 'track',
          artKey: '${widget.track.itemId}',
          direct: widget.track.art,
          size: 56,
          radius: 0,
        ),
      );
    }
    widget.onPlay();
  }

  /// The now-playing green, from `#22c55e1f` / `#22c55e66` / `#22c55e` in
  /// ui/page_music.slint. Deliberately not the section pink: the row you are
  /// hearing has to be findable in a list where pink is already the accent.
  static const Color _np = Color(0xFF22C55E);

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tr = widget.track;
    final playing = widget.controller.now?.itemId == tr.itemId &&
        (widget.controller.now?.loaded ?? false);
    final art = widget.dense ? 32.0 : 40.0;
    // The same menu the Songs grid has. Slint puts it on both — a row and a
    // tile are two drawings of one song, and the things you can do to it do
    // not change with the drawing.
    final row = SongContextMenu(
      controller: widget.controller,
      track: tr,
      onPlay: widget.onPlay,
      onQueue: widget.onQueue,
      onRemove: widget.onRemove,
      child: MouseRegion(
        onEnter: (_) => setState(() => _hovered = true),
        onExit: (_) => setState(() => _hovered = false),
        child: GestureDetector(
          onTap: _play,
          child: Container(
            // 52 dense, not 44. The queue is a list you drag rows around in,
            // and a 44px row with a 32px thumbnail in it leaves six pixels of
            // margin to grab.
            height: widget.dense ? 52 : 56,
            padding: EdgeInsets.only(
              left: widget.compact ? 10 : 12,
              right: widget.compact ? 16 : 12,
            ),
            // A skin presses the row you are hearing (or have picked) into its
            // material; the rest stay flat on it.
            decoration: (widget.selected || playing
                    ? context.skin.control(active: true, radius: 12)
                    : null) ??
                BoxDecoration(
              borderRadius: BorderRadius.circular(8),
              // Selection reads over now-playing: a row can be both, and while
              // you are picking rows the selection is what you are looking at.
              color: widget.selected
                  ? Tokens.secMusic.withValues(alpha: 0.18)
                  : playing
                      ? _np.withValues(alpha: 0.12)
                      : (_hovered ? t.nHover : Colors.transparent),
              border: widget.selected
                  ? Border.all(color: Tokens.secMusic.withValues(alpha: 0.55))
                  : playing
                      ? Border.all(color: _np.withValues(alpha: 0.4))
                      : null,
            ),
            child: Row(
              children: [
                if (!widget.compact)
                  SizedBox(
                    width: 28,
                    child: playing
                        ? Icon(context.skin.icon(Icons.equalizer),
                            size: 16, color: _np)
                        : Text(
                            '${widget.index + 1}',
                            textAlign: TextAlign.center,
                            style: TextStyle(fontSize: 12, color: t.nInk2),
                          ),
                  ),
                if (widget.showArt) ...[
                  // Playing beats hover: the marker stays while the pointer is
                  // over the row you are already hearing, so it never flickers.
                  Stack(
                    children: [
                      KeyedSubtree(
                        key: _artKey,
                        child: MusicArt(
                          controller: widget.controller,
                          kind: 'track',
                          artKey: '${tr.itemId}',
                          direct: tr.art,
                          size: art,
                          radius: 5,
                        ),
                      ),
                      if (playing || _hovered)
                        Positioned.fill(
                          child: ClipRRect(
                            borderRadius: BorderRadius.circular(5),
                            child: ColoredBox(
                              color: Color(playing ? 0xAA000000 : 0x88000000),
                              child: Icon(
                                context.skin.icon(
                                    playing ? Icons.equalizer : Icons.play_arrow),
                                size: 15,
                                color: playing ? _np : Colors.white,
                              ),
                            ),
                          ),
                        ),
                    ],
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
                          fontWeight: FontWeight.w600,
                          color: playing ? _np : t.nInk,
                        ),
                      ),
                      const SizedBox(height: 2),
                      if (tr.artist.isNotEmpty || tr.album.isNotEmpty)
                        Text(
                          widget.compact
                              ? tr.artist
                              : [tr.artist, tr.album]
                                  .where((s) => s.isNotEmpty)
                                  .join(' · '),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 11, color: t.nInk2),
                        ),
                    ],
                  ),
                ),
                // None of this in the queue. A 432px panel has to spend its
                // width on the two things you are reading — the song and who
                // made it — and the marks belong to a list you are working in.
                // They are all still on the right-click menu.
                if (!widget.compact && !widget.dense) ...[
                  if (tr.lyrics.isNotEmpty)
                    Tooltip(
                      message: tr.lyrics == 'synced'
                          ? 'Synced lyrics'
                          : 'Lyrics stored',
                      child: Icon(
                        context.skin.icon(Icons.lyrics_outlined),
                        size: 15,
                        color:
                            tr.lyrics == 'synced' ? Tokens.secMusic : t.nInk2,
                      ),
                    ),
                  // Only when the file actually carries credit tags, so the
                  // chevron is a promise rather than a coin toss.
                  if (tr.hasCredits)
                    IconButton(
                      iconSize: 16,
                      visualDensity: VisualDensity.compact,
                      tooltip: _showCredits ? 'Hide credits' : 'Credits',
                      icon: Icon(
                        context.skin.icon(_showCredits
                            ? Icons.expand_less
                            : Icons.expand_more),
                        color: _showCredits ? Tokens.secMusic : t.nInk2,
                      ),
                      onPressed: _toggleCredits,
                    ),
                  const SizedBox(width: 8),
                  IconButton(
                    iconSize: 17,
                    visualDensity: VisualDensity.compact,
                    tooltip: tr.loved ? 'Remove from favourites' : 'Favourite',
                    icon: Icon(
                      context.skin.icon(
                          tr.loved ? Icons.favorite : Icons.favorite_border),
                      color: tr.loved ? Tokens.secMusic : t.nInk2,
                    ),
                    onPressed: () => widget.controller
                        .send(MusicCmd.love(itemId: tr.itemId)),
                  ),
                  _Stars(
                    stars: tr.stars,
                    onSet: (n) => widget.controller
                        .send(MusicCmd.rate(itemId: tr.itemId, stars: n)),
                  ),
                  const SizedBox(width: 8),
                ],
                SizedBox(
                  width: 48,
                  child: Text(
                    fmtClock(tr.durationS),
                    textAlign: TextAlign.right,
                    style: TextStyle(fontSize: 12, color: t.nInk2),
                  ),
                ),
                if (widget.draggable) const SizedBox(width: 34),
                // Not in the queue: the panel is 432px, this menu duplicates
                // the right-click one exactly, and a column of three-dot
                // buttons down the side of a queue is the widest thing in it
                // that does the least.
                if (!widget.compact && !widget.dense)
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
                      const PopupMenuItem(
                          value: 'tags', child: Text('Edit tags…')),
                      if (widget.onRemove != null)
                        const PopupMenuItem(
                            value: 'remove',
                            child: Text('Remove from this list')),
                      const PopupMenuItem(
                          value: 'delete', child: Text('Delete from disk…')),
                    ],
                  ),
              ],
            ),
          ),
        ),
      ),
    );

    // A sibling under the row rather than anything inside it: every list in
    // the section draws this row, and none of them fixes its height -- the
    // separators between rows are SizedBoxes, not extents -- so growing
    // downward is free. Changing the row's own layout would not have been.
    if (!_showCredits) return row;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        row,
        _CreditsPanel(
          controller: widget.controller,
          credits: _credits,
          indent: widget.showArt ? 68.0 : 12.0,
        ),
      ],
    );
  }

  /// The three-dot menu's actions. The same list the right-click menu offers,
  /// reached from the button on the row itself.
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
              context.skin.icon(i <= stars ? Icons.star : Icons.star_border),
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
    this.compact = false,
  });

  final int page;
  final int pages;
  final void Function(int) onGo;

  /// Docked in a header row rather than centred under a list — no vertical
  /// padding, and 28px discs instead of full IconButtons.
  final bool compact;

  @override
  Widget build(BuildContext context) {
    if (pages <= 1) return const SizedBox.shrink();
    final t = context.tokens;
    if (compact) {
      // Filled, not a chip: a pager sitting at the end of a row of coloured
      // action pills in the section's own accent was two grey discs nobody
      // found. The disabled end of the range keeps the chip grey, so the pager
      // still says which way it can go.
      final skin = context.skin;
      Widget btn(IconData icon, int to, bool on) => !skin.isStandard
          ? SkinButton(
              width: 28,
              height: 28,
              radius: 14,
              onTap: on ? () => onGo(to) : null,
              child: Icon(skin.icon(icon),
                  size: 15, color: on ? skin.accent : skin.inkDim),
            )
          : SizedBox(
            width: 28,
            height: 28,
            child: Material(
              color: on ? Tokens.secMusic : t.nChip,
              shape: const CircleBorder(),
              clipBehavior: Clip.antiAlias,
              child: InkWell(
                onTap: on ? () => onGo(to) : null,
                child: Icon(icon, size: 15, color: on ? Colors.white : t.nInk3),
              ),
            ),
          );
      return Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          btn(Icons.chevron_left, page - 1, page > 0),
          const SizedBox(width: 8),
          Text(
            '${page + 1} / $pages',
            style: TextStyle(
              fontFamily: skin.fontFamily ?? Tokens.fontFamily,
              fontSize: 12,
              fontWeight: FontWeight.w700,
              color: t.nInk,
            ),
          ),
          const SizedBox(width: 8),
          btn(Icons.chevron_right, page + 1, page + 1 < pages),
        ],
      );
    }
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 16),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          IconButton(
            tooltip: 'Previous page',
            icon: const Icon(Icons.chevron_left),
            color: page > 0 ? Tokens.secMusic : t.nInk3,
            onPressed: page > 0 ? () => onGo(page - 1) : null,
          ),
          Semantics(
            // Two chevrons and a fraction. Read out bare it is "1 / 12", which
            // is not a sentence — this is the one place in the section where a
            // number needs saying rather than showing.
            label: 'Page ${page + 1} of $pages',
            excludeSemantics: true,
            child: Text('${page + 1} / $pages',
                style: TextStyle(fontSize: 13, color: t.nInk)),
          ),
          IconButton(
            tooltip: 'Next page',
            icon: const Icon(Icons.chevron_right),
            color: page + 1 < pages ? Tokens.secMusic : t.nInk3,
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
            FilledButton(
              style: musicFilledStyle(),
              onPressed: action!.$2,
              child: Text(action!.$1),
            ),
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
    this.connect = false,
  });

  final String title;
  final double height;
  final List<Widget> children;
  final (String, VoidCallback)? action;

  /// Slint's `yt-home-connect`: a gradient hairline under the rail's heading,
  /// tying the row to the section's colour. Off everywhere else, because only
  /// the YouTube Home rails carry it and only when the user leaves it on.
  final bool connect;

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
        if (connect)
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 0, 24, 10),
            child: Container(
              height: 2,
              decoration: const BoxDecoration(
                gradient: LinearGradient(
                  colors: [Color(0xFFF43F5E), Color(0x00F97316)],
                ),
              ),
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

/// The edge-to-edge grid every browse page in ui/page_music.slint uses.
///
/// The cell is the unit, not the tile: `cols = floor(width / target)` clamped,
/// `cell = width / cols`, and the tile is the cell less its inset. That is why
/// the grids hold their rhythm as the window moves — the tile size drifts
/// within a column count instead of the column count jumping about. A
/// SliverGrid with a max extent gives the opposite behaviour, which is what
/// made the port's pages feel unrelated to the Slint ones.
class MusicGrid extends StatelessWidget {
  const MusicGrid({
    super.key,
    required this.count,
    required this.builder,
    this.target = 150,
    this.maxCols = 7,
    this.minCols = 1,
    this.inset = 8,
    this.labelHeight = kCardLabelTwo,
    this.colChoices,
  });

  final int count;
  final Widget Function(BuildContext, int) builder;

  /// The cell width to aim for. Slint drives this off the density slider on
  /// the songs grid and hard-codes 150 (or 170) everywhere else.
  final double target;
  final int maxCols;
  final int minCols;
  final double inset;

  /// What the row keeps for the label under each tile. Defaults to two lines,
  /// because nearly every grid in the section draws a subtitle; pass
  /// [kCardLabelOne] for the ones that do not.
  final double labelHeight;

  /// The only column counts this grid is allowed to use, ascending.
  ///
  /// For a server-paged grid the count is not free: a page of 32 laid out
  /// seven across ends in a row of four, which reads as the list running out
  /// rather than the page ending. Pass the divisors of the page size and the
  /// width picks the nearest of those instead of any number that fits. Null —
  /// every other grid in the section — means whatever the width says.
  final List<int>? colChoices;

  @override
  Widget build(BuildContext context) {
    if (count == 0) return const SizedBox.shrink();
    return LayoutBuilder(
      builder: (context, box) {
        final fits = (box.maxWidth / target).floor().clamp(minCols, maxCols);
        final choices = colChoices;
        // Nearest by *cell size*, not by column count.
        //
        // Counting columns gets this wrong in the middle. At 900px with a
        // 150px target, six columns fit — and six is equally far from four and
        // from eight, so a tie-break has to choose. By count, four wins and
        // the tiles come out 225px: half again the size asked for. By size,
        // eight wins at 112px, which is 38px off the target rather than 75.
        // `target` is a cell width, so the distance that matters is a cell
        // width.
        final cols = choices == null || choices.isEmpty
            ? fits
            : choices.reduce((a, b) =>
                (box.maxWidth / a - target).abs() <=
                        (box.maxWidth / b - target).abs()
                    ? a
                    : b);
        final cell = box.maxWidth / cols;
        final rows = (count / cols).ceil();
        return Column(
          children: [
            for (var r = 0; r < rows; r++)
              SizedBox(
                height: cell + labelHeight,
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    for (var c = 0; c < cols; c++)
                      SizedBox(
                        width: cell,
                        child: r * cols + c < count
                            ? Padding(
                                padding:
                                    EdgeInsets.symmetric(horizontal: inset),
                                child: builder(context, r * cols + c),
                              )
                            : null,
                      ),
                  ],
                ),
              ),
          ],
        );
      },
    );
  }
}

/// The 40px action pill above the Playlists and Folders grids.
class BrowseChip extends StatefulWidget {
  const BrowseChip({
    super.key,
    required this.label,
    required this.onTap,
    this.icon,
    this.dot = false,
  });

  final String label;
  final VoidCallback onTap;
  final IconData? icon;

  /// A filled accent dot ahead of the label — Slint's `on`.
  final bool dot;

  @override
  State<BrowseChip> createState() => _BrowseChipState();
}

class _BrowseChipState extends State<BrowseChip> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: Container(
          height: 40,
          padding: const EdgeInsets.symmetric(horizontal: 18),
          decoration: context.skin.control(
                  active: widget.dot, hovered: _hovered, radius: 20) ??
              BoxDecoration(
            color: _hovered ? t.nHover : t.nChip,
            borderRadius: BorderRadius.circular(20),
            border: Border.all(color: t.nHair),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (widget.dot) ...[
                Container(
                  width: 8,
                  height: 8,
                  decoration: const BoxDecoration(
                    shape: BoxShape.circle,
                    color: Tokens.secMusic,
                  ),
                ),
                const SizedBox(width: 8),
              ],
              if (widget.icon != null) ...[
                Icon(context.skin.icon(widget.icon!),
                    size: 14, color: t.nInk2),
                const SizedBox(width: 8),
              ],
              Text(
                widget.label,
                style: TextStyle(
                  fontFamily: context.skin.fontFamily ?? Tokens.fontFamily,
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                  color: t.nInk2,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A 28px sort pill. `dir` is Slint's: 0 none, 1 ascending, 2 descending, and
/// the arrow is what tells you a second click on the active chip flips it.
/// One sort control, shaped like the pills beside it.
///
/// Every list in My Music sorts, and each one had grown its own control — a
/// popup on Songs, a pair of chips on the browse grids. One shape, so row 2
/// reads the same wherever you are: the field you are sorting by, an arrow for
/// which way, and picking the field you are already on flips it.
class SortMenu extends StatelessWidget {
  const SortMenu({
    super.key,
    required this.modes,
    required this.mode,
    required this.dir,
    required this.onPick,
  });

  final Map<String, String> modes;
  final String mode;

  /// asc | desc
  final String dir;
  final void Function(String) onPick;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final up = dir != 'desc';
    return PopupMenuButton<String>(
      tooltip: 'Sort',
      position: PopupMenuPosition.under,
      onSelected: onPick,
      itemBuilder: (_) => [
        for (final e in modes.entries)
          PopupMenuItem(
            value: e.key,
            height: 34,
            child: Row(
              children: [
                if (e.key == mode)
                  Icon(up ? Icons.arrow_upward : Icons.arrow_downward,
                      size: 14, color: Tokens.secMusic)
                else
                  const SizedBox(width: 14),
                const SizedBox(width: 8),
                Text(e.value, style: const TextStyle(fontSize: 13)),
              ],
            ),
          ),
      ],
      child: Container(
        height: 38,
        padding: const EdgeInsets.only(left: 14, right: 12),
        decoration: context.skin.control(active: false, radius: 19) ??
            BoxDecoration(
          color: t.nChip,
          borderRadius: BorderRadius.circular(19),
          border: Border.all(color: t.nHair),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(context.skin.icon(Icons.sort), size: 15, color: t.nInk2),
            const SizedBox(width: 8),
            Text(
              modes[mode] ?? mode,
              style: TextStyle(
                fontFamily: context.skin.fontFamily ?? Tokens.fontFamily,
                fontSize: 13,
                fontWeight: FontWeight.w600,
                color: t.nInk2,
              ),
            ),
            const SizedBox(width: 6),
            Icon(
                context.skin
                    .icon(up ? Icons.arrow_upward : Icons.arrow_downward),
                size: 13,
                color: context.skin.accent ?? Tokens.secMusic),
          ],
        ),
      ),
    );
  }
}

class SortChip extends StatelessWidget {
  const SortChip({
    super.key,
    required this.label,
    required this.active,
    required this.onTap,
    this.dir = 0,
    this.icon,
  });

  final String label;
  final bool active;
  final VoidCallback onTap;
  final int dir;
  final IconData? icon;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final skin = context.skin;
    final fg = active
        ? (skin.accent ?? Tokens.secMusic)
        : (skin.inkDim ?? t.nInk2);
    return GestureDetector(
      onTap: onTap,
      child: Container(
        height: 28,
        padding: const EdgeInsets.symmetric(horizontal: 12),
        decoration: skin.control(active: active, radius: 14) ??
            BoxDecoration(
          color: active
              ? Tokens.secMusic.withValues(alpha: 0.15)
              : Colors.transparent,
          borderRadius: BorderRadius.circular(14),
          border: Border.all(
            color: active ? Tokens.secMusic.withValues(alpha: 0.4) : t.nHair,
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (icon != null) ...[
              Icon(icon, size: 13, color: fg),
              const SizedBox(width: 6),
            ],
            Text(
              label,
              style: TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: 12,
                fontWeight: active ? FontWeight.w700 : FontWeight.w500,
                color: fg,
              ),
            ),
            if (dir > 0) ...[
              const SizedBox(width: 6),
              Icon(
                dir == 1 ? Icons.keyboard_arrow_up : Icons.keyboard_arrow_down,
                size: 12,
                color: fg,
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// The 38px pill in a detail page's action row.
class DetailActionBtn extends StatefulWidget {
  const DetailActionBtn({
    super.key,
    this.icon,
    required this.label,
    required this.onTap,
    this.tint,
  });

  /// Optional: a row of these inside a list wants the words, not a wall of
  /// glyphs.
  final IconData? icon;
  final String label;
  final VoidCallback onTap;

  /// Paint it. A row of five identical grey pills gives the eye nothing to
  /// aim at; Slint colours the ones that *do* something to the whole page —
  /// Play all, Shuffle, the manager — and leaves the rest neutral.
  final Color? tint;

  @override
  State<DetailActionBtn> createState() => _DetailActionBtnState();
}

class _DetailActionBtnState extends State<DetailActionBtn> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = widget.tint;
    final ink = tint == null
        ? t.nInk2
        : (t.dark
            ? Color.lerp(Colors.white, tint, 0.55)!
            : Color.lerp(Colors.black, tint, 0.72)!);
    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: Container(
          height: 38,
          padding: const EdgeInsets.only(left: 15, right: 16),
          // A skin raises the pill out of its material; the tint stays on the
          // label and glyph, so Play all is still green and Clear still red.
          decoration: context.skin
                  .control(active: false, hovered: _hovered, radius: 19) ??
              BoxDecoration(
            color: tint == null
                ? (_hovered ? t.nHover : t.nChip)
                : tint.withValues(
                    alpha: _hovered
                        ? (t.dark ? 0.34 : 0.36)
                        : (t.dark ? 0.18 : 0.22),
                  ),
            borderRadius: BorderRadius.circular(19),
            border: Border.all(
              color: tint == null
                  ? t.nHair
                  : tint.withValues(alpha: t.dark ? 0.55 : 0.85),
            ),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (widget.icon != null) ...[
                Icon(context.skin.icon(widget.icon!),
                    size: 15,
                    color: tint == null
                        ? (_hovered ? Tokens.secMusic : t.nInk2)
                        : ink),
                const SizedBox(width: 8),
              ],
              Text(
                widget.label,
                style: TextStyle(
                  fontFamily: context.skin.fontFamily ?? Tokens.fontFamily,
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                  color: ink,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// The gradient tile Playlists and Folders use in place of artwork. Neither
/// has a cover of its own by nature, and a grid of grey squares with a glyph
/// in the middle is what those pages look like without this.
class TileCard extends StatefulWidget {
  const TileCard({
    super.key,
    required this.label,
    required this.icon,
    required this.onTap,
    this.art,
    this.controller,
    this.artKind = '',
    this.artKey = '',
    this.onMenu,
    this.hint,
    this.footer,
    this.strong = true,
  });

  final String label;
  final IconData icon;
  final VoidCallback onTap;
  final String? art;
  final MusicController? controller;
  final String artKind;
  final String artKey;
  final VoidCallback? onMenu;

  /// Small white line revealed over the tile on hover.
  final String? hint;

  /// Pinned to the bottom-left of the tile — the folder pages' section chip.
  final Widget? footer;

  /// Playlists get the full pink-to-violet; folders get it at a third, so a
  /// wall of folders does not read as a wall of playlists.
  final bool strong;

  @override
  State<TileCard> createState() => _TileCardState();
}

class _TileCardState extends State<TileCard> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final w = widget;
    // A playlist and a folder have no cover of their own, so they borrow one —
    // a playlist wears the newest thing in it. Asking the resolver here rather
    // than only when a path is already known is what starts that lookup; a
    // miss leaves the gradient tile alone instead of painting a grey box over
    // it, which is what handing an unresolved key to MusicArt would do.
    final borrowed = (w.art ?? '').isNotEmpty
        ? w.art
        : (w.controller != null && w.artKind.isNotEmpty
            ? w.controller!.artFor(w.artKind, w.artKey)
            : null);
    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: w.onTap,
        onSecondaryTap: w.onMenu,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.center,
          mainAxisSize: MainAxisSize.min,
          children: [
            Flexible(
              child: AspectRatio(
                aspectRatio: 1,
                child: Container(
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(12),
                    gradient: LinearGradient(
                      begin: Alignment.topLeft,
                      end: Alignment.bottomRight,
                      colors: w.strong
                          ? const [Color(0xFFEC4899), Color(0xFF7C3AED)]
                          : const [Color(0x55EC4899), Color(0x557C3AED)],
                    ),
                    border: _hovered
                        ? Border.all(color: Tokens.secMusic, width: 2)
                        : null,
                    boxShadow: _hovered
                        ? const [
                            BoxShadow(color: Color(0xAAEC4899), blurRadius: 26)
                          ]
                        : null,
                  ),
                  clipBehavior: Clip.antiAlias,
                  child: Stack(
                    fit: StackFit.expand,
                    children: [
                      if (borrowed != null && borrowed.isNotEmpty)
                        MusicArt(
                          controller: w.controller!,
                          kind: w.artKind,
                          artKey: w.artKey,
                          direct: borrowed,
                          size: 400,
                          radius: 0,
                          fallback: w.icon,
                        )
                      else
                        Center(
                          child: Icon(w.icon,
                              size: 40, color: const Color(0xDDFFFFFF)),
                        ),
                      if (_hovered)
                        ColoredBox(
                          color: const Color(0x44000000),
                          child: w.hint == null
                              ? const Align(
                                  alignment: Alignment(0.62, 0.62),
                                  child: _PlayFab2(),
                                )
                              : Center(
                                  child: Padding(
                                    padding: const EdgeInsets.symmetric(
                                        horizontal: 8),
                                    child: Text(
                                      w.hint!,
                                      textAlign: TextAlign.center,
                                      style: const TextStyle(
                                        fontFamily: Tokens.fontFamily,
                                        fontSize: 9,
                                        color: Colors.white,
                                      ),
                                    ),
                                  ),
                                ),
                        ),
                      if (w.footer != null)
                        Positioned(left: 6, bottom: 6, child: w.footer!),
                    ],
                  ),
                ),
              ),
            ),
            const SizedBox(height: 8),
            Text(
              w.label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              textAlign: TextAlign.center,
              style: TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: 13,
                fontWeight: FontWeight.w600,
                color: t.nInk,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _PlayFab2 extends StatelessWidget {
  const _PlayFab2();

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      width: 38,
      height: 38,
      alignment: Alignment.center,
      decoration: BoxDecoration(shape: BoxShape.circle, color: t.nInk),
      child: Icon(Icons.play_arrow, size: 16, color: t.nCard),
    );
  }
}
