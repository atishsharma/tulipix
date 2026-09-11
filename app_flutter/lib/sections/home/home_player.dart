// Home's four players.
//
// Every layout gives the transport a different shape — Classic embeds the real
// mini in a rail, Cinema stands it up in a 300px glass column, Stream lays it
// across a 396px rail, Welcome pins it along the bottom of the page — and all
// four read `MusicController.instance`, the same controller the floating mini
// and the Music section's own bar read. Two players that can disagree about
// what is playing is the bug this avoids.

import 'dart:io';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../design/skin.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/music.dart';
import '../music/mini_player.dart';
import '../music/music_controller.dart';
import '../music/music_viz.dart';
import '../music/player_widgets.dart' show Marquee;
import 'home_shared.dart';

/// The tab the sound is coming from, by name and by colour: the card says where
/// to go to change what is on, rather than a generic PLAYING.
({String name, Color accent, IconData icon}) playingSource(String mode) =>
    switch (mode) {
      'podcast' => (
          name: 'Podcasts',
          accent: kPodcast,
          icon: Icons.mic_none_outlined
        ),
      'radio' => (
          name: 'Radio',
          accent: Tokens.secCloud,
          icon: Icons.radio_outlined
        ),
      'book' => (
          name: 'Audiobooks',
          accent: Tokens.secBooks,
          icon: Icons.menu_book_outlined
        ),
      'youtube' => (
          name: 'YouTube',
          accent: kYoutube,
          icon: Icons.play_arrow_rounded
        ),
      _ => (
          name: 'My Music',
          accent: Tokens.secMusic,
          icon: Icons.music_note_outlined
        ),
    };

/// m:ss, for the labels riding inside the seek bar.
String clock(double secs) {
  if (secs.isNaN || secs.isInfinite || secs <= 0) return '0:00';
  final s = secs.round();
  final h = s ~/ 3600;
  final m = (s ~/ 60) % 60;
  final ss = (s % 60).toString().padLeft(2, '0');
  return h > 0 ? '$h:${m.toString().padLeft(2, '0')}:$ss' : '$m:$ss';
}

/// Open the tab the sound is coming from.
void openPlayingTab(MusicController c) {
  ShellController.instance.go(Section.music);
  final mode = c.now?.mode ?? 'music';
  final view = switch (mode) {
    'podcast' => 'podcasts',
    'book' => 'audiobooks',
    'radio' => 'radio',
    'youtube' => 'youtube',
    _ => 'mymusic',
  };
  if (c.view != view) c.send(MusicCmd.setView(name: view));
}

// ── The thick seek bar with both times riding inside it ─────────────────────

/// Click to jump; NOT drag — every move event would be one more mpv IPC
/// command, and mpv logs a broken pipe for each dead client the burst leaves.
class HomeSeek extends StatelessWidget {
  const HomeSeek({
    super.key,
    required this.pos,
    required this.dur,
    required this.accent,
    required this.onSeek,
    this.height = 22,
    this.ink,
    this.dim,
  });

  final double pos;
  final double dur;
  final Color accent;
  final ValueChanged<double> onSeek;
  final double height;

