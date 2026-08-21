// Flutter shell — phase 4 territory, standing in as the thinnest thing that can
// host a section. Sidebar, routing, settings panels, the custom caption row and
// the home rails are not ported yet; Photos is.

import 'package:flutter/material.dart';

import 'design/tokens.dart';
import 'sections/photos/photos_page.dart';
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

  @override
  Widget build(BuildContext context) {
    final tokens = _dark ? Tokens.dark(oled: _oled) : Tokens.light();
    return MaterialApp(
      title: 'Tulipix',
      debugShowCheckedModeBanner: false,
      theme: tulipixTheme(tokens),
      home: Scaffold(
        body: const PhotosPage(),
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
