// Live figures for Settings › Advanced › Performance, as the Slint build's
// PERFORMANCE (LIVE) block had them: how long this copy took to put its first
// frame up, how many frames since have missed 16 ms, and what the whole
// process holds in memory.
//
// Timed from `main()`, as Slint's READY_MS is timed from its own `main()`: the
// engine's start before that is not in the figure.

import 'dart:io' show ProcessInfo;

import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';

class Vitals {
  Vitals._();

  static final Stopwatch _clock = Stopwatch();

  /// Main to first frame, in ms. Null until that frame is on screen.
  static int? startupMs;

  /// Frames over 16 ms since launch.
  static int slowFrames = 0;

  /// The whole process: engine, Dart, Rust and libmpv.
  static int get residentMb => ProcessInfo.currentRss ~/ (1024 * 1024);

  /// First thing in `main()`, once the binding is up.
  static void start() {
    _clock.start();
    WidgetsBinding.instance.waitUntilFirstFrameRasterized.then((_) {
      startupMs = _clock.elapsedMilliseconds;
      _clock.stop();
    });
    SchedulerBinding.instance.addTimingsCallback((timings) {
      for (final t in timings) {
        if (t.totalSpan > const Duration(milliseconds: 16)) slowFrames++;
      }
    });
  }
}
