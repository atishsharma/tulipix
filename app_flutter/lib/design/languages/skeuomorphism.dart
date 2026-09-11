// Skeuomorphism — "Unibody".
//
// Not stitched leather: the tactile industrial design of today's music
// hardware. Anodised aluminium lit from above, keys that latch down when on,
// black-glass screens with light text, a lathe-turned jog wheel carrying the
// transport, and a knurled volume knob. Screens are black on every tier, as a
// device's are. OLED is black glass all over: no grain, no casts, every part
// drawn by its 1px machined edge.
//
// The values are the mockup's (docs/mockups/music-skeuomorphism.html).

import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../../src/rust/api/music.dart' show MusicCmd;
import '../design_language.dart';
import '../skin.dart';
import '../soft_decoration.dart';
import '../tokens.dart';
import 'symbols_glyphs.dart';

class _Metal {
  const _Metal({
    required this.m1,
    required this.m2,
    required this.wLo,
    required this.pocket,
    required this.ink,
    required this.inkDim,
    required this.hi,
    required this.lo,
    required this.shade,
    required this.key1,
    required this.key2,
    required this.screen,
    required this.signal,
    required this.signal2,
  });

  /// The plate, top to bottom, and the dark lobe of a turned face.
  final Color m1;
  final Color m2;
  final Color wLo;

  /// Milled into the plate.
  final Color pocket;
  final Color ink;
  final Color inkDim;

  /// The chamfer catching the light, and the edge under it.
  final Color hi;
  final Color lo;
  final Color shade;

  /// A key's face, top to bottom.
  final Color key1;
  final Color key2;
  final Color screen;

  /// Pink anodised, lit and in shadow.
  final Color signal;
  final Color signal2;
}

class UnibodySkin extends MusicSkin {
  UnibodySkin(Tokens t)
      : oled = t.dark && t.oled,
        _m = !t.dark ? _light : (t.oled ? _oled : _dark);

  final bool oled;
  final _Metal _m;

  static const _light = _Metal(
    m1: Color(0xFFEEEEEB),
    m2: Color(0xFFDADAD6),
    wLo: Color(0xFFC6C6C1),
    pocket: Color(0xFFD6D6D1),
    ink: Color(0xFF18191B),
    inkDim: Color(0xFF56585D),
    hi: Color(0xF2FFFFFF),
    lo: Color(0x1F000000),
    shade: Color(0x2418181E),
    key1: Color(0xFFF7F7F5),
    key2: Color(0xFFE0E0DC),
    screen: Color(0xFF0B0B0D),
    signal: Color(0xFFFF3D8B),
    signal2: Color(0xFFC81E63),
  );
  static const _dark = _Metal(
    m1: Color(0xFF333438),
    m2: Color(0xFF25262A),
    wLo: Color(0xFF1D1E21),
    pocket: Color(0xFF202124),
    ink: Color(0xFFECECEE),
    inkDim: Color(0xFF9C9EA5),
    hi: Color(0x17FFFFFF),
    lo: Color(0x8C000000),
    shade: Color(0x80000000),
    key1: Color(0xFF3D3E43),
    key2: Color(0xFF2C2D31),
    screen: Color(0xFF060607),
    signal: Color(0xFFFF4F97),
    signal2: Color(0xFFD12B6E),
  );
  static const _oled = _Metal(
    m1: Color(0xFF000000),
    m2: Color(0xFF000000),
    wLo: Color(0xFF050506),
    pocket: Color(0xFF000000),
    ink: Color(0xFFF3F3F5),
    inkDim: Color(0xFF8D8F96),
    hi: Color(0x24FFFFFF),
    lo: Color(0x12FFFFFF),
    shade: Color(0x00000000),
    key1: Color(0xFF151517),
    key2: Color(0xFF09090A),
    screen: Color(0xFF000000),
    signal: Color(0xFFFF4F97),
    signal2: Color(0xFFC21F62),
  );

  @override
  DesignLanguage get language => DesignLanguage.skeuomorphism;

  @override
  Color get canvas => _m.m2;
  @override
  Color get ink => _m.ink;
  @override
  Color get inkDim => _m.inkDim;
  @override
  Color get accent => _m.signal;
  @override
  Color get accentSoft => _m.signal2;
  @override
  String get fontFamily => 'Geist';
  @override
  String get numberFamily => 'Doto';
  @override
  double get iconFill => 1;
  @override
  Color get onProminent => Colors.white;
  @override
  Color get wellInk => const Color(0xFFF4F4F5);
  @override
  Color get wellInkDim => const Color(0x85F4F4F5);

