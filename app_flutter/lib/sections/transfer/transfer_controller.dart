// Transfer section state.
//
// Polled, not streamed. A progress bar is a sampled value — there is no event
// to subscribe to that says "a further 64 KB arrived" — so the bridge computes
// rates from the gap between two reads and this drives the clock. 600 ms is
// what ui/page_transfer.slint's timer uses, and the rate smoothing in Rust is
// tuned for that interval.
//
// The QR is fetched separately, only when `qrRev` moves. It is a 33x33 matrix,
// and carrying a kilobyte on every tick to say nothing new is the sort of thing
// that makes a poll look expensive when it is not.

import 'dart:async';

import 'package:flutter/foundation.dart';

import '../../design/pick.dart';
import '../../src/rust/api/transfer.dart';

/// How often the page re-reads the service while it is on screen. Fast enough
/// that a progress bar moves smoothly, slow enough to be free.
const _tick = Duration(milliseconds: 600);

/// The same, while sharing is on but the user is somewhere else in the app.
/// Nothing is on screen to repaint, but an upload can still land, and the
/// post-upload library rescan hangs off this tick.
const _idleTick = Duration(seconds: 3);

class TransferController extends ChangeNotifier {
  TransferState? state;
  Object? error;

  /// A command is in flight. Buttons that would queue a second one go flat.
  bool busy = false;

  QrCode? qr;
  int _qrRev = -1;

  Timer? _timer;
  Duration? _every;
  bool _visible = true;

  /// A tick is already waiting on the bridge. Without this a slow refresh — the
  /// first one after Start, which binds a socket and mints a certificate —
  /// stacks up behind itself.
  bool _polling = false;

  void start() {
    _retime();
    _poll();
  }

  /// On screen or not. The server is not stopped either way — sharing lasts as
  /// long as the app does, so a phone can keep pulling a 4 GB file while the
  /// desktop is used for something else — but a page nobody is looking at does
  /// not need repainting six times a second.
  void setVisible(bool visible) {
    if (_visible == visible) return;
    _visible = visible;
    _retime();
    if (visible) _poll();
  }

  void _retime() {
    // Not sharing and not on screen: nothing can change without a command, so
    // there is nothing to poll for.
    final want = _visible
        ? _tick
        : (state?.running ?? false)
            ? _idleTick
            : null;
    if (want == _every) return;
    _every = want;
    _timer?.cancel();
    _timer = want == null ? null : Timer.periodic(want, (_) => _poll());
  }

  @override
  void dispose() {
    _timer?.cancel();
    _timer = null;
    super.dispose();
  }

  Future<void> _poll() async {
    if (_polling || busy) return;
    _polling = true;
    try {
      final s = await transferDispatch(cmd: const TransferCmd.refresh());
      _apply(s);
    } catch (e) {
      error = e;
      notifyListeners();
    } finally {
      _polling = false;
    }
  }

  /// Send a command and take the page it returns.
  ///
  /// Every command answers with the whole state, including the ones that only
  /// opened a file chooser — the alternative is a UI that has to guess when to
  /// ask again.
  Future<void> send(TransferCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      _apply(await transferDispatch(cmd: cmd));
    } catch (e) {
      error = e;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  void _apply(TransferState s) {
    final wasRunning = state?.running ?? false;
    state = s;
    error = null;
    // Starting or stopping while off screen changes which clock applies.
    if (!_visible && wasRunning != s.running) _retime();
    if (s.qrRev != _qrRev) {
      _qrRev = s.qrRev;
      // Synchronous on the Rust side: it hands back a matrix it is already
      // holding, so this is a read and not a render.
      qr = s.qrRev == 0 ? null : transferQr();
    }
    notifyListeners();
  }

  // --- the commands the page sends, named ---

  Future<void> toggleSharing() => send((state?.running ?? false)
      ? const TransferCmd.stop()
      : const TransferCmd.start());

  /// The chooser is opened here rather than in the bridge, which is why these
  /// three are the only actions on this page that do anything before sending.
  /// A cancelled chooser sends nothing at all — the old `rfd` arms dispatched
  /// regardless and paid for a snapshot to show an unchanged tray.
  Future<void> addFiles() async {
    final paths = await pickFiles();
    if (paths.isEmpty) return;
    await send(TransferCmd.addFiles(paths: paths));
  }

  Future<void> addFolder() async {
    final path = await pickDirectory();
    if (path == null) return;
    await send(TransferCmd.addFolder(path: path));
  }
  Future<void> removeFile(int id) => send(TransferCmd.remove(id: id));
  Future<void> clearTray() => send(const TransferCmd.clear());
  Future<void> setShareTarget(String token) =>
      send(TransferCmd.setShareTarget(token: token));
  Future<void> openUrl() => send(const TransferCmd.openUrl());
  Future<void> forget(String token) => send(TransferCmd.forget(token: token));
  Future<void> renameDevice(String token, String name) =>
      send(TransferCmd.renameDevice(token: token, name: name));
  Future<void> setIface(String ip) => send(TransferCmd.setIface(ip: ip));
  Future<void> pickInbox() async {
    final path = await pickDirectory(initial: state?.inbox);
    if (path == null) return;
    await send(TransferCmd.setInbox(path: path));
  }
  Future<void> openInbox() => send(const TransferCmd.openInbox());
  Future<void> openRow(int rowId) => send(TransferCmd.openRow(rowId: rowId));
  Future<void> retryRow(int rowId) => send(TransferCmd.retryRow(rowId: rowId));
  Future<void> clearHistory() => send(const TransferCmd.clearHistory());
  Future<void> setPage(int page) => send(TransferCmd.setPage(page: page));
  Future<void> dismissUpload(int id) => send(TransferCmd.dismissUpload(id: id));

  /// A second click on the active column flips direction; a first click on a
  /// new one starts from the reading most people want — newest, biggest,
  /// A-first — rather than always ascending.
  Future<void> sortBy(int col) {
    final s = state;
    if (s != null && s.sort == col) {
      return send(TransferCmd.setSort(col: col, desc: !s.sortDesc));
    }
    return send(TransferCmd.setSort(col: col, desc: col == 1 || col == 5));
  }
}
