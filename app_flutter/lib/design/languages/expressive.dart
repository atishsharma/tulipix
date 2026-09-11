// Material 3 Expressive — "Bloom".
//
// Colour blooms from a seed into a tonal palette; shape carries the emphasis;
// icons fill when a control is on; the play button is a rounded square while
// playing and a circle at rest; the seek bar waves while music plays and lies
// flat when it stops. No shadows anywhere: depth is the surface-container tier.
//
// The pieces are small custom widgets rather than stock Material ones, which
// are not Expressive yet. The values are the mockup's
// (docs/mockups/music-expressive.html).

import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart' show Ticker;

import '../../playback/audio_deck.dart' show audioPositionS;
import '../../sections/music/music_controller.dart' show fmtClock;
import '../design_language.dart';
import '../skin.dart';
import '../tokens.dart';
import 'symbols_glyphs.dart';

class ExpressiveSkin extends AppSkin {
  ExpressiveSkin(Tokens t)
      : reduceMotion = t.reduceMotion,
        s = _scheme(t);

  final bool reduceMotion;
  final ColorScheme s;

  /// The Music pink as the seed, in the Expressive variant's wider hue spread.
  /// OLED keeps every role and takes the surfaces down to near-black.
  static ColorScheme _scheme(Tokens t) {
    final base = ColorScheme.fromSeed(
      seedColor: Tokens.secMusic,
      brightness: t.dark ? Brightness.dark : Brightness.light,
      dynamicSchemeVariant: DynamicSchemeVariant.expressive,
    );
    if (!(t.dark && t.oled)) return base;
    return base.copyWith(
      surface: const Color(0xFF000000),
      surfaceContainerLowest: const Color(0xFF000000),
      surfaceContainerLow: const Color(0xFF0C0709),
      surfaceContainer: const Color(0xFF130D10),
      surfaceContainerHigh: const Color(0xFF1B1418),
      surfaceContainerHighest: const Color(0xFF251C20),
    );
  }

  @override
  DesignLanguage get language => DesignLanguage.expressive;

  @override
  Color get canvas => s.surface;
  @override
  Color get ink => s.onSurface;
  @override
  Color get inkDim => s.onSurfaceVariant;
  @override
  Color get accent => s.primary;
  @override
  Color get accentSoft => s.primaryContainer;
  @override
  Color get activeInk => s.onSecondaryContainer;
  @override
  Color get onProminent => s.onPrimary;
  @override
  String get fontFamily => 'GoogleSansFlex';

  /// The tonal surfaces, tier by tier. No top-edge light: depth is the
  /// container tier, never a highlight.
  @override
  Tokens retint(Tokens base) => tokensFrom(
        base,
        page: s.surface,
        atmosphere: s.surfaceContainerLowest,
        panel: s.surfaceContainer,
        panel2: s.surfaceContainerHigh,
        modal: s.surfaceContainerHigh,
        ink: s.onSurface,
        inkDim: s.onSurfaceVariant,
        inkInv: s.surface,
        hair: s.outlineVariant,
        light: const Color(0x00000000),
        fill: s.onSurface.withValues(alpha: 0.05),
        fillStrong: s.onSurface.withValues(alpha: 0.10),
      );
  @override
  double get controlRadius => 20;
  @override
  double get panelRadius => 28;

  @override
  Decoration surface(SurfaceRole role, {double radius = 16}) => switch (role) {
        SurfaceRole.card => BoxDecoration(
            color: s.surfaceContainer,
            borderRadius: BorderRadius.circular(math.max(radius, 22)),
          ),
        SurfaceRole.art => BoxDecoration(
            color: s.surfaceContainerHighest,
            borderRadius: BorderRadius.circular(radius + 6),
          ),
        SurfaceRole.well => BoxDecoration(
            color: s.surfaceContainerHigh,
            borderRadius: BorderRadius.circular(radius),
          ),
        SurfaceRole.bar => BoxDecoration(
            color: s.surfaceContainerHigh,
            borderRadius: BorderRadius.circular(36),
          ),
      };

  @override
  Decoration control({
    required bool active,
    bool hovered = false,
    bool pressed = false,
    bool prominent = false,
    double radius = 18,
    Color? tint,
  }) {
    if (prominent) {
      // Playing: a rounded square. Paused: a circle. A press squares it more.
      final r = pressed ? radius * 0.45 : (active ? radius * 0.62 : radius);
      return BoxDecoration(
        color: s.primary,
        borderRadius: BorderRadius.circular(r),
      );
    }
    if (active) {
      // Selected goes fully round.
      return BoxDecoration(
        color: s.secondaryContainer,
        borderRadius: BorderRadius.circular(radius),
      );
    }
    final fill = hovered || pressed
        ? Color.alphaBlend(
            s.onSurface.withValues(alpha: 0.08), s.surfaceContainerHighest)
        : s.surfaceContainerHighest;
    return BoxDecoration(
      color: fill,
      borderRadius:
          BorderRadius.circular(pressed ? radius * 0.5 : radius * 0.55),
    );
  }

  @override
  Widget seekBar(SeekSlot slot) => WavySeek(
        slot: slot,
        color: s.primary,
        track: s.primaryContainer,
        ink: s.onSurfaceVariant,
        still: reduceMotion,
      );

  @override
  IconData icon(IconData material) => roundedSymbols[material] ?? material;
}

