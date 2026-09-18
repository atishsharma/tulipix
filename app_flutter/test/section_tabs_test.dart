// Trimming a section's tab row.
//
// The two properties that matter are both about not stranding anyone: the tab
// you are standing on cannot be taken out from under you, and a row can never
// end up empty however the setting got that way.

import 'package:flutter_test/flutter_test.dart';
import 'package:tulipix/shell/section_tabs.dart';

typedef T = ({String id, String label});

const tabs = <T>[
  (id: 'recent', label: 'Timeline'),
  (id: 'people', label: 'People'),
  (id: 'places', label: 'Places'),
];

String idOf(T t) => t.id;
List<String> ids(List<T> v) => [for (final t in v) t.id];

void main() {
  setUp(() => applyTabsOff(const []));

  test('nothing switched off is the whole row', () {
    expect(keepTabs('photos', tabs, idOf), tabs);
  });

  test('a switched-off tab is dropped, and only from its own section', () {
    applyTabsOff(const ['photos:places']);
    expect(ids(keepTabs('photos', tabs, idOf)), ['recent', 'people']);
    expect(ids(keepTabs('videos', tabs, idOf)), ['recent', 'people', 'places']);
  });

  test('the tab you are on stays until you leave it', () {
    applyTabsOff(const ['photos:places']);
    final shown = keepTabs(
      'photos',
      tabs,
      idOf,
      active: (t) => t.id == 'places',
    );
    expect(ids(shown), ['recent', 'people', 'places']);
  });

  test('a row never ends up empty', () {
    // Not reachable through the panel, which refuses the last one — but a
    // settings file is a text file and a blank tab row is a page with no way
    // anywhere.
    applyTabsOff(const ['photos:recent', 'photos:people', 'photos:places']);
    expect(keepTabs('photos', tabs, idOf), tabs);
  });

  test('a toggle is visible without waiting for the next shell snapshot', () {
    setTabOff('photos', 'people', false);
    expect(tabOn('photos', 'people'), isFalse);
    expect(ids(keepTabs('photos', tabs, idOf)), ['recent', 'places']);
    setTabOff('photos', 'people', true);
    expect(tabOn('photos', 'people'), isTrue);
  });

  test('every listed section names tabs its page actually has', () {
    // The catalogue is built from the pages own lists, so this is really a
    // guard against a section being added here with a hand-typed list.
    expect(sectionTabs.keys, containsAll(['photos', 'videos', 'music']));
    for (final MapEntry(key: section, value: entries) in sectionTabs.entries) {
      expect(entries, isNotEmpty, reason: section);
      expect(
        entries.map((e) => e.id).toSet().length,
        entries.length,
        reason: '$section has a duplicate tab id',
      );
      for (final e in entries) {
        expect(e.id.trim(), isNotEmpty);
        expect(e.label.trim(), isNotEmpty);
      }
    }
  });
}
