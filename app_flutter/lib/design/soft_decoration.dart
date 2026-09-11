// Soft shadows that fall inside a shape as well as outside it.
//
// Flutter's BoxShadow only casts outward, and the sunk wells that neumorphism,
// clay and the machined skins are built from need the shadow *inside* the
// edge. This is the one Decoration they share: a fill, the outer casts, then
// the inner ones clipped to the shape, and an optional hairline for the OLED
// tier, where no shadow shows against black.

import 'package:flutter/foundation.dart'
    show VoidCallback, immutable, listEquals;
import 'package:flutter/painting.dart';

@immutable
class SoftShadow {
  const SoftShadow(this.color, this.offset, this.blur);

  final Color color;
  final Offset offset;

  /// A CSS-style blur radius. 0 is a hard rim.
  final double blur;

  @override
  bool operator ==(Object other) =>
      other is SoftShadow &&
      other.color == color &&
      other.offset == offset &&
      other.blur == blur;

  @override
  int get hashCode => Object.hash(color, offset, blur);
}

@immutable
class SoftDecoration extends Decoration {
  const SoftDecoration({
    required this.color,
    this.gradient,
    this.radius = 16,
    this.outer = const [],
    this.inner = const [],
    this.border,
  });

  final Color color;

  /// Painted over [color] when set — a convex face, a brushed plate.
  final Gradient? gradient;

  /// Clamped to half the short side, so a big radius on a small box is a pill.
  final double radius;
  final List<SoftShadow> outer;
  final List<SoftShadow> inner;

  /// A 1px hairline just inside the edge.
  final Color? border;

  RRect _shape(Rect rect) => RRect.fromRectAndRadius(
        rect,
        Radius.circular(radius.clamp(0.0, rect.shortestSide / 2)),
      );

  @override
  bool hitTest(Size size, Offset position, {TextDirection? textDirection}) =>
      _shape(Offset.zero & size).contains(position);

  /// A Container that clips its child asks the decoration for the shape, and
  /// the base class throws. The casts are painted outside the clip, so they
  /// survive it.
  @override
  Path getClipPath(Rect rect, TextDirection textDirection) =>
      Path()..addRRect(_shape(rect));

  @override
  BoxPainter createBoxPainter([VoidCallback? onChanged]) => _SoftPainter(this);

  // Equality matters: AnimatedContainer compares decorations on every build,
  // and one built fresh each frame would otherwise restart its tween forever.
  @override
  bool operator ==(Object other) =>
      other is SoftDecoration &&
      other.color == color &&
      other.gradient == gradient &&
      other.radius == radius &&
      other.border == border &&
      listEquals(other.outer, outer) &&
      listEquals(other.inner, inner);

  @override
  int get hashCode => Object.hash(color, gradient, radius, border,
      Object.hashAll(outer), Object.hashAll(inner));
}

class _SoftPainter extends BoxPainter {
  _SoftPainter(this.d);

  final SoftDecoration d;

  static Paint _shadow(SoftShadow s) {
    final p = Paint()..color = s.color;
    if (s.blur > 0) {
      // The radius→sigma conversion BoxShadow uses, so a blur here matches
      // the same number in CSS and in a BoxShadow.
      p.maskFilter =
          MaskFilter.blur(BlurStyle.normal, s.blur * 0.57735 + 0.5);
    }
    return p;
  }

  @override
  void paint(Canvas canvas, Offset offset, ImageConfiguration configuration) {
    final size = configuration.size;
    if (size == null || size.isEmpty) return;
    final rect = offset & size;
    final shape = d._shape(rect);

    if (d.outer.isNotEmpty) {
      // Outside the shape only, as CSS draws a box-shadow. Under an opaque
      // fill nobody could tell; under 7% glass the cast would be a grey smear
      // across the pane's own face.
      canvas.save();
      canvas.clipPath(Path()
        ..fillType = PathFillType.evenOdd
        ..addRect(rect.inflate(200))
        ..addRRect(shape));
      for (final s in d.outer) {
        canvas.drawRRect(shape.shift(s.offset), _shadow(s));
      }
      canvas.restore();
    }

    final fill = Paint()..color = d.color;
    if (d.gradient != null) fill.shader = d.gradient!.createShader(rect);
    canvas.drawRRect(shape, fill);

    if (d.inner.isNotEmpty) {
      canvas.save();
      canvas.clipRRect(shape);
      for (final s in d.inner) {
        // A frame around the shape, moved by the offset: its (blurred) inner
        // edge is the shadow falling in from that side, and the clip keeps it
        // inside.
        final frame = Path()
          ..fillType = PathFillType.evenOdd
          ..addRect(rect.inflate(s.blur * 2 + s.offset.distance + 4))
          ..addRRect(shape.shift(s.offset));
        canvas.drawPath(frame, _shadow(s));
      }
      canvas.restore();
    }

    if (d.border != null) {
      canvas.drawRRect(
        shape.deflate(0.5),
        Paint()
          ..color = d.border!
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1,
      );
    }
  }
}
