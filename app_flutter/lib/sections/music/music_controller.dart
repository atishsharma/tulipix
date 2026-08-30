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

import '../../design/pick.dart';
import '../../design/tokens.dart';
import '../../playback/audio_deck.dart';
import '../../playback/video_layer.dart';
import '../../src/rust/api/music.dart';
import '../../shell/shell_controller.dart';
import '../../shell/window.dart';
import 'music_accent.dart';
import 'music_viz.dart' show visStyleNames;

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
/// category row. Slint paints each with a `@linear-gradient(120deg, tint,
/// tint2)`; the same pair also colours the chip at rest, as a wash and an
/// outline, which is what makes a row of five read as five things.
const List<MusicView> musicViews = [
  MusicView('mymusic', 'My Music', Icons.library_music_outlined,
      Color(0xFFEC4899), Color(0xFFF43F5E)),
  MusicView('podcasts', 'Podcasts', Icons.mic_none_outlined, Color(0xFF8B5CF6),
      Color(0xFF6366F1)),
  MusicView('audiobooks', 'Audiobooks', Icons.menu_book_outlined,
      Color(0xFF3B82F6), Color(0xFF06B6D4)),
  MusicView('radio', 'Radio', Icons.radio_outlined, Color(0xFF14B8A6),
      Color(0xFF22C55E)),
  MusicView('youtube', 'YouTube', Icons.play_arrow_rounded, Color(0xFFEF4444),
      Color(0xFFF59E0B)),
];

/// My Music's own sub-tabs — `lib-tab` on the Slint page, all ten of them.
class LibTab {
  const LibTab(this.id, this.label, this.icon, this.tint, this.tint2);
  final String id;
  final String label;
  final IconData icon;
  final Color tint;
  final Color tint2;
}

/// Ten more gradients, walking the same wheel the row above does. These chips
/// are collapsible in Slint: icon only until hovered or active, which is how
/// ten of them fit on one row without a scrollbar.
const List<LibTab> libTabs = [
  LibTab('home', 'Home', Icons.home_outlined, Color(0xFFEC4899),
      Color(0xFFF43F5E)),
  LibTab('songs', 'Songs', Icons.music_note_outlined, Color(0xFF8B5CF6),
      Color(0xFF6366F1)),
  LibTab('albums', 'Albums', Icons.grid_view_outlined, Color(0xFF3B82F6),
      Color(0xFF06B6D4)),
  LibTab('artists', 'Artists', Icons.person_outline, Color(0xFF06B6D4),
      Color(0xFF14B8A6)),
  LibTab('genres', 'Genres', Icons.library_music_outlined, Color(0xFF14B8A6),
      Color(0xFF22C55E)),
  LibTab('playlists', 'Playlists', Icons.queue_music_outlined,
      Color(0xFF22C55E), Color(0xFF84CC16)),
  LibTab('folders', 'Folders', Icons.folder_outlined, Color(0xFFF59E0B),
      Color(0xFFF97316)),
  LibTab('favorites', 'Loved', Icons.favorite, Color(0xFFF43F5E),
      Color(0xFFEC4899)),
  LibTab('history', 'History', Icons.rotate_left, Color(0xFFA855F7),
      Color(0xFF8B5CF6)),
  LibTab('downloader', 'Downloader', Icons.download_outlined, Color(0xFFEC4899),
      Color(0xFF8B5CF6)),
];

class MusicController extends ChangeNotifier {
  /// One controller for the whole app, not one per page. The floating mini
  /// player and the zen player are hosted above every section — the same
  /// arrangement the Slint build uses — so they cannot hang off the Music
  /// page's own State.
  static final MusicController instance = MusicController._();

