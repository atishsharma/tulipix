// The profile photo and cover cropper.
//
// Any image, placed under a fixed mask — a circle for the photo, a 4:1 band
// for the cover — by dragging, zooming and quarter-turning it. One function
// paints the stage, the previews and the exported PNG, so the previews are
// exactly what gets written. No package: a PictureRecorder renders the crop at
// the output size.
//
// 4:1 rather than the mockup's 5:1 because the Slint build reads and writes
// the same `cover.png` at 4:1, and two ratios over one file would each crop
// the other's picture.

import 'dart:io';
import 'dart:math' as math;
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:desktop_drop/desktop_drop.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';

import '../../design/pick.dart';
import '../../design/tokens.dart';

enum CropKind {
  avatar(
    stage: Size(360, 360),
    mask: Rect.fromLTWH(40, 40, 280, 280),
    out: Size(512, 512),
  ),
  cover(
    stage: Size(640, 220),
    mask: Rect.fromLTWH(20, 35, 600, 150),
    out: Size(1600, 400),
  );

  const CropKind({required this.stage, required this.mask, required this.out});

  /// The working area, drawn 1:1.
  final Size stage;

  /// The part of the stage that becomes the picture.
  final Rect mask;

  /// The PNG's pixel size — the mask's ratio, bigger.
  final Size out;
}

/// What Use hands back: a rendered PNG, a request to remove the picture, or —
/// photo only — an emoji to stand in for it. Null from the dialog is Cancel.
typedef CropResult = ({Uint8List? png, bool remove, String? emoji});

Future<CropResult?> showCropper(
  BuildContext context, {
  required CropKind kind,
  Uint8List? source,
  String emoji = '',
  bool hasPicture = false,
}) =>
    showDialog<CropResult>(
      context: context,
      builder: (_) => _Cropper(
        kind: kind,
        source: source,
        emoji: emoji,
        hasPicture: hasPicture,
      ),
    );

const List<String> _emojis = [
  '🙂',
  '😎',
  '🎧',
  '🎬',
  '📚',
  '🌊',
  '🦊',
  '🐧',
  '🌙',
  '⚡',
  '🍁',
  '🔮',
];

/// Big photos are decoded down to this on the long side. The export is at
/// most 1600 px wide, and a 24 MP original held at full size is ~100 MB.
const int _decodeCap = 2400;

const double _maxZoom = 4;

Future<ui.Image?> _decode(Uint8List bytes) async {
  try {
    final buffer = await ui.ImmutableBuffer.fromUint8List(bytes);
    final codec = await ui.instantiateImageCodecWithSize(
      buffer,
      getTargetSize: (w, h) {
        final long = math.max(w, h);
        if (long <= _decodeCap) return ui.TargetImageSize(width: w, height: h);
        final k = _decodeCap / long;
        return ui.TargetImageSize(
            width: (w * k).round(), height: (h * k).round());
      },
    );
    final frame = await codec.getNextFrame();
    codec.dispose();
    return frame.image;
  } catch (_) {
    return null;
  }
}

/// The picture's size once turned: a quarter turn swaps its sides.
(int, int) _turned(ui.Image img, int turns) =>
    turns.isOdd ? (img.height, img.width) : (img.width, img.height);

/// Mask pixels per image pixel. At zoom 1 the turned picture just covers the
/// mask, so there is never a gap to fill.
double _scale(ui.Image img, Size mask, double zoom, int turns) {
  final (w, h) = _turned(img, turns);
  return math.max(mask.width / w, mask.height / h) * zoom;
}

/// The furthest the picture can move before one of its edges comes inside
/// the mask.
Offset _clampPan(Offset pan, ui.Image img, Size mask, double zoom, int turns) {
  final s = _scale(img, mask, zoom, turns);
  final (w, h) = _turned(img, turns);
  final mx = math.max(0.0, (w * s - mask.width) / 2);
  final my = math.max(0.0, (h * s - mask.height) / 2);
  return Offset(pan.dx.clamp(-mx, mx), pan.dy.clamp(-my, my));
}