  /// Welcome's bar carries its own ink so it reads on both themes.
  final Color? ink;
  final Color? dim;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final base = ink ?? t.text;
    final faint = dim ?? t.textDim;
    final frac = dur > 0 ? (pos / dur).clamp(0.0, 1.0) : 0.0;
    return LayoutBuilder(
      builder: (context, box) => GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTapDown: (d) =>
            onSeek(dur * (d.localPosition.dx / box.maxWidth).clamp(0.0, 1.0)),
        child: MouseRegion(
          cursor: SystemMouseCursors.click,
          child: Container(
            height: height,
            decoration: BoxDecoration(
              color: base.withValues(alpha: 0.13),
              borderRadius: BorderRadius.circular(height / 2),
              border: Border.all(color: base.withValues(alpha: 0.22)),
            ),
            clipBehavior: Clip.antiAlias,
            child: Stack(
              children: [
                FractionallySizedBox(
                  widthFactor: frac,
                  child: DecoratedBox(
                    decoration: BoxDecoration(
                      gradient: LinearGradient(
                        colors: t.dark
                            ? [accent, brighter(accent, 0.45), Colors.white]
                            : [accent, Tokens.brand],
                        stops: t.dark ? const [0, 0.55, 1] : null,
                      ),
                    ),
                  ),
                ),
                // Elapsed rides the fill, so it flips to white once the fill
                // has actually reached it.
                Positioned(
                  left: 10,
                  top: 0,
                  bottom: 0,
                  child: Center(
                    child: Text(
                      clock(pos),
                      style: TextStyle(
                        fontSize: 10,
                        fontWeight: FontWeight.w700,
                        color: frac > 0.14 ? Colors.white : faint,
                      ),
                    ),
                  ),
                ),
                Positioned(
                  right: 10,
                  top: 0,
                  bottom: 0,
                  child: Center(
                    child: Text(
                      clock(dur),
                      style: TextStyle(
                          fontSize: 10,
                          fontWeight: FontWeight.w700,
                          color: faint),
                    ),
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

// ── The volume track, with the knob on it ───────────────────────────────────

/// A drag moves a local value and only commits on release: one mpv IPC command
/// per gesture, not one per mouse move.
class HomeVolume extends StatefulWidget {
  const HomeVolume({
    super.key,
    required this.volume,
    required this.muted,
    required this.accent,
    required this.onVolume,
    this.ink,
  });

  /// mpv's 0–130 percent, not a fraction. The track spans 0–100; the boost
  /// range simply pins the fill full.
  final double volume;
  final bool muted;
  final Color accent;
  final ValueChanged<double> onVolume;
  final Color? ink;

  @override
  State<HomeVolume> createState() => _HomeVolumeState();
}

class _HomeVolumeState extends State<HomeVolume> {
  double? _drag;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final base = widget.ink ?? t.text;
    final frac =
        _drag ?? (widget.muted ? 0.0 : (widget.volume / 100).clamp(0.0, 1.0));
    return LayoutBuilder(
      builder: (context, box) {
        final w = box.maxWidth - 16;
        void to(double dx) =>
            setState(() => _drag = ((dx - 8) / w).clamp(0.0, 1.0));
        return GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTapDown: (d) {
            to(d.localPosition.dx);
            widget.onVolume(100 * (_drag ?? 0));
            setState(() => _drag = null);
          },
          onHorizontalDragUpdate: (d) => to(d.localPosition.dx),
          onHorizontalDragEnd: (_) {
            final v = _drag;
            setState(() => _drag = null);
            if (v != null) widget.onVolume(100 * v);
          },
          child: MouseRegion(
            cursor: SystemMouseCursors.click,
            child: SizedBox(
              height: 32,
              child: Stack(
                clipBehavior: Clip.none,
                children: [
                  Positioned(
                    left: 8,
                    right: 8,
                    top: 10,
                    child: Container(
                      height: 12,
                      decoration: BoxDecoration(
                        color: base.withValues(alpha: 0.13),
                        borderRadius: BorderRadius.circular(6),
                        border: Border.all(color: base.withValues(alpha: 0.22)),
                      ),
                      clipBehavior: Clip.antiAlias,
                      child: FractionallySizedBox(
                        widthFactor: frac,
                        alignment: Alignment.centerLeft,
                        child: ColoredBox(color: widget.accent),
                      ),
                    ),
                  ),
                  Positioned(
                    left: (w * frac).clamp(0.0, w),
                    top: 8,
                    child: Container(
                      width: 16,
                      height: 16,
                      decoration: BoxDecoration(
                        color: t.dark ? Colors.white : base,
                        shape: BoxShape.circle,
                        border: Border.all(color: widget.accent, width: 2),
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}

// ── Up next ─────────────────────────────────────────────────────────────────

/// The queue, wherever it is shown. The lit row is the one whose PLAYBACK
/// POSITION is playing — not the n-th row.
class HomeQueue extends StatelessWidget {
  const HomeQueue({
    super.key,
    required this.controller,
    required this.accent,
    this.rowHeight = 34,
    this.dense = true,
  });

  final MusicController controller;
  final Color accent;
  final double rowHeight;
  final bool dense;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final rows = controller.state?.queue ?? const <Track>[];
    final at = controller.state?.now.itemId ?? 0;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Expanded(
              child: Text('Up next',
                  style: TextStyle(
                      fontSize: dense ? 12 : 13,
                      fontWeight: FontWeight.w700,
                      color: t.text)),
            ),
            Text('${rows.length} tracks',
                style:
                    TextStyle(fontSize: dense ? 10 : 10.5, color: t.textDim)),
          ],
        ),
        const SizedBox(height: 6),
        if (rows.isEmpty)
          Text('The queue is empty — press play and it fills up.',
              style: TextStyle(fontSize: 10.5, color: t.textDim))
        else
          Expanded(
            child: ListView.separated(
              padding: EdgeInsets.zero,
              itemCount: rows.length,
              separatorBuilder: (_, __) => const SizedBox(height: 2),
              itemBuilder: (context, i) {
                final q = rows[i];
                final now = q.itemId == at && at != 0;
                return Hover(
                  onTap: () => controller.send(MusicCmd.queuePlayAt(index: i)),
                  builder: (context, hov) => Container(
                    height: rowHeight,
                    padding: const EdgeInsets.fromLTRB(5, 3, 7, 3),
                    decoration: BoxDecoration(
                      color: now
                          ? accent.withValues(alpha: 0.18)
                          : (hov ? t.glassStrong : Colors.transparent),
                      borderRadius: BorderRadius.circular(9),
                    ),
                    child: Row(
                      children: [
                        ClipRRect(
                          borderRadius: BorderRadius.circular(7),
                          child: SizedBox(
                            width: rowHeight - 6,
                            height: rowHeight - 6,
                            child: LazyCover(
                              section: Section.music,
                              id: q.itemId,
                              tint: accent,
                              icon: Icons.music_note,
                              iconSize: 12,
                            ),
                          ),
                        ),
                        const SizedBox(width: 7),
                        Expanded(
                          child: Column(
                            mainAxisAlignment: MainAxisAlignment.center,
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              Text(q.title,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: TextStyle(
                                      fontSize: 11,
                                      fontWeight: now
                                          ? FontWeight.w700
                                          : FontWeight.w400,
                                      color: now ? accent : t.text)),
                              Text(q.artist,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style:
                                      TextStyle(fontSize: 9, color: t.textDim)),
                            ],
                          ),
                        ),
                        const SizedBox(width: 6),
                        Text(clock(q.durationS),
                            style: TextStyle(fontSize: 9, color: t.textDim)),
                      ],
                    ),
                  ),
                );
              },
            ),
          ),
      ],
    );
  }
}

// ── Three lyric lines ───────────────────────────────────────────────────────

/// The window Cinema draws beside Continue and Stream draws under its seek bar:
/// the line before, the line now, the line next.
class HomeLyrics extends StatelessWidget {
  const HomeLyrics({
    super.key,
    required this.controller,
    required this.accent,
    this.big = false,
  });

  final MusicController controller;
  final Color accent;

  /// Cinema's is 19px in the shelf's spare 30%; Stream's is 13px in a rail.
  final bool big;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final lines = controller.state?.lyrics ?? const <LyricLine>[];
    final i = controller.activeLyric;
    if (i < 0 || lines.isEmpty) return const SizedBox.shrink();
    String at(int k) => k >= 0 && k < lines.length ? lines[k].text : '';
    final faint = big
        ? accent.withValues(alpha: 0.45)
        : t.textDim.withValues(alpha: 0.55);
    Widget line(String s, {required bool cur}) => Text(
          s,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          textAlign: TextAlign.center,
          style: TextStyle(
            fontSize: cur ? (big ? 19 : 13) : (big ? 13 : 10.5),
            fontWeight: cur ? FontWeight.w700 : FontWeight.w400,
            color: cur ? accent : faint,
          ),
        );
    return Column(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        line(at(i - 1), cur: false),
        SizedBox(height: big ? 7 : 2),
        line(at(i), cur: true),
        SizedBox(height: big ? 7 : 2),
        line(at(i + 1), cur: false),
      ],
    );
  }
}

// ── The five transport keys ─────────────────────────────────────────────────

/// Shuffle · prev · play/pause · next · repeat, the set every Home player
/// carries. With nothing loaded the middle key starts a shuffled library, so
/// the block is never a dead control.
class HomeTransport extends StatelessWidget {
  const HomeTransport({
    super.key,
    required this.controller,
    required this.accent,
    this.spacing = 8,
    this.alignment = MainAxisAlignment.center,
  });

  final MusicController controller;
  final Color accent;
  final double spacing;
  final MainAxisAlignment alignment;

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    final now = controller.now;
    final loaded = (now?.title ?? '').isNotEmpty;
    return Row(
      mainAxisAlignment: alignment,
      mainAxisSize: MainAxisSize.min,
      children: [
        CineBtn(
          icon: Icons.shuffle,
          lit: st?.shuffle ?? false,
          accent: accent,
          onTap: () => controller.send(const MusicCmd.toggleShuffle()),
        ),
        SizedBox(width: spacing),
        CineBtn(
          icon: Icons.skip_previous,
          accent: accent,
          onTap: () => controller.send(const MusicCmd.prev()),
        ),
        SizedBox(width: spacing),
        CineBtn(
          icon: controller.tickPlaying ? Icons.pause : Icons.play_arrow,
          primary: true,
          accent: accent,
          // Nothing loaded and there is no shuffle-all on the bridge, so the
          // key is a door into Music rather than a button that does nothing.
          onTap: () => loaded
              ? controller.send(const MusicCmd.playPause())
              : ShellController.instance.go(Section.music),
        ),
        SizedBox(width: spacing),
        CineBtn(
          icon: Icons.skip_next,
          accent: accent,
          onTap: () => controller.send(const MusicCmd.next()),
        ),
        SizedBox(width: spacing),
        CineBtn(
          icon:
              (st?.repeat ?? 'off') == 'one' ? Icons.repeat_one : Icons.repeat,
          lit: (st?.repeat ?? 'off') != 'off',
          accent: accent,
          onTap: () => controller.send(const MusicCmd.cycleRepeat()),
        ),
      ],
    );
  }
}

/// Queue · mute · volume · zen — the row under every Home transport.
class HomePlayerBar extends StatelessWidget {
  const HomePlayerBar({
    super.key,
    required this.controller,
    required this.accent,
    required this.queueOpen,
    required this.onQueue,
    this.ink,
  });

  final MusicController controller;
  final Color accent;
  final bool queueOpen;
  final VoidCallback onQueue;
  final Color? ink;

  @override
  Widget build(BuildContext context) {
    final now = controller.now;
    return Row(
      children: [
        CineBtn(
          icon: Icons.queue_music,
          lit: queueOpen,
          accent: accent,
          onTap: onQueue,
        ),
        const SizedBox(width: 8),
        CineBtn(
          icon: (now?.muted ?? false) ? Icons.volume_off : Icons.volume_up,
          accent: accent,
          onTap: () => controller.send(const MusicCmd.toggleMute()),
        ),
        const SizedBox(width: 8),
        Expanded(
          child: HomeVolume(
            volume: now?.volume ?? 100,
            muted: now?.muted ?? false,
            accent: accent,
            ink: ink,
            onVolume: (v) => controller.send(MusicCmd.setVolume(volume: v)),
          ),
        ),
        const SizedBox(width: 8),
        CineBtn(
          icon: Icons.fullscreen,
          accent: accent,
          onTap: controller.openZen,
        ),
      ],
    );
  }
}

// ── Cinema: the right glass column ──────────────────────────────────────────

/// The panel is CONTENT tall, not column tall: stretched to the full column it
/// opened a dead band above and below the artwork.
const double kCinemaChrome = 238;

class CinemaPlayer extends StatefulWidget {
  const CinemaPlayer({super.key, required this.height});

  final double height;

  @override
  State<CinemaPlayer> createState() => _CinemaPlayerState();
}

// Two controllers — the flip and the breathing ring — so `Ticker*s*`, plural.
class _CinemaPlayerState extends State<CinemaPlayer>
    with TickerProviderStateMixin {
  /// The queue lives on the BACK of the artwork: the button turns the square
  /// over rather than dropping a dialog on top of the card.
  bool _flipped = false;
  late final AnimationController _turn = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 320),
  );

  /// The breathing ring, while something plays.
  late final AnimationController _pulse = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 1240),
  )..repeat(reverse: true);

  @override
  void dispose() {
    _turn.dispose();
    _pulse.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = MusicController.instance;
    return AnimatedBuilder(
      animation: c,
      builder: (context, _) {
        final now = c.now;
        final accent = c.accent;
        final src = playingSource(now?.mode ?? 'idle');
        final art = (now?.art ?? '').isNotEmpty ? now!.art : '';
        // The artwork takes every pixel the controls do not.
        final side = (widget.height - kCinemaChrome).clamp(0.0, 300.0 - 26.0);
        return Stack(
          clipBehavior: Clip.none,
          children: [
            // The ring stands OFF the card, 8px clear on every side, so the
            // panel keeps its own edge.
            if (c.tickPlaying)
              Positioned(
                left: -8,
                top: -8,
                right: -8,
                bottom: -8,
                child: AnimatedBuilder(
                  animation: _pulse,
                  builder: (context, _) => DecoratedBox(
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(23),
                      border: Border.all(
                        width: 4,
                        color: mix(
                                accent,
                                t.dark ? Colors.white : Tokens.brand2,
                                1 - _pulse.value)
                            .withValues(alpha: 0.35 + 0.5 * _pulse.value),
                      ),
                    ),
                  ),
                ),
              ),
            Container(
              decoration: BoxDecoration(
                // Deliberately not a real backdrop blur: blurring a live
                // 1440-wide image every frame is the expensive thing, and a
                // dimmed plate over a scrim reads the same at this size.
                color:
                    t.dark ? const Color(0xB00C0C14) : const Color(0xC4FFFFFF),
                borderRadius: BorderRadius.circular(15),
                border: Border.all(color: t.glassBorder),
              ),
              clipBehavior: Clip.antiAlias,
              child: DecoratedBox(
                decoration: BoxDecoration(
                  color: src.accent.withValues(alpha: t.dark ? 0.16 : 0.10),
                ),
                child: Padding(
                  padding: const EdgeInsets.all(13),
                  child: Column(
                    children: [
                      // The header names the TAB, not the state, and the whole
                      // row is the door back to it.
                      Hover(
                        onTap: () => openPlayingTab(c),
                        builder: (context, hov) => AnimatedContainer(
                          duration: const Duration(milliseconds: 120),
                          height: 22,
                          decoration: BoxDecoration(
                            color: hov
                                ? src.accent.withValues(alpha: 0.16)
                                : Colors.transparent,
                            borderRadius: BorderRadius.circular(11),
                          ),
                          child: Row(
                            mainAxisAlignment: MainAxisAlignment.center,
                            children: [
                              Icon(src.icon, size: 13, color: src.accent),
                              const SizedBox(width: 7),
                              Text(src.name,
                                  style: TextStyle(
                                      fontSize: 11,
                                      fontWeight: FontWeight.w700,
                                      letterSpacing: 0.3,
                                      color: src.accent)),
                            ],
                          ),
                        ),
                      ),
                      const SizedBox(height: 10),
                      // Title FIRST, then the art: you read what is playing,
                      // then look at it.
                      SizedBox(
                        height: 20,
                        child: Text(
                          (now?.title ?? '').isEmpty
                              ? 'Nothing playing'
                              : now!.title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          textAlign: TextAlign.center,
                          style: TextStyle(
                              fontSize: 15,
                              fontWeight: FontWeight.w700,
                              color: t.text),
                        ),
                      ),
                      SizedBox(
                        height: 14,
                        child: Text(
                          (now?.title ?? '').isEmpty
                              ? 'Press play for a shuffled library'
                              : now!.artist,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          textAlign: TextAlign.center,
                          style: TextStyle(fontSize: 10.5, color: t.textDim),
                        ),
                      ),
                      const SizedBox(height: 7),
                      SizedBox(
                        height: side,
                        child: Center(
                          child: AnimatedBuilder(
                            animation: _turn,
                            builder: (context, child) {
                              // 0 → 0.5 → 1 squeezes the square to nothing and
                              // back; the faces swap at the midpoint, which is
                              // what makes it read as a flip.
                              final squeeze =
                                  (1 - 2 * _turn.value).abs().clamp(0.02, 1.0);
                              return SizedBox(
                                width: side * squeeze,
                                height: side,
                                child: ClipRRect(
                                  borderRadius: BorderRadius.circular(12),
                                  child: OverflowBox(
                                    maxWidth: side,
                                    minWidth: side,
                                    child: _turn.value < 0.5
                                        ? _cover(art, accent, side)
                                        : _queueFace(side, accent, c),
                                  ),
                                ),
                              );
                            },
                          ),
                        ),
                      ),
                      const SizedBox(height: 8),
                      HomeSeek(
                        pos: c.tickPos,
                        dur: c.tickDur,
                        accent: accent,
                        onSeek: (v) => c.send(MusicCmd.seek(secs: v)),
                      ),
                      const SizedBox(height: 12),
                      SizedBox(
                        height: 53,
                        child: HomeTransport(controller: c, accent: accent),
                      ),
                      const SizedBox(height: 10),
                      SizedBox(
                        height: 32,
                        child: HomePlayerBar(
                          controller: c,
                          accent: accent,
                          queueOpen: _flipped,
                          onQueue: () {
                            setState(() => _flipped = !_flipped);
                            if (_flipped) {
                              _turn.forward();
                              c.refresh();
                            } else {
                              _turn.reverse();
                            }
                          },
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ],
        );
      },
    );
  }

  Widget _cover(String art, Color accent, double side) => ColoredBox(
        color: accent.withValues(alpha: 0.18),
        child: art.isEmpty
            ? Icon(Icons.music_note, size: 34, color: accent)
            : Image.file(File(art),
                fit: BoxFit.cover,
                errorBuilder: (_, __, ___) =>
                    Icon(Icons.music_note, size: 34, color: accent)),
      );

  Widget _queueFace(double side, Color accent, MusicController c) {
    final t = context.tokens;
    return Container(
      color: t.dark ? const Color(0xE60C0C14) : const Color(0xE6FFFFFF),
      padding: const EdgeInsets.all(10),
      child: HomeQueue(controller: c, accent: accent),
    );
  }
}

// ── Stream: the 396px rail block ────────────────────────────────────────────

class StreamRailPlayer extends StatefulWidget {
  const StreamRailPlayer({super.key, required this.railWidth});

  final double railWidth;

  @override
  State<StreamRailPlayer> createState() => _StreamRailPlayerState();
}

class _StreamRailPlayerState extends State<StreamRailPlayer>
    with SingleTickerProviderStateMixin {
  bool _queue = false;
  late final AnimationController _pulse = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 1240),
  )..repeat(reverse: true);

  @override
  void dispose() {
    _pulse.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = MusicController.instance;
    // Cover side = 30% of the rail; the two lines and the transport take the
    // 70 — at 40 the five keys had to shrink to fit beside it.
    final art = (widget.railWidth * 0.3).roundToDouble();
    final pad = widget.railWidth * 0.03;
    return AnimatedBuilder(
      animation: c,
      builder: (context, _) {
        final now = c.now;
        final accent = c.accent;
        final src = playingSource(now?.mode ?? 'idle');
        final live = (now?.title ?? '').isEmpty;
        return Padding(
          padding: EdgeInsets.symmetric(horizontal: pad),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              // The caption names the tab the sound comes from, and is the door
              // back to it.
              Hover(
                onTap: () => openPlayingTab(c),
                builder: (context, hov) => AnimatedContainer(
                  duration: const Duration(milliseconds: 120),
                  height: 26,
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: hov
                        ? src.accent.withValues(alpha: 0.14)
                        : Colors.transparent,
                    borderRadius: BorderRadius.circular(8),
                  ),
                  child: Text(src.name.toUpperCase(),
                      style: TextStyle(
                          fontSize: 10,
                          fontWeight: FontWeight.w700,
                          letterSpacing: 1.5,
                          color: src.accent)),
                ),
              ),
              const SizedBox(height: 10),
              // 30 : 70 — the cover on the left, everything about the track on
              // the right: two lines and the five keys, stacked.
              SizedBox(
                height: art,
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    // The slot is the ring's box; the cover sits 7px inside it,
                    // so the breathing outline stands OFF the artwork.
                    AnimatedBuilder(
                      animation: _pulse,
                      builder: (context, _) => Container(
                        width: art,
                        height: art,
                        decoration: BoxDecoration(
                          borderRadius: BorderRadius.circular(18),
                          border: Border.all(
                            width: c.tickPlaying ? 3 : 0,
                            color: c.tickPlaying
                                ? mix(
                                        accent,
                                        t.dark ? Colors.white : Tokens.brand2,
                                        1 - _pulse.value)
                                    .withValues(alpha: 0.4 + 0.6 * _pulse.value)
                                : Colors.transparent,
                          ),
                        ),
                        padding: const EdgeInsets.all(7),
                        child: ClipRRect(
                          borderRadius: BorderRadius.circular(12),
                          child: ColoredBox(
                            color: accent.withValues(alpha: 0.2),
                            child: (now?.art ?? '').isEmpty
                                ? Icon(Icons.music_note,
                                    size: 34, color: accent)
                                : Image.file(File(now!.art),
                                    fit: BoxFit.cover,
                                    errorBuilder: (_, __, ___) => Icon(
                                        Icons.music_note,
                                        size: 34,
                                        color: accent)),
                          ),
                        ),
                      ),
                    ),
                    const SizedBox(width: 12),
                    Expanded(
                      child: Column(
                        mainAxisAlignment: MainAxisAlignment.center,
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          // Both lines RUN rather than elide — a long title is
                          // readable in a 260px column.
                          SizedBox(
                            height: 19,
                            child: Marquee(
                              text: live ? 'Nothing playing' : now!.title,
                              style: TextStyle(
                                  fontSize: 14,
                                  fontWeight: FontWeight.w800,
                                  color: t.text),
                            ),
                          ),
                          const SizedBox(height: 4),
                          SizedBox(
                            height: 15,
                            child: Marquee(
                              text: live
                                  ? 'Press play for a shuffled library'
                                  : now!.artist,
                              style: TextStyle(fontSize: 11, color: t.textDim),
                            ),
                          ),
                          const SizedBox(height: 10),
                          SizedBox(
                            height: 53,
                            child: HomeTransport(
                              controller: c,
                              accent: accent,
                              spacing: 5,
                              alignment: MainAxisAlignment.start,
                            ),
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(height: 10),
              HomeSeek(
                pos: c.tickPos,
                dur: c.tickDur,
                accent: accent,
                onSeek: (v) => c.send(MusicCmd.seek(secs: v)),
              ),
              // Only while there is a synced lyric to show — an empty band
              // under the seek bar was dead space.
              if (c.tickPlaying && c.activeLyric >= 0) ...[
                const SizedBox(height: 10),
                HomeLyrics(controller: c, accent: accent),
              ],
              const SizedBox(height: 10),
              SizedBox(
                height: 32,
                child: HomePlayerBar(
                  controller: c,
                  accent: accent,
                  queueOpen: _queue,
                  onQueue: () {
                    setState(() => _queue = !_queue);
                    if (_queue) c.refresh();
                  },
                ),
              ),
              // The drop-down pushes the blocks under it down rather than
              // covering them — the rail is a column, and a column has room.
              if (_queue) ...[
                const SizedBox(height: 10),
                Container(
                  height: 210,
                  padding: const EdgeInsets.all(10),
                  decoration: BoxDecoration(
                    color: t.dark
                        ? const Color(0xB30C0C14)
                        : const Color(0xCCFFFFFF),
                    borderRadius: BorderRadius.circular(12),
                    border: Border.all(color: t.glassBorder),
                  ),
                  child: HomeQueue(controller: c, accent: accent),
                ),
              ],
            ],
          ),
        );
      },
    );
  }
}

// ── Welcome: the bar pinned along the bottom of the page ────────────────────

/// The card's own height never changes — 67px, the launch bar's height to the
/// pixel. What moves is how much of it stands above the window floor.
const double kWelcomeBarH = 67;
const double kWelcomeStubH = 26;

class WelcomePlayerBar extends StatefulWidget {
  const WelcomePlayerBar({super.key, required this.width});

  final double width;

  @override
  State<WelcomePlayerBar> createState() => _WelcomePlayerBarState();
}

class _WelcomePlayerBarState extends State<WelcomePlayerBar>
    with SingleTickerProviderStateMixin {
  bool _queue = false;
  late final AnimationController _pulse = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 1240),
  )..repeat(reverse: true);

  @override
  void dispose() {
    _pulse.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = MusicController.instance;
    return AnimatedBuilder(
      animation: c,
      builder: (context, _) {
        final now = c.now;
        // The bar carries its own ink so it reads on every theme: dark ink over
        // the artwork's accent on light themes, white over pink on dark.
        final ink = t.dark ? Colors.white : const Color(0xFF17121F);
        final dim = t.dark
            ? Colors.white.withValues(alpha: 0.69)
            : const Color(0xFF17121F).withValues(alpha: 0.60);
        final accent = t.dark ? Tokens.secMusic : c.accent;
        final live = (now?.title ?? '').isEmpty;
        return AnimatedBuilder(
          animation: _pulse,
          builder: (context, child) => Container(
            width: widget.width,
            height: kWelcomeBarH,
            decoration: BoxDecoration(
              color: t.dark ? t.panel2 : t.panel,
              borderRadius: BorderRadius.circular(kCardRadius),
              // ONE outline, doing both jobs: the hairline IS the live ring —
              // it thickens and breathes while something plays.
              border: Border.all(
                width: c.tickPlaying ? 2.5 : 1,
                color: c.tickPlaying
                    ? mix(accent, t.dark ? Colors.white : Tokens.brand2,
                            _pulse.value)
                        .withValues(alpha: 0.45 + 0.45 * _pulse.value)
                    : accent.withValues(alpha: t.dark ? 0.45 : 0.30),
              ),
            ),
            child: child,
          ),
          child: Padding(
            padding: const EdgeInsets.fromLTRB(14, 0, 16, 0),
            child: Row(
              children: [
                // Artwork — opens the app-wide mini player.
                Hover(
                  onTap: c.toggleMini,
                  builder: (context, hov) => Container(
                    width: 50,
                    height: 50,
                    decoration: BoxDecoration(
                      color: accent.withValues(alpha: 0.25),
                      borderRadius: BorderRadius.circular(12),
                      border: Border.all(color: accent, width: hov ? 2 : 0),
                    ),
                    clipBehavior: Clip.antiAlias,
                    child: (now?.art ?? '').isEmpty
                        ? Icon(Icons.music_note, size: 22, color: accent)
                        : Image.file(File(now!.art),
                            fit: BoxFit.cover,
                            errorBuilder: (_, __, ___) => Icon(Icons.music_note,
                                size: 22, color: accent)),
                  ),
                ),
                const SizedBox(width: 12),
                SizedBox(
                  width: 176,
                  child: Column(
                    mainAxisAlignment: MainAxisAlignment.center,
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      SizedBox(
                        height: 17,
                        child: Marquee(
                          text: live ? 'Nothing playing' : now!.title,
                          style: TextStyle(
                              fontSize: 13,
                              fontWeight: FontWeight.w700,
                              color: ink),
                        ),
                      ),
                      const SizedBox(height: 2),
                      SizedBox(
                        height: 14,
                        child: Marquee(
                          text: live ? '' : now!.artist,
                          style: TextStyle(
                              fontSize: 10.5,
                              fontWeight: FontWeight.w500,
                              color: dim),
                        ),
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 12),
                HomeTransport(controller: c, accent: accent),
                const SizedBox(width: 12),
                // One row, so the seek bar gets to be thick and wide enough to
                // carry both times inside it.
                Expanded(
                  child: FractionallySizedBox(
                    widthFactor: 0.90,
                    child: HomeSeek(
                      pos: c.tickPos,
                      dur: c.tickDur,
                      accent: accent,
                      ink: ink,
                      dim: dim,
                      onSeek: (v) => c.send(MusicCmd.seek(secs: v)),
                    ),
                  ),
                ),
                const SizedBox(width: 12),
                // Queue · mute · volume · zen, at the end of the row.
                SizedBox(
                  width: 246,
                  child: Row(
                    children: [
                      CineBtn(
                        icon: Icons.queue_music,
                        lit: _queue,
                        accent: accent,
                        onTap: () {
                          setState(() => _queue = !_queue);
                          if (_queue) {
                            c.refresh();
                            _showQueue(context, c, accent);
                          }
                        },
                      ),
                      const SizedBox(width: 6),
                      CineBtn(
                        icon: (now?.muted ?? false)
                            ? Icons.volume_off
                            : Icons.volume_up,
                        accent: accent,
                        onTap: () => c.send(const MusicCmd.toggleMute()),
                      ),
                      const SizedBox(width: 6),
                      SizedBox(
                        width: 126,
                        child: HomeVolume(
                          volume: now?.volume ?? 100,
                          muted: now?.muted ?? false,
                          accent: accent,
                          ink: ink,
                          onVolume: (v) =>
                              c.send(MusicCmd.setVolume(volume: v)),
                        ),
                      ),
                      const SizedBox(width: 6),
                      CineBtn(
                        icon: Icons.keyboard_arrow_up,
                        accent: accent,
                        onTap: c.openZen,
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }

  /// The queue drops UP out of the bar. A dialog rather than a hand-placed
  /// overlay: the bar is already at the window floor, and an anchored popup
  /// that has to dodge the edge is more geometry than the list is worth.
  void _showQueue(BuildContext context, MusicController c, Color accent) {
    final t = context.tokens;
    showDialog<void>(
      context: context,
      barrierColor: Colors.transparent,
      builder: (context) => Align(
        alignment: Alignment.bottomCenter,
        child: Padding(
          padding: const EdgeInsets.only(bottom: 96),
          child: Material(
            color: t.dark ? t.panel2 : t.panel,
            borderRadius: BorderRadius.circular(14),
            elevation: 12,
            child: Container(
              width: 316,
              height: 330,
              padding: const EdgeInsets.all(12),
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(14),
                border: Border.all(color: t.outline),
              ),
              child: AnimatedBuilder(
                animation: c,
                builder: (context, _) =>
                    HomeQueue(controller: c, accent: accent, dense: false),
              ),
            ),
          ),
        ),
      ),
    ).then((_) {
      if (mounted) setState(() => _queue = false);
    });
  }
}

// ── Classic: the music rail card ────────────────────────────────────────────

/// MUSIC cap + the real mini player + five circular source tabs.
///
/// The Slint card embeds `MusicMini` / `PodcastMini` / `BookMini` at
/// `embedded: true` rather than cloning them, so a fix to the player is a fix
/// here too. [MiniPlayer] is the port's one player card; this passes it the
/// same flag.
class ClassicMusicCard extends StatelessWidget {
  const ClassicMusicCard({super.key, required this.playerScale});

  final double playerScale;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = MusicController.instance;
    return AnimatedBuilder(
      animation: c,
      builder: (context, _) {
        final mode = c.now?.mode ?? 'idle';
        final tab = switch (mode) {
          'podcast' => 'podcasts',
          'book' => 'audiobooks',
          'radio' => 'radio',
          'youtube' => 'youtube',
          _ => 'mymusic',
        };
        final src = playingSource(mode);
        return Container(
          decoration: context.skin
                  .surface(SurfaceRole.card, radius: kCardRadius) ??
              BoxDecoration(
            // Pink reads far brighter than the other section accents, so this
            // card sits at a third of the standard fill to land at the same
            // perceived darkness.
            color: plateQuiet(Tokens.secMusic),
            borderRadius: BorderRadius.circular(kCardRadius),
            border: Border.all(
              color: plateBorder(Tokens.secMusic, false),
              width: plateBorderW(false),
            ),
          ),
          padding: const EdgeInsets.all(12),
          child: Column(
            children: [
              // Centred title pill naming the source that is playing.
              TitlePill(
                icon: src.icon,
                accent: mode == 'radio' ? kRadio : src.accent,
                name: src.name,
              ),
              const SizedBox(height: 8),
              // The player floats on the card's canvas in its own well.
              Expanded(
                child: Container(
                  decoration: context.skin
                          .surface(SurfaceRole.card, radius: 12) ??
                      BoxDecoration(
                    // Theme-aware fill: white-cream on light themes, the panel
                    // surface on dark — the fixed cream glared.
                    color: t.dark ? t.panel2 : const Color(0xFFFAFAF7),
                    borderRadius: BorderRadius.circular(12),
                    border: Border.all(
                        color: Tokens.secMusic.withValues(alpha: 0.60)),
                  ),
                  clipBehavior: Clip.antiAlias,
                  // The mini's own layout is a fixed 300 x 470 frame times its
                  // scale; a short window would leave the well shorter than
                  // that. `contain` keeps the frame's proportions and fits it
                  // to whatever the rail actually left, which is what Slint's
                  // `width: 100%; height: 100%; uiscale:` does.
                  child: FittedBox(
                    fit: BoxFit.contain,
                    child: SizedBox(
                      width: kMiniSize.width * playerScale,
                      height: kMiniSize.height * playerScale,
                      child: MiniPlayer(
                        controller: c,
                        embedded: true,
                        scale: playerScale,
                      ),
                    ),
                  ),
                ),
              ),
              const SizedBox(height: 8),
              // The five sources, as circular tinted discs. The one that is
              // playing reads brightest.
              SizedBox(
                height: 72,
                child: Row(
                  children: [
                    for (final s in const [
                      (
                        id: 'mymusic',
                        label: 'My Music',
                        icon: Icons.music_note_outlined,
                        tint: Tokens.secMusic,
                        view: 'mymusic'
                      ),
                      (
                        id: 'podcasts',
                        label: 'Podcasts',
                        icon: Icons.mic_none_outlined,
                        tint: kPodcast,
                        view: 'podcasts'
                      ),
                      (
                        id: 'audiobooks',
                        label: 'Audiobooks',
                        icon: Icons.headphones,
                        tint: kAudiobook,
                        view: 'audiobooks'
                      ),
                      (
                        id: 'radio',
                        label: 'Radio',
                        icon: Icons.radio_outlined,
                        tint: kRadio,
                        view: 'radio'
                      ),
                      (
                        id: 'youtube',
                        label: 'YouTube',
                        icon: Icons.play_arrow_rounded,
                        tint: kYoutube,
                        view: 'youtube'
                      ),
                    ])
                      Expanded(
                        child: HomeCircleTab(
                          icon: s.icon,
                          label: s.label,
                          tint: s.tint,
                          active: tab == s.id,
                          onTap: () => ShellController.instance
                              .goTab(Section.music, s.view),
                        ),
                      ),
                  ],
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

/// One circular source button — a tinted icon disc with a solid pill under it.
/// Active = solid fill and a white inner ring; idle = a tinted wash.
class HomeCircleTab extends StatelessWidget {
  const HomeCircleTab({
    super.key,
    required this.icon,
    required this.label,
    required this.tint,
    required this.active,
    required this.onTap,
  });

  final IconData icon;
  final String label;
  final Color tint;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => Hover(
        onTap: onTap,
        builder: (context, hov) => Column(
          mainAxisAlignment: MainAxisAlignment.center,
          mainAxisSize: MainAxisSize.min,
          children: [
            AnimatedContainer(
              duration: const Duration(milliseconds: 120),
              width: 48,
              height: 48,
              decoration: context.skin.control(
                    active: active,
                    hovered: hov,
                    tint: tint,
                    radius: 24,
                  ) ??
                  BoxDecoration(
                color: active ? tint : wash(tint, hov ? 0.30 : 0.15),
                shape: BoxShape.circle,
                border: Border.all(
                  color: active
                      ? darker(tint, 0.45)
                      : tint.withValues(alpha: 0.40),
                  width: active ? 2 : 1,
                ),
                boxShadow: active
                    ? [
                        BoxShadow(
                            color: tint.withValues(alpha: 0.45),
                            blurRadius: 14),
                      ]
                    : null,
              ),
              child: Center(
                child: Container(
                  width: 40,
                  height: 40,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    // The whiter second ring, drawn inside so the footprint is
                    // unchanged.
                    border: Border.all(
                      color: active ? Colors.white : Colors.transparent,
                      width: active ? 2 : 0,
                    ),
                  ),
                  child: Icon(context.skin.icon(icon),
                      size: 22,
                      color: !active
                          ? tint
                          : context.skin.isStandard
                              ? Colors.white
                              : (context.skin.activeInk ?? tint)),
                ),
              ),
            ),
            const SizedBox(height: 5),
            Container(
              height: 18,
              padding: const EdgeInsets.symmetric(horizontal: 9),
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: tint,
                borderRadius: BorderRadius.circular(9),
                border:
                    Border.all(color: Colors.white, width: active ? 1.5 : 0),
              ),
              child: Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: 10,
                  fontWeight: active ? FontWeight.w800 : FontWeight.w700,
                  color: Colors.white,
                ),
              ),
            ),
          ],
        ),
      );
}

/// The header spectrum Classic and Welcome ride while something plays, with the
/// active synced line as a white pill over it.
class HeaderViz extends StatelessWidget {
  const HeaderViz({
    super.key,
    required this.controller,
    required this.on,
    required this.lyrics,
  });

  final MusicController controller;

  /// The spectrum itself. The lyric pill shows independently of it.
  final bool on;
  final bool lyrics;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final mode = c.now?.mode ?? 'idle';
    // Only My Music and Radio drive it — podcasts, audiobooks and YouTube do
    // not. Radio additionally has no synced lyrics to show.
    final allowed = c.tickPlaying && (mode == 'radio' || mode == 'music');
    if (!allowed) return const SizedBox.shrink();
    final lines = c.state?.lyrics ?? const <LyricLine>[];
    final i = c.activeLyric;
    final showLyric = lyrics && mode == 'music' && i >= 0 && lines.isNotEmpty;
    return Stack(
      alignment: Alignment.center,
      children: [
        if (on) VizView(style: c.visStyle, playing: true),
        if (showLyric)
          Container(
            constraints: const BoxConstraints(maxWidth: 420),
            height: 30,
            padding: const EdgeInsets.symmetric(horizontal: 14),
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: Colors.white,
              borderRadius: BorderRadius.circular(15),
              border: Border.all(color: const Color(0x22000000)),
            ),
            child: Text(
              lines[i].text,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                  fontSize: 13,
                  fontWeight: FontWeight.w700,
                  color: Color(0xFF14141B)),
            ),
          ),
      ],
    );
  }
}
