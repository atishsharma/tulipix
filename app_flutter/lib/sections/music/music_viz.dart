// The audio visualizer, in six styles.
//
// What it draws is the same trick the Slint build plays: the *shape* is a
// deterministic function of elapsed time, and the *energy* is the real
// momentary loudness of the audio, read from mpv's ebur128 meter. So the bars
// move to the song rather than to a clock, without decoding a single sample of
// PCM in the UI process.
//
// Loudness is polled, not pushed. It changes many times a second, and putting
// it on the event stream would make it by far the loudest thing on it. It used
// to be an atomic in Rust behind a sync bridge symbol; now that the deck is
// media_kit in this process, the r128 meter is observed there and left in a
// plain variable, which the ticker here reads at ~11 fps.

import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart' show Ticker;

import '../../playback/audio_deck.dart' show audioLoudness;

const List<String> visStyleNames = [
  'Bars',
  'Mirror',
  'Dots',
  'Levels',
  'Line',
  'Spectrum',
];

const int _kBars = 24;

/// The idle shape — what is drawn when nothing is playing, so the strip reads
/// as a visualizer at rest rather than as an empty box.
const List<double> _idle = [
  0.12,
  0.2,
  0.16,
  0.26,
  0.18,
  0.3,
  0.22,
  0.34,
  0.24,
  0.3,
  0.2,
  0.26,
  0.16,
  0.22,
  0.14,
  0.2,
  0.12,
  0.18,
  0.14,
  0.1,
  0.16,
  0.22,
  0.14,
  0.1,
];

/// Deterministic animated spectrum keyed on elapsed time. A direct port of
/// `tulipix_music::visualizer::synthetic_bars` — the domain crate is frozen and
/// cannot be called from here for one twenty-line function.
List<double> syntheticBars(int n, double t) => List<double>.generate(n, (i) {
      final phase = i * 0.6;
      final a = math.sin(t * 3.1 + phase);
      final b = math.sin(t * 1.7 - phase * 0.5);
      return (0.5 + 0.30 * a + 0.18 * b).clamp(0.0, 1.0);
    });

/// A live visualizer. Owns its own ticker so the rest of the section is not
/// rebuilt eleven times a second on its behalf.
class VizView extends StatefulWidget {
  const VizView({
    super.key,
    required this.style,
    required this.playing,
    this.barWidth,
  });

  final int style;
  final bool playing;

  /// Fixed stroke width, for the styles that draw lines rather than columns.
  final double? barWidth;

  @override
  State<VizView> createState() => _VizViewState();
}

class _VizViewState extends State<VizView> with SingleTickerProviderStateMixin {
  late final Ticker _ticker;
  final Stopwatch _clock = Stopwatch()..start();
  Duration _lastFrame = Duration.zero;
  List<double> _bars = _idle;

  @override
  void initState() {
    super.initState();
    _ticker = createTicker(_onFrame)..start();
  }

  void _onFrame(Duration now) {
    // ~11 fps, matching the Slint timer. Anything faster is invisible on bars
    // this wide and costs a rebuild every frame.
    if (now - _lastFrame < const Duration(milliseconds: 90)) return;
    _lastFrame = now;
    if (!widget.playing) {
      if (!identical(_bars, _idle)) setState(() => _bars = _idle);
      return;
    }
    // 0.18 floor: at true silence the bars should still breathe, or a quiet
    // passage looks like playback stopped.
    final env = 0.18 + 0.82 * audioLoudness;
    final shaped = syntheticBars(_kBars, _clock.elapsedMicroseconds / 1e6)
        .map((b) => (b * env).clamp(0.0, 1.0))
        .toList();
    setState(() => _bars = shaped);
  }

  @override
  void dispose() {
    _ticker.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => CustomPaint(
        painter: _VizPainter(
          bars: _bars,
          style: widget.style,
          active: widget.playing,
          barWidth: widget.barWidth,
        ),
        size: Size.infinite,
      );
}

class _VizPainter extends CustomPainter {
  _VizPainter({
    required this.bars,
    required this.style,
    required this.active,
    this.barWidth,
  });

  final List<double> bars;
  final int style;
  final bool active;
  final double? barWidth;

  static const Color _lo = Color(0xFF8B5CF6);
  static const Color _hi = Color(0xFFEC4899);
  static const Color _top = Color(0xFFF43F5E);

  @override
  void paint(Canvas canvas, Size size) {
    if (size.width <= 0 || size.height <= 0 || bars.isEmpty) return;
    final n = bars.length;
    final gap = style == 4 ? 1.0 : (style == 5 ? 2.0 : 3.0);
    final slot = (size.width - gap * (n - 1)) / n;
    if (slot <= 0) return;
    final dim = active ? 1.0 : 0.5;
    final paint = Paint()..style = PaintingStyle.fill;

    for (var i = 0; i < n; i++) {
      final h = bars[i].clamp(0.0, 1.0);
      final left = i * (slot + gap);
      // Every style but the spectrum colours by level; the spectrum colours by
      // frequency band, so the wave reads as the song rather than the volume.
      final colour = style == 5
          ? _bandColour(n == 1 ? 0.5 : i / (n - 1))
          : Color.lerp(_lo, _hi, h)!;
      paint.color = colour.withValues(alpha: dim);

      switch (style) {
        case 0: // Bars — bottom-anchored columns.
          final bh = 4 + h * (size.height - 8);
          canvas.drawRRect(
            RRect.fromRectAndRadius(
              Rect.fromLTWH(left, size.height - bh, slot, bh),
              const Radius.circular(1.5),
            ),
            paint,
          );
        case 1: // Mirror — symmetric about the middle.
          final bh = 4 + h * (size.height - 8);
          canvas.drawRRect(
            RRect.fromRectAndRadius(
              Rect.fromLTWH(left, (size.height - bh) / 2, slot, bh),
              const Radius.circular(1.5),
            ),
            paint,
          );
        case 2: // Dots — a circle riding the band level.
          final d = math.min(slot, 8.0);
          canvas.drawCircle(
            Offset(left + slot / 2, (size.height - d) * (1 - h) + d / 2),
            d / 2,
            paint,
          );
        case 3: // Levels — a segmented LED column.
          const segs = [0.86, 0.72, 0.58, 0.44, 0.3, 0.16];
          final sh = (size.height - 10) / 6;
          for (var s = 0; s < segs.length; s++) {
            final lit = h >= segs[s];
            paint.color = lit
                ? Color.lerp(_lo, _hi, segs[s])!.withValues(alpha: dim)
                : const Color(0x22FFFFFF);
            canvas.drawRRect(
              RRect.fromRectAndRadius(
                Rect.fromLTWH(left, s * (sh + 2), slot, sh),
                const Radius.circular(1),
              ),
              paint,
            );
          }
        case 4: // Line — thin centred spikes.
        case 5: // Spectrum — the same, coloured by band.
          final w = barWidth ?? 2.0;
          final bh = style == 4
              ? 2 + h * (size.height - 6)
              : math.max(2.0, 2 + h * (size.height - 4));
          canvas.drawRRect(
            RRect.fromRectAndRadius(
              Rect.fromLTWH(
                  left + (slot - w) / 2, (size.height - bh) / 2, w, bh),
              const Radius.circular(1),
            ),
            paint,
          );
      }
    }
  }

  Color _bandColour(double frac) => frac < 0.5
      ? Color.lerp(_lo, _hi, frac * 2)!
      : Color.lerp(_hi, _top, (frac - 0.5) * 2)!;

  @override
  bool shouldRepaint(_VizPainter old) =>
      old.bars != bars || old.style != style || old.active != active;
}
