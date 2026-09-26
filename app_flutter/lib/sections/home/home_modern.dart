// What the four newer Home layouts share — Media, Play, Calm and Today.
//
// docs/mockups/NewSections/home-media-play-deck.html, home-calm-deck.html and
// home-today-deck.html. They read the same `HomeState` as the first four and
// open things through the same helpers in home_shared.dart; this file is the
// handful of pieces those four draw and the older ones do not.

import 'dart:io';

import 'package:flutter/material.dart';

import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/arcade.dart';
import '../../src/rust/api/home.dart';
import '../../src/rust/api/music.dart';
import '../music/music_controller.dart';
import '../music/mini_player.dart' show MiniLyrics;
import '../music/music_widgets.dart';
import '../music/player_widgets.dart';
import 'home_player.dart' show openPlayingTab;
import 'home_shared.dart';

// ── the hour ────────────────────────────────────────────────────────────────

enum DayPart { morning, day, evening, night }

DayPart dayPart([DateTime? at]) {
  final h = (at ?? DateTime.now()).hour;
  if (h < 5) return DayPart.night;
  if (h < 12) return DayPart.morning;
  if (h < 17) return DayPart.day;
  if (h < 22) return DayPart.evening;
  return DayPart.night;
}

String dayPartName(DayPart p) => switch (p) {
      DayPart.morning => 'Morning',
      DayPart.day => 'Afternoon',
      DayPart.evening => 'Evening',
      DayPart.night => 'Night',
    };

/// The name out of the bridge's greeting ("Good Evening, Asha"), or '' for the
/// "there" it falls back to when no name is set.
String firstName(HomeState st) {
  final i = st.greeting.indexOf(', ');
  final n = i < 0 ? '' : st.greeting.substring(i + 2).trim();
  return n == 'there' ? '' : n;
}

// ── what is on ──────────────────────────────────────────────────────────────

/// Whether the section is in the sidebar: a door to a section that is off
/// opens nothing.
bool sectionOn(Section s) => ShellController.instance.sections.contains(s);

/// Whether Settings › You & Home left this card on.
bool cardOn(HomeState st, String key) => st.cards.contains(key);

String count(int n) {
  final s = '$n';
  final b = StringBuffer();
  for (var i = 0; i < s.length; i++) {
    if (i > 0 && (s.length - i) % 3 == 0) b.write(',');
    b.write(s[i]);
  }
  return b.toString();
}

String plural(int n, String one, [String? many]) =>
    '${count(n)} ${n == 1 ? one : (many ?? '${one}s')}';

/// One soft line of what is in a section, from the counts Home already has.
/// Empty where the snapshot has nothing to say about it.
String sectionLine(Section s, HomeState st) {
  final c = st.counts;
  return switch (s) {
    Section.photos => c.photos > 0 ? plural(c.photos, 'photo') : '',
    Section.videos => c.videos > 0
        ? '${plural(c.videos, 'video')}'
            '${c.videosShows > 0 ? ' · ${plural(c.videosShows, 'show')}' : ''}'
        : '',
    Section.music => c.songs > 0 ? plural(c.songs, 'song') : '',
    Section.books => c.booksReading > 0
        ? '${count(c.booksReading)} in progress'
        : c.books > 0
            ? plural(c.books, 'book')
            : '',
    Section.cloud => c.cloudRemotes > 0 ? plural(c.cloudRemotes, 'remote') : '',
    Section.tools => c.toolsRunning > 0
        ? '${count(c.toolsRunning)} running'
        : 'Convert, compress, rename',
    Section.finances =>
      c.financesDue > 0 ? '${count(c.financesDue)} due soon' : st.finSpent,
    _ => '',
  };
}

// ── opening things ──────────────────────────────────────────────────────────

