// The three mini-player size classes — bar · square · pill.
//
// A port of ui/mini_widget.slint, whose geometry lives in
// crates/tulipix-music/src/mini_player.rs (`MiniStyle::base_size`). Every
// length here is that source's literal times `s`, which is the same `uiscale`
// the Slint widget derives from its own window width.
//
// These take the WINDOW, they do not float in it. `open_mini` in
// crates/tulipix-app/src/miniwin.rs shows a second frameless always-on-top
// toplevel and hides the main one — "the app becomes the widget". Flutter's
// desktop embedding opens exactly one window, so the one window becomes the
// widget instead: shrunk to the style's box, kept above, painting on nothing.
// See `miniIsWidget` in music_controller.dart and `widgetMode` in the runner.
//
// The pin button is still dropped: keep-above is applied for as long as the
// widget is up rather than toggled, which is the rule the Slint build already
// applies on Wayland (`no-stacking-control`), where the compositor has no
// stacking request to make.
//
// What the styles are: `bar` is art on the left at full body height with five
// rows beside it; `square` is a big cover with the transport under it; `pill` is
// collapsed to art, track, play and a chevron that slides the window buttons out
// of the right edge. Cycled by the shuffle button in the chrome, which is what
// `cycle-style` does there.

import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../../design/app_mark.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../shell/window.dart' show WindowChrome;
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_widgets.dart' show MusicArt;
import 'player_widgets.dart' show TrackBar;

/// The three size classes the desktop widget can be.
///
/// Exactly `MiniStyle` in crates/tulipix-music/src/mini_player.rs — three, and
/// no card. The 300x470 card is `MusicMini` in ui/main.slint: a different
/// component, drawn in the main window, reached from the player bar, and not a
/// stop on this cycle. See [kMiniSize].
enum MiniStyle {
  bar,
  square,
  pill;

  /// The design reference in logical px. `MiniStyle::base_size()`.
  Size get base => switch (this) {
        MiniStyle.bar => const Size(441, 212),
        MiniStyle.square => const Size(280, 496),
        MiniStyle.pill => const Size(300, 80),
      };

  /// What the "next layout" button says it is switching to.
  String get label => switch (this) {
        MiniStyle.bar => 'Bar',
        MiniStyle.square => 'Square',
        MiniStyle.pill => 'Pill',
      };

  /// bar -> square -> pill -> bar, `cycle-style` in ui/mini_widget.slint.
  MiniStyle get next => MiniStyle.values[(index + 1) % MiniStyle.values.length];
}

/// Extra WIDTH the pill takes when its button cluster is out, in reference px —
/// five 24px buttons, the gaps between them and the row's own spacing.
/// `PILL_CLUSTER` in mini_player.rs. The window grows; the title keeps the
/// width it had, which is why the pill measures its scale against its height.
const double kPillCluster = 148;

/// The same, one button short: where there is no pin to draw.
/// `PILL_CLUSTER_NO_PIN`.
const double kPillClusterNoPin = 119;

/// Extra HEIGHT the pill takes when its lyrics row is out. `PILL_LYRICS`.
const double kPillLyrics = 32;

/// The gap between the window's edge and the widget. None: the widget floats
/// on the desktop with nothing around it. It used to be a 7/2/7/12 ring for a
/// drop shadow, which read as a block of colour behind the widget -- the
/// shadow, and on a compositor without alpha the window's own fill.
const EdgeInsets kMiniPad = EdgeInsets.zero;

/// The widget's two-colour ramp, and every gradient built from it.
///
/// Light follows the ARTWORK — `a` is the dominant colour pulled off the cover,
/// so the widget wears the record it is playing. Dark ignores it and runs the
/// Music section's pink -> violet instead: an art colour can be anything,
/// including a muddy brown or a near-black, and on a dark panel that is a
/// control you cannot find.
class _Tone {
  _Tone(this.dark, Color art)
      : a = dark ? const Color(0xFFEC4899) : art,
        b = dark ? const Color(0xFF8B5CF6) : _shade(art, 0.45),
        ink = dark ? const Color(0xFFEC4899) : _shade(art, 0.25),
        ring = dark ? const Color(0xFF06B6D4) : _lift(art, 0.35);

  final bool dark;
  final Color a;
  final Color b;

  /// The ramp pushed dark enough to sit under white glyphs and thin type — a
  /// pale cover loses both.
  final Color ink;

  /// The third stop of the 2px border gradient.
  final Color ring;

  Gradient get diag => LinearGradient(
      begin: Alignment.topLeft, end: Alignment.bottomRight, colors: [a, b]);

  Gradient get horiz => LinearGradient(colors: [a, b]);

  Gradient get border => LinearGradient(
        begin: Alignment.topLeft,
        end: Alignment.bottomRight,
        colors: [a, b, ring],
        stops: const [0, 0.55, 1],
      );

  static Color _shade(Color c, double f) {
    final h = HSLColor.fromColor(c);
    return h.withLightness((h.lightness / (1 + f)).clamp(0.0, 1.0)).toColor();
  }

  static Color _lift(Color c, double f) {
    final h = HSLColor.fromColor(c);
    return h
        .withLightness((h.lightness + (1 - h.lightness) * f).clamp(0.0, 1.0))
        .toColor();
  }
}

