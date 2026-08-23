// Music section state. The five tabs share one controller for the same reason
// they share one bridge snapshot: they share a player. A queue started in My
// Music, a podcast episode, a radio stream and a YouTube video all land on the
// same transport, and splitting the controller would mean five objects that
// have to agree about which of them is currently making noise.
//
// Everything ephemeral stays here rather than crossing the FFI boundary:
// selection, which dialog is open, what has been typed into a box that has not
// been submitted, and the artwork cache. The Slint page had to declare all of
// that as properties because Slint has no store.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart' show Int64List;

import '../../src/rust/api/music.dart';
import 'music_accent.dart';

/// One of the five top-level categories. `view` on the Slint page.
class MusicView {
  const MusicView(this.id, this.label, this.icon, this.tint, this.tint2);

  final String id;
  final String label;
  final IconData icon;
  final Color tint;
  final Color tint2;
}

/// The order, icons and per-tab gradients ui/page_music.slint gives its top
/// category row. Slint paints each with a two-stop gradient; `tint` is the
/// first stop and `tint2` the second.
const List<MusicView> musicViews = [
  MusicView('mymusic', 'My Music', Icons.library_music_outlined,
      Color(0xFFEC4899), Color(0xFF8B5CF6)),
  MusicView('podcasts', 'Podcasts', Icons.podcasts_outlined, Color(0xFFF97316),
      Color(0xFFEC4899)),
  MusicView('audiobooks', 'Audiobooks', Icons.auto_stories_outlined,
      Color(0xFF10B981), Color(0xFF14B8A6)),
  MusicView('radio', 'Radio', Icons.radio_outlined, Color(0xFF06B6D4),
      Color(0xFF3B82F6)),
  MusicView('youtube', 'YouTube', Icons.smart_display_outlined,
      Color(0xFFF43F5E), Color(0xFFF97316)),
];

/// My Music's own sub-tabs — `lib-tab` on the Slint page, all nine of them.
class LibTab {
  const LibTab(this.id, this.label, this.icon);
  final String id;
  final String label;
  final IconData icon;
}

const List<LibTab> libTabs = [
  LibTab('home', 'Home', Icons.dashboard_outlined),
  LibTab('songs', 'Songs', Icons.music_note_outlined),
  LibTab('albums', 'Albums', Icons.album_outlined),
  LibTab('artists', 'Artists', Icons.person_outline),
  LibTab('genres', 'Genres', Icons.category_outlined),
  LibTab('playlists', 'Playlists', Icons.queue_music_outlined),
  LibTab('folders', 'Folders', Icons.folder_outlined),
  LibTab('favorites', 'Favourites', Icons.favorite_outline),
  LibTab('history', 'History', Icons.history),
  LibTab('downloader', 'Downloader', Icons.download_outlined),
];

class MusicController extends ChangeNotifier {
  /// One controller for the whole app, not one per page. The floating mini
  /// player and the zen player are hosted above every section — the same
  /// arrangement the Slint build uses — so they cannot hang off the Music
  /// page's own State.
  static final MusicController instance = MusicController._();

  MusicController._() {
    _events = musicEvents().listen(_onEvent, onError: (Object e) {
      error = e;
      notifyListeners();
    });
  }

  MusicState? state;
  Object? error;
  bool busy = false;

  /// "" | queue | lyrics | eq — which side panel the player bar has open.
  String panel = '';

  /// Live scan / refresh progress, from the event stream. Null when nothing is
  /// running. Kept in Dart because it is a notification, not state: a snapshot
  /// taken a second later should not resurrect a finished scan.
  ({String label, int done, int total})? progress;

  StreamSubscription<MusicEvent>? _events;

  /// kind:key → resolved artwork path. Survives a refresh, which is the point:
  /// re-resolving a cover that already painted is a wasted FFI round trip per
  /// tile per scroll.
  final Map<String, String> _art = <String, String>{};
  final Set<String> _artInFlight = <String>{};

