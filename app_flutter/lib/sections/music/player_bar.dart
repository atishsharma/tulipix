// The player bar, and the three panels that slide out of it.
//
// It belongs to the section, not to a tab: My Music, Podcasts, Audiobooks,
// Radio and YouTube all drive the same mpv process, and the bar is where that
// shows. `mode` on the snapshot says which of them owns the current sound, and
// the transport changes shape accordingly — Next means the next track in a
// queue, the next episode, the next chapter or the next station, and a radio
// stream has no seek bar at all because it has no end to seek towards.
//
// Three rows, 124px: the synced lyric line, the seek pill with the equalizer
// and volume beside it, and the controls. The cover's own colour washes in
// from the left behind all of it, so the bar changes with the record.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';
import 'music_viz.dart';
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
          if (controller.panel.isNotEmpty)
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

class _ControlsState extends State<_Controls> {
  bool _artHover = false;
  bool _vizHover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = widget.controller;
    final now = widget.now;
    final accent = c.accent;

    return Row(
      children: [
        // Art — click opens the zen player, which is the only way in that does
        // not need a menu.
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
                      color: accent.withValues(alpha: 0.33), blurRadius: 14),
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
        SizedBox(
          width: 240,
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              SizedBox(
                height: 20,
                child: Marquee(
                  // A radio stream's ICY title is the actual song; the station
                  // name is ours and sits on the line below.
                  text: widget.live && now.streamTitle.isNotEmpty
                      ? now.streamTitle
                      : (now.title.isEmpty ? 'Nothing playing' : now.title),
                  style: TextStyle(
                      fontSize: 15, fontWeight: FontWeight.w700, color: t.nInk),
                ),
              ),
              SizedBox(
                height: 16,
                child: Marquee(
                  text: now.title.isEmpty
                      ? 'Pick a track'
                      : [now.artist, now.album]
                          .where((s) => s.isNotEmpty)
                          .join('  ·  '),
                  speed: 26,
                  style: TextStyle(fontSize: 12, color: t.nInk2),
                ),
              ),
            ],
          ),
        ),
        const SizedBox(width: 12),
        // Visualizer — the whole strip is the click target for zen.
        Expanded(
          child: MouseRegion(
            cursor: SystemMouseCursors.click,
            onEnter: (_) => setState(() => _vizHover = true),
            onExit: (_) => setState(() => _vizHover = false),
            child: GestureDetector(
              onTap: c.openZen,
              child: SizedBox(
                height: 56,
                child: Stack(
                  alignment: Alignment.center,
                  children: [
                    if (c.visOn)
                      FractionallySizedBox(
                        widthFactor: 0.8,
                        heightFactor: 0.86,
                        child:
                            VizView(style: c.visStyle, playing: c.tickPlaying),
                      ),
                    if (_vizHover)
                      DecoratedBox(
                        decoration: BoxDecoration(
                          color: const Color(0x66000000),
                          borderRadius: BorderRadius.circular(6),
                        ),
                        child: const Center(
                          child: Row(
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              Icon(Icons.open_in_full,
                                  size: 11, color: Colors.white),
                              SizedBox(width: 6),
                              Text('zen',
                                  style: TextStyle(
                                      fontSize: 11, color: Colors.white)),
                            ],
                          ),
                        ),
                      ),
                  ],
                ),
              ),
            ),
          ),
        ),
        _VizStyleButton(controller: c),
        const SizedBox(width: 6),
        Transport(controller: c, mode: now.mode, live: widget.live),
        const SizedBox(width: 10),
        if (widget.library)
          PlayerBtn(
            icon: now.loved ? Icons.favorite : Icons.favorite_border,
            tip: now.loved ? 'Unlike' : 'Like',
            active: now.loved,
            accent: accent,
            onTap: () => c.send(MusicCmd.love(itemId: now.itemId)),
          ),
        Container(width: 1, height: 22, color: t.nHover),
        PlayerBtn(
          icon: Icons.queue_music,
          tip: 'Queue',
          iconSize: 17,
          active: c.panel == 'queue',
          accent: accent,
          onTap: () => c.setPanel('queue'),
        ),
        if (widget.library)
          PlayerBtn(
            icon: Icons.lyrics_outlined,
            tip: 'Lyrics',
            iconSize: 17,
            active: c.panel == 'lyrics',
            accent: accent,
            onTap: () => c.setPanel('lyrics'),
          ),
        if (widget.library)
          PlayerBtn(
            icon: Icons.playlist_add,
            tip: 'Add to playlist',
            iconSize: 17,
            accent: accent,
            onTap: () async {
              // The playlist API takes tracks, and the only full Track row for
              // what is playing is the queue entry it came from.
              final queue = c.state?.queue ?? const <Track>[];
              final at = queue.indexWhere((t) => t.itemId == now.itemId);
              if (at < 0) return;
              await addToPlaylist(context, c, [queue[at]]);
            },
          ),
        _SleepButton(controller: c),
        PlayerBtn(
          icon: Icons.picture_in_picture_alt,
          tip: 'Mini player',
          iconSize: 17,
          active: c.miniOpen,
          accent: accent,
          onTap: c.toggleMini,
        ),
        PlayerBtn(
          icon: Icons.settings_outlined,
          tip: 'Audio settings',
          iconSize: 17,
          accent: accent,
          onTap: () => audioSettings(context, c),
        ),
      ],
    );
  }
}

