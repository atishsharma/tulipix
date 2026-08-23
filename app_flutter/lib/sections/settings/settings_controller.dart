// The Settings section's state.
//
// Six of the nine tabs are one shape: a list of rows, each a toggle, a text
// box, a read-only status line, an action button or a choice. The controller
// does not know what any of them mean — the row carries its key, and the key
// is what the bridge writes.

import 'package:flutter/material.dart';

import '../../src/rust/api/settings.dart';

/// The left rail, in order. `status` is last because it is a dashboard rather
/// than a setting; it is here because that is where the Slint build put it.
const List<({String id, String label, IconData icon})> kSettingsTabs = [
  (id: 'profile', label: 'You & Home', icon: Icons.person_outline),
  (id: 'libraries', label: 'Libraries', icon: Icons.folder_outlined),
  (id: 'playback', label: 'Playback', icon: Icons.play_circle_outline),
  (id: 'services', label: 'Services & Keys', icon: Icons.bolt_outlined),
  (id: 'ai', label: 'AI Features', icon: Icons.auto_awesome_outlined),
  (id: 'security', label: 'Security', icon: Icons.lock_outline),
  (id: 'data', label: 'Backup & Data', icon: Icons.archive_outlined),
  (id: 'advanced', label: 'Advanced', icon: Icons.build_outlined),
  (id: 'status', label: 'Status', icon: Icons.monitor_heart_outlined),
];

/// The four Home layouts. All four draw the same snapshot; the note is what
/// each one does with it.
const List<({String id, String label, String note})> kHomeLayouts = [
  (id: 'classic', label: 'Classic', note: 'Counts, Continue, shelves'),
  (
    id: 'welcome',
    label: 'Welcome',
    note: 'A hello, launchers, everything offered as cards'
  ),
  (
    id: 'cinema',
    label: 'Cinema',
    note: 'Newest item full-bleed, the player standing up'
  ),
  (
    id: 'stream',
    label: 'Stream',
    note: 'One timeline of everything that happened'
  ),
];

class SettingsController extends ChangeNotifier {
  SettingsState? state;
  Object? error;
  bool busy = false;
  String notice = '';

  Future<void> send(SettingsCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      final next = await settingsDispatch(cmd: cmd);
      state = next;
      notice = next.notice;
    } catch (e) {
      error = e;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  Future<void> refresh() => send(const SettingsCmd.refresh());

  /// The rows for the tab currently open, or an empty list for the two tabs
  /// that are not row lists.
  List<SettingItem> rowsFor(String tab) {
    final st = state;
    if (st == null) return const [];
    return switch (tab) {
      'playback' => st.playback,
      'services' => st.services,
      'ai' => st.ai,
      'security' => st.security,
      'data' => st.data,
      'advanced' => st.advanced,
      _ => const [],
    };
  }

  void clearError() {
    error = null;
    notifyListeners();
  }

  void clearNotice() {
    notice = '';
    notifyListeners();
  }
}