  /// Position and duration between snapshots. The bar redraws from these on
  /// every tick, so a seek bar moves once a second without a full re-query of
  /// the library behind it.
  double tickPos = 0;
  double tickDur = 0;
  bool tickPlaying = false;

  // --- the two players that live above the section --------------------------

  /// Zen: the full-window player. Mini: the draggable card that keeps playing
  /// in view while you are somewhere else entirely.
  bool zenOpen = false;
  bool miniOpen = false;

  /// The mini docked to the window edge as a bubble — still playing, out of
  /// the way, one click from coming back.
  bool miniBubble = false;

  /// Where the mini sits and how big it is. Null position means "not placed
  /// yet"; the host puts it bottom-right on first open.
  Offset? miniPos;
  double miniScale = 1.0;

  /// "" art | queue | lyrics — the mini's square flips to show these.
  String miniFace = '';

  /// Zen: inline three-line lyrics, and the slide-in queue panel.
  bool zenLyrics = false;
  String zenPanel = '';

  /// Visualizer style (0..5, matching the style list) and whether it is drawn
  /// at all. Session-scoped: the Slint build persists these, the port does not
  /// yet.
  int visStyle = 0;
  bool visOn = true;

  /// Dominant colour of the current artwork. Everything tinted by the track —
  /// the bar's wash, the seek fill, the mini's ring — reads this.
  Color accent = const Color(0xFFEC4899);
  String _accentFor = '';

  void _onEvent(MusicEvent event) {
    switch (event) {
      case MusicEvent_Tick(:final pos, :final dur, :final playing):
        tickPos = pos;
        tickDur = dur;
        tickPlaying = playing;
        notifyListeners();
      case MusicEvent_TrackChanged():
        // The track changed under us — the loved state, the artwork and the
        // queue position are all now wrong in the held snapshot.
        refresh();
        _refreshAccent();
      case MusicEvent_Ended():
        // Rust does not advance the queue itself: one place decides what plays
        // next, and it is the one holding the repeat and shuffle switches.
        send(const MusicCmd.next());
      case MusicEvent_ScanProgress(:final root, :final done, :final total):
        progress = (label: root, done: done, total: total);
        notifyListeners();
      case MusicEvent_ScanFinished():
        progress = null;
        refresh();
      case MusicEvent_Failed(:final message):
        error = message;
        progress = null;
        notifyListeners();
    }
  }

  String get view => state?.view ?? 'mymusic';
  String get libTab => state?.libTab ?? 'home';
  NowPlaying? get now => state?.now;

