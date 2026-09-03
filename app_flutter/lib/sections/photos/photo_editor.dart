// The non-destructive editor.
//
// Nothing is computed here. Every control sends its value to the bridge and
// draws the preview that comes back, because the nine adjustment curves, the
// seven filter presets and the render order all already exist in
// tulipix-photos -- and a second implementation in Dart would have to agree
// with that one forever, silently, with no compiler to notice when it stopped.
//
// The cost of that choice is that a preview costs a round trip, so sliders
// commit on release rather than on every frame. The number beside the handle
// moves live; the picture catches up when you let go.
//
// Geometry is the one place Dart does arithmetic: a drag lands in widget
// coordinates and the bridge wants source pixels. Both crop and red-eye are
// measured against the image *as displayed* -- which is the current crop if
// there is one, because `ops::apply` runs the crop before either of them.

import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/editor.dart';
import 'photos_controller.dart';

Future<void> openPhotoEditor(
  BuildContext context,
  PhotosController controller,
  int itemId,
) {
  return Navigator.of(context).push(
    MaterialPageRoute<void>(
      builder: (_) => _Editor(controller: controller, itemId: itemId),
    ),
  );
}

/// The seven presets `editor::filters::Preset` defines, with the names the
/// bridge parses. Order matches the enum.
const _filters = <(String, String)>[
  ('bw', 'B&W'),
  ('sepia', 'Sepia'),
  ('vintage', 'Vintage'),
  ('drama', 'Drama'),
  ('hdr', 'HDR'),
  ('polaroid', 'Polaroid'),
  ('faded', 'Faded'),
];

/// `crop::AspectPreset`, in enum order. `free` clears the crop.
const _aspects = <(String, String)>[
  ('free', 'Free'),
  ('square', '1:1'),
  ('3x2', '3:2'),
  ('4x3', '4:3'),
  ('16x9', '16:9'),
  ('9x16', '9:16'),
  ('5x4', '5:4'),
];

/// What the pointer does over the preview. Only one at a time: a drag cannot
/// mean both "move the crop box" and "put an eye here".
enum _Tool { none, crop, redEye }

/// True for 90 and 270 -- the rotations that swap the frame's width and
/// height, and so the frame a crop rectangle is measured against.
bool _quarterTurned(double deg) => ((deg % 180) - 90).abs() < 45;

/// Size, in source pixels, of the image the preview is showing. The crop op
/// runs before red-eye and before any further crop, so everything downstream
/// is measured against its output and not against the original frame.
Size _displayedSize(EditorState s) {
  final crop = s.crop;
  if (crop != null && crop.w > 0 && crop.h > 0) {
    return Size(crop.w.toDouble(), crop.h.toDouble());
  }
  final turned = _quarterTurned(s.rotateDeg);
  final w = (turned ? s.height : s.width).toDouble();
  final h = (turned ? s.width : s.height).toDouble();
  // A photo_meta row written before the EXIF pass has zeros; a zero here
  // would make every fraction infinite.
  return Size(w > 0 ? w : 1, h > 0 ? h : 1);
}

/// Where a `BoxFit.contain` image of this aspect lands inside `box`.
Rect _fitRect(Size box, double aspect) {
  final w = math.min(box.width, box.height * aspect);
  final h = w / aspect;
  return Rect.fromLTWH((box.width - w) / 2, (box.height - h) / 2, w, h);
}

class _Editor extends StatefulWidget {
  const _Editor({required this.controller, required this.itemId});

  final PhotosController controller;
  final int itemId;

  @override
  State<_Editor> createState() => _EditorState();
}

class _EditorState extends State<_Editor> {
  EditorState? _s;
  Object? _error;
  bool _busy = true;

  _Tool _tool = _Tool.none;

  /// The crop box being dragged, as a fraction of the displayed image. Null
  /// until the first drag; committing it clears it again.
  Rect? _draft;

  /// Red-eye radius as a fraction of the displayed image's width, so the same
  /// setting means the same thing on a 6 MP and a 60 MP photo.
  double _eyeRadius = 0.02;

  String _curveChannel = 'all';

  @override
  void initState() {
    super.initState();
    _send(EditCmd.open(itemId: widget.itemId));
  }

  @override
  void dispose() {
    // Frees the decoded 1400px source and deletes the last preview file.
    photosEditClose();
    super.dispose();
  }

