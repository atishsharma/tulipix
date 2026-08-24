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
// plain variable, which the timer here reads at ~11 fps.

import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import '../../playback/audio_deck.dart' show audioLoudness;
import 'music_controller.dart';
import 'music_dialogs.dart';
import 'player_widgets.dart';

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

/// Rides the bass band, so a thing sitting on it moves with the music.
///
/// `y: ... - 5px * vis-bars[0]` in ui/page_music.slint, on both the note glyph
/// and the "Music" wordmark in the section header. Both use the same
/// expression, so they move together and one wrapper does for the pair.
///
/// It runs the same 90 ms timer the visualizer does, and for the same reason:
/// a Ticker would ask the engine for a frame at every vsync to move a title by
/// three pixels eleven times a second. The offset goes through a notifier so
/// the child is built once and only the transform is rebuilt.
class BeatBounce extends StatefulWidget {
  const BeatBounce({
    super.key,
    required this.playing,
    required this.child,
    this.travel = 5,
  });

  final bool playing;
  final Widget child;

  /// Peak displacement, upward.
  final double travel;

  @override
  State<BeatBounce> createState() => _BeatBounceState();
}

class _BeatBounceState extends State<BeatBounce> {
  final Stopwatch _clock = Stopwatch()..start();
  final ValueNotifier<double> _lift = ValueNotifier<double>(0);
  Timer? _timer;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _retime(TickerMode.valuesOf(context).enabled);
  }

  @override
  void didUpdateWidget(BeatBounce old) {
    super.didUpdateWidget(old);
    if (!widget.playing && _lift.value != 0) _lift.value = 0;
  }

  void _retime(bool on) {
    _timer?.cancel();
    _timer = on ? Timer.periodic(_kFrame, (_) => _onFrame()) : null;
  }

  void _onFrame() {
    if (!widget.playing) {
      if (_lift.value != 0) _lift.value = 0;
      return;
    }
    // Band 0 of the same shape the strip draws, scaled by the same envelope —
    // so the title and the bars are moving to one signal, not to two clocks
    // that drift apart.
    final env = 0.18 + 0.82 * audioLoudness;
    final t = _clock.elapsedMicroseconds / 1e6;
    _lift.value = (syntheticBars(1, t).first * env).clamp(0.0, 1.0);
  }

  @override
  void dispose() {
    _timer?.cancel();
    _lift.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => ValueListenableBuilder<double>(
        valueListenable: _lift,
        // Built once. Only the translation re-runs.
        child: widget.child,
        builder: (context, lift, child) => Transform.translate(
          offset: Offset(0, -widget.travel * lift),
          child: child,
        ),
      );
}

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

/// ~11 fps, matching the Slint timer. Anything faster is invisible on bars
/// this wide.
const Duration _kFrame = Duration(milliseconds: 90);

class _VizViewState extends State<VizView> {
  final Stopwatch _clock = Stopwatch()..start();

  /// The bars, as something the painter can subscribe to rather than something
  /// a rebuild carries down to it. This is the difference between eleven
  /// repaints a second and eleven full frames a second: `setState` marks the
  /// element dirty, and the pipeline that follows re-runs build, layout and
  /// semantics over a render tree that holds all ten sections. Handing the
  /// notifier to `CustomPainter.repaint` marks one RenderCustomPaint as needing
  /// paint and nothing else, so the frame does the work of the strip and no
  /// more.
  final ValueNotifier<List<double>> _bars = ValueNotifier<List<double>>(_idle);

  /// A Timer, not a Ticker, and that is the whole point of this widget's cost.
  /// A running Ticker asks the engine for a frame at *every* vsync for as long
  /// as it runs — the throttle that used to sit at the top of this callback
  /// dropped four frames in five, but the app had already built, laid out,
  /// painted and re-walked its semantics for all five. The bars want eleven
  /// frames a second; a Timer asks for exactly those eleven, and for none at
  /// all while the deck is paused.
  Timer? _timer;

