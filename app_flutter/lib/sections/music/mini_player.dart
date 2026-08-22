// The floating mini player — the one that follows you out of the section.
//
// It is a card inside the window, not an OS window: draggable anywhere,
// resizable from the corner, and collapsible to a bubble at the edge when it is
// in the way. That is deliberate. A second native window would need its own
// renderer and its own copy of the state; this needs neither, and it can sit
// over the Photos grid while the album keeps playing.
//
// The square in the middle flips: the spinning record, the queue, or the words.

import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_widgets.dart';
import 'player_bar.dart' show Transport;
import 'player_widgets.dart';

const Size kMiniSize = Size(300, 468);

class MiniPlayer extends StatelessWidget {
  const MiniPlayer({super.key, required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = controller.state;
    final now = st?.now;
    if (st == null || now == null) return const SizedBox.shrink();

    final accent = controller.accent;
    final live = now.mode == 'radio' || controller.tickDur <= 0;
    final library = now.itemId != 0 && now.mode == 'music';
    final s = controller.miniScale;

    return Container(
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(18 * s),
        // The ring is not decoration: this thing floats over arbitrary content,
        // and without a hard edge it reads as part of whatever is behind it.
        border: GradientBoxBorder(
          gradient: const LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [Color(0xFFEC4899), Color(0xFF8B5CF6), Color(0xFF06B6D4)],
            stops: [0.0, 0.55, 1.0],
          ),
          width: 2 * s,
        ),
        boxShadow: const [
          BoxShadow(color: Color(0xE6000000), blurRadius: 42, spreadRadius: -6),
        ],
      ),
      clipBehavior: Clip.antiAlias,
      child: DecoratedBox(
        decoration: artWash(accent, from: Alignment.topCenter),
        child: Padding(
          padding: EdgeInsets.all(14 * s),
          child: Column(
            children: [
              _Header(controller: controller, now: now, scale: s),
              SizedBox(height: 6 * s),
              _SecondLine(
                  controller: controller, now: now, live: live, scale: s),
              SizedBox(height: 10 * s),
              Expanded(
                child: _Face(
                  controller: controller,
                  now: now,
                  library: library,
                  scale: s,
                ),
              ),
              SizedBox(height: 10 * s),
              if (!live)
                SeekPill(
                  pos: controller.tickPos,
                  dur: controller.tickDur,
                  accent: accent,
                  scale: s,
                  onSeek: (v) => controller.send(MusicCmd.seek(secs: v)),
                ),
              SizedBox(height: 8 * s),
              Transport(
                controller: controller,
                mode: now.mode,
                live: live,
                compact: true,
                scale: 0.86 * s,
              ),
              SizedBox(height: 6 * s),
              Row(
                children: [
                  Expanded(
                    child: VolPill(
                      volume: now.volume,
                      muted: now.muted,
                      accent: accent,
                      width: double.infinity,
                      scale: s,
                      onVolume: (v) =>
                          controller.send(MusicCmd.setVolume(volume: v)),
                      onMute: () =>
                          controller.send(const MusicCmd.toggleMute()),
                    ),
                  ),
                  if (library)
                    PlayerBtn(
                      icon: Icons.queue_music,
                      tip: 'Queue',
                      size: 30 * s,
                      iconSize: 15 * s,
                      active: controller.miniFace == 'queue',
                      accent: accent,
                      onTap: () => controller.setMiniFace('queue'),
                    ),
                  if (library)
                    PlayerBtn(
                      icon: Icons.lyrics_outlined,
                      tip: 'Lyrics',
                      size: 30 * s,
                      iconSize: 15 * s,
                      active: controller.miniFace == 'lyrics',
                      accent: accent,
                      onTap: () => controller.setMiniFace('lyrics'),
                    ),
                  PlayerBtn(
                    icon: Icons.open_in_full,
                    tip: 'Zen player',
                    size: 30 * s,
                    iconSize: 15 * s,
                    accent: accent,
                    onTap: controller.openZen,
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _Header extends StatelessWidget {
  const _Header(
      {required this.controller, required this.now, required this.scale});

  final MusicController controller;
  final NowPlaying now;
  final double scale;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      children: [
        PlayerBtn(
          icon: Icons.close,
          tip: 'Close',
          size: 26 * scale,
          iconSize: 13 * scale,
          accent: controller.accent,
          onTap: controller.toggleMini,
        ),
        Expanded(
          child: SizedBox(
            height: 20 * scale,
            child: Marquee(
              text: now.title.isEmpty ? 'Nothing playing' : now.title,
              centred: true,
              style: TextStyle(
                fontSize: 15 * scale,
                fontWeight: FontWeight.w700,
                color: t.nInk,
              ),
            ),
          ),
        ),
        PlayerBtn(
          icon: Icons.chevron_right,
          tip: 'Dock as a bubble',
          size: 26 * scale,
          iconSize: 14 * scale,
          accent: controller.accent,
          onTap: () => controller.setMiniBubble(true),
        ),
      ],
    );
  }
}

class _SecondLine extends StatelessWidget {
  const _SecondLine({
    required this.controller,
    required this.now,
    required this.live,
    required this.scale,
  });

  final MusicController controller;
  final NowPlaying now;
  final bool live;
  final double scale;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (live) {
      // On a station the title line is the station; this line is whatever the
      // stream says is playing, which is the only place the song exists.
      return Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Flexible(
            child: Text(
              now.streamTitle.isEmpty ? 'On air' : now.streamTitle,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(fontSize: 11 * scale, color: t.nInk2),
            ),
          ),
          SizedBox(width: 6 * scale),
          Container(
            height: 16 * scale,
            padding: EdgeInsets.symmetric(horizontal: 7 * scale),
            decoration: BoxDecoration(
              gradient: const LinearGradient(
                  colors: [Color(0xFF22C55E), Color(0xFF16A34A)]),
              borderRadius: BorderRadius.circular(8 * scale),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Container(
                  width: 5 * scale,
                  height: 5 * scale,
                  decoration: const BoxDecoration(
                      color: Colors.white, shape: BoxShape.circle),
                ),
                SizedBox(width: 4 * scale),
                Text('LIVE',
                    style: TextStyle(
                        fontSize: 9 * scale,
                        fontWeight: FontWeight.w800,
                        color: Colors.white)),
              ],
            ),
          ),
        ],
      );
    }
    final text = now.title.isEmpty
        ? 'Pick a track'
        : (now.mode == 'book'
            ? now.artist
            : [now.artist, now.album].where((s) => s.isNotEmpty).join('  ·  '));
    return SizedBox(
      height: 16 * scale,
      child: Marquee(
        text: text,
        centred: true,
        speed: 26,
        style: TextStyle(fontSize: 12 * scale, color: t.nInk2),
      ),
    );
  }
}

/// The square: record, queue or lyrics.
class _Face extends StatelessWidget {
  const _Face({
    required this.controller,
    required this.now,
    required this.library,
    required this.scale,
  });

  final MusicController controller;
  final NowPlaying now;
  final bool library;
  final double scale;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget body;
    switch (controller.miniFace) {
      case 'queue':
        final queue = controller.state?.queue ?? const <Track>[];
        body = queue.isEmpty
            ? Center(
                child: Text('Queue is empty.',
                    style: TextStyle(fontSize: 12 * scale, color: t.nInk2)))
            : ListView.builder(
                padding: EdgeInsets.all(4 * scale),
                itemCount: queue.length,
                itemBuilder: (_, i) => TrackRow(
                  controller: controller,
                  track: queue[i],
                  index: i,
                  dense: true,
                  showArt: false,
                  onPlay: () => controller.playQueueAt(i),
                ),
              );
      case 'lyrics':
        body = _MiniLyrics(controller: controller, scale: scale);
      default:
        body = _Vinyl(controller: controller, now: now, scale: scale);
    }
    return DecoratedBox(
      decoration: BoxDecoration(
        color: controller.miniFace.isEmpty ? Colors.transparent : t.nTile,
        borderRadius: BorderRadius.circular(12 * scale),
      ),
      child: ClipRRect(
        borderRadius: BorderRadius.circular(12 * scale),
        child: body,
      ),
    );
  }
}

/// The record, turning while it plays. A cover in a square is a thumbnail; a
/// cover on a spinning disc is a player.
class _Vinyl extends StatefulWidget {
  const _Vinyl({
    required this.controller,
    required this.now,
    required this.scale,
  });

  final MusicController controller;
  final NowPlaying now;
  final double scale;

  @override
  State<_Vinyl> createState() => _VinylState();
}

class _VinylState extends State<_Vinyl> with SingleTickerProviderStateMixin {
  late final AnimationController _spin = AnimationController(
    vsync: this,
    duration: const Duration(seconds: 6),
  );

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _sync();
  }

  @override
  void didUpdateWidget(_Vinyl old) {
    super.didUpdateWidget(old);
    _sync();
  }

  void _sync() {
    final playing = widget.controller.tickPlaying;
    if (playing && !_spin.isAnimating) {
      _spin.repeat();
    } else if (!playing && _spin.isAnimating) {
      _spin.stop();
    }
  }

  @override
  void dispose() {
    _spin.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final accent = widget.controller.accent;
    return LayoutBuilder(
      builder: (context, box) {
        final d = math.min(box.maxWidth, box.maxHeight);
        return Center(
          child: RotationTransition(
            turns: _spin,
            child: Container(
              width: d,
              height: d,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                gradient: RadialGradient(
                  colors: [
                    const Color(0xFF1A1A20),
                    const Color(0xFF0D0D12),
                    accent.withValues(alpha: 0.35),
                    const Color(0xFF0D0D12),
                  ],
                  stops: const [0.0, 0.55, 0.72, 1.0],
                ),
                boxShadow: [
                  BoxShadow(
                      color: accent.withValues(alpha: 0.28),
                      blurRadius: 24 * widget.scale),
                ],
              ),
              child: Center(
                child: ClipOval(
                  child: SizedBox(
                    width: d * 0.46,
                    height: d * 0.46,
                    child: MusicArt(
                      controller: widget.controller,
                      kind: 'track',
                      artKey: '${widget.now.itemId}',
                      direct: widget.now.art,
                      size: d * 0.46,
                      radius: 0,
                    ),
                  ),
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

class _MiniLyrics extends StatelessWidget {
  const _MiniLyrics({required this.controller, required this.scale});

  final MusicController controller;
  final double scale;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final lines = controller.state?.lyrics ?? const <LyricLine>[];
    if (lines.isEmpty) {
      return Center(
        child: Padding(
          padding: EdgeInsets.all(12 * scale),
          child: Text(
            'No synced lyrics for this track.',
            textAlign: TextAlign.center,
            style: TextStyle(fontSize: 12 * scale, color: t.nInk2),
          ),
        ),
      );
    }
    final a = controller.activeLyric;
    Widget line(int i, double size, Color colour, FontWeight w) => Text(
          i >= 0 && i < lines.length ? lines[i].text : '',
          textAlign: TextAlign.center,
          maxLines: 2,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(fontSize: size, color: colour, fontWeight: w),
        );
    return Padding(
      padding: EdgeInsets.all(12 * scale),
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          line(a - 1, 12 * scale, t.nInk3, FontWeight.w400),
          SizedBox(height: 10 * scale),
          line(a, 17 * scale, Tokens.secMusic, FontWeight.w700),
          SizedBox(height: 10 * scale),
          line(a + 1, 12 * scale, t.nInk3, FontWeight.w400),
        ],
      ),
    );
  }
}

/// The collapsed mini: a puck at the window edge that keeps the cover and the
/// play button in reach without covering anything.
class MiniBubble extends StatelessWidget {
  const MiniBubble({super.key, required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final now = controller.state?.now;
    if (now == null) return const SizedBox.shrink();
    return Material(
      color: t.nCard,
      elevation: 12,
      borderRadius: const BorderRadius.horizontal(left: Radius.circular(28)),
      clipBehavior: Clip.antiAlias,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(10, 8, 6, 8),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            InkWell(
              onTap: () => controller.setMiniBubble(false),
              customBorder: const CircleBorder(),
              child: ClipOval(
                child: SizedBox(
                  width: 40,
                  height: 40,
                  child: MusicArt(
                    controller: controller,
                    kind: 'track',
                    artKey: '${now.itemId}',
                    direct: now.art,
                    size: 40,
                    radius: 0,
                  ),
                ),
              ),
            ),
            const SizedBox(width: 4),
            PlayerBtn(
              icon: controller.tickPlaying ? Icons.pause : Icons.play_arrow,
              size: 34,
              iconSize: 18,
              accent: controller.accent,
              onTap: () => controller.send(const MusicCmd.playPause()),
            ),
          ],
        ),
      ),
    );
  }
}

/// A gradient border, which BoxBorder cannot do on its own.
class GradientBoxBorder extends BoxBorder {
  const GradientBoxBorder({required this.gradient, this.width = 1});

  final Gradient gradient;
  final double width;

  @override
  BorderSide get top => BorderSide(width: width);
  @override
  BorderSide get bottom => BorderSide(width: width);

  @override
  EdgeInsetsGeometry get dimensions => EdgeInsets.all(width);

  @override
  bool get isUniform => true;

  @override
  void paint(
    Canvas canvas,
    Rect rect, {
    TextDirection? textDirection,
    BoxShape shape = BoxShape.rectangle,
    BorderRadius? borderRadius,
  }) {
    final paint = Paint()
      ..strokeWidth = width
      ..shader = gradient.createShader(rect)
      ..style = PaintingStyle.stroke;
    final inner = rect.deflate(width / 2);
    if (borderRadius != null) {
      canvas.drawRRect(borderRadius.toRRect(inner), paint);
    } else if (shape == BoxShape.circle) {
      canvas.drawCircle(inner.center, inner.shortestSide / 2, paint);
    } else {
      canvas.drawRect(inner, paint);
    }
  }

  @override
  ShapeBorder scale(double t) =>
      GradientBoxBorder(gradient: gradient, width: width * t);
}