/// Resume a Continue row. A video goes to its own page, which the shared
/// `continueOpen` cannot do yet; everything else goes through it.
void resumeRow(HomeContinue r) {
  if (r.kind == 'video' && r.id >= 0) {
    ShellController.instance.goOpen(Section.videos, 'item', '${r.id}');
  } else {
    continueOpen(r);
  }
}

/// The hero is the first Continue row spelled out; opening it is the same.
void resumeHero(HomeHero h) => resumeRow(HomeContinue(
      kind: h.kind,
      title: h.title,
      author: '',
      sub: h.meta,
      frac: h.frac,
      id: h.id,
      path: h.path,
    ));

/// The picture for a hero, the same way `continueArt` finds one for a row.
({String kind, String key})? heroArt(HomeHero h) => switch (h.kind) {
      'podcast' when h.path.isNotEmpty => (kind: 'podcast', key: h.path),
      'audiobook' when h.path.isNotEmpty => (kind: 'book', key: h.path),
      _ => null,
    };

void openVideo(int id) =>
    ShellController.instance.goOpen(Section.videos, 'item', '$id');

void openBook(int id) =>
    ShellController.instance.goOpen(Section.books, 'detail', '$id');

void playGame(GameTile g) =>
    arcadeDispatch(cmd: ArcadeCmd.play(key: g.key)).ignore();

// ── small pieces ────────────────────────────────────────────────────────────

/// A thin progress line, or nothing when the fraction is unknown.
class ThinBar extends StatelessWidget {
  const ThinBar(
      {super.key, required this.frac, required this.color, this.track});

  final double frac;
  final Color color;
  final Color? track;

  @override
  Widget build(BuildContext context) {
    if (frac < 0) return const SizedBox.shrink();
    return ClipRRect(
      borderRadius: BorderRadius.circular(4),
      child: LinearProgressIndicator(
        value: frac.clamp(0.0, 1.0),
        minHeight: 4,
        color: color,
        backgroundColor: track ?? context.tokens.outlineStrong,
      ),
    );
  }
}

/// A Continue row's cover, through the owning section's resolver.
class RowCover extends StatelessWidget {
  const RowCover({super.key, required this.row, this.iconSize = 24});

  final HomeContinue row;
  final double iconSize;

  @override
  Widget build(BuildContext context) => LazyCover(
        section: kindSection(row.kind),
        id: row.id,
        art: continueArt(row),
        tint: kindColor(row.kind),
        icon: kindIcon(row.kind),
        iconSize: iconSize,
      );
}

/// A game's cover, or its title on colours drawn from the title when there is
/// none — the same fallback the Arcade shelf uses.
class GameCover extends StatelessWidget {
  const GameCover({super.key, required this.game, this.big = false});

  final GameTile game;
  final bool big;

  @override
  Widget build(BuildContext context) {
    final hue = (game.title.hashCode % 360).abs().toDouble();
    final fallback = Container(
      decoration: BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [
            HSLColor.fromAHSL(1, hue, 0.45, 0.28).toColor(),
            HSLColor.fromAHSL(1, (hue + 50) % 360, 0.55, 0.45).toColor(),
          ],
        ),
      ),
      alignment: Alignment.bottomLeft,
      padding: const EdgeInsets.all(10),
      child: Text(game.title,
          maxLines: 3,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(
              fontSize: big ? 20 : 13,
              fontWeight: FontWeight.w800,
              color: Colors.white,
              height: 1.15)),
    );
    return game.cover.isEmpty
        ? fallback
        : Image.file(File(game.cover),
            fit: BoxFit.cover,
            cacheWidth: big ? 600 : 300,
            errorBuilder: (_, __, ___) => fallback);
  }
}

/// A clickable surface with a hover lift, for the cards these layouts are
/// made of.
class Tap extends StatelessWidget {
  const Tap({super.key, required this.onTap, required this.child});

  final VoidCallback? onTap;
  final Widget child;

  @override
  Widget build(BuildContext context) => MouseRegion(
        cursor: onTap == null ? MouseCursor.defer : SystemMouseCursors.click,
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: onTap,
          child: child,
        ),
      );
}