  Future<void> _send(EditCmd cmd) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final s = await photosEdit(cmd: cmd);
      if (!mounted) return;
      setState(() => _s = s);
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = e);
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  Adjust get _adjust => _s?.adjust ?? _zeroAdjust;

  static const _zeroAdjust = Adjust(
    exposure: 0,
    contrast: 0,
    saturation: 0,
    temperature: 0,
    tint: 0,
    highlights: 0,
    shadows: 0,
    blacks: 0,
    whites: 0,
  );

  List<CurvePoint> _curve(String channel) {
    final s = _s;
    if (s == null) return const [];
    return switch (channel) {
      'r' => s.curveR,
      'g' => s.curveG,
      'b' => s.curveB,
      _ => s.curveAll,
    };
  }

  /// A dragged box, in fractions of what is on screen, becomes source pixels
  /// relative to the original frame -- so a crop inside a crop composes
  /// instead of jumping back to the top-left of the photo.
  void _applyDraft() {
    final s = _s;
    final d = _draft;
    if (s == null || d == null) return;
    final size = _displayedSize(s);
    final ox = s.crop?.x ?? 0;
    final oy = s.crop?.y ?? 0;
    setState(() => _draft = null);
    _send(
      EditCmd.setCrop(
        rect: CropRect(
          x: ox + (d.left * size.width).round(),
          y: oy + (d.top * size.height).round(),
          w: math.max(1, (d.width * size.width).round()),
          h: math.max(1, (d.height * size.height).round()),
        ),
      ),
    );
  }

  void _placeEye(Offset frac) {
    final s = _s;
    if (s == null) return;
    final size = _displayedSize(s);
    _send(
      EditCmd.addRedEye(
        spot: RedEyeSpot(
          cx: (frac.dx * size.width).round(),
          cy: (frac.dy * size.height).round(),
          radius: math.max(2, (_eyeRadius * size.width).round()),
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = _s;
    return Scaffold(
      backgroundColor: t.bg,
      appBar: AppBar(
        backgroundColor: t.nCanvas,
        titleSpacing: 8,
        title: Row(
          children: [
            Text(
              'Edit',
              style: TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: 18,
                fontWeight: FontWeight.w600,
                color: t.nInk,
              ),
            ),
            if (s?.dirty ?? false) ...[
              const SizedBox(width: 10),
              Container(
                width: 7,
                height: 7,
                decoration: const BoxDecoration(
                  color: Tokens.secPhotos,
                  shape: BoxShape.circle,
                ),
              ),
              const SizedBox(width: 6),
              Text(
                'unsaved',
                style: TextStyle(fontSize: 12, color: t.nInk2),
              ),
            ],
          ],
        ),
        actions: [
          IconButton(
            tooltip: 'Undo',
            onPressed: (s?.canUndo ?? false) && !_busy
                ? () => _send(const EditCmd.undo())
                : null,
            icon: const Icon(Icons.undo),
          ),
          IconButton(
            tooltip: 'Redo',
            onPressed: (s?.canRedo ?? false) && !_busy
                ? () => _send(const EditCmd.redo())
                : null,
            icon: const Icon(Icons.redo),
          ),
          IconButton(
            tooltip: 'Back to the original',
            onPressed: _busy ? null : () => _send(const EditCmd.reset()),
            icon: const Icon(Icons.restart_alt),
          ),
          const SizedBox(width: 8),
          TextButton.icon(
            onPressed: _busy ? null : () => _promptExport(),
            icon: const Icon(Icons.ios_share, size: 18),
            label: const Text('Export'),
          ),
          const SizedBox(width: 8),
          FilledButton(
            onPressed: _busy ? null : () => _save(),
            child: const Text('Save'),
          ),
          const SizedBox(width: 12),
        ],
      ),
      body: _error != null
          ? _EditorError(
              error: _error!, onBack: () => Navigator.of(context).pop())
          : Row(
              children: [
                Expanded(
                  child: _Preview(
                    state: s,
                    busy: _busy,
                    tool: _tool,
                    draft: _draft,
                    onDraft: (r) => setState(() => _draft = r),
                    onTapEye: _placeEye,
                  ),
                ),
                SizedBox(
                  width: 340,
                  child: _Panel(
                    state: s,
                    busy: _busy,
                    adjust: _adjust,
                    tool: _tool,
                    draft: _draft,
                    eyeRadius: _eyeRadius,
                    curveChannel: _curveChannel,
                    curve: _curve(_curveChannel),
                    onAdjustCommit: (a) => _send(EditCmd.setAdjust(adjust: a)),
                    onFilter: (preset, strength) => _send(
                      EditCmd.setFilter(preset: preset, strength: strength),
                    ),
                    onSharpen: (v) => _send(EditCmd.setSharpen(amount: v)),
                    onEnhance: (v) => _send(EditCmd.setEnhance(enabled: v)),
                    onRotate: (q) => _send(EditCmd.rotate(quarterTurns: q)),
                    onFlip: (h) => _send(EditCmd.flip(horizontal: h)),
                    onTool: (tool) => setState(() {
                      _tool = _tool == tool ? _Tool.none : tool;
                      _draft = null;
                    }),
                    onAspect: (p) => _send(EditCmd.setAspect(preset: p)),
                    onApplyCrop: _applyDraft,
                    onCancelCrop: () => setState(() => _draft = null),
                    onClearCrop: () => _send(
                      const EditCmd.setCrop(
                        rect: CropRect(x: 0, y: 0, w: 0, h: 0),
                      ),
                    ),
                    onEyeRadius: (v) => setState(() => _eyeRadius = v),
                    onClearEyes: () => _send(const EditCmd.clearRedEye()),
                    onCurveChannel: (c) => setState(() => _curveChannel = c),
                    onCurve: (pts) => _send(
                      EditCmd.setCurve(channel: _curveChannel, points: pts),
                    ),
                    onRevert: _confirmRevert,
                  ),
                ),
              ],
            ),
    );
  }

  Future<void> _save() async {
    await _send(const EditCmd.save());
    if (!mounted) return;
    // The grid shows the original, not the render, so nothing about the tile
    // changes -- but the edited flag and any future thumbnail do.
    await widget.controller.refresh();
    if (!mounted) return;
    ScaffoldMessenger.of(context)
        .showSnackBar(const SnackBar(content: Text('Edit saved')));
  }

  Future<void> _confirmRevert() async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Discard all edits?'),
        content: const Text(
          'The photo goes back to how it was scanned, and the saved edit is '
          'deleted. The original file on disk was never modified.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('Discard'),
          ),
        ],
      ),
    );
    if (ok ?? false) await _send(const EditCmd.revert());
  }

  Future<void> _promptExport() async {
    final s = _s;
    if (s == null) return;
    final src = File(s.source);
    final dir = TextEditingController(text: src.parent.path);
    final stem = TextEditingController(
      text: '${src.uri.pathSegments.last.split('.').first}-edited',
    );
    var format = 'jpeg';
    var quality = 92.0;
    var keepExif = true;

    final go = await showDialog<bool>(
      context: context,
      builder: (context) => StatefulBuilder(
        builder: (context, setLocal) => AlertDialog(
          title: const Text('Export a copy'),
          content: SizedBox(
            width: 420,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                TextField(
                  controller: dir,
                  decoration: const InputDecoration(labelText: 'Folder'),
                ),
                TextField(
                  controller: stem,
                  decoration: const InputDecoration(labelText: 'File name'),
                ),
                const SizedBox(height: 14),
                Row(
                  children: [
                    const Text('Format'),
                    const SizedBox(width: 12),
                    DropdownButton<String>(
                      value: format,
                      onChanged: (v) => setLocal(() => format = v ?? 'jpeg'),
                      items: const [
                        DropdownMenuItem(value: 'jpeg', child: Text('JPEG')),
                        DropdownMenuItem(value: 'png', child: Text('PNG')),
                        DropdownMenuItem(value: 'webp', child: Text('WebP')),
                        DropdownMenuItem(value: 'tiff', child: Text('TIFF')),
                        DropdownMenuItem(value: 'heic', child: Text('HEIC')),
                      ],
                    ),
                  ],
                ),
                if (format == 'jpeg' || format == 'heic')
                  Row(
                    children: [
                      const Text('Quality'),
                      Expanded(
                        child: Slider(
                          value: quality,
                          min: 1,
                          max: 100,
                          divisions: 99,
                          label: quality.round().toString(),
                          onChanged: (v) => setLocal(() => quality = v),
                        ),
                      ),
                    ],
                  ),
                SwitchListTile(
                  contentPadding: EdgeInsets.zero,
                  value: keepExif,
                  onChanged: (v) => setLocal(() => keepExif = v),
                  title: const Text('Keep EXIF'),
                  subtitle: const Text('Off strips camera and GPS metadata'),
                ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(context).pop(false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: () => Navigator.of(context).pop(true),
              child: const Text('Export'),
            ),
          ],
        ),
      ),
    );

    final outDir = dir.text.trim();
    final outStem = stem.text.trim();
    dir.dispose();
    stem.dispose();
    if (!(go ?? false) || outDir.isEmpty || outStem.isEmpty) return;

    await _send(
      EditCmd.saveCopy(
        format: format,
        quality: quality.round(),
        keepExif: keepExif,
        outDir: outDir,
        stem: outStem,
      ),
    );
    if (!mounted) return;
    final written = _s?.lastExport ?? '';
    if (written.isNotEmpty) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('Wrote $written')));
    }
  }
}

