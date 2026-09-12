// Soft shadows that fall inside a shape as well as outside it.
//
// Flutter's BoxShadow only casts outward, and the sunk wells neumorphism is
// built from need the shadow *inside* the edge. This is the Decoration it and
// Glass's panes draw with: a fill, the outer casts, then the inner ones clipped
// to the shape, and an optional hairline for the OLED tier, where no shadow
// shows against black.
//
// Inner shades and blurred casts are not painted where they are drawn: each is
// rendered once into a small image and stretched to fit. See [_ShadowCache]
// for why.

import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart'
    show VoidCallback, immutable, listEquals, visibleForTesting;
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

  /// Paint every blurred shadow directly, on every frame, instead of from the
  /// cache. The reference the cache is tested against.
  @visibleForTesting
  static bool debugPaintDirect = false;

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

/// A shadow and the Paint that draws it.
typedef _Shadow = (SoftShadow, Paint);

class _SoftPainter extends BoxPainter {
  _SoftPainter(this.d)
      : _clip = !_opaque(d),
        _border = d.border == null
            ? null
            : (Paint()
              ..color = d.border!
              ..style = PaintingStyle.stroke
              ..strokeWidth = 1) {
    // A shadow in a transparent colour is dropped: blurring nothing still cost
    // the pass.
    for (final s in d.outer) {
      if (s.color.a == 0) continue;
      (s.blur > 0 ? _outerBlur : _outerHard).add((s, _shadow(s)));
    }
    for (final s in d.inner) {
      if (s.color.a != 0) _inner.add((s, _shadow(s)));
    }
  }

  final SoftDecoration d;

  /// Each shadow with its Paint, built once: a BoxPainter lives as long as its
  /// decoration, and a Paint and a MaskFilter per shadow per paint was garbage
  /// for every surface on every frame.
  ///
  /// A blurred cast goes through the cache; a hard one is the shape filled
  /// again, one draw, and stays where it is. Every inner shade goes through
  /// the cache, hard ones too: each was a clip and a path to tessellate, per
  /// surface, per frame — the sheen on every Glass pane, the rims of every
  /// Neumorphism surface on OLED. The cache lands on whole device pixels, so a 1px rim stays
  /// one row rather than softening across two.
  final List<_Shadow> _outerBlur = [];
  final List<_Shadow> _outerHard = [];
  final List<_Shadow> _inner = [];

  late final List<SoftShadow> _outerId = [for (final (s, _) in _outerBlur) s];
  late final List<SoftShadow> _innerId = [for (final (s, _) in _inner) s];
  late final double _outerReach = _reach(_outerBlur);
  late final double _innerReach = _reach(_inner);

  /// Whether the casts have to be cut out of the shape. Only a fill you can
  /// see through shows the cast behind it; an opaque one covers it anyway,
  /// and the clip is a path rather than a rect, so it costs a draw to set and
  /// another to drop, per surface, per frame.
  final bool _clip;
  final Paint? _border;

  /// A gradient is painted as the fill's shader, so it alone decides.
  static bool _opaque(SoftDecoration d) {
    final g = d.gradient;
    return g == null ? d.color.a >= 1 : g.colors.every((c) => c.a >= 1);
  }

  /// The radius→sigma conversion BoxShadow uses, so a blur here matches the
  /// same number in CSS and in a BoxShadow.
  static double _sigma(double blur) => blur * 0.57735 + 0.5;

  static Paint _shadow(SoftShadow s) {
    final p = Paint()..color = s.color;
    if (s.blur > 0) {
      p.maskFilter = MaskFilter.blur(BlurStyle.normal, _sigma(s.blur));
    }
    return p;
  }

  /// How far past the edge they fall from these shadows reach: three sigma of
  /// the blur, where a Gaussian is under a 255th of its peak, plus the offset.
  /// Whole pixels, and one to spare.
  static double _reach(List<_Shadow> shadows) {
    var reach = 0.0;
    for (final (s, _) in shadows) {
      reach = math.max(
        reach,
        3 * _sigma(s.blur) + math.max(s.offset.dx.abs(), s.offset.dy.abs()),
      );
    }
    return reach.ceilToDouble() + 1;
  }

