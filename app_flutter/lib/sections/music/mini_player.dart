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

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import '../../design/motion_clock.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_widgets.dart';
import 'player_bar.dart' show Transport;
import 'player_widgets.dart';
import '../../design/decode.dart';

/// The host frame, before `miniScale`. `music-mini-w` / `music-mini-h` in
/// ui/main.slint, which is 300 x 470 for music and books and 300 x 456 for
/// podcasts. One frame here rather than three: the contents differ by mode —
/// see [_MiniTransport] and the flip panel — and a fourteen-pixel difference in
/// the shell is not worth a second constant to keep in step with the first.
const Size kMiniSize = Size(300, 470);

/// The minimised bubble's diameter. It hangs 18px off the right wall
/// (`x: root.width - 78px` over a 60px disc) and rides up and down it.
///
/// Slint's 60 plus 30%: at 60 the cover in it is a thumbnail of a thumbnail,
/// and the bubble is the only thing on screen saying what is playing.
const double kBubbleSize = 78;

class MiniPlayer extends StatelessWidget {
  const MiniPlayer({
    super.key,
    required this.controller,
    this.embedded = false,
    this.scale,
  });

  final MusicController controller;

  /// Dropped into a host card rather than floating over the app — Home's
  /// Classic rail. `embedded: true` on Slint's `MusicMini`, and it means the
  /// same thing: the card the player sits in already has an edge, a fill and a
  /// cast, so the player draws none of its own.
  final bool embedded;

  /// Overrides `controller.miniScale`, which belongs to the floating copy. The
  /// embedded one is sized by the rail it is in.
  final double? scale;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = controller.state;
    final now = st?.now;
    if (st == null || now == null) return const SizedBox.shrink();

    final accent = controller.accent;
    final live = now.mode == 'radio' || controller.tickDur <= 0;
    final library = now.itemId != 0 && now.mode == 'music';
    final s = scale ?? controller.miniScale;
    // Slint has three mini components, not one: `MusicMini` (which carries its
    // own radio and YouTube branches), `PodcastMini` and `BookMini`. They share
    // a frame and nothing else — a spoken-word player has no vinyl, no shuffle
    // and no lyrics, and a book has chapters where an album has a queue.
    //
    // One widget here, three sets of contents, branched on `now.mode`: the
    // transport below is a different five buttons per mode, the flip panel a
    // different set of faces, and the ring a different colour. The frame is the
    // only thing genuinely shared, which is the only thing worth sharing.
    final podcast = now.mode == 'podcast';
    final book = now.mode == 'book';

