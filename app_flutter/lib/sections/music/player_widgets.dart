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
    final button = SizedBox(
      width: size,
      height: size,
      child: Material(
        color: active ? accent.withValues(alpha: 0.18) : Colors.transparent,
        shape: const CircleBorder(),
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
      height: 26 * s,
      padding: EdgeInsets.symmetric(horizontal: 10 * s),
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(13 * s),
        border: Border.all(color: t.nHair),
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
                    height: 26 * s,
                    child: Center(
                      child: Stack(
                        clipBehavior: Clip.none,
                        alignment: Alignment.centerLeft,
                        children: [
                          Container(
                            height: 4 * s,
                            decoration: BoxDecoration(
                              color: t.nHover,
                              borderRadius: BorderRadius.circular(2 * s),
                            ),
                          ),
                          FractionallySizedBox(
                            widthFactor: (shown / dur).clamp(0.0, 1.0),
                            child: Container(
                              height: 4 * s,
                              decoration: BoxDecoration(
                                gradient: LinearGradient(colors: [
                                  widget.accent.withValues(alpha: 0.65),
                                  widget.accent,
                                ]),
                                borderRadius: BorderRadius.circular(2 * s),
                              ),
                            ),
                          ),
                          Align(
                            alignment: Alignment(
                              ((shown / dur).clamp(0.0, 1.0)) * 2 - 1,
                              0,
                            ),
                            child: Container(
                              width: 10 * s,
                              height: 10 * s,
                              decoration: BoxDecoration(
                                color: widget.accent,
                                shape: BoxShape.circle,
                                boxShadow: [
                                  BoxShadow(
                                    color:
                                        widget.accent.withValues(alpha: 0.55),
                                    blurRadius: 6 * s,
                                  ),
                                ],
                              ),
                            ),
                          ),
                        ],
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
    return Container(
      width: width,
      height: 26 * s,
      padding: EdgeInsets.only(left: 4 * s, right: 10 * s),
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(13 * s),
        border: Border.all(color: t.nHair),
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
          SizedBox(width: 2 * s),
          Expanded(
            child: SliderTheme(
              data: SliderTheme.of(context).copyWith(
                trackHeight: 3 * s,
                thumbShape: RoundSliderThumbShape(enabledThumbRadius: 5 * s),
                overlayShape: RoundSliderOverlayShape(overlayRadius: 10 * s),
                activeTrackColor: muted ? t.nInk2 : accent,
                inactiveTrackColor: t.nHover,
                thumbColor: muted ? t.nInk2 : accent,
              ),
              // 130, not 100: mpv's softvol goes past unity and quiet rips need
              // it. The Slint build allows the same headroom.
              child: Slider(
                value: volume.clamp(0, 130),
                max: 130,
                onChanged: onVolume,
              ),
            ),
          ),
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
  late final Ticker _ticker;
  double _offset = 0;
  double _overflow = 0;
  Duration _last = Duration.zero;

  @override
  void initState() {
    super.initState();
    _ticker = createTicker(_tick)..start();
  }

  void _tick(Duration now) {
    if (_overflow <= 0) {
      if (_offset != 0) setState(() => _offset = 0);
      _last = now;
      return;
    }
    final dt = (now - _last).inMicroseconds / 1e6;
    _last = now;
    // A pause at each end: text that never stops moving is unreadable.
    final span = _overflow + 64;
    var next = _offset + widget.speed * dt;
    if (next > span) next = -32;
    setState(() => _offset = next);
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
        final painter = TextPainter(
          text: TextSpan(text: widget.text, style: widget.style),
          textDirection: TextDirection.ltr,
          maxLines: 1,
        )..layout();
        _overflow = math.max(0, painter.width - box.maxWidth);
        final shift = _overflow <= 0 ? 0.0 : -_offset.clamp(0.0, _overflow);
        return ClipRect(
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
