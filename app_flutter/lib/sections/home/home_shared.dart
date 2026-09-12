// The pieces every Home layout is built from, transcribed from the four Slint
// pages rather than reinvented.
//
// `Surface` in ui/surface.slint is three visual languages behind one API; the
// port ships the Standard one, where every branch collapses to the literal
// value the component asked for (`rr(base) == base`, no rim, no rest cast).
// That is why the helpers below are one line each: they are the Standard
// branch, named so the transcription reads like the original.

import 'dart:io';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../design/skin.dart';
import '../../shell/shell_controller.dart';
import '../../shell/sidebar.dart';
import '../../src/rust/api/books.dart';
import '../../src/rust/api/home.dart';
import '../../src/rust/api/music.dart';
import '../../src/rust/api/photos.dart';
import '../../src/rust/api/videos.dart';
import '../music/music_controller.dart';

// ── Surface, Standard branch ────────────────────────────────────────────────

/// `Surface.card-radius`.
const double kCardRadius = 14;

/// `Surface.plate(a)` — a section-accented card body.
Color plate(Color a) => a.withValues(alpha: 0.08);

/// `Surface.plate-quiet(a)` — the third-strength fill the pink cards use.
Color plateQuiet(Color a) => a.withValues(alpha: 0.03);

/// `Surface.plate-border(a, strong)`.
Color plateBorder(Color a, bool strong) =>
    a.withValues(alpha: strong ? 0.4 : 0.24);

/// `Surface.plate-border-w(strong)`.
double plateBorderW(bool strong) => strong ? 1.5 : 1;

/// `Surface.wash(a, amt)`.
Color wash(Color a, double amt) => a.withValues(alpha: amt);

/// `color.darker(f)` — Slint scales lightness down by the factor.
Color darker(Color c, double f) {
  final h = HSLColor.fromColor(c);
  return h.withLightness((h.lightness / (1 + f)).clamp(0.0, 1.0)).toColor();
}

/// `color.brighter(f)`.
Color brighter(Color c, double f) {
  final h = HSLColor.fromColor(c);
  return h.withLightness((h.lightness * (1 + f)).clamp(0.0, 1.0)).toColor();
}

/// `a.mix(b, f)` — Slint keeps `f` of `a`.
Color mix(Color a, Color b, double f) => Color.lerp(b, a, f.clamp(0, 1))!;

// ── The palette Continue and the feed use, kind for kind ────────────────────

const Color kPodcast = Color(0xFF8B5CF6);
const Color kAudiobook = Color(0xFF3B82F6);
const Color kYoutube = Color(0xFFEF4444);
const Color kRadio = Color(0xFF14B8A6);
const Color kLiveTv = Color(0xFFF97316);

Color kindColor(String kind) => switch (kind) {
      'video' => Tokens.secVideos,
      'book' => Tokens.secBooks,
      'podcast' => kPodcast,
      'audiobook' => kAudiobook,
      _ => Tokens.secMusic,
    };

IconData kindIcon(String kind) => switch (kind) {
      'video' => Icons.movie_outlined,
      'book' => Icons.menu_book_outlined,
      'podcast' => Icons.mic_none_outlined,
      _ => Icons.headphones,
    };

String kindLabel(String kind) => switch (kind) {
      'video' => 'VIDEO',
      'podcast' => 'PODCAST',
      'book' => 'BOOK',
      _ => 'AUDIOBOOK',
    };

/// Which section a Continue row belongs to — where a click lands and which
/// resolver draws the cover.
Section kindSection(String kind) => switch (kind) {
      'video' => Section.videos,
      'book' => Section.books,
      _ => Section.music,
    };

/// The five kind filters, in the order every layout lists them.
const List<({String id, String label, Color accent})> kContinueKinds = [
  (id: 'all', label: 'All', accent: Tokens.secMusic),
  (id: 'book', label: 'Books', accent: Tokens.secBooks),
  (id: 'video', label: 'Videos', accent: Tokens.secVideos),
  (id: 'podcast', label: 'Podcasts', accent: kPodcast),
  (id: 'audiobook', label: 'Audiobooks', accent: kAudiobook),
];

// ── The one hover primitive ─────────────────────────────────────────────────

