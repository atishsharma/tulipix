// The three widget styles have to survive being BUILT, not just measured.
//
// mini_widget_test.dart pins the arithmetic; this pumps the real card at the
// real box. It exists because a mini that throws during build is replaced by an
// ErrorWidget, and an ErrorWidget is an UNPOSITIONED child of the overlay's
// Stack — so it sizes itself to the whole window and paints over the app. The
// symptom is "the app vanished and only the player is left", which names
// neither the style nor the exception.

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:tulipix/design/tokens.dart';
import 'package:tulipix/sections/music/mini_widget.dart';
import 'package:tulipix/sections/music/music_controller.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  final c = MusicController.instance;

  // The real theme: every colour in the widget comes off the `Tokens` theme
  // extension, and `context.tokens` null-asserts it.
  Widget host(Size box) => MaterialApp(
        theme: tulipixTheme(Tokens.dark()),
        home: Scaffold(
          body: Center(
            child: SizedBox(
              width: box.width,
              height: box.height,
              child: MiniWidgetCard(controller: c),
            ),
          ),
        ),
      );

  for (final style in MiniStyle.values) {
    testWidgets('${style.label} builds at its own box', (tester) async {
      c.widgetStyle = style;
      c.widgetScale = 1.0;
      c.pillOpen = false;
      c.pillLyrics = false;
      await tester.pumpWidget(host(c.widgetWindow));
      expect(tester.takeException(), isNull);
    });

    testWidgets('${style.label} builds at 0.7 and at 3.0', (tester) async {
      for (final scale in [0.7, 3.0]) {
        c.widgetStyle = style;
        c.scaleWidget(scale);
        await tester.pumpWidget(host(c.widgetWindow));
        expect(tester.takeException(), isNull, reason: 'at $scale');
      }
    });
  }

  testWidgets('the pill builds with both extras out', (tester) async {
    c.widgetStyle = MiniStyle.pill;
    c.widgetScale = 1.0;
    c.pillOpen = true;
    c.pillLyrics = true;
    await tester.pumpWidget(host(c.widgetWindow));
    expect(tester.takeException(), isNull);
  });
}
