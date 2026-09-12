// Durations as a clock reads them.
//
// Here rather than in Music, which had it first: the design kit's own seek bar
// shows a clock too, and lib/design does not reach into a section. Music
// re-exports it, so its callers did not move.

/// Seconds as m:ss, or h:mm:ss once there is an hour. Used by every duration
/// the app shows.
String fmtClock(double secs) {
  if (secs.isNaN || secs.isInfinite || secs <= 0) return '0:00';
  final s = secs.round();
  final m = (s ~/ 60) % 60;
  final h = s ~/ 3600;
  final ss = (s % 60).toString().padLeft(2, '0');
  if (h > 0) return '$h:${m.toString().padLeft(2, '0')}:$ss';
  return '$m:$ss';
}