/// A pill button: solid for the thing to do, glass for the others.
///
/// Under a design language the language draws it — the solid one as its
/// prominent control, the others as plain ones — and its ink follows.
/// [fill] and [ink] are the Standard look only.
class PillBtn extends StatelessWidget {
  const PillBtn({
    super.key,
    required this.label,
    required this.onTap,
    this.icon,
    this.solid = true,
    this.fill,
    this.ink,
  });

  final String label;
  final IconData? icon;
  final VoidCallback onTap;
  final bool solid;
  final Color? fill;
  final Color? ink;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final skin = context.skin;
    return Hover(
      onTap: onTap,
      builder: (context, hov) {
        final skinned = skin.control(
          active: false,
          hovered: hov,
          prominent: solid,
          radius: 12,
          tint: fill,
        );
        final bg = fill ??
            (solid ? Colors.white : Colors.white.withValues(alpha: 0.14));
        final fg = skinned == null
            ? (ink ?? (solid ? const Color(0xFF111111) : Colors.white))
            : solid
                ? (skin.onProminent ?? skin.accent ?? t.text)
                : t.text;
        return AnimatedContainer(
          duration: const Duration(milliseconds: 120),
          height: 38,
          padding: const EdgeInsets.symmetric(horizontal: 16),
          decoration: skinned ??
              BoxDecoration(
                color:
                    hov ? Color.alphaBlend(fg.withValues(alpha: 0.08), bg) : bg,
                borderRadius: BorderRadius.circular(12),
              ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (icon != null) ...[
                Icon(skin.icon(icon!), size: 17, color: fg),
                const SizedBox(width: 7),
              ],
              Text(label,
                  style: TextStyle(
                      fontSize: 13, fontWeight: FontWeight.w700, color: fg)),
            ],
          ),
        );
      },
    );
  }
}

/// A section heading with an optional link on the right.
class ModernHead extends StatelessWidget {
  const ModernHead(this.title, {super.key, this.note, this.link, this.onLink});

  final String title;
  final String? note;
  final String? link;
  final VoidCallback? onLink;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(bottom: 12),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.baseline,
        textBaseline: TextBaseline.alphabetic,
        children: [
          Text(title,
              style: TextStyle(
                  fontSize: 16, fontWeight: FontWeight.w700, color: t.text)),
          if (note != null) ...[
            const SizedBox(width: 10),
            Flexible(
              child: Text(note!.toUpperCase(),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 10.5,
                      fontWeight: FontWeight.w700,
                      letterSpacing: 1.2,
                      color: t.textDim)),
            ),
          ],
          const Spacer(),
          if (link != null)
            Tap(
              onTap: onLink,
              child: Text(link!,
                  style: TextStyle(
                      fontSize: 12,
                      fontWeight: FontWeight.w600,
                      color: t.textDim)),
            ),
        ],
      ),
    );
  }
}

/// A plain card on the panel colour.
class PanelCard extends StatelessWidget {
  const PanelCard(
      {super.key, required this.child, this.padding = 16, this.onTap});

  final Widget child;
  final double padding;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final box = Container(
      padding: EdgeInsets.all(padding),
      decoration: context.skin.surface(SurfaceRole.card, radius: 18) ??
          BoxDecoration(
            color: t.panel,
            borderRadius: BorderRadius.circular(18),
            border: Border.all(color: t.outline),
          ),
      child: child,
    );
    return onTap == null ? box : Tap(onTap: onTap, child: box);
  }
}

/// A horizontal rail that scrolls, with a fixed item height.
class ModernRail extends StatelessWidget {
  const ModernRail({
    super.key,
    required this.height,
    required this.children,
    this.gap = 14,
  });

  final double height;
  final List<Widget> children;
  final double gap;