  /// Outside the shape only, as CSS draws a box-shadow, where [clip] says the
  /// fill would show it. Under 7% glass the cast would be a grey smear across
  /// the pane's own face.
  static void _outside(Canvas canvas, RRect shape, List<_Shadow> casts,
      {required bool clip}) {
    if (clip) {
      canvas.save();
      canvas.clipPath(Path()
        ..fillType = PathFillType.evenOdd
        ..addRect(shape.outerRect.inflate(200))
        ..addRRect(shape));
    }
    for (final (s, p) in casts) {
      canvas.drawRRect(shape.shift(s.offset), p);
    }
    if (clip) canvas.restore();
  }

  static void _inside(Canvas canvas, RRect shape, List<_Shadow> shades) {
    canvas.save();
    canvas.clipRRect(shape);
    for (final (s, p) in shades) {
      // A frame around the shape, moved by the offset: its (blurred) inner
      // edge is the shadow falling in from that side, and the clip keeps it
      // inside.
      final frame = Path()
        ..fillType = PathFillType.evenOdd
        ..addRect(shape.outerRect.inflate(s.blur * 2 + s.offset.distance + 4))
        ..addRRect(shape.shift(s.offset));
      canvas.drawPath(frame, p);
    }
    canvas.restore();
  }

  @override
  void paint(Canvas canvas, Offset offset, ImageConfiguration configuration) {
    final size = configuration.size;
    if (size == null || size.isEmpty) return;
    final rect = offset & size;
    final shape = d._shape(rect);
    final dpr = configuration.devicePixelRatio ?? 1.0;
    final direct = SoftDecoration.debugPaintDirect;

    if (_outerBlur.isNotEmpty) {
      if (direct) {
        _outside(canvas, shape, _outerBlur, clip: _clip);
      } else {
        _ShadowCache.draw(canvas, shape, _outerBlur, _outerId,
            inner: false, clip: _clip, reach: _outerReach, dpr: dpr);
      }
    }
    if (_outerHard.isNotEmpty) _outside(canvas, shape, _outerHard, clip: _clip);

    final fill = Paint()..color = d.color;
    if (d.gradient != null) fill.shader = d.gradient!.createShader(rect);
    canvas.drawRRect(shape, fill);

    if (_inner.isNotEmpty) {
      if (direct) {
        _inside(canvas, shape, _inner);
      } else {
        _ShadowCache.draw(canvas, shape, _inner, _innerId,
            inner: true, clip: false, reach: _innerReach, dpr: dpr);
      }
    }

    final border = _border;
    if (border != null) canvas.drawRRect(shape.deflate(0.5), border);
  }
}

/// Inner shades and blurred casts, rendered once and stretched to fit.
///
/// Impeller — the Linux default — draws a blurred rounded rect analytically,
/// but a blurred path goes through an offscreen texture and a two-pass
/// Gaussian, and Impeller keeps no raster cache to hold on to the result.
/// Every inner shadow is such a path, so each one on screen cost that pass on
/// every frame; under Neumorphism every sunk well is one. A hard inner rim
/// skips the blur but not the path: a clip set and cleared and a frame
/// tessellated, per surface, per frame, on every Glass pane.
///
/// Past its corners, though, a rounded rect's shadow is the same all along
/// each edge. So a small image of the shadow — the shape just long enough
/// that the middle of each edge is out of both corners' reach — drawn in nine
/// slices is the same shadow at any size: the corners at the size they were
/// baked, one device pixel in the middle of each edge stretched across the
/// rest. One textured draw a frame, and the blur runs once per shape. An axis
/// too short to stretch is baked at its real length instead.
abstract final class _ShadowCache {
  /// Insertion-ordered, as every Dart map literal is: the first key is the
  /// least recently drawn.
  static final _images = <_ShadowKey, ui.Image>{};
  static int _bytes = 0;

