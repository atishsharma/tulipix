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

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';
import 'music_widgets.dart';
import 'player_widgets.dart';

class PlayerBar extends StatelessWidget {
  const PlayerBar({super.key, required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = controller.state;
    final now = st?.now;
    if (st == null || now == null || !now.loaded && now.title.isEmpty) {
      return const SizedBox.shrink();
    }

    // A stream has no duration, so the scrubber would be a bar that never
    // fills — the elapsed clock is the honest thing to show instead.
    final live = now.mode == 'radio' || controller.tickDur <= 0;
    final accent = controller.accent;
    final library = now.itemId != 0 && now.mode == 'music';

    return Container(
      decoration: BoxDecoration(
        color: t.panel,
        border: Border(top: BorderSide(color: t.nHair)),
        boxShadow: const [
          BoxShadow(color: Color(0x66000000), blurRadius: 34, spreadRadius: -8),
        ],
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          // Only the equalizer pushes up out of the bar now. Queue and Lyrics
          // dock to the right of the page — see side_panel.dart.
          if (controller.panel == 'eq')
            SizedBox(height: 260, child: _Panel(controller: controller)),
          SizedBox(
            height: 124,
            child: DecoratedBox(
              decoration: artWash(accent),
              child: Padding(
                padding: const EdgeInsets.fromLTRB(22, 6, 22, 9),
                child: Column(
                  children: [
                    _LyricLine(controller: controller),
                    const SizedBox(height: 5),
                    _SeekRow(controller: controller, live: live),
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
    if (!shown) return const SizedBox(height: 16);
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
              style: const TextStyle(
                fontSize: 12,
                fontWeight: FontWeight.w800,
                color: Tokens.secMusic,
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
    return SizedBox(
      height: 26,
      child: Row(
        children: [
          Expanded(
            child: live
                ? Container(
                    height: 26,
                    padding: const EdgeInsets.symmetric(horizontal: 12),
                    decoration: BoxDecoration(
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
                                color: t.nInk2)),
                        const SizedBox(width: 12),
                        Expanded(
                          child: Text(
                            now.streamTitle.isEmpty
                                ? fmtClock(controller.tickPos)
                                : now.streamTitle,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 11, color: t.nInk2),
                          ),
                        ),
                      ],
                    ),
                  )
                : SeekPill(
                    pos: controller.tickPos,
                    dur: controller.tickDur,
                    accent: controller.accent,
                    onSeek: (v) => controller.send(MusicCmd.seek(secs: v)),
                  ),
          ),
          const SizedBox(width: 12),
          PlayerBtn(
            icon: Icons.tune,
            tip: 'Equalizer',
            size: 26,
            iconSize: 15,
            active: controller.panel == 'eq',
            accent: controller.accent,
            onTap: () => controller.setPanel('eq'),
          ),
          const SizedBox(width: 6),
          VolPill(
            volume: now.volume,
            muted: now.muted,
            accent: controller.accent,
            onVolume: (v) => controller.send(MusicCmd.setVolume(volume: v)),
            onMute: () => controller.send(const MusicCmd.toggleMute()),
          ),
        ],
      ),
    );
  }
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
                    decoration: BoxDecoration(
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
                        MusicArt(
                          controller: c,
                          kind: 'track',
                          artKey: '${now.itemId}',
                          direct: now.art,
                          size: 56,
                          radius: 0,
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
                child: NowPlayingLines(
                  controller: c,
                  now: now,
                  live: widget.live,
                  titleSize: 15,
                  subSize: 12,
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
                _SleepButton(controller: c),
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
    final ordered = !live && !book;
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
        onTap: () => controller.send(
          podcast
              ? const MusicCmd.podSkip(secs: -30)
              : book
                  ? const MusicCmd.bookChapter(delta: -1)
                  : const MusicCmd.prev(),
        ),
      ),
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
        onTap: () => controller.send(
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
  const _Panel({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return DecoratedBox(
      decoration: BoxDecoration(
        color: t.panel2,
        border: Border(bottom: BorderSide(color: t.outline)),
      ),
      child: switch (controller.panel) {
        'eq' => _EqPanel(controller: controller),
        _ => const SizedBox.shrink(),
      },
    );
  }
}

class _EqPanel extends StatelessWidget {
  const _EqPanel({required this.controller});

  final MusicController controller;

  /// The ISO centre frequencies `tulipix_music::eq::BANDS_HZ` declares. Labels
  /// only — the gains themselves come from the snapshot.
  static const List<String> _labels = [
    '31', '62', '125', '250', '500', '1k', '2k', '4k', '8k', '16k', //
  ];

  @override
  Widget build(BuildContext context) {
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
                              value: bands[i].clamp(-12, 12),
                              min: -12,
                              max: 12,
                              activeColor: Tokens.secMusic,
                              onChanged: (v) => controller.send(
                                  MusicCmd.setEqBand(index: i, gainDb: v)),
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