/// One of the three size classes, drawn.
///
/// Sized by its host, which reads [MusicController.miniWindow] — the style's
/// base size plus whatever the pill's two toggles have added.
class MiniWidgetCard extends StatelessWidget {
  const MiniWidgetCard({super.key, required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = controller.state;
    final now = st?.now;
    if (st == null || now == null) return const SizedBox.shrink();

    final style = controller.widgetStyle;
    final s = controller.widgetScale;
    final tone = _Tone(t.dark, controller.accent);
    final pill = style == MiniStyle.pill;
    final skin = context.skin;
    final body = switch (style) {
      MiniStyle.bar => _Bar(controller: controller, now: now, tone: tone, s: s),
      MiniStyle.square =>
        _Square(controller: controller, now: now, tone: tone, s: s),
      _ => _PillLayout(controller: controller, now: now, tone: tone, s: s),
    };
    final pad = EdgeInsets.fromLTRB(kMiniPad.left * s, kMiniPad.top * s,
        kMiniPad.right * s, kMiniPad.bottom * s);

    // Every language but Standard: the language's own slab -- Neumorphism's
    // raised sheet, Glass's pane, Expressive's tonal container -- in place of
    // the gradient ring, which is Standard's signature and nobody else's. The
    // page colour goes under it first: the window paints on nothing, and a
    // glass pane over nothing is a pane you can read the desktop through.
    if (!skin.isStandard) {
      final r = (pill ? 16 : 20) * s;
      return Padding(
        padding: pad,
        child: Material(
          type: MaterialType.transparency,
          child: Container(
            decoration: BoxDecoration(
              color: skin.canvas ?? t.nCanvas,
              borderRadius: BorderRadius.circular(r),
            ),
            // Flat: the language's raised slab is a pair of shadows in
            // Neumorphism and a drop in Glass, and the widget floats with
            // none. Its buttons and grooves still speak the language.
            child: ClipRRect(
              borderRadius: BorderRadius.circular(r),
              child: body,
            ),
          ),
        ),
      );
    }

    // Transparent so the gradient ring's rounded corners are not boxed in by a
    // square fill, and inset so the shadow has somewhere to fall.
    return Padding(
      padding: pad,
      child: Material(
        // Or every Text in here is drawn with the yellow double underline
        // MaterialApp paints to say "this text has no Material ancestor" — the
        // widget is a Stack child of the app root, not of a Scaffold.
        type: MaterialType.transparency,
        child: Container(
          decoration: BoxDecoration(
            gradient: tone.border,
            borderRadius: BorderRadius.circular((pill ? 16 : 20) * s),
          ),
          padding: EdgeInsets.all(2 * s),
          child: ClipRRect(
            borderRadius: BorderRadius.circular((pill ? 14 : 18) * s),
            child: Container(
              color: t.dark ? const Color(0xE6121422) : const Color(0xE6FBFAFF),
              child: body,
            ),
          ),
        ),
      ),
    );
  }
}

// ── shared parts ────────────────────────────────────────────────────────────

/// Round glass button. `tone` follows `WBtn` in the Slint source:
/// 0 ghost · 1 glass · 2 accent-on · 3 gradient play · 4 solid fill.
class _WBtn extends StatefulWidget {
  const _WBtn({
    required this.icon,
    required this.onTap,
    required this.tone,
    required this.s,
    required this.ramp,
    this.size = 24,
    this.isz = 12,
    this.fill = const Color(0xFF8B5CF6),
    this.tip,
  });

  final IconData icon;
  final VoidCallback? onTap;
  final int tone;
  final double s;
  final _Tone ramp;
  final double size;
  final double isz;
  final Color fill;
  final String? tip;

  @override
  State<_WBtn> createState() => _WBtnState();
}

class _WBtnState extends State<_WBtn> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final w = widget;
    final skin = context.skin;
    if (!skin.isStandard) {
      // The language's round control, the same one the player bar's
      // transport is: tone 3 is the prominent play button, tone 2 latched on,
      // and a chrome button (tone 4) keeps its own colour on its glyph so the
      // five stay tellable apart.
      final b = SkinButton(
        width: w.size * w.s,
        height: w.size * w.s,
        radius: w.size * w.s / 2,
        active: w.tone == 2,
        prominent: w.tone == 3,
        onTap: w.onTap,
        child: Icon(
          skin.icon(w.icon),
          size: w.isz * w.s,
          color: switch (w.tone) {
            3 => skin.onProminent ?? skin.accent,
            2 => skin.accent,
            4 => w.fill,
            _ => skin.inkDim,
          },
          fill: w.tone == 2 ? 1 : null,
        ),
      );
      return w.tip == null ? b : Tooltip(message: w.tip!, child: b);
    }
    final glass = t.dark ? const Color(0x12FFFFFF) : const Color(0x0D1A1040);
    final glassOn = t.dark ? const Color(0x24FFFFFF) : const Color(0x1A1A1040);
    final hair = t.dark ? const Color(0x1AFFFFFF) : const Color(0x1A1A1040);