  MusicController._() {
    // The deck is the media_kit player the bridge drives. Started here because
    // this controller is the only thing that receives the events it answers to,
    // and it outlives every page.
    //
    // Guarded, because this is a singleton every Home layout touches on the way
    // to drawing its player: without the guard a bridge that is not up yet
    // throws out of a *constructor*, and what the user sees is not "the player
    // is unavailable" but a blank landing page. `send` already reports its own
    // failures the same way, and it is also what lets the layout tests run
    // without the native library.
    try {
      AudioDeck.instance.start();
      _events = musicEvents().listen(_onEvent, onError: (Object e) {
        error = e;
        notifyListeners();
      });
    } catch (e) {
      error = e;
    }
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
  /// yet"; the host centres it on first open, as main.slint does.
  Offset? miniPos;
  double miniScale = 1.0;

  /// How far down the right wall the minimised bubble sits. Null is
  /// main.slint's `music-bubble-y < 0px`: uninitialised, so the host centres
  /// it vertically until it is dragged.
  double? bubbleY;

  /// "" art | queue | lyrics — the mini's square flips to show these.
  String miniFace = '';

  /// Zen: inline three-line lyrics, and the slide-in queue panel.
  ///
  /// On by default, which is what "auto-show when available" means here: the
  /// zen page only draws the lines when the track actually has words, so a
  /// default of off meant pressing L on every song that did. Toggling it still
  /// sticks for the session.
  bool zenLyrics = true;
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
      case MusicEvent_Stale():
        // Something behind the page filled itself in — an artist biography that
        // had to be fetched. Nothing about the player changed, so this is a
        // plain re-read rather than the accent-and-artwork reload a track
        // change gets.
        refresh();
      case MusicEvent_ScanFinished():
        progress = null;
        refresh();
      case MusicEvent_Failed(:final message):
        error = message;
        progress = null;
        notifyListeners();

      // The deck. These are not state — nothing here redraws — so they go
      // straight to the player and never touch `notifyListeners`.
      case MusicEvent_AudioPlay(
          :final token,
          :final src,
          :final startAt,
          :final props
        ):
        AudioDeck.instance.play(
          token: token,
          src: src,
          startAt: startAt,
          props: props,
        );
      case MusicEvent_AudioStop():
        AudioDeck.instance.stop();
      case MusicEvent_AudioProp(:final name, :final value):
        AudioDeck.instance.setProperty(name, value);
      case MusicEvent_AudioSeek(:final secs):
        AudioDeck.instance.seek(secs);
      // The picture. Same layer the Videos section uses -- it wraps the whole
      // app, so a music video keeps playing while you look at Photos, and a
      // film and a music video are the one player rather than two.
      case MusicEvent_VideoPlay(
          :final token,
          :final src,
          :final startAt,
          :final props
        ):
        videoRequest.value = VideoRequest(
          token: token,
          src: src,
          startAt: startAt,
          props: props,
          // Reports go to the music half of the bridge, which resumes the
          // audio at whatever frame the picture stopped on.
          onEnded: (t, pos, dur) async {
            await musicVideoEnded(token: t, pos: pos, dur: dur);
            notifyListeners();
          },
        );
        notifyListeners();
      case MusicEvent_VideoStop():
        videoRequest.value = null;
        notifyListeners();
      case MusicEvent_Remote(:final action, :final value):
        _remote(action, value);
    }
  }

  /// A command from outside the app — a media key, the desktop's media applet,
  /// a Bluetooth remote, the tray menu.
  ///
  /// Rust does not act on any of these: the deck is here, so a media key has to
  /// arrive as an event and leave as a `MusicCmd`, exactly as if the matching
  /// button had been clicked. That also means the whole transport is honoured
  /// for free — shuffle, repeat, the queue, the sleep timer.
  void _remote(String action, double value) {
    switch (action) {
      case 'toggle':
        send(const MusicCmd.playPause());
      case 'play':
        if (!tickPlaying) send(const MusicCmd.playPause());
      case 'pause':
        if (tickPlaying) send(const MusicCmd.playPause());
      case 'stop':
        send(const MusicCmd.stop());
      case 'next':
        send(const MusicCmd.next());
      case 'prev':
        send(const MusicCmd.prev());
      case 'shuffle':
        send(const MusicCmd.toggleShuffle());
      case 'repeat':
        // The tray submenu names the target state rather than asking for a
        // step, so cycle until it matches. Three states, at most two steps.
        final want = ['off', 'all', 'one'][value.round().clamp(0, 2)];
        for (var i = 0; i < 3 && (state?.repeat ?? 'off') != want; i++) {
          send(const MusicCmd.cycleRepeat());
        }
      case 'seek':
        send(MusicCmd.seek(secs: value));
      case 'seekby':
        send(MusicCmd.seek(secs: (tickPos + value).clamp(0, tickDur)));
      case 'volume':
        send(MusicCmd.setVolume(volume: value.clamp(0, 130)));
      case 'raise':
        presentWindow();
      case 'mini':
        presentWindow();
        if (!miniOpen) toggleMini();
      case 'quit':
        closeWindow();
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

  /// The zen theme button was pressed: the page follows the app from here on.
  void takeZenTheme() {
    zenThemed = true;
    notifyListeners();
  }

  Future<void> refresh() => send(const MusicCmd.refresh());

  /// The header's `+ Add`, and the empty state's. The chooser is opened here
  /// rather than in the bridge, so a cancelled one dispatches nothing at all.
  Future<void> addFolder() async {
    final path = await pickDirectory();
    if (path == null) return;
    await send(MusicCmd.addFolder(path: path));
  }

  // --- the players ----------------------------------------------------------

  /// Has the zen theme button been pressed since this zen session opened?
  ///
  /// Zen opens dark whatever the app is, but the button at its top right has to
  /// do something visible — so once it is pressed the page follows the app's
  /// theme for the rest of the session. Reset on every open, because "always
  /// starts dark" is the rule the light palette is unreadable without.
  bool zenThemed = false;

  void openZen() {
    zenOpen = true;
    miniOpen = false;
    zenThemed = false;
    // Spectrum on the way in. Zen gives the bars the middle of a fullscreen
    // window, and the shape that earns that much room is the one with the most
    // in it — Bars is what the 290px strip in the player used to draw.
    visStyle = visStyleNames.length - 1;
    visOn = true;
    // Real fullscreen, not a page that happens to fill the window. Slint's
    // `music_enter_zen` calls `set_fullscreen(true)` on the toplevel; the
    // title bar going away is most of what makes zen feel like stopping.
    setWindowFullscreen(true);
    notifyListeners();
  }

  void closeZen() {
    zenOpen = false;
    zenPanel = '';
    setWindowFullscreen(false);
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

  void moveBubble(double y) {
    bubbleY = y;
    notifyListeners();
  }

  /// 1.0-3.0, the clamp on `music-mini-scale` in ui/main.slint. Not the
  /// 0.7-3.0 of `tulipix_music::mini_player` — that is the separate
  /// always-on-top widget window, a different thing entirely.
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

  /// Open the album or artist page of what is playing.
  ///
  /// The click can come from anywhere — the bar, the zen page, the mini
  /// floating over Photos — so getting to the page is three steps, not one:
  /// leave zen if it is up, put the shell on Music and Music on My Music, and
  /// only then ask the bridge to open the detail. Without the first two the
  /// page opens correctly and behind whatever you were looking at.
  Future<void> openNowDetail({required bool album}) async {
    if (zenOpen) closeZen();
    ShellController.instance.go(Section.music);
    if (view != 'mymusic') await send(const MusicCmd.setView(name: 'mymusic'));
    await send(
        album ? const MusicCmd.openNowAlbum() : const MusicCmd.openNowArtist());
  }

  /// Home's Random Radio launcher: a Bollywood or Punjabi station, picked for
  /// you. `home_random_radio` in tulipix-sec-music does this in Rust; here it
  /// is three commands the Radio tab already has, because a fourth bridge
  /// entry point for "surprise me" would be a fourth thing to keep in step.
  ///
  /// The pick is off the clock rather than `dart:math` — good enough for
  /// surprise me, and the same source the Slint build uses.
  Future<void> randomRadio() async {
    const sources = ['bollywood', 'punjabi'];
    final nanos = DateTime.now().microsecondsSinceEpoch;
    if (view != 'radio') await send(const MusicCmd.setView(name: 'radio'));
    for (var k = 0; k < sources.length; k++) {
      await send(MusicCmd.radioSearch(query: sources[(nanos + k) % 2]));
      // `radioPlay` indexes the search result; page 0 is the window on it, so
      // an index inside the first page is an index into the list.
      final n = state?.radioStations.length ?? 0;
      if (n > 0) {
        await send(MusicCmd.radioPlay(index: (nanos ~/ 7) % n));
        return;
      }
    }
  }

  /// Switch the docked panel without the toggle. The chips at its head pick
  /// between Queue and Lyrics; picking the one already showing should not
  /// close the panel you are looking at.
  void showPanel(String which) {
    if (panel == which) return;
    panel = which;
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

  /// Set (or clear) the picture for one album, artist, genre or playlist.
  ///
  /// The eviction is the whole point of routing this through the controller:
  /// `artFor` remembers every answer it has ever had, including the misses, so
  /// a new cover written straight through `send` would not appear until the app
  /// was restarted.
  Future<void> setCardArt(String kind, String key, String path) async {
    _art.remove('$kind:$key');
    await send(MusicCmd.setCardArt(kind: kind, key: key, path: path));
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