/// The picture, plus whichever overlay the active tool needs. The image is
/// laid out `contain`, so the overlay has to reproduce that rectangle to turn
/// a pointer position into a fraction of the photo.
class _Preview extends StatelessWidget {
  const _Preview({
    required this.state,
    required this.busy,
    required this.tool,
    required this.draft,
    required this.onDraft,
    required this.onTapEye,
  });

  final EditorState? state;
  final bool busy;
  final _Tool tool;
  final Rect? draft;
  final void Function(Rect?) onDraft;
  final void Function(Offset frac) onTapEye;

  @override
  Widget build(BuildContext context) {
    final s = state;
    return ColoredBox(
      color: const Color(0xFF111114),
      child: Stack(
        fit: StackFit.expand,
        children: [
          if (s != null && s.preview.isNotEmpty)
            Padding(
              padding: const EdgeInsets.all(24),
              child: LayoutBuilder(
                builder: (context, box) {
                  final size = _displayedSize(s);
                  final rect = _fitRect(
                    Size(box.maxWidth, box.maxHeight),
                    size.width / size.height,
                  );
                  return Stack(
                    children: [
                      Positioned.fromRect(
                        rect: rect,
                        child: Image.file(
                          File(s.preview),
                          fit: BoxFit.fill,
                          // The file name carries a counter, so this is always
                          // a new path and never a cached decode of the
                          // previous render.
                          gaplessPlayback: true,
                        ),
                      ),
                      if (tool == _Tool.crop)
                        Positioned.fromRect(
                          rect: rect,
                          child: _CropOverlay(draft: draft, onDraft: onDraft),
                        ),
                      if (tool == _Tool.redEye)
                        Positioned.fromRect(
                          rect: rect,
                          child: _RedEyeOverlay(
                            state: s,
                            onTap: onTapEye,
                          ),
                        ),
                    ],
                  );
                },
              ),
            ),
          if (busy)
            const Align(
              alignment: Alignment.topRight,
              child: Padding(
                padding: EdgeInsets.all(18),
                child: SizedBox(
                  width: 18,
                  height: 18,
                  child: CircularProgressIndicator(strokeWidth: 2),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// A rubber-band rectangle with corner handles, in fractions of the image.
/// Fractions rather than pixels because the widget is re-laid-out on every
/// window resize and a pixel rect would drift off the photo.
class _CropOverlay extends StatefulWidget {
  const _CropOverlay({required this.draft, required this.onDraft});

  final Rect? draft;
  final void Function(Rect?) onDraft;

  @override
  State<_CropOverlay> createState() => _CropOverlayState();
}

class _CropOverlayState extends State<_CropOverlay> {
  /// Which handle the current drag grabbed: -1 while drawing a new box, 0..3
  /// for a corner, 4 for the whole box.
  int _grab = -1;
  Offset _from = Offset.zero;
  Rect _start = Rect.zero;

  static const _handle = 16.0;

  Offset _frac(Offset local, Size box) => Offset(
        (local.dx / box.width).clamp(0.0, 1.0),
        (local.dy / box.height).clamp(0.0, 1.0),
      );

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, c) {
        final box = Size(c.maxWidth, c.maxHeight);
        final d = widget.draft;
        return GestureDetector(
          behavior: HitTestBehavior.opaque,
          onPanStart: (e) {
            _from = _frac(e.localPosition, box);
            _start = d ?? Rect.fromPoints(_from, _from);
            _grab = d == null ? -1 : _grabbed(e.localPosition, d, box);
            if (_grab == -1) widget.onDraft(Rect.fromPoints(_from, _from));
          },
          onPanUpdate: (e) {
            final at = _frac(e.localPosition, box);
            widget.onDraft(_resize(at));
          },
          onPanEnd: (_) {
            final r = widget.draft;
            // A tap with no drag is not a crop; clearing it puts the panel
            // back to "drag a box" instead of offering to crop to nothing.
            if (r != null && (r.width < 0.02 || r.height < 0.02)) {
              widget.onDraft(null);
            }
          },
          child: CustomPaint(painter: _CropPainter(d)),
        );
      },
    );
  }

  int _grabbed(Offset local, Rect d, Size box) {
    final corners = [
      Offset(d.left * box.width, d.top * box.height),
      Offset(d.right * box.width, d.top * box.height),
      Offset(d.right * box.width, d.bottom * box.height),
      Offset(d.left * box.width, d.bottom * box.height),
    ];
    for (var i = 0; i < corners.length; i++) {
      if ((corners[i] - local).distance <= _handle) return i;
    }
    final px = Rect.fromLTRB(d.left * box.width, d.top * box.height,
        d.right * box.width, d.bottom * box.height);
    return px.contains(local) ? 4 : -1;
  }

  Rect _resize(Offset at) {
    final s = _start;
    switch (_grab) {
      case 0:
        return _norm(Rect.fromLTRB(at.dx, at.dy, s.right, s.bottom));
      case 1:
        return _norm(Rect.fromLTRB(s.left, at.dy, at.dx, s.bottom));
      case 2:
        return _norm(Rect.fromLTRB(s.left, s.top, at.dx, at.dy));
      case 3:
        return _norm(Rect.fromLTRB(at.dx, s.top, s.right, at.dy));
      case 4:
        final dx = (at.dx - _from.dx).clamp(-s.left, 1.0 - s.right).toDouble();
        final dy = (at.dy - _from.dy).clamp(-s.top, 1.0 - s.bottom).toDouble();
        return s.shift(Offset(dx, dy));
      default:
        return _norm(Rect.fromPoints(_from, at));
    }
  }

  /// Dragging a corner past its opposite gives a negative rectangle, which
  /// composes into a negative crop and a panic in `image::crop_imm`.
  Rect _norm(Rect r) => Rect.fromLTRB(
        math.min(r.left, r.right).clamp(0.0, 1.0),
        math.min(r.top, r.bottom).clamp(0.0, 1.0),
        math.max(r.left, r.right).clamp(0.0, 1.0),
        math.max(r.top, r.bottom).clamp(0.0, 1.0),
      );
}

class _CropPainter extends CustomPainter {
  const _CropPainter(this.draft);

  final Rect? draft;

  @override
  void paint(Canvas canvas, Size size) {
    final full = Offset.zero & size;
    final d = draft;
    if (d == null) {
      canvas.drawRect(full, Paint()..color = const Color(0x33000000));
      return;
    }
    final px = Rect.fromLTRB(d.left * size.width, d.top * size.height,
        d.right * size.width, d.bottom * size.height);
    // Scrim everything outside the box, so the crop reads as a selection and
    // not as a floating rectangle.
    canvas.drawPath(
      Path.combine(
        PathOperation.difference,
        Path()..addRect(full),
        Path()..addRect(px),
      ),
      Paint()..color = const Color(0x99000000),
    );
    final line = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1
      ..color = const Color(0xFFFFFFFF);
    canvas.drawRect(px, line);
    // Thirds, the one guide that is worth drawing by default.
    final thin = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 0.5
      ..color = const Color(0x66FFFFFF);
    for (var i = 1; i < 3; i++) {
      final x = px.left + px.width * i / 3;
      final y = px.top + px.height * i / 3;
      canvas.drawLine(Offset(x, px.top), Offset(x, px.bottom), thin);
      canvas.drawLine(Offset(px.left, y), Offset(px.right, y), thin);
    }
    final knob = Paint()..color = const Color(0xFFFFFFFF);
    for (final c in [px.topLeft, px.topRight, px.bottomRight, px.bottomLeft]) {
      canvas.drawCircle(c, 5, knob);
    }
  }

  @override
  bool shouldRepaint(_CropPainter old) => old.draft != draft;
}

/// Tap to add a correction circle; the existing ones are drawn where they sit.
class _RedEyeOverlay extends StatelessWidget {
  const _RedEyeOverlay({required this.state, required this.onTap});

  final EditorState state;
  final void Function(Offset frac) onTap;

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, c) {
        final box = Size(c.maxWidth, c.maxHeight);
        final size = _displayedSize(state);
        return GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTapDown: (e) => onTap(
            Offset(
              (e.localPosition.dx / box.width).clamp(0.0, 1.0),
              (e.localPosition.dy / box.height).clamp(0.0, 1.0),
            ),
          ),
          child: MouseRegion(
            cursor: SystemMouseCursors.precise,
            child: CustomPaint(
              painter: _RedEyePainter(spots: state.redEye, source: size),
            ),
          ),
        );
      },
    );
  }
}

class _RedEyePainter extends CustomPainter {
  const _RedEyePainter({required this.spots, required this.source});

