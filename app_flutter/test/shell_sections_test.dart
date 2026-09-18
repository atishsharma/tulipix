// The sidebar list, and the number the IndexedStack is driven by.
//
// `Section`'s ordinal doubled as the stack index for the whole port, which was
// fine while the sidebar was a const list of all ten. It stops being fine the
// moment one can be hidden: an ordinal used as a position points at the wrong
// page as soon as anything ahead of it is gone, and that failure is silent —
// you press Music and Books appears.

import 'package:flutter_test/flutter_test.dart';
import 'package:tulipix/design/tokens.dart';
import 'package:tulipix/shell/shell_controller.dart';

void main() {
  test('nothing saved yet means everything', () {
    // Before the first snapshot. A shell that starts short and then fills in
    // shows a sidebar that grows in front of you, and the landing section would
    // be whatever happened to be first for as long as the round trip takes.
    expect(sectionsFrom(null), Section.values);
    expect(sectionsFrom(const []), Section.values);
  });

  test('the saved order is the order', () {
    expect(
      sectionsFrom(const ['music', 'home', 'photos', 'settings']),
      [Section.music, Section.home, Section.photos, Section.settings],
    );
  });

  test('an id this build does not have is dropped, not a hole', () {
    final out = sectionsFrom(const ['photos', 'nonesuch', 'settings']);
    expect(out, [Section.photos, Section.settings]);
  });

  test('Settings is always reachable', () {
    // It cannot be switched off on the Rust side, but a settings file written
    // by a build that did not know that must not be able to strand anyone.
    expect(sectionsFrom(const ['photos']), contains(Section.settings));
    expect(sectionsFrom(const ['photos']).last, Section.settings);
  });

  test('position is not the ordinal', () {
    // The whole point. With Home, Photos and Videos hidden, Music is at 0 and
    // its ordinal is 3 — an IndexedStack given 3 would show whatever is fourth,
    // or throw if nothing is.
    final shown = sectionsFrom(const ['music', 'books', 'settings']);
    expect(shown.indexOf(Section.music), 0);
    expect(Section.music.index, isNot(0));
    for (final s in shown) {
      expect(shown.indexOf(s), lessThan(shown.length));
    }
  });
}