  @override
  Widget build(BuildContext context) => SizedBox(
        height: height,
        child: ListView.separated(
          scrollDirection: Axis.horizontal,
          itemCount: children.length,
          separatorBuilder: (_, __) => SizedBox(width: gap),
          itemBuilder: (_, i) => children[i],
        ),
      );
}

/// What is playing, with the three transport buttons, following the app-wide
/// player. With nothing on it offers Music rather than an empty card.
class NowPlayingCard extends StatefulWidget {
  const NowPlayingCard({super.key, this.art = 86});

  final double art;

  @override
  State<NowPlayingCard> createState() => _NowPlayingCardState();
}

class _NowPlayingCardState extends State<NowPlayingCard> {
  /// The card flipped to lyrics, as the mini's square flips.
  bool _lyrics = false;

  @override
  Widget build(BuildContext context) {
    final art = widget.art;
    final t = context.tokens;
    final c = MusicController.instance;
    return AnimatedBuilder(
      animation: Listenable.merge([c, c.ticks]),
      builder: (context, _) {
        final now = c.now;
        final on = now != null && now.loaded && now.title.isNotEmpty;
        if (!on) {
          return Row(
            children: [
              Container(
                width: art,
                height: art,
                decoration: BoxDecoration(
                  color: Tokens.secMusic.withValues(alpha: 0.14),
                  borderRadius: BorderRadius.circular(14),
                ),
                child: Icon(context.skin.icon(Icons.music_note),
                    size: art * 0.36, color: Tokens.secMusic),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text('Nothing playing',
                        style: TextStyle(
                            fontSize: 15,
                            fontWeight: FontWeight.w700,
                            color: t.text)),
                    const SizedBox(height: 4),
                    Text('Pick something in Music, or start a radio.',
                        style: TextStyle(fontSize: 12, color: t.textDim)),
                    const SizedBox(height: 10),
                    Tap(
                      onTap: () => c.randomRadio(),
                      child: const Text('Random radio',
                          style: TextStyle(
                              fontSize: 12.5,
                              fontWeight: FontWeight.w700,
                              color: Tokens.secMusic)),
                    ),
                  ],
                ),
              ),
            ],
          );
        }
        // A small mini player, in the mini's bar style: the cover and the
        // lines with the way to the zen player, the seek pill, and the
        // transport beside the volume. A live stream has nowhere to seek.
        final accent = c.accent;
        final live = now.mode == 'radio' || c.tickDur <= 0;
        return Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Tap(
                  onTap: () => openPlayingTab(c),
                  child: ClipRRect(
                    borderRadius: BorderRadius.circular(12),
                    child: SizedBox(
                      width: art,
                      height: art,
                      child: MusicArt(
                        controller: c,
                        kind: 'track',
                        artKey: '${now.itemId}',
                        direct: now.art,
                        size: art,
                        radius: 0,
                      ),
                    ),
                  ),
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(
                          now.streamTitle.isNotEmpty
                              ? now.streamTitle
                              : now.title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 14.5,
                              fontWeight: FontWeight.w700,
                              color: t.text)),
                      const SizedBox(height: 2),
                      Text(
                          [now.artist, now.album]
                              .where((s) => s.isNotEmpty)
                              .join(' · '),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 12, color: t.textDim)),
                    ],
                  ),
                ),
                if (now.itemId != 0)
                  _FillBtn(
                    icon: Icons.lyrics_outlined,
                    tip: 'Lyrics',
                    on: _lyrics,
                    accent: accent,
                    onTap: () => setState(() => _lyrics = !_lyrics),
                  ),
              ],
            ),
            if (_lyrics && now.itemId != 0) ...[
              const SizedBox(height: 8),
              SizedBox(
                height: 120,
                child: c.hasLyrics
                    ? MiniLyrics(controller: c, scale: 0.9)
                    : Center(
                        child: Tap(
                          onTap: () =>
                              c.send(MusicCmd.fetchLyrics(itemId: now.itemId)),
                          child: Text('No lyrics yet · Find lyrics',
                              style: TextStyle(
                                  fontSize: 12.5,
                                  fontWeight: FontWeight.w600,
                                  color: t.textDim)),
                        ),
                      ),
              ),
            ],
            if (!live) ...[
              const SizedBox(height: 10),
              // The pills are 30 × scale tall and measure themselves with a
              // LayoutBuilder. Boxed at that height, the card can sit in an
              // IntrinsicHeight row: the height question stops at the box.
              SizedBox(
                height: 30 * 0.85,
                child: SeekPill(
                  pos: c.tickPos,
                  dur: c.tickDur,
                  accent: accent,
                  scale: 0.85,
                  onSeek: (v) => c.send(MusicCmd.seek(secs: v)),
                ),
              ),
            ],
            const SizedBox(height: 10),
            Row(
              children: [
                if (!live)
                  _Transport(Icons.skip_previous_rounded,
                      () => c.send(const MusicCmd.prev())),
                const SizedBox(width: 6),
                _Transport(
                    c.tickPlaying
                        ? Icons.pause_rounded
                        : Icons.play_arrow_rounded,
                    () => c.send(const MusicCmd.playPause()),
                    main: true),
                const SizedBox(width: 6),
                if (!live)
                  _Transport(Icons.skip_next_rounded,
                      () => c.send(const MusicCmd.next())),
                const SizedBox(width: 10),
                Expanded(
                  child: SizedBox(
                    height: 30 * 0.85,
                    child: VolPill(
                      volume: c.volume,
                      muted: c.muted,
                      accent: accent,
                      width: double.infinity,
                      scale: 0.85,
                      onVolume: (v) => c.setVolume(v),
                      onMute: () => c.send(const MusicCmd.toggleMute()),
                    ),
                  ),
                ),
                const SizedBox(width: 8),
                PlayerBtn(
                  icon: Icons.open_in_full,
                  tip: 'Zen player',
                  size: 30,
                  iconSize: 15,
                  accent: accent,
                  onTap: c.openZen,
                ),
              ],
            ),
          ],
        );
      },
    );
  }
}