/// [img] as it sits under the mask, in mask coordinates. The stage, the
/// previews and the export all come through here.
void _paintCrop(
    Canvas c, ui.Image img, Size mask, Offset pan, double zoom, int turns) {
  final s = _scale(img, mask, zoom, turns);
  c.save();
  c.translate(mask.width / 2 + pan.dx, mask.height / 2 + pan.dy);
  c.rotate(turns * math.pi / 2);
  c.scale(s);
  c.drawImage(
    img,
    Offset(-img.width / 2, -img.height / 2),
    Paint()..filterQuality = FilterQuality.high,
  );
  c.restore();
}

class _Cropper extends StatefulWidget {
  const _Cropper({
    required this.kind,
    required this.source,
    required this.emoji,
    required this.hasPicture,
  });

  final CropKind kind;
  final Uint8List? source;
  final String emoji;

  /// Whether there is a picture to remove.
  final bool hasPicture;

  @override
  State<_Cropper> createState() => _CropperState();
}

class _CropperState extends State<_Cropper> {
  ui.Image? _img;
  bool _loading = false;
  bool _rendering = false;
  String _error = '';

  Offset _pan = Offset.zero;
  double _zoom = 1;
  int _turns = 0;

  bool _emojiTab = false;
  late String _emoji = widget.emoji;
  final TextEditingController _typed = TextEditingController();

  /// A file is being dragged over the dialog.
  bool _over = false;

  CropKind get _k => widget.kind;
  Size get _mask => _k.mask.size;

  @override
  void initState() {
    super.initState();
    final src = widget.source;
    if (src != null) _load(src);
  }

  @override
  void dispose() {
    _img?.dispose();
    _typed.dispose();
    super.dispose();
  }

  Future<void> _load(Uint8List bytes) async {
    if (!mounted) return;
    setState(() {
      _loading = true;
      _error = '';
    });
    final img = await _decode(bytes);
    if (!mounted) {
      img?.dispose();
      return;
    }
    setState(() {
      _loading = false;
      if (img == null) {
        _error = 'That file is not an image Tulipix can open. '
            'Try a PNG, JPG, WebP or GIF.';
        return;
      }
      _img?.dispose();
      _img = img;
      _emojiTab = false;
      _pan = Offset.zero;
      _zoom = 1;
      _turns = 0;
    });
  }

  Future<void> _loadFile(String path) async {
    try {
      await _load(await File(path).readAsBytes());
    } catch (_) {
      if (mounted) {
        setState(() => _error =
            'Could not read ${path.split(Platform.pathSeparator).last}.');
      }
    }
  }

  Future<void> _choose() async {
    final path = await pickFile(
      label: 'Images',
      extensions: const ['png', 'jpg', 'jpeg', 'webp', 'gif', 'bmp'],
    );
    if (path != null) await _loadFile(path);
  }

  void _drop(DropDoneDetails d) {
    setState(() => _over = false);
    if (d.files.isNotEmpty) _loadFile(d.files.first.path);
  }

  void _setZoom(double z) {
    final img = _img;
    if (img == null) return;
    setState(() {
      _zoom = z.clamp(1.0, _maxZoom);
      _pan = _clampPan(_pan, img, _mask, _zoom, _turns);
    });
  }

  void _turn(int by) {
    final img = _img;
    if (img == null) return;
    setState(() {
      _turns = (_turns + by) % 4;
      _pan = _clampPan(_pan, img, _mask, _zoom, _turns);
    });
  }

  void _reset() => setState(() {
        _pan = Offset.zero;
        _zoom = 1;
        _turns = 0;
      });