  final List<RedEyeSpot> spots;
  final Size source;

  @override
  void paint(Canvas canvas, Size size) {
    final sx = size.width / source.width;
    final sy = size.height / source.height;
    final ring = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.5
      ..color = const Color(0xFF22D3EE);
    for (final s in spots) {
      canvas.drawCircle(
        Offset(s.cx * sx, s.cy * sy),
        s.radius * sx,
        ring,
      );
    }
  }

  @override
  bool shouldRepaint(_RedEyePainter old) =>
      old.spots != spots || old.source != source;
}

/// A 0..1 tone curve over its own histogram. Points are what the bridge
/// stores; the interpolation between them is `curves::eval`'s business and is
/// only sketched here, because what the user judges is the picture and not
/// this line.
class _CurveEditor extends StatefulWidget {
  const _CurveEditor({
    required this.points,
    required this.histogram,
    required this.channel,
    required this.busy,
    required this.onCommit,
  });

  final List<CurvePoint> points;
  final Histogram? histogram;
  final String channel;
  final bool busy;
  final void Function(List<CurvePoint>) onCommit;

  @override
  State<_CurveEditor> createState() => _CurveEditorState();
}

class _CurveEditorState extends State<_CurveEditor> {
  /// Non-null only while a point is being dragged. The rest of the time the
  /// bridge's answer is the truth, exactly as with the sliders.
  List<CurvePoint>? _drag;
  int _held = -1;

