// The zen player: the whole window, one record.
//
// It is not a bigger bar. The bar exists so you can keep browsing; this exists
// so you can stop. Hence the giant title, the visualizer given the middle of
// the screen, and lyrics that are the only thing there when you switch the
// bars off — with the queue pushed out to a panel you have to ask for.
//
// It renders above every section, not inside the Music page, because the thing
// playing does not stop being the thing playing when you go and look at your
// photos.

import 'dart:io';
import 'dart:ui' show ImageFilter;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_viz.dart';
import 'music_widgets.dart';
import 'player_bar.dart' show Transport;
import 'player_widgets.dart';

class ZenPlayer extends StatelessWidget {
  const ZenPlayer({super.key, required this.controller});

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
    // Lyrics only exist for library audio. With them impossible AND the
    // visualizer off, the middle of the page would be a hole — so the bars come
    // back for the drawing without touching the stored preference, which
    // belongs to the tracks that do have words.
    final vizShown = controller.visOn || !library;
    final lyricsShown = controller.zenLyrics && library;
    final lyricsBig = lyricsShown && !vizShown;

    return Material(
      color: t.nCanvas,
      child: CallbackShortcuts(
        bindings: <ShortcutActivator, VoidCallback>{
          const SingleActivator(LogicalKeyboardKey.escape): () =>
              controller.zenPanel.isEmpty
                  ? controller.closeZen()
                  : controller.setZenPanel(''),
          const SingleActivator(LogicalKeyboardKey.space): () =>
              controller.send(const MusicCmd.playPause()),
          const SingleActivator(LogicalKeyboardKey.keyL):
              controller.toggleZenLyrics,
          const SingleActivator(LogicalKeyboardKey.keyV): () =>
              controller.setVisOn(!controller.visOn),
          const SingleActivator(LogicalKeyboardKey.keyQ): () =>
              controller.setZenPanel('queue'),
          const SingleActivator(LogicalKeyboardKey.keyN): () =>
              controller.send(const MusicCmd.next()),
          const SingleActivator(LogicalKeyboardKey.keyP): () =>
              controller.send(const MusicCmd.prev()),
          const SingleActivator(LogicalKeyboardKey.keyM): () =>
              controller.send(const MusicCmd.toggleMute()),
          const SingleActivator(LogicalKeyboardKey.arrowLeft): () =>
              _nudge(-5, live),
          const SingleActivator(LogicalKeyboardKey.arrowRight): () =>
              _nudge(5, live),
        },
        child: Focus(
          autofocus: true,
          child: Stack(
            children: [
              // The cover, blurred out to fill the room.
              if (now.art.isNotEmpty)
                Positioned.fill(
                  child: ImageFiltered(
                    imageFilter: ImageFilter.blur(sigmaX: 60, sigmaY: 60),
                    child: Image.file(
                      File(now.art),
                      fit: BoxFit.cover,
                      opacity: const AlwaysStoppedAnimation(0.55),
                      errorBuilder: (_, __, ___) => const SizedBox.shrink(),
                    ),
                  ),
                ),
              Positioned.fill(
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    gradient: LinearGradient(
                      begin: Alignment.topCenter,
                      end: Alignment.bottomCenter,
                      colors: [
                        t.nCanvas.withValues(alpha: 0.72),
                        t.nCanvas.withValues(alpha: 0.94),
                      ],
                    ),
                  ),
                ),
              ),
              Padding(
                padding: const EdgeInsets.all(48),
                child: Column(
                  children: [
                    _TopBar(controller: controller),
                    const SizedBox(height: 24),
                    _Title(controller: controller, now: now, live: live),
                    if (vizShown)
                      Expanded(
                        child: Padding(
                          padding: const EdgeInsets.symmetric(vertical: 10),
                          child: FractionallySizedBox(
                            widthFactor: 0.86,
                            child: VizView(
                              style: controller.visStyle,
                              playing: controller.tickPlaying,
                            ),
                          ),
                        ),
                      ),
                    if (lyricsShown)
                      _ZenLyrics(controller: controller, big: lyricsBig),
                    if (!vizShown && !lyricsShown) const Spacer(),
                    const SizedBox(height: 24),
                    if (!live)
                      SeekPill(
                        pos: controller.tickPos,
                        dur: controller.tickDur,
                        accent: accent,
                        scale: 1.25,
                        onSeek: (v) => controller.send(MusicCmd.seek(secs: v)),
                      ),
                    const SizedBox(height: 16),
                    _ZenControls(
                      controller: controller,
                      now: now,
                      live: live,
                      library: library,
                    ),
                  ],
                ),
              ),
              if (controller.zenPanel == 'queue')
                Positioned(
                  right: 0,
                  top: 0,
                  bottom: 0,
                  width: 400,
                  child: _QueuePanel(controller: controller),
                ),
            ],
          ),
        ),
      ),
    );
  }

  void _nudge(double by, bool live) {
    if (live) return;
    controller.send(MusicCmd.seek(
        secs: (controller.tickPos + by).clamp(0, controller.tickDur)));
  }
}

class _TopBar extends StatelessWidget {
  const _TopBar({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      height: 80,
      child: Row(
        children: [
          PlayerBtn(
            icon: Icons.chevron_left,
            size: 72,
            iconSize: 40,
            accent: controller.accent,
            onTap: controller.closeZen,
          ),
          Expanded(
            child: Center(
              child: Text(
                'ZEN',
                style: TextStyle(
                  fontSize: 13,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 3,
                  color: t.nInk2,
                ),
              ),
            ),
          ),
          PlayerBtn(
            icon: Icons.picture_in_picture_alt,
            tip: 'Mini player',
            size: 56,
            iconSize: 22,
            accent: controller.accent,
            onTap: () {
              controller.closeZen();
              controller.toggleMini();
            },
          ),
        ],
      ),
    );
  }
}

