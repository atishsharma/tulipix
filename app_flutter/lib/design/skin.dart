// The Music section's skin — the design language as something a widget can ask.
//
// Every question has a default meaning "draw what you draw today", and
// Standard answers nothing else, so picking Standard runs exactly the code the
// section had before skins existed: each skinned widget reads
// `skin.x ?? <what it drew before>`. A language overrides only what its look
// actually changes.
//
// Signature pieces that are more than paint — a jog wheel, a wavy seek bar, a
// backdrop blur — are added here by the plan that first needs them, not ahead
// of it.

import 'package:flutter/material.dart';

import '../sections/music/music_controller.dart' show MusicController;
import 'design_language.dart';
import 'languages/claymorphism.dart';
import 'languages/expressive.dart';
import 'languages/glassmorphism.dart';
import 'languages/neumorphism.dart';
import 'languages/skeuomorphism.dart';
import 'tokens.dart';

/// What kind of surface is being drawn.
enum SurfaceRole {
  /// A panel or card standing on the page: stats, resume tiles, the pills.
  card,

  /// A cover's mat — the frame a piece of artwork sits in.
  art,

  /// Sunk into the page: search, the seek groove, a volume track.
  well,

  /// The player bar.
  bar,
}

abstract class MusicSkin extends ThemeExtension<MusicSkin> {
  const MusicSkin();

  DesignLanguage get language;

  bool get isStandard => language == DesignLanguage.standard;

  /// The page behind everything in the section.
  Color? get canvas => null;
  Color? get ink => null;
  Color? get inkDim => null;
  Color? get accent => null;

  /// The pale end of the accent, for fills that run into it.
  Color? get accentSoft => null;
  String? get fontFamily => null;

  Decoration? surface(SurfaceRole role, {double radius = 16}) => null;

  /// Something you press. [active] is a selected tab or a switched-on toggle;
  /// [pressed] is the pointer being down on it right now; [prominent] is the
  /// one control a screen is built around — the play button. [tint] is the
  /// control's own colour, where it has one — a library tab's.
  Decoration? control({
    required bool active,
    bool hovered = false,
    bool pressed = false,
    bool prominent = false,
    double radius = 18,
    Color? tint,
  }) =>
      null;

  /// Ink on an active control. Null: the accent.
  Color? get activeInk => null;

  /// How far a control shrinks while pressed. 1: it does not.
  double get pressScale => 1;

  /// The glyph on the prominent control. Null: the accent, for a language
  /// whose play button is not itself filled with the accent.
  Color? get onProminent => null;

  /// Text inside a [SurfaceRole.well], where a well is another material from
  /// the page — the black glass of a machined screen. Null: [ink], [inkDim].
  Color? get wellInk => null;
  Color? get wellInkDim => null;

  /// A face for figures at 16px and up — totals, counts. Null: [fontFamily].
  String? get numberFamily => null;

  /// The fill axis for every icon in the section, for the glyph fonts that
  /// have one. Null: each icon's own.
  double? get iconFill => null;

  /// The player bar's height. Null: the bar's own 124.
  double? get barHeight => null;

  /// Painted behind the whole section, over the canvas colour — an aura, a
  /// brushed plate. [accent] and [alt] are the playing cover's colours.
  Widget? pageBackdrop({required Color accent, Color? alt}) => null;

  /// Wraps a surface's widget — a backdrop blur, say. Identity by default.
  Widget frame(SurfaceRole role, Widget child, {double radius = 16}) => child;

  /// The bar's whole transport row. Null: `Transport` as it is.
  Widget? transport(TransportSlot s) => null;

  /// The bar's volume control. Null: the volume pill.
  Widget? volume(VolumeSlot s) => null;

  /// The bar's seek control. Null: the seek pill.
  Widget? seekBar(SeekSlot slot) => null;

  /// This language's glyph for a Material icon, or the icon itself.
  IconData icon(IconData material) => material;

  @override
  MusicSkin copyWith() => this;

  /// Languages swap, they do not cross-fade — the same rule `Tokens.lerp`
  /// keeps for themes.
  @override
  MusicSkin lerp(covariant ThemeExtension<MusicSkin>? other, double t) =>
      other is MusicSkin && t >= 0.5 ? other : this;
}

class StandardSkin extends MusicSkin {
  const StandardSkin();

  @override
  DesignLanguage get language => DesignLanguage.standard;
}

