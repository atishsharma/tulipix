// Flutter shell — phase 4 territory, standing in as the thinnest thing that can
// host a section. The real sidebar, routing, settings panels, the custom
// caption row and the home rails are not ported yet. What is here is a stock
// NavigationRail, which exists so the two ported sections can both be reached
// and compared against the Slint build; it is scaffolding, not a port of
// ui/sidebar.slint, and phase 4 replaces it wholesale.

// `AppExitResponse` is declared in dart:ui and re-exported by nothing, so it
// is named directly. `show` rather than a bare import: dart:ui also carries a
// Color and a Size, and material's are the ones this file means.
import 'dart:ui' show AppExitResponse;

import 'package:flutter/material.dart';

import 'design/tokens.dart';
import 'sections/books/books_page.dart';
import 'sections/cloud/cloud_page.dart';
import 'sections/finances/finances_page.dart';
import 'sections/music/music_overlay.dart';
import 'sections/music/music_page.dart';
import 'sections/photos/photos_page.dart';
import 'sections/tools/tools_page.dart';
import 'sections/transfer/transfer_page.dart';
import 'src/rust/api/music.dart';
import 'src/rust/api/transfer.dart';
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
  // Theme.dark / Theme.oled from ui/tokens.slint. Reading and writing the real
  // Settings file is a phase 4 gate item, so for now these are session-local.
  bool _dark = true;
  bool _oled = false;

  int _section = 0;

  /// Closing the window has to close the listening socket, and stop the audio.
  /// The process exit would do both too, but not before the OS has had a moment
  /// to hold the port — and not before the speakers have had another second of
  /// whatever was playing after the window went away.
  AppLifecycleListener? _lifecycle;

  @override
  void initState() {
    super.initState();
    _lifecycle = AppLifecycleListener(
      onExitRequested: () async {
        transferShutdown();
        musicShutdown();
        return AppExitResponse.exit;
      },
    );
  }

  @override
  void dispose() {
    _lifecycle?.dispose();
    super.dispose();
  }

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
          body: Row(
            children: [
              NavigationRail(
                backgroundColor: tokens.panel,
                selectedIndex: _section,
                labelType: NavigationRailLabelType.all,
                onDestinationSelected: (i) => setState(() => _section = i),
                destinations: const [
                  NavigationRailDestination(
                    icon: Icon(Icons.photo_library_outlined),
                    selectedIcon: Icon(Icons.photo_library),
                    label: Text('Photos'),
                  ),
                  NavigationRailDestination(
                    icon: Icon(Icons.library_music_outlined),
                    selectedIcon: Icon(Icons.library_music),
                    label: Text('Music'),
                  ),
                  NavigationRailDestination(
                    icon: Icon(Icons.menu_book_outlined),
                    selectedIcon: Icon(Icons.menu_book),
                    label: Text('Books'),
                  ),
                  NavigationRailDestination(
                    icon: Icon(Icons.cloud_outlined),
                    selectedIcon: Icon(Icons.cloud),
                    label: Text('Cloud'),
                  ),
                  NavigationRailDestination(
                    icon: Icon(Icons.build_outlined),
                    selectedIcon: Icon(Icons.build),
                    label: Text('Tools'),
                  ),
                  NavigationRailDestination(
                    icon: Icon(Icons.share_outlined),
                    selectedIcon: Icon(Icons.share),
                    label: Text('Transfer'),
                  ),
                  NavigationRailDestination(
                    icon: Icon(Icons.savings_outlined),
                    selectedIcon: Icon(Icons.savings),
                    label: Text('Finances'),
                  ),
                ],
              ),
              // All three stay alive across a switch: Transfer polls a running
              // server, Music holds a playing mpv and a queue, and rebuilding
              // either every time it is looked at would restart the poll and
              // lose the ledger's page and sort, or drop the now-playing.
              Expanded(
                child: IndexedStack(
                  index: _section,
                  children: [
                    const PhotosPage(),
                    const MusicPage(),
                    const BooksPage(),
                    const CloudPage(),
                    const ToolsPage(),
                    // Told when it is on screen so its poll can slow down: the
                    // server keeps running when the user is in Photos, but
                    // nothing there needs repainting six times a second.
                    TransferPage(visible: _section == 5),
                    const FinancesPage(),
                  ],
                ),
              ),
            ],
          ),
          floatingActionButton: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              // Parity checking means flipping both themes and the OLED tier on
              // the same screen, side by side with the Slint build.
              FloatingActionButton.small(
                heroTag: 'theme',
                tooltip: _dark ? 'Light theme' : 'Dark theme',
                onPressed: () => setState(() => _dark = !_dark),
                child: Icon(_dark ? Icons.light_mode : Icons.dark_mode),
              ),
              const SizedBox(width: 8),
              FloatingActionButton.small(
                heroTag: 'oled',
                tooltip: 'OLED tier',
                onPressed: () => setState(() => _oled = !_oled),
                child: const Icon(Icons.contrast),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
