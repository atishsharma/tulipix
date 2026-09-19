// The Settings section's state.
//
// Six of the nine tabs are one shape: a list of rows, each a toggle, a text
// box, a read-only status line, an action button or a choice. The controller
// does not know what any of them mean — the row carries its key, and the key
// is what the bridge writes.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../shell/lock/lock_controller.dart';
import '../../src/rust/api/settings.dart';

/// The left rail, in order. `status` is last because it is a dashboard rather
/// than a setting; it is here because that is where the Slint build put it.
/// Each tab's colour is its page head's too.
const List<({String id, String label, IconData icon, Color tint})>
    kSettingsTabs = [
  (
    id: 'profile',
    label: 'You & Home',
    icon: Icons.person_outline,
    tint: Color(0xFF7C3AED)
  ),
  (
    id: 'sections',
    label: 'Sections',
    icon: Icons.tune_outlined,
    tint: Color(0xFF6366F1)
  ),
  (
    id: 'libraries',
    label: 'Libraries',
    icon: Icons.folder_outlined,
    tint: Color(0xFFF59E0B)
  ),
  (
    id: 'playback',
    label: 'Playback',
    icon: Icons.play_circle_outline,
    tint: Color(0xFF0EA5E9)
  ),
  (
    id: 'services',
    label: 'Services & Keys',
    icon: Icons.bolt_outlined,
    tint: Color(0xFFEAB308)
  ),
  (
    id: 'ai',
    label: 'AI Features',
    icon: Icons.auto_awesome_outlined,
    tint: Color(0xFF8B5CF6)
  ),
  (
    id: 'security',
    label: 'Security',
    icon: Icons.lock_outline,
    tint: Color(0xFF10B981)
  ),
  (
    id: 'data',
    label: 'Backup & Data',
    icon: Icons.archive_outlined,
    tint: Color(0xFF14B8A6)
  ),
  (
    id: 'advanced',
    label: 'Advanced',
    icon: Icons.build_outlined,
    tint: Color(0xFF94A3B8)
  ),
  (
    id: 'status',
    label: 'Status',
    icon: Icons.monitor_heart_outlined,
    tint: Color(0xFF34D399)
  ),
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

  /// Unsaved edits on You & Home. That page stages everything until Save, so
  /// the rail marks its tab and leaving it asks first.
  final ValueNotifier<bool> profileDirty = ValueNotifier(false);

  /// Whether the Settings page is on screen. Long work is only watched while
  /// it is: a library analysis runs for an hour, and a snapshot a second for
  /// a page nobody is looking at is the cost with none of the use.
  bool _visible = false;

  set visible(bool on) {
    if (_visible == on) return;
    _visible = on;
    _syncPoll();
  }

  Future<void> send(SettingsCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      final next = await settingsDispatch(cmd: cmd);
      state = next;
      notice = next.notice;
      // The lock reads Security's keys; a new timeout or PIN applies now. A
      // plain refresh changed nothing it reads.
      if (cmd is! SettingsCmd_Refresh) LockController.instance.reload();
    } catch (e) {
      error = e;
    } finally {
      busy = false;
      notifyListeners();
      _syncPoll();
    }
  }

  Future<void> refresh() => send(const SettingsCmd.refresh());

  Future<void> sendAction(String key) => send(SettingsCmd.action(key: key));

  /// A notice from this side: what a tab did without a round trip through
  /// the settings dispatch (a rescan, a copy to the clipboard).
  void say(String text) {
    notice = text;
    notifyListeners();
  }

  // ── long work ─────────────────────────────────────────────────────────────
  //
  // A model download and a library analysis both run detached in Rust and
  // report into state the next snapshot reads; there is no stream back.
  // Polling a refresh while either is running is the whole mechanism -- one
  // timer, stopped the moment nothing is, which is cheaper than a stream
  // nobody else needs.

  Timer? _poll;

  /// True while a model download or an update check is in flight.
  bool get aiBusy =>
      (state?.ai ?? const <SettingItem>[]).any((r) => r.state == 'busy');

  bool get _working =>
      aiBusy ||
      (state?.analysing ?? false) ||
      (state?.taskKey.isNotEmpty ?? false);

  /// Checked after every snapshot. The last tick after the work ends is the
  /// one that clears its row, so stopping here, after it landed, never leaves
  /// a bar frozen at 98%.
  void _syncPoll() {
    if (_working && _visible) {
      _poll ??= Timer.periodic(const Duration(seconds: 1), (_) {
        if (!busy) refresh();
      });
    } else {
      _stopPoll();
    }
  }

  void _stopPoll() {
    _poll?.cancel();
    _poll = null;
  }

  @override
  void dispose() {
    _stopPoll();
    profileDirty.dispose();
    super.dispose();
  }

  /// The rows for a data-driven tab, or an empty list for the others.
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
