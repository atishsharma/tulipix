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

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:media_kit/media_kit.dart';
import 'package:media_kit_video/media_kit_video.dart';

import '../src/rust/api/videos.dart';

/// Everything the layer needs to show a video, or null when nothing is on.
class VideoRequest {
  const VideoRequest({
    required this.token,
    required this.src,
    required this.startAt,
    required this.props,
  });

  final int token;
  final String src;
  final double startAt;
  final List<String> props;
}

/// What is on screen, set by the videos controller from the bridge's events.
final ValueNotifier<VideoRequest?> videoRequest = ValueNotifier(null);

/// Wraps the app. Draws nothing until something is playing.
class VideoLayer extends StatelessWidget {
  const VideoLayer({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder<VideoRequest?>(
      valueListenable: videoRequest,
      builder: (context, request, _) {
        return Stack(
          children: [
            child,
            if (request != null)
              Positioned.fill(
                // Keyed on the token: a new source is a new player rather than
                // a reused one, which is what makes switching channels reliable
                // — libmpv holding a dead live stream open is the failure the
                // old build worked around by killing the process.
                child: _VideoStage(key: ValueKey(request.token), request: request),
              ),
          ],
        );
      },
    );
  }
}

class _VideoStage extends StatefulWidget {
  const _VideoStage({super.key, required this.request});

  final VideoRequest request;

  @override
  State<_VideoStage> createState() => _VideoStageState();
}

class _VideoStageState extends State<_VideoStage> {
  late final Player _player = Player();
  late final VideoController _controller = VideoController(_player);
  final FocusNode _focus = FocusNode();
  final List<StreamSubscription<dynamic>> _subs = [];

  /// Sent once, when the clock first runs. Live TV's status line waits for it.
  bool _reportedStart = false;

  /// The last position seen. Read on the way out, because that is what gets
  /// written back into `watch_progress` — and by then the player may already
  /// have been torn down.
  double _pos = 0;
  double _dur = 0;

  NativePlayer get _native => _player.platform as NativePlayer;

  @override
  void initState() {
    super.initState();
    _open();
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
        videosPlaybackStarted(token: r.token);
      }
    }));
    _subs.add(_player.stream.duration.listen((d) => _dur = d.inMilliseconds / 1000.0));
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
    if (videoRequest.value?.token == token) {
      videoRequest.value = null;
    }
    await videosPlaybackEnded(token: token, pos: pos, dur: dur);
  }

  @override
  void dispose() {
    for (final s in _subs) {
      s.cancel();
    }
    _focus.dispose();
    _player.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Focus(
      focusNode: _focus,
      autofocus: true,
      onKeyEvent: (node, event) {
        if (event is! KeyDownEvent) return KeyEventResult.ignored;
        switch (event.logicalKey) {
          case LogicalKeyboardKey.escape:
            _close();
            return KeyEventResult.handled;
          case LogicalKeyboardKey.space:
            _player.playOrPause();
            return KeyEventResult.handled;
          case LogicalKeyboardKey.arrowLeft:
            _player.seek(Duration(seconds: (_pos - 10).clamp(0, double.infinity).round()));
            return KeyEventResult.handled;
          case LogicalKeyboardKey.arrowRight:
            _player.seek(Duration(seconds: (_pos + 10).round()));
            return KeyEventResult.handled;
          default:
            return KeyEventResult.ignored;
        }
      },
      child: ColoredBox(
        color: Colors.black,
        child: Stack(
          children: [
            Positioned.fill(
              // media_kit's own desktop controls: seek bar, volume, playback
              // speed, subtitle and audio track pickers, fullscreen. The old
              // build got all of that from mpv's OSC, and re-drawing it here
              // would be a week for something the package already ships.
              // ponytail: no live-edge seek on an audio-track change. The old
              // build watched `aid` over IPC and seeked to 100% when it moved,
              // because a new track on a live stream downloads from the edge and
              // leaves the picture behind. The picker belongs to these controls,
              // so hooking it means replacing them — worth it only if switching
              // audio on Live TV turns out to matter.
              child: Video(controller: _controller, controls: AdaptiveVideoControls),
            ),
            Positioned(
              top: 12,
              right: 12,
              child: IconButton(
                tooltip: 'Close (Esc)',
                icon: const Icon(Icons.close, color: Colors.white),
                style: IconButton.styleFrom(backgroundColor: Colors.black54),
                onPressed: _close,
              ),
            ),
          ],
        ),
      ),
    );
  }
}
