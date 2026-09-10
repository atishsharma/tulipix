// The audio deck: one media_kit player, driven by Rust.
//
// This is the Dart half of `crates/tulipix-bridge/src/mpv.rs`. There used to be
// a headless mpv process on the far side of a JSON IPC socket; media_kit is the
// same libmpv, in this process, reached over FFI. What crosses the boundary now
// is a source and a list of mpv properties going one way, and a report going
// back about once a second.
//
// Rust still decides everything. Which flags, what ReplayGain does, which tab
// owns the deck, what happens at the end of a track — all of that is in the
// bridge and the frozen domain crate. Nothing here chooses a value; it applies
// the ones it is handed and says what happened.

import 'dart:async';

import 'package:media_kit/media_kit.dart';

import '../src/rust/api/music.dart';

/// Momentary loudness, 0..1, for the visualizer's envelope.
///
/// It used to be an atomic in Rust that the UI polled through a sync bridge
/// symbol, because the ebur128 meter updates many times a second and putting
/// that on the event stream would have made it the loudest thing on it. The
/// value is produced on this side now, so it is a plain variable and the
/// visualizer's ticker reads it directly.
double audioLoudness = 0.0;

/// Where the deck is, in seconds, updated on every position event.
///
/// A plain variable for the same reason [audioLoudness] is one: the position
/// stream fires many times a second, and the visualiser's timer reads this
/// directly to index the current track's precomputed spectrum. The tick sent
/// over the bridge is throttled to whole seconds because the clock and the
/// scrubber only move that often; a spectrum column is a tenth of a second, so
/// it needs the unthrottled value.
double audioPositionS = 0.0;

/// mpv's `ebur128` momentary loudness in LUFS, mapped to 0..1.
///
/// −45 LUFS is about the floor of anything audible and −6 is about as loud as
/// mastered music gets; the same 39 dB window the Slint build used, so the
/// visualizer has the same range it always had.
double _normaliseLufs(double lufs) {
  final v = (lufs + 45.0) / 39.0;
  return v.isNaN ? 0.0 : v.clamp(0.0, 1.0);
}

class AudioDeck {
  AudioDeck._();
  static final AudioDeck instance = AudioDeck._();

  // No `VideoController` is ever attached to this one, which is what makes it
  // audio-only; `vid=no` goes down with every source's properties as well, so a
  // muxed YouTube stream does not pull a video track nobody is looking at.
  final Player _player = Player(
    configuration: const PlayerConfiguration(title: 'Tulipix'),
  );

  /// The launch this deck is playing. Every report carries it, and Rust drops
  /// the ones that belong to a source it has already replaced.
  int _token = -1;

  /// Last values sent, so a report is only sent when something actually moved.
  /// `pos` is compared at whole seconds — the position stream fires far more
  /// often than the clock in the player bar can show.
  int _sentSecond = -1;
  bool? _sentPaused;
  double? _sentVolume;
  bool? _sentMuted;
  String _sentTitle = '';

  /// mpv's own `media-title`, observed rather than taken from the state: for a
  /// radio stream this is the ICY title, which changes mid-stream and is the
  /// only place the actual song is readable.
  String _mediaTitle = '';

  /// Volume and mute as mpv holds them. media_kit's `volume` is the same 0..100
  /// scale, but it has no mute of its own — muting is `volume 0` there — so the
  /// deck keeps the flag and the level apart the way mpv does.
  double _volume = 80.0;
  bool _muted = false;

  bool _started = false;
  final List<StreamSubscription<dynamic>> _subs = [];

  /// Everything inbound runs in the order it arrived.
  ///
  /// The bridge sends `AudioStop` immediately before every `AudioPlay`, and a
  /// `stop()` that finished after the `open()` it preceded would stop the track
  /// that just started. The events arrive in order; this is what keeps their
  /// effects in order too.
  Future<void> _serial = Future.value();

  Future<void> _next(Future<void> Function() work) {
    final done = _serial.then((_) => work());
    _serial = done.catchError((Object _) {});
    return done;
  }

  NativePlayer get _native => _player.platform as NativePlayer;

  /// Wire the streams. Called once, from the music controller's constructor.
  void start() {
    if (_started) return;
    _started = true;

    _subs.add(_player.stream.position.listen((_) => _report()));
    _subs.add(_player.stream.duration.listen((_) => _report()));
    _subs.add(_player.stream.playing.listen((_) => _report()));
    _subs.add(_player.stream.volume.listen((v) {
      // Rust set it, or mpv did; either way the bar should show what is true.
      if (!_muted) _volume = v;
      _report();
    }));
    _subs.add(_player.stream.completed.listen((done) {
      if (done && _token >= 0) musicAudioEnded(token: _token);
    }));
    _subs.add(_player.stream.error.listen((e) {
      if (_token >= 0) musicAudioFailed(token: _token, message: e.toString());
    }));

    // Two properties media_kit does not surface on its own. `media-title` is
    // the radio ICY title; the r128 meter is the visualizer's energy, and it
    // never leaves this process — the notifier above is read by the ticker.
    _native.observeProperty('media-title', (value) async {
      _mediaTitle = value;
      _report();
    });
    _native.observeProperty('af-metadata/vis/lavfi.r128.M', (value) async {
      final lufs = double.tryParse(value);
      audioLoudness = lufs == null ? 0.0 : _normaliseLufs(lufs);
    });
  }