  // ponytail: a fixed byte budget, dropping the least recently drawn. Each
  // image is a few hundred pixels a side at most, so this holds dozens of
  // shapes; raise it if a page cycles through more distinct ones than that.
  static const int _budget = 48 << 20;

  static final Paint _stretch = Paint()..filterQuality = FilterQuality.low;

  static void draw(
    Canvas canvas,
    RRect shape,
    List<_Shadow> shadows,
    List<SoftShadow> id, {
    required bool inner,
    required bool clip,
    required double reach,
    required double dpr,
  }) {
    final r = shape.tlRadiusX;
    final least = 2 * (r + reach) + 1;
    final w = shape.width >= least ? least : shape.width;
    final h = shape.height >= least ? least : shape.height;
    // Casts fall outside the shape, so their image is the shape plus their
    // reach; shades fall inside it, so theirs is the shape.
    final grow = inner ? 0.0 : reach;

    final key = _ShadowKey(inner, clip, id, r, dpr, w, h);
    var image = _images.remove(key);
    if (image == null) {
      image = _bake(shadows,
          inner: inner, clip: clip, w: w, h: h, r: r, grow: grow, dpr: dpr);
      _bytes += image.width * image.height * 4;
      while (_bytes > _budget && _images.isNotEmpty) {
        final old = _images.remove(_images.keys.first)!;
        _bytes -= old.width * old.height * 4;
        old.dispose();
      }
    }
    _images[key] = image;

    final dst = shape.outerRect.inflate(grow);
    final mid = Rect.fromLTWH(
      (image.width / 2).floorToDouble(),
      (image.height / 2).floorToDouble(),
      1,
      1,
    );
    // In whole device pixels, so the corners land at the size they were baked
    // and a hard 1px rim stays one row instead of softening across two.
    canvas
      ..save()
      ..scale(1 / dpr)
      ..drawImageNine(
          image,
          mid,
          Rect.fromLTRB(
            (dst.left * dpr).roundToDouble(),
            (dst.top * dpr).roundToDouble(),
            (dst.right * dpr).roundToDouble(),
            (dst.bottom * dpr).roundToDouble(),
          ),
          _stretch)
      ..restore();
  }

  /// Baked exactly as the painter would draw it — clip included, and only
  /// where the painter clips: under an opaque fill the cast is left whole, so
  /// the fill's own antialiased edge shows the same cast through it here as
  /// it did when painted directly.
  static ui.Image _bake(
    List<_Shadow> shadows, {
    required bool inner,
    required bool clip,
    required double w,
    required double h,
    required double r,
    required double grow,
    required double dpr,
  }) {
    final recorder = ui.PictureRecorder();
    final canvas = ui.Canvas(recorder)..scale(dpr);
    final shape = RRect.fromLTRBR(
        grow, grow, grow + w, grow + h, Radius.circular(r));
    if (inner) {
      _SoftPainter._inside(canvas, shape, shadows);
    } else {
      _SoftPainter._outside(canvas, shape, shadows, clip: clip);
    }
    final picture = recorder.endRecording();
    final image = picture.toImageSync(
      ((w + 2 * grow) * dpr).ceil(),
      ((h + 2 * grow) * dpr).ceil(),
    );
    picture.dispose();
    return image;
  }
}

@immutable
class _ShadowKey {
  const _ShadowKey(
      this.inner, this.clip, this.shadows, this.r, this.dpr, this.w, this.h);

  final bool inner;
  final bool clip;
  final List<SoftShadow> shadows;
  final double r;
  final double dpr;

  /// The shape as baked: its real length along an axis too short to stretch,
  /// the least that stretches otherwise.
  final double w;
  final double h;

  @override
  bool operator ==(Object other) =>
      other is _ShadowKey &&
      other.inner == inner &&
      other.clip == clip &&
      other.r == r &&
      other.dpr == dpr &&
      other.w == w &&
      other.h == h &&
      listEquals(other.shadows, shadows);

  @override
  int get hashCode =>
      Object.hash(inner, clip, r, dpr, w, h, Object.hashAll(shadows));
}
