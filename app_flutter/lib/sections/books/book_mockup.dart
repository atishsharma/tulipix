// The "wrapped onto the book" cover, drawn rather than baked.
//
// The Slint build gets this from Rust: `covers::bake_hero` / `bake_tile` warp
// the cover art onto the mockup's face quad, multiply the mockup's own lighting
// back over it, and cache the result as a PNG. The bridge does not send those
// bakes, and the fallback the Slint file keeps for that case — a contain-fitted
// cover floating inside the face's bounding box — is visibly worse: it letterboxes
// the art and leaves the mockup showing through.
//
// So the warp happens here instead, on the GPU. The quads are the ones
// `covers.rs` measured (QUAD_TILE and QUAD_HERO, in their frames' own pixel
// space); the patch is bilinear, the same one `warp_onto` walks; and the frame
// is drawn back over the art with a multiply, which is what its `shade` argument
// does. A book with no art of its own gets a generated cover rendered offscreen
// and warped the same way, so the placeholder sits on the face rather than
// beside it.

import 'dart:io';
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show rootBundle;

import '../../src/rust/api/books.dart';
import 'books_controller.dart';

/// One hardcover mockup: its artwork, its size in that artwork's pixels, and
/// the front-face quad [TL, TR, BR, BL] the cover is warped onto.
@immutable
class Mockup {
  const Mockup({
    required this.asset,
    required this.frame,
    required this.quad,
    required this.darken,
  });

  final String asset;
  final Size frame;
  final List<Offset> quad;

  /// The dark-theme rendition is the same mockup pre-darkened, so no second
  /// asset ships — `bake_hero` scales the frame by this before compositing.
  final double darken;

  /// The upright hardcover the grid tiles use.
  static const tile = Mockup(
    asset: 'assets/bookhero/bookframe.png',
    frame: Size(600, 830),
    quad: [
      Offset(12.8, 26.5),
      Offset(511.4, 0),
      Offset(512.7, 829),
      Offset(12.2, 791)
    ],
    darken: 1,
  );

  /// The tilted hardcover the Continue card lays down beside its text.
  static const hero = Mockup(
    asset: 'assets/bookhero/herocover-frame.png',
    frame: Size(600, 600),
    quad: [
      Offset(81, 149.3),
      Offset(327.8, 74.3),
      Offset(578.3, 378),
      Offset(273, 480.8)
    ],
    darken: 0.52,
  );
}

/// A book on its mockup. Sizes itself to the box it is given, contain-fitting
/// the frame inside it exactly as the Slint element does.
class BookMockup extends StatefulWidget {
  const BookMockup({
    super.key,
    required this.controller,
    required this.book,
    required this.mockup,
    this.dark = false,
  });

  final BooksController controller;
  final Book book;
  final Mockup mockup;
  final bool dark;

  @override
  State<BookMockup> createState() => _BookMockupState();
}

class _BookMockupState extends State<BookMockup> {
  ui.Image? _frame;
  ui.Image? _face;
  String? _facePath;

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void didUpdateWidget(BookMockup old) {
    super.didUpdateWidget(old);
    if (old.mockup.asset != widget.mockup.asset ||
        old.book.id != widget.book.id) {
      _load();
    }
  }

  Future<void> _load() async {
    final frame = await _frameImage(widget.mockup.asset);
    if (!mounted) return;
    setState(() => _frame = frame);
    await _loadFace();
  }

  Future<void> _loadFace() async {
    final path = widget.controller.coverFor(widget.book);
    if (path == _facePath && _face != null) return;
    _facePath = path;
    final face = path == null
        ? await _generatedFace(widget.book)
        : await _fileImage(path);
    if (!mounted) return;
    setState(() => _face = face ?? _face);
  }

  @override
  Widget build(BuildContext context) {
    // The cover resolver answers on a later frame the first time; ask again on
    // every build until it does, the way BookCover does.
    if (_face == null || _facePath != widget.controller.coverFor(widget.book)) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _loadFace();
      });
    }
    final frame = _frame;
    if (frame == null) return const SizedBox.shrink();
    return CustomPaint(
      painter: _MockupPainter(
        frame: frame,
        face: _face,
        mockup: widget.mockup,
        dark: widget.dark,
      ),
      size: Size.infinite,
    );
  }
}

class _MockupPainter extends CustomPainter {
  _MockupPainter({
    required this.frame,
    required this.face,
    required this.mockup,
    required this.dark,
  });