/// The wavy seek bar: the played part a sine wave that drifts while playing and
/// relaxes to a line on pause, a bar handle, a gap, the rest of the track and a
/// stop dot, between the two clocks. Drag or tap to seek; a drag commits on
/// release, as the seek pill's does.
class WavySeek extends StatefulWidget {
  const WavySeek({
    super.key,
    required this.slot,
    required this.color,
    required this.track,
    this.ink,
    this.still = false,
  });

  static const Key paintKey = ValueKey('wavy-seek-paint');

  final SeekSlot slot;
  final Color color;
  final Color track;

  /// The clocks. Null: the ambient text colour.
  final Color? ink;

  /// Reduce motion: no drift, and the amplitude changes at once.
  final bool still;

  @override
  State<WavySeek> createState() => _WavySeekState();
}

class _WavySeekState extends State<WavySeek>
    with SingleTickerProviderStateMixin {
  late final Ticker _ticker = createTicker((e) {
    setState(() => _phase = e.inMicroseconds / 1e6 * 2 * math.pi);
  });
  double _phase = 0;
  double? _drag;

  @override
  void initState() {
    super.initState();
    _sync();
  }

  @override
  void didUpdateWidget(WavySeek old) {
    super.didUpdateWidget(old);
    _sync();
  }

  void _sync() {
    final run = widget.slot.playing && !widget.still;
    if (run && !_ticker.isActive) _ticker.start();
    if (!run && _ticker.isActive) _ticker.stop();
  }

  @override
  void dispose() {
    _ticker.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final dur = widget.slot.dur <= 0 ? 1.0 : widget.slot.dur;
    // The snapshot's position moves once a second, which lurches. While the
    // wave drifts it repaints every frame anyway, so it reads the deck's own
    // position — the source the seek pill's `smooth` mode uses.
    final pos = _drag ??
        (widget.slot.playing && _ticker.isActive
            ? audioPositionS
            : widget.slot.pos);
    final frac = (pos / dur).clamp(0.0, 1.0);
    final clock = TextStyle(
      fontSize: 11,
      fontWeight: FontWeight.w600,
      color: widget.ink,
      fontFeatures: const [FontFeature.tabularFigures()],
    );
    return Row(
      children: [
        Text(fmtClock(pos.clamp(0.0, dur)), style: clock),
        const SizedBox(width: 10),
        Expanded(
          child: LayoutBuilder(
            builder: (context, box) {
              double at(double dx) => (dx / box.maxWidth).clamp(0.0, 1.0) * dur;
              return GestureDetector(
                behavior: HitTestBehavior.opaque,
                onHorizontalDragUpdate: (d) =>
                    setState(() => _drag = at(d.localPosition.dx)),
                onHorizontalDragEnd: (_) {
                  final v = _drag;
                  setState(() => _drag = null);
                  if (v != null) widget.slot.onSeek(v);
                },
                onTapDown: (d) => widget.slot.onSeek(at(d.localPosition.dx)),
                child: TweenAnimationBuilder<double>(
                  tween: Tween(end: widget.slot.playing ? 4.5 : 0),
                  duration: widget.still
                      ? Duration.zero
                      : const Duration(milliseconds: 600),
                  curve: Curves.easeOutBack,
                  builder: (context, amp, _) => CustomPaint(
                    key: WavySeek.paintKey,
                    size: Size(box.maxWidth, 30),
                    painter: WavePainter(
                      frac: frac,
                      amplitude: amp,
                      phase: _phase,
                      color: widget.color,
                      track: widget.track,
                    ),
                  ),
                ),
              );
            },
          ),
        ),
        const SizedBox(width: 10),
        Text(fmtClock(widget.slot.dur), style: clock),
      ],
    );
  }
}

class WavePainter extends CustomPainter {
  WavePainter({
    required this.frac,
    required this.amplitude,
    required this.phase,
    required this.color,
    required this.track,
  });

  final double frac;
  final double amplitude;
  final double phase;
  final Color color;
  final Color track;

  @override
  void paint(Canvas canvas, Size size) {
    final mid = size.height / 2;
    final x = size.width * frac;
    final stroke = Paint()
      ..color = color
      ..strokeWidth = 4
      ..strokeCap = StrokeCap.round
      ..style = PaintingStyle.stroke;
    // The played part: a 20px-period sine, drifting with the phase.
    final wave = Path()..moveTo(0, mid);
    for (var px = 0.0; px <= x - 8; px += 1) {
      wave.lineTo(
          px, mid + amplitude * math.sin(px / 20 * 2 * math.pi - phase));
    }
    canvas.drawPath(wave, stroke);
    // The handle, then a gap, then the rest of the track and its stop dot.
    canvas.drawRRect(
      RRect.fromRectAndRadius(
        Rect.fromCenter(
            center: Offset(x, mid), width: 4, height: size.height),
        const Radius.circular(2),
      ),
      Paint()..color = color,
    );
    if (x + 8 < size.width) {
      canvas.drawRRect(
        RRect.fromLTRBR(
            x + 8, mid - 2, size.width, mid + 2, const Radius.circular(2)),
        Paint()..color = track,
      );
      canvas.drawCircle(Offset(size.width - 2, mid), 2, Paint()..color = color);
    }
  }

  @override
  bool shouldRepaint(WavePainter o) =>
      o.frac != frac ||
      o.amplitude != amplitude ||
      o.phase != phase ||
      o.color != color ||
      o.track != track;
}
