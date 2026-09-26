// The video player's controls, and the operations behind them.
//
// media_kit ships `MaterialDesktopVideoControls` and the port used it, which
// bought a seek bar, a volume slider and a fullscreen button for nothing. What
// it does not have is chapters, a subtitle finder, a screenshot, rebindable
// keys, or a seek bar small enough for the docked card — and its buttons
// cannot be added to, only replaced. So they are replaced.
//
// [VideoOps] is the half that does the work. It exists because every one of
// these actions has two callers — a button here and a key in video_keys.dart —
// and "next audio track" written twice is "next audio track" fixed once.

import 'dart:async';
import 'dart:io';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:media_kit/media_kit.dart';

import '../design/tokens.dart';
import '../src/rust/api/videos.dart';
import 'audio_deck.dart' show kDefaultVolume;

/// How tall [VideoControls] draws below the gradient's fade: the bottom
/// padding, the button row, the gap, and the seek lane. The subtitle view is
/// pushed above this so a line of dialogue never lands behind the buttons.
const double kVideoControlsHeight = 12 + 44 + 4 + 22;

/// One entry from mpv's `chapter-list`.
class VideoChapter {
  const VideoChapter(
      {required this.index, required this.title, required this.start});

  final int index;
  final String title;
  final Duration start;
}

/// h:mm:ss, or m:ss under an hour. Negative clamps to zero — a remaining time
/// briefly goes negative at the very end of a file.
String fmtDuration(Duration d) {
  final n = d.isNegative ? Duration.zero : d;
  final mm = (n.inMinutes % 60).toString().padLeft(2, '0');
  final ss = (n.inSeconds % 60).toString().padLeft(2, '0');
  return n.inHours > 0 ? '${n.inHours}:$mm:$ss' : '${n.inMinutes % 60}:$ss';
}

/// Where a seek of [seconds] from [base] should land in a file of [duration].
///
/// Clamped at both ends, and that is the whole reason it exists: mpv treats a
/// negative seek as an error rather than a seek to zero, and seeking past the
/// end of a local file *ends* it — which would write a film off as watched
/// because the key was held a moment too long at the credits. A [duration] of
/// zero means it is not known yet (a stream still negotiating), so only the
/// lower bound applies.
Duration seekTargetFor(Duration base, int seconds, Duration duration) {
  final at = base + Duration(seconds: seconds);
  if (at < Duration.zero) return Duration.zero;
  if (duration > Duration.zero && at > duration) {
    return duration - const Duration(seconds: 1);
  }
  return at;
}

/// Everything the player can be told to do, in one place.
class VideoOps {
  VideoOps(this.player, {required this.source, String? title})
      : _title = title;

  final Player player;

  /// What is playing — a path or a URL. The subtitle finder hashes it when it
  /// is a real file, and the screenshot takes its name from it.
  final String source;

  final String? _title;

  /// A readable name: the one the source was opened with (a YouTube video's
  /// own title), else the source's file name. A film on disk is named after
  /// itself, and a stream URL's "file name" is a query string nobody can read.
  late final String title =
      _title?.trim().isNotEmpty == true ? _title!.trim() : _titleFrom(source);

  static String _titleFrom(String source) {
    final base = _basename(source);
    final dot = base.lastIndexOf('.');
    final stem = dot > 0 ? base.substring(0, dot) : base;
    return stem.replaceAll('.', ' ').replaceAll('_', ' ').trim();
  }

  /// mpv's chapters, read once the file reports a duration. Empty for most
  /// files, which is why the chapters button hides itself.
  final ValueNotifier<List<VideoChapter>> chapters = ValueNotifier(const []);

  /// The subtitle track to come back to when subtitles are switched off and on
  /// again. Without it, "on" would mean "the first one", which is not where you
  /// were.
  SubtitleTrack? _lastSubtitle;

  NativePlayer get _native => player.platform as NativePlayer;

  void dispose() {
    _seekTimer?.cancel();
    _runTimer?.cancel();
    chapters.dispose();
    seekTarget.dispose();
  }

  // ----------------------------------------------------------- transport ---

