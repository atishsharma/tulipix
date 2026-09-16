// The player bar, and the three panels that slide out of it.
//
// It belongs to the section, not to a tab: My Music, Podcasts, Audiobooks,
// Radio and YouTube all drive the same deck — one media_kit player — and the
// bar is where that
// shows. `mode` on the snapshot says which of them owns the current sound, and
// the transport changes shape accordingly — Next means the next track in a
// queue, the next episode, the next chapter or the next station, and a radio
// stream has no seek bar at all because it has no end to seek towards.
//
// Three rows, 124px: the synced lyric line, the seek pill with the equalizer
// and volume beside it, and the controls. The cover's own colour washes in
// from the left behind all of it, so the bar changes with the record.
//
// No visualizer down here. There was one wedged between the title and the
// transport, and it was the thing pushing the play button off centre for a
// drawing nobody watches while they are browsing. The zen player is where the
// bars belong — it is the page you open to look at them.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../design/pick.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../playback/audio_deck.dart' show audioPositionS;
import '../../playback/video_layer.dart' show videoPlayPause, videoPlaying;
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_motion.dart';
import 'music_dialogs.dart';
import 'music_widgets.dart';
import 'player_widgets.dart';
import 'youtube/yt_details.dart';
import 'youtube/yt_format_sheet.dart';

/// Chapter starts per YouTube video, asked once a session. The format lookup
/// is cached for six hours in SQLite, so a replay costs nothing; a failure
/// (offline, yt-dlp missing) is no chapters, not an error.
// ponytail: one entry per video played, never pruned; bound it if sessions
// ever run to thousands of videos.
final Map<String, Future<List<double>>> _chapters = {};

Future<List<double>> _chaptersOf(String videoId) => _chapters.putIfAbsent(
      videoId,
      () => musicYtFormats(videoId: videoId).then(
            (i) => [for (final ch in i.chapters) ch.startS],
            onError: (_) => const <double>[],
          ),
    );

/// What is playing, as the format panel wants a video.
YtVideo _nowVideo(NowPlaying now) => YtVideo(
      videoId: now.key,
      title: now.title,
      channel: now.artist,
      thumb: now.art,
      duration: now.dur.round(),
      mediaPath: '',
      meta: '',
      progress: 0,
      quality: '',
      channelId: '',
      offline: '',
      bytes: -1,
    );

class PlayerBar extends StatelessWidget {
  const PlayerBar({super.key, required this.controller});

  final MusicController controller;

  /// Whether there is anything for the bar to show.
  static bool visible(MusicController c) {
    final now = c.state?.now;
    return now != null && (now.loaded || now.title.isNotEmpty);
  }