/// A round button filled with the accent, lighter under the pointer and solid
/// while on.
class _FillBtn extends StatelessWidget {
  const _FillBtn({
    required this.icon,
    required this.tip,
    required this.on,
    required this.accent,
    required this.onTap,
  });

  final IconData icon;
  final String tip;
  final bool on;
  final Color accent;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => Tooltip(
        message: tip,
        child: Hover(
          onTap: onTap,
          builder: (context, hovered) => AnimatedContainer(
            duration: const Duration(milliseconds: 140),
            width: 30,
            height: 30,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color:
                  on ? accent : accent.withValues(alpha: hovered ? 0.42 : 0.2),
            ),
            child: Icon(icon,
                size: 15,
                color: on || hovered ? Colors.white : context.tokens.text),
          ),
        ),
      );
}

class _Transport extends StatelessWidget {
  const _Transport(this.icon, this.onTap, {this.main = false});

  final IconData icon;
  final VoidCallback onTap;
  final bool main;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final skin = context.skin;
    return Hover(
      onTap: onTap,
      builder: (context, hov) {
        final skinned = skin.control(
          active: false,
          hovered: hov,
          prominent: main,
          radius: 10,
          tint: Tokens.secMusic,
        );
        return Container(
          width: 32,
          height: 32,
          decoration: skinned ??
              BoxDecoration(
                color: main ? Tokens.secMusic : (hov ? t.panel : t.panel2),
                borderRadius: BorderRadius.circular(10),
              ),
          child: Icon(skin.icon(icon),
              size: 18,
              color: skinned == null
                  ? (main ? Colors.white : t.textDim)
                  : main
                      ? (skin.onProminent ?? Tokens.secMusic)
                      : t.text),
        );
      },
    );
  }
}
