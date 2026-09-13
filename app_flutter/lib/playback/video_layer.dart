// The video player, in the window.
//
// This is the Dart half of `crates/tulipix-bridge/src/vmpv.rs`. mpv used to
// draw its own top-level window with its own OSC, which is why the old build
// needed `PR_SET_PDEATHSIG` so a crash did not leave an orphan film playing,
// and a `kill -9` to close it. media_kit is the same libmpv rendering into a
// Flutter texture, so the picture is part of the window and the controls are
// widgets over it.
//
// Hosted above the section stack, like the music overlay: a film started from
// Live TV keeps playing while you look at something else, and it is the same
// player either way.
//
// Docked, it is still that same player — the same widget, the same libmpv, in
// a smaller box. That is the whole trick: nothing is torn down, so nothing
// reloads, the position keeps counting and a live stream is not re-opened at
// the edge. What changes is the rectangle it is given and which controls it
// draws.

import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:media_kit/media_kit.dart';
import 'package:media_kit_video/media_kit_video.dart';

import '../design/pick.dart';
import '../design/tokens.dart';
import '../shell/window.dart';
import '../src/rust/api/videos.dart';
import 'video_controls.dart';
import 'video_info.dart';
import 'video_keys.dart';
import 'video_subs.dart';

/// The docked card at full size. Shrinks to fit a window too narrow for it.
const double kVideoDockW = 384;

/// Its margin from the window edge — the same one the docked music bubble
/// keeps, so the two sit on the same invisible gutter.
const double kVideoDockGap = 18;

/// Everything the layer needs to show a video, or null when nothing is on.
///
/// Two sections put pictures here now — Videos, and a YouTube music video —
/// and they report back to different halves of the bridge. Rather than teach
/// the layer which is which, the requester supplies the two callbacks; the
/// defaults are the Videos ones, so that side needed no change.
class VideoRequest {
  const VideoRequest({
    required this.token,
    required this.src,
    required this.startAt,
    required this.props,
    this.onStarted,
    this.onEnded,
  });

  final int token;
  final String src;
  final double startAt;
  final List<String> props;

  /// Fired once, when the clock first runs. Live TV's status line waits on it.
  final void Function(int token)? onStarted;

  /// Fired on every way out: end of file, Escape, the close button, or Rust
  /// replacing the source. This is what writes the position back.
  final Future<void> Function(int token, double pos, double dur)? onEnded;
}

/// What is on screen, set by the videos controller from the bridge's events.
final ValueNotifier<VideoRequest?> videoRequest = ValueNotifier(null);

/// Docked to the corner rather than filling the window.
///
/// It survives a source change on purpose. The autoplay chain starting the
/// next episode, and picking another film while docked, both arrive as the
/// same `VideoPlay` and both should stay in the corner — undocking on the
/// second episode of a series would be the surprise, not the courtesy.
final ValueNotifier<bool> videoDocked = ValueNotifier(false);

/// Whether a picture is playing. The lock screen's idle clock stands still
/// while one is: a film being watched is not a window left alone.
final ValueNotifier<bool> videoPlaying = ValueNotifier(false);

/// For the lock screen, set while a player exists: the paused frame as a
/// JPEG, and the way to carry on after unlocking.
Future<Uint8List?> Function()? videoFrame;
VoidCallback? videoResume;

/// Which player set the two above, so only that one clears them — a new
/// source's player can open before the old one has gone.
Object? _hooksOwner;

/// Put the player away.
///
/// The dock flag goes with it, so the next thing started opens full rather
/// than in a corner nobody asked for. Three places end playback — this file,
/// and the two controllers that answer a `VideoStop` — and they all mean the
/// same thing by it, which is why it is a function rather than an assignment
/// repeated three times and forgotten in one.
void clearVideo() {
  videoRequest.value = null;
  videoDocked.value = false;
  // A file that ends mid-drag would otherwise leave a stale offset for the next
  // card to appear at. The pinned spot survives; the drag in progress does not.
  videoDockDrag.value = null;
}

/// Where the docked card is pinned.
///
/// A side and a fraction rather than a point, so resizing the window keeps the
/// card proportionally where it was put and can never leave it off-screen —
/// which a stored pixel offset does the first time the window is made smaller.
class VideoDockSpot {
  const VideoDockSpot({required this.right, required this.y});