  final ui.Image frame;
  final ui.Image? face;
  final Mockup mockup;
  final bool dark;

  /// Cells per side of the bilinear patch. Sixteen is past the point where the
  /// seams are visible and still 512 triangles.
  static const int _n = 16;

  @override
  void paint(Canvas canvas, Size size) {
    // Contain-fit the frame in the box, as `image-fit: contain` does.
    final ar = mockup.frame.width / mockup.frame.height;
    final w = size.width < size.height * ar ? size.width : size.height * ar;
    final h = w / ar;
    final ox = (size.width - w) / 2;
    final oy = (size.height - h) / 2;
    final scale = w / mockup.frame.width;
    final dst = Rect.fromLTWH(ox, oy, w, h);

    final framePaint = Paint()..filterQuality = FilterQuality.medium;
    if (dark) {
      // The same mockup pre-darkened, so the boards follow the dark theme.
      final k = mockup.darken;
      framePaint.colorFilter = ColorFilter.matrix(<double>[
        k, 0, 0, 0, 0, //
        0, k, 0, 0, 0, //
        0, 0, k, 0, 0, //
        0, 0, 0, 1, 0, //
      ]);
    }
    canvas.drawImageRect(
      frame,
      Rect.fromLTWH(0, 0, frame.width.toDouble(), frame.height.toDouble()),
      dst,
      framePaint,
    );

    final art = face;
    if (art == null) return;

    final quad = [
      for (final p in mockup.quad) Offset(ox + p.dx * scale, oy + p.dy * scale),
    ];

    // Everything from here is confined to the face, so the multiply below can
    // never darken the boards or the shadow around them.
    canvas.save();
    canvas.clipPath(Path()
      ..addPolygon(quad, true)
      ..close());

    canvas.drawVertices(
      _mesh(quad, art),
      BlendMode.src,
      Paint()
        ..filterQuality = FilterQuality.medium
        ..shader = ui.ImageShader(
          art,
          TileMode.clamp,
          TileMode.clamp,
          Matrix4.identity().storage,
          filterQuality: FilterQuality.medium,
        ),
    );

    // The mockup's own lighting, back over the art — `warp_onto`'s `shade`.
    canvas.drawImageRect(
      frame,
      Rect.fromLTWH(0, 0, frame.width.toDouble(), frame.height.toDouble()),
      dst,
      Paint()
        ..blendMode = BlendMode.multiply
        ..filterQuality = FilterQuality.medium
        // The face is a light grey, and multiplying by it raw costs the art
        // about four percent; Rust divides by 245 rather than 255 for the same
        // reason. This is that division.
        ..colorFilter = const ColorFilter.matrix(<double>[
          1.041, 0, 0, 0, 0, //
          0, 1.041, 0, 0, 0, //
          0, 0, 1.041, 0, 0, //
          0, 0, 0, 1, 0, //
        ]),
    );
    canvas.restore();
  }

  /// The bilinear patch, as two triangles per cell, with the art's own
  /// coordinates as texture coordinates.
  ui.Vertices _mesh(List<Offset> q, ui.Image art) {
    final positions = <Offset>[];
    final coords = <Offset>[];
    final aw = art.width.toDouble();
    final ah = art.height.toDouble();

    Offset at(double u, double v) => Offset(
          (1 - u) * (1 - v) * q[0].dx +
              u * (1 - v) * q[1].dx +
              u * v * q[2].dx +
              (1 - u) * v * q[3].dx,
          (1 - u) * (1 - v) * q[0].dy +
              u * (1 - v) * q[1].dy +
              u * v * q[2].dy +
              (1 - u) * v * q[3].dy,
        );

    for (var i = 0; i < _n; i++) {
      for (var j = 0; j < _n; j++) {
        final u0 = i / _n, u1 = (i + 1) / _n;
        final v0 = j / _n, v1 = (j + 1) / _n;
        final p00 = at(u0, v0), p10 = at(u1, v0);
        final p11 = at(u1, v1), p01 = at(u0, v1);
        final t00 = Offset(u0 * aw, v0 * ah);
        final t10 = Offset(u1 * aw, v0 * ah);
        final t11 = Offset(u1 * aw, v1 * ah);
        final t01 = Offset(u0 * aw, v1 * ah);
        positions.addAll([p00, p10, p11, p00, p11, p01]);
        coords.addAll([t00, t10, t11, t00, t11, t01]);
      }
    }
    return ui.Vertices(ui.VertexMode.triangles, positions,
        textureCoordinates: coords);
  }