  /// The cost of the Timer is that it does not know about [TickerMode], which
  /// is what silences this widget when Music is behind another section. Read it
  /// here instead: `didChangeDependencies` runs again whenever it flips.
  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _retime(TickerMode.valuesOf(context).enabled);
  }

  void _retime(bool on) {
    _timer?.cancel();
    _timer = on ? Timer.periodic(_kFrame, (_) => _onFrame()) : null;
  }

  void _onFrame() {
    if (!widget.playing) {
      // No setState once the bars are already at rest, so a paused deck asks
      // for no frames at all rather than eleven identical ones a second.
      if (!identical(_bars.value, _idle)) _bars.value = _idle;
      return;
    }
    // 0.18 floor: at true silence the bars should still breathe, or a quiet
    // passage looks like playback stopped.
    final env = 0.18 + 0.82 * audioLoudness;
    final shaped = syntheticBars(_kBars, _clock.elapsedMicroseconds / 1e6)
        .map((b) => (b * env).clamp(0.0, 1.0))
        .toList();
    _bars.value = shaped;
  }

  @override
  void dispose() {
    _timer?.cancel();
    _bars.dispose();
    super.dispose();
  }

  // RepaintBoundary because the surfaces this sits on -- the 124px glass bar,
  // the zen backdrop -- are the expensive things in the frame. Without it a
  // bar moving by two pixels repaints the blur behind it.
  @override
  Widget build(BuildContext context) => RepaintBoundary(
        child: CustomPaint(
          painter: _VizPainter(
            bars: _bars,
            style: widget.style,
            active: widget.playing,
            barWidth: widget.barWidth,
          ),
          size: Size.infinite,
        ),
      );
}

class _VizPainter extends CustomPainter {
  _VizPainter({
    required this.bars,
    required this.style,
    required this.active,
    this.barWidth,
  }) : super(repaint: bars);

  /// Listened to, not read once: `repaint` above is what lets a tick reach the
  /// canvas without a rebuild.
  final ValueListenable<List<double>> bars;
  final int style;
  final bool active;
  final double? barWidth;

  static const Color _lo = Color(0xFF8B5CF6);
  static const Color _hi = Color(0xFFEC4899);
  static const Color _top = Color(0xFFF43F5E);

  @override
  void paint(Canvas canvas, Size size) {
    final bars = this.bars.value;
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
  // Not the bars: those arrive through `repaint`. This is only for the
  // properties that come down from a rebuild.
  bool shouldRepaint(_VizPainter old) =>
      old.style != style || old.active != active || old.barWidth != barWidth;
}

/// Which of the six shapes the bars draw, and whether they are drawn at all.
///
/// It sat in the player bar beside a strip of bars that is no longer there;
/// this is Slint's `zvizpop`, which is where it has always belonged — the
/// selector goes with the thing it selects for.
class VizStyleButton extends StatelessWidget {
  const VizStyleButton({
    super.key,
    required this.controller,
    this.size = 38,
    this.iconSize = 18,
  });

  final MusicController controller;
  final double size;
  final double iconSize;

  @override
  Widget build(BuildContext context) => Builder(
        builder: (btn) => PlayerBtn(
          icon: Icons.graphic_eq,
          tip: 'Visualizer style',
          size: size,
          iconSize: iconSize,
          active: controller.visOn,
          accent: controller.accent,
          onTap: () async {
            // -1 is the Off row: picking a style turns it back on, which is
            // why the list would otherwise look dead while it is off.
            final v = await dropUp<int>(btn, items: [
              for (var i = 0; i < visStyleNames.length; i++)
                CheckedPopupMenuItem(
                  value: i,
                  checked: controller.visOn && controller.visStyle == i,
                  child: Text(visStyleNames[i]),
                ),
              const PopupMenuDivider(),
              CheckedPopupMenuItem(
                value: -1,
                checked: !controller.visOn,
                child: const Text('Off'),
              ),
            ]);
            if (v == null) return;
            v < 0 ? controller.setVisOn(false) : controller.setVisStyle(v);
          },
        ),
      );
}