  /// Pinned to the window's right edge rather than its left.
  final bool right;

  /// How far down the free vertical travel the card sits: 0 is against the top
  /// gutter, 1 against the bottom one.
  final double y;

  @override
  bool operator ==(Object other) =>
      other is VideoDockSpot && other.right == right && other.y == y;

  @override
  int get hashCode => Object.hash(right, y);
}

/// The corner the card returns to. Bottom-right, which is where it always was.
const VideoDockSpot kDefaultDockSpot = VideoDockSpot(right: true, y: 1);

/// Where the card is pinned, remembered between runs.
final ValueNotifier<VideoDockSpot> videoDockSpot =
    ValueNotifier(loadDockSpot());

/// The card's live top-left while it is being dragged, null the rest of the
/// time. The drag is free; the snap to a side happens on release.
final ValueNotifier<Offset?> videoDockDrag = ValueNotifier(null);

VideoDockSpot loadDockSpot() {
  try {
    final side = playerPrefGet(key: 'player.dock.side').trim();
    if (side.isEmpty) return kDefaultDockSpot;
    final y = double.tryParse(playerPrefGet(key: 'player.dock.y').trim());
    return VideoDockSpot(
      right: side != 'left',
      y: (y ?? 1).clamp(0.0, 1.0),
    );
  } catch (_) {
    // No bridge, no memory. Tests run this way, and so does the first launch.
    return kDefaultDockSpot;
  }
}

void saveDockSpot(VideoDockSpot spot) {
  try {
    playerPrefSet(
        key: 'player.dock.side', value: spot.right ? 'right' : 'left');
    playerPrefSet(key: 'player.dock.y', value: spot.y.toStringAsFixed(4));
  } catch (_) {
    // Losing the position is not worth interrupting playback for.
  }
}

/// The card's width in a window of this size — the fixed width, unless the
/// window is too narrow to hold it and its two gutters.
double dockCardWidth(Size area) =>
    math.max(0.0, math.min(kVideoDockW, area.width - kVideoDockGap * 2));

/// Where the picture sits: the whole window, or a 16:9 card on one edge.
Rect videoStageRect(Size area, bool docked,
    [VideoDockSpot spot = kDefaultDockSpot]) {
  if (!docked) return Offset.zero & area;
  final w = dockCardWidth(area);
  final h = w * 9 / 16;
  final travel = math.max(0.0, area.height - h - kVideoDockGap * 2);
  return Rect.fromLTWH(
    math.max(0.0, spot.right ? area.width - w - kVideoDockGap : kVideoDockGap),
    math.max(0.0, kVideoDockGap + travel * spot.y.clamp(0.0, 1.0)),
    w,
    h,
  );
}

/// The spot a card dropped at [topLeft] settles into.
///
/// The side is whichever half the card's own centre ended up in, not the
/// pointer's — dragging by the left corner of a 384px card would otherwise
/// keep flinging it left. The height is kept as dropped, because "up and down
/// on those sides" is the point of being able to move it at all.
VideoDockSpot dockSpotFor(Offset topLeft, Size area) {
  final w = dockCardWidth(area);
  final h = w * 9 / 16;
  final travel = math.max(0.0, area.height - h - kVideoDockGap * 2);
  return VideoDockSpot(
    right: topLeft.dx + w / 2 >= area.width / 2,
    y: travel <= 0
        ? 0.0
        : ((topLeft.dy - kVideoDockGap) / travel).clamp(0.0, 1.0),
  );
}

/// Whether the full-window stage hides the app beneath it.
///
/// It does once a picture is playing in the window and has finished arriving
/// there. A new source opens straight onto the full rect, but a move between
/// the corner card and the window slides for 240 ms, and the app has to show
/// around the card while it does. Asked on every build of [VideoLayer]; told
/// by [settle] when a slide ends.
class StageCover {
  int? _token;
  bool _docked = false;
  bool _settled = false;

  bool covers({required int? token, required bool docked}) {
    if (token != _token) {
      _token = token;
      _settled = token != null && !docked;
    } else if (docked != _docked) {
      _settled = false;
    }
    _docked = docked;
    return token != null && !docked && _settled;
  }