  Future<void> send(MusicCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      state = await musicDispatch(cmd: cmd);
      final n = state!.now;
      tickPos = n.pos;
      tickDur = n.dur;
      tickPlaying = n.playing;
      unawaited(_refreshAccent());
    } catch (e) {
      error = e;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  Future<void> refresh() => send(const MusicCmd.refresh());

  // --- the players ----------------------------------------------------------

  void openZen() {
    zenOpen = true;
    miniOpen = false;
    notifyListeners();
  }

  void closeZen() {
    zenOpen = false;
    zenPanel = '';
    notifyListeners();
  }

  void toggleMini() {
    miniOpen = !miniOpen;
    if (miniOpen) {
      miniBubble = false;
    }
    notifyListeners();
  }

  void setMiniBubble(bool bubble) {
    miniBubble = bubble;
    notifyListeners();
  }

  void moveMini(Offset to) {
    miniPos = to;
    notifyListeners();
  }

  void scaleMini(double to) {
    miniScale = to.clamp(1.0, 3.0);
    notifyListeners();
  }

  void setMiniFace(String face) {
    miniFace = miniFace == face ? '' : face;
    notifyListeners();
  }

  void setZenPanel(String which) {
    zenPanel = zenPanel == which ? '' : which;
    notifyListeners();
  }

  void toggleZenLyrics() {
    zenLyrics = !zenLyrics;
    notifyListeners();
  }

  void setVisStyle(int style) {
    visStyle = style;
    visOn = true;
    notifyListeners();
  }

  void setVisOn(bool on) {
    visOn = on;
    notifyListeners();
  }

  /// Index of the lyric line that should be lit, or -1. Derived from the live
  /// tick rather than stored, so it stays right between snapshots.
  int get activeLyric {
    final lines = state?.lyrics ?? const <LyricLine>[];
    if (lines.isEmpty) return -1;
    final at = tickPos + (state?.lyricsOffsetMs ?? 0) / 1000.0;
    var hit = -1;
    for (var i = 0; i < lines.length; i++) {
      if (lines[i].atMs / 1000.0 <= at) {
        hit = i;
      } else {
        break;
      }
    }
    return hit;
  }

  /// Pull the dominant colour out of the current cover. Cheap because it
  /// decodes to 16x16 first — the answer is one colour, not an image.
  Future<void> _refreshAccent() async {
    final path = state?.now.art ?? '';
    if (path == _accentFor) return;
    _accentFor = path;
    final next = await dominantColour(path);
    if (next != null && _accentFor == path) {
      accent = next;
      notifyListeners();
    } else if (next == null) {
      accent = const Color(0xFFEC4899);
      notifyListeners();
    }
  }

  /// Dismiss the error banner. A method rather than the widget poking the
  /// field, because `notifyListeners` is protected — and rightly: the object
  /// that owns the state is the one that should say when it changed.
  void clearError() {
    error = null;
    notifyListeners();
  }

  void setPanel(String which) {
    panel = panel == which ? '' : which;
    notifyListeners();
  }

  /// Artwork for one thing, resolved once and remembered. Returns null on the
  /// first call and notifies when the answer arrives, which is what lets a
  /// cold library paint progressively instead of blocking on ffmpeg.
  String? artFor(String kind, String key) {
    if (key.isEmpty) return null;
    final id = '$kind:$key';
    final hit = _art[id];
    if (hit != null) return hit.isEmpty ? null : hit;
    if (_artInFlight.add(id)) {
      musicEnsureArt(kind: kind, key: key).then((path) {
        // Remember a miss as well as a hit: a track with no embedded cover
        // would otherwise be re-probed on every scroll past it.
        _art[id] = path ?? '';
        _artInFlight.remove(id);
        if (path != null && path.isNotEmpty) notifyListeners();
      }).catchError((Object _) {
        _art[id] = '';
        _artInFlight.remove(id);
      });
    }
    return null;
  }

  // --- shorthands the widgets use constantly --------------------------------

  Future<void> playFrom(List<Track> tracks, int index, String source) {
    if (tracks.isEmpty) return Future.value();
    return send(MusicCmd.playList(
      itemIds: Int64List.fromList(tracks.map((t) => t.itemId).toList()),
      index: index,
      source: source,
    ));
  }

  Future<void> playQueueAt(int index) =>
      send(MusicCmd.queuePlayAt(index: index));

  Future<void> queueAll(List<Track> tracks) => send(MusicCmd.queueAdd(
      itemIds: Int64List.fromList(tracks.map((t) => t.itemId).toList())));

  @override
  void dispose() {
    // The stream outlives the widget tree otherwise, and its handler calls
    // notifyListeners on a disposed notifier.
    _events?.cancel();
    super.dispose();
  }
}

/// Seconds as m:ss, or h:mm:ss once there is an hour. Used by every duration
/// the section shows.
String fmtClock(double secs) {
  if (secs.isNaN || secs.isInfinite || secs <= 0) return '0:00';
  final s = secs.round();
  final m = (s ~/ 60) % 60;
  final h = s ~/ 3600;
  final ss = (s % 60).toString().padLeft(2, '0');
  if (h > 0) return '$h:${m.toString().padLeft(2, '0')}:$ss';
  return '$m:$ss';
}

/// Unix seconds as "12 Mar 2026". Episode and release dates only — nothing
/// here needs a clock.
String fmtDate(int unixSeconds) {
  if (unixSeconds <= 0) return '';
  final d = DateTime.fromMillisecondsSinceEpoch(unixSeconds * 1000);
  const months = [
    'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', //
    'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
  ];
  return '${d.day} ${months[d.month - 1]} ${d.year}';
}