    // Tone 4 sits back until you reach for it: a tinted plate at rest, the
    // solid colour under the pointer. Five saturated discs in a 24px row was
    // the loudest thing in a widget whose job is to sit in a corner.
    final (Color bg, Color edge, Color ink) = switch (w.tone) {
      0 => (
          _hover ? glass : Colors.transparent,
          Colors.transparent,
          _hover ? t.nInk : t.nInk2
        ),
      2 => (
          w.ramp.a.withValues(alpha: 0.22),
          w.ramp.a.withValues(alpha: 0.5),
          w.ramp.a
        ),
      3 => (Colors.transparent, Colors.transparent, Colors.white),
      4 => (
          _hover ? w.fill : w.fill.withValues(alpha: 0.22),
          _hover ? Colors.transparent : w.fill.withValues(alpha: 0.45),
          _hover ? Colors.white : w.fill
        ),
      _ => (_hover ? glassOn : glass, hair, _hover ? t.nInk : t.nInk2),
    };

    Widget button = AnimatedContainer(
      duration: const Duration(milliseconds: 120),
      width: w.size * w.s,
      height: w.size * w.s,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        color: w.tone == 3 ? null : bg,
        // The play button is the one filled control — accent gradient, own glow.
        gradient: w.tone == 3 ? w.ramp.diag : null,
        border: edge == Colors.transparent ? null : Border.all(color: edge),
        boxShadow: w.tone == 3
            ? [
                BoxShadow(
                  color: w.ramp.a.withValues(alpha: 0.4),
                  blurRadius: 14 * w.s,
                )
              ]
            : null,
      ),
      child: Icon(w.icon, size: w.isz * w.s, color: ink),
    );

    button = MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(onTap: w.onTap, child: button),
    );
    return w.tip == null ? button : Tooltip(message: w.tip!, child: button);
  }
}

/// Elapsed · bar · total, or mute · level · number, inside one outlined pill.
///
/// The same control at three heights, which is what `SeekPill`/`VolPill` and
/// their `compact` flag are in the Slint source. Drawn here rather than taken
/// from player_widgets.dart because those are sized for the 1120px player bar
/// and this one has to be 17px tall inside a 40px row.
class _Pill extends StatelessWidget {
  const _Pill({
    required this.tone,
    required this.s,
    required this.frac,
    required this.onScrub,
    this.height = 26,
    this.leading,
    this.trailing,
    this.onLeading,
    this.knob = false,
    this.bare = false,
  });

  /// No capsule round it: the volume row, which is a bar and not a second
  /// seek pill.
  final bool bare;

  final _Tone tone;
  final double s;
  final double frac;
  final ValueChanged<double> onScrub;
  final double height;

  /// A clock on the seek pill, an icon on the volume one.
  final Widget? leading;
  final String? trailing;
  final VoidCallback? onLeading;

  /// The white dot, only on the roomy styles — the pill's bar is decoration
  /// plus a hit box.
  final bool knob;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final full = height >= 26;
    final pad = (full ? 12.0 : 9.0) * s;
    final gap = (full ? 8.0 : 5.0) * s;
    final thick = (full ? 8.0 : 6.0) * s;
    final label = TextStyle(
      fontSize: (full ? 10.0 : 9.0) * s,
      fontWeight: FontWeight.w700,
      color: t.nInk,
      fontFeatures: const [FontFeature.tabularFigures()],
    );

    final skin = context.skin;
    return Container(
      height: height * s,
      padding: EdgeInsets.symmetric(horizontal: pad),
      // The language's well; Standard's tinted capsule otherwise.
      decoration: bare
          ? null
          : skin.isStandard
              ? BoxDecoration(
                  color: tone.a.withValues(alpha: 0.20),
                  borderRadius: BorderRadius.circular(height * s / 2),
                  border: Border.all(color: tone.a.withValues(alpha: 0.45)),
                )
              : skin.surface(SurfaceRole.well, radius: height * s / 2),
      child: Row(
        children: [
          if (leading != null) ...[
            onLeading == null
                ? leading!
                : GestureDetector(onTap: onLeading, child: leading!),
            SizedBox(width: gap),
          ],
          Expanded(
            child: LayoutBuilder(
              builder: (context, box) {
                void to(double dx) =>
                    onScrub((dx / box.maxWidth).clamp(0.0, 1.0));
                return GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTapDown: (d) => to(d.localPosition.dx),
                  onHorizontalDragUpdate: (d) => to(d.localPosition.dx),
                  child: SizedBox(
                    // A 4px line keeps a 14px hit box so it stays grabbable in
                    // a 17px-tall pill.
                    height: (full ? 14.0 : 12.0) * s,
                    // The player bar's own groove in every other language:
                    // it already knows how each one sinks a track and
                    // raises a knob.
                    child: !skin.isStandard
                        ? Center(
                            child: TrackBar(
                              frac: frac,
                              accent: skin.accent ?? tone.a,
                              scale: s,
                              thickness: full ? 6 : 4,
                            ),
                          )
                        : Stack(
                            alignment: Alignment.centerLeft,
                            clipBehavior: Clip.none,
                            children: [
                              Container(
                                height: thick,
                                decoration: BoxDecoration(
                                  color: t.dark
                                      ? const Color(0x24FFFFFF)
                                      : const Color(0x1A1A1040),
                                  borderRadius:
                                      BorderRadius.circular(thick / 2),
                                ),
                              ),
                              FractionallySizedBox(
                                widthFactor: frac.clamp(0.0, 1.0),
                                child: Container(
                                  height: thick,
                                  decoration: BoxDecoration(
                                    gradient: tone.horiz,
                                    borderRadius:
                                        BorderRadius.circular(thick / 2),
                                  ),
                                ),
                              ),
                              if (knob)
                                Positioned(
                                  left: box.maxWidth * frac.clamp(0.0, 1.0) -
                                      4.5 * s,
                                  child: Container(
                                    width: 9 * s,
                                    height: 9 * s,
                                    decoration: BoxDecoration(
                                      shape: BoxShape.circle,
                                      color: Colors.white,
                                      boxShadow: [
                                        BoxShadow(
                                          color: tone.a.withValues(alpha: 0.63),
                                          blurRadius: 6 * s,
                                        )
                                      ],
                                    ),
                                  ),
                                ),
                            ],
                          ),
                  ),
                );
              },
            ),
          ),
          if (trailing != null) ...[
            SizedBox(width: gap),
            Text(trailing!, style: label),
          ],
        ],
      ),
    );
  }
}