  List<CurvePoint> get _pts {
    final live = _drag;
    if (live != null) return live;
    // An empty list means the identity curve; showing its two endpoints is
    // what makes the control draggable at all.
    return widget.points.isEmpty
        ? const [CurvePoint(x: 0, y: 0), CurvePoint(x: 1, y: 1)]
        : widget.points;
  }

  Offset _at(CurvePoint p, Size box) =>
      Offset(p.x * box.width, (1 - p.y) * box.height);

  CurvePoint _from(Offset local, Size box) => CurvePoint(
        x: (local.dx / box.width).clamp(0.0, 1.0),
        y: (1 - local.dy / box.height).clamp(0.0, 1.0),
      );

  int _nearest(Offset local, Size box) {
    var best = -1;
    var dist = 18.0;
    for (var i = 0; i < _pts.length; i++) {
      final d = (_at(_pts[i], box) - local).distance;
      if (d < dist) {
        dist = d;
        best = i;
      }
    }
    return best;
  }

  List<CurvePoint> _sorted(List<CurvePoint> pts) {
    final out = [...pts]..sort((a, b) => a.x.compareTo(b.x));
    return out;
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AspectRatio(
      aspectRatio: 1,
      child: Container(
        decoration: BoxDecoration(
          color: t.nTile,
          borderRadius: BorderRadius.circular(Tokens.radiusSm),
        ),
        clipBehavior: Clip.antiAlias,
        child: LayoutBuilder(
          builder: (context, c) {
            final box = Size(c.maxWidth, c.maxHeight);
            return GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTapUp: (e) {
                if (widget.busy) return;
                final hit = _nearest(e.localPosition, box);
                if (hit >= 0) return;
                widget.onCommit(
                  _sorted([..._pts, _from(e.localPosition, box)]),
                );
              },
              // Right-click removes a point. Below two points there is no
              // curve left, and the bridge reads that as "no curve op".
              onSecondaryTapUp: (e) {
                if (widget.busy) return;
                final hit = _nearest(e.localPosition, box);
                if (hit < 0) return;
                final next = [..._pts]..removeAt(hit);
                widget.onCommit(next.length >= 2 ? next : const []);
              },
              onPanStart: (e) {
                _held = _nearest(e.localPosition, box);
                if (_held >= 0) setState(() => _drag = [..._pts]);
              },
              onPanUpdate: (e) {
                final live = _drag;
                if (live == null || _held < 0) return;
                setState(() => live[_held] = _from(e.localPosition, box));
              },
              onPanEnd: (_) {
                final live = _drag;
                setState(() => _drag = null);
                if (live != null && !widget.busy) {
                  widget.onCommit(_sorted(live));
                }
              },
              child: CustomPaint(
                painter: _CurvePainter(
                  points: _pts,
                  histogram: widget.histogram,
                  channel: widget.channel,
                  grid: t.nInk2.withValues(alpha: 0.25),
                ),
              ),
            );
          },
        ),
      ),
    );
  }
}

class _CurvePainter extends CustomPainter {
  const _CurvePainter({
    required this.points,
    required this.histogram,
    required this.channel,
    required this.grid,
  });

  final List<CurvePoint> points;
  final Histogram? histogram;
  final String channel;
  final Color grid;

  static const _tint = <String, Color>{
    'r': Color(0xFFF87171),
    'g': Color(0xFF4ADE80),
    'b': Color(0xFF60A5FA),
    'all': Color(0xFFE5E7EB),
  };