/// What a replacement transport is given: what `Transport` takes.
class TransportSlot {
  const TransportSlot({
    required this.controller,
    required this.mode,
    required this.live,
    required this.compact,
    this.loved,
    this.onFav,
  });

  final MusicController controller;
  final String mode;
  final bool live;
  final bool compact;

  /// Null where there is nothing to love.
  final bool? loved;
  final VoidCallback? onFav;
}

/// What a replacement volume control is given: what the volume pill takes.
class VolumeSlot {
  const VolumeSlot({
    required this.volume,
    required this.muted,
    required this.onVolume,
    required this.onMute,
  });

  /// 0..130 — mpv's softvol headroom.
  final double volume;
  final bool muted;
  final ValueChanged<double> onVolume;
  final VoidCallback onMute;
}

/// What a replacement seek bar is given.
class SeekSlot {
  const SeekSlot({
    required this.pos,
    required this.dur,
    required this.playing,
    required this.onSeek,
  });

  /// Seconds, from the once-a-second snapshot.
  final double pos;
  final double dur;
  final bool playing;
  final ValueChanged<double> onSeek;
}

/// The skin for [language] under the theme [t] describes.
MusicSkin skinFor(DesignLanguage language, Tokens t) => switch (language) {
      DesignLanguage.standard => const StandardSkin(),
      DesignLanguage.neumorphism => NeuSkin(t),
      DesignLanguage.claymorphism => ClaySkin(t),
      DesignLanguage.skeuomorphism => UnibodySkin(t),
      DesignLanguage.glassmorphism => GlassSkin(t),
      DesignLanguage.expressive => ExpressiveSkin(t),
    };

extension SkinOf on BuildContext {
  MusicSkin get skin =>
      Theme.of(this).extension<MusicSkin>() ?? const StandardSkin();
}

/// Something pressable that a skin draws: rest, hover, focus and pressed all
/// come from [MusicSkin.control], so the language decides what pressing looks
/// like — sinking into the sheet, squashing, a key travelling down.
///
/// Only used while a skin is on. Standard keeps each widget's own Material
/// button, ripple and all, which is what makes it a true no-op.
class SkinButton extends StatefulWidget {
  const SkinButton({
    super.key,
    required this.child,
    required this.onTap,
    this.active = false,
    this.prominent = false,
    this.radius = 18,
    this.width,
    this.height,
    this.padding,
  });

  final Widget child;

  /// Null disables it: no pointer, no focus, no press.
  final VoidCallback? onTap;
  final bool active;
  final bool prominent;
  final double radius;
  final double? width;
  final double? height;
  final EdgeInsetsGeometry? padding;

  @override
  State<SkinButton> createState() => _SkinButtonState();
}

class _SkinButtonState extends State<SkinButton> {
  bool _hovered = false;
  bool _focused = false;
  bool _pressed = false;

  void _press(bool down) {
    if (_pressed != down) setState(() => _pressed = down);
  }

  @override
  Widget build(BuildContext context) {
    final enabled = widget.onTap != null;
    // Focus lights the control the way hover does, so a keyboard user can see
    // where they are on a surface with no outlines to ring.
    return FocusableActionDetector(
      enabled: enabled,
      mouseCursor:
          enabled ? SystemMouseCursors.click : SystemMouseCursors.basic,
      onShowHoverHighlight: (v) => setState(() => _hovered = v),
      onShowFocusHighlight: (v) => setState(() => _focused = v),
      actions: {
        ActivateIntent: CallbackAction<ActivateIntent>(
          onInvoke: (_) {
            widget.onTap?.call();
            return null;
          },
        ),
      },
      child: Semantics(
        button: true,
        enabled: enabled,
        child: GestureDetector(
          onTapDown: enabled ? (_) => _press(true) : null,
          onTapUp: enabled ? (_) => _press(false) : null,
          onTapCancel: enabled ? () => _press(false) : null,
          onTap: widget.onTap,
          child: AnimatedScale(
            scale: _pressed ? context.skin.pressScale : 1,
            duration: context.tokens.reduceMotion
                ? Duration.zero
                : const Duration(milliseconds: 180),
            curve: Curves.easeOutBack,
            child: Container(
              width: widget.width,
              height: widget.height,
              padding: widget.padding,
              alignment: Alignment.center,
              decoration: context.skin.control(
                active: widget.active,
                hovered: enabled && (_hovered || _focused),
                pressed: _pressed,
                prominent: widget.prominent,
                radius: widget.radius,
              ),
              child: widget.child,
            ),
          ),
        ),
      ),
    );
  }
}
