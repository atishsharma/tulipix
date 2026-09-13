// The Status section's state.
//
// One snapshot, polled. `collect()` queries ten databases, so the tick is slow
// (30s) unless someone is looking at the page and fast (2s) while they are —
// the same rule the loopback dashboard follows, and for the same reason: this
// page is a battery graph if it does not.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/status.dart';

/// While the page is on screen.
const Duration kStatusFast = Duration(seconds: 2);

/// While it is not. The controller keeps ticking because a stale dashboard that
/// says "3 failed jobs" from ten minutes ago is worse than one that is late.
const Duration kStatusSlow = Duration(seconds: 30);

class StatusController extends ChangeNotifier {
  StatusState? state;
  Object? error;
  bool busy = false;

  /// What the last action said it did. Cleared by the next refresh, so it does
  /// not sit under a page that has moved on.
  String notice = '';

  bool _watched = false;
  Timer? _tick;

  StatusController() {
    _schedule();
  }

  /// Whether anyone is looking. The page sets it in `initState`/`dispose`.
  set watched(bool on) {
    if (_watched == on) return;
    _watched = on;
    _schedule();
    if (on) refresh();
  }

  void _schedule() {
    _tick?.cancel();
    _tick = Timer.periodic(_watched ? kStatusFast : kStatusSlow, (_) {
      if (!busy) refresh();
    });
  }

  @override
  void dispose() {
    _tick?.cancel();
    super.dispose();
  }

  Future<void> send(StatusCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      final next = await statusDispatch(cmd: cmd);
      state = next;
      notice = next.notice;
      if (next.error.isNotEmpty) error = next.error;
    } catch (e) {
      error = e;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  Future<void> refresh() => send(const StatusCmd.refresh());

  /// A rescan in flight. Not [busy]: the call returns when the last scan does,
  /// and the tick has to keep refreshing under it or nothing moves on screen.
  bool rescanning = false;

  Future<void> rescan() async {
    if (rescanning) return;
    rescanning = true;
    notifyListeners();
    try {
      final next = await statusDispatch(cmd: const StatusCmd.rescan());
      state = next;
      notice = next.notice;
      if (next.error.isNotEmpty) error = next.error;
    } catch (e) {
      error = e;
    } finally {
      rescanning = false;
      notifyListeners();
    }
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

/// The dashboard palette, resolved from the accent *names* the snapshot uses.
///
/// The HTML page reads these as CSS custom properties and the Slint page holds
/// them as literals. Here they stay names until the moment of painting, which
/// is the one version of this that a theme could still reach.
Color accentOf(String name) => switch (name) {
      'violet' => const Color(0xFF8B5CF6),
      'indigo' => const Color(0xFF6366F1),
      'pink' => const Color(0xFFEC4899),
      'emerald' => const Color(0xFF10B981),
      'teal' => const Color(0xFF14B8A6),
      'cyan' => const Color(0xFF06B6D4),
      'orange' => const Color(0xFFF97316),
      'lime' => const Color(0xFF84CC16),
      'brand' => const Color(0xFF7C3AED),
      // Anything unrecognised is slate, exactly as the page's `accent()` does.
      _ => const Color(0xFF94A3B8),
    };

/// The health dot. Three colours and only three, the same ones the sidebar lamp
/// uses — this is the one place in the app that reports health, and health does
/// not borrow a section accent.
Color levelColor(String level) => switch (level) {
      'ok' => const Color(0xFF22C55E),
      'busy' => const Color(0xFF10B981),
      'problem' => const Color(0xFFEF4444),
      _ => const Color(0xFF94A3B8),
    };

/// key → glyph. The one place this file knows anything section-specific, and it
/// is a picture, not a fact.
IconData iconForKey(String key) => switch (key) {
      'photos' => Icons.image_outlined,
      'videos' => Icons.movie_outlined,
      'music' => Icons.music_note_outlined,
      'books' => Icons.menu_book_outlined,
      'cloud' => Icons.cloud_outlined,
      'tools' => Icons.build_outlined,
      'transfer' => Icons.share_outlined,
      'finances' => Icons.account_balance_wallet_outlined,
      'ai' => Icons.auto_awesome_outlined,
      _ => Icons.terminal_outlined,
    };
