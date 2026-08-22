// The Tools section's state.
//
// Everything here is a job: you fill a form, it goes on a queue, a worker in
// Rust runs it and reports back. The controller holds the snapshot and the
// live console; the queue itself lives in `tools.db` and survives a restart.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/tools.dart';

/// The six tabs, and the accent each one wears.
const List<({String id, String label, IconData icon, Color tint})> toolTabs = [
  (
    id: 'fileops',
    label: 'File ops',
    icon: Icons.folder_copy_outlined,
    tint: Color(0xFF6C4DF6)
  ),
  (
    id: 'video',
    label: 'Video',
    icon: Icons.movie_outlined,
    tint: Color(0xFFE0518F)
  ),
  (
    id: 'audio',
    label: 'Audio',
    icon: Icons.graphic_eq,
    tint: Color(0xFF2FBF71)
  ),
  (
    id: 'photo',
    label: 'Photo',
    icon: Icons.image_outlined,
    tint: Color(0xFFF5A623)
  ),
  (
    id: 'subtitles',
    label: 'Subtitles',
    icon: Icons.subtitles_outlined,
    tint: Color(0xFF3A86FF)
  ),
  (id: 'queue', label: 'Queue', icon: Icons.list_alt, tint: Color(0xFFB5179E)),
];

Color tintFor(String category) => toolTabs
    .firstWhere((t) => t.id == category, orElse: () => toolTabs.first)
    .tint;

class ToolsController extends ChangeNotifier {
  ToolsState? state;
  Object? error;
  bool busy = false;

  /// Live per-job progress from the event stream, which arrives far more often
  /// than a snapshot does. Keyed by job id.
  final Map<int, double> liveProgress = <int, double>{};

  StreamSubscription<ToolsEvent>? _events;

  ToolsController() {
    _events = toolsEvents().listen(_onEvent, onError: (Object e) {
      error = e;
      notifyListeners();
    });
  }

  void _onEvent(ToolsEvent event) {
    switch (event) {
      case ToolsEvent_Progress(:final id, :final progress):
        liveProgress[id] = progress;
        notifyListeners();
      case ToolsEvent_Log():
        // The console is read off the snapshot, not accumulated here — one
        // copy of a capped log is enough, and it lives in Rust.
        break;
      case ToolsEvent_Finished(:final id):
        liveProgress.remove(id);
        refresh();
      case ToolsEvent_Failed(:final message):
        error = message;
        notifyListeners();
    }
  }

  Future<void> send(ToolsCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      state = await toolsDispatch(cmd: cmd);
    } catch (e) {
      error = e;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  Future<void> refresh() => send(const ToolsCmd.refresh());

  void clearError() {
    error = null;
    notifyListeners();
  }

  /// The queue's own progress, overridden by anything newer from the stream.
  double progressOf(Job job) => liveProgress[job.id] ?? job.progress;

  @override
  void dispose() {
    _events?.cancel();
    super.dispose();
  }
}

/// A job state as a colour and an icon.
({Color colour, IconData icon}) jobLook(String state) => switch (state) {
      'running' => (colour: const Color(0xFF3A86FF), icon: Icons.play_arrow),
      'queued' => (colour: const Color(0xFF8A8A94), icon: Icons.schedule),
      'paused' => (colour: const Color(0xFFF5A623), icon: Icons.pause),
      'done' => (colour: const Color(0xFF2FBF71), icon: Icons.check),
      'failed' => (colour: const Color(0xFFEF4444), icon: Icons.error_outline),
      'cancelled' => (colour: const Color(0xFF8A8A94), icon: Icons.block),
      _ => (colour: const Color(0xFF8A8A94), icon: Icons.circle_outlined),
    };
