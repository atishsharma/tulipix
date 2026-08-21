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
import 'sections/photos/photos_page.dart';
import 'sections/transfer/transfer_page.dart';
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

  /// Closing the window has to close the listening socket. The process exit
  /// would do it too, but not before the OS has had a moment to hold the port —
  /// and `abort` is what the Slint build's shutdown path calls for the same
  /// reason.
  AppLifecycleListener? _lifecycle;

  @override
  void initState() {
    super.initState();
    _lifecycle = AppLifecycleListener(
      onExitRequested: () async {
        transferShutdown();
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
      home: Scaffold(
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
                  icon: Icon(Icons.share_outlined),
                  selectedIcon: Icon(Icons.share),
                  label: Text('Transfer'),
                ),
              ],
            ),
            // Both stay alive across a switch: Transfer polls a running server,
            // and rebuilding the page every time it is looked at would restart
            // that poll and lose the ledger's page and sort.
            Expanded(
              child: IndexedStack(
                index: _section,
                children: [
                  const PhotosPage(),
                  // Told when it is on screen so its poll can slow down: the
                  // server keeps running when the user is in Photos, but
                  // nothing there needs repainting six times a second.
                  TransferPage(visible: _section == 1),
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
    );
  }
}