  /// Tall enough for the jog wheel, which carries the transport.
  @override
  double get barHeight => 184;

  /// Lit from above: a chamfer of light on the top edge, a contact shadow, a
  /// short one and a long soft one.
  SoftDecoration lift(double radius) => SoftDecoration(
        color: _m.key2,
        gradient: LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [_m.key1, _m.key2],
        ),
        radius: radius,
        outer: [
          SoftShadow(_m.shade, const Offset(0, 1), 1),
          SoftShadow(_m.shade, const Offset(0, 3), 6),
          SoftShadow(_m.shade, const Offset(0, 10), 20),
        ],
        inner: [
          SoftShadow(_m.hi, const Offset(0, 1), 0),
          SoftShadow(_m.lo, const Offset(0, -1), 0),
        ],
      );

  /// Milled into the plate.
  SoftDecoration pocketOf(double radius) => SoftDecoration(
        color: _m.pocket,
        radius: radius,
        inner: [
          SoftShadow(_m.shade, const Offset(0, 2), 3),
          SoftShadow(_m.shade, const Offset(0, 10), 16),
        ],
        border: _m.lo,
      );

  /// Black glass set into the metal: a sheen line, a hairline bezel.
  SoftDecoration screenOf(double radius) => SoftDecoration(
        color: _m.screen,
        gradient: const LinearGradient(
          begin: Alignment(-1, -1),
          end: Alignment(1, 1),
          stops: [0, 0.32, 0.323, 1],
          colors: [
            Color(0x10FFFFFF),
            Color(0x10FFFFFF),
            Color(0x00FFFFFF),
            Color(0x00FFFFFF),
          ],
        ),
        radius: radius,
        inner: const [SoftShadow(Color(0xE6000000), Offset(0, 2), 8)],
        border: const Color(0x12FFFFFF),
      );

  @override
  Decoration surface(SurfaceRole role, {double radius = 16}) => switch (role) {
        SurfaceRole.well => screenOf(radius),
        SurfaceRole.card => pocketOf(radius + 4),
        SurfaceRole.art => SoftDecoration(
            color: _m.screen,
            radius: radius,
            outer: [SoftShadow(_m.shade, const Offset(0, 4), 10)],
          ),
        // The device's body.
        SurfaceRole.bar => SoftDecoration(
            color: _m.m1,
            radius: radius,
            outer: [SoftShadow(_m.shade, const Offset(0, 10), 20)],
            inner: [SoftShadow(_m.hi, const Offset(0, 1), 0)],
            border: _m.lo,
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
      // Pink anodised: the play button in the jog wheel's hub.
      return SoftDecoration(
        color: _m.signal2,
        radius: radius,
        gradient: RadialGradient(
          center: const Alignment(-0.3, -0.45),
          radius: 0.9,
          colors: [
            Color.lerp(_m.signal, Colors.white, 0.35)!,
            _m.signal,
            _m.signal2,
          ],
        ),
        outer: [
          SoftShadow(_m.shade, Offset(0, pressed ? 1 : 2), pressed ? 2 : 5),
        ],
        inner: const [SoftShadow(Color(0x73FFFFFF), Offset(0, 1), 0)],
      );
    }
    // A key that is on stays latched down; a key under the finger goes down
    // too.
    if (active || pressed) return pocketOf(radius);
    return lift(radius);
  }

  @override
  Widget? pageBackdrop({required Color accent, Color? alt}) => oled
      ? null
      : DecoratedBox(
          decoration: BoxDecoration(
            gradient: LinearGradient(
              begin: Alignment.topCenter,
              end: Alignment.bottomCenter,
              colors: [_m.m1, _m.m2],
            ),
          ),
        );

  @override
  Widget transport(TransportSlot s) {
    final st = s.controller.state;
    final c = s.controller;
    // The rules `Transport` keeps: previous/next step 30 seconds in a podcast
    // and a chapter in an audiobook, and shuffle/repeat only exist where there
    // is an order to disturb.
    final podcast = s.mode == 'podcast';
    final book = s.mode == 'book';
    final ordered = !s.live && !book;
    final wheel = JogWheel(
      skin: this,
      size: s.compact ? 104 : 132,
      playing: c.tickPlaying,
      onPlay: () => c.send(const MusicCmd.playPause()),
      prevTip: book
          ? 'Previous chapter'
          : podcast
              ? 'Back 30s'
              : 'Previous',
      nextTip: book
          ? 'Next chapter'
          : podcast
              ? 'Forward 30s'
              : 'Next',
      onPrev: () => c.send(podcast
          ? const MusicCmd.podSkip(secs: -30)
          : book
              ? const MusicCmd.bookChapter(delta: -1)
              : const MusicCmd.prev()),
      onNext: () => c.send(podcast
          ? const MusicCmd.podSkip(secs: 30)
          : book
              ? const MusicCmd.bookChapter(delta: 1)
              : const MusicCmd.next()),
      shuffleOn: (st?.shuffle ?? false) && ordered,
      repeatOn: (st?.repeat ?? 'off') != 'off' && ordered,
      onShuffle: ordered ? () => c.send(const MusicCmd.toggleShuffle()) : null,
      onRepeat: ordered ? () => c.send(const MusicCmd.cycleRepeat()) : null,
    );
    // The heart leads the transport in every skin. The wheel replaces the
    // whole row, so it carries the heart too: a latching key beside the dish.
    final loved = s.loved;
    if (s.compact || loved == null) return wheel;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Tooltip(
          message: loved ? 'Unlike' : 'Like',
          child: SkinButton(
            width: 44,
            height: 40,
            radius: 11,
            active: loved,
            onTap: s.onFav,
            child: Icon(
              icon(Icons.favorite),
              size: 19,
              color: loved ? accent : inkDim,
            ),
          ),
        ),
        const SizedBox(width: 14),
        wheel,
      ],
    );
  }

  @override
  Widget volume(VolumeSlot s) => VolumeKnob(skin: this, slot: s);

  @override
  IconData icon(IconData material) => roundedSymbols[material] ?? material;
}

