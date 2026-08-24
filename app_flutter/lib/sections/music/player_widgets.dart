// The parts every player is built from — the bottom bar, the zen page and the
// floating mini all use these, at three different sizes.
//
// They take a `scale` rather than being three separate widgets because that is
// what the Slint versions do: the zen player is the bar's controls at 1.25×,
// the mini is the same shapes again inside 300px. Keeping one implementation
// means a fix to the seek behaviour is a fix in all three.

import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/scheduler.dart' show Ticker;

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';

/// A round icon button. `active` lights it in the accent; that is the only
/// state these carry.
class PlayerBtn extends StatelessWidget {
  const PlayerBtn({
    super.key,
    required this.icon,
    required this.onTap,
    this.tip,
    this.size = 38,
    this.iconSize = 18,
    this.active = false,
    this.accent = Tokens.secMusic,
  });

  final IconData icon;
  final VoidCallback? onTap;
  final String? tip;
  final double size;
  final double iconSize;
  final bool active;
  final Color accent;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // Outlined, as in Slint: `border-width: 1px` with the accent at half
    // strength when active and the hairline otherwise. A bare glyph on a
    // washed bar does not read as a thing you can press — which is what the
    // transport looked like, seven marks floating in a gap.
    final button = SizedBox(
      width: size,
      height: size,
      child: Material(
        color: active ? accent.withValues(alpha: 0.18) : Colors.transparent,
        shape: CircleBorder(
          side: BorderSide(
            color: active ? accent.withValues(alpha: 0.5) : t.nHair,
          ),
        ),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onTap,
          child: Icon(
            icon,
            size: iconSize,
            color: active ? accent : (onTap == null ? t.nInk2 : t.nInk),
          ),
        ),
      ),
    );
    return tip == null ? button : Tooltip(message: tip!, child: button);
  }
}

/// The play/pause circle. Squircles and glows on press, which is the one bit of
/// motion the transport has.
class BigPlayButton extends StatefulWidget {
  const BigPlayButton({
    super.key,
    required this.playing,
    required this.onTap,
    this.size = 52,
    this.accent = Tokens.secMusic,
  });

  final bool playing;
  final VoidCallback onTap;
  final double size;
  final Color accent;

  @override
  State<BigPlayButton> createState() => _BigPlayButtonState();
}

class _BigPlayButtonState extends State<BigPlayButton> {
  bool _hover = false;
  bool _down = false;

  @override
  Widget build(BuildContext context) {
    final lit = _hover || _down;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTapDown: (_) => setState(() => _down = true),
        onTapUp: (_) => setState(() => _down = false),
        onTapCancel: () => setState(() => _down = false),
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 140),
          curve: Curves.easeInOut,
          width: widget.size,
          height: widget.size,
          decoration: BoxDecoration(
            color: lit ? widget.accent : const Color(0xFFF0F0F3),
            borderRadius: BorderRadius.circular(
                _down ? widget.size * 0.31 : widget.size / 2),
            boxShadow: [
              BoxShadow(
                color: widget.accent.withValues(alpha: 0.53),
                blurRadius: _down ? widget.size * 0.5 : widget.size * 0.34,
              ),
            ],
          ),
          child: Icon(
            widget.playing ? Icons.pause : Icons.play_arrow,
            size: widget.size * 0.42,
            color: const Color(0xFF14141B),
          ),
        ),
      ),
    );
  }
}

/// Position, a draggable track and duration, all inside one pill.
///
/// It commits on release rather than on every drag frame: seeking mpv sixty
/// times a second makes it stutter, and the label following the finger is what
/// the eye actually reads as responsiveness.
class SeekPill extends StatefulWidget {
  const SeekPill({
    super.key,
    required this.pos,
    required this.dur,
    required this.onSeek,
    this.accent = Tokens.secMusic,
    this.scale = 1.0,
  });

  final double pos;
  final double dur;
  final ValueChanged<double> onSeek;
  final Color accent;
  final double scale;