    // Material, or every Text in here is drawn with the yellow double
    // underline MaterialApp paints to say "this text has no Material
    // ancestor". The mini is a Stack child of the app root rather than of a
    // Scaffold, so it had none, and almost every line in it was struck
    // through. Transparency, not a surface: the Container below is the
    // surface.
    return Material(
      type: MaterialType.transparency,
      child: Container(
        decoration: BoxDecoration(
          color: embedded ? Colors.transparent : t.nCard,
          borderRadius: BorderRadius.circular(embedded ? 0 : 18 * s),
          // The ring is not decoration: this thing floats over arbitrary content,
          // and without a hard edge it reads as part of whatever is behind it.
          // Each of the three wears its own ring, the way Slint's do: pink for
          // music, violet for podcasts, blue for books. It is the fastest read
          // of what the thing floating in the corner is playing.
          border: embedded
              ? null
              : GradientBoxBorder(
                  gradient: LinearGradient(
                    begin: Alignment.topLeft,
                    end: Alignment.bottomRight,
                    colors: podcast
                        ? const [
                            Color(0xFF8B5CF6),
                            Color(0xFFEC4899),
                            Color(0xFF06B6D4)
                          ]
                        : book
                            ? const [
                                Color(0xFF3B82F6),
                                Color(0xFF06B6D4),
                                Color(0xFF8B5CF6)
                              ]
                            : const [
                                Color(0xFFEC4899),
                                Color(0xFF8B5CF6),
                                Color(0xFF06B6D4)
                              ],
                    stops: const [0.0, 0.55, 1.0],
                  ),
                  width: 2 * s,
                ),
          boxShadow: embedded
              ? null
              : const [
                  BoxShadow(
                      color: Color(0xE6000000),
                      blurRadius: 42,
                      spreadRadius: -6),
                ],
        ),
        clipBehavior: Clip.antiAlias,
        child: DecoratedBox(
          decoration: artWash(accent, from: Alignment.topCenter),
          child: Padding(
            padding: EdgeInsets.all(14 * s),
            child: Column(
              children: [
                _Header(
                    controller: controller,
                    now: now,
                    scale: s,
                    embedded: embedded),
                SizedBox(height: 6 * s),
                _SecondLine(
                    controller: controller, now: now, live: live, scale: s),
                SizedBox(height: 10 * s),
                Expanded(
                  child: _Face(
                    controller: controller,
                    now: now,
                    library: library,
                    live: live,
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
                // Live radio has no transport at all in Slint: a stream has no
                // previous, no next and nowhere to seek to, so the single play
                // button moves down into the control row with the volume.
                if (!(live && now.mode == 'radio'))
                  _MiniTransport(controller: controller, now: now, scale: s),
                SizedBox(height: 6 * s),
                _MiniBar(
                  controller: controller,
                  now: now,
                  library: library,
                  scale: s,
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// The mini's transport, which is a different set of five in each of Slint's
/// three mini players.
///
/// Music keeps the album transport. A podcast has nowhere to skip *to* — an
/// episode is one file an hour long — so it gets jumps in seconds and a speed
/// cycle instead. A book has both: chapters to step and seconds to nudge.
class _MiniTransport extends StatelessWidget {
  const _MiniTransport({
    required this.controller,
    required this.now,
    required this.scale,
  });

  final MusicController controller;
  final NowPlaying now;
  final double scale;

  @override
  Widget build(BuildContext context) {
    final s = scale;
    final accent = controller.accent;
    final st = controller.state;
    final playing = controller.tickPlaying;

    Widget play() => BigPlayButton(
          playing: playing,
          accent: accent,
          size: 48 * s,
          onTap: () => controller.send(const MusicCmd.playPause()),
        );

    Widget jump(int secs) => _JumpBtn(
          secs: secs,
          scale: s,
          accent: accent,
          onTap: () => controller.send(MusicCmd.podSkip(secs: secs.toDouble())),
        );

    Widget speed(double value, List<double> steps, void Function(double) set) =>
        _SpeedBtn(
          value: value,
          scale: s,
          accent: accent,
          onTap: () {
            final i = steps.indexWhere((v) => v > value + 0.001);
            set(i < 0 ? steps.first : steps[i]);
          },
        );

    final children = switch (now.mode) {
      'podcast' => [
          jump(-15),
          play(),
          jump(30),
          speed(
            st?.podSpeed ?? 1.0,
            const [1.0, 1.25, 1.5, 2.0],
            (v) => controller.send(MusicCmd.podSetSpeed(speed: v)),
          ),
        ],
      'book' => [
          PlayerBtn(
            icon: Icons.skip_previous,
            tip: 'Previous chapter',
            size: 36 * s,
            iconSize: 18 * s,
            accent: accent,
            onTap: () => controller.send(const MusicCmd.bookChapter(delta: -1)),
          ),
          jump(-30),
          play(),
          jump(30),
          PlayerBtn(
            icon: Icons.skip_next,
            tip: 'Next chapter',
            size: 36 * s,
            iconSize: 18 * s,
            accent: accent,
            onTap: () => controller.send(const MusicCmd.bookChapter(delta: 1)),
          ),
        ],
      _ => const <Widget>[],
    };
    if (children.isEmpty) {
      return Transport(
        controller: controller,
        mode: now.mode,
        live: controller.tickDur <= 0,
        compact: true,
        scale: 0.86 * s,
      );
    }
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        for (var i = 0; i < children.length; i++) ...[
          if (i > 0) SizedBox(width: 8 * s),
          children[i],
        ],
      ],
    );
  }
}

/// A round ±N-seconds button — the glyph and the number as one control, which
/// is how Slint draws its `-15` and `+30`.
class _JumpBtn extends StatelessWidget {
  const _JumpBtn({
    required this.secs,
    required this.scale,
    required this.accent,
    required this.onTap,
  });

  final int secs;
  final double scale;
  final Color accent;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final back = secs < 0;
    return Tooltip(
      message: back ? 'Back ${-secs}s' : 'Forward ${secs}s',
      child: SizedBox(
        width: 42 * scale,
        height: 42 * scale,
        child: Material(
          color: Colors.transparent,
          shape: CircleBorder(side: BorderSide(color: t.nHair)),
          clipBehavior: Clip.antiAlias,
          child: InkWell(
            onTap: onTap,
            child: Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Icon(back ? Icons.rotate_left : Icons.rotate_right,
                    size: 12 * scale, color: t.nInk2),
                SizedBox(width: 1 * scale),
                Text('${secs.abs()}',
                    style: TextStyle(
                      fontFamily: Tokens.fontFamily,
                      fontSize: 12 * scale,
                      fontWeight: FontWeight.w700,
                      color: t.nInk2,
                    )),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// The playback-speed cycle. One button, one number.
class _SpeedBtn extends StatelessWidget {
  const _SpeedBtn({
    required this.value,
    required this.scale,
    required this.accent,
    required this.onTap,
  });

  final double value;
  final double scale;
  final Color accent;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final fast = value != 1.0;
    return Tooltip(
      message: 'Playback speed',
      child: SizedBox(
        width: 42 * scale,
        height: 42 * scale,
        child: Material(
          color: fast ? accent.withValues(alpha: 0.18) : Colors.transparent,
          shape: CircleBorder(
            side: BorderSide(
                color: fast ? accent.withValues(alpha: 0.5) : t.nHair),
          ),
          clipBehavior: Clip.antiAlias,
          child: InkWell(
            onTap: onTap,
            child: Center(
              child: Text(
                '${value.toStringAsFixed(value == value.roundToDouble() ? 0 : 2)}×',
                style: TextStyle(
                  fontFamily: Tokens.fontFamily,
                  fontSize: 12 * scale,
                  fontWeight: FontWeight.w700,
                  color: fast ? accent : t.nInk2,
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// The row under the transport: volume, and whatever flips the square above it.
///
/// Music gets queue and lyrics; a podcast gets its episode list and the show
/// notes; a book gets its chapters, a speed cycle and a bookmark; a station
/// gets the play button itself, since it has no transport row of its own.
class _MiniBar extends StatelessWidget {
  const _MiniBar({
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
    final s = scale;
    final accent = controller.accent;
    final st = controller.state;
    final radio = now.mode == 'radio';
    final podcast = now.mode == 'podcast';
    final book = now.mode == 'book';

    Widget flip(IconData icon, String face, String tip) => PlayerBtn(
          icon: icon,
          tip: tip,
          size: 30 * s,
          iconSize: 15 * s,
          active: controller.miniFace == face,
          accent: accent,
          onTap: () => controller.setMiniFace(face),
        );

    return Row(
      children: [
        if (radio)
          BigPlayButton(
            playing: controller.tickPlaying,
            accent: const Color(0xFFEF4444),
            size: 42 * s,
            onTap: () => controller.send(const MusicCmd.playPause()),
          ),
        if (library) flip(Icons.queue_music, 'queue', 'Queue'),
        if (podcast) flip(Icons.list_alt, 'episodes', 'Episodes'),
        if (book) flip(Icons.list_alt, 'chapters', 'Chapters'),
        if (book)
          _SpeedBtn(
            value: st?.bookSpeed ?? 1.0,
            scale: 0.72 * s,
            accent: accent,
            onTap: () {
              const steps = [1.0, 1.25, 1.5, 2.0, 3.0, 0.75];
              final v = st?.bookSpeed ?? 1.0;
              final i = steps.indexWhere((x) => x > v + 0.001);
              controller.send(
                  MusicCmd.bookSetSpeed(speed: i < 0 ? steps.first : steps[i]));
            },
          ),
        if (book)
          PlayerBtn(
            icon: Icons.bookmark_add_outlined,
            tip: 'Bookmark here',
            size: 30 * s,
            iconSize: 15 * s,
            accent: accent,
            onTap: () => controller.send(const MusicCmd.bookmarkAdd(label: '')),
          ),
        Expanded(
          child: VolPill(
            volume: controller.volume,
            muted: controller.muted,
            accent: accent,
            width: double.infinity,
            scale: s,
            onVolume: (v) => controller.setVolume(v),
            onMute: () => controller.send(const MusicCmd.toggleMute()),
          ),
        ),
        if (library) flip(Icons.lyrics_outlined, 'lyrics', 'Lyrics'),
        if (podcast) flip(Icons.notes, 'summary', 'Show notes'),
        PlayerBtn(
          icon: Icons.open_in_full,
          tip: 'Zen player',
          size: 30 * s,
          iconSize: 15 * s,
          accent: accent,
          onTap: controller.openZen,
        ),
      ],
    );
  }
}

class _Header extends StatelessWidget {
  const _Header({
    required this.controller,
    required this.now,
    required this.scale,
    required this.embedded,
  });

  final MusicController controller;
  final NowPlaying now;
  final double scale;

  /// Dropped into Home's Classic rail rather than floating. The rail sizes the
  /// player itself, so a control that changes the player's size has nothing to
  /// change — the layout button is for the floating copy only, and so are
  /// Close and Dock-as-a-bubble: `MusicMini` gates both on `if !root.embedded`
  /// (ui/page_music.slint:8242, :8256 — "renders inline inside a card, so drop
  /// the floating chrome"). Closing the player inside the rail that exists to
  /// hold it leaves an empty rail and no way back.
  final bool embedded;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      children: [
        if (!embedded)
          PlayerBtn(
            icon: Icons.close,
            tip: 'Close',
            size: 26 * scale,
            iconSize: 13 * scale,
            accent: controller.accent,
            onTap: controller.toggleMini,
          ),
        Expanded(
          child: _Link(
            controller: controller,
            now: now,
            album: true,
            child: (colour) => SizedBox(
              height: 20 * scale,
              child: Marquee(
                text: now.title.isEmpty ? 'Nothing playing' : now.title,
                centred: true,
                style: TextStyle(
                  fontSize: 15 * scale,
                  fontWeight: FontWeight.w700,
                  color: colour ?? t.nInk,
                ),
              ),
            ),
          ),
        ),
        if (!embedded)
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
    return _Link(
      controller: controller,
      now: now,
      album: false,
      child: (colour) => SizedBox(
        height: 16 * scale,
        child: Marquee(
          text: text,
          centred: true,
          speed: 26,
          style: TextStyle(fontSize: 12 * scale, color: colour ?? t.nInk2),
        ),
      ),
    );
  }
}

/// One of the mini's two title lines, as a link to its page.
///
/// The mini floats over every section, so a click here is the fastest route
/// from a bubble in the corner of Photos to the album it is playing. The
/// builder takes the pink to paint with while hovered, or null for the line's
/// own colour — the two lines have different resting inks and neither should
/// be hard-coded twice.
class _Link extends StatefulWidget {
  const _Link({
    required this.controller,
    required this.now,
    required this.album,
    required this.child,
  });

  final MusicController controller;
  final NowPlaying now;
  final bool album;
  final Widget Function(Color?) child;

  @override
  State<_Link> createState() => _LinkState();
}

class _LinkState extends State<_Link> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final now = widget.now;
    final linked = now.itemId != 0 && now.mode == 'music';
    if (!linked) return widget.child(null);
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: () => widget.controller.openNowDetail(album: widget.album),
        child: widget.child(_hover ? Tokens.secMusic : null),
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
    required this.live,
    required this.scale,
  });

  final MusicController controller;
  final NowPlaying now;
  final bool library;
  final bool live;
  final double scale;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget body;
    switch (controller.miniFace) {
      case 'queue':
        // What is playing, then everything else in queue order. The mini shows
        // four rows at a time: with the deck's own track wherever it happened
        // to sit in the list, the one row you actually want was usually off
        // the top of it. The queue itself is untouched — this is the order it
        // is *read* in, so `onPlay` still names the real position.
        final queue = controller.state?.queue ?? const <Track>[];
        final playing = controller.now?.itemId ?? 0;
        final order = [
          for (var i = 0; i < queue.length; i++)
            if (queue[i].itemId == playing) i,
          for (var i = 0; i < queue.length; i++)
            if (queue[i].itemId != playing) i,
        ];
        body = queue.isEmpty
            ? Center(
                child: Text('Queue is empty.',
                    style: TextStyle(fontSize: 12 * scale, color: t.nInk2)))
            : ListView.builder(
                padding: EdgeInsets.all(4 * scale),
                itemCount: order.length,
                itemBuilder: (_, i) => TrackRow(
                  controller: controller,
                  track: queue[order[i]],
                  index: order[i],
                  dense: true,
                  showArt: false,
                  onPlay: () => controller.playQueueAt(order[i]),
                ),
              );
      case 'lyrics':
        body = MiniLyrics(controller: controller, scale: scale);
      case 'episodes':
        final eps = controller.state?.podEpisodes ?? const <Episode>[];
        body = eps.isEmpty
            ? Center(
                child: Text('No episodes.',
                    style: TextStyle(fontSize: 12 * scale, color: t.nInk2)))
            : ListView.builder(
                padding: EdgeInsets.all(4 * scale),
                itemCount: eps.length,
                itemBuilder: (_, i) => _EpisodeRow(
                  episode: eps[i],
                  scale: scale,
                  onPlay: () =>
                      controller.send(MusicCmd.podPlay(episodeId: eps[i].id)),
                ),
              );
      case 'summary':
        final text = controller.state?.podDetail?.description ?? '';
        body = SingleChildScrollView(
          padding: EdgeInsets.all(12 * scale),
          child: Text(
            text.isEmpty ? 'No summary available.' : text,
            style: TextStyle(fontSize: 12 * scale, height: 1.5, color: t.nInk2),
          ),
        );
      case 'chapters':
        final chapters = controller.state?.bookChapters ?? const <Chapter>[];
        body = chapters.isEmpty
            ? Center(
                child: Text('No chapters.',
                    style: TextStyle(fontSize: 12 * scale, color: t.nInk2)))
            : ListView.builder(
                padding: EdgeInsets.all(4 * scale),
                itemCount: chapters.length,
                itemBuilder: (_, i) => _ChapterRow(
                  chapter: chapters[i],
                  n: i + 1,
                  playing: chapters[i].itemId == now.itemId,
                  scale: scale,
                  onPlay: () => controller
                      .send(MusicCmd.bookPlay(itemId: chapters[i].itemId)),
                ),
              );
      default:
        // Four different pictures, because they are four different objects. A
        // record spins and wears a tonearm; a station has no cover at all and
        // shows its initial; a book is a book — Slint's `BookMini` says so in
        // as many words — and a podcast is a disc with the show's thumb for a
        // label.
        body = switch (now.mode) {
          'radio' =>
            _StationFace(controller: controller, now: now, scale: scale),
          'book' => _CoverFace(
              controller: controller,
              now: now,
              scale: scale,
              portrait: true,
              fallback: Icons.menu_book_outlined,
            ),
          'podcast' => _CoverFace(
              controller: controller,
              now: now,
              scale: scale,
              portrait: false,
              round: true,
              fallback: Icons.mic_none,
            ),
          _ => _Vinyl(controller: controller, now: now, scale: scale),
        };
    }
    return DecoratedBox(
      decoration: BoxDecoration(
        // The record floats on the card; every other face is a panel with a
        // fill, because a list of rows needs an edge to sit inside.
        color: controller.miniFace.isEmpty && now.mode != 'book'
            ? Colors.transparent
            : t.nTile,
        borderRadius: BorderRadius.circular(12 * scale),
      ),
      child: ClipRRect(
        borderRadius: BorderRadius.circular(12 * scale),
        child: body,
      ),
    );
  }
}

/// One picture, from wherever it happens to live.
///
/// A podcast's `art` is an http(s) URL as often as it is a path — the feed
/// hands over a link and nothing downloads it — so this is the one place in the
/// section that has to look at the scheme before picking a loader. Everything
/// else in the library is a file on disk.
Widget miniArt(String path, IconData fallback, double size, Color accent) {
  Widget miss() => Center(child: Icon(fallback, size: size, color: accent));
  if (path.isEmpty) return miss();
  Widget err(BuildContext _, Object __, StackTrace? ___) => miss();
  return path.startsWith('http')
      ? Image.network(path, fit: BoxFit.cover, errorBuilder: err)
      : FileArt(path, errorBuilder: err);
}

/// The face for a station: no record, no cover, just the artwork if the
/// station published a favicon and its initial in a washed disc if it did not.
///
/// Slint drops the vinyl entirely in `radio-mode` — a stream is not a record,
/// there is nothing to seek and nothing to spin, and a tonearm riding a
/// position that never advances is a lie about what the player is doing.
class _StationFace extends StatelessWidget {
  const _StationFace({
    required this.controller,
    required this.now,
    required this.scale,
  });

  final MusicController controller;
  final NowPlaying now;
  final double scale;

  @override
  Widget build(BuildContext context) {
    final accent = controller.accent;
    final initial =
        now.title.isEmpty ? '' : now.title.characters.first.toUpperCase();
    return Center(
      child: LayoutBuilder(
        builder: (context, box) {
          final side = math.min(box.maxWidth, box.maxHeight);
          return Container(
            width: side,
            height: side,
            clipBehavior: Clip.antiAlias,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(14 * scale),
              gradient: LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: [
                  accent.withValues(alpha: 0.55),
                  accent.withValues(alpha: 0.18),
                ],
              ),
              boxShadow: [
                BoxShadow(
                    color: accent.withValues(alpha: 0.45),
                    blurRadius: 26 * scale),
              ],
            ),
            child: now.art.isEmpty
                ? Center(
                    child: Text(
                      initial,
                      style: TextStyle(
                        fontFamily: Tokens.fontFamily,
                        fontSize: 52 * scale,
                        fontWeight: FontWeight.w800,
                        color: const Color(0xD0FFFFFF),
                      ),
                    ),
                  )
                : miniArt(now.art, Icons.radio, 44 * scale, accent),
          );
        },
      ),
    );
  }
}

/// The face for a book or a podcast: the cover itself.
///
/// A book is portrait — Slint draws it 148x188 with a coloured glow, because a
/// paperback is taller than it is wide and a square crop of one looks wrong. A
/// podcast is a circle, the disc of `PodcastMini`, spinning while it plays.
class _CoverFace extends StatelessWidget {
  const _CoverFace({
    required this.controller,
    required this.now,
    required this.scale,
    required this.portrait,
    required this.fallback,
    this.round = false,
  });

  final MusicController controller;
  final NowPlaying now;
  final double scale;
  final bool portrait;
  final bool round;
  final IconData fallback;

  @override
  Widget build(BuildContext context) {
    final accent = controller.accent;
    return Center(
      child: LayoutBuilder(
        builder: (context, box) {
          final side = math.min(box.maxWidth, box.maxHeight) * 0.86;
          final w = portrait ? side * 0.78 : side;
          final radius = round ? w : 10 * scale;
          final art = miniArt(now.art, fallback, 44 * scale, accent);
          // Neither turns. The vinyl next door does not either -- see
          // [_Vinyl] -- and a cover rotating in a 200px square reads as a
          // shudder rather than as motion.
          return Container(
            width: w,
            height: portrait ? side : w,
            clipBehavior: Clip.antiAlias,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(radius),
              color: accent.withValues(alpha: 0.25),
              boxShadow: [
                BoxShadow(
                    color: accent.withValues(alpha: 0.5),
                    blurRadius: 22 * scale),
              ],
            ),
            child: art,
          );
        },
      ),
    );
  }
}

/// One row of the podcast episode list on the mini's flipped face.
class _EpisodeRow extends StatelessWidget {
  const _EpisodeRow({
    required this.episode,
    required this.scale,
    required this.onPlay,
  });

  final Episode episode;
  final double scale;
  final VoidCallback onPlay;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onPlay,
      borderRadius: BorderRadius.circular(8 * scale),
      child: Padding(
        padding:
            EdgeInsets.symmetric(horizontal: 8 * scale, vertical: 7 * scale),
        child: Row(
          children: [
            Icon(
              episode.played ? Icons.check : Icons.play_arrow,
              size: 12 * scale,
              color: episode.played ? const Color(0xFF22C55E) : t.nInk3,
            ),
            SizedBox(width: 8 * scale),
            Expanded(
              child: Text(
                episode.title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12 * scale, color: t.nInk),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// One row of the audiobook chapter list.
class _ChapterRow extends StatelessWidget {
  const _ChapterRow({
    required this.chapter,
    required this.n,
    required this.playing,
    required this.scale,
    required this.onPlay,
  });

  final Chapter chapter;
  final int n;
  final bool playing;
  final double scale;
  final VoidCallback onPlay;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onPlay,
      borderRadius: BorderRadius.circular(8 * scale),
      child: Padding(
        padding:
            EdgeInsets.symmetric(horizontal: 8 * scale, vertical: 6 * scale),
        child: Row(
          children: [
            SizedBox(
              width: 22 * scale,
              child: Text(
                '$n',
                style: TextStyle(
                  fontSize: 11 * scale,
                  fontWeight: FontWeight.w700,
                  color: playing ? Tokens.secMusic : t.nInk3,
                ),
              ),
            ),
            Expanded(
              child: Text(
                chapter.title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: 12 * scale,
                  color: playing ? Tokens.secMusic : t.nInk,
                ),
              ),
            ),
            SizedBox(width: 6 * scale),
            Text(
              fmtClock(chapter.durationS),
              style: TextStyle(fontSize: 11 * scale, color: t.nInk3),
            ),
          ],
        ),
      ),
    );
  }
}

/// The record, the tonearm, and the label in the middle of it.
///
/// A port of `MusicMini`'s `vinyl` block in ui/page_music.slint, geometry and
/// all: a 222-unit disc washed from the artwork's accent, two groove rings, a
/// spinning centre label, a static spindle, and an arm pivoted at (210, 16)
/// whose needle rides 150 units out at 12° when the track starts and 34° when
/// it ends.
///
/// The label turns and nothing else does. The disc as a whole used to rotate,
/// shadow and all, which at this size read as a shudder rather than a
/// turntable — and it is also the expensive way round, since a moving shadow
/// and a moving gradient are a fresh raster every frame. The artwork is the
/// only part with anything on it to see turn, so it is the only part that
/// turns: wrapped in a [RepaintBoundary] the rotation is a transform on a
/// layer that is already cached, which the GPU does for nothing, and the
/// grooves, the rim, the spindle and the arm are never repainted at all.
///
/// The clock is the shared [MotionClock] rather than a ticker of its own, and
/// it is joined only while the deck is playing AND this is the section on
/// screen, stepped rather than beaten — the same rule the minimised bubble
/// follows, and for the same reason: a whole-window frame to turn one disc.
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

class _VinylState extends State<_Vinyl> {
  final ValueNotifier<double> _t = ValueNotifier<double>(0);
  bool _joined = false;

  MusicController get controller => widget.controller;
  NowPlaying get now => widget.now;
  double get scale => widget.scale;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _sync();
  }

  @override
  void didUpdateWidget(covariant _Vinyl old) {
    super.didUpdateWidget(old);
    _sync();
  }

  void _sync() {
    final on = TickerMode.valuesOf(context).enabled && controller.tickPlaying;
    if (on == _joined) return;
    _joined = on;
    if (on) {
      MotionClock.instance.join(_onBeat);
    } else {
      MotionClock.instance.leave(_onBeat);
    }
  }

  /// Every beat, NOT every step.
  ///
  /// The minimised bubble steps because it is a 60px disc in the corner. This
  /// one is the middle of the player: at the step's ten a second, one turn per
  /// 5.04s makes each update a 7° jump, and 7° on a disc this size is not a
  /// slow spin — it is a stutter. On the beat it is 2.4° and reads as rotation.
  ///
  /// Three times the updates, and each one costs a matrix on a layer the
  /// [RepaintBoundary] has already rasterised: nothing repaints, nothing
  /// re-lays-out, and it runs only while the deck plays with this on screen.
  void _onBeat() => _t.value = MotionClock.instance.seconds * 1000;

  @override
  void dispose() {
    if (_joined) MotionClock.instance.leave(_onBeat);
    _t.dispose();
    super.dispose();
  }

  /// Slint's viewbox. Everything below is in these units and scaled to fit.
  static const double _side = 222;
  static const double _pivotX = 210;
  static const double _pivotY = 16;
  static const double _armLen = 150;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final accent = controller.accent;
    final dur = controller.tickDur;
    final frac = dur > 0 ? (controller.tickPos / dur).clamp(0.0, 1.0) : 0.0;
    // Parked at 2° until it is playing, then in from 12° to 34°.
    final deg = controller.tickPlaying ? 12 + 22 * frac : 2.0;
    final rad = deg * math.pi / 180;
    final nx = _pivotX - _armLen * math.sin(rad);
    final ny = _pivotY + _armLen * math.cos(rad);

    return LayoutBuilder(
      builder: (context, box) {
        final d = math.min(box.maxWidth, box.maxHeight);
        final u = d / _side;
        // The label is 146 units in Slint. A third again, as asked: the art is
        // what you are looking at, and the vinyl around it is a frame.
        const label = 146 * 1.35;
        Widget ring(double units, Color colour) => Center(
              child: Container(
                width: units * u,
                height: units * u,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  border: Border.all(color: colour),
                ),
              ),
            );
        Widget dot(double units, double x, double y, Widget? child,
                {Color? fill, Color? edge, double border = 1.5, Color? glow}) =>
            Positioned(
              left: x * u - units * u / 2,
              top: y * u - units * u / 2,
              width: units * u,
              height: units * u,
              child: DecoratedBox(
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: fill,
                  border: edge == null
                      ? null
                      : Border.all(color: edge, width: border),
                  boxShadow: glow == null
                      ? null
                      : [BoxShadow(color: glow, blurRadius: 10 * u)],
                ),
                child: child,
              ),
            );

        return Center(
          child: SizedBox(
            width: d,
            height: d,
            child: Stack(
              children: [
                // The disc: transparent under the label, deepening to the rim.
                Center(
                  child: Container(
                    width: _side * u,
                    height: _side * u,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      gradient: RadialGradient(
                        colors: [
                          accent.withValues(alpha: 0.22),
                          accent.withValues(alpha: 0.60),
                          accent.withValues(alpha: 0.92),
                        ],
                        stops: const [0.30, 0.62, 1.0],
                      ),
                    ),
                  ),
                ),
                // The grooves, outside the enlarged label rather than under it.
                ring(214, t.nHover),
                ring(206, t.nHair),
                // The label. It turns while the deck plays — the one moving
                // part other than the arm, and the only one a record has.
                Center(
                  child: ClipOval(
                    child: SizedBox(
                      width: label * u,
                      height: label * u,
                      child: RepaintBoundary(
                        child: _Spin(
                          tick: _t,
                          spinning: controller.tickPlaying,
                          child: MusicArt(
                            controller: controller,
                            kind: 'track',
                            artKey: '${now.itemId}',
                            direct: now.art,
                            size: label * u,
                            radius: 0,
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
                // No spindle. A real record needs the hole; this one is the
                // album art, and a card-coloured dot punched through the middle
                // of it reads as damage rather than as a turntable — more so now
                // that the art is what turns.
                // The arm, from the pivot to the needle.
                Positioned.fill(
                  child: CustomPaint(
                    painter: _Tonearm(
                      from: Offset(_pivotX * u, _pivotY * u),
                      to: Offset(nx * u, ny * u),
                      colour: t.nInk2,
                      width: 3 * scale,
                    ),
                  ),
                ),
                // The counterweight at the pivot, and the cartridge riding the
                // groove.
                dot(18, _pivotX, _pivotY, null,
                    fill: t.nInk3, edge: t.nCard, border: 2),
                dot(13, nx, ny, null,
                    fill: Tokens.secMusic,
                    glow: Tokens.secMusic.withValues(alpha: 0.53)),
              ],
            ),
          ),
        );
      },
    );
  }
}

/// The straight line between the pivot and the needle.
class _Tonearm extends CustomPainter {
  _Tonearm({
    required this.from,
    required this.to,
    required this.colour,
    required this.width,
  });

  final Offset from;
  final Offset to;
  final Color colour;
  final double width;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawLine(
      from,
      to,
      Paint()
        ..color = colour
        ..strokeWidth = width
        ..strokeCap = StrokeCap.round,
    );
  }

  @override
  bool shouldRepaint(_Tonearm old) =>
      old.to != to || old.from != from || old.colour != colour;
}

/// Three lines — the one being sung, lit, between its neighbours. Public so
/// Home's player card can flip to the same face.
class MiniLyrics extends StatelessWidget {
  const MiniLyrics({required this.controller, required this.scale});

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
    // Every line is the full width of the face and wraps inside it. They used
    // to be laid out to their own text width, so the block breathed in and out
    // as the song went from "Oh." to a line that fills two rows — and the one
    // line you are reading moved sideways every few seconds.
    Widget line(int i, double size, Color colour, FontWeight w) => SizedBox(
          width: double.infinity,
          child: Text(
            i >= 0 && i < lines.length ? lines[i].text : '',
            textAlign: TextAlign.center,
            maxLines: 3,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(fontSize: size, color: colour, fontWeight: w),
          ),
        );
    return Padding(
      padding: EdgeInsets.all(12 * scale),
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        crossAxisAlignment: CrossAxisAlignment.stretch,
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
/// The minimised mini: a 60px record pinned to the right wall.
///
/// A port of the bubble in ui/main.slint, which is a good deal more than the
/// pill this used to be: a pulsing ring in the artwork's accent, the cover
/// clipped round and spinning while the deck plays, and a spindle dot in the
/// middle of it. Tap to bring the mini back; drag it up and down the wall to
/// get it out of the way. The host positions it — see [kBubbleSize].
class MiniBubble extends StatefulWidget {
  const MiniBubble({super.key, required this.controller});

  final MusicController controller;

  @override
  State<MiniBubble> createState() => _MiniBubbleState();
}

class _MiniBubbleState extends State<MiniBubble> {
  /// Milliseconds on the [MotionClock], as something the painter subscribes
  /// to. Same reason as the visualizer: a rebuild for this would re-run build,
  /// layout and semantics across the whole app to turn a 60px disc, where a
  /// repaint of one boundary is the entire job.
  final ValueNotifier<double> _t = ValueNotifier<double>(_kRingRest);
  bool _joined = false;

  /// A clock reading where `|sin(t / 280)|` peaks: the ring at full strength.
  static const double _kRingRest = 280 * math.pi / 2;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _sync();
  }

  @override
  void didUpdateWidget(MiniBubble old) {
    super.didUpdateWidget(old);
    _sync();
  }

  /// The ring pulses while the deck plays and rests at full strength when it
  /// does not. It used to pulse regardless, as what says the bubble is a
  /// control and not a sticker — but it sits above every section, where no
  /// section's TickerMode reaches, and it drew the whole window at every vsync.
  /// A still, full ring says "control" as well. A muted TickerMode stops it
  /// too, and so does the app losing this widget. While it runs it turns on
  /// the motion clock, in step with everything else that moves.
  void _sync() {
    final on =
        TickerMode.valuesOf(context).enabled && widget.controller.tickPlaying;
    if (on == _joined) return;
    _joined = on;
    if (on) {
      MotionClock.instance.join(_onBeat);
    } else {
      MotionClock.instance.leave(_onBeat);
      _t.value = _kRingRest;
    }
  }

  /// On the step, not the beat: over every section, every beat was a
  /// whole-window frame to turn a 60px disc.
  void _onBeat() {
    if (MotionClock.instance.onStep) {
      _t.value = MotionClock.instance.seconds * 1000;
    }
  }

  @override
  void dispose() {
    if (_joined) MotionClock.instance.leave(_onBeat);
    _t.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final now = c.state?.now;
    if (now == null) return const SizedBox.shrink();
    final letter =
        now.mode == 'radio' && now.art.isEmpty && now.title.isNotEmpty
            ? now.title.characters.first.toUpperCase()
            : '';
    return SizedBox(
      width: kBubbleSize,
      height: kBubbleSize,
      // Same reason as the mini: the station initial is a Text with no
      // Material over it.
      child: Material(
        type: MaterialType.transparency,
        child: RepaintBoundary(
          child: Stack(
            children: [
              // The ring, painted rather than built: it is the thing that
              // changes on every beat.
              Positioned.fill(
                child: CustomPaint(
                  painter: _BubbleRing(tick: _t, accent: c.accent),
                ),
              ),
              // 56px of art inset 2px, spun by the same clock. A radio station
              // with no favicon gets its initial, the way the mini and the bar
              // do.
              Positioned(
                left: 2,
                top: 2,
                width: kBubbleSize - 4,
                height: kBubbleSize - 4,
                child: ClipOval(
                  child: ColoredBox(
                    color: const Color(0xFF14141B),
                    child: _Spin(
                      tick: _t,
                      spinning: c.tickPlaying,
                      child: letter.isNotEmpty
                          ? DecoratedBox(
                              decoration: const BoxDecoration(
                                gradient: LinearGradient(
                                  begin: Alignment.topLeft,
                                  end: Alignment.bottomRight,
                                  colors: [
                                    Color(0xFF14B8A6),
                                    Color(0xFF0EA5E9)
                                  ],
                                ),
                              ),
                              child: Center(
                                child: Text(
                                  letter,
                                  style: const TextStyle(
                                    fontFamily: Tokens.fontFamily,
                                    fontSize: 28,
                                    fontWeight: FontWeight.w800,
                                    color: Color(0xD0FFFFFF),
                                  ),
                                ),
                              ),
                            )
                          : Opacity(
                              opacity: 0.9,
                              child: MusicArt(
                                controller: c,
                                kind: 'track',
                                artKey: '${now.itemId}',
                                direct: now.art,
                                size: kBubbleSize - 4,
                                radius: 0,
                              ),
                            ),
                    ),
                  ),
                ),
              ),
              // The spindle. Not spun — a record's hub does not turn with it.
              Center(
                child: Container(
                  width: 8,
                  height: 8,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: const Color(0xFF14141B),
                    border:
                        Border.all(color: const Color(0xCCFFFFFF), width: 1.5),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Pulsing accent ring. `0.45 + 0.55 * |sin(t / 280)|`, straight off main.slint.
class _BubbleRing extends CustomPainter {
  _BubbleRing({required this.tick, required this.accent})
      : super(repaint: tick);

  final ValueListenable<double> tick;
  final Color accent;

  @override
  void paint(Canvas canvas, Size size) {
    final o = 0.45 + 0.55 * math.sin(tick.value / 280).abs();
    canvas.drawCircle(
      size.center(Offset.zero),
      size.shortestSide / 2 - 1.5,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 3
        ..color = accent.withValues(alpha: o),
    );
  }

  @override
  bool shouldRepaint(_BubbleRing old) => old.accent != accent;
}

/// One full turn every 5.04s while playing — `animation-tick() / 14` degrees.
class _Spin extends StatelessWidget {
  const _Spin({
    required this.tick,
    required this.spinning,
    required this.child,
  });

  final ValueListenable<double> tick;
  final bool spinning;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    if (!spinning) return child;
    return AnimatedBuilder(
      animation: tick,
      builder: (context, child) => Transform.rotate(
        angle: (tick.value / 14) % 360 * math.pi / 180,
        child: child,
      ),
      child: child,
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