  /// A slide ended. True when that changes what [covers] answers.
  bool settle() {
    if (_settled) return false;
    _settled = true;
    return _token != null && !_docked;
  }
}

/// Wraps the app. Draws nothing until something is playing.
class VideoLayer extends StatefulWidget {
  const VideoLayer({super.key, required this.child});

  final Widget child;

  @override
  State<VideoLayer> createState() => _VideoLayerState();
}

class _VideoLayerState extends State<VideoLayer> {
  final StageCover _cover = StageCover();

  @override
  Widget build(BuildContext context) {
    // One builder over the four things that move the picture, rather than four
    // nested ones.
    return AnimatedBuilder(
      animation: Listenable.merge(
        [videoRequest, videoDocked, videoDockSpot, videoDockDrag],
      ),
      builder: (context, _) {
        final request = videoRequest.value;
        final docked = videoDocked.value;
        final drag = videoDockDrag.value;

        // The window, which is also this Stack: `home` is this widget, so
        // there is no app bar or inset between the two and MediaQuery's
        // size is the Stack's size. Measured that way rather than with a
        // LayoutBuilder because `AnimatedPositioned` is a ParentDataWidget
        // and has to land directly on the RenderStack — a LayoutBuilder in
        // between makes it a RenderLayoutBuilder's child and throws at
        // build time, which is the trap music_overlay.dart documents.
        final rect = videoStageRect(
            MediaQuery.sizeOf(context), docked, videoDockSpot.value);
        // Under the finger while dragging, easing to the edge on release.
        // Animating a drag would make the card lag the pointer by a quarter
        // of a second, which reads as the drag not having taken.
        final at = drag != null && docked ? drag & rect.size : rect;
        // Under a picture that fills the window, the app is not drawn at all.
        // Every video frame is a frame of the whole scene -- this renderer has
        // no partial repaint -- so the app beneath was laid out, painted and
        // rasterised at the film's frame rate, design language and all, behind
        // an opaque black stage. Offstage keeps every page's State; TickerMode
        // stops what animates down there, which would ask for frames of its
        // own; ExcludeFocus keeps the keyboard with the film. Flags only, so the
        // tree's shape never changes and nothing underneath is rebuilt.
        final covered = _cover.covers(token: request?.token, docked: docked);
        return Stack(
          children: [
            ExcludeFocus(
              excluding: covered,
              child: TickerMode(
                enabled: !covered,
                child: Offstage(offstage: covered, child: widget.child),
              ),
            ),
            if (request != null)
              AnimatedPositioned(
                duration: drag != null
                    ? Duration.zero
                    : const Duration(milliseconds: 240),
                curve: Curves.easeOutCubic,
                // The app shows around the card while it slides; it goes once
                // the picture has arrived in the window.
                onEnd: () {
                  if (_cover.settle()) setState(() {});
                },
                left: at.left,
                top: at.top,
                width: at.width,
                height: at.height,
                // Keyed on the token: a new source is a new player rather
                // than a reused one, which is what makes switching
                // channels reliable — libmpv holding a dead live stream
                // open is the failure the old build worked around by
                // killing the process. Docking is not a new token, so the
                // same player slides into the corner still running.
                child: _VideoStage(
                  key: ValueKey(request.token),
                  request: request,
                  docked: docked,
                ),
              ),
          ],
        );
      },
    );
  }
}

class _VideoStage extends StatefulWidget {
  const _VideoStage({
    super.key,
    required this.request,
    required this.docked,
  });

  final VideoRequest request;
  final bool docked;

  @override
  State<_VideoStage> createState() => _VideoStageState();
}

class _VideoStageState extends State<_VideoStage> {
  late final Player _player = Player(
    configuration: PlayerConfiguration(libass: _libass()),
  );
  late final VideoController _controller = VideoController(
    _player,
    configuration: VideoControllerConfiguration(hwdec: _hwdec()),
  );
  late final VideoOps _ops = VideoOps(_player, source: widget.request.src);
  final FocusNode _focus = FocusNode();
  final List<StreamSubscription<dynamic>> _subs = [];

  /// Sent once, when the clock first runs. Live TV's status line waits for it.
  bool _reportedStart = false;

  /// The last position seen. Read on the way out, because that is what gets
  /// written back into `watch_progress` — and by then the player may already
  /// have been torn down.
  double _pos = 0;
  double _dur = 0;