/// A lathe-turned dish with the transport on its ring and play in the hub.
class JogWheel extends StatelessWidget {
  const JogWheel({
    super.key,
    required this.skin,
    required this.playing,
    required this.onPlay,
    required this.onPrev,
    required this.onNext,
    this.onShuffle,
    this.onRepeat,
    this.shuffleOn = false,
    this.repeatOn = false,
    this.prevTip = 'Previous',
    this.nextTip = 'Next',
    this.size = 132,
  });

  final UnibodySkin skin;
  final bool playing;
  final bool shuffleOn;
  final bool repeatOn;
  final VoidCallback onPlay;
  final VoidCallback onPrev;
  final VoidCallback onNext;
  final VoidCallback? onShuffle;
  final VoidCallback? onRepeat;
  final String prevTip;
  final String nextTip;

  /// The largest it is drawn. Less when the space it is given is shorter —
  /// the bar's controls row is what is left after the seek row.
  final double size;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
        builder: (context, box) {
          final d = math.min(size, math.min(box.maxWidth, box.maxHeight));
          Widget ring(Alignment at, IconData glyph, String tip,
                  VoidCallback? onTap, bool on) =>
              Align(
                alignment: at,
                child: IconButton(
                  tooltip: tip,
                  onPressed: onTap,
                  padding: EdgeInsets.zero,
                  constraints: BoxConstraints.tight(Size.square(d * 0.26)),
                  iconSize: d * 0.14,
                  color: on ? skin.accent : skin.inkDim,
                  icon: Icon(skin.icon(glyph)),
                ),
              );
          return SizedBox.square(
            dimension: d,
            // IconButton splashes onto a Material; the dish is painted, so it
            // gets a transparent one.
            child: Material(
              type: MaterialType.transparency,
              child: Stack(
                children: [
                  Positioned.fill(
                    child: CustomPaint(painter: _DishPainter(skin._m)),
                  ),
                  ring(Alignment.topCenter, Icons.shuffle, 'Shuffle',
                      onShuffle, shuffleOn),
                  ring(Alignment.bottomCenter, Icons.repeat, 'Repeat',
                      onRepeat, repeatOn),
                  ring(Alignment.centerLeft, Icons.skip_previous, prevTip,
                      onPrev, false),
                  ring(Alignment.centerRight, Icons.skip_next, nextTip,
                      onNext, false),
                  Center(
                    child: Tooltip(
                      message: playing ? 'Pause' : 'Play',
                      child: SkinButton(
                        width: d * 0.42,
                        height: d * 0.42,
                        radius: d * 0.21,
                        prominent: true,
                        active: playing,
                        onTap: onPlay,
                        child: Icon(
                          skin.icon(playing ? Icons.pause : Icons.play_arrow),
                          size: d * 0.2,
                          color: skin.onProminent,
                        ),
                      ),
                    ),
                  ),
                ],
              ),
            ),
          );
        },
      );
}

class _DishPainter extends CustomPainter {
  _DishPainter(this.m);

  final _Metal m;