  @override
  void paint(Canvas canvas, Size size) {
    _paintHistogram(canvas, size);

    final thin = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 0.5
      ..color = grid;
    for (var i = 1; i < 4; i++) {
      final x = size.width * i / 4;
      final y = size.height * i / 4;
      canvas.drawLine(Offset(x, 0), Offset(x, size.height), thin);
      canvas.drawLine(Offset(0, y), Offset(size.width, y), thin);
    }
    canvas.drawLine(
      Offset(0, size.height),
      Offset(size.width, 0),
      thin..color = grid.withValues(alpha: 0.5),
    );

    if (points.length < 2) return;
    final colour = _tint[channel] ?? _tint['all']!;
    final path = Path();
    for (var i = 0; i < points.length; i++) {
      final p = Offset(
        points[i].x * size.width,
        (1 - points[i].y) * size.height,
      );
      if (i == 0) {
        // The curve is defined outside its first and last point too: the
        // executor holds the end values flat, so draw that.
        path.moveTo(0, p.dy);
        path.lineTo(p.dx, p.dy);
      } else {
        path.lineTo(p.dx, p.dy);
      }
    }
    path.lineTo(size.width, (1 - points.last.y) * size.height);
    canvas.drawPath(
      path,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1.6
        ..color = colour,
    );
    for (final p in points) {
      canvas.drawCircle(
        Offset(p.x * size.width, (1 - p.y) * size.height),
        4,
        Paint()..color = colour,
      );
    }
  }

  void _paintHistogram(Canvas canvas, Size size) {
    final h = histogram;
    if (h == null) return;
    final channels = channel == 'all'
        ? <(List<int>, Color)>[(h.luma, const Color(0x33FFFFFF))]
        : <(List<int>, Color)>[
            (
              switch (channel) {
                'r' => h.r,
                'g' => h.g,
                _ => h.b,
              },
              (_tint[channel] ?? const Color(0xFFFFFFFF))
                  .withValues(alpha: 0.28)
            ),
          ];
    for (final (buckets, colour) in channels) {
      if (buckets.isEmpty) continue;
      // Clipping at the peak makes every histogram look like one tall spike;
      // the 99th percentile is what photo tools actually plot.
      final sorted = [...buckets]..sort();
      final top = math.max(1, sorted[(sorted.length * 0.99).floor()]);
      final path = Path()..moveTo(0, size.height);
      for (var i = 0; i < buckets.length; i++) {
        final x = size.width * i / (buckets.length - 1);
        final y = size.height * (1 - math.min(1.0, buckets[i] / top));
        path.lineTo(x, y);
      }
      path
        ..lineTo(size.width, size.height)
        ..close();
      canvas.drawPath(path, Paint()..color = colour);
    }
  }

  @override
  bool shouldRepaint(_CurvePainter old) =>
      old.points != points ||
      old.histogram != histogram ||
      old.channel != channel;
}

class _Panel extends StatelessWidget {
  const _Panel({
    required this.state,
    required this.busy,
    required this.adjust,
    required this.tool,
    required this.draft,
    required this.eyeRadius,
    required this.curveChannel,
    required this.curve,
    required this.onAdjustCommit,
    required this.onFilter,
    required this.onSharpen,
    required this.onEnhance,
    required this.onRotate,
    required this.onFlip,
    required this.onTool,
    required this.onAspect,
    required this.onApplyCrop,
    required this.onCancelCrop,
    required this.onClearCrop,
    required this.onEyeRadius,
    required this.onClearEyes,
    required this.onCurveChannel,
    required this.onCurve,
    required this.onRevert,
  });

  final EditorState? state;
  final bool busy;
  final Adjust adjust;
  final _Tool tool;
  final Rect? draft;
  final double eyeRadius;
  final String curveChannel;
  final List<CurvePoint> curve;
  final void Function(Adjust) onAdjustCommit;
  final void Function(String preset, double strength) onFilter;
  final void Function(double) onSharpen;
  final void Function(bool) onEnhance;
  final void Function(int quarterTurns) onRotate;
  final void Function(bool horizontal) onFlip;
  final void Function(_Tool) onTool;
  final void Function(String preset) onAspect;
  final VoidCallback onApplyCrop;
  final VoidCallback onCancelCrop;
  final VoidCallback onClearCrop;
  final void Function(double) onEyeRadius;
  final VoidCallback onClearEyes;
  final void Function(String) onCurveChannel;
  final void Function(List<CurvePoint>) onCurve;
  final VoidCallback onRevert;