  /// Whether the chrome is up. It goes away three seconds after the pointer
  /// stops, and only while something is playing: a paused film with no visible
  /// controls looks like a crash.
  bool _chrome = true;
  Timer? _idle;

  /// Whether *this* put the window into fullscreen. The zen player uses the
  /// same switch, so the way out has to be "undo what I did" rather than
  /// "turn it off".
  bool _fullscreen = false;

  /// Whether the pointer is over the docked card.
  bool _hover = false;

  /// A line of feedback over the picture — where a screenshot went, which
  /// subtitle loaded. A SnackBar would be drawn by the Scaffold underneath and
  /// so would appear behind the very thing it is reporting on.
  String? _flash;
  Timer? _flashTimer;

  NativePlayer get _native => _player.platform as NativePlayer;

  /// How mpv decodes, which media_kit otherwise leaves at `auto`.
  ///
  /// `auto` probes every interop path the machine might have, in mpv's own
  /// order — which on a box with an Intel iGPU and no NVIDIA driver means
  /// trying VDPAU first and logging a failure to load `libvdpau_nvidia.so`
  /// before it gets to VA-API.
  ///
  /// `auto-copy` is mpv's hybrid, and its manual is explicit about what it is:
  /// "selects only modes that copy the video data back to system memory". The
  /// GPU decodes, the frames come back, and rendering goes through the ordinary
  /// path. That keeps hardware decoding while staying out of the zero-copy GPU
  /// interop — which is the fragile part of this stack, because the frames have
  /// to end up in a Flutter texture either way and every interop bug in that
  /// handoff shows up as a picture that is one flat colour.
  ///
  /// Overridable: a machine whose interop is good gets more from `auto-safe`,
  /// and `no` is the way to rule decoding out while chasing something else.
  /// Who draws the subtitles.
  ///
  /// Off, media_kit observes mpv's `sub-text` and draws the line in Dart, which
  /// is the only way it can be placed above the control bar — mpv burns its own
  /// into the frame, wherever the frame ends. That is the default because
  /// position is what was asked for.
  ///
  /// The cost: `sub-text` is documented as empty for anything that is not
  /// text, so PGS and VobSub — the image-based tracks most Blu-ray rips carry —
  /// produce nothing at all. `player.libass` = `yes` hands rendering back to
  /// mpv, which draws those, and ASS styling and positioning with them.
  static bool _libass() {
    try {
      return playerPrefGet(key: 'player.libass').trim() == 'yes';
    } catch (_) {
      return false;
    }
  }

  static String _hwdec() {
    try {
      final override = playerPrefGet(key: 'player.hwdec').trim();
      if (override.isNotEmpty) return override;
    } catch (_) {
      // No bridge, no override. The default is the point of having one.
    }
    return 'auto-copy';
  }

  @override
  void initState() {
    super.initState();
    _open();
  }