/// A clock cell for the seek pill's two ends.
Widget _clockText(BuildContext context, double secs, bool full, double s) {
  final t = context.tokens;
  return Text(
    _clock(secs),
    style: TextStyle(
      fontSize: (full ? 10.0 : 9.0) * s,
      fontWeight: FontWeight.w700,
      color: t.nInk,
      fontFeatures: const [FontFeature.tabularFigures()],
    ),
  );
}

String _clock(double secs) {
  if (!secs.isFinite || secs < 0) return '0:00';
  final n = secs.round();
  final m = n ~/ 60;
  final r = (n % 60).toString().padLeft(2, '0');
  return m >= 60
      ? '${m ~/ 60}:${(m % 60).toString().padLeft(2, '0')}:$r'
      : '$m:$r';
}

/// The five transport controls, in the order they were asked for:
/// shuffle · previous · play/pause · next · repeat. Both the bar and the square
/// use it, so the order cannot drift between them.
class _Transport extends StatelessWidget {
  const _Transport({
    required this.controller,
    required this.tone,
    required this.s,
    required this.play,
    required this.gap,
  });

  final MusicController controller;
  final _Tone tone;
  final double s;
  final double play;
  final double gap;

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    final shuffle = st?.shuffle ?? false;
    final repeat = st?.repeat ?? 'off';
    final playing = controller.tickPlaying;
    final space = SizedBox(width: gap * s);

    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      mainAxisSize: MainAxisSize.min,
      children: [
        _WBtn(
          icon: Icons.shuffle,
          tone: shuffle ? 2 : 0,
          size: 26,
          isz: 13,
          s: s,
          ramp: tone,
          tip: 'Shuffle',
          onTap: () => controller.send(const MusicCmd.toggleShuffle()),
        ),
        space,
        _WBtn(
          icon: Icons.skip_previous,
          tone: 0,
          size: 28,
          isz: 14,
          s: s,
          ramp: tone,
          tip: 'Previous',
          onTap: () => controller.send(const MusicCmd.prev()),
        ),
        space,
        _WBtn(
          icon: playing ? Icons.pause : Icons.play_arrow,
          tone: 3,
          size: play,
          isz: 16,
          s: s,
          ramp: tone,
          tip: playing ? 'Pause' : 'Play',
          onTap: () => controller.send(const MusicCmd.playPause()),
        ),
        space,
        _WBtn(
          icon: Icons.skip_next,
          tone: 0,
          size: 28,
          isz: 14,
          s: s,
          ramp: tone,
          tip: 'Next',
          onTap: () => controller.send(const MusicCmd.next()),
        ),
        space,
        _WBtn(
          icon: repeat == 'one' ? Icons.repeat_one : Icons.repeat,
          tone: repeat == 'off' ? 0 : 2,
          size: 26,
          isz: 13,
          s: s,
          ramp: tone,
          tip: repeat == 'one'
              ? 'Repeat one'
              : (repeat == 'off' ? 'Repeat off' : 'Repeat all'),
          onTap: () => controller.send(const MusicCmd.cycleRepeat()),
        ),
      ],
    );
  }
}

/// Pin · back to app · theme · next layout · close. One component so no style
/// can end up with different chrome.
///
/// The pin is drawn only where "keep above" means something (X11, XWayland):
/// on native Wayland it is dropped, the rule `no-stacking-control` applies in
/// the Slint build, because a pin that cannot pin is worse than no pin. Close
/// gives the window back like the restore button does, without also jumping
/// the shell to Music.
class _Chrome extends StatelessWidget {
  const _Chrome(
      {required this.controller, required this.tone, required this.s});