  /// Where a run of seeks is heading, or null when none is in progress.
  ///
  /// Holding the arrow key fires a `KeyRepeatEvent` at the OS repeat rate —
  /// around thirty a second — and thirty five-second jumps a second is both
  /// unusable and thirty demuxer reopens a second. So presses accumulate into
  /// a target and the real seek goes out on a throttle: a scrub rather than a
  /// stampede. The clock reads this, so you can see where you are heading
  /// while the picture catches up.
  final ValueNotifier<Duration?> seekTarget = ValueNotifier(null);

  /// How often a held key is allowed to actually move the demuxer.
  static const Duration _seekThrottle = Duration(milliseconds: 110);

  /// How long after the last press a run counts as finished. Comfortably
  /// longer than [_seekThrottle], so a scheduled seek always lands before the
  /// target it was scheduled against is cleared.
  static const Duration _seekRunEnds = Duration(milliseconds: 500);

  Timer? _seekTimer;
  Timer? _runTimer;
  DateTime _lastSeek = DateTime.fromMillisecondsSinceEpoch(0);

  void seekBy(int seconds) {
    // From the target rather than the position, so a second press counts from
    // where the first one was going. Two quick taps on +10 are +20, even
    // though the demuxer has not reported arriving at +10 yet.
    final base = seekTarget.value ?? player.state.position;
    seekTarget.value = seekTargetFor(base, seconds, player.state.duration);
    _runTimer?.cancel();
    _runTimer = Timer(_seekRunEnds, () => seekTarget.value = null);
    _scheduleSeek();
  }

  void _scheduleSeek() {
    if (_seekTimer != null) return;
    final wait = _seekThrottle - DateTime.now().difference(_lastSeek);
    if (wait <= Duration.zero) {
      _fireSeek();
    } else {
      _seekTimer = Timer(wait, _fireSeek);
    }
  }

  void _fireSeek() {
    _seekTimer = null;
    final target = seekTarget.value;
    if (target == null) return;
    _lastSeek = DateTime.now();
    player.seek(target);
  }

  void nudgeVolume(double delta) {
    player.setVolume((player.state.volume + delta).clamp(0.0, 100.0));
  }

  /// Scroll arrives in logical pixels, not notches: one mouse wheel click is a
  /// single large delta, a touchpad is a stream of small ones. Dividing rather
  /// than taking the sign is what makes both feel the same — a wheel click is
  /// about nine points of volume, a slow two-finger drag is a slow slide.
  void scrollVolume(double dy) => nudgeVolume(-dy / 6);

  /// The same, for seeking, but [seekBy] counts whole seconds — so the
  /// remainder is kept rather than truncated away, or a touchpad would scroll
  /// forever and never reach one second.
  double _seekScroll = 0;

  void scrollSeek(double dy) {
    _seekScroll += -dy / 10;
    final whole = _seekScroll.truncate();
    if (whole == 0) return;
    _seekScroll -= whole;
    seekBy(whole);
  }

  /// Mute is a volume of zero here rather than mpv's `mute` property, so the
  /// slider and the icon never disagree about what is happening.
  void toggleMute() {
    final volume = player.state.volume;
    if (volume > 0) {
      _premute = volume;
      player.setVolume(0);
    } else {
      player.setVolume(_premute <= 0 ? kDefaultVolume : _premute);
    }
  }

  double _premute = 100;

  void nudgeRate(double delta) {
    player.setRate(((player.state.rate + delta) * 100).round() / 100);
  }

  void setRate(double rate) => player.setRate(rate);

  // -------------------------------------------------------------- tracks ---

  /// Real audio tracks, without mpv's synthetic "auto" and "no" entries — you
  /// cycle between the languages a file has, not through the idea of silence.
  List<AudioTrack> get audioTracks => [
        for (final t in player.state.tracks.audio)
          if (t.id != 'auto' && t.id != 'no') t,
      ];

  List<SubtitleTrack> get subtitleTracks => [
        for (final t in player.state.tracks.subtitle)
          if (t.id != 'auto' && t.id != 'no') t,
      ];