  Future<Uint8List?> _render() async {
    final img = _img;
    if (img == null) return null;
    final out = _k.out;
    final rec = ui.PictureRecorder();
    final c = Canvas(rec);
    c.scale(out.width / _mask.width);
    _paintCrop(c, img, _mask, _pan, _zoom, _turns);
    final pic = rec.endRecording();
    final shot = await pic.toImage(out.width.round(), out.height.round());
    pic.dispose();
    final data = await shot.toByteData(format: ui.ImageByteFormat.png);
    shot.dispose();
    return data?.buffer.asUint8List();
  }

  Future<void> _use() async {
    if (_emojiTab) {
      Navigator.pop<CropResult>(
          context, (png: null, remove: true, emoji: _emoji));
      return;
    }
    setState(() => _rendering = true);
    final png = await _render();
    if (!mounted) return;
    if (png == null) {
      setState(() {
        _rendering = false;
        _error = 'Could not render the crop.';
      });
      return;
    }
    Navigator.pop<CropResult>(context, (png: png, remove: false, emoji: null));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final avatar = _k == CropKind.avatar;
    return Dialog(
      // `panel2` is opaque on both palettes; the keys and the source button
      // inside it take `panelSolid`, because plain `panel` is 82% white on the
      // light one and let this dialog's own fill through them.
      backgroundColor: t.panel2,
      insetPadding: const EdgeInsets.all(24),
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(20),
        side: BorderSide(
          color: _over ? Tokens.brand : t.outline,
          width: _over ? 2 : 1,
        ),
      ),
      clipBehavior: Clip.antiAlias,
      child: DropTarget(
        onDragEntered: (_) => setState(() => _over = true),
        onDragExited: (_) => setState(() => _over = false),
        onDragDone: _drop,
        child: SizedBox(
          width: avatar ? 780 : 700,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              _head(t, avatar),
              Divider(height: 1, color: t.outline),
              Flexible(
                child: SingleChildScrollView(
                  padding: const EdgeInsets.fromLTRB(22, 20, 22, 20),
                  child: _emojiTab
                      ? _emojiBody(t)
                      : avatar
                          ? _avatarBody(t)
                          : _coverBody(t),
                ),
              ),
              Divider(height: 1, color: t.outline),
              _foot(t, avatar),
            ],
          ),
        ),
      ),
    );
  }

  Widget _head(Tokens t, bool avatar) => Padding(
        padding: const EdgeInsets.fromLTRB(22, 14, 12, 12),
        child: Row(
          children: [
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(avatar ? 'Profile photo' : 'Cover image',
                      style: TextStyle(
                          fontSize: 17,
                          fontWeight: FontWeight.w800,
                          color: t.text)),
                  Text(
                      avatar
                          ? 'Any image, cropped to a circle'
                          : 'Any image, cropped to the header’s 4:1 band',
                      style: TextStyle(fontSize: 12, color: t.textDim)),
                ],
              ),
            ),
            if (avatar)
              SegmentedButton<bool>(
                segments: const [
                  ButtonSegment(value: false, label: Text('Photo')),
                  ButtonSegment(value: true, label: Text('Emoji')),
                ],
                selected: {_emojiTab},
                showSelectedIcon: false,
                style: const ButtonStyle(visualDensity: VisualDensity.compact),
                onSelectionChanged: (s) => setState(() => _emojiTab = s.first),
              ),
            const SizedBox(width: 8),
            IconButton(
              tooltip: 'Close',
              onPressed: () => Navigator.pop(context),
              icon: const Icon(Icons.close, size: 18),
            ),
          ],
        ),
      );

  Widget _foot(Tokens t, bool avatar) {
    final canUse = _emojiTab || (_img != null && !_rendering);
    return Padding(
      padding: const EdgeInsets.fromLTRB(22, 12, 16, 12),
      child: Row(
        children: [
          Expanded(
            child: Text(
              _error.isNotEmpty
                  ? _error
                  : 'Shows on the page now, and is kept when you press '
                      'Save changes.',
              style: TextStyle(
                  fontSize: 11.5,
                  color: _error.isNotEmpty ? Tokens.error : t.textDim),
            ),
          ),
          if (widget.hasPicture && !_emojiTab)
            TextButton(
              onPressed: () => Navigator.pop<CropResult>(
                  context, (png: null, remove: true, emoji: null)),
              child: Text(avatar ? 'Remove photo' : 'Remove cover'),
            ),
          const SizedBox(width: 6),
          OutlinedButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
          const SizedBox(width: 8),
          FilledButton(
            onPressed: canUse ? _use : null,
            child: Text(_emojiTab
                ? 'Use emoji'
                : avatar
                    ? 'Use photo'
                    : 'Use cover'),
          ),
        ],
      ),
    );
  }

  // ── bodies ────────────────────────────────────────────────────────────────

  Widget _avatarBody(Tokens t) => Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          _stageBlock(t),
          const SizedBox(width: 22),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                _label(t, 'Where it shows'),
                const SizedBox(height: 10),
                Row(
                  crossAxisAlignment: CrossAxisAlignment.end,
                  children: [
                    _figure(t, _preview(t, 28, 28, round: true), 'Sidebar'),
                    const SizedBox(width: 22),
                    _figure(t, _preview(t, 96, 96, round: true), 'Profile'),
                  ],
                ),
                const SizedBox(height: 22),
                _label(t, 'Picture'),
                const SizedBox(height: 10),
                _source(t, 'PNG, JPG, WebP or GIF from your computer'),
                const SizedBox(height: 8),
                _dropHint(t),
              ],
            ),
          ),
        ],
      );

  Widget _coverBody(Tokens t) => Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Center(child: _stageBlock(t)),
          const SizedBox(height: 18),
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _label(t, 'How the profile header shows it'),
                  const SizedBox(height: 8),
                  _preview(t, 360, 90, round: false),
                ],
              ),
              const SizedBox(width: 22),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    _label(t, 'Picture'),
                    const SizedBox(height: 8),
                    _source(t, 'Wide images work best'),
                    const SizedBox(height: 8),
                    _dropHint(t),
                  ],
                ),
              ),
            ],
          ),
        ],
      );

  Widget _emojiBody(Tokens t) => Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          _label(t, 'An emoji instead of a photo'),
          const SizedBox(height: 12),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [for (final e in _emojis) _emojiKey(t, e)],
          ),
          const SizedBox(height: 14),
          SizedBox(
            width: 220,
            child: TextField(
              controller: _typed,
              decoration: const InputDecoration(
                isDense: true,
                border: OutlineInputBorder(),
                hintText: 'Or type any emoji',
              ),
              onChanged: (v) => setState(() => _emoji = v.trim()),
            ),
          ),
          const SizedBox(height: 12),
          Text(
            'An emoji takes the photo’s place. It is also what shows if the '
            'photo file goes missing.',
            style: TextStyle(fontSize: 12, color: t.textDim),
          ),
        ],
      );

  Widget _emojiKey(Tokens t, String e) {
    final on = _emoji == e;
    return Material(
      color: t.panelSolid,
      borderRadius: BorderRadius.circular(12),
      child: InkWell(
        borderRadius: BorderRadius.circular(12),
        onTap: () => setState(() {
          _emoji = e;
          _typed.clear();
        }),
        child: Container(
          width: 48,
          height: 48,
          alignment: Alignment.center,
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(12),
            border: Border.all(
              color: on ? Tokens.brand : t.outline,
              width: on ? 2 : 1,
            ),
          ),
          child: Text(e, style: const TextStyle(fontSize: 24)),
        ),
      ),
    );
  }

  // ── the stage ─────────────────────────────────────────────────────────────

  Widget _stageBlock(Tokens t) => SizedBox(
        width: _k.stage.width,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            _stage(t),
            const SizedBox(height: 8),
            _tools(),
          ],
        ),
      );

  Widget _stage(Tokens t) {
    final img = _img;
    return ClipRRect(
      borderRadius: BorderRadius.circular(14),
      child: SizedBox.fromSize(
        size: _k.stage,
        child: ColoredBox(
          color: const Color(0xFF08090B),
          child: Listener(
            onPointerSignal: (e) {
              if (e is! PointerScrollEvent || img == null) return;
              GestureBinding.instance.pointerSignalResolver.register(
                e,
                (e) => _setZoom(
                    _zoom - (e as PointerScrollEvent).scrollDelta.dy.sign * 0.1),
              );
            },
            child: MouseRegion(
              cursor:
                  img == null ? MouseCursor.defer : SystemMouseCursors.grab,
              child: GestureDetector(
                onPanUpdate: img == null
                    ? null
                    : (d) => setState(() => _pan = _clampPan(
                        _pan + d.delta, img, _mask, _zoom, _turns)),
                child: CustomPaint(
                  painter: _StagePainter(
                    kind: _k,
                    img: img,
                    pan: _pan,
                    zoom: _zoom,
                    turns: _turns,
                  ),
                  child: img == null
                      ? _empty()
                      : const Align(
                          alignment: Alignment.bottomCenter,
                          child: Padding(
                            padding: EdgeInsets.only(bottom: 10),
                            child: _Hint(),
                          ),
                        ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _empty() => Center(
        child: _loading
            ? const CircularProgressIndicator(strokeWidth: 2)
            : Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  const Icon(Icons.add_photo_alternate_outlined,
                      size: 30, color: Colors.white70),
                  const SizedBox(height: 10),
                  FilledButton.icon(
                    onPressed: _choose,
                    icon: const Icon(Icons.upload, size: 16),
                    label: const Text('Choose an image…'),
                  ),
                  const SizedBox(height: 8),
                  const Text('or drop one on this window',
                      style: TextStyle(fontSize: 11.5, color: Colors.white60)),
                ],
              ),
      );

  Widget _tools() {
    final on = _img != null;
    return Row(
      children: [
        _tool(Icons.remove, 'Zoom out', on ? () => _setZoom(_zoom - 0.25) : null),
        Expanded(
          child: Slider(
            value: _zoom,
            min: 1,
            max: _maxZoom,
            onChanged: on ? _setZoom : null,
          ),
        ),
        _tool(Icons.add, 'Zoom in', on ? () => _setZoom(_zoom + 0.25) : null),
        const SizedBox(width: 6),
        _tool(Icons.rotate_left, 'Rotate left', on ? () => _turn(-1) : null),
        _tool(Icons.rotate_right, 'Rotate right', on ? () => _turn(1) : null),
        _tool(Icons.restart_alt, 'Reset', on ? _reset : null),
      ],
    );
  }

  Widget _tool(IconData icon, String tip, VoidCallback? onTap) => IconButton(
        tooltip: tip,
        onPressed: onTap,
        visualDensity: VisualDensity.compact,
        iconSize: 18,
        icon: Icon(icon),
      );

  // ── the side ──────────────────────────────────────────────────────────────

  Widget _preview(Tokens t, double w, double h, {required bool round}) {
    final img = _img;
    final Widget face = img == null
        ? ColoredBox(color: t.panelSolid)
        : CustomPaint(
            painter: _CropPainter(
              img: img,
              mask: _mask,
              pan: _pan,
              zoom: _zoom,
              turns: _turns,
            ),
          );
    return SizedBox(
      width: w,
      height: h,
      child: round
          ? ClipOval(child: face)
          : ClipRRect(borderRadius: BorderRadius.circular(10), child: face),
    );
  }

  Widget _figure(Tokens t, Widget child, String caption) => Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          child,
          const SizedBox(height: 6),
          Text(caption, style: TextStyle(fontSize: 11, color: t.textDim)),
        ],
      );

  Widget _label(Tokens t, String text) => Text(
        text.toUpperCase(),
        style: TextStyle(
          fontSize: 10,
          fontWeight: FontWeight.w700,
          letterSpacing: 0.8,
          color: t.textDim,
        ),
      );

  Widget _source(Tokens t, String note) => Material(
        color: t.panelSolid,
        borderRadius: BorderRadius.circular(12),
        child: InkWell(
          borderRadius: BorderRadius.circular(12),
          onTap: _choose,
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(12),
              border: Border.all(color: t.outline),
            ),
            child: Row(
              children: [
                Icon(Icons.upload, size: 18, color: t.textDim),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text('Choose an image…',
                          style: TextStyle(
                              fontSize: 12.5,
                              fontWeight: FontWeight.w700,
                              color: t.text)),
                      Text(note,
                          style: TextStyle(fontSize: 11, color: t.textDim)),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      );

  Widget _dropHint(Tokens t) => Container(
        padding: const EdgeInsets.all(10),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(12),
          border: Border.all(color: _over ? Tokens.brand : t.outline),
        ),
        child: Text(
          'or drop an image anywhere on this window',
          textAlign: TextAlign.center,
          style: TextStyle(fontSize: 11.5, color: t.textDim),
        ),
      );
}

class _Hint extends StatelessWidget {
  const _Hint();

  @override
  Widget build(BuildContext context) => Container(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
        decoration: BoxDecoration(
          color: const Color(0x73000000),
          borderRadius: BorderRadius.circular(6),
        ),
        child: const Text('drag to move · scroll to zoom',
            style: TextStyle(fontSize: 11, color: Colors.white)),
      );
}

/// A preview: the crop scaled into whatever box it is given, which must have
/// the mask's ratio.
class _CropPainter extends CustomPainter {
  _CropPainter({
    required this.img,
    required this.mask,
    required this.pan,
    required this.zoom,
    required this.turns,
  });

  final ui.Image img;
  final Size mask;
  final Offset pan;
  final double zoom;
  final int turns;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.scale(size.width / mask.width);
    _paintCrop(canvas, img, mask, pan, zoom, turns);
  }

  @override
  bool shouldRepaint(_CropPainter o) =>
      o.img != img || o.pan != pan || o.zoom != zoom || o.turns != turns;
}

/// The stage: the whole picture, dimmed outside the mask, with the mask's
/// outline and a rule-of-thirds grid inside it.
class _StagePainter extends CustomPainter {
  _StagePainter({
    required this.kind,
    required this.img,
    required this.pan,
    required this.zoom,
    required this.turns,
  });

  final CropKind kind;
  final ui.Image? img;
  final Offset pan;
  final double zoom;
  final int turns;

  @override
  void paint(Canvas canvas, Size size) {
    final m = kind.mask;
    final img = this.img;
    if (img != null) {
      canvas.save();
      canvas.translate(m.left, m.top);
      _paintCrop(canvas, img, m.size, pan, zoom, turns);
      canvas.restore();
    }
    final hole = kind == CropKind.avatar
        ? (Path()..addOval(m))
        : (Path()..addRRect(RRect.fromRectAndRadius(m, const Radius.circular(10))));
    canvas.drawPath(
      Path.combine(PathOperation.difference, Path()..addRect(Offset.zero & size),
          hole),
      Paint()..color = const Color(0xA0060709),
    );
    if (img != null) {
      final grid = Paint()
        ..color = const Color(0x47FFFFFF)
        ..strokeWidth = 1;
      canvas.save();
      canvas.clipPath(hole);
      for (var i = 1; i < 3; i++) {
        final x = m.left + m.width * i / 3;
        final y = m.top + m.height * i / 3;
        canvas.drawLine(Offset(x, m.top), Offset(x, m.bottom), grid);
        canvas.drawLine(Offset(m.left, y), Offset(m.right, y), grid);
      }
      canvas.restore();
    }
    canvas.drawPath(
      hole,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2
        ..color = const Color(0xEBFFFFFF),
    );
  }

  @override
  bool shouldRepaint(_StagePainter o) =>
      o.img != img || o.pan != pan || o.zoom != zoom || o.turns != turns;
}
