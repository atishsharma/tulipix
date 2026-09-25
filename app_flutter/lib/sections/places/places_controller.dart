// The Places section's state.
//
// The bridge keeps the session — tab and the trip open — and works everything
// out from Photos, so this holds the last snapshot, the notice it left, and
// which year the trips are filtered to (a view choice, not worth a round trip).

import 'dart:async';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/places.dart';

export '../kitchen/kitchen_controller.dart' show plainError, plural;

typedef PlacesTab = ({String id, String label});

/// The header's tabs, in the order it draws them. Also Settings → Sections'
/// list of what can be switched off.
const List<PlacesTab> placesTabs = [
  (id: 'trips', label: 'Trips'),
  (id: 'map', label: 'Map'),
  (id: 'places', label: 'Places'),
  (id: 'been', label: 'Been'),
];

const Color kPlaces = Tokens.secPlaces;
const Color kPlaces2 = Tokens.secPlaces2;

class PlacesController extends ChangeNotifier {
  PlacesState? state;
  Object? error;
  bool busy = false;

  /// Trips shown for one year; 0 is all.
  int year = 0;

  String notice = '';
  Timer? _noticeTimer;

  Future<PlacesState?> send(PlacesCmd cmd, {bool quiet = false}) async {
    if (!quiet) {
      busy = true;
      error = null;
      notifyListeners();
    }
    try {
      final st = await placesDispatch(cmd: cmd);
      state = st;
      if (st.notice.isNotEmpty) say(st.notice);
      return st;
    } catch (e) {
      error = e;
      return null;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  Future<void> refresh({bool quiet = false}) =>
      send(const PlacesCmd.refresh(), quiet: quiet);

  void setYear(int y) {
    year = y;
    notifyListeners();
  }

  /// A line under the header that goes away on its own.
  void say(String text) {
    notice = text;
    _noticeTimer?.cancel();
    _noticeTimer = Timer(const Duration(seconds: 6), dismissNotice);
    notifyListeners();
  }

  void dismissNotice() {
    notice = '';
    notifyListeners();
  }

  void clearError() {
    error = null;
    notifyListeners();
  }

  @override
  void dispose() {
    _noticeTimer?.cancel();
    super.dispose();
  }
}