/// Hover state without a StatefulWidget per control.
///
/// Almost every Slint component on these four pages reads `t.has-hover` for a
/// fill, an outline or a lift. Writing twenty `State` classes to carry one bool
/// each is twenty places for the same bug.
class Hover extends StatefulWidget {
  const Hover({
    super.key,
    required this.builder,
    this.onTap,
    this.cursor = SystemMouseCursors.click,
  });

  final Widget Function(BuildContext context, bool hovered) builder;
  final VoidCallback? onTap;
  final MouseCursor cursor;

  @override
  State<Hover> createState() => _HoverState();
}

class _HoverState extends State<Hover> {
  bool _on = false;

  @override
  Widget build(BuildContext context) {
    final child = MouseRegion(
      cursor: widget.onTap == null ? MouseCursor.defer : widget.cursor,
      onEnter: (_) => setState(() => _on = true),
      onExit: (_) => setState(() => _on = false),
      child: widget.builder(context, _on),
    );
    if (widget.onTap == null) return child;
    return GestureDetector(
      behavior: HitTestBehavior.opaque,
      onTap: widget.onTap,
      child: child,
    );
  }
}

// ── HomeChip — the 30px pill, one size everywhere ───────────────────────────

/// `HomeChip` in ui/page_home.slint. Active is a SOLID accent fill with white
/// text: a translucent fill left accent-on-accent text unreadable.
class HomeChip extends StatelessWidget {
  const HomeChip({
    super.key,
    required this.label,
    required this.on,
    required this.onTap,
    this.accent = Tokens.brand,
  });

  final String label;
  final bool on;
  final VoidCallback onTap;
  final Color accent;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Hover(
      onTap: onTap,
      builder: (context, hov) {
        // A skin draws the chip as its own control, latched in the kind's
        // colour when on.
        final skin = context.skin;
        final skinned = skin.control(
            active: on, hovered: hov, tint: accent, radius: 15);
        return AnimatedContainer(
          duration: const Duration(milliseconds: 120),
          height: 30,
          padding: const EdgeInsets.symmetric(horizontal: 14),
          alignment: Alignment.center,
          decoration: skinned ??
              BoxDecoration(
                color: on ? accent : (hov ? t.panel : t.panel2),
                borderRadius: BorderRadius.circular(15),
                border: Border.all(color: on ? accent : t.outline),
              ),
          child: Text(
            label,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              fontSize: 11,
              fontWeight: FontWeight.w600,
              color: !on
                  ? t.text
                  : skinned == null
                      ? Colors.white
                      : (skin.activeInk ?? accent),
            ),
          ),
        );
      },
    );
  }
}

/// The kind filter row, shared by Classic, Welcome and Cinema.
class ContinueTabs extends StatelessWidget {
  const ContinueTabs({
    super.key,
    required this.active,
    required this.onPick,
  });

  final String active;
  final ValueChanged<String> onPick;

  @override
  Widget build(BuildContext context) => Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (final k in kContinueKinds)
            Padding(
              padding: EdgeInsets.only(left: k == kContinueKinds.first ? 0 : 6),
              child: HomeChip(
                label: k.label,
                on: active == k.id,
                accent: k.accent,
                onTap: () => onPick(k.id),
              ),
            ),
        ],
      );
}

// ── TitlePill — bare tinted icon | hairline | name (+ count) ────────────────

class TitlePill extends StatelessWidget {
  const TitlePill({
    super.key,
    required this.icon,
    required this.accent,
    required this.name,
    this.stat = '',
  });

  final IconData icon;
  final Color accent;
  final String name;
  final String stat;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // Dark: a deep-accent pill with WHITE ink so the label shines in the card's
    // colour. Light keeps the white pill with accent text.
    // A skin draws the pill as its latched control in the card's colour.
    final skin = context.skin;
    final skinned = skin.control(active: true, tint: accent, radius: 10);
    final ink = skinned != null
        ? (skin.activeInk ?? accent)
        : (t.dark ? Colors.white : accent);
    return Container(
      padding: const EdgeInsets.fromLTRB(11, 6, 12, 6),
      decoration: skinned ??
          BoxDecoration(
            color: t.dark ? darker(accent, 0.55) : Colors.white,
            borderRadius: BorderRadius.circular(10),
            border: Border.all(color: accent, width: 1.5),
          ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 20, color: ink),
          const SizedBox(width: 9),
          Container(width: 1, height: 20, color: ink.withValues(alpha: 0.30)),
          const SizedBox(width: 9),
          Flexible(
            child: Text(
              name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 17, fontWeight: FontWeight.w800, color: ink),
            ),
          ),
          if (stat.isNotEmpty) ...[
            const SizedBox(width: 9),
            Flexible(
              child: Text(
                stat,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 12, fontWeight: FontWeight.w600, color: ink),
              ),
            ),
          ],
        ],
      ),
    );
  }
}