  @override
  void paint(Canvas canvas, Size size) {
    final c = size.center(Offset.zero);
    final r = size.shortestSide / 2;
    // The body: an anisotropic highlight in four lobes, the way a turned face
    // catches a single light.
    canvas.drawCircle(
      c,
      r,
      Paint()
        ..shader = SweepGradient(
          transform: const GradientRotation(math.pi / 9),
          colors: [m.m1, m.wLo, m.m1, m.wLo, m.m1],
        ).createShader(Rect.fromCircle(center: c, radius: r)),
    );
    // Tooling marks: a ring every two pixels.
    final marks = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1
      ..color = const Color(0x0B000000);
    for (var i = 2.0; i < r; i += 2) {
      canvas.drawCircle(c, i, marks);
    }
    // The chamfer on the rim.
    canvas.drawCircle(
      c,
      r - 0.5,
      Paint()
        ..style = PaintingStyle.stroke
        ..color = m.hi,
    );
    // The recess the hub sits in.
    canvas.drawCircle(c, r * 0.54, Paint()..color = m.pocket);
  }

  @override
  bool shouldRepaint(_DishPainter old) => old.m != m;
}

/// A knurled knob: drag across it to set the volume, click it to mute.
class VolumeKnob extends StatelessWidget {
  const VolumeKnob({
    super.key,
    required this.skin,
    required this.slot,
    this.size = 64,
  });

  final UnibodySkin skin;
  final VolumeSlot slot;

  /// The largest it is drawn; the seek row it sits in is shorter.
  final double size;

  /// -135°..+135° spans 0..130 (mpv's softvol headroom).
  static const double _sweep = 270;

  @override
  Widget build(BuildContext context) {
    final v = slot.volume.clamp(0.0, 130.0);
    final angle = (-_sweep / 2 + v / 130 * _sweep) * math.pi / 180;
    return LayoutBuilder(
      builder: (context, box) {
        final d = math.min(size, box.maxHeight);
        return Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Semantics(
              slider: true,
              label: 'Volume',
              value: '${v.round()}',
              child: MouseRegion(
                cursor: SystemMouseCursors.click,
                child: GestureDetector(
                  // Up or right turns it up; a full sweep is ~240px of travel.
                  onPanUpdate: (e) => slot.onVolume(
                      (v + (e.delta.dx - e.delta.dy) / 240 * 130)
                          .clamp(0.0, 130.0)),
                  onTap: slot.onMute,
                  child: SizedBox.square(
                    dimension: d,
                    child: CustomPaint(
                      painter: _KnobPainter(skin._m, angle, slot.muted),
                    ),
                  ),
                ),
              ),
            ),
            const SizedBox(width: 6),
            SizedBox(
              width: 26,
              child: Text(
                '${v.round()}',
                textAlign: TextAlign.right,
                style: TextStyle(
                  fontSize: 10,
                  color: skin.inkDim,
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
            ),
          ],
        );
      },
    );
  }
}

class _KnobPainter extends CustomPainter {
  _KnobPainter(this.m, this.angle, this.muted);

  final _Metal m;
  final double angle;
  final bool muted;

  @override
  void paint(Canvas canvas, Size size) {
    final c = size.center(Offset.zero);
    final r = size.shortestSide / 2;
    final body = Rect.fromCircle(center: c, radius: r);
    // Pink anodised body, knurled: a dark tooth every 5 degrees round the rim.
    canvas.drawCircle(
      c,
      r,
      Paint()
        ..shader = LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [m.signal, m.signal2],
        ).createShader(body),
    );
    final teeth = Paint()
      ..color = const Color(0x3D000000)
      ..strokeWidth = math.max(1, r * 0.05);
    for (var a = 0.0; a < 2 * math.pi; a += math.pi / 36) {
      final dir = Offset(math.cos(a), math.sin(a));
      canvas.drawLine(c + dir * (r * 0.82), c + dir * r, teeth);
    }
    // The cap.
    canvas.drawCircle(
      c,
      r * 0.74,
      Paint()
        ..shader = RadialGradient(
          center: const Alignment(-0.35, -0.45),
          colors: [
            Color.lerp(m.signal, Colors.white, 0.4)!,
            m.signal,
            m.signal2,
          ],
        ).createShader(body),
    );
    // The pointer.
    final p = c + Offset(math.sin(angle), -math.cos(angle)) * (r * 0.48);
    canvas.drawCircle(
      p,
      math.max(1.5, r * 0.12),
      Paint()..color = muted ? m.inkDim : Colors.white,
    );
  }

  @override
  bool shouldRepaint(_KnobPainter old) =>
      old.angle != angle || old.muted != muted || old.m != m;
}
