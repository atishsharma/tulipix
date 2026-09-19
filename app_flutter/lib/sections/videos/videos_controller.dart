// The Videos section's state.
//
// One snapshot for seven tabs. The Rust session holds everything — the local
// grid, the Discover rails, the Stream detail pane, the Live TV channel list
// and Stream Plus's shelves — and every command answers with the whole of it.
// That is the same contract Slint properties gave the shipping build, and it is
// what keeps a keystroke in the library search box from re-querying Live TV.
//
// Two things do not ride the snapshot. Grid posters are fetched per tile as it
// scrolls in (`videosEnsureThumb`), because rendering three hundred frames to
// answer one refresh is not a refresh. Download progress arrives on the event
// stream instead, many times a second, and patches one row rather than costing
// a snapshot each time.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../playback/video_layer.dart';
import '../../src/rust/api/videos.dart';

/// The seven top-level tabs, in the order the header draws them. The first
/// three are the library, drawn as one segmented control; the other four keep
/// their pages and sit after a divider (docs/videos-deck.html).
const kVideoKinds = <({String id, String label, Color hue})>[
  (id: 'movies', label: 'Movies', hue: cSave),
  (id: 'tv', label: 'Shows', hue: cInfo),
  (id: 'local', label: 'Local', hue: cDl),
  (id: 'discover', label: 'Discover', hue: cBack),
  (id: 'livetv', label: 'Live TV', hue: cCopy),
  (id: 'stream', label: 'Stream', hue: cPlay),
  (id: 'splus', label: 'Stream Plus', hue: Tokens.secVideos),
];

/// The library sub-tabs, shared by TV / Movies / Local.
const kVideoCategories = <({String id, String label, Color hue})>[
  (id: 'library', label: 'Library', hue: Tokens.secVideos),
  (id: 'continue', label: 'Continue', hue: cPlay),
  (id: 'starred', label: 'Starred', hue: cBack),
  (id: 'archive', label: 'Archive', hue: cDl),
  (id: 'trash', label: 'Trash', hue: cErr),
];

// The section's function colours, lifted verbatim from `VP` in
// ui/page_videos.slint. They are deliberately not derived from the theme: a
// Play button is green in light, dark and OLED, and white ink reads on all six.
const Color cPlay = Color(0xFF16B364); // green  — Play
const Color cCopy = Color(0xFF0EA5E9); // sky    — Copy link
const Color cDl = Color(0xFF14B8A6); //   teal   — Download
const Color cSave = Color(0xFFF43F5E); // rose   — Save / bookmark
const Color cBack = Color(0xFFD97706); // amber  — Back
const Color cInfo = Color(0xFF8B5CF6); // violet — Open / info
const Color cErr = Color(0xFFC0392B); //  brick  — destructive
const Color cBadgeSeries = Color(0xCC8B0000);
const Color cBadgeMovie = Color(0xCC0047AB);

/// Titles for the library sub-tabs, matching `tab-title` in the Slint page.
String categoryTitle(String c) => switch (c) {
      'continue' => 'Continue Watching',
      'starred' => 'Starred',
      'archive' => 'Archive',
      'trash' => 'Trash',
      _ => 'Recently Added',
    };

/// The empty-state line for each sub-tab, matching `empty-msg`.
String categoryEmpty(String c) => switch (c) {
      'continue' =>
        'Nothing in progress. Press play on a title and it lands here.',
      'starred' => 'Star a title (right-click) to pin it here.',
      'archive' => 'Archived titles are hidden from your main library.',
      'trash' => 'Trashed titles are kept here before they age out.',
      _ =>
        'Add a movie or TV folder and Tulipix builds your cinematic library.',
    };

class VideosController extends ChangeNotifier {
  VideosState? state;
  Object? error;
  bool busy = false;

  /// A scan is running, and how far it got. Null when nothing is scanning.
  String? scanRoot;
  String? scanResult;

  /// Live download progress, keyed by job id. The Downloads page prefers these
  /// over the snapshot's rows, which only move when a command is dispatched.
  final Map<int, ({double progress, String detail})> liveProgress = {};

  StreamSubscription<VideosEvent>? _events;

  /// Coalesces the `Changed` events a background task fires — the download
  /// runner and the autoplay chain can both emit several in a second, and each
  /// one asking for a fresh snapshot would serialise behind the last.
  Timer? _debounce;

  VideosController() {
    _events = videosEvents().listen(_onEvent, onError: (Object e) {
      error = e;
      notifyListeners();
    });
  }