// ── ProgressPill — remaining text, percent, and a thick fill ────────────────

/// The pill Classic, Welcome, Cinema's hero and Cinema's rail tiles all draw:
/// white fill on light, deep accent fill on dark, with the bar inside it.
class ProgressPill extends StatelessWidget {
  const ProgressPill({
    super.key,
    required this.sub,
    required this.frac,
    required this.accent,
    this.showPercent = true,
    this.compact = false,
  });

  final String sub;
  final double frac;
  final Color accent;
  final bool showPercent;

  /// Classic's card stacks the line over the bar with no percent column.
  final bool compact;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // Progress holds a value, so a skin sinks it into its well.
    final skin = context.skin;
    final well = skin.surface(SurfaceRole.well, radius: 9);
    final ink = well != null || !t.dark ? t.text : Colors.white;
    return Container(
      padding: const EdgeInsets.fromLTRB(9, 4, 9, 5),
      decoration: well ??
          BoxDecoration(
        color: t.dark
            ? darker(accent, 0.30)
            : (compact ? Colors.white : accent.withValues(alpha: 0.12)),
        borderRadius: BorderRadius.circular(9),
        border: Border.all(
          color: t.dark
              ? Colors.white.withValues(alpha: 0.80)
              : (compact
                  ? Colors.black.withValues(alpha: 0.53)
                  : accent.withValues(alpha: 0.55)),
        ),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  sub,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: compact ? 14 : 11,
                      fontWeight: compact ? FontWeight.w600 : FontWeight.w700,
                      color: ink),
                ),
              ),
              if (showPercent && !compact && frac >= 0)
                Text(
                  '${(frac * 100).round()}%',
                  style: TextStyle(
                      fontSize: 10, fontWeight: FontWeight.w800, color: ink),
                ),
            ],
          ),
          // A negative fraction is unknown, and the bar is hidden rather than
          // drawn at zero under something clearly part-way through.
          if (frac >= 0) ...[
            const SizedBox(height: 3),
            ClipRRect(
              borderRadius: BorderRadius.circular(compact ? 6 : 4),
              child: LinearProgressIndicator(
                value: frac.clamp(0.0, 1.0),
                minHeight: compact ? 12 : 8,
                backgroundColor: well != null
                    ? t.glassStrong
                    : t.dark
                        ? Colors.white.withValues(alpha: 0.25)
                        : accent.withValues(alpha: 0.18),
                color: well != null
                    ? accent
                    : t.dark
                        ? Colors.white
                        : accent,
              ),
            ),
          ],
        ],
      ),
    );
  }
}

// ── LazyCover — art fetched by the owning section's own resolver ────────────

class LazyCover extends StatefulWidget {
  const LazyCover({
    super.key,
    required this.section,
    required this.id,
    required this.tint,
    this.icon,
    this.fit = BoxFit.cover,
    this.iconSize = 20,
  });

  final Section section;
  final int id;
  final Color tint;
  final IconData? icon;
  final BoxFit fit;
  final double iconSize;

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
    if (id < 0) return;
    try {
      // Every section that has thumbnails has a resolver; Music's takes a
      // kind and a key rather than a bare id.
      final path = switch (widget.section) {
        Section.photos => await photosEnsureThumb(itemId: id),
        Section.videos => await videosEnsureThumb(itemId: id),
        Section.books => await booksEnsureCover(id: id),
        Section.music => await musicEnsureArt(kind: 'track', key: '$id'),
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
          : Center(
              child:
                  Icon(widget.icon, size: widget.iconSize, color: widget.tint)),
    );
    final path = _path;
    if (path == null) return plate;
    return Image.file(File(path),
        fit: widget.fit, errorBuilder: (_, __, ___) => plate);
  }
}

// ── HubTile — the double-height tile Welcome and Cinema share ───────────────

