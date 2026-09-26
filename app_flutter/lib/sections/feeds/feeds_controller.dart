// The Feeds section's state, and its voice.
//
// The bridge keeps the session — which tab, which source, which article is
// open — so this holds the last snapshot, the two things that are only
// presentation (the reader's text size, whether the summary is folded), and
// the notice the last command left.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart' show AnyhowException;
import 'package:media_kit/media_kit.dart';

import '../../src/rust/api/books.dart' show booksTtsSay;
import '../../src/rust/api/feeds.dart';
import '../../playback/audio_deck.dart' show kDefaultVolume;

typedef FeedTab = ({String id, String label});

/// The header's tabs, in the order it draws them. Also Settings → Sections'
/// list of what can be switched off.
const List<FeedTab> feedTabs = [
  (id: 'today', label: 'Today'),
  (id: 'unread', label: 'Unread'),
  (id: 'saved', label: 'Read later'),
  (id: 'highlights', label: 'Highlights'),
];

/// How often the feeds are asked while the page is built.
const Duration kFeedsTick = Duration(minutes: 30);

class FeedsController extends ChangeNotifier {
  FeedsState? state;
  Object? error;
  bool busy = false;

  /// The network refresh, which takes seconds and gets its own line.
  bool fetching = false;

  /// The article page being fetched for the reader.
  int fullFor = 0;

  String notice = '';
  Timer? _noticeTimer;

  /// The reader's body size: four stops, 18 to start.
  static const List<double> textSizes = [16, 18, 20, 22];
  int textSize = 1;
  bool summaryOpen = true;