  bool get subtitlesOn => player.state.track.subtitle.id != 'no';

  void cycleAudio() {
    final tracks = audioTracks;
    if (tracks.length < 2) return;
    final at = tracks.indexWhere((t) => t.id == player.state.track.audio.id);
    player.setAudioTrack(tracks[(at + 1) % tracks.length]);
  }

  void cycleSubtitle() {
    final tracks = subtitleTracks;
    if (tracks.isEmpty) return;
    final current = player.state.track.subtitle;
    final at = tracks.indexWhere((t) => t.id == current.id);
    // Off is part of the cycle here, unlike audio: rolling past the last track
    // back to no subtitles is how every other player behaves.
    if (at == tracks.length - 1) {
      _lastSubtitle = current;
      player.setSubtitleTrack(SubtitleTrack.no());
    } else {
      player.setSubtitleTrack(tracks[at + 1]);
    }
  }

  void toggleSubtitles() {
    if (subtitlesOn) {
      _lastSubtitle = player.state.track.subtitle;
      player.setSubtitleTrack(SubtitleTrack.no());
      return;
    }
    final back =
        _lastSubtitle ?? (subtitleTracks.isEmpty ? null : subtitleTracks.first);
    if (back != null) player.setSubtitleTrack(back);
  }

  void setSubtitleTrack(SubtitleTrack track) {
    if (track.id != 'no') _lastSubtitle = track;
    player.setSubtitleTrack(track);
  }

  /// Load a subtitle file the user found or picked, and select it.
  void addSubtitleFile(String path) {
    final track = SubtitleTrack.uri(path, title: _basename(path));
    _lastSubtitle = track;
    player.setSubtitleTrack(track);
  }

  // ------------------------------------------------------------ chapters ---

  /// Read mpv's chapter list. Called once, when the duration first arrives —
  /// before that the demuxer has not necessarily parsed them.
  /// One mpv property, trimmed, or an empty string if it has none.
  ///
  /// mpv answers with an error rather than a blank for a property that does not
  /// apply — a file with no video track has no `video-codec` — and for the
  /// media-info panel "not present" and "failed to ask" are the same answer.
  Future<String> property(String name) async {
    try {
      return (await _native.getProperty(name)).trim();
    } catch (_) {
      return '';
    }
  }

  Future<void> loadChapters() async {
    try {
      final count =
          int.tryParse(await _native.getProperty('chapter-list/count')) ?? 0;
      if (count <= 1) return; // A single chapter is the same as none.
      final out = <VideoChapter>[];
      for (var i = 0; i < count; i++) {
        final title = await _native.getProperty('chapter-list/$i/title');
        final at = double.tryParse(
                await _native.getProperty('chapter-list/$i/time')) ??
            0;
        out.add(VideoChapter(
          index: i,
          title: title.trim().isEmpty ? 'Chapter ${i + 1}' : title.trim(),
          start: Duration(milliseconds: (at * 1000).round()),
        ));
      }
      chapters.value = out;
    } catch (_) {
      // A container with no chapter list answers with an error rather than a
      // zero. Not having chapters is not a fault worth reporting.
    }
  }

  void jumpChapter(int delta) {
    // mpv's own relative chapter seek, which already knows that "previous" from
    // halfway through a chapter means the start of this one.
    _native.command(['add', 'chapter', '$delta']);
  }

  void gotoChapter(VideoChapter chapter) {
    _native.setProperty('chapter', '${chapter.index}');
  }

  // ---------------------------------------------------------- screenshot ---

  /// Write a still, and return where it landed.
  ///
  /// `subtitles` rather than `video`: what is on screen includes the subtitle
  /// line, and a screenshot of a subtitled film without the subtitle is not the
  /// frame anyone meant to keep.
  Future<String?> screenshot() async {
    final dir = playerScreenshotDir();
    if (dir.isEmpty) return null;
    final at = player.state.position;
    final stamp =
        '${at.inHours}.${(at.inMinutes % 60).toString().padLeft(2, '0')}'
        '.${(at.inSeconds % 60).toString().padLeft(2, '0')}';
    final name = '${_stem()}-$stamp.png';
    final file = '$dir${Platform.pathSeparator}$name';
    try {
      await _native.command(['screenshot-to-file', file, 'subtitles']);
      return file;
    } catch (_) {
      return null;
    }
  }