/// `HubTile` in ui/page_home_welcome.slint: an accent-washed tile with a white
/// disc riding the top, the name, and the count on its own solid accent pill.
class HubTile extends StatelessWidget {
  const HubTile({
    super.key,
    required this.name,
    required this.count,
    required this.icon,
    required this.accent,
    required this.onTap,
    this.disc = 46,
    this.textScale = 1.0,
    this.washScale = 1.0,
  });

  final String name;
  final String count;
  final IconData icon;
  final Color accent;
  final VoidCallback onTap;
  final double disc;
  final double textScale;

  /// One tile alone in a column needs more paint than eight in a row.
  final double washScale;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = textScale;
    return Hover(
      onTap: onTap,
      builder: (context, hov) => AnimatedContainer(
        duration: const Duration(milliseconds: 130),
        // A skin stands the tile up as its card; the armed ring below still
        // answers the hover.
        decoration: context.skin.surface(SurfaceRole.card, radius: 16) ??
            BoxDecoration(
          color: accent.withValues(
              alpha: hov
                  ? (t.dark ? 0.28 : 0.18 * washScale)
                  : (t.dark ? 0.16 : 0.10 * washScale)),
          borderRadius: BorderRadius.circular(16),
          border: Border.all(
              color: hov ? accent.withValues(alpha: 0.45) : Colors.transparent),
        ),
        // The armed inner ring, held off the tile's own edge — a button that is
        // armed rather than one that merely lit up.
        child: Padding(
          padding: const EdgeInsets.all(5),
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 130),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(11),
              border: Border.all(
                color:
                    hov ? accent.withValues(alpha: 0.75) : Colors.transparent,
                width: hov ? 2 : 0,
              ),
            ),
            padding: EdgeInsets.fromLTRB(3, 5 * s, 3, 5 * s),
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              mainAxisSize: MainAxisSize.min,
              children: [
                Container(
                  width: disc,
                  height: disc,
                  // A skin's disc is its latched control in the section's
                  // colour — pressed in, a lit pane, a tonal chip.
                  decoration: context.skin
                          .control(active: true, tint: accent, radius: 15) ??
                      BoxDecoration(
                        // White on every theme — the accent glyph is drawn for
                        // a light plate and the dark panel swallowed it.
                        color: Colors.white,
                        borderRadius: BorderRadius.circular(15),
                      ),
                  child: Icon(context.skin.icon(icon),
                      size: disc * 0.44, color: accent),
                ),
                SizedBox(height: 7 * s),
                // Flexible, not a bare Text: the disc and the count pill are
                // fixed, so the name is the only row that can give when the
                // band is a pixel short of what the type asks for.
                Flexible(
                  child: Text(
                    name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 14 * s,
                        fontWeight: FontWeight.w700,
                        color: t.text),
                  ),
                ),
                SizedBox(height: 7 * s),
                Container(
                  height: 19 * s,
                  padding: EdgeInsets.symmetric(horizontal: 9 * s),
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: accent,
                    borderRadius: BorderRadius.circular(9.5 * s),
                    border: Border.all(color: darker(accent, 0.25)),
                  ),
                  child: Text(
                    count,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 10 * s,
                        fontWeight: FontWeight.w700,
                        color: Colors.white),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// The eight hub cards, with the figures each section actually counts.
/// `hub-cards` in ui/page_home_cinema.slint, and the eight `HubTile`s Welcome
/// spells out — one list, because two copies drift.
typedef HubCard = ({
  String card,
  String name,
  String count,
  IconData icon,
  Color accent,
  Section section,
});

List<HubCard> homeHubCards(HomeState st) {
  final c = st.counts;
  return [
    (
      card: 'photos',
      name: 'Photos',
      count: '${c.photos} items',
      icon: Icons.image_outlined,
      accent: Tokens.secPhotos,
      section: Section.photos
    ),
    (
      card: 'videos',
      name: 'Videos',
      count: '${c.videos} items',
      icon: Icons.movie_outlined,
      accent: Tokens.secVideos,
      section: Section.videos
    ),
    (
      card: 'music',
      name: 'Music',
      count: '${c.songs} songs',
      icon: Icons.music_note_outlined,
      accent: Tokens.secMusic,
      section: Section.music
    ),
    (
      card: 'books',
      name: 'Books',
      count: '${c.books} books',
      icon: Icons.menu_book_outlined,
      accent: Tokens.secBooks,
      section: Section.books
    ),
    (
      card: 'cloud',
      name: 'Clouds',
      count: '${c.cloudRemotes} services',
      icon: Icons.cloud_outlined,
      accent: Tokens.secCloud,
      section: Section.cloud
    ),
    (
      card: 'tools',
      name: 'Tools',
      // The catalog's op count, not "utilities" — the other tiles all say how
      // much is in there.
      count: '${st.toolCount} tools',
      icon: Icons.build_outlined,
      accent: Tokens.secTools,
      section: Section.tools
    ),
    (
      card: 'transfer',
      name: 'Transfer',
      count: st.transferInbox.isEmpty ? 'local network' : st.transferInbox,
      icon: Icons.share_outlined,
      accent: Tokens.secTransfer,
      section: Section.transfer
    ),
    (
      card: 'finances',
      name: 'Finances',
      count: st.finMonth.isEmpty ? 'overview' : st.finMonth,
      icon: Icons.account_balance_wallet_outlined,
      accent: Tokens.secFinances,
      section: Section.finances
    ),
  ];
}

// ── The six launchers ───────────────────────────────────────────────────────

/// The doors that skip a section's front page and land on the thing you
/// actually wanted. Every layout carries the same six, in the same order.
const List<({String label, IconData icon, Color accent, String id})>
    kLaunchers = [
  (
    label: 'Stream',
    icon: Icons.monitor_outlined,
    accent: Tokens.secVideos,
    id: 'stream'
  ),
  (
    label: 'YouTube',
    icon: Icons.play_arrow_rounded,
    accent: kYoutube,
    id: 'youtube'
  ),
  (
    label: 'Music D/L',
    icon: Icons.download_outlined,
    accent: Tokens.secMusic,
    id: 'download'
  ),
  (
    label: 'Genesis Books',
    icon: Icons.auto_awesome_outlined,
    accent: Tokens.secBooks,
    id: 'genesis'
  ),
  (
    label: 'Random Radio',
    icon: Icons.radio_outlined,
    accent: Tokens.brand,
    id: 'radio'
  ),
  (
    label: 'Live TV',
    icon: Icons.tv_outlined,
    accent: Tokens.secCloud,
    id: 'livetv'
  ),
];

/// What a launcher does. Five of the six are deep links into a section's own
/// tab; Random Radio is an action, because there is no "random" tab to land on.
void launch(String id) {
  final shell = ShellController.instance;
  switch (id) {
    case 'stream':
      shell.goTab(Section.videos, 'stream');
    case 'livetv':
      shell.goTab(Section.videos, 'livetv');
    case 'youtube':
      shell.goTab(Section.music, 'youtube');
    case 'download':
      shell.goTab(Section.music, 'downloader');
    case 'genesis':
      shell.goTab(Section.books, 'genesis');
    case 'radio':
      shell.go(Section.music);
      MusicController.instance.randomRadio();
  }
}

// ── TopPill — Cinema's and Stream's header launcher ─────────────────────────

class TopPill extends StatelessWidget {
  const TopPill({
    super.key,
    required this.label,
    required this.icon,
    required this.onTap,
    this.accent = Tokens.brand,
    this.solid = false,
  });

  final String label;
  final IconData icon;
  final VoidCallback onTap;
  final Color accent;

  /// Solid = filled with the accent, white ink. The glass version disappeared
  /// into a bright backdrop.
  final bool solid;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Hover(
      onTap: onTap,
      builder: (context, hov) => AnimatedContainer(
        duration: const Duration(milliseconds: 120),
        height: 31,
        padding: const EdgeInsets.symmetric(horizontal: 12),
        // Solid stays the section's colour; the glass version is the skin's
        // control.
        decoration: (solid
                ? null
                : context.skin.control(
                    active: false, hovered: hov, tint: accent, radius: 16)) ??
            BoxDecoration(
          color: solid
              ? (hov ? accent.withValues(alpha: 0.85) : accent)
              : (hov ? t.glassStrong : t.glass),
          borderRadius: BorderRadius.circular(16),
          border: solid ? null : Border.all(color: t.glassBorder),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 14, color: solid ? Colors.white : accent),
            const SizedBox(width: 7),
            Text(
              label,
              style: TextStyle(
                fontSize: 11.5,
                fontWeight: solid ? FontWeight.w600 : FontWeight.w400,
                color: solid ? Colors.white : (hov ? t.text : t.textDim),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// The launcher row Cinema and Stream wear in their header.
///
/// Six pills is more than a 1280px header has room for once the wordmark and
/// the greeting have had theirs, so the row scrolls instead of spilling —
/// `reverse`, which keeps it pinned to the right the way the layout wants.
class LauncherPills extends StatelessWidget {
  const LauncherPills({super.key});

  @override
  Widget build(BuildContext context) => SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        reverse: true,
        child: _row(),
      );

  Widget _row() => Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (final l in kLaunchers)
            Padding(
              padding: EdgeInsets.only(left: l == kLaunchers.first ? 0 : 10),
              child: TopPill(
                solid: true,
                label: l.label == 'Genesis Books' ? 'Genesis' : l.label,
                icon: l.icon,
                accent: l.accent,
                onTap: () => launch(l.id),
              ),
            ),
        ],
      );
}

// ── CineBtn — the round transport key ───────────────────────────────────────

class CineBtn extends StatelessWidget {
  const CineBtn({
    super.key,
    required this.icon,
    required this.onTap,
    required this.accent,
    this.primary = false,
    this.lit = false,
  });

  final IconData icon;
  final VoidCallback onTap;
  final Color accent;

  /// play/pause — filled at rest, 20% up on the rest.
  final bool primary;

  /// shuffle on / repeat on.
  final bool lit;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final size = primary ? 53.0 : 32.0;
    final skin = context.skin;
    return Hover(
      onTap: onTap,
      builder: (context, hov) {
        final on = primary || lit || hov;
        return AnimatedContainer(
          duration: const Duration(milliseconds: 120),
          width: size,
          height: size,
          // A skin's key: the play button its prominent control, a lit one
          // latched.
          decoration: skin.control(
                active: lit,
                hovered: hov,
                prominent: primary,
                tint: accent,
                radius: size / 2,
              ) ??
              BoxDecoration(
                color: on ? accent : t.glass,
                shape: BoxShape.circle,
                border: Border.all(color: on ? accent : t.glassBorder),
              ),
          child: Icon(skin.icon(icon),
              size: primary ? 22 : 14,
              color: skin.isStandard
                  ? (on ? Colors.white : t.text)
                  : primary
                      ? (skin.onProminent ?? accent)
                      : lit
                          ? (skin.activeInk ?? accent)
                          : t.text),
        );
      },
    );
  }
}

// ── HeroBtn — Cinema's Resume / Details ─────────────────────────────────────

class HeroBtn extends StatelessWidget {
  const HeroBtn({
    super.key,
    required this.label,
    required this.icon,
    required this.onTap,
    this.primary = false,
    this.fill = Tokens.secVideos,
  });

  final String label;
  final IconData icon;
  final VoidCallback onTap;
  final bool primary;

  /// The primary button wears the item's category colour, not white — a white
  /// pill on a bright backdrop was the one control you could lose.
  final Color fill;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Hover(
      onTap: onTap,
      builder: (context, hov) => AnimatedContainer(
        duration: const Duration(milliseconds: 120),
        height: 42,
        padding: const EdgeInsets.symmetric(horizontal: 18),
        decoration: (primary
                ? null
                : context.skin
                    .control(active: false, hovered: hov, radius: 21)) ??
            BoxDecoration(
          color: primary
              ? (hov ? fill.withValues(alpha: 0.88) : fill)
              : (hov ? t.glassStrong : t.glass),
          borderRadius: BorderRadius.circular(21),
          border: primary ? null : Border.all(color: t.glassBorder),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 16, color: primary ? Colors.white : t.text),
            const SizedBox(width: 9),
            Text(
              label,
              style: TextStyle(
                fontSize: 13,
                fontWeight: primary ? FontWeight.w700 : FontWeight.w600,
                color: primary ? Colors.white : t.text,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ── Kind tag — the mini chip every Continue surface wears ───────────────────

class KindTag extends StatelessWidget {
  const KindTag({
    super.key,
    required this.kind,
    required this.accent,
    this.filled = false,
    this.height = 18,
  });

  final String kind;
  final Color accent;

  /// Cinema's rail tiles and hero fill it; the rest wash it.
  final bool filled;
  final double height;

  @override
  Widget build(BuildContext context) => Container(
        height: height,
        padding: EdgeInsets.symmetric(horizontal: filled ? 8 : 7),
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: filled ? accent : wash(accent, 0.15),
          borderRadius: BorderRadius.circular(filled ? 6 : height / 2),
          border:
              filled ? null : Border.all(color: accent.withValues(alpha: 0.55)),
        ),
        child: Text(
          kindLabel(kind),
          style: TextStyle(
            fontSize: filled ? 8.5 : 8,
            fontWeight: FontWeight.w800,
            letterSpacing: filled ? 0.7 : 0.6,
            color: filled ? Colors.white : accent,
          ),
        ),
      );
}

// ── The greeting's own bits ─────────────────────────────────────────────────

/// The app mark, in whichever of the five the profile picked.
///
/// `logo-choice` is a Settings value the Home bridge does not carry, so the
/// port draws the default mark. Kept as one widget so plumbing the choice
/// later is one edit rather than four.
/// The header avatar, with the online dot on its edge.
class HomeAvatar extends StatelessWidget {
  const HomeAvatar({super.key, this.size = 40, this.dot = false});

  final double size;
  final bool dot;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Hover(
      onTap: () => ShellController.instance.go(Section.settings),
      builder: (context, hov) => SizedBox(
        width: size + (dot ? 4 : 0),
        height: size + (dot ? 4 : 0),
        child: Stack(
          children: [
            AnimatedContainer(
              duration: const Duration(milliseconds: 140),
              width: size,
              height: size,
              decoration: BoxDecoration(
                gradient: const LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: [Tokens.brand, Tokens.brand2],
                ),
                shape: BoxShape.circle,
                border: Border.all(
                  color: Tokens.brand.withValues(alpha: 0.6),
                  width: hov ? 2 : 0,
                ),
              ),
              child: Icon(Icons.person_outline,
                  size: size * 0.42, color: Colors.white),
            ),
            if (dot)
              Positioned(
                right: 0,
                bottom: 0,
                child: Container(
                  width: 14,
                  height: 14,
                  decoration: BoxDecoration(
                    color: const Color(0xFF22C55E),
                    shape: BoxShape.circle,
                    border: Border.all(color: t.panel, width: 2),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// The section a Stream feed row came from.
Section sectionOf(String name) => switch (name) {
      'photos' => Section.photos,
      'videos' => Section.videos,
      'music' => Section.music,
      'books' => Section.books,
      'cloud' => Section.cloud,
      'tools' => Section.tools,
      'transfer' => Section.transfer,
      'finances' => Section.finances,
      _ => Section.home,
    };

String sectionName(String s) => switch (s) {
      'photos' => 'Photos',
      'videos' => 'Videos',
      'music' => 'Music',
      'books' => 'Books',
      'cloud' => 'Clouds',
      'tools' => 'Tools',
      'transfer' => 'Transfers',
      'finances' => 'Finances',
      _ => 'Library',
    };

IconData sectionIcon(String s) => s == 'library'
    ? Icons.info_outline
    : (kSectionMeta[sectionOf(s)]?.icon ?? Icons.info_outline);

Color sectionAccent(String s) =>
    s == 'library' ? Tokens.brand : accentFor(sectionOf(s));

/// The spend for month [i] of Stream's twelve bars, recovered rather than
/// fetched.
///
/// The bridge sends twelve bar heights (each month over the peak) and this
/// month's figure as a string. The newest bar IS this month, so the peak falls
/// out of the two and every other month follows. Null when there is nothing to
/// divide by — a zero newest month, or a figure that will not parse — and the
/// block prints an em dash rather than a wrong number.
String? monthSpend(HomeState st, int i) {
  final months = st.finMonths;
  if (i < 0 || i >= months.length) return null;
  if (i == months.length - 1) {
    return st.finSpent.isEmpty ? null : st.finSpent;
  }
  final last = months.last;
  if (last <= 0) return null;
  final newest = double.tryParse(st.finSpent.replaceAll(',', ''));
  if (newest == null) return null;
  return (months[i] * (newest / last)).toStringAsFixed(2);
}
