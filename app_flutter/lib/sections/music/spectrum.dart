// The precomputed spectrum of the track now playing.
//
// A live spectrum is not reachable on this audio path: media_kit exposes no
// PCM or sample callback, and mpv's filters return loudness metadata but not
// bands — which is why the visualiser has always drawn a shape derived from the
// clock and scaled by one loudness number. So the analysis pass decodes each
// track once and stores its spectrum beside the waveform, and this indexes that
// by playback position. Same result on screen, no live tap.
//
// Everything here degrades to null rather than throwing: a track the analysis
// pass has not reached has no spectrum, and the visualiser goes on drawing what
// it drew before.

import 'dart:typed_data';

import '../../src/rust/api/music.dart';

/// One track's stored spectrum.
class TrackSpectrum {
  TrackSpectrum(this.itemId, this.data, this.bands, this.hz);

  final int itemId;

  /// `bands` levels per column, row-major by column.
  final Uint8List data;
  final int bands;

  /// Columns per second.
  final int hz;

  int get columns => bands <= 0 ? 0 : data.length ~/ bands;

  /// Whether there is anything here to draw.
  bool get isUsable => bands > 0 && hz > 0 && columns > 0;

  /// [out] band levels, 0..1, at [seconds] — or null past the end of the track.
  ///
  /// Interpolates between the two nearest columns. The stored resolution is ten
  /// columns a second and the display runs at sixty, so without this a held note
  /// would visibly step; with it the bars move the way an analyser's do.
  List<double>? at(double seconds, int out) {
    if (!isUsable || out <= 0 || seconds < 0) return null;
    final exact = seconds * hz;
    final c0 = exact.floor();
    if (c0 < 0 || c0 >= columns) return null;
    final c1 = (c0 + 1).clamp(0, columns - 1);
    final t = exact - c0;

    final result = List<double>.filled(out, 0.0);
    for (var i = 0; i < out; i++) {
      // Each output bar averages the stored bands that fall inside it. The
      // stored bands are already log-spaced, so an even split over band index
      // keeps the spacing the analysis chose.
      final lo = (i * bands) ~/ out;
      final hi = (((i + 1) * bands) ~/ out).clamp(lo + 1, bands);
      var sum = 0.0;
      for (var b = lo; b < hi; b++) {
        final a = data[c0 * bands + b] / 255.0;
        final z = data[c1 * bands + b] / 255.0;
        sum += a + (z - a) * t;
      }
      result[i] = (sum / (hi - lo)).clamp(0.0, 1.0);
    }
    return result;
  }
}

/// The spectrum for whatever is playing, or null when there is none.
TrackSpectrum? nowSpectrum;

/// The track a load is in flight for, so a burst of track changes does not
/// start the same read twice.
int _loading = 0;

/// Fetch the stored spectrum for [itemId] and make it current.
///
/// Never decodes: the bridge only reads what the analysis pass already wrote,
/// so this is a file read or nothing. Call it on every track change — clearing
/// the old one first matters, or the new track is drawn against the old track's
/// spectrum for as long as the read takes.
Future<void> loadSpectrum(int itemId) async {
  if (nowSpectrum?.itemId == itemId || _loading == itemId) return;
  nowSpectrum = null;
  if (itemId == 0) return;
  _loading = itemId;
  try {
    final data = await musicSpectrogram(itemId: itemId);
    // The track may have changed again while this was in flight.
    if (_loading != itemId) return;
    if (data.isEmpty) return;
    final spec = TrackSpectrum(
      itemId,
      data,
      musicSpectrogramBands(),
      musicSpectrogramHz(),
    );
    if (spec.isUsable) nowSpectrum = spec;
  } catch (_) {
    // No spectrum is the normal state for an unanalysed library, not an error.
  } finally {
    if (_loading == itemId) _loading = 0;
  }
}
