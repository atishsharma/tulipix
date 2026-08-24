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

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_viz.dart';
import 'music_widgets.dart';
import 'player_bar.dart' show Transport;
import 'player_widgets.dart';

class ZenPlayer extends StatelessWidget {
  const ZenPlayer({super.key, required this.controller});

  final MusicController controller;

  /// Zen opens dark, whatever the app is.
  ///
  /// The page is a photograph with type over it: the light palette puts near
  /// black text on a 50%-white pane over someone's album cover, and which of
  /// those wins depends entirely on the record. So dark is the *opening*
  /// state, not a lock — the button at the top right cycles the app's theme
  /// and, from the first press onward, this page wears it too. Otherwise the
  /// one control on the page would have had no visible effect on the page.
  @override
  Widget build(BuildContext context) {
    final rm = context.tokens.reduceMotion;
    final theme = ShellController.instance.theme;
    final tokens = !controller.zenThemed
        ? Tokens.dark(reduceMotion: rm)
        : switch (theme) {
            'light' => Tokens.light(reduceMotion: rm),
            'extra-dark' => Tokens.dark(oled: true, reduceMotion: rm),
            _ => Tokens.dark(reduceMotion: rm),
          };
    return Theme(
      data: tulipixTheme(tokens),
      child: Builder(builder: _body),
    );
  }