  /// How much of the page the bar stands on: its own height with the
  /// equalizer shut, or nothing while it is hidden. The page pads its foot by
  /// this and the bar floats over that strip, so the equalizer opening on top
  /// of the bar grows it up over the page rather than taking height from it.
  static double baseHeight(BuildContext context, MusicController c) {
    if (!visible(c)) return 0;
    // Standard: 124 and its 1px top border. A skin's slab floats clear of
    // the edges, 4 above it and 22 below.
    return context.skin.surface(SurfaceRole.bar, radius: 30) == null
        ? 125
        : 150;
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = controller.state;
    final now = st?.now;
    if (st == null || now == null || !visible(controller)) {
      return const SizedBox.shrink();
    }

    // A stream has no duration, so the scrubber would be a bar that never
    // fills — the elapsed clock is the honest thing to show instead.
    final live = now.mode == 'radio' || controller.tickDur <= 0;
    final accent = controller.accent;
    final library = now.itemId != 0 && now.mode == 'music';

    // A skin's bar is a slab standing on its own material, not a strip bolted
    // to the window's foot — so it floats clear of the edges.
    final skin = context.skin;
    final slab = skin.surface(SurfaceRole.bar, radius: 30);
    final bar = Container(
      decoration: slab ?? BoxDecoration(
        color: t.panel,
        border: Border(top: BorderSide(color: t.nHair)),
        boxShadow: const [
          BoxShadow(color: Color(0x66000000), blurRadius: 34, spreadRadius: -8),
        ],
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          // The equalizer opens out of the top of the bar, as part of it. The
          // page keeps room for the bar's own height only ([baseHeight]) and
          // the bar floats over its foot, so opening this grows the bar up
          // over the page instead of shrinking the page under it. Queue and
          // Lyrics dock to the right of the page — see side_panel.dart.
          if (controller.eqOpen)
            SizedBox(
              height: 260,
              child: _Panel(
                  key: MusicController.eqPanelKey, controller: controller),
            ),
          SizedBox(
            height: 124,
            // Animated, not decorated: the palette should arrive with the next
            // record rather than snap to it, and a DecoratedBox cannot tween.
            child: AnimatedContainer(
              duration: Motion.wash,
              curve: Motion.ease,
              // The cover wash is a Standard trait; a skin keeps its material.
              decoration: slab != null
                  ? const BoxDecoration()
                  : artWash(accent, alt: controller.accentAlt),
              child: Padding(
                padding: const EdgeInsets.fromLTRB(22, 6, 22, 9),
                child: Column(
                  children: [
                    // The two rows a position tick changes, and the only two
                    // it rebuilds. It used to rebuild the whole bar once a
                    // second: under a skin, twenty-odd SkinButtons.
                    ListenableBuilder(
                      listenable: controller.ticks,
                      builder: (_, __) => _LyricLine(controller: controller),
                    ),
                    const SizedBox(height: 5),
                    ListenableBuilder(
                      listenable: controller.ticks,
                      builder: (_, __) =>
                          _SeekRow(controller: controller, live: live),
                    ),
                    const SizedBox(height: 5),
                    Expanded(
                      child: _Controls(
                        controller: controller,
                        now: now,
                        live: live,
                        library: library,
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ],
      ),
    );
    // The bar floats over the page's foot now, so it needs a ground of its
    // own: a translucent panel (light Standard is 81%) or a glassy slab let
    // the page show through the equalizer. The canvas it used to stand on,
    // painted behind it, so it reads exactly as it did.
    final backed = DecoratedBox(
      decoration: BoxDecoration(
        color: (skin.canvas ?? t.nCanvas).withValues(alpha: 1),
        borderRadius: slab == null ? null : BorderRadius.circular(30),
      ),
      child: bar,
    );
    if (slab == null) return backed;
    return Padding(
      padding: const EdgeInsets.fromLTRB(22, 4, 22, 22),
      child: backed,
    );
  }
}

/// One line: the lyric before, the lyric now, the lyric next. Auto-shows while
/// something with synced words is playing, independent of the lyrics panel —
/// it is the line you glance at, not the page you read.
class _LyricLine extends StatelessWidget {
  const _LyricLine({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final lines = controller.state?.lyrics ?? const <LyricLine>[];
    final a = controller.activeLyric;
    final shown = controller.tickPlaying && a >= 0 && a < lines.length;
    if (!shown) return _YtFormat(controller: controller);
    Widget quiet(String text) => Text(
          text,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(fontSize: 12, color: t.nInk3),
        );
    return SizedBox(
      height: 16,
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          if (a - 1 >= 0) Flexible(child: quiet(lines[a - 1].text)),
          const SizedBox(width: 16),
          Flexible(
            child: Text(
              lines[a].text,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontSize: 12,
                fontWeight: FontWeight.w800,
                color: context.skin.accent ?? Tokens.secMusic,
              ),
            ),
          ),
          const SizedBox(width: 16),
          if (a + 1 < lines.length) Flexible(child: quiet(lines[a + 1].text)),
        ],
      ),
    );
  }
}

/// A YouTube track has no lyrics, so its line names the streams playing:
/// "Opus 160k", or "1080p60 avc1 + Opus 160k" while the picture is up. Tapping
/// it opens the format panel.
class _YtFormat extends StatelessWidget {
  const _YtFormat({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    final now = st?.now;
    if (st == null || now == null || now.mode != 'youtube' || st.ytFormatFor != now.key) {
      return const SizedBox(height: 16);
    }
    final label = st.ytWatching && st.ytPictureFormat.isNotEmpty
        ? st.ytPictureFormat
        : st.ytSoundFormat;
    if (label.isEmpty) return const SizedBox(height: 16);
    final color = context.skin.inkDim ?? context.tokens.nInk3;
    return SizedBox(
      height: 16,
      child: Center(
        child: InkWell(
          borderRadius: BorderRadius.circular(8),
          onTap: () => showFormatSheet(context, controller, _nowVideo(now)),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(
                    st.ytWatching
                        ? Icons.smart_display_outlined
                        : Icons.graphic_eq,
                    size: 12,
                    color: color),
                const SizedBox(width: 6),
                Text(label,
                    style: TextStyle(
                        fontSize: 11,
                        fontWeight: FontWeight.w600,
                        color: color,
                        fontFeatures: const [FontFeature.tabularFigures()])),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Seek, equalizer, volume. Volume rides up here so the controls row below can
/// spend its width on the transport and the extras.
class _SeekRow extends StatelessWidget {
  const _SeekRow({required this.controller, required this.live});

  final MusicController controller;
  final bool live;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = controller.state!;
    final now = st.now;
    final skin = context.skin;
    final label = skin.inkDim ?? t.nInk2;
    return SizedBox(
      height: 26,
      child: Row(
        children: [
          Expanded(
            child: live
                ? Container(
                    height: 26,
                    padding: const EdgeInsets.symmetric(horizontal: 12),
                    decoration: context.skin
                            .surface(SurfaceRole.well, radius: 13) ??
                        BoxDecoration(
                      color: t.nChip,
                      borderRadius: BorderRadius.circular(13),
                      border: Border.all(color: t.nHair),
                    ),
                    child: Row(
                      children: [
                        const Icon(Icons.fiber_manual_record,
                            size: 9, color: Color(0xFF22C55E)),
                        const SizedBox(width: 6),
                        Text('LIVE',
                            style: TextStyle(
                                fontSize: 10,
                                fontWeight: FontWeight.w800,
                                color: label)),
                        const SizedBox(width: 12),
                        Expanded(
                          child: Text(
                            now.streamTitle.isEmpty
                                ? fmtClock(controller.tickPos)
                                : now.streamTitle,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 11, color: label),
                          ),
                        ),
                      ],
                    ),
                  )
                : now.mode == 'youtube' && now.key.isNotEmpty
                    ? FutureBuilder<List<double>>(
                        future: _chaptersOf(now.key),
                        builder: (_, snap) => _pill(context, now, snap.data),
                      )
                    : _pill(context, now, null),
          ),
          const SizedBox(width: 12),
          PlayerBtn(
            icon: Icons.tune,
            tip: 'Equalizer',
            size: 26,
            iconSize: 15,
            active: controller.eqOpen,
            accent: controller.accent,
            onTap: controller.toggleEq,
          ),
          const SizedBox(width: 6),
          VolPill(
            volume: controller.volume,
            muted: controller.muted,
            accent: controller.accent,
            onVolume: (v) => controller.setVolume(v),
            onMute: () => controller.send(const MusicCmd.toggleMute()),
          ),
        ],
      ),
    );
  }

  Widget _pill(BuildContext context, NowPlaying now, List<double>? marks) =>
      context.skin.seekBar(SeekSlot(
        pos: controller.tickPos,
        dur: controller.tickDur,
        playing: controller.tickPlaying,
        onSeek: (v) => controller.send(MusicCmd.seek(secs: v)),
        deck: () => audioPositionS,
      )) ??
      SeekPill(
        pos: controller.tickPos,
        dur: controller.tickDur,
        accent: controller.accent,
        // This one is the live deck, so the bar follows the unthrottled
        // position rather than the once-a-second tick.
        smooth: true,
        playing: controller.tickPlaying,
        // Only a library track gets one -- the same rule the bar uses for the
        // heart. A stream has no file to decode and no end to draw a shape
        // against.
        wave: now.itemId != 0 && now.mode == 'music'
            ? controller.waveFor(now.itemId)
            : null,
        marks: marks ?? const [],
        onSeek: (v) => controller.send(MusicCmd.seek(secs: v)),
      );
}

/// Art, the two title lines, the visualizer, the transport, the extras.
class _Controls extends StatefulWidget {
  const _Controls({
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
  State<_Controls> createState() => _ControlsState();
}

/// Below this the bar gives up shuffle, stop and repeat and narrows the title
/// block. A Row cannot shrink a fixed child, so without a breakpoint every
/// window under about 1,450 pixels reported a `RenderFlex overflowed` across
/// the whole width of the player — on every tab, because the bar is the one
/// thing all five share.
const double _barFull = 1120;

class _ControlsState extends State<_Controls> {
  bool _artHover = false;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
        builder: (context, box) => _row(context, box.maxWidth >= _barFull),
      );

  Widget _row(BuildContext context, bool wide) {
    final t = context.tokens;
    final c = widget.controller;
    final now = widget.now;
    final accent = c.accent;

    // Three parts, and the middle one is centred: the two flanks are Expanded
    // with the same flex, so the transport sits in the middle of the bar
    // whatever the title on the left is doing. It used to be laid out left to
    // right with a visualizer taking up the slack, which put the play button
    // wherever the song's name happened to end.
    return Row(
      children: [
        Expanded(
          child: Row(
            children: [
              // Art — click opens the zen player, which is the only way in
              // that does not need a menu.
              MouseRegion(
                cursor: SystemMouseCursors.click,
                onEnter: (_) => setState(() => _artHover = true),
                onExit: (_) => setState(() => _artHover = false),
                child: GestureDetector(
                  onTap: c.openZen,
                  child: Container(
                    width: 56,
                    height: 56,
                    decoration: context.skin
                            .surface(SurfaceRole.art, radius: 10) ??
                        BoxDecoration(
                      color: t.nTile,
                      borderRadius: BorderRadius.circular(10),
                      border: Border.all(color: t.nHair),
                      boxShadow: [
                        BoxShadow(
                            color: accent.withValues(alpha: 0.33),
                            blurRadius: 14),
                      ],
                    ),
                    clipBehavior: Clip.antiAlias,
                    child: Stack(
                      fit: StackFit.expand,
                      children: [
                        // Keyed on the track, so a song change dissolves the
                        // old cover into the new one instead of swapping it
                        // between two frames.
                        CrossFade(
                          key: playerArtKey,
                          slotKey: 'art:${now.itemId}:${now.art}',
                          alignment: Alignment.center,
                          child: MusicArt(
                            controller: c,
                            kind: 'track',
                            artKey: '${now.itemId}',
                            direct: now.art,
                            size: 56,
                            radius: 0,
                          ),
                        ),
                        if (_artHover)
                          const ColoredBox(
                            color: Color(0xAA000000),
                            child: Icon(Icons.open_in_full,
                                size: 20, color: Colors.white),
                          ),
                      ],
                    ),
                  ),
                ),
              ),
              const SizedBox(width: 12),
              Flexible(
                // The words change with the picture: same key, same duration,
                // so the title and the cover cannot land a frame apart.
                child: CrossFade(
                  slotKey: 'lines:${now.itemId}:${now.title}',
                  child: NowPlayingLines(
                    controller: c,
                    now: now,
                    live: widget.live,
                    titleSize: 15,
                    subSize: 12,
                  ),
                ),
              ),
            ],
          ),
        ),
        const SizedBox(width: 12),
        Transport(
          controller: c,
          mode: now.mode,
          live: widget.live,
          compact: !wide,
          // The heart leads the transport rather than trailing it: with it
          // there the seven controls read as three, the play circle, and three,
          // which is a shape. Behind it they were six and a stray.
          loved: widget.library ? now.loved : null,
          onFav: () => c.send(MusicCmd.love(itemId: now.itemId)),
        ),
        const SizedBox(width: 12),
        Expanded(
          child: Row(
            mainAxisAlignment: MainAxisAlignment.end,
            children: [
              Container(width: 1, height: 22, color: t.nHover),
              const SizedBox(width: 10),
              // The extras. Slint separates every one of these by 12px; the
              // port had them touching, which is what made a row of icons read
              // as a toolbar rather than as buttons.
              for (final btn in <Widget>[
                PlayerBtn(
                  icon: Icons.queue_music,
                  tip: 'Queue',
                  active: c.panel == 'queue',
                  accent: accent,
                  onTap: () => c.setPanel('queue'),
                ),
                if (widget.library)
                  PlayerBtn(
                    icon: Icons.lyrics_outlined,
                    tip: 'Lyrics',
                    active: c.panel == 'lyrics',
                    accent: accent,
                    onTap: () => c.setPanel('lyrics'),
                  ),
                if (widget.library)
                  Builder(
                    builder: (btnContext) => PlayerBtn(
                      icon: Icons.playlist_add,
                      tip: 'Add to playlist',
                      accent: accent,
                      onTap: () async {
                        // The playlist API takes tracks, and the only full
                        // Track row for what is playing is the queue entry it
                        // came from.
                        final queue = c.state?.queue ?? const <Track>[];
                        final at =
                            queue.indexWhere((t) => t.itemId == now.itemId);
                        if (at < 0) return;
                        await playlistDropUp(btnContext, c, [queue[at]]);
                      },
                    ),
                  ),
                if (now.mode == 'youtube') ...[
                  // Audio | Video: the picture opens where the sound had got
                  // to, and closing it resumes the sound where it stopped.
                  _YtModeSwitch(controller: c, accent: accent),
                  PlayerBtn(
                    icon: Icons.video_settings_outlined,
                    tip: 'Formats and download',
                    accent: accent,
                    onTap: () => showFormatSheet(context, c, _nowVideo(now)),
                  ),
                  PlayerBtn(
                    icon: Icons.info_outline,
                    tip: 'Description and chapters',
                    accent: accent,
                    onTap: () => showYtDetails(context, c, _nowVideo(now)),
                  ),
                ],
                _SleepButton(controller: c),
                _CastButton(controller: c),
                PlayerBtn(
                  icon: Icons.picture_in_picture_alt,
                  tip: 'Mini player',
                  active: c.miniOpen,
                  accent: accent,
                  onTap: c.toggleMini,
                ),
                PlayerBtn(
                  icon: Icons.settings_outlined,
                  tip: 'Audio settings',
                  accent: accent,
                  onTap: () => audioSettings(context, c),
                ),
              ]) ...[
                btn,
                const SizedBox(width: 10),
              ],
            ],
          ),
        ),
      ],
    );
  }
}

/// The transport, whichever source owns the sound.
///
/// Prev/Next are not one thing: in a queue they step tracks, in an audiobook
/// they step chapters, in a podcast they jump thirty seconds, and a live
/// station has neither. Shuffle and repeat only exist where there is an order
/// to disturb.
class Transport extends StatelessWidget {
  const Transport({
    super.key,
    required this.controller,
    required this.mode,
    required this.live,
    this.scale = 1.0,
    this.compact = false,
    this.loved,
    this.onFav,
  });

  final MusicController controller;
  final String mode;
  final bool live;
  final double scale;

  /// The mini's shape: shuffle, prev, play, next, repeat — exactly Slint's
  /// `MusicMini` control row. It drops the heart and Stop, which the bar has
  /// room for and 300px does not, and keeps the two switches, which are the
  /// whole reason you would reach for the mini rather than the bar.
  final bool compact;

  /// Whether the current track is loved, or null where there is nothing to
  /// love. The heart leads the transport rather than trailing it: with it there
  /// the row reads as three controls, the play circle, and three — a shape.
  /// Behind it, it was six and a stray.
  final bool? loved;
  final VoidCallback? onFav;

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    final shuffle = st?.shuffle ?? false;
    final repeat = st?.repeat ?? 'off';
    final playing = controller.tickPlaying;
    final accent = controller.accent;
    final book = mode == 'book';
    final podcast = mode == 'podcast';
    // A YouTube video on screen: the deck is stopped, so play and pause
    // reach the picture, Stop closes it, and stepping waits until it is shut.
    final watching = st?.ytWatching ?? false;
    final ordered = !live && !book && !watching;
    final s = scale;

    // Slint's controls row is one `HorizontalLayout { spacing: 12px }`, so
    // every button in the transport stands apart from its neighbour. The port
    // had them flush, which read as a strip of glyphs rather than as six
    // things you can press. Halved for the mini, which has 300px to work in.
    final gap = SizedBox(width: (compact ? 4 : 10) * s);

    // Seven in the bar, five in the mini -- always, whatever is playing. The
    // controls that mean nothing to a live stream or an audiobook chapter are
    // *disabled* rather than removed: a transport that changes shape between
    // tabs makes the play button move under the pointer, and the muscle memory
    // for "next" is a position, not a glyph.
    final buttons = <Widget>[
      if (!compact)
        PlayerBtn(
          icon: (loved ?? false) ? Icons.favorite : Icons.favorite_border,
          tip: loved == null ? 'Nothing to like' : (loved! ? 'Unlike' : 'Like'),
          size: 40 * s,
          iconSize: 19 * s,
          active: loved ?? false,
          accent: accent,
          onTap: loved == null ? null : onFav,
        ),
      PlayerBtn(
        icon: Icons.shuffle,
        tip: ordered ? 'Shuffle' : 'Nothing to shuffle',
        size: 40 * s,
        iconSize: 19 * s,
        active: shuffle && ordered,
        accent: accent,
        onTap: ordered
            ? () => controller.send(const MusicCmd.toggleShuffle())
            : null,
      ),
      PlayerBtn(
        icon: podcast ? Icons.replay_30 : Icons.skip_previous,
        tip: book
            ? 'Previous chapter'
            : podcast
                ? 'Back 30s'
                : 'Previous',
        size: 44 * s,
        iconSize: 24 * s,
        accent: accent,
        onTap: watching
            ? null
            : () => controller.send(
                  podcast
                      ? const MusicCmd.podSkip(secs: -30)
                      : book
                          ? const MusicCmd.bookChapter(delta: -1)
                          : const MusicCmd.prev(),
                ),
      ),
      if (watching)
        ValueListenableBuilder<bool>(
          valueListenable: videoPlaying,
          builder: (_, on, __) => BigPlayButton(
            playing: on,
            accent: accent,
            size: 56 * s,
            onTap: () => videoPlayPause?.call(),
          ),
        )
      else
        BigPlayButton(
          playing: playing,
          accent: accent,
          size: 56 * s,
          onTap: () => controller.send(const MusicCmd.playPause()),
        ),
      PlayerBtn(
        icon: podcast ? Icons.forward_30 : Icons.skip_next,
        tip: book
            ? 'Next chapter'
            : podcast
                ? 'Forward 30s'
                : 'Next',
        size: 44 * s,
        iconSize: 24 * s,
        accent: accent,
        onTap: watching
            ? null
            : () => controller.send(
                  podcast
                      ? const MusicCmd.podSkip(secs: 30)
                      : book
                          ? const MusicCmd.bookChapter(delta: 1)
                          : const MusicCmd.next(),
                ),
      ),
      if (!compact)
        PlayerBtn(
          icon: Icons.stop,
          tip: 'Stop',
          size: 40 * s,
          iconSize: 19 * s,
          accent: accent,
          onTap: () => controller.send(const MusicCmd.stop()),
        ),
      PlayerBtn(
        icon: repeat == 'one' ? Icons.repeat_one : Icons.repeat,
        tip: !ordered
            ? 'Nothing to repeat'
            : repeat == 'one'
                ? 'Repeat one'
                : (repeat == 'all' ? 'Repeat all' : 'Repeat'),
        size: 40 * s,
        iconSize: 19 * s,
        active: repeat != 'off' && ordered,
        accent: accent,
        onTap: ordered
            ? () => controller.send(const MusicCmd.cycleRepeat())
            : null,
      ),
      if (book || podcast) _SpeedButton(controller: controller, book: book),
    ];

    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (var i = 0; i < buttons.length; i++) ...[
          if (i > 0) gap,
          buttons[i],
        ],
      ],
    );
  }
}

class _SpeedButton extends StatelessWidget {
  const _SpeedButton({required this.controller, required this.book});

  final MusicController controller;
  final bool book;

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    final speed = book ? (st?.bookSpeed ?? 1.0) : (st?.podSpeed ?? 1.0);
    return Builder(
      builder: (btn) => GestureDetector(
        onTap: () async {
          final v = await dropUp<double>(btn, items: [
            for (final x in const [0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0])
              CheckedPopupMenuItem(
                value: x,
                checked: speed == x,
                child: Text('$x×'),
              ),
          ]);
          if (v == null) return;
          controller.send(book
              ? MusicCmd.bookSetSpeed(speed: v)
              : MusicCmd.podSetSpeed(speed: v));
        },
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
          child: Text('$speed×',
              style:
                  const TextStyle(fontSize: 12, fontWeight: FontWeight.w600)),
        ),
      ),
    );
  }
}

/// Send what is playing to a speaker on the network.
///
/// The renderer pulls the file over HTTP from this machine — see
/// `crate::cast_serve` — so this is only offered for a track that is on disk.
/// A YouTube or radio stream already has a URL, and casting those is the
/// Videos section's job.
class _CastButton extends StatelessWidget {
  const _CastButton({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    final active = st?.castActive ?? false;
    final target = st?.castTarget ?? '';
    return PlayerBtn(
      icon: active ? Icons.speaker : Icons.cast,
      tip: active ? 'Playing on $target' : 'Play on a speaker',
      active: active,
      accent: controller.accent,
      onTap: () => castPicker(context, controller),
    );
  }
}

class _SleepButton extends StatelessWidget {
  const _SleepButton({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final mins = controller.state?.sleepMin ?? 0;
    return Builder(
      builder: (btn) => PlayerBtn(
        icon: Icons.bedtime_outlined,
        tip: mins > 0 ? 'Sleep in $mins min' : 'Sleep timer',
        active: mins != 0,
        accent: controller.accent,
        onTap: () async {
          final v = await dropUp<int>(btn, items: [
            for (final m in const [0, 10, 15, 30, 45, 60, 90, -1])
              CheckedPopupMenuItem(
                value: m,
                checked: mins == m,
                child: Text(switch (m) {
                  0 => 'Off',
                  -1 => 'After this track',
                  _ => '$m minutes',
                }),
              ),
          ]);
          if (v != null) controller.send(MusicCmd.setSleep(minutes: v));
        },
      ),
    );
  }
}

class _Panel extends StatelessWidget {
  const _Panel({super.key, required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // The bar's own material, not a second one laid over it, so the two read
    // as one panel opened upward. Under a skin the slab already paints behind
    // this, rounded at its top corners as it is at its bottom ones, and a fill
    // here squared them off. Standard's bar is `t.panel` too.
    final washed = context.skin.surface(SurfaceRole.bar, radius: 30) == null;
    return DecoratedBox(
      decoration: BoxDecoration(
        color: context.skin.isStandard ? t.panel : null,
        border: Border(bottom: BorderSide(color: t.outline)),
      ),
      // The same cover wash as the bar's own strip under it, so the two read
      // as one surface rather than a plain panel stacked on a coloured one.
      child: AnimatedContainer(
        duration: Motion.wash,
        curve: Motion.ease,
        decoration: washed
            ? artWash(controller.accent, alt: controller.accentAlt)
            : const BoxDecoration(),
        child: _EqPanel(controller: controller),
      ),
    );
  }
}

class _EqPanel extends StatefulWidget {
  const _EqPanel({required this.controller});

  final MusicController controller;

  @override
  State<_EqPanel> createState() => _EqPanelState();
}

class _EqPanelState extends State<_EqPanel> {
  /// The ISO centre frequencies `tulipix_music::eq::BANDS_HZ` declares. Labels
  /// only — the gains themselves come from the snapshot.
  static const List<String> _labels = [
    '31', '62', '125', '250', '500', '1k', '2k', '4k', '8k', '16k', //
  ];

  /// A band being dragged right now, and the gain the pointer is at.
  ///
  /// `Slider.onChanged` fires on every pointer move, and `SetEqBand` is not a
  /// cheap command: it reloads the stored curve, writes settings.json, writes
  /// it a second time to record that the preset is now "custom", and pushes a
  /// new filter chain at mpv. Sending one per frame was several hundred file
  /// rewrites per drag. The slider now follows the finger from here and the
  /// bridge hears from us on a timer.
  int? _band;
  double _gain = 0;
  Timer? _throttle;

  /// Often enough that the ear hears the sweep, rarely enough that the disk
  /// does not.
  static const Duration _rate = Duration(milliseconds: 110);

  @override
  void dispose() {
    _throttle?.cancel();
    super.dispose();
  }

  void _drag(int index, double gain) {
    setState(() {
      _band = index;
      _gain = gain;
    });
    if (_throttle?.isActive ?? false) return;
    _send(index, gain);
    _throttle = Timer(_rate, () {
      // Whatever the finger reached while we were quiet.
      if (_band == index) _send(index, _gain);
    });
  }

  /// The pointer is up: the value is final, so nothing may be dropped.
  void _commit(int index, double gain) {
    _throttle?.cancel();
    _send(index, gain);
    setState(() => _band = null);
  }

  void _send(int index, double gain) =>
      widget.controller.send(MusicCmd.setEqBand(index: index, gainDb: gain));

  /// A curve from AutoEq, onto the bands we have.
  ///
  /// Said plainly rather than sold: ten bands cannot hold a dozen filters with
  /// a Q of four on them. What survives is the broad tilt, which is most of
  /// what a headphone correction is, and the preamp that keeps the boosted
  /// bands from clipping.
  Future<void> _loadAutoEq(BuildContext context) async {
    final ok = await confirm(
      context,
      title: 'Load an AutoEq curve?',
      body: 'Pick the ParametricEQ.txt for your headphones from AutoEq. Its '
          'filters are sampled onto these ten bands, which keeps the overall '
          'shape and loses the narrow corrections — a graphic equalizer cannot '
          'hold those. Your current bands are replaced.',
      action: 'Choose a file',
      danger: false,
    );
    if (!ok || !context.mounted) return;
    final path = await pickFile(label: 'AutoEq preset', extensions: const ['txt']);
    if (path == null) return;
    await widget.controller.send(MusicCmd.applyAutoEq(path: path));
  }

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    final t = context.tokens;
    final st = controller.state;
    final bands = st?.eqBands ?? List<double>.filled(10, 0);
    final on = st?.eqOn ?? false;
    final preset = st?.eqPreset ?? 'flat';

    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 12, 24, 12),
      child: Column(
        children: [
          Row(
            children: [
              Switch(
                value: on,
                onChanged: (_) => controller.send(const MusicCmd.toggleEq()),
              ),
              const SizedBox(width: 8),
              Text('Equalizer',
                  style: TextStyle(fontWeight: FontWeight.w600, color: t.text)),
              const SizedBox(width: 24),
              for (final p in const [
                'flat',
                'rock',
                'pop',
                'jazz',
                'bass',
                'treble'
              ])
                Padding(
                  padding: const EdgeInsets.only(right: 6),
                  child: MusicChip(
                    label: p[0].toUpperCase() + p.substring(1),
                    active: preset == p,
                    onTap: () => controller.send(MusicCmd.setEqPreset(name: p)),
                  ),
                ),
              const Spacer(),
              // Headphone correction, without a catalogue. AutoEq publishes a
              // ParametricEQ.txt per model; this reads one and flattens its
              // filters onto the ten bands. Auto-detecting the connected
              // headphones would need those thousands of files shipped with the
              // app, which is a decision about the installer.
              TextButton.icon(
                icon: const Icon(Icons.headphones_outlined, size: 16),
                label: const Text('Load AutoEq…'),
                onPressed: () => _loadAutoEq(context),
              ),
            ],
          ),
          Expanded(
            child: Row(
              children: [
                for (var i = 0; i < 10; i++)
                  Expanded(
                    child: Column(
                      children: [
                        Expanded(
                          child: RotatedBox(
                            quarterTurns: 3,
                            child: Slider(
                              // ±12 dB is `eq::MAX_GAIN_DB`; the bridge
                              // clamps to it as well, so a drag past the end
                              // cannot store an out-of-range gain.
                              //
                              // The band under the finger reads from the drag,
                              // not the snapshot — the snapshot arrives on the
                              // throttle and would otherwise pull the handle
                              // backwards mid-sweep.
                              value: (_band == i ? _gain : bands[i])
                                  .clamp(-12, 12),
                              min: -12,
                              max: 12,
                              activeColor: Tokens.secMusic,
                              // Ten vertical sliders in a row are ten
                              // identical announcements otherwise. The band is
                              // the only thing distinguishing them, and it is
                              // in the label under each one where a screen
                              // reader will not connect the two.
                              label: '${_labels[i]}Hz',
                              semanticFormatterCallback: (v) =>
                                  '${_labels[i]} hertz, '
                                  '${v.toStringAsFixed(0)} decibels',
                              onChanged: (v) => _drag(i, v),
                              onChangeEnd: (v) => _commit(i, v),
                            ),
                          ),
                        ),
                        Text(_labels[i],
                            style: TextStyle(fontSize: 10, color: t.textDim)),
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

/// Audio | Video for the YouTube video playing, as one two-part switch: which
/// half is lit says how it is playing, the other half switches.
class _YtModeSwitch extends StatelessWidget {
  const _YtModeSwitch({required this.controller, required this.accent});

  final MusicController controller;
  final Color accent;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final watching = controller.state?.ytWatching ?? false;
    Widget half(IconData icon, String label, bool on, MusicCmd cmd) => Tooltip(
          message: on ? 'Playing as ${label.toLowerCase()}' : 'Switch to $label',
          child: Material(
            color: on ? accent.withValues(alpha: 0.18) : Colors.transparent,
            borderRadius: BorderRadius.circular(9),
            child: InkWell(
              borderRadius: BorderRadius.circular(9),
              onTap: on ? null : () => controller.send(cmd),
              child: Padding(
                padding: const EdgeInsets.symmetric(horizontal: 10),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(icon, size: 15, color: on ? accent : t.nInk2),
                    const SizedBox(width: 5),
                    Text(label,
                        style: TextStyle(
                            fontSize: 12,
                            fontWeight: FontWeight.w600,
                            color: on ? accent : t.nInk2)),
                  ],
                ),
              ),
            ),
          ),
        );
    return Container(
      height: 38,
      padding: const EdgeInsets.all(3),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: t.nHair),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          half(Icons.headphones_outlined, 'Audio', !watching,
              const MusicCmd.ytStopWatching()),
          const SizedBox(width: 2),
          half(Icons.smart_display_outlined, 'Video', watching,
              const MusicCmd.ytWatchCurrent()),
        ],
      ),
    );
  }
}
