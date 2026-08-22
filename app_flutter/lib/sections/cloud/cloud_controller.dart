// The Cloud section's state.
//
// Everything here is rclone, one subprocess at a time. The controller holds the
// snapshot; the bridge holds the session — which remote is open, where in it,
// and what the connect form currently contains — because every listing has to
// be re-derived from those and shipping them back and forth per keystroke would
// be the whole cost of the section.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/cloud.dart';

class CloudController extends ChangeNotifier {
  CloudState? state;
  Object? error;
  bool busy = false;

  /// The current long-running rclone call, from the event stream. Null when
  /// nothing is moving bytes.
  ({String label, double frac})? transfer;

  StreamSubscription<CloudEvent>? _events;

  CloudController() {
    _events = cloudEvents().listen(_onEvent, onError: (Object e) {
      error = e;
      notifyListeners();
    });
  }

  void _onEvent(CloudEvent event) {
    switch (event) {
      case CloudEvent_Progress(:final label, :final frac):
        transfer = (label: label, frac: frac);
        notifyListeners();
      case CloudEvent_Done():
        transfer = null;
        refresh();
      case CloudEvent_Failed(:final message):
        transfer = null;
        error = message;
        notifyListeners();
    }
  }

  Future<void> send(CloudCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      state = await cloudDispatch(cmd: cmd);
    } catch (e) {
      error = e;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  Future<void> refresh() => send(const CloudCmd.refresh());

  void clearError() {
    error = null;
    notifyListeners();
  }

  /// The path split for a breadcrumb, with the remote itself as the root.
  List<({String label, String path})> get crumbs {
    final st = state;
    if (st == null || st.activeRemote.isEmpty) return const [];
    final out = <({String label, String path})>[
      (label: st.activeRemote, path: ''),
    ];
    var acc = '';
    for (final part in st.path.split('/').where((p) => p.isNotEmpty)) {
      acc = acc.isEmpty ? part : '$acc/$part';
      out.add((label: part, path: acc));
    }
    return out;
  }

  @override
  void dispose() {
    _events?.cancel();
    super.dispose();
  }
}

String fmtBytes(int n) {
  if (n <= 0) return '';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  var v = n.toDouble();
  var u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return '${v.toStringAsFixed(u == 0 ? 0 : 1)} ${units[u]}';
}

/// rclone's RFC-3339 ModTime, shortened to something a column can hold.
String fmtModTime(String iso) {
  if (iso.isEmpty) return '';
  final at = DateTime.tryParse(iso);
  if (at == null) return iso.split('T').first;
  final local = at.toLocal();
  const months = [
    'Jan',
    'Feb',
    'Mar',
    'Apr',
    'May',
    'Jun',
    'Jul',
    'Aug',
    'Sep',
    'Oct',
    'Nov',
    'Dec',
  ];
  return '${local.day} ${months[local.month - 1]} ${local.year}';
}

String fmtWhen(int unixSecs) {
  if (unixSecs <= 0) return 'never';
  final d = DateTime.fromMillisecondsSinceEpoch(unixSecs * 1000);
  final ago = DateTime.now().difference(d);
  if (ago.inMinutes < 1) return 'just now';
  if (ago.inHours < 1) return '${ago.inMinutes} min ago';
  if (ago.inDays < 1) return '${ago.inHours} h ago';
  if (ago.inDays < 30) return '${ago.inDays} d ago';
  return '${d.day}/${d.month}/${d.year}';
}

/// An icon for a file, guessed from its extension. Directories are handled by
/// the caller.
IconData iconFor(String name) {
  final ext = name.contains('.') ? name.split('.').last.toLowerCase() : '';
  return switch (ext) {
    'jpg' ||
    'jpeg' ||
    'png' ||
    'gif' ||
    'webp' ||
    'avif' ||
    'heic' =>
      Icons.image_outlined,
    'mp4' || 'mkv' || 'mov' || 'avi' || 'webm' => Icons.movie_outlined,
    'mp3' || 'flac' || 'm4a' || 'opus' || 'wav' => Icons.audiotrack_outlined,
    'pdf' => Icons.picture_as_pdf_outlined,
    'epub' || 'mobi' || 'azw3' => Icons.menu_book_outlined,
    'zip' || 'tar' || 'gz' || '7z' || 'rar' => Icons.folder_zip_outlined,
    'txt' || 'md' || 'log' => Icons.description_outlined,
    _ => Icons.insert_drive_file_outlined,
  };
}