/// Which of the six visualizer shapes is drawn, and whether it is drawn at all.
class _VizStyleButton extends StatelessWidget {
  const _VizStyleButton({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return PopupMenuButton<int>(
      tooltip: 'Visualizer style',
      // -1 is the Off row: picking a style turns it back on, which is why the
      // list would otherwise look dead while it is off.
      onSelected: (v) =>
          v < 0 ? controller.setVisOn(false) : controller.setVisStyle(v),
      itemBuilder: (_) => [
        for (var i = 0; i < visStyleNames.length; i++)
          CheckedPopupMenuItem(
            value: i,
            checked: controller.visOn && controller.visStyle == i,
            child: Text(visStyleNames[i]),
          ),
        const PopupMenuDivider(),
        CheckedPopupMenuItem(
          value: -1,
          checked: !controller.visOn,
          child: const Text('Off'),
        ),
      ],
      child: Icon(Icons.graphic_eq, size: 16, color: t.nInk2),
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
  });

  final MusicController controller;
  final String mode;
  final bool live;
  final double scale;

  /// Drop shuffle, repeat and stop — for the mini, which has no room for them.
  final bool compact;

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

    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        if (ordered && !compact)
          PlayerBtn(
            icon: Icons.shuffle,
            tip: 'Shuffle',
            size: 38 * s,
            iconSize: 18 * s,
            active: shuffle,
            accent: accent,
            onTap: () => controller.send(const MusicCmd.toggleShuffle()),
          ),
        PlayerBtn(
          icon: podcast ? Icons.replay_30 : Icons.skip_previous,
          tip: book
              ? 'Previous chapter'
              : podcast
                  ? 'Back 30s'
                  : 'Previous',
          size: 42 * s,
          iconSize: 22 * s,
          accent: accent,
          onTap: () => controller.send(
            podcast
                ? const MusicCmd.podSkip(secs: -30)
                : book
                    ? const MusicCmd.bookChapter(delta: -1)
                    : const MusicCmd.prev(),
          ),
        ),
        SizedBox(width: 4 * s),
        BigPlayButton(
          playing: playing,
          accent: accent,
          size: 52 * s,
          onTap: () => controller.send(const MusicCmd.playPause()),
        ),
        SizedBox(width: 4 * s),
        PlayerBtn(
          icon: podcast ? Icons.forward_30 : Icons.skip_next,
          tip: book
              ? 'Next chapter'
              : podcast
                  ? 'Forward 30s'
                  : 'Next',
          size: 42 * s,
          iconSize: 22 * s,
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
            size: 38 * s,
            iconSize: 18 * s,
            accent: accent,
            onTap: () => controller.send(const MusicCmd.stop()),
          ),
        if (ordered && !compact)
          PlayerBtn(
            icon: repeat == 'one' ? Icons.repeat_one : Icons.repeat,
            tip: repeat == 'one'
                ? 'Repeat one'
                : (repeat == 'all' ? 'Repeat all' : 'Repeat'),
            size: 38 * s,
            iconSize: 18 * s,
            active: repeat != 'off',
            accent: accent,
            onTap: () => controller.send(const MusicCmd.cycleRepeat()),
          ),
        if (book || podcast) _SpeedButton(controller: controller, book: book),
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
    return PopupMenuButton<double>(
      tooltip: 'Playback speed',
      onSelected: (v) => controller.send(
        book ? MusicCmd.bookSetSpeed(speed: v) : MusicCmd.podSetSpeed(speed: v),
      ),
      itemBuilder: (_) => [
        for (final v in const [0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 3.0])
          PopupMenuItem(value: v, child: Text('$v×')),
      ],
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
        child: Text('$speed×',
            style: const TextStyle(fontSize: 12, fontWeight: FontWeight.w600)),
      ),
    );
  }
}

