// The Flutter shell.
//
// A port of ui/sidebar.slint over an IndexedStack of the ten sections. The
// sidebar owns which section is up (`ShellController`, a singleton, because
// Home's tiles and the Status lamp both navigate); this file owns the theme,
// which is read from and written back to the same settings file the Slint
// build uses.
//
// Still scaffolding above the sidebar: no custom caption row, no command
// palette. The lock screen wraps everything (shell/lock/lock_screen.dart).

import 'dart:ui' show AppExitResponse;

import 'package:flutter/material.dart';
import 'package:media_kit/media_kit.dart';

import 'design/app_theme.dart';
import 'design/bloom.dart';
import 'design/design_language.dart';
import 'design/skin.dart';
import 'design/tokens.dart';
import 'sections/arcade/arcade_page.dart';
import 'sections/archive/archive_page.dart';
import 'sections/books/books_page.dart';
import 'sections/cloud/cloud_page.dart';
import 'sections/feeds/feeds_page.dart';
import 'sections/finances/finances_page.dart';
import 'sections/home/home_page.dart';
import 'sections/journal/journal_page.dart';
import 'sections/kitchen/kitchen_page.dart';
import 'sections/music/music_controller.dart';
import 'sections/music/music_overlay.dart';
import 'sections/music/music_page.dart';
import 'sections/papers/papers_page.dart';
import 'sections/photos/photos_page.dart';
import 'sections/settings/settings_page.dart';
import 'sections/studio/studio_page.dart';
import 'sections/tools/tools_page.dart';
import 'sections/places/places_page.dart';
import 'sections/voice/voice_page.dart';
import 'sections/transfer/transfer_page.dart';
import 'sections/videos/videos_page.dart';
import 'playback/video_layer.dart';
import 'shell/chat_overlay.dart';
import 'shell/lock/lock_controller.dart';
import 'shell/lock/lock_screen.dart';
import 'shell/onboarding/onboarding_flow.dart';
import 'shell/shell_controller.dart';
import 'shell/sidebar.dart';
import 'shell/title_row.dart';
import 'shell/tray_panel.dart';
import 'shell/vitals.dart';
import 'shell/window.dart';
import 'src/rust/api/cloud.dart' show cloudShutdown;
import 'src/rust/api/music.dart';
import 'src/rust/api/transfer.dart';
import 'src/rust/api/videos.dart';
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  // Startup time and slow frames, for Settings › Advanced › Performance.
  Vitals.start();
  // libmpv, for both players. Must run before any `Player` is constructed, and
  // the music controller builds one the moment the Music section is touched.
  MediaKit.ensureInitialized();
  await RustLib.init();
  // Whether the runner dropped the system frame, so the shell knows to draw a
  // caption row and the resize edges. Asked before the first frame: finding out
  // afterwards means the row appearing a beat after the window does.
  await WindowChrome.instance.init();
  // Not `runApp`: that draws into the one implicit view, and the tray panel
  // is a second window on this same engine. See lib/shell/tray_panel.dart.
  runWidget(const AppViews(app: TulipixApp()));
}

/// One section's page. Seven of them take `visible` because they hold something
/// that should stop when it is not on screen — a poll, a queue ticker.
Widget _pageFor(Section s, Section at) => switch (s) {
      Section.home => HomePage(visible: at == Section.home),
      Section.photos => const PhotosPage(),
      Section.videos => const VideosPage(),
      Section.music => const MusicPage(),
      Section.books => const BooksPage(),
      Section.cloud => const CloudPage(),
      Section.tools => ToolsPage(visible: at == Section.tools),
      Section.transfer => TransferPage(visible: at == Section.transfer),
      Section.finances => const FinancesPage(),
      Section.feeds => FeedsPage(visible: at == Section.feeds),
      Section.journal => JournalPage(visible: at == Section.journal),
      Section.kitchen => KitchenPage(visible: at == Section.kitchen),
      Section.papers => PapersPage(visible: at == Section.papers),
      Section.voice => VoicePage(visible: at == Section.voice),
      Section.places => PlacesPage(visible: at == Section.places),
      Section.studio => StudioPage(visible: at == Section.studio),
      Section.archive => ArchivePage(visible: at == Section.archive),
      Section.arcade => ArcadePage(visible: at == Section.arcade),
      Section.settings => SettingsPage(visible: at == Section.settings),
    };

