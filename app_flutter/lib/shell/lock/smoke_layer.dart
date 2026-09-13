// The lock screen's smoke: one fragment shader (shaders/smoke.frag), shaded a
// third of the window's size into an image and scaled up.
//
// It only draws when the lock screen's motion clock ticks — twenty times a
// second at most, and never while the window is hidden or motion is off. With
// motion off it draws one frame, again only when what it shows changes. Fog
// has no detail to lose at a third of the size, and a third each way is a
// ninth of the pixels to shade.

import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

/// Loaded once per run: the lock screen comes and goes, the program does not.
Future<ui.FragmentProgram>? _program;

class SmokeLayer extends StatefulWidget {
  const SmokeLayer({
    super.key,
    required this.colors,
    required this.calm,
    required this.energy,
    required this.clock,
    required this.pointer,
    required this.moving,
  });

  /// Three colours: the cover's, the paused frame's, or the brand's.
  final List<Color> colors;

  /// 0..1, how much smoke. It eases there, so the lock fades in through it.
  final double calm;

  /// 0..1, read on every frame: the track's loudness at the playhead.
  final double Function() energy;

  /// The lock screen's motion clock, in seconds.
  final ValueListenable<double> clock;

  /// The pointer, 0..1 with y up. Read on each frame, never listened to.
  final ValueListenable<Offset> pointer;

  final bool moving;

  @override
  State<SmokeLayer> createState() => _SmokeLayerState();
}

class _SmokeLayerState extends State<SmokeLayer> {
  ui.FragmentShader? _shader;
  ui.Image? _frame;
  List<double> _pal = const [];
  double _calm = 0;
  double _energy = 0;
  double _last = -1;
  Offset _ptr = const Offset(.5, .3);
  Size _size = Size.zero;

  static List<double> _flat(List<Color> cs) => [
        for (final c in cs) ...[c.r, c.g, c.b]
      ];

  @override
  void initState() {
    super.initState();
    _pal = _flat(widget.colors);
    (_program ??= ui.FragmentProgram.fromAsset('shaders/smoke.frag')).then(
      (p) {
        if (!mounted) return;
        _shader = p.fragmentShader();
        WidgetsBinding.instance.addPostFrameCallback((_) => _still());
      },
      // No shader, no smoke: the backdrop under it still reads as a room.
      onError: (Object e) => debugPrint('smoke: $e'),
    );
    widget.clock.addListener(_tick);
  }

  @override
  void didUpdateWidget(SmokeLayer old) {
    super.didUpdateWidget(old);
    if (old.clock != widget.clock) {
      old.clock.removeListener(_tick);
      widget.clock.addListener(_tick);
    }
    // Standing still, the one frame follows a change of colour or face.
    if (!widget.moving) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _still());
    }
  }

  @override
  void dispose() {
    widget.clock.removeListener(_tick);
    _frame?.dispose();
    _shader?.dispose();
    super.dispose();
  }

  void _still() {
    if (!mounted || widget.moving) return;
    _pal = _flat(widget.colors);
    _calm = widget.calm;
    _energy = widget.energy();
    _render(24);
  }

  void _tick() {
    if (!widget.moving) return;
    final t = widget.clock.value;
    final dt = _last < 0 ? 0.05 : (t - _last).clamp(0.0, 0.25);
    _last = t;
    // Colours, density, loudness and the pointer all ease, so a new track or
    // a jump in loudness drifts in rather than cutting.
    final target = _flat(widget.colors);
    final k = (dt * 2.5).clamp(0.0, 1.0);
    if (_pal.length != target.length) _pal = List.of(target);
    for (var i = 0; i < _pal.length; i++) {
      _pal[i] += (target[i] - _pal[i]) * k;
    }
    _calm += (widget.calm - _calm) * (dt * 1.5).clamp(0.0, 1.0);
    final e = widget.energy();
    _energy += (e - _energy) * (dt * (e > _energy ? 8 : 3)).clamp(0.0, 1.0);
    _ptr += (widget.pointer.value - _ptr) * (dt * 3).clamp(0.0, 1.0);
    _render(t);
  }

  void _render(double t) {
    final shader = _shader;
    if (shader == null || _size.isEmpty) return;
    final w = (_size.width / 3).ceil();
    final h = (_size.height / 3).ceil();
    shader
      ..setFloat(0, w.toDouble())
      ..setFloat(1, h.toDouble())
      ..setFloat(2, t)
      ..setFloat(3, _energy)
      ..setFloat(4, _calm);
    for (var i = 0; i < 9; i++) {
      shader.setFloat(5 + i, i < _pal.length ? _pal[i] : 0);
    }
    shader
      ..setFloat(14, _ptr.dx)
      ..setFloat(15, _ptr.dy);
    final rec = ui.PictureRecorder();
    Canvas(rec).drawRect(
      Rect.fromLTWH(0, 0, w.toDouble(), h.toDouble()),
      Paint()..shader = shader,
    );
    final pic = rec.endRecording();
    final img = pic.toImageSync(w, h);
    pic.dispose();
    setState(() {
      _frame?.dispose();
      _frame = img;
    });
  }

  @override
  Widget build(BuildContext context) => LayoutBuilder(
        builder: (context, box) {
          final moved = _size != box.biggest;
          _size = box.biggest;
          if (moved && !widget.moving) {
            WidgetsBinding.instance.addPostFrameCallback((_) => _still());
          }
          return RawImage(
            image: _frame,
            fit: BoxFit.fill,
            filterQuality: FilterQuality.low,
          );
        },
      );
}