  @override
  State<SeekPill> createState() => _SeekPillState();
}

class _SeekPillState extends State<SeekPill> {
  double? _dragging;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final dur = widget.dur <= 0 ? 1.0 : widget.dur;
    final shown = (_dragging ?? widget.pos).clamp(0.0, dur);
    final s = widget.scale;
    final label = TextStyle(
      fontSize: 11 * s,
      fontWeight: FontWeight.w600,
      color: t.nInk2,
      fontFeatures: const [FontFeature.tabularFigures()],
    );

    return Container(
      height: 30 * s,
      padding: EdgeInsets.symmetric(horizontal: 12 * s),
      decoration: BoxDecoration(
        // The pill wears the record's colour too, at a fifth: Slint's is
        // `accent.with-alpha(0.20)` over a `0.45` outline, and a neutral chip
        // under a coloured fill made the bar look bolted on.
        color: widget.accent.withValues(alpha: 0.20),
        borderRadius: BorderRadius.circular(15 * s),
        border: Border.all(color: widget.accent.withValues(alpha: 0.45)),
      ),
      child: Row(
        children: [
          Text(_clock(shown), style: label),
          SizedBox(width: 8 * s),
          Expanded(
            child: LayoutBuilder(
              builder: (context, box) {
                void to(double dx) {
                  final v = (dx / box.maxWidth).clamp(0.0, 1.0) * dur;
                  setState(() => _dragging = v);
                }

                return GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onHorizontalDragStart: (d) => to(d.localPosition.dx),
                  onHorizontalDragUpdate: (d) => to(d.localPosition.dx),
                  onHorizontalDragEnd: (_) {
                    final v = _dragging;
                    setState(() => _dragging = null);
                    if (v != null) widget.onSeek(v);
                  },
                  onTapDown: (d) {
                    final v =
                        (d.localPosition.dx / box.maxWidth).clamp(0.0, 1.0) *
                            dur;
                    widget.onSeek(v);
                  },
                  child: SizedBox(
                    height: 30 * s,
                    child: Center(
                      child: TrackBar(
                        frac: (shown / dur).clamp(0.0, 1.0),
                        accent: widget.accent,
                        scale: s,
                      ),
                    ),
                  ),
                );
              },
            ),
          ),
          SizedBox(width: 8 * s),
          Text(_clock(widget.dur), style: label),
        ],
      ),
    );
  }
}

/// The filled bar every player draws — seek, volume, crossfade.
///
/// The fill is a gradient, not a flat colour. Slint's scrubber runs
/// `@linear-gradient(90deg, fill, fill.brighter(0.3))` and the port had one
/// flat theme colour, which on a coloured pill reads as a block rather than a
/// level. It runs the record's own accent into the section's violet, so it is
/// visibly a gradient at any width and still changes with the record.
class TrackBar extends StatelessWidget {
  const TrackBar({
    super.key,
    required this.frac,
    required this.accent,
    this.scale = 1.0,
    this.thickness = 8,
  });

  final double frac;
  final Color accent;
  final double scale;

  /// 8 at full size — double the 4 the port was drawing, and the thickness
  /// Slint's pill scrubber uses.
  final double thickness;

  @override
  Widget build(BuildContext context) {
    final h = thickness * scale;
    final r = BorderRadius.circular(h / 2);
    final f = frac.clamp(0.0, 1.0);
    return Stack(
      clipBehavior: Clip.none,
      alignment: Alignment.centerLeft,
      children: [
        Container(
          height: h,
          decoration: BoxDecoration(
            color: accent.withValues(alpha: 0.22),
            borderRadius: r,
          ),
        ),
        FractionallySizedBox(
          widthFactor: f,
          child: Container(
            height: h,
            decoration: BoxDecoration(
              gradient: LinearGradient(
                colors: [accent, Color.lerp(accent, Tokens.brand, 0.75)!],
              ),
              borderRadius: r,
              boxShadow: [
                BoxShadow(
                  color: accent.withValues(alpha: 0.5),
                  blurRadius: 5 * scale,
                ),
              ],
            ),
          ),
        ),
        Align(
          alignment: Alignment(f * 2 - 1, 0),
          child: Container(
            width: (thickness + 4) * scale,
            height: (thickness + 4) * scale,
            decoration: BoxDecoration(
              color: Colors.white,
              shape: BoxShape.circle,
              border: Border.all(color: accent, width: 2 * scale),
              boxShadow: [
                BoxShadow(
                  color: accent.withValues(alpha: 0.55),
                  blurRadius: 6 * scale,
                ),
              ],
            ),
          ),
        ),
      ],
    );
  }
}

