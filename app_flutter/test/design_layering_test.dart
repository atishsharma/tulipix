// lib/design is handed what it draws; it does not go and get it.
//
// Every section imports skin.dart. A design file that imported a section, the
// playback engine or the bridge handed all of them that dependency too — which
// is how Music's controller, its commands and the audio deck came to sit under
// Books and Finances. The slots carry values and callbacks instead.

import 'dart:io';

import 'package:flutter_test/flutter_test.dart';

void main() {
  test('the design layer reaches into no section, engine or bridge', () {
    final files = Directory('lib/design')
        .listSync(recursive: true)
        .whereType<File>()
        .where((f) => f.path.endsWith('.dart'));
    expect(files, isNotEmpty);
    for (final f in files) {
      for (final line in f.readAsLinesSync()) {
        if (!line.startsWith('import ') && !line.startsWith('export ')) {
          continue;
        }
        expect(line, isNot(matches(r'sections/|playback/|src/rust|shell/')),
            reason: f.path);
      }
    }
  });

  /// A popup is opaque, in every design language.
  ///
  /// `Tokens.panel` is 0xD0FFFFFF on the light palette and `modal` 0xE6FFFFFF.
  /// That translucency is what makes a panel read as lying ON the page, and it
  /// is exactly wrong for a dialog or a menu floating OVER arbitrary content,
  /// where whatever is behind it reads through the text. `panelSolid` and
  /// `modalSolid` are the same hues composed over the ground.
  ///
  /// Two call sites had already worked around this by hand with
  /// `t.panel.withValues(alpha: 1)`, which is the same fix without the blend —
  /// a good sign it was worth making general rather than finding a third time.
  test('no dialog is given a see-through fill', () {
    final files = Directory('lib')
        .listSync(recursive: true)
        .whereType<File>()
        .where((f) => f.path.endsWith('.dart'));
    expect(files, isNotEmpty);
    final offenders = <String>[];
    for (final f in files) {
      final lines = f.readAsLinesSync();
      for (var i = 0; i < lines.length; i++) {
        final line = lines[i];
        // The fill a Dialog / AlertDialog / PopupMenu is handed.
        if (!RegExp(r'(backgroundColor|color)\s*:\s*t\.(panel|modal)\b')
            .hasMatch(line)) {
          continue;
        }
        // Only where it IS a popup's own surface: look back a few lines for
        // the widget that opened it.
        final back = lines.sublist(i - 4 < 0 ? 0 : i - 4, i).join(' ');
        if (RegExp(r'(AlertDialog|Dialog|PopupMenuThemeData|DialogThemeData)\(')
            .hasMatch(back)) {
          offenders.add('${f.path}:${i + 1}  ${line.trim()}');
        }
      }
    }
    expect(offenders, isEmpty,
        reason: 'use t.panelSolid / t.modalSolid:\n${offenders.join('\n')}');
  });

}