  Future<void> dispose() async {
    for (final s in _subs) {
      await s.cancel();
    }
    _subs.clear();
    await _player.dispose();
  }

  // ------------------------------------------------------------- inbound ---

  /// `MusicEvent_AudioPlay`. Properties go on before the source does: an EQ
  /// chain or an output device applied after `open` would take effect a beat
  /// late, and `start` has to be in place before the first frame is decoded.
  Future<void> play({
    required int token,
    required String src,
    required double startAt,
    required List<String> props,
  }) =>
      _next(() => _play(token, src, startAt, props));

  Future<void> _play(
    int token,
    String src,
    double startAt,
    List<String> props,
  ) async {
    _token = token;
    _sentSecond = -1;
    _sentPaused = null;
    _sentVolume = null;
    _sentMuted = null;
    _sentTitle = '';
    _mediaTitle = '';
    audioLoudness = 0.0;
    await _applyProps(props);
    // Always set, never only when resuming: `start` is an mpv option, not a
    // one-shot, so a resume point left over from the last track would seek the
    // next one to the same offset.
    await _native.setProperty(
      'start',
      startAt > 0 ? startAt.toStringAsFixed(3) : 'none',
    );
    await _player.open(Media(src));
  }

  /// `MusicEvent_AudioStop`.
  Future<void> stop() => _next(() async {
        _token = -1;
        audioLoudness = 0.0;
        await _player.stop();
      });

  /// `MusicEvent_AudioProp`. `value` is a JSON literal, which is the form these
  /// took when they were `set_property` commands down the IPC socket.
  Future<void> setProperty(String name, String value) =>
      _next(() => _setProperty(name, value));

  Future<void> _setProperty(String name, String value) async {
    final v = _unquote(value);
    switch (name) {
      case 'pause':
        // mpv's `pause` is the inverse of media_kit's `playing`.
        if (v == 'true') {
          await _player.pause();
        } else {
          await _player.play();
        }
      case 'volume':
        _volume = double.tryParse(v) ?? _volume;
        if (!_muted) await _player.setVolume(_volume);
      case 'mute':
        _muted = v == 'true' || v == 'yes';
        await _player.setVolume(_muted ? 0.0 : _volume);
      case 'speed':
        await _player.setRate(double.tryParse(v) ?? 1.0);
      default:
        // `af`, `loop-file`, and anything else Rust decides to set: straight
        // through to mpv, which is where they came from.
        await _native.setProperty(name, v);
    }
    _report();
  }

  /// `MusicEvent_AudioSeek`.
  Future<void> seek(double secs) =>
      _next(() => _player.seek(Duration(milliseconds: (secs * 1000).round())));

  // ------------------------------------------------------------ outbound ---

  /// Send a report if anything the player bar can show has changed.
  ///
  /// Position is compared at whole seconds: the stream fires many times a
  /// second and the clock, the scrubber and the lyric line all move once. That
  /// throttle used to live in the Rust reader thread, for the same reason.
  void _report() {
    if (_token < 0) return;
    final pos = _player.state.position.inMilliseconds / 1000.0;
    // Before the throttle: the visualiser wants every one of these.
    audioPositionS = pos;
    final dur = _player.state.duration.inMilliseconds / 1000.0;
    final paused = !_player.state.playing;
    final second = pos.floor();
    if (second == _sentSecond &&
        paused == _sentPaused &&
        _volume == _sentVolume &&
        _muted == _sentMuted &&
        _mediaTitle == _sentTitle) {
      return;
    }
    _sentSecond = second;
    _sentPaused = paused;
    _sentVolume = _volume;
    _sentMuted = _muted;
    _sentTitle = _mediaTitle;
    musicAudioTick(
      token: _token,
      pos: pos,
      dur: dur,
      paused: paused,
      volume: _volume,
      muted: _muted,
      title: _mediaTitle,
    );
  }

  /// `"inf"` → `inf`. Rust sends JSON literals, and mpv wants the bare value.
  static String _unquote(String v) =>
      v.length >= 2 && v.startsWith('"') && v.endsWith('"')
          ? v.substring(1, v.length - 1)
          : v;

  /// `k=v` in order. mpv cares about the order — an `af` set before the source
  /// is opened is part of the chain the first sample goes through.
  Future<void> _applyProps(List<String> props) async {
    for (final p in props) {
      final i = p.indexOf('=');
      if (i <= 0) continue;
      final name = p.substring(0, i);
      final value = p.substring(i + 1);
      switch (name) {
        case 'volume':
          _volume = double.tryParse(value) ?? _volume;
          await _player.setVolume(_muted ? 0.0 : _volume);
        case 'mute':
          _muted = value == 'yes' || value == 'true';
          await _player.setVolume(_muted ? 0.0 : _volume);
        default:
          await _native.setProperty(name, value);
      }
    }
  }
}