  @override
  void didUpdateWidget(_VideoStage old) {
    super.didUpdateWidget(old);
    // Coming back out of the corner, take the keyboard back. `autofocus` fires
    // once when the node is first attached and never again, so without this
    // Escape and the space bar would work before the first dock and not after.
    //
    // After the frame, not during it. This runs *before* the build that hands
    // the Focus below `canRequestFocus: true`, and `FocusNode.requestFocus`
    // returns silently on a node that cannot take focus — so asking here asks
    // the docked node, and nothing happens. That is the whole of the bug where
    // a film played from the Videos section took Escape (a fresh node, whose
    // `autofocus` fired on attach) and the same film restored from the corner
    // did not.
    if (old.docked && !widget.docked) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _focus.requestFocus();
      });
      _wake();
    }
  }

  Future<void> _open() async {
    final r = widget.request;
    for (final p in r.props) {
      final i = p.indexOf('=');
      if (i <= 0) continue;
      // `sub-file` repeats — mpv appends each to a list, and the first is the
      // track the user chose. `setProperty` would replace, so these are added
      // the way the CLI added them.
      final name = p.substring(0, i);
      final value = p.substring(i + 1);
      if (name == 'sub-file') {
        await _native.command(['sub-add', value, 'auto']);
      } else {
        await _native.setProperty(name, value);
      }
    }
    // A player per source, so there is nothing stale to clear — but `start` is
    // an option rather than a one-shot, and being explicit is what keeps that
    // true if this ever reuses one.
    await _native.setProperty(
      'start',
      r.startAt > 0 ? r.startAt.toStringAsFixed(3) : 'none',
    );
    _subs.add(_player.stream.position.listen((p) {
      _pos = p.inMilliseconds / 1000.0;
      if (!_reportedStart && _pos > 0) {
        _reportedStart = true;
        final started = r.onStarted;
        if (started != null) {
          started(r.token);
        } else {
          videosPlaybackStarted(token: r.token);
        }
      }
    }));
    _subs.add(_player.stream.duration.listen((d) {
      final was = _dur;
      _dur = d.inMilliseconds / 1000.0;
      // Chapters are read once, when a duration first arrives: before that the
      // demuxer has not necessarily parsed the container far enough to have
      // them, and asking twice for a file that has none costs two more failed
      // property reads for nothing.
      if (was <= 0 && _dur > 0) _ops.loadChapters();
    }));
    _hooksOwner = this;
    videoFrame = () => _player.screenshot();
    videoResume = () {
      _player.play();
    };
    _subs.add(_player.stream.playing.listen((playing) {
      videoPlaying.value = playing;
      // Pausing brings the chrome back and keeps it; playing starts the clock
      // that takes it away again.
      if (mounted) _wake();
    }));
    _subs.add(_player.stream.completed.listen((done) {
      if (done) _close();
    }));
    await _player.open(Media(r.src));
  }

  /// Every way out of the player goes through here: end of file, Escape, the
  /// close button, and Rust replacing the source. The position report is what
  /// writes a film's place back into the library, so it must not be skipped on
  /// the paths that are not end-of-file.
  Future<void> _close() async {
    final token = widget.request.token;
    final pos = _pos;
    final dur = _dur;
    final ended = widget.request.onEnded;
    _leaveFullscreen();
    if (videoRequest.value?.token == token) {
      clearVideo();
    }
    if (ended != null) {
      await ended(token, pos, dur);
    } else {
      await videosPlaybackEnded(token: token, pos: pos, dur: dur);
    }
  }

  @override
  void dispose() {
    for (final s in _subs) {
      s.cancel();
    }
    _idle?.cancel();
    _flashTimer?.cancel();
    // Leaving a window fullscreen because the film ended by itself would strand
    // the app with no title bar and nothing playing.
    _leaveFullscreen();
    _focus.dispose();
    _ops.dispose();
    if (_hooksOwner == this) {
      _hooksOwner = null;
      videoFrame = null;
      videoResume = null;
      videoPlaying.value = false;
    }
    _player.dispose();
    super.dispose();
  }

  // --------------------------------------------------------------- chrome ---

  void _wake() {
    _idle?.cancel();
    if (!_chrome && mounted) setState(() => _chrome = true);
    _idle = Timer(const Duration(seconds: 3), () {
      if (mounted && _player.state.playing && !widget.docked) {
        setState(() => _chrome = false);
      }
    });
  }

  void _flashMessage(String message) {
    _flashTimer?.cancel();
    setState(() => _flash = message);
    _flashTimer = Timer(const Duration(seconds: 4), () {
      if (mounted) setState(() => _flash = null);
    });
  }

  void _toggleFullscreen() {
    final on = !_fullscreen;
    setState(() => _fullscreen = on);
    setWindowFullscreen(on);
  }

  void _leaveFullscreen() {
    if (!_fullscreen) return;
    _fullscreen = false;
    setWindowFullscreen(false);
  }

  void _dock() {
    // Docking out of fullscreen would leave a 384 px card on an otherwise black
    // screen with no way back to the app.
    _leaveFullscreen();
    videoDocked.value = true;
  }

  // -------------------------------------------------------------- actions ---

  Future<void> _screenshot() async {
    final path = await _ops.screenshot();
    if (!mounted) return;
    _flashMessage(path == null ? 'Could not save a screenshot' : 'Saved $path');
  }

  Future<void> _pickSubtitleFile() async {
    final path = await pickFile(
      label: 'Subtitles',
      extensions: const ['srt', 'ass', 'ssa', 'vtt', 'sub', 'sup'],
    );
    if (path == null || !mounted) return;
    _ops.addSubtitleFile(path);
    _flashMessage('Loaded subtitles');
  }

  Future<void> _findSubtitles() async {
    final path = await findSubtitles(
      context,
      source: _ops.source,
      title: _ops.title,
    );
    if (path == null || !mounted) return;
    _ops.addSubtitleFile(path);
    _flashMessage('Loaded $path');
  }

  void _run(VideoAction action) {
    switch (action) {
      case VideoAction.playPause:
        _player.playOrPause();
      case VideoAction.speedUp:
        _ops.nudgeRate(0.25);
      case VideoAction.speedDown:
        _ops.nudgeRate(-0.25);
      case VideoAction.speedReset:
        _ops.setRate(1);
      case VideoAction.seekBack5:
        _ops.seekBy(-5);
      case VideoAction.seekFwd5:
        _ops.seekBy(5);
      case VideoAction.seekBack10:
        _ops.seekBy(-10);
      case VideoAction.seekFwd10:
        _ops.seekBy(10);
      case VideoAction.prevChapter:
        _ops.jumpChapter(-1);
      case VideoAction.nextChapter:
        _ops.jumpChapter(1);
      case VideoAction.volumeUp:
        _ops.nudgeVolume(5);
      case VideoAction.volumeDown:
        _ops.nudgeVolume(-5);
      case VideoAction.mute:
        _ops.toggleMute();
      case VideoAction.cycleAudio:
        _ops.cycleAudio();
      case VideoAction.toggleSubtitles:
        _ops.toggleSubtitles();
      case VideoAction.cycleSubtitle:
        _ops.cycleSubtitle();
      case VideoAction.findSubtitles:
        _findSubtitles();
      case VideoAction.screenshot:
        _screenshot();
      case VideoAction.fullscreen:
        _toggleFullscreen();
      case VideoAction.minimise:
        _dock();
      case VideoAction.close:
        _close();
    }
    _wake();
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    if (event is! KeyDownEvent) return KeyEventResult.ignored;
    // Escape leaves fullscreen before it means anything else. It is bound to
    // "minimise" by default and that is what it does in a window — but every
    // fullscreen surface on every desktop answers Escape by giving the window
    // back, and a player that docked instead would be the odd one out.
    if (event.logicalKey == LogicalKeyboardKey.escape && _fullscreen) {
      _leaveFullscreen();
      setState(() {});
      return KeyEventResult.handled;
    }
    final action = VideoKeymap.instance.resolve(event);
    if (action == null) return KeyEventResult.ignored;
    // Held: seek and keep seeking, but swallow everything else rather than
    // letting the OS repeat rate fire it. The seek itself is throttled and
    // accumulated in VideoOps — see the note on `seekTarget`.
    if (event is KeyRepeatEvent && !kRepeatableActions.contains(action)) {
      return KeyEventResult.handled;
    }
    _run(action);
    return KeyEventResult.handled;
  }

  // ---------------------------------------------------------------- build ---

  @override
  Widget build(BuildContext context) {
    final docked = widget.docked;
    return Focus(
      focusNode: _focus,
      autofocus: !docked,
      // A docked player must not hold the keyboard. The app underneath is
      // being used, and the space bar belongs to whatever is being typed into.
      canRequestFocus: !docked,
      onKeyEvent: _onKey,
      child: docked ? _dockedCard() : _fullView(),
    );
  }

  // Two fixed instances rather than one built per frame:
  // SubtitleViewConfiguration has no `==`, so Video takes every new object as a
  // change and pushes it through its view-parameter notifier, and this widget
  // rebuilds on every hover.
  static const _subsLow = SubtitleViewConfiguration(
    padding: EdgeInsets.fromLTRB(24, 0, 24, 24),
  );
  static const _subsRaised = SubtitleViewConfiguration(
    padding: EdgeInsets.fromLTRB(24, 0, 24, kVideoControlsHeight + 16),
  );

  Widget _fullView() {
    return MouseRegion(
      // The cursor goes with the chrome. A pointer parked over a film is the
      // one thing a player is expected to hide.
      cursor: _chrome ? MouseCursor.defer : SystemMouseCursors.none,
      onHover: (_) => _wake(),
      child: ColoredBox(
        color: Colors.black,
        child: Stack(
          children: [
            Positioned.fill(
              child: Listener(
                // Scroll anywhere over the picture is volume. The seek bar
                // registers for the same signal and wins it, because the
                // resolver gives a pointer signal to the innermost registrant.
                onPointerSignal: (event) {
                  if (event is! PointerScrollEvent) return;
                  GestureBinding.instance.pointerSignalResolver.register(
                    event,
                    (e) => _ops
                        .scrollVolume((e as PointerScrollEvent).scrollDelta.dy),
                  );
                  _wake();
                },
                child: GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTap: _player.playOrPause,
                  onDoubleTap: _toggleFullscreen,
                  child: Video(
                    controller: _controller,
                    controls: NoVideoControls,
                    // Subtitles sit 24px off the bottom by default, which is
                    // squarely behind the control bar — the line was being
                    // drawn, under the buttons. They ride above the bar while
                    // the chrome is up and drop back when it hides.
                    subtitleViewConfiguration: _chrome ? _subsRaised : _subsLow,
                  ),
                ),
              ),
            ),
            _chromeLayer(
              Positioned(
                top: 12,
                right: 12,
                // Opaque so a click on the strip beside the buttons is not also
                // a click on the picture behind it, which would pause the film.
                child: Listener(
                  behavior: HitTestBehavior.opaque,
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      _StageButton(
                        icon: Icons.picture_in_picture_alt_rounded,
                        tooltip: 'Minimise',
                        onTap: _dock,
                      ),
                      const SizedBox(width: 8),
                      _StageButton(
                        icon: Icons.close,
                        tooltip: 'Close',
                        onTap: _close,
                      ),
                    ],
                  ),
                ),
              ),
            ),
            _chromeLayer(
              Positioned(
                left: 0,
                right: 0,
                bottom: 0,
                child: Listener(
                  behavior: HitTestBehavior.opaque,
                  child: VideoControls(
                    ops: _ops,
                    fullscreen: _fullscreen,
                    onToggleFullscreen: _toggleFullscreen,
                    onFindSubtitles: _findSubtitles,
                    onPickSubtitleFile: _pickSubtitleFile,
                    onEditShortcuts: () => showVideoShortcuts(context),
                    onMediaInfo: () => showMediaInfo(context, _ops),
                    onScreenshot: _screenshot,
                  ),
                ),
              ),
            ),
            if (_flash != null)
              Positioned(
                left: 18,
                top: 18,
                right: 120,
                child: _Flash(message: _flash!),
              ),
          ],
        ),
      ),
    );
  }

  /// Fades a piece of chrome out with the rest, and stops it taking clicks once
  /// it is invisible — an invisible close button is still a close button.
  Widget _chromeLayer(Positioned child) {
    return Positioned(
      left: child.left,
      top: child.top,
      right: child.right,
      bottom: child.bottom,
      width: child.width,
      height: child.height,
      child: IgnorePointer(
        ignoring: !_chrome,
        child: AnimatedOpacity(
          opacity: _chrome ? 1 : 0,
          duration: const Duration(milliseconds: 180),
          child: child.child,
        ),
      ),
    );
  }

  void _dragStart() {
    videoDockDrag.value =
        videoStageRect(MediaQuery.sizeOf(context), true, videoDockSpot.value)
            .topLeft;
  }

  void _dragBy(DragUpdateDetails d) {
    final at = videoDockDrag.value;
    if (at == null) return;
    videoDockDrag.value = at + d.delta;
  }

  void _dragEnd() {
    final at = videoDockDrag.value;
    videoDockDrag.value = null;
    if (at == null) return;
    final spot = dockSpotFor(at, MediaQuery.sizeOf(context));
    videoDockSpot.value = spot;
    saveDockSpot(spot);
  }

  Widget _dockedCard() {
    final radius = BorderRadius.circular(Tokens.radiusMd);
    return DecoratedBox(
      decoration: BoxDecoration(
        color: Colors.black,
        borderRadius: radius,
        boxShadow: const [
          BoxShadow(
              color: Color(0x73000000), blurRadius: 28, offset: Offset(0, 10)),
        ],
      ),
      child: ClipRRect(
        borderRadius: radius,
        child: Stack(
          children: [
            Positioned.fill(
              child: Video(
                controller: _controller,
                controls: NoVideoControls,
                // A 216px-tall picture cannot carry full-size subtitles, and
                // scaling them down to fit makes them unreadable rather than
                // small. They come back with the picture.
                subtitleViewConfiguration:
                    const SubtitleViewConfiguration(visible: false),
              ),
            ),
            Positioned.fill(
              child: MouseRegion(
                // The card is draggable, and a cursor is the only thing that
                // says so on a control with no handle.
                cursor: SystemMouseCursors.move,
                onEnter: (_) => setState(() => _hover = true),
                onExit: (_) => setState(() => _hover = false),
                child: GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  // The picture is the way back, as it is on YouTube. The
                  // buttons drawn over it win the gesture arena, so pressing
                  // one is not also a tap on this.
                  onTap: () => videoDocked.value = false,
                  onPanStart: (_) => _dragStart(),
                  onPanUpdate: _dragBy,
                  onPanEnd: (_) => _dragEnd(),
                  onPanCancel: _dragEnd,
                  child: AnimatedOpacity(
                    opacity: _hover ? 1 : 0,
                    duration: const Duration(milliseconds: 120),
                    child:
                        IgnorePointer(ignoring: !_hover, child: _dockChrome()),
                  ),
                ),
              ),
            ),
            // Last, so its eight pixels win over the tap-to-expand above: the
            // corner card is small, but a film you cannot scrub is a film you
            // have to restore to use.
            Positioned(
              left: 0,
              right: 0,
              bottom: 0,
              child: VideoSeekBar(
                ops: _ops,
                accent: Tokens.accentOf(Section.videos),
                compact: true,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _dockChrome() {
    return ColoredBox(
      color: const Color(0x73000000),
      child: Stack(
        children: [
          Center(
            child: StreamBuilder<bool>(
              stream: _player.stream.playing,
              initialData: _player.state.playing,
              builder: (context, snap) {
                final playing = snap.data ?? false;
                return _StageButton(
                  icon: playing ? Icons.pause : Icons.play_arrow,
                  tooltip: playing ? 'Pause' : 'Play',
                  onTap: _player.playOrPause,
                  size: 42,
                );
              },
            ),
          ),
          Positioned(
            top: 6,
            left: 6,
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                _StageButton(
                  icon: Icons.replay_10,
                  tooltip: 'Back 10 seconds',
                  onTap: () => _ops.seekBy(-10),
                  size: 28,
                ),
                const SizedBox(width: 6),
                _StageButton(
                  icon: Icons.forward_10,
                  tooltip: 'Forward 10 seconds',
                  onTap: () => _ops.seekBy(10),
                  size: 28,
                ),
              ],
            ),
          ),
          Positioned(
            top: 6,
            right: 6,
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                _StageButton(
                  icon: Icons.open_in_full_rounded,
                  tooltip: 'Expand',
                  onTap: () => videoDocked.value = false,
                  size: 28,
                ),
                const SizedBox(width: 6),
                _StageButton(
                  icon: Icons.close,
                  tooltip: 'Close',
                  onTap: _close,
                  size: 28,
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// A round button over the picture, at whatever size the surface it is on can
/// carry. Both the full-screen chrome and the docked one are white-on-black
/// over video, so they are the same button rather than two.
class _StageButton extends StatelessWidget {
  const _StageButton({
    required this.icon,
    required this.tooltip,
    required this.onTap,
    this.size = 34,
  });

  final IconData icon;
  final String tooltip;
  final VoidCallback onTap;
  final double size;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: tooltip,
      child: Material(
        color: Colors.black54,
        shape: const CircleBorder(),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onTap,
          child: SizedBox(
            width: size,
            height: size,
            child: Icon(icon, size: size * 0.55, color: Colors.white),
          ),
        ),
      ),
    );
  }
}

/// The one-line report that replaces a SnackBar, which the player would cover.
class _Flash extends StatelessWidget {
  const _Flash({required this.message});

  final String message;

  @override
  Widget build(BuildContext context) {
    return Align(
      alignment: Alignment.topLeft,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 9),
        decoration: BoxDecoration(
          color: Colors.black87,
          borderRadius: BorderRadius.circular(Tokens.radiusSm),
        ),
        child: Text(
          message,
          maxLines: 2,
          overflow: TextOverflow.ellipsis,
          style: const TextStyle(color: Colors.white),
        ),
      ),
    );
  }
}