  @override
  bool shouldRepaint(_MockupPainter old) =>
      old.frame != frame ||
      old.face != face ||
      old.dark != dark ||
      old.mockup.asset != mockup.asset;
}

// ── loading ─────────────────────────────────────────────────────────────────

final Map<String, ui.Image> _frames = <String, ui.Image>{};

Future<ui.Image?> _frameImage(String asset) async {
  final hit = _frames[asset];
  if (hit != null) return hit;
  try {
    final data = await rootBundle.load(asset);
    final img = await _decode(data.buffer.asUint8List());
    if (img != null) _frames[asset] = img;
    return img;
  } catch (_) {
    return null;
  }
}

/// Decoded covers, keyed by path. A shelf redraw must not re-decode a JPEG per
/// tile per frame.
final Map<String, ui.Image> _faces = <String, ui.Image>{};

Future<ui.Image?> _fileImage(String path) async {
  final hit = _faces[path];
  if (hit != null) return hit;
  try {
    final img = await _decode(await File(path).readAsBytes());
    if (img != null) {
      // A shelf is six tiles and a hero; a few dozen decoded covers is the
      // whole session's worth.
      if (_faces.length > 64) _faces.clear();
      _faces[path] = img;
    }
    return img;
  } catch (_) {
    return null;
  }
}

Future<ui.Image?> _decode(Uint8List bytes) async {
  try {
    final codec = await ui.instantiateImageCodec(bytes);
    final frame = await codec.getNextFrame();
    return frame.image;
  } catch (_) {
    return null;
  }
}

/// The drawn cover a book with no art keeps, rendered offscreen so it can be
/// warped onto the face like any other.
Future<ui.Image?> _generatedFace(Book book) async {
  const w = 420.0;
  const h = 630.0;
  final hue = coverHue(book.title);
  final recorder = ui.PictureRecorder();
  final canvas = Canvas(recorder, const Rect.fromLTWH(0, 0, w, h));

  canvas.drawRect(
    const Rect.fromLTWH(0, 0, w, h),
    Paint()
      ..shader = ui.Gradient.linear(
        Offset.zero,
        const Offset(w, h),
        [hue, Color.lerp(hue, Colors.black, 0.35)!],
      ),
  );
  // The inner hairline frame.
  canvas.drawRRect(
    RRect.fromRectAndRadius(
        const Rect.fromLTWH(22, 22, w - 44, h - 44), const Radius.circular(10)),
    Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 5
      ..color = const Color(0x55FFFFFF),
  );

  void text(String s, double top, double size, FontWeight weight,
      {bool italic = false, Color colour = Colors.white}) {
    if (s.isEmpty) return;
    final tp = TextPainter(
      text: TextSpan(
        text: s,
        style: TextStyle(
          fontFamily: 'serif',
          fontSize: size,
          height: 1.2,
          fontWeight: weight,
          fontStyle: italic ? FontStyle.italic : FontStyle.normal,
          color: colour,
        ),
      ),
      textAlign: TextAlign.center,
      textDirection: TextDirection.ltr,
      maxLines: 4,
      ellipsis: '…',
    )..layout(maxWidth: w - 100);
    tp.paint(canvas, Offset((w - tp.width) / 2, top));
  }

  if (book.format.isNotEmpty) {
    final tp = TextPainter(
      text: TextSpan(
        text: book.format.toUpperCase(),
        style: const TextStyle(
            fontSize: 20, fontWeight: FontWeight.w700, color: Colors.white),
      ),
      textDirection: TextDirection.ltr,
    )..layout();
    final pill = RRect.fromRectAndRadius(
        Rect.fromLTWH(44, 44, tp.width + 24, 34), const Radius.circular(9));
    canvas.drawRRect(pill, Paint()..color = const Color(0x33FFFFFF));
    tp.paint(canvas, const Offset(56, 50));
  }

  text(book.title, h * 0.30, 40, FontWeight.w700);
  canvas.drawRect(const Rect.fromLTWH(w / 2 - 46, h * 0.56, 92, 2),
      Paint()..color = const Color(0x77FFFFFF));
  text(book.author, h * 0.60, 28, FontWeight.w400,
      italic: true, colour: const Color(0xDDFFFFFF));

  try {
    return await recorder.endRecording().toImage(w.round(), h.round());
  } catch (_) {
    return null;
  }
}
