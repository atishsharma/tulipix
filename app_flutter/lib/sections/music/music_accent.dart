// The colour a track paints the room with.
//
// The Slint build hands the UI an `np-accent` the backend derived from the
// cover; here the image is already a file on disk that Flutter is about to
// decode anyway, so the answer is taken on this side. Decoding to 16x16 first
// keeps it honest about cost: the output is one colour, not a picture.

import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';

/// Remembered per path — a cover's dominant colour does not change, and this
/// is called on every track change.
final Map<String, Color> _cache = <String, Color>{};

/// The dominant colour of an image file, or null when there is no usable one.
///
/// Not a true dominant: it is a saturation-and-brightness-weighted mean, which
/// is what a wash behind glass actually wants. A real k-means would pick the
/// biggest region, and on most covers that is the black border.
Future<Color?> dominantColour(String path) async {
  if (path.isEmpty) return null;
  final hit = _cache[path];
  if (hit != null) return hit;
  final file = File(path);
  if (!await file.exists()) return null;

  ui.Image? small;
  try {
    final bytes = await file.readAsBytes();
    final codec = await ui.instantiateImageCodec(
      bytes,
      targetWidth: 16,
      targetHeight: 16,
    );
    small = (await codec.getNextFrame()).image;
    final data = await small.toByteData(format: ui.ImageByteFormat.rawRgba);
    if (data == null) return null;
    final px = data.buffer.asUint8List();

    var wr = 0.0, wg = 0.0, wb = 0.0, total = 0.0;
    for (var i = 0; i + 3 < px.length; i += 4) {
      final a = px[i + 3];
      if (a < 128) continue;
      final r = px[i] / 255.0, g = px[i + 1] / 255.0, b = px[i + 2] / 255.0;
      final maxc = [r, g, b].reduce((x, y) => x > y ? x : y);
      final minc = [r, g, b].reduce((x, y) => x < y ? x : y);
      final sat = maxc <= 0 ? 0.0 : (maxc - minc) / maxc;
      // Near-black and near-white pixels are the frame and the paper, not the
      // colour of the record. Weight them close to nothing.
      final lift = maxc * (1 - maxc) * 4;
      final w = (0.15 + sat) * (0.25 + lift);
      wr += r * w;
      wg += g * w;
      wb += b * w;
      total += w;
    }
    if (total <= 0) return null;

    var c = HSLColor.fromColor(Color.fromARGB(
      255,
      (wr / total * 255).round().clamp(0, 255),
      (wg / total * 255).round().clamp(0, 255),
      (wb / total * 255).round().clamp(0, 255),
    ));
    // A wash has to read against both themes: pull washed-out and near-black
    // averages back into a band that still looks like a colour.
    c = c
        .withSaturation(c.saturation.clamp(0.35, 0.95))
        .withLightness(c.lightness.clamp(0.42, 0.68));
    final out = c.toColor();
    _cache[path] = out;
    return out;
  } catch (_) {
    // A cover that will not decode is not an error worth a banner — the wash
    // just stays the section's own pink.
    return null;
  } finally {
    small?.dispose();
  }
}