class _Title extends StatelessWidget {
  const _Title({
    required this.controller,
    required this.now,
    required this.live,
  });

  final MusicController controller;
  final NowPlaying now;
  final bool live;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final title = live && now.streamTitle.isNotEmpty
        ? now.streamTitle
        : (now.title.isEmpty ? 'Nothing playing' : now.title);
    final sub =
        [now.artist, now.album].where((s) => s.isNotEmpty).join('   ·   ');
    return Column(
      children: [
        SizedBox(
          height: 54,
          child: Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              if (now.art.isNotEmpty) ...[
                MusicArt(
                  controller: controller,
                  kind: 'track',
                  artKey: '${now.itemId}',
                  direct: now.art,
                  size: 46,
                  radius: 10,
                ),
                const SizedBox(width: 14),
              ],
              Flexible(
                child: Text(
                  title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 40, fontWeight: FontWeight.w800, color: t.nInk),
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: 6),
        Text(
          sub,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(fontSize: 16, color: t.nInk2),
        ),
      ],
    );
  }
}

/// Three lines: what was sung, what is being sung, what comes next.
class _ZenLyrics extends StatelessWidget {
  const _ZenLyrics({required this.controller, required this.big});

  final MusicController controller;
  final bool big;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final lines = controller.state?.lyrics ?? const <LyricLine>[];
    final a = controller.activeLyric;
    Widget quiet(String text) => Text(
          text,
          textAlign: TextAlign.center,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(fontSize: big ? 26 : 18, color: t.nInk3),
        );
    final body = Column(
      mainAxisAlignment: MainAxisAlignment.center,
      mainAxisSize: MainAxisSize.min,
      children: [
        quiet(a > 0 ? lines[a - 1].text : ''),
        SizedBox(height: big ? 22 : 8),
        Text(
          a >= 0 && a < lines.length
              ? lines[a].text
              : (lines.isEmpty
                  ? 'No synced lyrics — fetch them from the lyrics panel'
                  : ''),
          textAlign: TextAlign.center,
          maxLines: 2,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(
            fontSize: big ? 44 : 28,
            fontWeight: FontWeight.w700,
            color: Tokens.secMusic,
          ),
        ),
        SizedBox(height: big ? 22 : 8),
        quiet(a >= 0 && a + 1 < lines.length ? lines[a + 1].text : ''),
      ],
    );
    return big
        ? Expanded(
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 56),
              child: Center(child: body),
            ),
          )
        : Padding(
            padding: const EdgeInsets.symmetric(vertical: 10),
            child: body,
          );
  }
}

class _ZenControls extends StatelessWidget {
  const _ZenControls({
    required this.controller,
    required this.now,
    required this.live,
    required this.library,
  });

  final MusicController controller;
  final NowPlaying now;
  final bool live;
  final bool library;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final accent = c.accent;
    return Wrap(
      alignment: WrapAlignment.center,
      crossAxisAlignment: WrapCrossAlignment.center,
      spacing: 14,
      runSpacing: 8,
      children: [
        if (library)
          PlayerBtn(
            icon: Icons.lyrics_outlined,
            tip: 'Lyrics',
            size: 44,
            iconSize: 20,
            active: c.zenLyrics,
            accent: accent,
            onTap: c.toggleZenLyrics,
          ),
        PlayerBtn(
          icon: Icons.graphic_eq,
          tip: c.visOn ? 'Visualizer off' : 'Visualizer on',
          size: 44,
          iconSize: 18,
          active: c.visOn,
          accent: accent,
          onTap: () => c.setVisOn(!c.visOn),
        ),
        if (library)
          PlayerBtn(
            icon: now.loved ? Icons.favorite : Icons.favorite_border,
            size: 44,
            iconSize: 19,
            active: now.loved,
            accent: accent,
            onTap: () => c.send(MusicCmd.love(itemId: now.itemId)),
          ),
        Transport(controller: c, mode: now.mode, live: live, scale: 1.3),
        VolPill(
          volume: now.volume,
          muted: now.muted,
          accent: accent,
          width: 172,
          scale: 1.2,
          onVolume: (v) => c.send(MusicCmd.setVolume(volume: v)),
          onMute: () => c.send(const MusicCmd.toggleMute()),
        ),
        PlayerBtn(
          icon: Icons.queue_music,
          tip: 'Up next',
          size: 44,
          iconSize: 20,
          active: c.zenPanel == 'queue',
          accent: accent,
          onTap: () => c.setZenPanel('queue'),
        ),
      ],
    );
  }
}

class _QueuePanel extends StatelessWidget {
  const _QueuePanel({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final queue = controller.state?.queue ?? const <Track>[];
    return Material(
      color: t.nCard,
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Text('Up next',
                    style: TextStyle(
                        fontSize: 20,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
                const Spacer(),
                PlayerBtn(
                  icon: Icons.close,
                  size: 32,
                  iconSize: 16,
                  accent: controller.accent,
                  onTap: () => controller.setZenPanel(''),
                ),
              ],
            ),
            const SizedBox(height: 14),
            Expanded(
              child: queue.isEmpty
                  ? Text('Queue is empty.',
                      style: TextStyle(fontSize: 14, color: t.nInk2))
                  : ListView.builder(
                      itemCount: queue.length,
                      itemBuilder: (_, i) => TrackRow(
                        controller: controller,
                        track: queue[i],
                        index: i,
                        dense: true,
                        onPlay: () => controller.playQueueAt(i),
                      ),
                    ),
            ),
          ],
        ),
      ),
    );
  }
}