  String _stem() {
    final base = title.trim().isEmpty ? _basename(source) : title.trim();
    final cleaned = base
        .split('')
        .where((c) => RegExp(r'[A-Za-z0-9 _\-.]').hasMatch(c))
        .join()
        .trim();
    final dot = cleaned.lastIndexOf('.');
    final stem = dot > 0 ? cleaned.substring(0, dot) : cleaned;
    return stem.isEmpty ? 'tulipix' : stem;
  }

  static String _basename(String path) {
    final at = path.lastIndexOf(RegExp(r'[/\\]'));
    return at < 0 ? path : path.substring(at + 1);
  }
}

// ------------------------------------------------------------- seek bar ----

/// The seek bar, at two sizes.
///
/// Dragging updates only this widget and commits on release: a seek is a libmpv
/// command that reopens the demuxer at a new offset, and firing one per pointer
/// move makes a network stream stutter for as long as the drag lasts. The same
/// rule the music deck's own seek follows.
class VideoSeekBar extends StatefulWidget {
  const VideoSeekBar({
    super.key,
    required this.ops,
    required this.accent,
    this.compact = false,
    this.onPreview,
  });

  final VideoOps ops;
  final Color accent;

  /// The docked card's version: thin, no hover growth, no preview.
  final bool compact;

  /// Where the pointer or the drag is, as a fraction, or null when neither.
  /// The bar has no room for a time bubble, so whoever draws the clock shows it.
  final void Function(double? frac)? onPreview;

  @override
  State<VideoSeekBar> createState() => _VideoSeekBarState();
}

class _VideoSeekBarState extends State<VideoSeekBar> {
  double? _drag;
  bool _over = false;

  Player get _player => widget.ops.player;

  Duration get _duration => _player.state.duration;

  void _preview(double? frac) => widget.onPreview?.call(frac);

  void _commit(double frac) {
    final duration = _duration;
    if (duration <= Duration.zero) return;
    _player.seek(duration * frac.clamp(0.0, 1.0));
  }

