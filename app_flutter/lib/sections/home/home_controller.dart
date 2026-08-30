// The Home section's state.
//
// One snapshot of every section at once. Refreshed on entry and after a
// dismissal, not on a timer: nothing on this page changes without something
// else in the app having caused it, and a landing page that re-queries ten
// databases on a clock is a battery graph.

import 'package:flutter/material.dart';

import '../../src/rust/api/home.dart';

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
