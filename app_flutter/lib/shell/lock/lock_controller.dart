// When the app locks, and whether it is locked.
//
// The idle clock is the one `idle.rs` keeps for the Slint build, kept on this
// side: every pointer event and key press stamps `_lastInput`, and a
// one-second timer compares. Input notifies nobody — a pointer crossing the
// window is dozens of events a second — only a change of state does: the
// ten-second warning counting down, locking, unlocking.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../playback/video_layer.dart';
import '../../src/rust/api/lock.dart';

class LockController extends ChangeNotifier {
  static final LockController instance = LockController._();

  LockController._();

  /// Settings › Security, as last read. Null until the first read lands.
  LockConfig? config;

  bool locked = false;

  /// Seconds left while the warning shows; 0 the rest of the time.
  int warning = 0;

  /// How long the window dims and says so before it locks.
  static const int warnSecs = 10;

  DateTime _lastInput = DateTime.now();
  Timer? _timer;

  /// Read the settings again. Settings calls this after every change it
  /// sends, so a new timeout or PIN applies without a restart.
  Future<void> reload() async {
    try {
      config = await lockConfig();
    } catch (_) {
      return;
    }
    final on = config?.enabled ?? false;
    if (on) {
      _timer ??= Timer.periodic(const Duration(seconds: 1), (_) => _check());
    } else {
      _timer?.cancel();
      _timer = null;
      warning = 0;
    }
    notifyListeners();
  }

  /// Any pointer event or key press. Cheap on purpose: it is called for every
  /// one of them.
  void markActive() {
    _lastInput = DateTime.now();
    if (warning != 0) {
      warning = 0;
      notifyListeners();
    }
  }

  void _check() {
    final cfg = config;
    if (cfg == null || !cfg.enabled || locked) return;
    // A film playing is being watched, not left alone.
    if (videoPlaying.value) {
      _lastInput = DateTime.now();
      return;
    }
    final left = cfg.idleSecs - DateTime.now().difference(_lastInput).inSeconds;
    if (left <= 0) {
      lock();
      return;
    }
    final w = left <= warnSecs ? left : 0;
    if (w != warning) {
      warning = w;
      notifyListeners();
    }
  }

  /// Lock now — the idle clock running out, or Ctrl+L.
  void lock() {
    if (locked) return;
    locked = true;
    warning = 0;
    notifyListeners();
  }

  /// Check [pin] in Rust and unlock when it opens. The core compares the salted
  /// hash in constant time; nothing here ever holds the stored PIN.
  Future<bool> tryPin(String pin) async {
    final ok = await lockVerifyPin(pin: pin);
    if (ok) {
      unlock();
      // A PIN set before its length was kept has it recorded by this unlock;
      // read it back, so the next one opens on the last digit.
      if ((config?.pinLen ?? 0) == 0) reload();
    }
    return ok;
  }

  /// Ask for a fingerprint or a security-key touch, and unlock when it is
  /// given. Rust waits on the reader or the key; this is the one unlock that
  /// can take as long as the user takes.
  Future<bool> tryPasskey() async {
    final ok = await lockVerifyPasskey();
    if (ok) unlock();
    return ok;
  }

  void unlock() {
    if (!locked) return;
    locked = false;
    _lastInput = DateTime.now();
    notifyListeners();
  }
}
