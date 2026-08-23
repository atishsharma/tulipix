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
import 'shell/shell_controller.dart';
import 'shell/sidebar.dart';
import 'src/rust/api/music.dart';
import 'src/rust/api/shell.dart';
import 'src/rust/api/transfer.dart';
import 'src/rust/api/videos.dart';
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
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
        // Same reason as the other two: the process going away does not take
        // the mpv window with it on every platform, and a film left playing
        // over a closed app is not a thing anyone asked for.
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
    if (dark == _dark && oled == _oled) {
      setState(() {});
      return;
    }
    setState(() {
      _dark = dark;
      _oled = oled;
    });
  }

  /// light → dark → extra-dark → light, and stored. The sidebar's theme button
  /// calls this; the same three stops the Slint build cycles through.
  void _cycleTheme() {
    final next = !_dark
        ? 'dark'
        : !_oled
            ? 'extra-dark'
            : 'light';
    _shell.send(ShellCmd.setTheme(theme: next));
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
      // The floating mini and the zen player wrap the whole app: what is
      // playing does not stop playing when you leave the Music section, and
      // they are how that stays visible.
      home: MusicOverlay(
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
                    onCycleTheme: _cycleTheme,
                    themeIcon: _themeIcon,
                  ),
                  // Every section stays alive across a switch: Transfer polls a
                  // running server, Music holds a playing mpv and a queue, and
                  // rebuilding either every time it is looked at would restart
                  // the poll and lose the ledger's page and sort, or drop the
                  // now-playing. The ones that cost something to keep warm are
                  // told whether they are on screen instead.
                  Expanded(
                    child: IndexedStack(
                      index: at.index,
                      children: [
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
                      ],
                    ),
                  ),
                ],
              );
            },
          ),
        ),
      ),
    );
  }
}