  Adjust _with(String field, double v) => Adjust(
        exposure: field == 'exposure' ? v : adjust.exposure,
        contrast: field == 'contrast' ? v : adjust.contrast,
        saturation: field == 'saturation' ? v : adjust.saturation,
        temperature: field == 'temperature' ? v : adjust.temperature,
        tint: field == 'tint' ? v : adjust.tint,
        highlights: field == 'highlights' ? v : adjust.highlights,
        shadows: field == 'shadows' ? v : adjust.shadows,
        blacks: field == 'blacks' ? v : adjust.blacks,
        whites: field == 'whites' ? v : adjust.whites,
      );

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = state;
    final crop = s?.crop;
    // Material rather than a coloured Container, for the SwitchListTile below:
    // see the note on the same swap in photos_page.dart.
    return Material(
      color: t.nCanvas,
      child: ListView(
        padding: const EdgeInsets.fromLTRB(18, 16, 18, 40),
        children: [
          if ((s?.carriedOps ?? 0) > 0) _Carried(count: s!.carriedOps),
          const _Section(label: 'Light & colour'),
          // Ranges are the ones adjust::AdjustParams documents: exposure in
          // stops, everything else -1..1.
          _Slide('Exposure', adjust.exposure, -2, 2, busy,
              (v) => onAdjustCommit(_with('exposure', v))),
          _Slide('Contrast', adjust.contrast, -1, 1, busy,
              (v) => onAdjustCommit(_with('contrast', v))),
          _Slide('Saturation', adjust.saturation, -1, 1, busy,
              (v) => onAdjustCommit(_with('saturation', v))),
          _Slide('Temperature', adjust.temperature, -1, 1, busy,
              (v) => onAdjustCommit(_with('temperature', v))),
          _Slide('Tint', adjust.tint, -1, 1, busy,
              (v) => onAdjustCommit(_with('tint', v))),
          _Slide('Highlights', adjust.highlights, -1, 1, busy,
              (v) => onAdjustCommit(_with('highlights', v))),
          _Slide('Shadows', adjust.shadows, -1, 1, busy,
              (v) => onAdjustCommit(_with('shadows', v))),
          _Slide('Blacks', adjust.blacks, -1, 1, busy,
              (v) => onAdjustCommit(_with('blacks', v))),
          _Slide('Whites', adjust.whites, -1, 1, busy,
              (v) => onAdjustCommit(_with('whites', v))),

          const _Section(label: 'Curves'),
          _ChannelRow(
            channel: curveChannel,
            onPick: onCurveChannel,
            edited: {
              'all': (s?.curveAll ?? const []).isNotEmpty,
              'r': (s?.curveR ?? const []).isNotEmpty,
              'g': (s?.curveG ?? const []).isNotEmpty,
              'b': (s?.curveB ?? const []).isNotEmpty,
            },
          ),
          const SizedBox(height: 8),
          _CurveEditor(
            points: curve,
            histogram: s?.histogram,
            channel: curveChannel,
            busy: busy,
            onCommit: onCurve,
          ),
          Padding(
            padding: const EdgeInsets.only(top: 6),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    'Click to add, drag to move, right-click to remove.',
                    style: TextStyle(fontSize: 10.5, color: t.nInk2),
                  ),
                ),
                if (curve.isNotEmpty)
                  TextButton(
                    onPressed: busy ? null : () => onCurve(const []),
                    child: const Text('Reset'),
                  ),
              ],
            ),
          ),

          const _Section(label: 'Filters'),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              ChoiceChip(
                label: const Text('None'),
                selected: (s?.filter ?? '').isEmpty,
                onSelected: busy ? null : (_) => onFilter('', 1),
              ),
              for (final (id, label) in _filters)
                ChoiceChip(
                  label: Text(label),
                  selected: s?.filter == id,
                  onSelected: busy
                      ? null
                      : (_) => onFilter(id, s?.filterStrength ?? 1.0),
                ),
            ],
          ),
          if ((s?.filter ?? '').isNotEmpty)
            _Slide('Strength', s?.filterStrength ?? 1, 0, 1, busy,
                (v) => onFilter(s!.filter, v)),

          const _Section(label: 'Detail'),
          _Slide('Sharpen', s?.sharpen ?? 0, 0, 4, busy, onSharpen),
          SwitchListTile(
            contentPadding: EdgeInsets.zero,
            value: s?.enhance ?? false,
            onChanged: busy ? null : onEnhance,
            title: const Text('Auto enhance'),
            subtitle: const Text('One-pass levels and contrast'),
          ),

          const _Section(label: 'Crop'),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              for (final (id, label) in _aspects)
                ActionChip(
                  label: Text(label),
                  onPressed: busy ? null : () => onAspect(id),
                ),
            ],
          ),
          const SizedBox(height: 10),
          Row(
            children: [
              Expanded(
                child: OutlinedButton.icon(
                  onPressed: busy ? null : () => onTool(_Tool.crop),
                  icon: Icon(
                    tool == _Tool.crop ? Icons.check_box : Icons.crop,
                    size: 17,
                  ),
                  label: Text(tool == _Tool.crop ? 'Cropping' : 'Crop tool'),
                ),
              ),
              if (crop != null) ...[
                const SizedBox(width: 8),
                IconButton(
                  tooltip: 'Back to the full frame',
                  onPressed: busy ? null : onClearCrop,
                  icon: const Icon(Icons.aspect_ratio, size: 18),
                ),
              ],
            ],
          ),
          if (crop != null)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Text(
                'Cropped to ${crop.w} × ${crop.h} px',
                style: TextStyle(fontSize: 11, color: t.nInk2),
              ),
            ),
          if (tool == _Tool.crop)
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: draft == null
                  ? Text(
                      'Drag a box over the photo. Cropping again works inside '
                      'the crop you already have.',
                      style: TextStyle(
                          fontSize: 10.5, color: t.nInk2, height: 1.4),
                    )
                  : Row(
                      children: [
                        Expanded(
                          child: FilledButton(
                            onPressed: busy ? null : onApplyCrop,
                            child: const Text('Apply crop'),
                          ),
                        ),
                        const SizedBox(width: 8),
                        TextButton(
                          onPressed: onCancelCrop,
                          child: const Text('Clear'),
                        ),
                      ],
                    ),
            ),

          const _Section(label: 'Red-eye'),
          Row(
            children: [
              Expanded(
                child: OutlinedButton.icon(
                  onPressed: busy ? null : () => onTool(_Tool.redEye),
                  icon: Icon(
                    tool == _Tool.redEye
                        ? Icons.check_box
                        : Icons.remove_red_eye_outlined,
                    size: 17,
                  ),
                  label: Text(tool == _Tool.redEye ? 'Placing' : 'Fix red-eye'),
                ),
              ),
              if ((s?.redEye ?? const []).isNotEmpty) ...[
                const SizedBox(width: 8),
                IconButton(
                  tooltip: 'Remove every correction',
                  onPressed: busy ? null : onClearEyes,
                  icon: const Icon(Icons.backspace_outlined, size: 17),
                ),
              ],
            ],
          ),
          if (tool == _Tool.redEye) ...[
            _Slide('Size', eyeRadius, 0.005, 0.08, busy, onEyeRadius),
            Text(
              '${(s?.redEye ?? const []).length} placed. Click each eye once.',
              style: TextStyle(fontSize: 10.5, color: t.nInk2),
            ),
          ],

          const _Section(label: 'Rotate & flip'),
          Row(
            children: [
              _ToolButton(Icons.rotate_left, 'Rotate left',
                  busy ? null : () => onRotate(-1)),
              _ToolButton(Icons.rotate_right, 'Rotate right',
                  busy ? null : () => onRotate(1)),
              _ToolButton(Icons.flip, 'Flip horizontally',
                  busy ? null : () => onFlip(true)),
              _ToolButton(Icons.flip_camera_android, 'Flip vertically',
                  busy ? null : () => onFlip(false)),
            ],
          ),

          const SizedBox(height: 28),
          TextButton.icon(
            onPressed: busy ? null : onRevert,
            icon: const Icon(Icons.delete_outline, size: 18),
            label: const Text('Discard all edits'),
            style: TextButton.styleFrom(
              foregroundColor: const Color(0xFFF43F5E),
            ),
          ),
        ],
      ),
    );
  }
}