/// Volume, with mute living inside the pill rather than beside it.
class VolPill extends StatelessWidget {
  const VolPill({
    super.key,
    required this.volume,
    required this.muted,
    required this.onVolume,
    required this.onMute,
    this.accent = Tokens.secMusic,
    this.width = 150,
    this.scale = 1.0,
  });

  final double volume;
  final bool muted;
  final ValueChanged<double> onVolume;
  final VoidCallback onMute;
  final Color accent;
  final double width;
  final double scale;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = scale;
    final tint = muted ? t.nInk2 : accent;
    return Container(
      width: width,
      height: 30 * s,
      padding: EdgeInsets.only(left: 6 * s, right: 10 * s),
      decoration: BoxDecoration(
        color: tint.withValues(alpha: 0.20),
        borderRadius: BorderRadius.circular(15 * s),
        border: Border.all(color: tint.withValues(alpha: 0.45)),
      ),
      child: Row(
        children: [
          InkResponse(
            onTap: onMute,
            radius: 14 * s,
            child: Icon(
              muted
                  ? Icons.volume_off
                  : (volume > 66
                      ? Icons.volume_up
                      : (volume > 0 ? Icons.volume_down : Icons.volume_mute)),
              size: 15 * s,
              color: muted ? t.nInk2 : accent,
            ),
          ),
          SizedBox(width: 6 * s),
          Expanded(
            // The same bar as the seek pill, not a Material `Slider`: a
            // SliderTheme cannot carry a gradient on its active track, which
            // is the whole point. 130, not 100 — mpv's softvol goes past unity
            // and quiet rips need it; the Slint build allows the same
            // headroom.
            child: LayoutBuilder(
              builder: (context, box) {
                void to(double dx) =>
                    onVolume((dx / box.maxWidth).clamp(0.0, 1.0) * 130);
                return GestureDetector(
                  behavior: HitTestBehavior.opaque,
                  onTapDown: (d) => to(d.localPosition.dx),
                  onHorizontalDragStart: (d) => to(d.localPosition.dx),
                  onHorizontalDragUpdate: (d) => to(d.localPosition.dx),
                  child: SizedBox(
                    height: 30 * s,
                    child: Center(
                      child: TrackBar(
                        frac: volume.clamp(0, 130) / 130,
                        accent: tint,
                        scale: s,
                        thickness: 6,
                      ),
                    ),
                  ),
                );
              },
            ),
          ),
          SizedBox(width: 6 * s),
          SizedBox(
            width: 26 * s,
            child: Text(
              '${volume.round()}',
              textAlign: TextAlign.right,
              style: TextStyle(
                fontSize: 10 * s,
                color: t.nInk2,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// Text that scrolls itself when it does not fit, and sits still when it does.
///
/// Titles here are song titles: eliding them hides the part that distinguishes
/// two live versions of the same song, which is exactly the part you were
/// reading.
/// The two now-playing lines, wherever they appear.
///
/// They are links. In Slint every player — the bar, the zen page, the mini —
/// wraps the title in a TouchArea that opens the album and the subtitle in one
/// that opens the artist, and both go pink under the pointer. That is the only
/// route from "what is this" to "what else is on it" that does not go through
/// a search box, and the port had it in none of the three.
///
/// The ids are not on `NowPlaying`; `MusicCmd.openNowAlbum` / `openNowArtist`
/// look them up from the deck's item. So this needs nothing but the controller.
class NowPlayingLines extends StatefulWidget {
  const NowPlayingLines({
    super.key,
    required this.controller,
    required this.now,
    required this.live,
    this.titleSize = 15,
    this.subSize = 12,
    this.centred = false,
    this.marquee = true,
  });

  final MusicController controller;
  final NowPlaying now;

  /// A radio stream's ICY title is the actual song; the station name is ours
  /// and sits on the line below.
  final bool live;

  final double titleSize;
  final double subSize;
  final bool centred;

  /// Off for the zen page, whose type is too big to scroll and whose lines
  /// elide instead.
  final bool marquee;

  @override
  State<NowPlayingLines> createState() => _NowPlayingLinesState();
}

class _NowPlayingLinesState extends State<NowPlayingLines> {
  bool _titleHover = false;
  bool _subHover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final now = widget.now;
    final c = widget.controller;
    // Only library audio has an album and an artist page to land on. A station
    // and a video do not, so there is nothing to link and no pointer change.
    final linked = now.itemId != 0 && now.mode == 'music';

    final title = widget.live && now.streamTitle.isNotEmpty
        ? now.streamTitle
        : (now.title.isEmpty ? 'Nothing playing' : now.title);
    final sub = now.title.isEmpty
        ? 'Pick a track'
        : [now.artist, now.album].where((s) => s.isNotEmpty).join('  ·  ');

    Widget line({
      required String text,
      required double size,
      required FontWeight weight,
      required Color colour,
      required bool hovered,
      required void Function(bool) onHover,
      required VoidCallback onTap,
      required double height,
    }) {
      final label = widget.marquee
          ? Marquee(
              text: text,
              speed: size >= 15 ? 34 : 26,
              style: TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: size,
                fontWeight: weight,
                color: hovered && linked ? Tokens.secMusic : colour,
              ),
            )
          : Text(
              text,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              textAlign: widget.centred ? TextAlign.center : TextAlign.start,
              style: TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: size,
                fontWeight: weight,
                color: hovered && linked ? Tokens.secMusic : colour,
              ),
            );
      final box = SizedBox(height: height, child: label);
      if (!linked) return box;
      return MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => onHover(true),
        onExit: (_) => onHover(false),
        child: GestureDetector(onTap: onTap, child: box),
      );
    }

    return Column(
      mainAxisAlignment: MainAxisAlignment.center,
      crossAxisAlignment:
          widget.centred ? CrossAxisAlignment.center : CrossAxisAlignment.start,
      children: [
        line(
          text: title,
          size: widget.titleSize,
          weight: FontWeight.w700,
          colour: t.nInk,
          hovered: _titleHover,
          onHover: (v) => setState(() => _titleHover = v),
          onTap: () => c.openNowDetail(album: true),
          height: widget.titleSize + 5,
        ),
        line(
          text: sub,
          size: widget.subSize,
          weight: FontWeight.w400,
          colour: t.nInk2,
          hovered: _subHover,
          onHover: (v) => setState(() => _subHover = v),
          onTap: () => c.openNowDetail(album: false),
          height: widget.subSize + 4,
        ),
      ],
    );
  }
}

class Marquee extends StatefulWidget {
  const Marquee({
    super.key,
    required this.text,
    required this.style,
    this.centred = false,
    this.speed = 34,
  });

  final String text;
  final TextStyle style;
  final bool centred;

  /// Pixels per second.
  final double speed;

  @override
  State<Marquee> createState() => _MarqueeState();
}

class _MarqueeState extends State<Marquee> with SingleTickerProviderStateMixin {
  late final Ticker _ticker = createTicker(_tick);
  double _offset = 0;
  double _overflow = 0;

  /// Null until the first tick. `Ticker` counts from when it started, so the
  /// first callback's elapsed time is the whole gap since then — taken as a
  /// delta it would fling the text off in one frame.
  Duration? _last;

  /// What [_overflow] was measured against. Laying out a string is one of the
  /// more expensive things in a frame and this one does not change between
  /// them: it used to be re-measured on every vsync, for both the title and
  /// the artist, for as long as anything was playing.
  String? _forText;
  TextStyle? _forStyle;
  double _forWidth = -1;

  void _tick(Duration now) {
    final last = _last;
    _last = now;
    if (last == null || _overflow <= 0) return;
    final dt = (now - last).inMicroseconds / 1e6;
    // A pause at each end: text that never stops moving is unreadable.
    final span = _overflow + 64;
    var next = _offset + widget.speed * dt;
    if (next > span) next = -32;
    setState(() => _offset = next);
  }

  /// Run the ticker only while there is something to scroll. A title that fits
  /// its box has nothing to move, and a Ticker that is merely *running* asks
  /// the engine for a frame at every vsync regardless — which is what kept the
  /// whole window rebuilding at the display's rate whenever the player bar was
  /// on screen, whether or not either line was long enough to travel.
  void _sync() {
    if (_overflow > 0) {
      if (!_ticker.isActive) {
        _last = null;
        _ticker.start();
      }
    } else if (_ticker.isActive) {
      _ticker.stop();
      _last = null;
      if (_offset != 0) setState(() => _offset = 0);
    }
  }

  void _measure(double maxWidth) {
    if (_forText == widget.text &&
        _forStyle == widget.style &&
        _forWidth == maxWidth) {
      return;
    }
    _forText = widget.text;
    _forStyle = widget.style;
    _forWidth = maxWidth;
    final painter = TextPainter(
      text: TextSpan(text: widget.text, style: widget.style),
      textDirection: TextDirection.ltr,
      maxLines: 1,
    )..layout();
    _overflow = math.max(0.0, painter.width - maxWidth);
    // Not during the build this was called from — starting a ticker schedules
    // a frame and stopping one can want a setState.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _sync();
    });
  }

  @override
  void didUpdateWidget(Marquee old) {
    super.didUpdateWidget(old);
    if (old.text != widget.text) _offset = -32;
  }

  @override
  void dispose() {
    _ticker.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, box) {
        _measure(box.maxWidth);
        final shift = _overflow <= 0 ? 0.0 : -_offset.clamp(0.0, _overflow);
        // RepaintBoundary: while the text is travelling this repaints every
        // frame, and what it sits on is the glass bar's blur and the cover
        // wash. Confine it.
        return RepaintBoundary(
          child: ClipRect(
            child: Align(
              alignment: _overflow > 0
                  ? Alignment.centerLeft
                  : (widget.centred ? Alignment.center : Alignment.centerLeft),
              child: Transform.translate(
                offset: Offset(shift, 0),
                child: Text(
                  widget.text,
                  maxLines: 1,
                  softWrap: false,
                  overflow: TextOverflow.visible,
                  style: widget.style,
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

/// The wash the current cover throws across a player surface.
BoxDecoration artWash(Color accent, {Alignment from = Alignment.centerLeft}) =>
    BoxDecoration(
      gradient: LinearGradient(
        begin: from,
        end: from == Alignment.centerLeft
            ? Alignment.centerRight
            : Alignment.bottomCenter,
        colors: [
          accent.withValues(alpha: 0.30),
          accent.withValues(alpha: 0.06),
          Colors.transparent,
        ],
        stops: const [0.0, 0.45, 0.75],
      ),
    );

String _clock(double secs) {
  if (secs.isNaN || secs.isInfinite || secs <= 0) return '0:00';
  final s = secs.round();
  final m = (s ~/ 60) % 60;
  final h = s ~/ 3600;
  final ss = (s % 60).toString().padLeft(2, '0');
  return h > 0 ? '$h:${m.toString().padLeft(2, '0')}:$ss' : '$m:$ss';
}