  Future<FeedsState?> send(FeedsCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      final st = await feedsDispatch(cmd: cmd);
      state = st;
      if (st.notice.isNotEmpty) say(st.notice);
      return st;
    } catch (e) {
      error = e;
      return null;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  Future<void> refresh() => send(const FeedsCmd.refresh());

  Future<void> fetch() async {
    if (fetching) return;
    fetching = true;
    notifyListeners();
    await send(const FeedsCmd.fetch());
    fetching = false;
    notifyListeners();
  }

  /// A fetch when the last one is older than [age] — on coming back to the
  /// page, not on every glance at it.
  void fetchIfStale(Duration age) {
    final at = state?.lastRefresh ?? 0;
    final last = DateTime.fromMillisecondsSinceEpoch(at * 1000);
    if (at == 0 || DateTime.now().difference(last) > age) fetch();
  }

  /// Open in the reader, then go for the page when the feed sent only part,
  /// then — when a model server is set — for a better summary.
  Future<void> open(int id) async {
    final st = await send(FeedsCmd.open(id: id));
    final view = st?.open;
    if (view == null) return;
    if (!view.full && view.url.isNotEmpty) {
      fullFor = id;
      notifyListeners();
      await send(FeedsCmd.fullText(id: id));
      if (fullFor == id) fullFor = 0;
      notifyListeners();
    }
    await _modelSummary(id);
  }

  /// The article the model server is summarising; 0 when none.
  int summarising = 0;

  /// Why the last model summary did not come, for a quiet line under it.
  String summaryError = '';

  Future<void> _modelSummary(int id) async {
    final st = state;
    final v = st?.open;
    if (st == null ||
        st.summarizer.isEmpty ||
        v == null ||
        v.id != id ||
        v.summaryModel ||
        v.paragraphs.length < 3) {
      return;
    }
    summarising = id;
    summaryError = '';
    notifyListeners();
    try {
      state = await feedsSummarise(id: id);
    } catch (e) {
      summaryError = _plain(e);
    }
    if (summarising == id) summarising = 0;
    notifyListeners();
  }

  /// Set or clear the mailbox; answers the reason when it did not take.
  Future<String?> setMail(
      String host, int port, String user, String password, String folder) async {
    await send(FeedsCmd.setMail(
        host: host, port: port, user: user, password: password, folder: folder));
    final e = error;
    if (e == null) return null;
    error = null;
    notifyListeners();
    return _plain(e);
  }

  /// Set or clear the model server; answers the reason when it did not take.
  Future<String?> setSummarizer(String url, String model) async {
    await send(FeedsCmd.setSummarizer(url: url, model: model));
    final e = error;
    if (e == null) return null;
    error = null;
    notifyListeners();
    return _plain(e);
  }

  /// Follow, answering whether it worked so the dialog can stay up with the
  /// reason when it did not.
  Future<String?> follow(String url, String folder) async {
    await send(FeedsCmd.follow(url: url, folder: folder));
    final e = error;
    if (e == null) return null;
    error = null;
    notifyListeners();
    return _plain(e);
  }

  /// Another reader's OPML export, followed folder by folder.
  Future<void> importOpml(String path) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      state = await feedsImportOpml(path: path);
      if (state!.notice.isNotEmpty) say(state!.notice);
    } catch (e) {
      error = e;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  void cycleTextSize() {
    textSize = (textSize + 1) % textSizes.length;
    notifyListeners();
  }

  void toggleSummary() {
    summaryOpen = !summaryOpen;
    notifyListeners();
  }

  /// A line under the header that goes away on its own.
  void say(String text) {
    notice = text;
    _noticeTimer?.cancel();
    _noticeTimer = Timer(const Duration(seconds: 5), dismissNotice);
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

/// A bridge error as its message, without the type wrapped around it.
String _plain(Object e) =>
    (e is AnyhowException ? e.message : '$e').split('\n\nStack backtrace').first.trim();

String plainError(Object e) => _plain(e);

/// Listen: an article or the brief, spoken a sentence at a time in the Books
/// reader's voice.
///
/// The same loop as `ReadAloud` in books/read_aloud.dart, without the page
/// turning — synthesise one sentence, play it, wait for the end, ask for the
/// next — and [_gen] for the same reason: an await cannot be cancelled, and a
/// sentence that lands after Stop must not start speaking.
class FeedsVoice extends ChangeNotifier {
  Player? _player;
  List<String> sentences = const [];
  int active = 0;
  bool playing = false;

  /// What is being read, for the bar. Empty when nothing is.
  String label = '';
  String note = '';
  int _gen = 0;

  bool get on => label.isNotEmpty;

  Future<void> read(String what, String text) async {
    await stop();
    final gen = ++_gen;
    label = what;
    notifyListeners();
    try {
      sentences = await feedsSentences(text: text);
    } catch (e) {
      sentences = const [];
    }
    if (gen != _gen) return;
    if (sentences.isEmpty) {
      note = 'There is no text to read yet.';
      notifyListeners();
      return;
    }
    active = 0;
    playing = true;
    notifyListeners();
    _run(gen);
  }

  void playPause() {
    if (sentences.isEmpty) return;
    playing = !playing;
    _gen++;
    if (playing) {
      _run(_gen);
    } else {
      _player?.pause();
    }
    notifyListeners();
  }

  void step(int delta) {
    if (sentences.isEmpty) return;
    active = (active + delta).clamp(0, sentences.length - 1);
    _gen++;
    _player?.stop();
    notifyListeners();
    if (playing) _run(_gen);
  }

  Future<void> stop() async {
    _gen++;
    playing = false;
    sentences = const [];
    active = 0;
    label = '';
    note = '';
    notifyListeners();
    await _player?.stop();
  }

  Future<void> _run(int gen) async {
    while (playing && gen == _gen && active < sentences.length) {
      try {
        final path = await booksTtsSay(
            text: sentences[active], voice: '', speed: 1.0);
        if (gen != _gen || !playing) return;
        final p = _player ??= (Player(
          configuration: const PlayerConfiguration(title: 'Tulipix — reading'),
        )..setVolume(kDefaultVolume));
        await p.open(Media(path));
        // `completed` also fires for the previous media: wait for the edge.
        await p.stream.completed.firstWhere((done) => done);
      } catch (e) {
        if (gen != _gen) return;
        playing = false;
        note = 'No voice to read with — ${_plain(e)}';
        notifyListeners();
        return;
      }
      if (gen != _gen || !playing) return;
      if (active + 1 >= sentences.length) {
        playing = false;
        notifyListeners();
        return;
      }
      active++;
      notifyListeners();
    }
  }

  @override
  void dispose() {
    _gen++;
    _player?.dispose();
    _player = null;
    super.dispose();
  }
}

// ------------------------------------------------------------ formatting ---

/// A source's colour: a fixed palette picked by feed id, so a feed keeps its
/// colour from one launch to the next and across every tab.
const List<Color> _palette = [
  Color(0xFFE11D48),
  Color(0xFFF97316),
  Color(0xFFF59E0B),
  Color(0xFF16A34A),
  Color(0xFF14B8A6),
  Color(0xFF0EA5E9),
  Color(0xFF1D4ED8),
  Color(0xFF6366F1),
  Color(0xFF8B5CF6),
  Color(0xFFDB2777),
  Color(0xFFB91C1C),
  Color(0xFF0D9488),
];

Color sourceColor(int seed) => _palette[seed.abs() % _palette.length];

/// A source's colour from its name, so it is the same on every tab — the
/// brief knows its sources only by name — and from one launch to the next,
/// which `String.hashCode` does not promise.
Color nameColor(String name) =>
    sourceColor(name.codeUnits.fold<int>(7, (h, c) => (h * 31 + c) & 0x7fffffff));

/// "12 min ago", "3 h", "Yesterday", "14 Sep".
String ago(int unixSecs) {
  if (unixSecs <= 0) return '';
  final at = DateTime.fromMillisecondsSinceEpoch(unixSecs * 1000);
  final d = DateTime.now().difference(at);
  if (d.inMinutes < 1) return 'just now';
  if (d.inHours < 1) return '${d.inMinutes} min ago';
  if (d.inHours < 24) return '${d.inHours} h';
  if (d.inDays < 2) return 'Yesterday';
  if (d.inDays < 7) return _weekdays[at.weekday - 1];
  return '${at.day} ${_months[at.month - 1].substring(0, 3)}';
}

const _weekdays = [
  'Monday',
  'Tuesday',
  'Wednesday',
  'Thursday',
  'Friday',
  'Saturday',
  'Sunday',
];
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

/// "Saturday 19 September".
String today() {
  final n = DateTime.now();
  return '${_weekdays[n.weekday - 1]} ${n.day} ${_months[n.month - 1]}';
}

/// "6:12".
String clock(int secs) =>
    '${secs ~/ 60}:${(secs % 60).toString().padLeft(2, '0')}';

String plural(int n, String one, [String? many]) =>
    n == 1 ? '1 $one' : '$n ${many ?? '${one}s'}';
