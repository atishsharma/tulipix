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
}
