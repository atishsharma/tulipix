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
import 'dart:io' show Platform;

import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart' show Int64List;

import '../../design/motion_clock.dart';
import '../../design/pick.dart';
import '../../design/tokens.dart';
import '../../playback/audio_deck.dart';
import '../../playback/video_layer.dart';
import '../../src/rust/api/music.dart';
import '../../shell/shell_controller.dart';
import '../../shell/window.dart';
import 'mini_player.dart' show kMiniSize;
import 'mini_widget.dart' show MiniStyle, kPillCluster, kPillLyrics;
import 'music_accent.dart';
import 'spectrum.dart';
import 'music_viz.dart' show visStyleNames;

// It lives in the design kit now, whose seek bar shows a clock too; every
// caller here still finds it through this file.
export '../../design/clock.dart' show fmtClock;

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
      // Volume and mute move the players' controls the moment the deck
      // applies them, and only those: every volume control already rebuilds
      // on [ticks], as the position readers do.
      audioLevel.addListener(_tickOnly);
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

  /// The equalizer, open out of the top of the player bar. Its own flag, not
  /// a value of [panel]: that holds one thing at a time, so opening the
  /// equalizer used to fold the Queue or Lyrics panel it sits beside.
  bool eqOpen = false;

  void toggleEq() {
    eqOpen = !eqOpen;
    notifyListeners();
  }

  // ── detail navigation ─────────────────────────────────────────────────────
  //
  // The bridge holds ONE open detail — a `detail_open` bool and a `detail_kind`,
  // a slot rather than a stack — so album → artist → album had nowhere to go
  // back to. The history is kept here instead: every command that opens a
  // detail page is pushed on the way through `send`, which is the one funnel
  // all twelve call sites already go through. Going back re-issues the previous
  // entry's own command, so the bridge never has to learn what a stack is.
  final List<MusicCmd> _trail = [];

  /// Human labels for the pushed entries, so a page can be named before it has
  /// been fetched. Same length as [_trail]. The detail hero draws them as its
  /// breadcrumb.
  final List<String> _trailLabels = [];

  /// True while `goBack` is re-issuing an entry, so the push in [send] does not
  /// re-add the page we are returning to.
  bool _restoring = false;

  /// Whether the YouTube picture just stopped was filling the window, so the
  /// next one in the queue opens at that size rather than back in the corner.
  bool _ytWatchFull = false;
  int _ytWatchToken = 0;

  bool get _watchingYt =>
      _ytWatchToken != 0 && videoRequest.value?.token == _ytWatchToken;

  /// The picture's clock, as the deck's tick would have reported it: the seek
  /// bar keeps moving in video mode, so switching back to audio is a glance.
  void _followVideo(double pos, double dur, bool playing) {
    audioPositionS = pos;
    final changed = playing != tickPlaying || dur != tickDur;
    final second = pos.floor() != tickPos.floor();
    tickPos = pos;
    tickDur = dur;
    tickPlaying = playing;
    if (changed) {
      notifyListeners();
    } else if (second) {
      _tickOnly();
    }
  }

  bool get canGoBack => _trail.isNotEmpty;

  /// The pages behind this one and this one, oldest first.
  List<String> get trail => List.unmodifiable(_trailLabels);

  /// Straight back to the entry at [index] in [trail], however many pages
  /// that skips.
  Future<void> goBackTo(int index) async {
    if (index < 0 || index >= _trail.length - 1) return;
    _trail.removeRange(index + 1, _trail.length);
    _trailLabels.removeRange(index + 1, _trailLabels.length);
    _restoring = true;
    try {
      await send(_trail.last);
    } finally {
      _restoring = false;
    }
  }

  /// One step back: drop the current page and re-open the one beneath it, or
  /// close the detail entirely when that was the last of them.
  Future<void> goBack() async {
    if (_trail.isEmpty) return;
    _trail.removeLast();
    _trailLabels.removeLast();
    _restoring = true;
    try {
      await send(_trail.isEmpty ? const MusicCmd.closeDetail() : _trail.last);
    } finally {
      _restoring = false;
    }
  }

  /// Back to the library, discarding the whole trail.
  Future<void> closeDetail() async {
    _trail.clear();
    _trailLabels.clear();
    await send(const MusicCmd.closeDetail());
  }

  /// The label to show for a command that opens a detail page, or null when the
  /// command does not open one. Ids are all this knows before the fetch; the
  /// real title replaces it in [send] once the state comes back.
  static String? _detailLabel(MusicCmd cmd) => switch (cmd) {
        MusicCmd_OpenAlbum() => 'Album',
        MusicCmd_OpenArtist() => 'Artist',
        MusicCmd_OpenNowAlbum() => 'Album',
        MusicCmd_OpenNowArtist() => 'Artist',
        MusicCmd_OpenGenre(:final name) => name,
        MusicCmd_OpenPlaylist() => 'Playlist',
        MusicCmd_OpenFolder(:final path) => path
                .split(Platform.pathSeparator)
                .where((s) => s.isNotEmpty)
                .lastOrNull ??
            'Folder',
        _ => null,
      };

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

  /// Fires on a tick that moved only the position. The controller itself fires
  /// only when a tick changes play/pause or the duration: the whole section
  /// and the shell listen to it, and only the clock, the scrubber and the lyric
  /// line read the position, so once a second everything rebuilt for them.
  final ValueNotifier<int> ticks = ValueNotifier<int>(0);

  /// The controller and [ticks] together, for a widget that shows position.
  late final Listenable live = Listenable.merge([this, ticks]);

  void _tickOnly() => ticks.value++;

  /// Volume and mute as the deck holds them: what every player's volume
  /// control and mute glyph shows. See [audioLevel] for why not the snapshot,
  /// which is only the fallback until Rust has handed the deck a value.
  double get volume => audioLevel.value?.volume ?? now?.volume ?? 100;
  bool get muted => audioLevel.value?.muted ?? now?.muted ?? false;

  Timer? _volumeSend;

  /// Moves the volume now, on the deck, and tells Rust -- which clamps it,
  /// stores it and hands the deck the same value back -- once the pointer has
  /// been still for a moment. Each drag step used to be a bridge round trip
  /// and a whole snapshot, and the bar moved only when that came back.
  void setVolume(double volume) {
    final v = volume.clamp(0.0, 130.0).toDouble();
    AudioDeck.instance.setProperty('volume', v.toString());
    _volumeSend?.cancel();
    _volumeSend = Timer(const Duration(milliseconds: 150),
        () => send(MusicCmd.setVolume(volume: v)));
  }

  @visibleForTesting
  void debugEvent(MusicEvent event) => _onEvent(event);

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

  // --- the desktop widget, which is NOT the mini player --------------------
  //
  // Two separate things that a shared `miniOpen` used to blur together. The
  // mini player is the 300x470 card floating inside the app (`MusicMini` in
  // ui/main.slint) and the player bar is the only way to it. The widget is the
  // three size classes that TAKE THE WINDOW (`MiniStyle` in
  // crates/tulipix-music/src/mini_player.rs) and the caption row is the only
  // way to it. Neither control reaches the other's object.

  /// The window is the widget. See [miniIsWidget] for the gates on it.
  bool widgetOpen = false;

  /// Which size class. Never a card — the card is not in `MiniStyle` there
  /// either, because Slint draws it in the main window rather than the widget.
  MiniStyle widgetStyle = MiniStyle.bar;

  /// Whether the stored choice has been applied yet. The shell snapshot is
  /// re-fetched every 30 seconds, and a seed that ran on each of those would
  /// undo the picker every half minute.
  bool _styleSeeded = false;

  /// Apply the style Settings last wrote, once, at startup.
  ///
  /// `ui.mini-widget.style` was written by the picker and read by nothing here,
  /// so the widget opened as a bar however many times it had been set to
  /// something else. `miniwin::wire` does this from the same key on the Slint
  /// side. An unknown or empty name leaves the default alone.
  void seedWidgetStyle(String name) {
    if (_styleSeeded) return;
    _styleSeeded = true;
    for (final s in MiniStyle.values) {
      if (s.name == name) {
        widgetStyle = s;
        notifyListeners();
        return;
      }
    }
  }

  /// The widget's own scale, separate from the card's: `MIN_SCALE`/`MAX_SCALE`
  /// are 0.7-3.0 and the card's clamp is 1.0-3.0, so one number could not carry
  /// both without one of them being wrong at the bottom.
  double widgetScale = 1.0;

  /// Pill only: the chevron has slid the window buttons out of the right edge,
  /// and the mic has pulled a lyrics row out under the track. Both grow the
  /// window rather than reflowing it, which is why they live here and not in
  /// the widget — the box has to be sized before the widget is built.
  bool pillOpen = false;
  bool pillLyrics = false;

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
  /// at all.
  ///
  /// Stored in settings under `music.viz.*` and read back off the snapshot, so
  /// a style chosen once is still there next launch — the Slint build has
  /// always persisted these. The local fields are the optimistic half: a menu
  /// pick redraws on the next frame rather than waiting for the round trip,
  /// and null means "whatever the snapshot says".
  int? _visStyle;
  bool? _visOn;

  int get visStyle => _visStyle ?? state?.vizStyle.toInt() ?? _kSpectrum;
  bool get visOn => _visOn ?? state?.vizOn ?? true;

  /// The default: the shape with the most in it, which is what zen used to
  /// force on the way in. It is a default now rather than an override, so
  /// picking something else actually sticks. `music.viz.style` in the bridge
  /// defaults to the same index.
  static final int _kSpectrum = visStyleNames.length - 1;

  /// Dominant colour of the current artwork. Everything tinted by the track —
  /// the bar's wash, the seek fill, the mini's ring — reads this.
  Color accent = const Color(0xFFEC4899);

  /// The wash's second colour. Together with [accent] this is the light the
  /// room is lit by rather than a tint on one control: the player bar, the
  /// queue panel and the seekbar all take their gradient from the pair, and
  /// both animate to the next record's colours on a track change.
  Color accentAlt = const Color(0xFF8B5CF6);
  String _accentFor = '';

  void _onEvent(MusicEvent event) {
    switch (event) {
      case MusicEvent_Tick(:final pos, :final dur, :final playing)
          // A picture on screen keeps the clock (see [_followVideo]).
          when !_watchingYt:
        final changed = playing != tickPlaying || dur != tickDur;
        tickPos = pos;
        tickDur = dur;
        tickPlaying = playing;
        // On the beat, while anything is moving: the clock and the scrubber
        // then land in a frame the motion is drawing anyway, rather than in
        // one of their own between two beats.
        MotionClock.instance.onNextBeat(changed ? notifyListeners : _tickOnly);
      case MusicEvent_Tick():
        break;
      case MusicEvent_TrackChanged():
        // The track changed under us — the loved state, the artwork and the
        // queue position are all now wrong in the held snapshot.
        refresh();
        unawaited(_followTrack());
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
        // The deck is stopped while a picture plays; the bar's seek is the
        // picture's then.
        if (_watchingYt) videoSeek?.call(secs);
      // The picture. Same layer the Videos section uses -- it wraps the whole
      // app, so a music video keeps playing while you look at Photos, and a
      // film and a music video are the one player rather than two.
      case MusicEvent_VideoPlay(
          :final token,
          :final src,
          :final startAt,
          :final props
        ):
        // A YouTube video opens as the corner card, the way the tab's Video
        // buttons promise; the card's expand button fills the window. One
        // already filling it (the queue moving on) stays that size.
        videoDocked.value = !_ytWatchFull;
        _ytWatchFull = false;
        _ytWatchToken = token;
        videoRequest.value = VideoRequest(
          token: token,
          src: src,
          startAt: startAt,
          props: props,
          // The bar follows the picture: its clock, its seek bar and its
          // volume are this video's while it is on screen.
          onProgress: _followVideo,
          level: audioLevel,
          onVolume: (v) {
            if (muted && v > 0) send(const MusicCmd.toggleMute());
            setVolume(v);
          },
          // Reports go to the music half of the bridge, which resumes the
          // audio at whatever frame the picture stopped on.
          onEnded: (t, pos, dur) async {
            await musicVideoEnded(token: t, pos: pos, dur: dur);
            notifyListeners();
          },
        );
        notifyListeners();
      case MusicEvent_VideoStop():
        // Sent just before every VideoPlay, so this is where the size of the
        // picture being replaced is still known.
        _ytWatchFull = videoRequest.value?.token == _ytWatchToken &&
            !videoDocked.value;
        clearVideo();
        notifyListeners();
      case MusicEvent_VideoSkips(:final token, :final segments):
        videoSkips.value = (token, segments.toList());
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
        setVolume(value);
      case 'raise':
        presentWindow();
      case 'mini':
        // The tray's item is the WIDGET, not the card. `tray.mini` is
        // `miniwin::open_mini(&w)` in crates/tulipix-app/src/main.rs -- the app
        // becomes the widget -- and this had it opening the in-app mini player
        // instead, which is the one thing the tray cannot reach in the Slint
        // build. (The menu's label still says "Mini Player"; it lives in
        // crates/tulipix-platform, which this branch does not touch.)
        presentWindow();
        if (!widgetOpen) toggleWidget();
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
    // Pushed before the await, so a breadcrumb drawn while the fetch is in
    // flight already shows where it is going.
    final label = _restoring ? null : _detailLabel(cmd);
    if (label != null) {
      _trail.add(cmd);
      _trailLabels.add(label);
    }
    notifyListeners();
    try {
      state = await musicDispatch(cmd: cmd);
      // The page knows its own name once it has loaded; an id-derived
      // placeholder never has to be shown twice.
      if (state!.detailOpen && _trailLabels.isNotEmpty) {
        final title = state!.detailTitle;
        if (title.isNotEmpty) _trailLabels[_trailLabels.length - 1] = title;
      }
      // Anything that closes the detail from the bridge's side (a tab switch,
      // a refresh that lands with it shut) invalidates the trail: keeping it
      // would offer a back button to a page that is no longer open.
      if (!state!.detailOpen && !_restoring) {
        _trail.clear();
        _trailLabels.clear();
      }
      final n = state!.now;
      // While a picture plays the clock is its own (see [_followVideo]); the
      // snapshot holds where the stopped deck was.
      if (!_watchingYt) {
        tickPos = n.pos;
        tickDur = n.dur;
        tickPlaying = n.playing;
      }
      unawaited(_followTrack());
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
    // Both floating players stand down: zen is the whole window, and the widget
    // has to give the window back before anything can be fullscreen in it.
    miniOpen = false;
    widgetOpen = false;
    zenThemed = false;
    // Spectrum used to be forced on the way in, on the grounds that zen gives
    // the bars the middle of a fullscreen window and that is the shape with
    // the most in it. It is the stored default instead now — forcing it meant
    // a style picked inside zen was thrown away the next time zen opened,
    // which is most of what "the port does not persist these" amounted to.
    // Real fullscreen, not a page that happens to fill the window. Slint's
    // `music_enter_zen` calls `set_fullscreen(true)` on the toplevel; the
    // title bar going away is most of what makes zen feel like stopping.
    setWindowFullscreen(true);
    notifyListeners();
  }

  void closeZen() {
    zenOpen = false;
    zenPanel = '';
    // Back to whatever the app was, not flat to windowed: logo-fullscreen is
    // the shell's own fullscreen and zen is a second claim on the same window
    // state. Dropping it unconditionally left the app un-fullscreened with its
    // caption row still hidden, which is a window with no way out of anything.
    setWindowFullscreen(ShellController.instance.appFullscreen);
    notifyListeners();
  }

  /// Whether the window IS the widget right now.
  ///
  /// In the Slint build opening the widget hides the main window — "the app
  /// becomes the widget", `open_mini`. There is one window here, so it becomes
  /// the widget instead; see [setWindowWidgetMode]. The zen player is the app
  /// again, and so is having nothing loaded.
  bool get miniIsWidget {
    final now = state?.now;
    // Nothing loaded, nothing to be. Otherwise stopping the deck while the
    // widget is up would leave the window shrunk around an empty card.
    if (now == null || !(now.loaded || now.title.isNotEmpty)) return false;
    return widgetOpen && !zenOpen;
  }

  bool _inWidgetWindow = false;
  Size? _widgetSize;

  /// Kept in step from [notifyListeners] rather than from each of the nine
  /// mutators that can reach it — a style set directly, a scale drag, a bubble,
  /// zen opening underneath it. Every one of them notifies, and none of them
  /// can be the one that forgot.
  void _syncWidgetWindow() {
    final want = miniIsWidget;
    if (want != _inWidgetWindow) {
      _inWidgetWindow = want;
      _widgetSize = want ? widgetWindow : null;
      setWindowWidgetMode(want, _widgetSize ?? Size.zero);
      return;
    }
    if (!want) return;
    // The scale grip and the pill's two toggles change the box while the window
    // is already the widget. Only when it actually moved: this runs on every
    // change to the controller.
    final box = widgetWindow;
    if (box != _widgetSize) {
      _widgetSize = box;
      setWindowSize(box);
    }
  }

  @override
  void notifyListeners() {
    _syncWidgetWindow();
    super.notifyListeners();
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

  /// The CARD's scale: 1.0-3.0, the clamp on `music-mini-scale` in
  /// ui/main.slint. The card is not designed to go below its reference size.
  void scaleMini(double to) {
    miniScale = to.clamp(1.0, 3.0);
    notifyListeners();
  }

  /// The WIDGET's scale: 0.7-3.0, `MIN_SCALE`/`MAX_SCALE` in
  /// crates/tulipix-music/src/mini_player.rs. The three classes are designed to
  /// go smaller than their reference size, which is why this clamp is not the
  /// card's.
  void scaleWidget(double to) {
    widgetScale = to.clamp(0.7, 3.0);
    notifyListeners();
  }

  /// Open the widget, or give the window back. The caption row's button, and
  /// the only door in — nothing on a player reaches this.
  void toggleWidget() {
    widgetOpen = !widgetOpen;
    notifyListeners();
  }

  void closeWidget() {
    widgetOpen = false;
    notifyListeners();
  }

  /// bar -> square -> pill -> bar, the `cycle-style` callback on the Slint
  /// widget. The card is not a stop on it: the card is a different object.
  ///
  /// The pill's two extras are dropped on the way out, the way `plain_pill` in
  /// crates/tulipix-app/src/miniwin.rs drops them whenever the widget leaves
  /// the pill: they are window WIDTH and HEIGHT rather than anything the layout
  /// can absorb, so a bar carrying them would just be a bar 148px too wide.
  void cycleWidgetStyle() => setWidgetStyle(widgetStyle.next);

  void setWidgetStyle(MiniStyle style) {
    widgetStyle = style;
    pillOpen = false;
    pillLyrics = false;
    notifyListeners();
  }

  void togglePillOpen() {
    pillOpen = !pillOpen;
    notifyListeners();
  }

  void togglePillLyrics() {
    pillLyrics = !pillLyrics;
    notifyListeners();
  }

  /// Whether the track has words to show. The flip affordance on the bar and
  /// the square, and the mic on the pill, are only offered when it does — a
  /// button that reveals an empty box is worse than no button.
  bool get hasLyrics => (state?.lyrics ?? const <LyricLine>[]).isNotEmpty;

  /// The box the overlay gives the in-app mini player card. `music-mini-w` /
  /// `music-mini-h` in ui/main.slint, times the card's own scale.
  Size get miniWindow =>
      Size(kMiniSize.width * miniScale, kMiniSize.height * miniScale);

  /// The box the WINDOW is resized to: the style's reference size times the
  /// widget's scale, plus whatever the pill's two toggles have added.
  ///
  /// The extras are added at the current scale but are NOT part of the ratio,
  /// which is what `resize_locked_with_extra` does on the Rust side — dragging
  /// the corner while the cluster is out keeps the designed shape instead of
  /// snapping back to the collapsed width.
  Size get widgetWindow {
    final base = widgetStyle.base;
    final pill = widgetStyle == MiniStyle.pill;
    final extraW = pill && pillOpen ? kPillCluster : 0.0;
    final extraH = pill && pillLyrics && hasLyrics ? kPillLyrics : 0.0;
    return Size((base.width + extraW) * widgetScale,
        (base.height + extraH) * widgetScale);
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
    _visStyle = style;
    _visOn = true;
    notifyListeners();
    send(MusicCmd.setAudio(key: 'music.viz.style', value: '$style'));
    send(const MusicCmd.setAudio(key: 'music.viz.on', value: '1'));
  }

  void setVisOn(bool on) {
    _visOn = on;
    notifyListeners();
    send(MusicCmd.setAudio(key: 'music.viz.on', value: on ? '1' : '0'));
  }

  /// Index of the lyric line that should be lit, or -1. Derived from the live
  /// tick rather than stored, so it stays right between snapshots.
  int get activeLyric {
    final lines = state?.lyrics ?? const <LyricLine>[];
    if (lines.isEmpty) return -1;
    // The deck's own position, not `tickPos`. The tick that feeds `tickPos` is
    // throttled to one a second on the Dart side, which is right for a clock
    // and wrong for a lyric: a line can be most of a second late, and since
    // lines do not fall on second boundaries each one is late by a different
    // amount — which reads as the words not following the song at all rather
    // than as a fixed lag. `audioPositionS` is the same unthrottled source
    // `SeekPill(smooth:)` reads for the progress edge.
    final live =
        tickPlaying && audioPositionS > 0 ? audioPositionS : tickPos;
    final at = live + (state?.lyricsOffsetMs ?? 0) / 1000.0;
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

  /// Everything that has to follow the track rather than the frame: the colour
  /// the room takes, and the spectrum the visualiser draws.
  ///
  /// Both are per-track reads that must not sit in the tick path — one decodes
  /// an image, the other reads a file off disk — and both are safe to lose: a
  /// failure leaves the previous accent and no spectrum, which is what an
  /// unanalysed library shows anyway.
  Future<void> _followTrack() async {
    unawaited(loadSpectrum(state?.now.itemId ?? 0));
    await _refreshAccent();
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
      accentAlt = companion(next);
      notifyListeners();
    } else if (next == null) {
      accent = const Color(0xFFEC4899);
      accentAlt = const Color(0xFF8B5CF6);
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

  /// Where the equalizer is on screen, so a click inside it is not taken for a
  /// click away from the Queue panel.
  static final GlobalKey eqPanelKey = GlobalKey();

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
    // The view has to be on radio while this runs and cannot stay there.
    // `fill_radio` is the only thing that puts stations on the snapshot and the
    // bridge only calls it for `view == "radio"` (music.rs:9528), so without
    // the switch the search lands and `radioStations` is still empty. But this
    // is a launcher that starts a station playing, not a trip to the Radio tab
    // — so whatever Music was showing is put back once a station is on the
    // deck. Playback does not care which view is up; it keeps going.
    final was = view;
    if (was != 'radio') await send(const MusicCmd.setView(name: 'radio'));
    try {
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
    } finally {
      if (was != 'radio') await send(MusicCmd.setView(name: was));
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

  // --- waveform ---------------------------------------------------------------

  final Map<int, List<int>> _wave = {};
  final Set<int> _waveInFlight = {};

  /// The loudness envelope for one track, resolved once and remembered.
  ///
  /// Same shape as [artFor]: null on the first call, a notify when it lands,
  /// and a remembered miss so a track ffmpeg cannot read is not re-decoded
  /// every time the seekbar rebuilds.
  List<int>? waveFor(int itemId) {
    if (itemId == 0) return null;
    final hit = _wave[itemId];
    if (hit != null) return hit.isEmpty ? null : hit;
    if (_waveInFlight.add(itemId)) {
      musicWaveform(itemId: itemId).then((bytes) {
        _wave[itemId] = bytes;
        _waveInFlight.remove(itemId);
        if (bytes.isNotEmpty) notifyListeners();
      }).catchError((Object _) {
        _wave[itemId] = const [];
        _waveInFlight.remove(itemId);
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

  // --- keyboard ---------------------------------------------------------------
  //
  // Named rather than inlined at the binding, because a shortcut has to decide
  // whether it applies: pressing space with nothing loaded should do nothing,
  // not start whatever happens to be first in the library.

  bool get _hasTrack => (state?.now.itemId ?? 0) != 0 || tickDur > 0;

  void keyPlayPause() {
    if (!_hasTrack) return;
    send(const MusicCmd.playPause());
  }

  void keyNext() {
    if (!_hasTrack) return;
    send(const MusicCmd.next());
  }

  void keyPrev() {
    if (!_hasTrack) return;
    send(const MusicCmd.prev());
  }

  /// Seek by [delta] seconds from where the tick says we are, clamped inside
  /// the track -- seeking past the end is how a keypress skips a song by
  /// accident.
  void nudge(double delta) {
    if (!_hasTrack || tickDur <= 0) return;
    final to = (tickPos + delta).clamp(0.0, tickDur);
    send(MusicCmd.seek(secs: to));
  }

  void keyLove() {
    final id = state?.now.itemId ?? 0;
    if (id == 0) return;
    send(MusicCmd.love(itemId: id));
  }

  void keyShuffle() => send(const MusicCmd.toggleShuffle());

  void keyRepeat() => send(const MusicCmd.cycleRepeat());

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
