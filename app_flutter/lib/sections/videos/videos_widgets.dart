// The small pieces every Videos tab is built out of: the pill tabs, the two
// button shapes, the 2:3 poster and the panels the Stream surfaces frame
// themselves with.
//
// Ported from the component block at the top of ui/page_videos.slint. The
// colours are the section's function palette rather than the theme accent —
// that is deliberate there and deliberate here: a Play button is green in all
// three themes, and white ink reads on every one of them.

import 'dart:io';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';

/// The idle fill for a pill or a ghost button, per theme.
Color _idle(Tokens t) =>
    t.dark ? const Color(0xFF2F3550) : const Color(0xFFBCB2EE);
Color _idleHi(Tokens t) =>
    t.dark ? const Color(0xFF3B4266) : const Color(0xFFA99DEA);

/// Ink on an idle fill.
Color _idleInk(Tokens t) =>
    t.dark ? const Color(0xFFECEEF8) : const Color(0xFF241D5E);

/// The tinted lettering an inactive coloured pill uses — its own hue, darkened
/// on the pale canvas so it is still legible.
Color hueInk(Tokens t, Color hue) =>
    t.dark ? hue : Color.lerp(hue, Colors.black, 0.45)!;

/// The section's poster well, which every empty artwork slot falls back to.
Color posterWell(Tokens t) => t.dark
    ? (t.oled ? const Color(0xFF101013) : const Color(0xFF2A2A2E))
    : const Color(0xFFD7D2EA);

/// A pill tab. Active fills with `hue`; inactive shows `hue` on the label.
class VideoTab extends StatefulWidget {
  const VideoTab({
    super.key,
    required this.label,
    required this.onTap,
    this.icon,
    this.active = false,
    this.hue = Tokens.secVideos,
  });

  final String label;
  final IconData? icon;
  final bool active;
  final Color hue;
  final VoidCallback onTap;

  @override
  State<VideoTab> createState() => _VideoTabState();
}

class _VideoTabState extends State<VideoTab> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final ink = widget.active ? Colors.white : hueInk(t, widget.hue);
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 140),
          height: 36,
          constraints: const BoxConstraints(minWidth: 88),
          padding: const EdgeInsets.symmetric(horizontal: 12),
          decoration: BoxDecoration(
            color:
                widget.active ? widget.hue : (_hover ? _idleHi(t) : _idle(t)),
            borderRadius: BorderRadius.circular(18),
            border: Border.all(
              color: widget.active
                  ? widget.hue
                  : widget.hue.withValues(alpha: 0.60),
            ),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              if (widget.icon != null) ...[
                Icon(widget.icon, size: 15, color: ink),
                if (widget.label.isNotEmpty) const SizedBox(width: 7),
              ],
              if (widget.label.isNotEmpty)
                Text(
                  widget.label,
                  style: TextStyle(
                      fontSize: 13, fontWeight: FontWeight.w700, color: ink),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

/// The rectangular action button. `filled` puts white ink on `hue`; otherwise
/// it is a bordered chip. `compact` drops the 120px floor for rows carrying
/// four or five actions.
class PlexButton extends StatefulWidget {
  const PlexButton({
    super.key,
    required this.label,
    required this.onTap,
    this.icon,
    this.filled = false,
    this.hue = Tokens.secVideos,
    this.compact = false,
    this.enabled = true,
  });

  final String label;
  final IconData? icon;
  final bool filled;
  final Color hue;
  final bool compact;
  final bool enabled;
  final VoidCallback onTap;

  @override
  State<PlexButton> createState() => _PlexButtonState();
}

class _PlexButtonState extends State<PlexButton> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final ink = widget.filled ? Colors.white : _idleInk(t);
    final bg = widget.filled
        ? (_hover ? Color.lerp(widget.hue, Colors.white, 0.18)! : widget.hue)
        : (_hover ? _idleHi(t) : _idle(t));
    return Opacity(
      opacity: widget.enabled ? 1 : 0.45,
      child: MouseRegion(
        cursor: widget.enabled
            ? SystemMouseCursors.click
            : SystemMouseCursors.basic,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTap: widget.enabled ? widget.onTap : null,
          child: Container(
            height: 40,
            constraints: BoxConstraints(minWidth: widget.compact ? 74 : 120),
            padding: EdgeInsets.symmetric(horizontal: widget.compact ? 8 : 4),
            decoration: BoxDecoration(
              color: bg,
              borderRadius: BorderRadius.circular(6),
              border: widget.filled
                  ? null
                  : Border.all(
                      color: t.dark
                          ? Colors.white.withValues(alpha: 0.20)
                          : const Color(0xFF241D5E).withValues(alpha: 0.42),
                    ),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                if (widget.icon != null) ...[
                  Icon(widget.icon, size: 15, color: ink),
                  SizedBox(width: widget.compact ? 5 : 7),
                ],
                Text(
                  widget.label,
                  style: TextStyle(
                      fontSize: 13, fontWeight: FontWeight.w700, color: ink),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// A round icon button, for the toolbars that have no room for a label.
class IconBtn extends StatelessWidget {
  const IconBtn({
    super.key,
    required this.icon,
    required this.onTap,
    this.tip = '',
    this.hue = Tokens.secVideos,
    this.filled = false,
    this.size = 34,
  });

  final IconData icon;
  final String tip;
  final Color hue;
  final bool filled;
  final double size;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final btn = GestureDetector(
      onTap: onTap,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        child: Container(
          width: size,
          height: size,
          decoration: BoxDecoration(
            color: filled ? hue : _idle(t),
            shape: BoxShape.circle,
            border: Border.all(color: hue.withValues(alpha: 0.6)),
          ),
          child: Icon(icon,
              size: size * 0.44, color: filled ? Colors.white : hueInk(t, hue)),
        ),
      ),
    );
    return tip.isEmpty ? btn : Tooltip(message: tip, child: btn);
  }
}

/// The sort chips above a grid: a small pill that fills with its key's colour.
class SortChip extends StatelessWidget {
  const SortChip({
    super.key,
    required this.label,
    required this.onTap,
    this.active = false,
    this.hue = Tokens.secVideos,
  });

  final String label;
  final bool active;
  final Color hue;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return GestureDetector(
      onTap: onTap,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        child: Container(
          height: 28,
          padding: const EdgeInsets.symmetric(horizontal: 12),
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: active ? hue : _idle(t),
            borderRadius: BorderRadius.circular(14),
            border: Border.all(color: hue.withValues(alpha: active ? 1 : 0.5)),
          ),
          child: Text(
            label,
            style: TextStyle(
              fontSize: 11,
              fontWeight: FontWeight.w700,
              color: active ? Colors.white : hueInk(t, hue),
            ),
          ),
        ),
      ),
    );
  }
}

/// A framed panel — the material every Stream surface sits on.
class PanelBox extends StatelessWidget {
  const PanelBox({
    super.key,
    required this.child,
    this.padding = const EdgeInsets.all(16),
    this.title,
    this.hint,
  });

  final Widget child;
  final EdgeInsets padding;
  final String? title;
  final String? hint;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: padding,
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        border: Border.all(color: t.nHair),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          if (title != null)
            Padding(
              padding: const EdgeInsets.only(bottom: 4),
              child: Text(title!,
                  style: TextStyle(
                      fontSize: 14,
                      fontWeight: FontWeight.w800,
                      color: t.nInk)),
            ),
          if (hint != null)
            Padding(
              padding: const EdgeInsets.only(bottom: 10),
              child:
                  Text(hint!, style: TextStyle(fontSize: 11, color: t.nInk2)),
            ),
          child,
        ],
      ),
    );
  }
}