  void _onEvent(VideosEvent event) {
    switch (event) {
      case VideosEvent_ScanStarted(:final root):
        scanRoot = root;
        scanResult = null;
        notifyListeners();
      case VideosEvent_ScanFinished(
          :final scanned,
          :final inserted,
          :final updated,
          :final missing
        ):
        scanRoot = null;
        scanResult = '$scanned scanned · $inserted new · $updated updated'
            '${missing > 0 ? ' · $missing missing' : ''}';
        notifyListeners();
      case VideosEvent_Changed():
        _nudge();
      case VideosEvent_DownloadTick(
          :final id,
          :final progress,
          :final detail,
          :final label,
          :final active
        ):
        if (active) {
          liveProgress[id] = (progress: progress, detail: detail);
        } else {
          liveProgress.remove(id);
        }
        _dlLabel = label;
        _dlFrac = progress;
        _dlActive = active;
        notifyListeners();
      case VideosEvent_Failed(:final message):
        error = message;
        notifyListeners();
      case VideosEvent_VideoPlay(
          :final token,
          :final src,
          :final startAt,
          :final props
        ):
        // The layer that draws it wraps the whole app, not this section: a
        // channel started here keeps playing while you look at Photos.
        videoRequest.value = VideoRequest(
          token: token,
          src: src,
          startAt: startAt,
          props: props,
        );
      case VideosEvent_VideoStop():
        clearVideo();
    }
  }

  // The header's own progress pill. Kept apart from the snapshot for the same
  // reason `liveProgress` is: it moves while nothing is being dispatched.
  String _dlLabel = '';
  double _dlFrac = 0;
  bool _dlActive = false;

  String get dlLabel => _dlActive ? _dlLabel : (state?.stream.dlLabel ?? '');
  double get dlFrac => _dlActive ? _dlFrac : (state?.stream.dlFrac ?? 0);
  bool get dlActive => _dlActive || (state?.stream.dlActive ?? false);

  void _nudge() {
    _debounce?.cancel();
    _debounce = Timer(const Duration(milliseconds: 250), refresh);
  }

  Future<void> send(VideosCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      state = await videosDispatch(cmd: cmd);
    } catch (e) {
      error = e;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  /// Fire and forget: for the handful of commands whose result the user is not
  /// waiting on (a hover preview, a suggestion list) and which must not flatten
  /// every button on the page while they run.
  Future<void> sendQuiet(VideosCmd cmd) async {
    try {
      state = await videosDispatch(cmd: cmd);
      notifyListeners();
    } catch (e) {
      error = e;
      notifyListeners();
    }
  }

  Future<void> refresh() => sendQuiet(const VideosCmd.refresh());

  void clearError() {
    error = null;
    notifyListeners();
  }

  void clearScanResult() {
    scanResult = null;
    notifyListeners();
  }

  /// The movie whose page is open, or null for the tab itself. Dart-side
  /// only: the page is a view of a tile the snapshot already carries, and
  /// Escape or the tab row close it without a round trip.
  VideoTile? openMovie;

  void openMoviePage(VideoTile? tile) {
    openMovie = tile;
    notifyListeners();
  }

  /// Backdrops by `item:<id>` or `show:<id>`. A miss is remembered too, so a
  /// title TMDB never matched is asked about once, not on every hover.
  final Map<String, String?> _backdrops = {};

  String? backdropCached(int itemId, int showId) =>
      _backdrops[showId > 0 ? 'show:$showId' : 'item:$itemId'];

  Future<String?> backdropFor(int itemId, int showId) async {
    final key = showId > 0 ? 'show:$showId' : 'item:$itemId';
    if (_backdrops.containsKey(key)) return _backdrops[key];
    String? path;
    try {
      path = await videosEnsureBackdrop(itemId: itemId, showId: showId);
    } catch (_) {
      path = null;
    }
    _backdrops[key] = path;
    return path;
  }

  /// The movie page's file details, fetched when it opens.
  Future<MovieDetail> movieDetail(int itemId) => videosMovieDetail(itemId: itemId);

  /// The grid poster for one library item, rendered on first sight.
  Future<String?> thumbFor(int itemId) async {
    try {
      return await videosEnsureThumb(itemId: itemId);
    } catch (_) {
      return null;
    }
  }

  @override
  void dispose() {
    _debounce?.cancel();
    _events?.cancel();
    super.dispose();
  }
}
