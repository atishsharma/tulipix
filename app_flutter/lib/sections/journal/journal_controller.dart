// The Journal section's state, and its voice notes' player.
//
// The bridge keeps the session — which tab, which day, which month — so this
// holds the last snapshot, the notice the last command left, and which voice
// note is being transcribed or played.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart' show AnyhowException;
import 'package:media_kit/media_kit.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/journal.dart';

typedef JournalTab = ({String id, String label});

/// The header's tabs, in the order it draws them. Also Settings → Sections'
/// list of what can be switched off.
const List<JournalTab> journalTabs = [
  (id: 'today', label: 'Today'),
  (id: 'calendar', label: 'Calendar'),
  (id: 'otd', label: 'On this day'),
  (id: 'insights', label: 'Insights'),
];

const Color kJournal = Tokens.secJournal;
const Color kJournal2 = Tokens.secJournal2;

class JournalController extends ChangeNotifier {
  JournalState? state;
  Object? error;
  bool busy = false;

  /// The voice note whisper is working on; 0 when none is.
  int transcribing = 0;

  String notice = '';
  Timer? _noticeTimer;

  /// [quiet] for the autosave, which runs as you type and should not flash
  /// the progress line every time.
  Future<JournalState?> send(JournalCmd cmd, {bool quiet = false}) async {
    busy = !quiet;
    error = null;
    if (!quiet) notifyListeners();
    try {
      final st = await journalDispatch(cmd: cmd);
      _take(st);
      return st;
    } catch (e) {
      error = e;
      return null;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  void _take(JournalState st) {
    state = st;
    if (st.notice.isNotEmpty) say(st.notice);
  }

  Future<void> refresh() => send(const JournalCmd.refresh());

  /// Stop, keep the note, then transcribe it — the second part takes a while
  /// and has its own spinner on the note.
  Future<void> stopRecording() async {
    final st = await send(const JournalCmd.recordStop());
    if (st == null) return;
    VoiceView? newest;
    for (final e in st.entries) {
      for (final v in e.voice) {
        if (v.state == 'new' && (newest == null || v.id > newest.id)) {
          newest = v;
        }
      }
    }
    if (newest != null) await transcribe(newest.id);
  }

  Future<void> transcribe(int id) async {
    if (transcribing != 0) return;
    transcribing = id;
    notifyListeners();
    try {
      _take(await journalTranscribe(id: id));
    } catch (e) {
      error = e;
    } finally {
      transcribing = 0;
      notifyListeners();
    }
  }

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
    super.dispose();
  }
}

/// Plays one voice note at a time, and says how far through it is.
class VoicePlayer extends ChangeNotifier {
  Player? _player;
  final List<StreamSubscription<Object?>> _subs = [];

  /// The note playing or paused; 0 for none.
  int current = 0;
  bool playing = false;
  double fraction = 0;

  Future<void> toggle(VoiceView v) async {
    final p = _player ??= _make();
    if (current == v.id) {
      await p.playOrPause();
      return;
    }
    current = v.id;
    fraction = 0;
    notifyListeners();
    await p.open(Media(v.path));
  }

  Player _make() {
    final p = Player(
      configuration: const PlayerConfiguration(title: 'Tulipix — voice note'),
    );
    _subs.add(p.stream.playing.listen((on) {
      playing = on;
      notifyListeners();
    }));
    _subs.add(p.stream.position.listen((pos) {
      final d = p.state.duration.inMilliseconds;
      fraction =
          d > 0 ? (pos.inMilliseconds / d).clamp(0.0, 1.0).toDouble() : 0.0;
      notifyListeners();
    }));
    _subs.add(p.stream.completed.listen((done) {
      if (!done) return;
      fraction = 1;
      playing = false;
      notifyListeners();
    }));
    return p;
  }

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

// ------------------------------------------------------------ formatting ---

/// A bridge error as its message, without the type wrapped around it.
String plainError(Object e) => (e is AnyhowException ? e.message : '$e')
    .split('\n\nStack backtrace')
    .first
    .trim();

const List<String> moodLabels = ['Rough', 'Low', 'Okay', 'Good', 'Great'];

const List<IconData> moodIcons = [
  Icons.sentiment_very_dissatisfied_outlined,
  Icons.sentiment_dissatisfied_outlined,
  Icons.sentiment_neutral_outlined,
  Icons.sentiment_satisfied_outlined,
  Icons.sentiment_very_satisfied_outlined,
];

/// The deck's five: slate, sky, violet, green, amber.
const List<Color> _moodColors = [
  Color(0xFF64748B),
  Color(0xFF0EA5E9),
  Color(0xFF8B5CF6),
  Color(0xFF22C55E),
  Color(0xFFF59E0B),
];

/// 1 rough … 5 great; null for no mood.
Color? moodColor(int mood) =>
    mood >= 1 && mood <= 5 ? _moodColors[mood - 1] : null;

typedef SourceLook = ({String label, IconData icon, Color tint});

/// Each gathered row wears its section's colour and icon.
SourceLook sourceLook(String source) => switch (source) {
      'photos' => (
          label: 'Photos',
          icon: Icons.photo_outlined,
          tint: Tokens.secPhotos
        ),
      'music' => (
          label: 'Music',
          icon: Icons.music_note_outlined,
          tint: Tokens.secMusic
        ),
      'finances' => (
          label: 'Finances',
          icon: Icons.account_balance_wallet_outlined,
          tint: Tokens.secFinances
        ),
      'videos' => (
          label: 'Videos',
          icon: Icons.movie_outlined,
          tint: Tokens.secVideos
        ),
      'books' => (
          label: 'Books',
          icon: Icons.menu_book_outlined,
          tint: Tokens.secBooks
        ),
      _ => (label: source, icon: Icons.circle_outlined, tint: Tokens.secSettings),
    };

const _months = [
  'January',
  'February',
  'March',
  'April',
  'May',
  'June',
  'July',
  'August',
  'September',
  'October',
  'November',
  'December',
];

/// "2026-09-19" → "19 September".
String dayMonth(String iso) {
  final d = DateTime.tryParse(iso);
  return d == null ? iso : '${d.day} ${_months[d.month - 1]}';
}

/// "0:48".
String clock(double secs) {
  final s = secs.round();
  return '${s ~/ 60}:${(s % 60).toString().padLeft(2, '0')}';
}

String thousands(int n) {
  final d = '$n';
  final b = StringBuffer();
  for (var i = 0; i < d.length; i++) {
    if (i > 0 && (d.length - i) % 3 == 0) b.write(',');
    b.write(d[i]);
  }
  return b.toString();
}

/// frb's `Int64List` holds `BigInt`s, not ints (Videos found this first), so
/// every id list that crosses comes through here.
List<int> ints(Iterable<BigInt> v) => [for (final x in v) x.toInt()];

String plural(int n, String one, [String? many]) =>
    n == 1 ? '1 $one' : '$n ${many ?? '${one}s'}';
