// The Flutter shell.
//
// A port of ui/sidebar.slint over an IndexedStack of the ten sections. The
// sidebar owns which section is up (`ShellController`, a singleton, because
// Home's tiles and the Status lamp both navigate); this file owns the theme,
// which is read from and written back to the same settings file the Slint
// build uses.
//
// Still scaffolding above the sidebar: no custom caption row, no command
// palette, no lock screen.

import 'dart:ui' show AppExitResponse;

import 'package:flutter/material.dart';
import 'package:media_kit/media_kit.dart';

import 'design/tokens.dart';
import 'sections/books/books_page.dart';
import 'sections/cloud/cloud_page.dart';
import 'sections/finances/finances_page.dart';
import 'sections/home/home_page.dart';
import 'sections/music/music_overlay.dart';
import 'sections/music/music_page.dart';
import 'sections/photos/photos_page.dart';
import 'sections/settings/settings_page.dart';
import 'sections/tools/tools_page.dart';
import 'sections/transfer/transfer_page.dart';
import 'sections/videos/videos_page.dart';
import 'playback/video_layer.dart';
import 'shell/shell_controller.dart';
import 'shell/sidebar.dart';
import 'src/rust/api/music.dart';
import 'src/rust/api/transfer.dart';
import 'src/rust/api/videos.dart';
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  // libmpv, for both players. Must run before any `Player` is constructed, and
  // the music controller builds one the moment the Music section is touched.
  MediaKit.ensureInitialized();
  await RustLib.init();
  runApp(const TulipixApp());
}

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
    // The first snapshot also carries the stored theme, which is why this runs
    // before anything is drawn rather than when the sidebar first appears.
    WidgetsBinding.instance.addPostFrameCallback((_) => _shell.refresh());
    _lifecycle = AppLifecycleListener(
      onExitRequested: () async {
        transferShutdown();
        musicShutdown();
        // Same reason as the other two: what is on should stop when the app
        // goes, and this is what drops the hooks that would otherwise write a
        // position back against a session nobody is watching.
        videosShutdown();
        return AppExitResponse.exit;
      },
    );
  }

  @override
  void dispose() {
    _shell.removeListener(_onShell);
    _lifecycle?.dispose();
    super.dispose();
  }

  void _onShell() {
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
    if (dark == _dark && oled == _oled) return;
    setState(() {
      _dark = dark;
      _oled = oled;
    });
  }

  IconData get _themeIcon => !_dark
      ? Icons.light_mode
      : _oled
          ? Icons.star_outline
          : Icons.dark_mode;

  @override
  Widget build(BuildContext context) {
    final tokens = _dark ? Tokens.dark(oled: _oled) : Tokens.light();
    return MaterialApp(
      title: 'Tulipix',
      debugShowCheckedModeBanner: false,
      theme: tulipixTheme(tokens),
      // The floating mini, the zen player and the video all wrap the whole
      // app: what is playing does not stop playing when you leave the section
      // that started it, and these are how it stays visible. Video is outermost
      // — a film covers the window, including the mini.
      home: VideoLayer(
        child: MusicOverlay(
          child: Scaffold(
            backgroundColor: tokens.bg,
            body: AnimatedBuilder(
              animation: _shell,
              builder: (context, _) {
                final at = _shell.section;
                return Row(
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
                        margin: const EdgeInsets.fromLTRB(14, 14, 14, 14),
                        decoration: BoxDecoration(
                          color: tokens.panel,
                          borderRadius: BorderRadius.circular(Tokens.radiusLg),
                          border: Border.all(color: tokens.outline),
                          boxShadow: const [
                            BoxShadow(color: Color(0x40000000), blurRadius: 32),
                          ],
                        ),
                        clipBehavior: Clip.antiAlias,
                        child: IndexedStack(
                          index: at.index,
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
                            for (final (i, page) in <Widget>[
                              HomePage(visible: at == Section.home),
                              const PhotosPage(),
                              const VideosPage(),
                              const MusicPage(),
                              const BooksPage(),
                              const CloudPage(),
                              const ToolsPage(),
                              TransferPage(visible: at == Section.transfer),
                              const FinancesPage(),
                              SettingsPage(visible: at == Section.settings),
                            ].indexed)
                              TickerMode(enabled: i == at.index, child: page),
                          ],
                        ),
                      ),
                    ),
                  ],
                );
              },
            ),
          ),
        ),
      ),
    );
  }
}