/// The four curve channels. A dot marks the ones that carry an edit, so a
/// curve set on the blue channel is not invisible from the other three.
class _ChannelRow extends StatelessWidget {
  const _ChannelRow({
    required this.channel,
    required this.onPick,
    required this.edited,
  });

  final String channel;
  final void Function(String) onPick;
  final Map<String, bool> edited;

  static const _labels = <(String, String)>[
    ('all', 'RGB'),
    ('r', 'R'),
    ('g', 'G'),
    ('b', 'B'),
  ];

  @override
  Widget build(BuildContext context) => Wrap(
        spacing: 8,
        children: [
          for (final (id, label) in _labels)
            ChoiceChip(
              label: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(label),
                  if (edited[id] ?? false) ...const [
                    SizedBox(width: 5),
                    _Dot(),
                  ],
                ],
              ),
              selected: channel == id,
              onSelected: (_) => onPick(id),
            ),
        ],
      );
}

class _Dot extends StatelessWidget {
  const _Dot();

  @override
  Widget build(BuildContext context) => Container(
        width: 5,
        height: 5,
        decoration: const BoxDecoration(
          color: Tokens.secPhotos,
          shape: BoxShape.circle,
        ),
      );
}

class _Carried extends StatelessWidget {
  const _Carried({required this.count});

  final int count;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      margin: const EdgeInsets.only(bottom: 14),
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(Tokens.radiusSm),
      ),
      child: Text(
        '$count edit${count == 1 ? '' : 's'} made elsewhere — curves, text or '
        'a crop — are kept as they are. They still apply, and saving here will '
        'not remove them.',
        style: TextStyle(fontSize: 11.5, color: t.nInk2, height: 1.45),
      ),
    );
  }
}

class _Section extends StatelessWidget {
  const _Section({required this.label});

  final String label;

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.only(top: 20, bottom: 6),
        child: Text(
          label.toUpperCase(),
          style: TextStyle(
            fontFamily: Tokens.fontFamily,
            fontSize: 10.5,
            letterSpacing: 1.1,
            fontWeight: FontWeight.w600,
            color: context.tokens.nInk2,
          ),
        ),
      );
}

/// Holds its own value while the thumb is down, so the number tracks the drag
/// even though the picture only re-renders on release. Without this the handle
/// would snap back on every frame: `value` comes from the bridge, and the
/// bridge has not been told yet.
class _Slide extends StatefulWidget {
  const _Slide(
    this.label,
    this.value,
    this.min,
    this.max,
    this.busy,
    this.onCommit,
  );

  final String label;
  final double value;
  final double min;
  final double max;
  final bool busy;
  final void Function(double) onCommit;

  @override
  State<_Slide> createState() => _SlideState();
}

class _SlideState extends State<_Slide> {
  double? _dragging;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final v = (_dragging ?? widget.value).clamp(widget.min, widget.max);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Expanded(
              child: Text(
                widget.label,
                style: TextStyle(fontSize: 12.5, color: t.nInk),
              ),
            ),
            Text(
              v.toStringAsFixed(2),
              style: TextStyle(
                fontSize: 11.5,
                color: v == 0 ? t.nInk2 : Tokens.secPhotos,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
          ],
        ),
        SliderTheme(
          data: SliderTheme.of(context).copyWith(
            trackHeight: 3,
            overlayShape: const RoundSliderOverlayShape(overlayRadius: 12),
          ),
          child: Slider(
            value: v,
            min: widget.min,
            max: widget.max,
            onChanged: (x) => setState(() => _dragging = x),
            onChangeEnd: (x) {
              setState(() => _dragging = null);
              if (!widget.busy) widget.onCommit(x);
            },
          ),
        ),
      ],
    );
  }
}

class _ToolButton extends StatelessWidget {
  const _ToolButton(this.icon, this.tip, this.onTap);

  final IconData icon;
  final String tip;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) => IconButton(
        onPressed: onTap,
        icon: Icon(icon, size: 20),
        tooltip: tip,
      );
}

class _EditorError extends StatelessWidget {
  const _EditorError({required this.error, required this.onBack});

  final Object error;
  final VoidCallback onBack;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 420),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.broken_image_outlined, size: 40, color: t.nInk2),
            const SizedBox(height: 12),
            Text(
              'This photo could not be opened for editing',
              style: TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: 16,
                fontWeight: FontWeight.w600,
                color: t.nInk,
              ),
            ),
            const SizedBox(height: 8),
            Text(
              '$error',
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 12.5, color: t.nInk2),
            ),
            const SizedBox(height: 16),
            FilledButton(onPressed: onBack, child: const Text('Back')),
          ],
        ),
      ),
    );
  }
}
