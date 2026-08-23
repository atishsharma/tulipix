// The Genesis section's state.
//
// A sub-page of Books, like the reader: it takes the whole section while it is
// open. Everything slow here is a mirror round trip, so the bridge announces
// what it finished and this asks for the snapshot — search results, covers
// landing one at a time, a download's progress.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/genesis.dart';

/// The three filter menus, in the order the Slint page lists them.
const List<String> kGenesisFields = [
  'Any',
  'Title',
  'Author',
  'Series',
  'Publisher',
  'Year',
  'ISBN',
];

const List<String> kGenesisFormats = [
  'Any',
  'epub',
  'pdf',
  'mobi',
  'azw3',
  'djvu',
  'cbz',
];

const List<String> kGenesisLanguages = [
  'Any',
  'English',
  'Hindi',
  'German',
  'French',
  'Spanish',
  'Russian',
  'Italian',
];

/// How long "Added to library" and "Saved ✓" stay before they clear themselves.
const Duration kDoneLinger = Duration(seconds: 6);
const Duration kSavedLinger = Duration(seconds: 2);

class GenesisController extends ChangeNotifier {
  GenesisState? state;
  Object? error;
  bool busy = false;

  /// Live download progress, straight off the event stream. The strip patches
  /// itself from this rather than costing a snapshot ten times a second.
  ({double pct, String detail})? tick;

  StreamSubscription<GenesisEvent>? _events;
  Timer? _coalesce;
  Timer? _dismiss;

  GenesisController() {
    _events = genesisEvents().listen(_onEvent, onError: (Object e) {
      error = e;
      notifyListeners();
    });
  }

  void _onEvent(GenesisEvent event) {
    switch (event) {
      case GenesisEvent_Changed():
        // The cover pass lands a row at a time and a mirror probe narrates
        // itself, so several of these can arrive in a frame.
        _coalesce?.cancel();
        _coalesce = Timer(const Duration(milliseconds: 200), refresh);
      case GenesisEvent_Progress(:final pct, :final detail):
        // -1 is a line that carries only a detail — which mirror answered,
        // which hop it is on — and leaves the bar where it is.
        tick = (pct: pct < 0 ? (tick?.pct ?? 0) : pct, detail: detail);
        notifyListeners();
    }
  }

  @override
  void dispose() {
    _coalesce?.cancel();
    _dismiss?.cancel();
    _events?.cancel();
    super.dispose();
  }

  Future<void> send(GenesisCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      final next = await genesisDispatch(cmd: cmd);
      state = next;
      if (!next.busy) tick = null;
      _armDismiss(next);
    } catch (e) {
      error = e;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  /// The two confirmations that clear themselves. Timed here rather than in
  /// Rust: a timer that exists to make a strip fade is a view concern.
  void _armDismiss(GenesisState st) {
    _dismiss?.cancel();
    if (st.doneName.isEmpty && !st.settingsSaved) return;
    _dismiss = Timer(
      st.doneName.isNotEmpty ? kDoneLinger : kSavedLinger,
      () => send(const GenesisCmd.dismiss()),
    );
  }

  Future<void> refresh() => send(const GenesisCmd.refresh());

  void clearError() {
    error = null;
    notifyListeners();
  }
}
