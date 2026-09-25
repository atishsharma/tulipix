// Which tabs a section draws, and which of them have been switched off.
//
// The tab lists themselves stay where they are — each page owns its own, and
// copying them here is how two lists start disagreeing. What this file adds is
// one name for each of them, so the Sections panel can offer the row without
// importing five pages, and one filter every tab row runs through.
//
// Nine of the fourteen sections have a tab row that is a list. Books draws filter
// chips whose active states are each computed differently, Cloud is a wizard
// and a browser rather than tabs, Transfer is one page of cards, Home's cards
// already have their own switches in Settings › Home, and Settings is the way
// back and keeps everything. Those five are not listed, and the panel shows no
// chips for them rather than chips that do nothing.

import '../sections/arcade/arcade_controller.dart' show arcadeTabs;
import '../sections/archive/archive_controller.dart' show archiveTabs;
import '../sections/feeds/feeds_controller.dart' show feedTabs;
import '../sections/finances/finances_controller.dart' show finTabs;
import '../sections/journal/journal_controller.dart' show journalTabs;
import '../sections/kitchen/kitchen_controller.dart' show kitchenTabs;
import '../sections/music/music_controller.dart' show musicViews;
import '../sections/papers/papers_controller.dart' show papersTabs;
import '../sections/photos/photos_controller.dart' show photoCategories;
import '../sections/studio/studio_controller.dart' show studioTabs;
import '../sections/tools/tools_controller.dart' show toolTabs;
import '../sections/places/places_controller.dart' show placesTabs;
import '../sections/voice/voice_controller.dart' show voiceTabs;
import '../sections/videos/videos_controller.dart' show kVideoCategories;

typedef TabEntry = ({String id, String label});

/// Section id → the tabs that section draws, in the order it draws them.
final Map<String, List<TabEntry>> sectionTabs = {
  'photos': [for (final c in photoCategories) (id: c.id, label: c.label)],
  'videos': [for (final c in kVideoCategories) (id: c.id, label: c.label)],
  'music': [for (final v in musicViews) (id: v.id, label: v.label)],
  'tools': [for (final t in toolTabs) (id: t.id, label: t.label)],
  'finances': [for (final f in finTabs) (id: f.id, label: f.label)],
  'feeds': [for (final f in feedTabs) (id: f.id, label: f.label)],
  'journal': [for (final f in journalTabs) (id: f.id, label: f.label)],
  'kitchen': [for (final f in kitchenTabs) (id: f.id, label: f.label)],
  'papers': [for (final f in papersTabs) (id: f.id, label: f.label)],
  'voice': [for (final f in voiceTabs) (id: f.id, label: f.label)],
  'places': [for (final f in placesTabs) (id: f.id, label: f.label)],
  'studio': [for (final f in studioTabs) (id: f.id, label: f.label)],
  'archive': [for (final f in archiveTabs) (id: f.id, label: f.label)],
  'arcade': [for (final f in arcadeTabs) (id: f.id, label: f.label)],
};

/// `<section>:<tab>` for every tab that is off. Rust's spelling, kept as it
/// arrives so there is one format rather than a translation to get wrong.
Set<String> _off = const {};

/// Take the shell's list. Called once per shell snapshot.
void applyTabsOff(Iterable<String> pairs) => _off = pairs.toSet();

/// Keep the live answer in step with a toggle the Sections panel just sent,
/// rather than waiting for the next shell snapshot to come round.
void setTabOff(String section, String tab, bool on) {
  final next = _off.toSet();
  if (on) {
    next.remove('$section:$tab');
  } else {
    next.add('$section:$tab');
  }
  _off = next;
}

bool tabOn(String section, String tab) => !_off.contains('$section:$tab');

/// The tabs a page should draw, in its own order.
///
/// The tab you are standing on is always kept, whatever the setting says: a
/// page whose header loses the tab it is showing leaves you looking at a body
/// with no chip for it, and switching a tab off from Settings would then strand
/// anyone who happened to be on it. It goes when you leave it.
///
/// If everything would go, nothing does — a blank tab row is a page with no way
/// anywhere, and the panel refusing the last one is not a guarantee a settings
/// file written by hand will respect.
List<T> keepTabs<T>(
  String section,
  List<T> all,
  String Function(T) id, {
  bool Function(T)? active,
}) {
  if (_off.isEmpty) return all;
  final kept = [
    for (final t in all)
      if (tabOn(section, id(t)) || (active?.call(t) ?? false)) t,
  ];
  return kept.isEmpty ? all : kept;
}