  Widget _body(BuildContext context) {
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
    // Auto-show. `zenLyrics` starts on, so a library track that has words
    // shows them without anyone pressing L; a track that has none draws
    // nothing rather than an empty three-line hole.
    final lyricsShown =
        controller.zenLyrics && library && (st.lyrics.isNotEmpty);
    final lyricsBig = lyricsShown && !vizShown;

    return Material(
      color: t.nCanvas,
      child: CallbackShortcuts(
        bindings: <ShortcutActivator, VoidCallback>{
          // One Escape always exits, closing any open panel with it — Slint's
          // zen focus scope does exactly that. Two presses to get out of a
          // fullscreen page is one too many. Space is not here: it is handled
          // for the whole app in music_overlay.dart, off the keyboard rather
          // than off the focus chain.
          const SingleActivator(LogicalKeyboardKey.escape): controller.closeZen,
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
              // The cover, sharp and full-bleed. It was blurred at sigma 60,
              // which is a texture rather than a picture: you could tell the
              // record was mostly blue. Glass, not frost — the art stays the
              // art and the page sits over it.
              if (now.art.isNotEmpty)
                Positioned.fill(
                  child: Image.file(
                    File(now.art),
                    fit: BoxFit.cover,
                    errorBuilder: (_, __, ___) => const SizedBox.shrink(),
                  ),
                ),
              // The glass. Half the canvas colour, so the cover reads through
              // it at about half strength everywhere — the panel is what the
              // controls stand on, and the art is what it stands over.
              Positioned.fill(
                child: ColoredBox(
                  color: t.nCanvas.withValues(alpha: 0.50),
                ),
              ),
              Padding(
                padding: const EdgeInsets.all(28),
                child: Column(
                  children: [
                    // Outside the pane, on the artwork itself: Back and the
                    // theme are the two things you reach for to *leave*, and
                    // they should not look like part of what you are looking
                    // at. Both carry a fill of their own so they stay legible
                    // over whatever the cover happens to be.
                    _TopBar(controller: controller),
                    const SizedBox(height: 16),
                    Expanded(
                      child: Container(
                        decoration: BoxDecoration(
                          // A second pane, inset, so there is a rim of
                          // untouched artwork all the way round: without it
                          // "floating above" is a claim the page never makes
                          // good on.
                          color: t.nCanvas.withValues(alpha: 0.43),
                          borderRadius: BorderRadius.circular(24),
                          border:
                              Border.all(color: t.nInk.withValues(alpha: 0.10)),
                        ),
                        // No padding on the pane itself — the bars run to its
                        // edges, and everything that is type pads itself.
                        clipBehavior: Clip.antiAlias,
                        child: Column(
                          children: [
                            const SizedBox(height: 28),
                            Padding(
                              padding:
                                  const EdgeInsets.symmetric(horizontal: 28),
                              child: _Title(
                                  controller: controller, now: now, live: live),
                            ),
                            if (vizShown)
                              Expanded(
                                child: Padding(
                                  padding:
                                      const EdgeInsets.symmetric(vertical: 10),
                                  child: VizView(
                                    style: controller.visStyle,
                                    playing: controller.tickPlaying,
                                  ),
                                ),
                              ),
                            if (lyricsShown)
                              Padding(
                                padding:
                                    const EdgeInsets.symmetric(horizontal: 28),
                                child: _ZenLyrics(
                                    controller: controller, big: lyricsBig),
                              ),
                            if (!vizShown && !lyricsShown) const Spacer(),
                            Padding(
                              padding:
                                  const EdgeInsets.fromLTRB(28, 24, 28, 28),
                              child: Column(
                                mainAxisSize: MainAxisSize.min,
                                children: [
                                  if (!live)
                                    SeekPill(
                                      pos: controller.tickPos,
                                      dur: controller.tickDur,
                                      accent: accent,
                                      scale: 1.25,
                                      onSeek: (v) => controller
                                          .send(MusicCmd.seek(secs: v)),
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
                          ],
                        ),
                      ),
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
    final shell = ShellController.instance;
    // Both discs get a fill: they sit on the artwork rather than on the pane,
    // and a bare glyph over a photograph is a glyph you cannot find.
    Widget disc(Widget child) => DecoratedBox(
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: t.nCanvas.withValues(alpha: 0.62),
            border: Border.all(color: t.nInk.withValues(alpha: 0.14)),
            boxShadow: const [
              BoxShadow(color: Color(0x55000000), blurRadius: 18),
            ],
          ),
          child: child,
        );
    return SizedBox(
      height: 72,
      child: Row(
        children: [
          disc(PlayerBtn(
            icon: Icons.chevron_left,
            size: 64,
            iconSize: 34,
            accent: controller.accent,
            onTap: controller.closeZen,
          )),
          Expanded(
            child: Center(
              child: Text(
                switch (shell.theme) {
                  'light' => 'ZEN  ·  LIGHT',
                  'extra-dark' => 'ZEN  ·  OLED',
                  _ => 'ZEN  ·  DARK',
                },
                style: TextStyle(
                  fontSize: 13,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 3,
                  color: t.nInk2,
                ),
              ),
            ),
          ),
          // The app theme, not the mini. Slint's zen top-right cycles
          // light → dark → OLED, and the mini is a thing you open to leave
          // the page — putting it at the top of the page you are on was the
          // port's own idea.
          disc(PlayerBtn(
            icon: switch (shell.theme) {
              'light' => Icons.light_mode,
              'extra-dark' => Icons.star_outline,
              _ => Icons.dark_mode,
            },
            tip: 'App theme',
            size: 64,
            iconSize: 32,
            active: shell.theme == 'extra-dark',
            accent: controller.accent,
            onTap: () {
              shell.cycleTheme();
              controller.takeZenTheme();
            },
          )),
        ],
      ),
    );
  }
}

/// The title block. Both lines are links, as everywhere else: the title goes
/// to the album, the subtitle to the artist, and opening either drops out of
/// fullscreen on the way — see `MusicController.openNowDetail`.
class _Title extends StatefulWidget {
  const _Title({
    required this.controller,
    required this.now,
    required this.live,
  });

  final MusicController controller;
  final NowPlaying now;
  final bool live;

  @override
  State<_Title> createState() => _TitleState();
}

class _TitleState extends State<_Title> {
  bool _titleHover = false;
  bool _subHover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final now = widget.now;
    final linked = now.itemId != 0 && now.mode == 'music';
    final title = widget.live && now.streamTitle.isNotEmpty
        ? now.streamTitle
        : (now.title.isEmpty ? 'Nothing playing' : now.title);
    final sub =
        [now.artist, now.album].where((s) => s.isNotEmpty).join('   ·   ');

    Widget link({
      required Widget child,
      required void Function(bool) onHover,
      required VoidCallback onTap,
    }) {
      if (!linked) return child;
      return MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => onHover(true),
        onExit: (_) => onHover(false),
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: onTap,
          child: child,
        ),
      );
    }

    return Column(
      children: [
        link(
          onHover: (v) => setState(() => _titleHover = v),
          onTap: () => widget.controller.openNowDetail(album: true),
          child: SizedBox(
            height: 54,
            child: Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                if (now.art.isNotEmpty) ...[
                  MusicArt(
                    controller: widget.controller,
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
                      fontSize: 40,
                      fontWeight: FontWeight.w800,
                      color: _titleHover && linked ? Tokens.secMusic : t.nInk,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
        const SizedBox(height: 6),
        link(
          onHover: (v) => setState(() => _subHover = v),
          onTap: () => widget.controller.openNowDetail(album: false),
          child: Text(
            sub,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              fontSize: 16,
              color: _subHover && linked ? Tokens.secMusic : t.nInk2,
            ),
          ),
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
        // All six shapes, and Off. The bar used to carry this next to its own
        // strip of bars; the bars live here now and so does the picker.
        VizStyleButton(controller: c, size: 44, iconSize: 18),
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
