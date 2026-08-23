// The Home section's state.
//
// One snapshot of every section at once. Refreshed on entry and after a
// dismissal, not on a timer: nothing on this page changes without something
// else in the app having caused it, and a landing page that re-queries ten
// databases on a clock is a battery graph.

import 'package:flutter/material.dart';

import '../../src/rust/api/home.dart';

/// The Continue strip's kind tabs, in the order the Slint page lists them.
const List<({String id, String label})> kContinueKinds = [
  (id: 'all', label: 'All'),
  (id: 'video', label: 'Videos'),
  (id: 'book', label: 'Books'),
  (id: 'podcast', label: 'Podcasts'),
  (id: 'audiobook', label: 'Audiobooks'),
];

class HomeController extends ChangeNotifier {
  HomeState? state;
  Object? error;
  bool busy = false;

  Future<void> send(HomeCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      state = await homeDispatch(cmd: cmd);
    } catch (e) {
      error = e;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  Future<void> refresh() => send(const HomeCmd.refresh());

  void clearError() {
    error = null;
    notifyListeners();
  }
}