  @override
  Widget build(BuildContext context) {
    final compact = widget.compact;
    final track = compact ? 4.0 : (_over || _drag != null ? 12.0 : 8.0);
    final lane = compact ? 8.0 : 22.0;

    return LayoutBuilder(
      builder: (context, box) {
        final width = box.maxWidth <= 0 ? 1.0 : box.maxWidth;
        double at(Offset local) => (local.dx / width).clamp(0.0, 1.0);

        return Listener(
          // Scrolling over the bar seeks instead of changing the volume.
          // Registered with the pointer-signal resolver rather than handled
          // outright: the bar's opaque Listener already stops the stage's
          // volume scroll from seeing this, but if the two ever do share a
          // hit-test path the resolver gives the event to the innermost
          // registrant only — this one — instead of running both.
          onPointerSignal: (event) {
            if (event is! PointerScrollEvent) return;
            GestureBinding.instance.pointerSignalResolver.register(
              event,
              (e) => widget.ops
                  .scrollSeek((e as PointerScrollEvent).scrollDelta.dy),
            );
          },
          child: MouseRegion(
            cursor: SystemMouseCursors.click,
            onEnter: (_) => setState(() => _over = true),
            onExit: (_) {
              setState(() => _over = false);
              if (_drag == null) _preview(null);
            },
            onHover: compact ? null : (e) => _preview(at(e.localPosition)),
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTapDown: (d) => _commit(at(d.localPosition)),
              onHorizontalDragStart: (d) => setState(() {
                _drag = at(d.localPosition);
                _preview(_drag);
              }),
              onHorizontalDragUpdate: (d) => setState(() {
                _drag = at(d.localPosition);
                _preview(_drag);
              }),
              onHorizontalDragEnd: (_) {
                final frac = _drag;
                setState(() => _drag = null);
                _preview(null);
                if (frac != null) _commit(frac);
              },
              child: SizedBox(
                height: lane,
                child: Center(
                  child: StreamBuilder<Duration>(
                    stream: _player.stream.position,
                    initialData: _player.state.position,
                    builder: (context, snap) {
                      final total = _duration.inMilliseconds;
                      final played = _drag ??
                          (total > 0
                              ? (snap.data ?? Duration.zero).inMilliseconds /
                                  total
                              : 0.0);
                      // Read rather than watched: the position stream already
                      // rebuilds this several times a second, and a second
                      // subscription would only make it more often.
                      final buffered = total > 0
                          ? (_player.state.buffer.inMilliseconds / total)
                              .clamp(0.0, 1.0)
                          : 0.0;
                      return SeekTrack(
                        height: track,
                        played: played.clamp(0.0, 1.0),
                        buffered: buffered.toDouble(),
                        accent: widget.accent,
                        knob: !compact && (_over || _drag != null),
                      );
                    },
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

/// The painted part of the seek bar: a lane, what is buffered, what is played.
///
/// Public and free of [Player] on purpose — it takes three numbers, so its
/// geometry can be checked in a test, which is what the fill growing out of the
/// centre needed and did not have.
class SeekTrack extends StatelessWidget {
  const SeekTrack({
    super.key,
    required this.height,
    required this.played,
    required this.buffered,
    required this.accent,
    required this.knob,
  });

  final double height;
  final double played;
  final double buffered;
  final Color accent;
  final bool knob;

  @override
  Widget build(BuildContext context) {
    return AnimatedContainer(
      duration: const Duration(milliseconds: 120),
      // The lane is the full width, always.
      //
      // The `Center` above hands this loose constraints, so without a width a
      // Container shrink-wraps its child — and the child is a Stack, which
      // sizes itself to its widest child, which is the fill. The lane was
      // therefore exactly as wide as the progress, and the Center put that in
      // the middle: a bar that grew out of the centre in both directions
      // instead of running left to right.
      width: double.infinity,
      height: height,
      decoration: BoxDecoration(
        color: const Color(0x33FFFFFF),
        borderRadius: BorderRadius.circular(height),
      ),
      child: ClipRRect(
        borderRadius: BorderRadius.circular(height),
        child: Stack(
          clipBehavior: Clip.none,
          children: [
            // Both fills are keyed so the layout test can measure them.
            FractionallySizedBox(
              key: const ValueKey('seek-buffered'),
              widthFactor: buffered,
              heightFactor: 1,
              child: const ColoredBox(color: Color(0x40FFFFFF)),
            ),
            FractionallySizedBox(
              key: const ValueKey('seek-played'),
              widthFactor: played,
              heightFactor: 1,
              child: ColoredBox(color: accent),
            ),
            if (knob)
              Align(
                alignment: Alignment(played * 2 - 1, 0),
                child: Container(
                  width: height,
                  height: height,
                  decoration:
                      BoxDecoration(color: accent, shape: BoxShape.circle),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

// --------------------------------------------------------- the whole bar ----

/// The bottom chrome: seek bar with a clock at each end, transport, volume,
/// speed, and the menus.
class VideoControls extends StatefulWidget {
  const VideoControls({
    super.key,
    required this.ops,
    required this.fullscreen,
    required this.onToggleFullscreen,
    required this.onFindSubtitles,
    required this.onPickSubtitleFile,
    required this.onEditShortcuts,
    required this.onScreenshot,
    required this.onMediaInfo,
  });

  final VideoOps ops;
  final bool fullscreen;
  final VoidCallback onToggleFullscreen;
  final VoidCallback onFindSubtitles;
  final VoidCallback onPickSubtitleFile;
  final VoidCallback onEditShortcuts;
  final VoidCallback onScreenshot;
  final VoidCallback onMediaInfo;

  @override
  State<VideoControls> createState() => _VideoControlsState();
}

class _VideoControlsState extends State<VideoControls> {
  final List<StreamSubscription<dynamic>> _subs = [];

  /// Where the pointer is on the seek bar, so the elapsed clock can show the
  /// time you are about to land on instead of the one you are leaving.
  double? _preview;

  Player get _player => widget.ops.player;

  @override
  void initState() {
    super.initState();
    // Everything the bar draws except the position, which the seek bar watches
    // on its own so a tick does not rebuild eleven buttons.
    void redraw(dynamic _) {
      if (mounted) setState(() {});
    }

    _subs
      ..add(_player.stream.playing.listen(redraw))
      ..add(_player.stream.volume.listen(redraw))
      ..add(_player.stream.rate.listen(redraw))
      ..add(_player.stream.duration.listen(redraw))
      ..add(_player.stream.tracks.listen(redraw))
      ..add(_player.stream.track.listen(redraw));
  }

  @override
  void dispose() {
    for (final s in _subs) {
      s.cancel();
    }
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final ops = widget.ops;
    final accent = Tokens.accentOf(Section.videos);
    final duration = _player.state.duration;
    final preview = _preview;

    // The bar is Material even though nothing here is a card.
    //
    // `InkWell`, `Slider` and `PopupMenuButton` all assert on a Material
    // ancestor, and the player deliberately has none: VideoLayer wraps the app
    // from *outside* the Scaffold, so a film can cover the whole window. Every
    // button in this bar threw on its first build without this, and the twelve
    // ErrorWidgets that replaced them each took the biggest width they were
    // offered — which is what an overflow of a million pixels in a Row is.
    // `transparency` so the gradient below still shows through.
    return Material(
      type: MaterialType.transparency,
      child: Container(
        decoration: const BoxDecoration(
          gradient: LinearGradient(
            begin: Alignment.topCenter,
            end: Alignment.bottomCenter,
            colors: [Color(0x00000000), Color(0xCC000000)],
          ),
        ),
        padding: const EdgeInsets.fromLTRB(18, 46, 18, 12),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Row(
              children: [
                // The elapsed clock is the one thing here that has to move on
                // its own. Nothing else in this bar watches the position, so
                // without its own subscription it only repainted when some
                // other stream happened to fire — which is a clock that jumps
                // two or three seconds at a time and sits still in between.
                ValueListenableBuilder<Duration?>(
                  // A held arrow key runs ahead of the picture; this is what
                  // shows where it is going while the demuxer catches up.
                  valueListenable: ops.seekTarget,
                  builder: (context, scrub, _) => StreamBuilder<Duration>(
                    stream: _player.stream.position,
                    initialData: _player.state.position,
                    builder: (context, snap) {
                      final shown = preview != null && duration > Duration.zero
                          ? duration * preview
                          : scrub ?? snap.data ?? Duration.zero;
                      return _Clock(
                        text: fmtDuration(shown),
                        dim: preview != null || scrub != null,
                      );
                    },
                  ),
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: VideoSeekBar(
                    ops: ops,
                    accent: accent,
                    onPreview: (frac) => setState(() => _preview = frac),
                  ),
                ),
                const SizedBox(width: 12),
                _Clock(text: fmtDuration(duration)),
              ],
            ),
            const SizedBox(height: 4),
            Row(
              children: [
                _Btn(
                  icon: Icons.replay_10,
                  tip: 'Back 10 seconds',
                  onTap: () => ops.seekBy(-10),
                ),
                _Btn(
                  icon: Icons.replay_5,
                  tip: 'Back 5 seconds',
                  onTap: () => ops.seekBy(-5),
                ),
                _Btn(
                  icon: _player.state.playing ? Icons.pause : Icons.play_arrow,
                  tip: _player.state.playing ? 'Pause' : 'Play',
                  size: 44,
                  onTap: _player.playOrPause,
                ),
                _Btn(
                  icon: Icons.forward_5,
                  tip: 'Forward 5 seconds',
                  onTap: () => ops.seekBy(5),
                ),
                _Btn(
                  icon: Icons.forward_10,
                  tip: 'Forward 10 seconds',
                  onTap: () => ops.seekBy(10),
                ),
                const SizedBox(width: 6),
                _Volume(ops: ops),
                // The title takes the slack between the two button groups, so
                // it sits in the middle without ever being able to overlap
                // them — which a Stack-centred title would, on a narrow window
                // with a long file name.
                Expanded(
                  child: Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 14),
                    child: Text(
                      ops.title,
                      textAlign: TextAlign.center,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                        color: Colors.white,
                        fontSize: 13,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                  ),
                ),
                _speed(ops),
                _chapters(ops),
                _audio(ops),
                _subtitles(ops),
                _Btn(
                  icon: Icons.photo_camera_outlined,
                  tip: 'Save a screenshot',
                  onTap: widget.onScreenshot,
                ),
                _Btn(
                  icon: Icons.info_outline,
                  tip: 'Media info',
                  onTap: widget.onMediaInfo,
                ),
                _Btn(
                  icon: Icons.keyboard_outlined,
                  tip: 'Shortcuts',
                  onTap: widget.onEditShortcuts,
                ),
                _Btn(
                  icon: widget.fullscreen
                      ? Icons.fullscreen_exit
                      : Icons.fullscreen,
                  tip: widget.fullscreen ? 'Leave fullscreen' : 'Fullscreen',
                  onTap: widget.onToggleFullscreen,
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }

  Widget _speed(VideoOps ops) {
    final rate = _player.state.rate;
    return _Menu<double>(
      tip: 'Playback speed',
      items: [
        for (final r in const [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0])
          _MenuRow(value: r, label: '$r×', selected: (rate - r).abs() < 0.01),
      ],
      onSelected: ops.setRate,
      // The number is the label: an icon for speed says less than "1.5×".
      child: Text(
        '${rate.toStringAsFixed(rate == rate.roundToDouble() ? 0 : 2)}×',
        style:
            const TextStyle(color: Colors.white, fontWeight: FontWeight.w600),
      ),
    );
  }

  Widget _chapters(VideoOps ops) {
    return ValueListenableBuilder<List<VideoChapter>>(
      valueListenable: ops.chapters,
      builder: (context, chapters, _) {
        if (chapters.isEmpty) return const SizedBox.shrink();
        final at = _player.state.position;
        var current = 0;
        for (var i = 0; i < chapters.length; i++) {
          if (chapters[i].start <= at) current = i;
        }
        return _Menu<VideoChapter>(
          tip: 'Chapters',
          icon: Icons.list_alt_outlined,
          items: [
            for (var i = 0; i < chapters.length; i++)
              _MenuRow(
                value: chapters[i],
                label:
                    '${fmtDuration(chapters[i].start)}   ${chapters[i].title}',
                selected: i == current,
              ),
          ],
          onSelected: ops.gotoChapter,
        );
      },
    );
  }

  Widget _audio(VideoOps ops) {
    final tracks = ops.audioTracks;
    if (tracks.length < 2) return const SizedBox.shrink();
    final current = _player.state.track.audio.id;
    return _Menu<AudioTrack>(
      tip: 'Audio track',
      icon: Icons.graphic_eq,
      items: [
        for (final t in tracks)
          _MenuRow(value: t, label: _trackLabel(t), selected: t.id == current),
      ],
      onSelected: _player.setAudioTrack,
    );
  }

  Widget _subtitles(VideoOps ops) {
    final tracks = ops.subtitleTracks;
    final current = _player.state.track.subtitle.id;
    return _Menu<Object>(
      tip: 'Subtitles',
      icon: ops.subtitlesOn ? Icons.subtitles : Icons.subtitles_off_outlined,
      items: [
        _MenuRow<Object>(value: 'off', label: 'Off', selected: current == 'no'),
        for (final t in tracks)
          _MenuRow<Object>(
              value: t, label: _trackLabel(t), selected: t.id == current),
        const _MenuRow<Object>(
            value: 'file', label: 'Load from file…', divided: true),
        const _MenuRow<Object>(value: 'find', label: 'Find subtitles online…'),
      ],
      onSelected: (value) {
        switch (value) {
          case 'off':
            ops.setSubtitleTrack(SubtitleTrack.no());
          case 'file':
            widget.onPickSubtitleFile();
          case 'find':
            widget.onFindSubtitles();
          case final SubtitleTrack t:
            ops.setSubtitleTrack(t);
        }
      },
    );
  }
}

String _trackLabel(dynamic track) {
  final title = track.title as String?;
  final language = track.language as String?;
  final parts = [
    if (title != null && title.trim().isNotEmpty) title.trim(),
    if (language != null && language.trim().isNotEmpty) language.trim(),
  ];
  return parts.isEmpty ? 'Track ${track.id}' : parts.join(' · ');
}

class _Clock extends StatelessWidget {
  const _Clock({required this.text, this.dim = false});

  final String text;
  final bool dim;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 72,
      child: Text(
        text,
        textAlign: TextAlign.center,
        style: TextStyle(
          color: dim ? Colors.white : const Color(0xE6FFFFFF),
          fontFeatures: const [FontFeature.tabularFigures()],
          fontWeight: dim ? FontWeight.w700 : FontWeight.w500,
        ),
      ),
    );
  }
}

class _Volume extends StatelessWidget {
  const _Volume({required this.ops});

  final VideoOps ops;

  @override
  Widget build(BuildContext context) {
    final volume = ops.player.state.volume;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        _Btn(
          icon: volume <= 0
              ? Icons.volume_off
              : volume < 50
                  ? Icons.volume_down
                  : Icons.volume_up,
          tip: volume <= 0 ? 'Unmute' : 'Mute',
          onTap: ops.toggleMute,
        ),
        SizedBox(
          width: 104,
          child: SliderTheme(
            data: SliderTheme.of(context).copyWith(
              trackHeight: 4,
              thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 6),
              overlayShape: const RoundSliderOverlayShape(overlayRadius: 12),
              activeTrackColor: Colors.white,
              inactiveTrackColor: const Color(0x40FFFFFF),
              thumbColor: Colors.white,
            ),
            child: Slider(
              value: volume.clamp(0.0, 100.0),
              max: 100,
              // Volume is a libmpv property write, not a demuxer seek: it is
              // cheap enough to follow the drag, unlike the seek bar.
              onChanged: ops.player.setVolume,
            ),
          ),
        ),
      ],
    );
  }
}

class _Btn extends StatelessWidget {
  const _Btn({
    required this.icon,
    required this.tip,
    required this.onTap,
    this.size = 36,
  });

  final IconData icon;
  final String tip;
  final VoidCallback onTap;
  final double size;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: tip,
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(size),
        child: SizedBox(
          width: size,
          height: size,
          child: Icon(icon, size: size * 0.6, color: Colors.white),
        ),
      ),
    );
  }
}

class _MenuRow<T> {
  const _MenuRow({
    required this.value,
    required this.label,
    this.selected = false,
    this.divided = false,
  });

  final T value;
  final String label;
  final bool selected;

  /// Draw a separator above this row.
  final bool divided;
}

class _Menu<T> extends StatelessWidget {
  const _Menu({
    required this.tip,
    required this.items,
    required this.onSelected,
    this.icon,
    this.child,
  });

  final String tip;
  final IconData? icon;
  final Widget? child;
  final List<_MenuRow<T>> items;
  final ValueChanged<T> onSelected;

  @override
  Widget build(BuildContext context) {
    return PopupMenuButton<T>(
      tooltip: tip,
      position: PopupMenuPosition.over,
      onSelected: onSelected,
      itemBuilder: (context) => [
        for (final row in items) ...[
          if (row.divided) const PopupMenuDivider(),
          PopupMenuItem<T>(
            value: row.value,
            child: Row(
              children: [
                SizedBox(
                  width: 24,
                  child:
                      row.selected ? const Icon(Icons.check, size: 16) : null,
                ),
                Flexible(
                  child: Text(row.label,
                      maxLines: 1, overflow: TextOverflow.ellipsis),
                ),
              ],
            ),
          ),
        ],
      ],
      child: SizedBox(
        height: 36,
        child: Center(
          child: child ??
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: 8),
                child: Icon(icon, size: 21, color: Colors.white),
              ),
        ),
      ),
    );
  }
}