/// A labelled switch row, as the Stream and Stream Plus settings use.
class StreamToggle extends StatelessWidget {
  const StreamToggle({
    super.key,
    required this.label,
    required this.value,
    required this.onChanged,
    this.hint,
  });

  final String label;
  final String? hint;
  final bool value;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      children: [
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(label,
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w600,
                      color: t.nInk)),
              if (hint != null)
                Text(hint!, style: TextStyle(fontSize: 11, color: t.nInk3)),
            ],
          ),
        ),
        Switch(
          value: value,
          activeThumbColor: Tokens.secVideos,
          onChanged: onChanged,
        ),
      ],
    );
  }
}

/// A −/value/+ stepper, for the numeric preferences.
class StepperPill extends StatelessWidget {
  const StepperPill({
    super.key,
    required this.label,
    required this.onMinus,
    required this.onPlus,
  });

  final String label;
  final VoidCallback onMinus;
  final VoidCallback onPlus;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 32,
      padding: const EdgeInsets.symmetric(horizontal: 4),
      decoration: BoxDecoration(
        color: _idle(t),
        borderRadius: BorderRadius.circular(16),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          IconButton(
            iconSize: 14,
            visualDensity: VisualDensity.compact,
            padding: EdgeInsets.zero,
            constraints: const BoxConstraints(minWidth: 26, minHeight: 26),
            onPressed: onMinus,
            icon: Icon(Icons.remove, color: _idleInk(t)),
          ),
          Text(label,
              style: TextStyle(
                  fontSize: 12,
                  fontWeight: FontWeight.w700,
                  color: _idleInk(t))),
          IconButton(
            iconSize: 14,
            visualDensity: VisualDensity.compact,
            padding: EdgeInsets.zero,
            constraints: const BoxConstraints(minWidth: 26, minHeight: 26),
            onPressed: onPlus,
            icon: Icon(Icons.add, color: _idleInk(t)),
          ),
        ],
      ),
    );
  }
}

/// A 2:3 artwork slot. `path` is a local file the bridge already cached; an
/// empty one draws the well with a film icon rather than a broken image.
class Artwork extends StatelessWidget {
  const Artwork({
    super.key,
    required this.path,
    this.width = 150,
    this.height = 225,
    this.radius = 8,
    this.fallback = Icons.movie_outlined,
  });