class _SleepButton extends StatelessWidget {
  const _SleepButton({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final mins = controller.state?.sleepMin ?? 0;
    return PopupMenuButton<int>(
      tooltip: mins > 0 ? 'Sleep in $mins min' : 'Sleep timer',
      icon: Icon(Icons.bedtime_outlined,
          size: 18, color: mins > 0 ? Tokens.secMusic : t.textDim),
      onSelected: (v) => controller.send(MusicCmd.setSleep(minutes: v)),
      itemBuilder: (_) => const [
        PopupMenuItem(value: 0, child: Text('Off')),
        PopupMenuItem(value: 15, child: Text('15 minutes')),
        PopupMenuItem(value: 30, child: Text('30 minutes')),
        PopupMenuItem(value: 60, child: Text('1 hour')),
      ],
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
        'queue' => _QueuePanel(controller: controller),
        'lyrics' => _LyricsPanel(controller: controller),
        'eq' => _EqPanel(controller: controller),
        _ => const SizedBox.shrink(),
      },
    );
  }
}

class _QueuePanel extends StatelessWidget {
  const _QueuePanel({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final queue = controller.state?.queue ?? const <Track>[];
    if (queue.isEmpty) {
      return const MusicEmpty(
        icon: Icons.queue_music_outlined,
        title: 'The queue is empty',
        body: 'Play an album, a playlist or a folder and it lands here.',
      );
    }
    return Column(
      children: [
        Row(
          children: [
            const SizedBox(width: 16),
            Text('Queue · ${queue.length}',
                style: const TextStyle(fontWeight: FontWeight.w600)),
            const Spacer(),
            TextButton(
              onPressed: () => controller.send(const MusicCmd.queueClear()),
              child: const Text('Clear'),
            ),
            const SizedBox(width: 8),
          ],
        ),
        Expanded(
          child: ReorderableListView.builder(
            buildDefaultDragHandles: true,
            itemCount: queue.length,
            // onReorderItem, unlike the deprecated onReorder, already
            // accounts for the lifted row.
            onReorderItem: (from, to) =>
                controller.send(MusicCmd.queueMove(from: from, to: to)),
            itemBuilder: (_, i) => TrackRow(
              key: ValueKey(queue[i].itemId),
              controller: controller,
              track: queue[i],
              index: i,
              dense: true,
              onPlay: () => controller.send(MusicCmd.queuePlayAt(index: i)),
              onRemove: () => controller
                  .send(MusicCmd.queueRemove(itemId: queue[i].itemId)),
            ),
          ),
        ),
      ],
    );
  }
}

class _LyricsPanel extends StatelessWidget {
  const _LyricsPanel({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = controller.state;
    final lines = st?.lyrics ?? const <LyricLine>[];
    final plain = st?.lyricsPlain ?? '';
    final itemId = st?.now.itemId ?? 0;

    if (lines.isEmpty && plain.isEmpty) {
      return MusicEmpty(
        icon: Icons.lyrics_outlined,
        title: 'No lyrics stored',
        body: itemId == 0
            ? 'Play something first.'
            : 'Fetch them from LRCLIB — they are saved to the library, so the '
                'Slint build sees them too.',
        action: itemId == 0
            ? null
            : (
                'Fetch lyrics',
                () => controller.send(MusicCmd.fetchLyrics(itemId: itemId))
              ),
      );
    }

    if (lines.isEmpty) {
      return SingleChildScrollView(
        padding: const EdgeInsets.all(20),
        child: Text(plain, style: TextStyle(fontSize: 14, color: t.text)),
      );
    }

    // Which line is live, with the user's own offset applied. The domain
    // crate's `active_line` does this in Rust for the Slint build; here the
    // position ticks in Dart, so the same arithmetic follows it.
    final atMs =
        (controller.tickPos * 1000).round() - (st?.lyricsOffsetMs ?? 0);
    var active = -1;
    for (var i = 0; i < lines.length; i++) {
      if (lines[i].atMs <= atMs) active = i;
    }

    return Column(
      children: [
        Expanded(
          child: ListView.builder(
            padding: const EdgeInsets.symmetric(vertical: 16, horizontal: 24),
            itemCount: lines.length,
            itemBuilder: (_, i) => Padding(
              padding: const EdgeInsets.symmetric(vertical: 3),
              child: Text(
                lines[i].text,
                textAlign: TextAlign.center,
                style: TextStyle(
                  fontSize: i == active ? 17 : 14,
                  fontWeight: i == active ? FontWeight.w700 : FontWeight.w400,
                  color: i == active ? Tokens.secMusic : t.textDim,
                ),
              ),
            ),
          ),
        ),
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            IconButton(
              tooltip: 'Lyrics 250ms earlier',
              icon: const Icon(Icons.fast_rewind, size: 18),
              onPressed: () =>
                  controller.send(const MusicCmd.lyricsOffset(deltaMs: -250)),
            ),
            Text('${st?.lyricsOffsetMs ?? 0} ms',
                style: TextStyle(fontSize: 12, color: t.textDim)),
            IconButton(
              tooltip: 'Lyrics 250ms later',
              icon: const Icon(Icons.fast_forward, size: 18),
              onPressed: () =>
                  controller.send(const MusicCmd.lyricsOffset(deltaMs: 250)),
            ),
          ],
        ),
      ],
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