  final MusicController controller;
  final _Tone tone;
  final double s;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final gap = SizedBox(width: 5 * s);
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        if (WindowChrome.instance.stacking) ...[
          _WBtn(
            icon: controller.widgetPinned
                ? Icons.push_pin
                : Icons.push_pin_outlined,
            // Latched in the record's colour, plain glass when off: `tone:
            // root.pinned ? 2 : 1` on the Slint pin.
            tone: controller.widgetPinned ? 2 : 1,
            s: s,
            ramp: tone,
            tip: controller.widgetPinned
                ? 'Always on top — on'
                : 'Always on top — off',
            onTap: controller.toggleWidgetPin,
          ),
          gap,
        ],
        _WBtn(
          icon: Icons.open_in_full,
          tone: 4,
          fill: const Color(0xFF8B5CF6),
          s: s,
          ramp: tone,
          tip: 'Back to the app',
          onTap: () {
            ShellController.instance.go(Section.music);
            controller.closeWidget();
          },
        ),
        gap,
        _WBtn(
          icon: t.dark ? Icons.light_mode : Icons.dark_mode,
          tone: 4,
          fill: const Color(0xFFF59E0B),
          s: s,
          ramp: tone,
          tip: t.dark ? 'Light theme' : 'Dark theme',
          onTap: ShellController.instance.cycleTheme,
        ),
        gap,
        _WBtn(
          icon: Icons.shuffle,
          tone: 4,
          fill: const Color(0xFF06B6D4),
          s: s,
          ramp: tone,
          tip: 'Next layout — ${controller.widgetStyle.next.label}',
          onTap: controller.cycleWidgetStyle,
        ),
        gap,
        _WBtn(
          icon: Icons.close,
          tone: 4,
          fill: const Color(0xFFEF4444),
          s: s,
          ramp: tone,
          tip: 'Close the widget',
          onTap: controller.closeWidget,
        ),
      ],
    );
  }
}

/// The app mark and its name, at the head of the bar and the square.
class _Brand extends StatelessWidget {
  const _Brand({required this.s, required this.size, required this.font});

  final double s;
  final double size;
  final double font;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        // The same mark the sidebar and Home draw — there is no
        // `tulipix.png` in assets/appicons, so the earlier reference here was
        // always falling through to its own fallback glyph.
        AppMark(size: size * s, radius: 5 * s),
        SizedBox(width: 7 * s),
        Text(
          'Tulipix',
          style: TextStyle(
            fontSize: font * s,
            fontWeight: FontWeight.w700,
            color: t.nInk,
          ),
        ),
      ],
    );
  }
}

/// The cover, with the flip affordance on it when the track has words.
class _Art extends StatelessWidget {
  const _Art({
    required this.controller,
    required this.now,
    required this.size,
    required this.radius,
    required this.s,
  });

  final MusicController controller;
  final NowPlaying now;
  final double size;
  final double radius;
  final double s;

  @override
  Widget build(BuildContext context) => MusicArt(
        controller: controller,
        kind: now.mode,
        artKey: now.key,
        direct: now.art.isEmpty ? null : now.art,
        size: size,
        radius: radius * s,
      );
}

/// The round badge on the artwork that says this track has words — a mic on the
/// bar and the pill, a flip arrow on the square.
class _Affordance extends StatelessWidget {
  const _Affordance({
    required this.icon,
    required this.onTap,
    required this.tone,
    required this.s,
    required this.size,
    required this.isz,
    this.on = false,
  });

  final IconData icon;
  final VoidCallback onTap;
  final _Tone tone;
  final double s;
  final double size;
  final double isz;

  /// Filled while the row it opened is out, so the badge doubles as its state.
  final bool on;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onTap,
        child: Container(
          width: size * s,
          height: size * s,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: on
                ? tone.ink
                : (t.dark ? const Color(0xCC0E1020) : const Color(0xDDFFFFFF)),
            border: on
                ? null
                : Border.all(
                    color: t.dark
                        ? const Color(0x2EFFFFFF)
                        : const Color(0x331A1040),
                  ),
          ),
          child: Icon(icon, size: isz * s, color: on ? Colors.white : t.nInk2),
        ),
      ),
    );
  }
}

/// Three lines — previous, current, next — as the bar and the square draw them.
///
/// Flattened from the snapshot's lyric list at the point of use, which is the
/// same three strings the Slint widget is handed: it is a separate window there
/// and so a separate component tree, and three lines is all either draws.
class _Lines extends StatelessWidget {
  const _Lines({
    required this.controller,
    required this.tone,
    required this.s,
    required this.near,
    required this.now,
  });

  final MusicController controller;
  final _Tone tone;
  final double s;
  final double near;
  final double now;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final lines = controller.state?.lyrics ?? const <LyricLine>[];
    final a = controller.activeLyric;
    String at(int i) => i >= 0 && i < lines.length ? lines[i].text : '';

    Widget line(String text, double size, Color colour, FontWeight w) => Text(
          text,
          textAlign: TextAlign.center,
          maxLines: 2,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(fontSize: size * s, color: colour, fontWeight: w),
        );

    // The lines run on the same ramp as every control in the widget: the
    // current one at full strength, the two round it dimmed toward the panel.
    final dim = Color.lerp(tone.a, t.dark ? Colors.black : t.nInk2, 0.45)!;
    return Column(
      mainAxisAlignment: MainAxisAlignment.center,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        line(at(a - 1), near, dim, FontWeight.w400),
        SizedBox(height: 6 * s),
        line(at(a).isEmpty ? '…' : at(a), now, tone.ink, FontWeight.w700),
        SizedBox(height: 6 * s),
        line(at(a + 1), near, dim, FontWeight.w400),
      ],
    );
  }
}