  final String path;
  final double width;
  final double height;
  final double radius;
  final IconData fallback;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      width: width,
      height: height,
      decoration: BoxDecoration(
        color: posterWell(t),
        borderRadius: BorderRadius.circular(radius),
      ),
      clipBehavior: Clip.antiAlias,
      child: path.isEmpty
          ? Icon(fallback, color: t.nInk3, size: width * 0.28)
          : Image.file(
              File(path),
              fit: BoxFit.cover,
              filterQuality: FilterQuality.medium,
              // A poster that vanished between the query and the paint is a
              // stale row, not a crash.
              errorBuilder: (_, __, ___) =>
                  Icon(fallback, color: t.nInk3, size: width * 0.28),
            ),
    );
  }
}

/// The small dark pill overlaid on artwork — a duration, an episode number, a
/// rating. Always white on a scrim, in every theme.
class OverlayPill extends StatelessWidget {
  const OverlayPill({
    super.key,
    required this.text,
    this.background = const Color(0xCC000000),
    this.ink = Colors.white,
    this.bold = true,
  });

  final String text;
  final Color background;
  final Color ink;
  final bool bold;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
      decoration: BoxDecoration(
        color: background,
        borderRadius: BorderRadius.circular(9),
      ),
      child: Text(
        text,
        style: TextStyle(
          fontSize: 10,
          fontWeight: bold ? FontWeight.w800 : FontWeight.w600,
          color: ink,
        ),
      ),
    );
  }
}

/// The thin bar across the bottom of a poster showing where playback got to.
class ProgressStrip extends StatelessWidget {
  const ProgressStrip(
      {super.key, required this.value, this.hue = Tokens.secVideos});

  final double value;
  final Color hue;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: 4,
      child: LinearProgressIndicator(
        value: value.clamp(0.0, 1.0),
        backgroundColor: const Color(0x66000000),
        valueColor: AlwaysStoppedAnimation(hue),
      ),
    );
  }
}

/// The section's empty state: an icon, a line, and optionally one action.
class VideosEmpty extends StatelessWidget {
  const VideosEmpty({
    super.key,
    required this.icon,
    required this.title,
    this.message = '',
    this.action,
  });

  final IconData icon;
  final String title;
  final String message;
  final Widget? action;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 44, color: t.nInk3),
          const SizedBox(height: 12),
          Text(title,
              style: TextStyle(
                  fontSize: 16, fontWeight: FontWeight.w700, color: t.nInk)),
          if (message.isNotEmpty) ...[
            const SizedBox(height: 6),
            SizedBox(
              width: 420,
              child: Text(message,
                  textAlign: TextAlign.center,
                  style: TextStyle(fontSize: 12, color: t.nInk2)),
            ),
          ],
          if (action != null) ...[const SizedBox(height: 16), action!],
        ],
      ),
    );
  }
}

/// The rounded search field the section uses in five places.
class VideoSearchField extends StatelessWidget {
  const VideoSearchField({
    super.key,
    required this.controller,
    required this.onChanged,
    this.onSubmitted,
    this.hint = 'Search',
    this.width = 360,
    this.large = false,
  });

  final TextEditingController controller;
  final ValueChanged<String> onChanged;
  final ValueChanged<String>? onSubmitted;
  final String hint;
  final double width;
  final bool large;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      width: width,
      height: large ? 52 : 44,
      child: TextField(
        controller: controller,
        onChanged: onChanged,
        onSubmitted: onSubmitted,
        style: TextStyle(fontSize: large ? 16 : 14, color: t.nInk),
        decoration: InputDecoration(
          isDense: true,
          filled: true,
          fillColor: t.dark
              ? (t.oled ? const Color(0xFF121420) : const Color(0xFF20233A))
              : Colors.white,
          hintText: hint,
          hintStyle: TextStyle(fontSize: large ? 15 : 13, color: t.nInk3),
          prefixIcon: Icon(Icons.search, size: large ? 20 : 16, color: t.nInk2),
          suffixIcon: controller.text.isEmpty
              ? null
              : IconButton(
                  iconSize: 14,
                  icon: Icon(Icons.close, color: t.nInk2),
                  onPressed: () {
                    controller.clear();
                    onChanged('');
                  },
                ),
          contentPadding: EdgeInsets.symmetric(vertical: large ? 14 : 10),
          border: OutlineInputBorder(
            borderRadius: BorderRadius.circular(large ? 26 : 22),
            borderSide: BorderSide(color: t.nHair),
          ),
          enabledBorder: OutlineInputBorder(
            borderRadius: BorderRadius.circular(large ? 26 : 22),
            borderSide: BorderSide(color: t.nHair),
          ),
          focusedBorder: OutlineInputBorder(
            borderRadius: BorderRadius.circular(large ? 26 : 22),
            borderSide: const BorderSide(color: Tokens.secVideos),
          ),
        ),
      ),
    );
  }
}
