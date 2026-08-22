// The Books section's one piece of state, and the only thing that talks to the
// bridge.
//
// Same shape as Photos, Transfer and Music: a snapshot in, a command out, and
// the widgets read fields off the snapshot rather than holding any of their
// own. The reader is part of that snapshot rather than a second controller,
// because turning a page writes progress and progress is what the library's
// percentage rings are drawn from — two controllers would mean two answers to
// the same question.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/books.dart';

/// The five ways the grid can be ordered, in the order the bridge indexes them.
const List<String> sortOptions = [
  'Recently added',
  'Recently read',
  'Title',
  'Author',
  'Rating',
];

/// The quick filters across the top of the library.
const List<({String id, String label, IconData icon})> quickFilters = [
  (id: 'all', label: 'All', icon: Icons.auto_stories_outlined),
  (id: 'reading', label: 'Reading', icon: Icons.bookmark_outline),
  (id: 'unread', label: 'Unread', icon: Icons.circle_outlined),
  (id: 'finished', label: 'Finished', icon: Icons.check_circle_outline),
  (id: 'favorite', label: 'Favourites', icon: Icons.favorite_outline),
  (id: 'missing', label: 'Missing', icon: Icons.link_off),
];

class BooksController extends ChangeNotifier {
  BooksState? state;
  Object? error;
  bool busy = false;

  /// Live scan progress, from the event stream. Null when nothing is running.
  ({int done, int total, String name})? progress;

  StreamSubscription<BooksEvent>? _events;

  /// id → resolved cover path. Survives a refresh: re-extracting a cover that
  /// already painted is a wasted unzip per tile per scroll.
  final Map<int, String> _covers = <int, String>{};
  final Set<int> _coversInFlight = <int>{};

  BooksController() {
    _events = booksEvents().listen(_onEvent, onError: (Object e) {
      error = e;
      notifyListeners();
    });
  }

  void _onEvent(BooksEvent event) {
    switch (event) {
      case BooksEvent_ScanProgress(:final done, :final total, :final name):
        progress = (done: done, total: total, name: name);
        notifyListeners();
      case BooksEvent_ScanFinished():
        progress = null;
        refresh();
      case BooksEvent_Failed(:final message):
        error = message;
        progress = null;
        notifyListeners();
    }
  }

  Reader? get reader {
    final r = state?.reader;
    return (r != null && r.open) ? r : null;
  }

  Future<void> send(BooksCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      state = await booksDispatch(cmd: cmd);
    } catch (e) {
      error = e;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  Future<void> refresh() => send(const BooksCmd.refresh());

  void clearError() {
    error = null;
    notifyListeners();
  }

  /// A cover, extracted once and remembered. Returns null the first time and
  /// notifies when the answer arrives, so a freshly scanned shelf paints
  /// progressively instead of blocking on a hundred zip files.
  String? coverFor(Book book) {
    if (book.cover.isNotEmpty) return book.cover;
    final id = book.id;
    final hit = _covers[id];
    if (hit != null) return hit.isEmpty ? null : hit;
    if (_coversInFlight.add(id)) {
      booksEnsureCover(id: id).then((path) {
        // Remember a miss too: a book with no embedded cover would otherwise
        // be re-opened on every scroll past it.
        _covers[id] = path ?? '';
        _coversInFlight.remove(id);
        if (path != null && path.isNotEmpty) notifyListeners();
      }).catchError((Object _) {
        _covers[id] = '';
        _coversInFlight.remove(id);
      });
    }
    return null;
  }

  @override
  void dispose() {
    _events?.cancel();
    super.dispose();
  }
}

/// A hue derived from the title, for books with no cover. Deterministic, so the
/// same book is the same colour every time it is drawn.
Color coverHue(String title) {
  var h = 0;
  for (final c in title.codeUnits) {
    h = (h * 31 + c) & 0x7fffffff;
  }
  const palette = [
    Color(0xFF6C4DF6),
    Color(0xFFE0518F),
    Color(0xFF2FBF71),
    Color(0xFFF5A623),
    Color(0xFF3A86FF),
    Color(0xFFB5179E),
  ];
  return palette[h % palette.length];
}

/// Bytes as the size a file browser would show.
String fmtSize(int bytes) {
  if (bytes <= 0) return '';
  const units = ['B', 'KB', 'MB', 'GB'];
  var v = bytes.toDouble();
  var u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return '${v.toStringAsFixed(u == 0 ? 0 : 1)} ${units[u]}';
}

/// Unix seconds as a short date, or '' for never.
String fmtDate(int secs) {
  if (secs <= 0) return '';
  final d = DateTime.fromMillisecondsSinceEpoch(secs * 1000);
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
  return '${d.day} ${months[d.month - 1]} ${d.year}';
}

String fmtReadTime(int secs) {
  if (secs <= 0) return '';
  if (secs < 3600) return '${(secs / 60).round()} min';
  return '${(secs / 3600).toStringAsFixed(1)} h';
}