// ── bar ─────────────────────────────────────────────────────────────────────
// Art on the left at full body height, everything else stacked right of it.

class _Bar extends StatelessWidget {
  const _Bar({
    required this.controller,
    required this.now,
    required this.tone,
    required this.s,
  });

  final MusicController controller;
  final NowPlaying now;
  final _Tone tone;
  final double s;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = controller;
    final flipped = c.miniFace == 'lyrics';
    final words = c.hasLyrics;
    // Square, and as tall as the body allows: the row's own padding is the
    // only thing between it and the panel's edges, so the art grows with every
    // resize instead of sitting in a fixed box.
    const art = 170.0;

    return Padding(
      padding: EdgeInsets.all(12 * s),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.center,
        children: [
          SizedBox(
            width: art * s,
            height: art * s,
            child: Stack(
              children: [
                _Art(controller: c, now: now, size: art * s, radius: 14, s: s),
                // The square shrinks its cover and puts the lines underneath,
                // which it can afford. This block IS the widget's height, so
                // instead the lines come up over the art behind a scrim.
                if (flipped && words)
                  Positioned.fill(
                    child: Container(
                      decoration: BoxDecoration(
                        color: t.dark
                            ? const Color(0xEE0B0D18)
                            : const Color(0xF2FFFFFF),
                        borderRadius: BorderRadius.circular(14 * s),
                        border: Border.all(
                          color: t.dark
                              ? const Color(0x1AFFFFFF)
                              : const Color(0x1A1A1040),
                        ),
                      ),
                      padding: EdgeInsets.all(10 * s),
                      child: _Lines(
                          controller: c, tone: tone, s: s, near: 9.5, now: 13),
                    ),
                  ),
                // The tell that this track has lyrics at all.
                if (words)
                  Positioned(
                    right: 7 * s,
                    bottom: 7 * s,
                    child: _Affordance(
                      icon: Icons.mic_none,
                      tone: tone,
                      s: s,
                      size: 24,
                      isz: 12,
                      on: flipped,
                      onTap: () => c.setMiniFace('lyrics'),
                    ),
                  ),
              ],
            ),
          ),
          SizedBox(width: 12 * s),
          Expanded(
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                // Mark and window buttons, on a row of their own. They used to
                // float over the top-right corner, which is why the title
                // underneath had to reserve width it could never use.
                SizedBox(
                  height: 24 * s,
                  child: Row(
                    children: [
                      _Brand(s: s, size: 16, font: 11.5),
                      const Spacer(),
                      _Chrome(controller: c, tone: tone, s: s),
                    ],
                  ),
                ),
                SizedBox(height: 4 * s),
                _Title(now: now, s: s, title: 14, sub: 11.5),
                SizedBox(height: 4 * s),
                _Pill(
                  tone: tone,
                  s: s,
                  height: 22,
                  frac: c.tickDur > 0 ? c.tickPos / c.tickDur : 0,
                  leading: _clockText(context, c.tickPos, false, s),
                  trailing: _clock(c.tickDur),
                  onScrub: (f) => c.send(MusicCmd.seek(secs: f * c.tickDur)),
                ),
                SizedBox(height: 4 * s),
                _Transport(controller: c, tone: tone, s: s, play: 34, gap: 9),
                SizedBox(height: 4 * s),
                Row(
                  children: [
                    // Volume across the row, always on screen. The flyout that
                    // used to hang off a button could only ever be as tall as
                    // the panel had left under it.
                    Expanded(
                      child: _Vol(
                          controller: c,
                          now: now,
                          tone: tone,
                          s: s,
                          height: 22),
                    ),
                    SizedBox(width: 8 * s),
                    _WBtn(
                      icon: Icons.fullscreen,
                      tone: 0,
                      s: s,
                      ramp: tone,
                      tip: 'Full screen',
                      onTap: c.openZen,
                    ),
                  ],
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

// ── square ──────────────────────────────────────────────────────────────────
// A title row of its own, then the cover, then everything under it.

class _Square extends StatelessWidget {
  const _Square({
    required this.controller,
    required this.now,
    required this.tone,
    required this.s,
  });

  final MusicController controller;
  final NowPlaying now;
  final _Tone tone;
  final double s;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final flipped = c.miniFace == 'lyrics';
    final words = c.hasLyrics;
    // How much the cover shrinks to when the lyrics come out from behind it.
    const shrink = 0.35;

    return Padding(
      padding: EdgeInsets.all(12 * s),
      child: Column(
        // Slack goes into the cover rather than being shared out as uneven
        // gaps between every row. Slint collects it at the bottom instead
        // (`alignment: start`) — at the reference size there are two pixels of
        // it either way, and giving them to the artwork is what keeps the
        // fixed rows below from ever running past the panel.
        mainAxisAlignment: MainAxisAlignment.start,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SizedBox(
            height: 26 * s,
            child: Row(
              children: [
                _Brand(s: s, size: 18, font: 12.5),
                const Spacer(),
                _Chrome(controller: c, tone: tone, s: s),
              ],
            ),
          ),
          SizedBox(height: 8 * s),
          // The cover takes whatever the fixed rows leave, capped at the
          // column's own width so it stays square.
          //
          // The Slint source gives this block `root.width - 28px` = 252, but
          // the column it sits in is only 238 wide — so there the cover comes
          // out 14px taller than it is wide and the rows below run past the
          // bottom of the panel, which `clip: true` then hides. Measuring the
          // leftover is both what "big square art" means and the one version
          // that cannot overflow: Flutter stripes where Slint clips, and the
          // six rows under it add up to within a pixel or two of the panel.
          Expanded(
            child: LayoutBuilder(
              builder: (context, box) {
                // Back to reference px, so every literal below stays the
                // number the Slint source uses. `s` is clamped to 0.7 at the
                // low end, so it is never zero.
                final art = math.min(box.maxHeight, box.maxWidth) / s;
                return Stack(
                  children: [
                    // Flush under the shrunk art, which is centred above: the lines
                    // start where it ends, so the three of them get the rest of the
                    // block instead of a band of empty panel.
                    if (flipped && words)
                      Positioned.fill(
                        top: art * 0.4 * s,
                        child: Padding(
                          padding: EdgeInsets.all(14 * s),
                          child: _Lines(
                              controller: c,
                              tone: tone,
                              s: s,
                              near: 11,
                              now: 16),
                        ),
                      ),
                    // Top CENTRE once it shrinks, not the corner: the lines under
                    // it are centred, and a cover pinned to one side left the block
                    // reading as two unrelated things.
                    Align(
                      alignment: Alignment.topCenter,
                      child: AnimatedContainer(
                        duration: const Duration(milliseconds: 180),
                        curve: Curves.easeOut,
                        width: art * (flipped && words ? shrink : 1.0) * s,
                        height: art * (flipped && words ? shrink : 1.0) * s,
                        child: _Art(
                          controller: c,
                          now: now,
                          size: art * (flipped && words ? shrink : 1.0) * s,
                          radius: 16,
                          s: s,
                        ),
                      ),
                    ),
                    if (words)
                      Positioned(
                        right: 8 * s,
                        bottom: 8 * s,
                        child: _Affordance(
                          icon: Icons.rotate_right,
                          tone: tone,
                          s: s,
                          size: 26,
                          isz: 13,
                          on: flipped,
                          onTap: () => c.setMiniFace('lyrics'),
                        ),
                      ),
                  ],
                );
              },
            ),
          ),
          SizedBox(height: 8 * s),
          _Title(now: now, s: s, title: 14, sub: 11, centred: true),
          SizedBox(height: 8 * s),
          // The full-size pill: this style has the room, and it is the one
          // people scrub on.
          _Pill(
            tone: tone,
            s: s,
            knob: true,
            frac: c.tickDur > 0 ? c.tickPos / c.tickDur : 0,
            leading: _clockText(context, c.tickPos, true, s),
            trailing: _clock(c.tickDur),
            onScrub: (f) => c.send(MusicCmd.seek(secs: f * c.tickDur)),
          ),
          SizedBox(height: 8 * s),
          _Transport(controller: c, tone: tone, s: s, play: 40, gap: 5),
          SizedBox(height: 10 * s),
          Padding(
            // Keeps the zen button clear of the resize ring in the corner —
            // the ring is on top and would eat its hit box.
            padding: EdgeInsets.only(right: 16 * s),
            child: Row(
              children: [
                Expanded(
                  child: _Vol(controller: c, now: now, tone: tone, s: s),
                ),
                SizedBox(width: 8 * s),
                _WBtn(
                  icon: Icons.fullscreen,
                  tone: 0,
                  s: s,
                  ramp: tone,
                  tip: 'Full screen',
                  onTap: c.openZen,
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

// ── pill ────────────────────────────────────────────────────────────────────
// The collapsed style, and it stays collapsed: art · track · play · a chevron.

class _PillLayout extends StatelessWidget {
  const _PillLayout({
    required this.controller,
    required this.now,
    required this.tone,
    required this.s,
  });

  final MusicController controller;
  final NowPlaying now;
  final _Tone tone;
  final double s;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final words = c.hasLyrics;
    final open = c.pillOpen;
    final lyrics = c.pillLyrics && words;

    return Padding(
      padding: EdgeInsets.all(8 * s),
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Row(
            children: [
              // Nothing is drawn on the cover at rest: this style is 40px of
              // artwork and a title, and a permanent badge on something that
              // small read as damage to the picture. Hovering brings the mic
              // up — that IS the tell — and clicking pulls the row out.
              _PillArt(controller: c, now: now, tone: tone, s: s),
              SizedBox(width: 8 * s),
              Expanded(
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    _Title(
                        now: now, s: s, title: 12, sub: 10, artistOnly: true),
                    SizedBox(height: 2 * s),
                    _Pill(
                      tone: tone,
                      s: s,
                      height: 17,
                      frac: c.tickDur > 0 ? c.tickPos / c.tickDur : 0,
                      leading: _clockText(context, c.tickPos, false, s),
                      trailing: _clock(c.tickDur),
                      onScrub: (f) =>
                          c.send(MusicCmd.seek(secs: f * c.tickDur)),
                    ),
                  ],
                ),
              ),
              SizedBox(width: 8 * s),
              _WBtn(
                icon: c.tickPlaying ? Icons.pause : Icons.play_arrow,
                tone: 3,
                size: 30,
                isz: 14,
                s: s,
                ramp: tone,
                tip: c.tickPlaying ? 'Pause' : 'Play',
                onTap: () => c.send(const MusicCmd.playPause()),
              ),
              SizedBox(width: 8 * s),
              _WBtn(
                icon: open ? Icons.chevron_right : Icons.chevron_left,
                tone: 4,
                fill: const Color(0xFF64748B),
                s: s,
                ramp: tone,
                tip: open ? 'Hide buttons' : 'More',
                onTap: c.togglePillOpen,
              ),
              // Present only when the window has been widened to hold it — the
              // buttons do not eat into the title's width, the card grows.
              if (open) ...[
                SizedBox(width: 8 * s),
                _Chrome(controller: c, tone: tone, s: s),
              ],
            ],
          ),
          // ONE line — the one being sung — because that is all 300px carries
          // and still reads as a pill; the bar and the square are where
          // previous and next belong.
          if (lyrics)
            SizedBox(
              height: 26 * s,
              child: Center(
                child: Text(
                  _nowLine(c),
                  textAlign: TextAlign.center,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: 10.5 * s,
                    fontWeight: FontWeight.w600,
                    color: tone.ink,
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }

  static String _nowLine(MusicController c) {
    final lines = c.state?.lyrics ?? const <LyricLine>[];
    final a = c.activeLyric;
    if (a < 0 || a >= lines.length || lines[a].text.isEmpty) return '…';
    return lines[a].text;
  }
}

/// The pill's 40px cover, with the mic that only appears under the pointer.
class _PillArt extends StatefulWidget {
  const _PillArt({
    required this.controller,
    required this.now,
    required this.tone,
    required this.s,
  });

  final MusicController controller;
  final NowPlaying now;
  final _Tone tone;
  final double s;

  @override
  State<_PillArt> createState() => _PillArtState();
}

class _PillArtState extends State<_PillArt> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final s = widget.s;
    final words = c.hasLyrics;
    final on = c.pillLyrics && words;
    return MouseRegion(
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: SizedBox(
        width: 40 * s,
        height: 40 * s,
        child: Stack(
          children: [
            _Art(
                controller: c, now: widget.now, size: 40 * s, radius: 10, s: s),
            if (words && (_hover || on))
              Positioned(
                right: 2 * s,
                bottom: 2 * s,
                child: _Affordance(
                  icon: Icons.mic_none,
                  tone: widget.tone,
                  s: s,
                  size: 17,
                  isz: 9,
                  on: on,
                  onTap: c.togglePillLyrics,
                ),
              ),
          ],
        ),
      ),
    );
  }
}

// ── odds and ends ───────────────────────────────────────────────────────────

/// Title over subtitle, both eliding — every style is width-bound.
class _Title extends StatelessWidget {
  const _Title({
    required this.now,
    required this.s,
    required this.title,
    required this.sub,
    this.centred = false,
    this.artistOnly = false,
  });

  final NowPlaying now;
  final double s;
  final double title;
  final double sub;
  final bool centred;

  /// The pill has room for the artist and not for the album.
  final bool artistOnly;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final second = now.mode == 'radio' && now.streamTitle.isNotEmpty
        ? now.streamTitle
        : artistOnly
            ? now.artist
            : [now.artist, now.album].where((x) => x.isNotEmpty).join(' · ');
    final align = centred ? TextAlign.center : TextAlign.start;
    return Column(
      crossAxisAlignment:
          centred ? CrossAxisAlignment.center : CrossAxisAlignment.stretch,
      children: [
        Text(
          now.title.isEmpty ? 'Nothing playing' : now.title,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          textAlign: align,
          style: TextStyle(
            fontSize: title * s,
            fontWeight: FontWeight.w700,
            color: t.nInk,
          ),
        ),
        Text(
          second.isEmpty ? 'Pick a track' : second,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          textAlign: align,
          style: TextStyle(fontSize: sub * s, color: t.nInk2),
        ),
      ],
    );
  }
}

/// Volume in the same pill: mute on the left, level inside, number on the
/// right. Every style used to carry a separate round mute button beside a bare
/// bar.
class _Vol extends StatelessWidget {
  const _Vol({
    required this.controller,
    required this.now,
    required this.tone,
    required this.s,
    this.height = 26,
  });

  final MusicController controller;
  final NowPlaying now;
  final _Tone tone;
  final double s;
  final double height;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final full = height >= 26;
    return _Pill(
      bare: true,
      tone: tone,
      s: s,
      height: height,
      // Muted reads as zero rather than as the level it will come back to —
      // the icon is the thing that says why.
      frac: controller.muted ? 0 : (controller.volume / 130).clamp(0.0, 1.0),
      leading: Icon(
        controller.muted ? Icons.volume_off : Icons.volume_up,
        size: (full ? 16.0 : 14.0) * s,
        color: controller.muted ? tone.ink : t.nInk,
      ),
      onLeading: () => controller.send(const MusicCmd.toggleMute()),
      trailing: controller.volume.round().toString(),
      onScrub: (f) => controller.setVolume(f * 130),
    );
  }
}
