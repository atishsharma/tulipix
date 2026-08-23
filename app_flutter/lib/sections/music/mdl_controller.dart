// The Downloader's state.
//
// Two things live here that do not live in Rust. The activity log is one: the
// bridge emits each line as an event and this buffer owns it, because a
// scrollback in the snapshot would ride across the boundary on every refresh
// for no reason. The row deltas are the other — a hundred-track download sends
// four progress events per track, and each one patches a single row rather than
// re-reading the whole queue.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/mdl.dart';

/// How many log lines to keep. Enough to watch a long playlist go past and see
/// what failed near the start; not so many that the list widget suffers.
const int kCliMax = 400;

/// The three tables under the queue.
enum MdlPane { queue, downloaded, searches }

class MdlController extends ChangeNotifier {
  MdlController() {
    _events = mdlEvents().listen(_onEvent, onError: (Object e) {
      error = e;
      notifyListeners();
    });
  }

  MdlState? state;
  Object? error;
  bool busy = false;

  MdlPane pane = MdlPane.queue;

  /// The four figures the progress events move. They live here rather than
  /// being read off `state` because frb generates immutable snapshots — an
  /// event cannot write into one, and re-reading the whole queue to learn that
  /// a counter went up by one is the round trip the deltas exist to avoid.
  /// Every snapshot re-seeds them, so they cannot drift.
  int done = 0;
  int skipped = 0;
  int failed = 0;
  String status = 'idle';

  /// The activity log, newest last.
  final List<String> cli = [];

  late final StreamSubscription<MdlEvent> _events;

  @override
  void dispose() {
    _events.cancel();
    super.dispose();
  }

  void _onEvent(MdlEvent e) {
    switch (e) {
      case MdlEvent_Changed():
        refresh();
      case MdlEvent_Row(
          :final index,
          :final stage,
          :final percent,
          :final file
        ):
        final rows = state?.rows;
        if (rows == null || index < 0 || index >= rows.length) return;
        // A patched copy rather than a mutation: MdlRow comes from frb with
        // final fields, and rebuilding one row is cheaper than a bridge call.
        final old = rows[index.toInt()];
        rows[index.toInt()] = MdlRow(
          title: old.title,
          length: old.length,
          album: old.album,
          artists: old.artists,
          mainArtist: old.mainArtist,
          stage: stage,
          percent: percent,
          file: file,
          selected: old.selected,
          artUrl: old.artUrl,
        );
        notifyListeners();
      case MdlEvent_Counters(
          done: final d,
          skipped: final s,
          failed: final f,
        ):
        done = d;
        skipped = s;
        failed = f;
        notifyListeners();
      case MdlEvent_Status(:final text):
        status = text;
        notifyListeners();
      case MdlEvent_Cli(:final line):
        cli.add(line);
        if (cli.length > kCliMax) cli.removeRange(0, cli.length - kCliMax);
        notifyListeners();
    }
  }

  void setPane(MdlPane p) {
    if (pane == p) return;
    pane = p;
    notifyListeners();
    // The two logs are read on demand — nothing writes to them while the queue
    // is on screen, so there is no reason to have loaded them before now.
    switch (p) {
      case MdlPane.downloaded:
        send(const MdlCmd.loadHistory(page: 0));
      case MdlPane.searches:
        send(const MdlCmd.loadSearches(page: 0));
      case MdlPane.queue:
        break;
    }
  }

  void clearCli() {
    cli.clear();
    notifyListeners();
  }

  Future<void> send(MdlCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      final next = await mdlDispatch(cmd: cmd);
      state = next;
      done = next.done;
      skipped = next.skipped;
      failed = next.failed;
      status = next.status;
    } catch (e) {
      error = e;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  Future<void> refresh() => send(const MdlCmd.refresh());

  void clearError() {
    error = null;
    notifyListeners();
  }
}

/// Provider display name → its brand colour, for the badge on a resolved URL.
/// The names come from Rust (`mdlProviders`), so this map is keyed by them
/// rather than by an enum the two sides would both have to carry.
Color providerColor(String name) => switch (name) {
      'Spotify' => const Color(0xFF1DB954),
      'Apple' => const Color(0xFFFA2D48),
      'Amazon Music' => const Color(0xFF25D1DA),
      'YT Music' => const Color(0xFFFF0000),
      'SoundCloud' => const Color(0xFFFF5500),
      'Bandcamp' => const Color(0xFF629AA9),
      'Qobuz' => const Color(0xFF0070EF),
      'Deezer' => const Color(0xFFA238FF),
      'Tidal' => const Color(0xFF00FFFF),
      _ => const Color(0xFF8B5CF6),
    };

/// Queue-row stage → the colour of its pill. Six states, and only "failed" is
/// alarming — "skipped" and "in library" are the downloader doing its job.
Color stageColor(String stage) => switch (stage) {
      'done' => const Color(0xFF10B981),
      'failed' => const Color(0xFFEF4444),
      'skipped' || 'in library' => const Color(0xFF94A3B8),
      'searching' || 'downloading' => const Color(0xFF6366F1),
      'tagging' || 'saving' => const Color(0xFFF59E0B),
      _ => const Color(0xFF64748B),
    };
