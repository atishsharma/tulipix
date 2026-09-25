// The Voice section's state, and the player a note is heard on.
//
// The bridge keeps the session — tab, filter, the open note, the search — and
// the recording itself, so this holds the last snapshot and the notice it left.
// What it adds is asking again: every second while recording, so the clock
// moves, and while whisper still has notes to read, so they fill in.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:media_kit/media_kit.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/voice.dart';

export '../kitchen/kitchen_controller.dart' show plainError, plural;

typedef VoiceTab = ({String id, String label});

/// The header's tabs, in the order it draws them. Also Settings → Sections'
/// list of what can be switched off.
const List<VoiceTab> voiceTabs = [
  (id: 'notes', label: 'Notes'),
  (id: 'ask', label: 'Ask'),
  (id: 'tasks', label: 'Tasks'),
  (id: 'setup', label: 'Setup'),
];

const Color kVoice = Tokens.secVoice;
const Color kVoice2 = Tokens.secVoice2;

/// Audio and video files whisper can be given; ffmpeg reads them in.
const List<String> kVoiceExtensions = [
  'wav', 'mp3', 'm4a', 'aac', 'ogg', 'opus', 'flac', 'webm', 'mp4', 'mkv', 'mov', //
];

class VoiceController extends ChangeNotifier {
  VoiceState? state;
  Object? error;
  bool busy = false;

  String notice = '';
  Timer? _noticeTimer;
  Timer? _again;

  Future<VoiceState?> send(VoiceCmd cmd, {bool quiet = false}) async {
    if (!quiet) {
      busy = true;
      error = null;
      notifyListeners();
    }
    try {
      final st = await voiceDispatch(cmd: cmd);
      state = st;
      if (st.notice.isNotEmpty) say(st.notice);
      _askAgain(st);
      return st;
    } catch (e) {
      error = e;
      return null;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  Future<void> refresh({bool quiet = false}) =>
      send(const VoiceCmd.refresh(), quiet: quiet);

  void _askAgain(VoiceState st) {
    _again?.cancel();
    final wait = st.recording
        ? const Duration(seconds: 1)
        : st.working > 0
            ? const Duration(milliseconds: 1500)
            : null;
    if (wait != null) _again = Timer(wait, () => refresh(quiet: true));
  }

  /// Stop any polling while the page is off screen; a recording keeps going
  /// in the bridge either way.
  void pause() => _again?.cancel();

  /// A line under the header that goes away on its own.
  void say(String text) {
    notice = text;
    _noticeTimer?.cancel();
    _noticeTimer = Timer(const Duration(seconds: 6), dismissNotice);
    notifyListeners();
  }

  void dismissNotice() {
    notice = '';
    notifyListeners();
  }

  void clearError() {
    error = null;
    notifyListeners();
  }

  @override
  void dispose() {
    _noticeTimer?.cancel();
    _again?.cancel();
    super.dispose();
  }
}

/// One note at a time, with where it is — which is what the transcript's
/// highlight and the waveform's played part follow.
class VoicePlayer extends ChangeNotifier {
  Player? _player;
  final List<StreamSubscription<Object?>> _subs = [];

  /// The note loaded; 0 for none.
  int current = 0;
  bool playing = false;
  int positionMs = 0;
  int durationMs = 0;
  double rate = 1;

  Player _make() {
    final p = Player(
      configuration: const PlayerConfiguration(title: 'Tulipix — voice note'),
    );
    _subs.add(p.stream.playing.listen((on) {
      playing = on;
      notifyListeners();
    }));
    _subs.add(p.stream.position.listen((pos) {
      positionMs = pos.inMilliseconds;
      notifyListeners();
    }));
    _subs.add(p.stream.duration.listen((d) {
      durationMs = d.inMilliseconds;
      notifyListeners();
    }));
    _subs.add(p.stream.completed.listen((done) {
      if (!done) return;
      playing = false;
      notifyListeners();
    }));
    return p;
  }

  Future<void> _load(VoiceNote n, {bool play = true}) async {
    final p = _player ??= _make();
    current = n.id;
    positionMs = 0;
    durationMs = (n.durationS * 1000).round();
    notifyListeners();
    await p.open(Media(n.path), play: play);
    await p.setRate(rate);
  }

  Future<void> toggle(VoiceNote n) async {
    if (n.path.isEmpty) return;
    if (current != n.id) return _load(n);
    await _player?.playOrPause();
  }

  /// Jump to a moment of a note, loading it first if it is not the one here.
  Future<void> seek(VoiceNote n, int ms) async {
    if (n.path.isEmpty) return;
    if (current != n.id) {
      await _load(n);
      // mpv takes a moment to know the file before a seek lands.
      await Future<void>.delayed(const Duration(milliseconds: 250));
    }
    await _player?.seek(Duration(milliseconds: ms));
    if (!playing) await _player?.play();
  }

  Future<void> skip(int deltaMs) async {
    final p = _player;
    if (p == null) return;
    final to = (positionMs + deltaMs)
        .clamp(0, durationMs > 0 ? durationMs : 1 << 30)
        .toInt();
    await p.seek(Duration(milliseconds: to));
  }

  Future<void> setRate(double r) async {
    rate = r;
    notifyListeners();
    await _player?.setRate(r);
  }

  /// Let go of a note that was deleted.
  Future<void> forget(int id) async {
    if (current != id) return;
    await _player?.stop();
    current = 0;
    playing = false;
    notifyListeners();
  }

  double fraction(VoiceNote n) => current == n.id && durationMs > 0
      ? (positionMs / durationMs).clamp(0.0, 1.0).toDouble()
      : 0.0;

  @override
  void dispose() {
    for (final s in _subs) {
      s.cancel();
    }
    _player?.dispose();
    _player = null;
    super.dispose();
  }
}

/// "0:42", "1:02:05".
String clockOf(num secs) {
  final s = secs.floor();
  final h = s ~/ 3600, m = s ~/ 60 % 60, r = s % 60;
  final mm = h > 0 ? m.toString().padLeft(2, '0') : '$m';
  return '${h > 0 ? '$h:' : ''}$mm:${r.toString().padLeft(2, '0')}';
}