class TulipixApp extends StatefulWidget {
  const TulipixApp({super.key});

  @override
  State<TulipixApp> createState() => _TulipixAppState();
}

class _TulipixAppState extends State<TulipixApp> {
  // Theme.dark / Theme.oled from ui/tokens.slint, resolved from the same
  // `Settings.theme` string the Slint build stores: "system" | "light" |
  // "dark" | "extra-dark".
  bool _dark = true;
  bool _oled = false;

  /// `ui.design-language`, from the same snapshot as the theme. Only Music
  /// reads it so far; see lib/design/skin.dart.
  DesignLanguage _language = DesignLanguage.standard;

  /// "Follow system accent" (null: off, or none reported) and "Honour OS
  /// font scale".
  Color? _accent;
  bool _fontScale = true;

  /// Bloom's colours, and whether section accents lean toward its seed.
  Bloom _bloom = bloom;
  bool _harmonise = false;

  final ShellController _shell = ShellController.instance;

  /// Closing the window has to close the listening socket, and stop the audio.
  /// The process exit would do both too, but not before the OS has had a moment
  /// to hold the port — and not before the speakers have had another second of
  /// whatever was playing after the window went away.
  AppLifecycleListener? _lifecycle;

  @override
  void initState() {
    super.initState();
    _shell.addListener(_onShell);
    // The cover as Bloom's seed: a new record re-tones the app.
    MusicController.instance.addListener(_onMusic);
    // The first snapshot also carries the stored theme, which is why this runs
    // before anything is drawn rather than when the sidebar first appears.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _shell.refresh();
      // Whether to lock when idle, and after how long.
      LockController.instance.reload();
    });
    _lifecycle = AppLifecycleListener(
      onExitRequested: () async {
        transferShutdown();
        musicShutdown();
        // Same reason as the other two: what is on should stop when the app
        // goes, and this is what drops the hooks that would otherwise write a
        // position back against a session nobody is watching.
        videosShutdown();
        // rclone mounts outlive the app otherwise, and a mount point left
        // behind cannot be mounted again until someone unmounts it by hand.
        // Capped: a wedged unmount must not keep the window from closing.
        try {
          await cloudShutdown().timeout(const Duration(seconds: 3));
        } catch (_) {}
        return AppExitResponse.exit;
      },
    );
  }

  @override
  void dispose() {
    _shell.removeListener(_onShell);
    MusicController.instance.removeListener(_onMusic);
    _lifecycle?.dispose();
    super.dispose();
  }

  void _onShell() {
    // The stored desktop-widget style rides in on the same snapshot as the
    // theme, and is applied once — see [MusicController.seedWidgetStyle].
    MusicController.instance
        .seedWidgetStyle(_shell.state?.miniWidgetStyle ?? '');
    final theme = _shell.state?.theme ?? 'system';
    final dark = theme != 'light';
    final oled = theme == 'extra-dark';
    // Only when the theme actually moved. This `setState` rebuilds MaterialApp,
    // and every rebuild hands its AnimatedTheme a freshly built ThemeData:
    // `Tokens` is a ThemeExtension with no value equality, so a new instance is
    // never `==` the old one and the app-wide 200 ms theme cross-fade restarts
    // from zero -- taking the AnimatedTheme that ButtonStyleButton wraps around
    // every Material button with it. Sixty-two of them were re-animating on the
    // shell's 30-second tick, for a theme that had not changed. Everything else
    // this file draws from the shell redraws through the AnimatedBuilder below.
    // The design language rides the same gate: it is part of the ThemeData too.
    final language = _shell.designLanguage;
    // Settings › Advanced: the desktop's accent, and whether its text size
    // counts. Same gate: both are baked into what MaterialApp draws.
    final accent = _shell.systemAccent;
    final fontScale = _shell.followOsFontScale;
    // Material 3 Expressive's seed. Same gate again: a record whose cover has
    // the same colour as the last one changes nothing.
    final next = _bloomNow();
    final harmonise = language == DesignLanguage.expressive &&
        (_shell.state?.bloomHarmonise ?? false);
    if (dark == _dark &&
        oled == _oled &&
        language == _language &&
        accent == _accent &&
        fontScale == _fontScale &&
        next == _bloom &&
        harmonise == _harmonise) {
      return;
    }
    Tokens.systemAccent = accent;
    Tokens.harmoniseTo = harmonise ? next.seed : null;
    bloom = next;
    setState(() {
      _dark = dark;
      _oled = oled;
      _language = language;
      _accent = accent;
      _fontScale = fontScale;
      _bloom = next;
      _harmonise = harmonise;
    });
  }

  /// Not before the first snapshot: [_onShell] seeds the mini's style once,
  /// and an empty snapshot would seed it with nothing.
  void _onMusic() {
    if (_shell.state != null) _onShell();
  }

  /// The seed from where Settings says it comes from; Music pink when that
  /// has none — nothing playing, or a desktop that reports no accent.
  Bloom _bloomNow() {
    final st = _shell.state;
    final seed = switch (st?.bloomSource) {
      'desktop' => hexColor(st!.bloomDesktop),
      'pick' => hexColor(st!.bloomSeed),
      _ => MusicController.instance.accent,
    };
    return (
      seed: seed ?? Tokens.secMusic,
      style: bloomStyle(st?.bloomStyle ?? ''),
      contrast: st?.bloomContrast ?? 0,
    );
  }

  IconData get _themeIcon => !_dark
      ? Icons.light_mode
      : _oled
          ? Icons.star_outline
          : Icons.dark_mode;

  @override
  Widget build(BuildContext context) {
    // Base tokens from the theme, the language's skin over them, and the
    // language's own tokens from the skin — so every `context.tokens` in
    // every section answers in the language. tokens.dart stays a literal
    // transcription of tokens.slint; Standard's retint is the base itself.
    final base = _dark ? Tokens.dark(oled: _oled) : Tokens.light();
    final skin = skinFor(_language, base);
    final tokens = skin.retint(base);
    return MaterialApp(
      title: 'Tulipix',
      debugShowCheckedModeBanner: false,
      theme: appTheme(tokens, skin),
      // "Honour OS font scale" off: text at 100 % whatever the desktop asks.
      builder: (context, child) => _fontScale
          ? child!
          : MediaQuery(
              data: MediaQuery.of(context)
                  .copyWith(textScaler: TextScaler.noScaling),
              child: child!,
            ),
      // The floating mini, the zen player and the video all wrap the whole
      // app: what is playing does not stop playing when you leave the section
      // that started it, and these are how it stays visible. Video covers the
      // window, including the mini; chat sits over the video, because Ctrl+J
      // has to answer from anywhere and a playing film is still anywhere.
      //
      // The lock screen is over all of it, chat included: a locked window
      // answers nothing but the lock.
      //
      // Setup sits inside the lock and outside everything else: a locked
      // window answers nothing, first run included, and the ten cards cover
      // the player and the chat the way any modal does.
      home: LockOverlay(
        child: OnboardingOverlay(
          child: ChatOverlay(
            child: VideoLayer(
              child: MusicOverlay(
                child: Scaffold(
                  backgroundColor: tokens.bg,
                  // Outside everything: the grab strip is the window's own edge, and
                  // a handle inside the page would be a handle the sidebar covers.
                  body: WindowResizeEdges(
                    child: AnimatedBuilder(
                      animation: _shell,
                      builder: (context, _) {
                        final at = _shell.section;
                        return Stack(children: [
                          // The language's backdrop under the whole shell — the
                          // aura, the plate. Always a child, empty under Standard:
                          // a child that comes and goes moves its siblings, and
                          // that costs every page its State (see the resize edges).
                          //
                          // The window's only one. While Music is up it is lit by
                          // the record, which is what Music's own backdrop was for:
                          // that one sat over this one, and under Glass the two
                          // were four window-sized gradients in every frame.
                          Positioned.fill(
                            child: ListenableBuilder(
                              listenable: MusicController.instance,
                              builder: (context, _) {
                                final music = at == Section.music
                                    ? MusicController.instance
                                    : null;
                                return skin.pageBackdrop(
                                      accent:
                                          music?.accent ?? Tokens.accentOf(at),
                                      alt: music?.accentAlt ?? Tokens.brand2,
                                    ) ??
                                    const SizedBox.shrink();
                              },
                            ),
                          ),
                          Column(children: [
                            // Above the rail and the page both, the way the caption row
                            // is in ui/main.slint — it is the window's row, not the
                            // shell's, so nothing sits beside it.
                            //
                            // Gone in logo-fullscreen, which is what makes that mode
                            // borderless: `if root.csd && !root.app-fullscreen` on
                            // the Slint row. The strip at the bottom of this Stack is
                            // what gives it back on hover.
                            if (!_shell.appFullscreen) const AppTitleRow(),
                            // Keyed, and it has to be. Entering logo-fullscreen
                            // drops the row above, which slides this from index 1
                            // to index 0; unkeyed, Flutter matches a Column's
                            // children by position, finds a different type there
                            // and rebuilds this whole branch -- taking the
                            // IndexedStack and every page's State with it. The key
                            // is what lets it be recognised as the same child that
                            // merely moved. Same defect the resize edges had.
                            Expanded(
                              key: const ValueKey('shell-body'),
                              child: Row(
                                children: [
                                  Sidebar(
                                    controller: _shell,
                                    onCycleTheme: _shell.cycleTheme,
                                    themeIcon: _themeIcon,
                                  ),
                                  // Every section stays alive across a switch: Transfer polls a
                                  // running server, Music holds a playing deck and a queue, and
                                  // rebuilding either every time it is looked at would restart
                                  // the poll and lose the ledger's page and sort, or drop the
                                  // now-playing. The ones that cost something to keep warm are
                                  // told whether they are on screen instead.
                                  //
                                  // `ContentSurface` in ui/main.slint: the page is a card, not
                                  // the window. The shell there is one
                                  // `HorizontalLayout { padding: 14px; spacing: 14px }` around
                                  // the rail and this, which is the gutter the port was
                                  // missing — the page ran flush into the sidebar and off
                                  // three edges of the window.
                                  Expanded(
                                    child: Container(
                                      margin: const EdgeInsets.fromLTRB(
                                          14, 14, 14, 14),
                                      // The language's card: a raised sheet, a
                                      // pane, a tonal card.
                                      decoration: skin.surface(SurfaceRole.card,
                                              radius: Tokens.radiusLg) ??
                                          BoxDecoration(
                                            color: tokens.panel,
                                            borderRadius: BorderRadius.circular(
                                                Tokens.radiusLg),
                                            border: Border.all(
                                                color: tokens.outline),
                                            boxShadow: const [
                                              BoxShadow(
                                                  color: Color(0x40000000),
                                                  blurRadius: 32),
                                            ],
                                          ),
                                      clipBehavior: Clip.antiAlias,
                                      child: IndexedStack(
                                        // The position in what is drawn, not
                                        // the enum's ordinal: the two were one
                                        // number until Settings → Sections let
                                        // the sidebar be shorter than ten, and
                                        // an ordinal used as a position points
                                        // at the wrong page the moment
                                        // anything ahead of it is hidden.
                                        index: _shell.stackIndex,
                                        children: [
                                          // TickerMode is what makes "kept alive" stop short of
                                          // "kept animating". IndexedStack holds all ten pages
                                          // in the tree and skips painting the nine underneath,
                                          // but it does not stop their tickers -- and one
                                          // offstage indeterminate spinner is enough to ask for
                                          // a frame at every vsync, for as long as the app is
                                          // open. Settings is exactly that: it does not load
                                          // until it is first opened, so its page sits on
                                          // FirstLoad's CircularProgressIndicator from launch,
                                          // and the window rebuilt, laid out, painted and
                                          // re-walked its semantics tree 144 times a second
                                          // over a screen where nothing moved.
                                          // Only what the sidebar shows gets
                                          // built. A hidden section is not a
                                          // page kept offstage — it is a page
                                          // that never existed this run, which
                                          // is the whole reason hiding one is
                                          // worth anything.
                                          //
                                          // Keyed by the section, so the list
                                          // can change while the app runs.
                                          // Unkeyed, Flutter matches a
                                          // multi-child list by position:
                                          // hiding Photos would re-match every
                                          // page after it and throw away its
                                          // State. With a key on each, the
                                          // survivors are recognised as the
                                          // same child that merely moved, which
                                          // is what lets Settings → Sections
                                          // take effect where you can see it
                                          // rather than at the next launch.
                                          for (final (i, s)
                                              in _shell.sections.indexed)
                                            TickerMode(
                                              key: ValueKey(s),
                                              enabled: i == _shell.stackIndex,
                                              child: _pageFor(s, at),
                                            ),
                                        ],
                                      ),
                                    ),
                                  ),
                                ],
                              ),
                            ),
                          ]),
                          if (_shell.appFullscreen) const FullscreenPeek(),
                        ]);
                      },
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
