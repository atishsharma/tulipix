// Settings' shared pieces: the page head, the tile, the row, and the handful
// of controls every tab draws its settings with
// (docs/mockups/settings-tabs.html).
//
// A tab looks its rows up by key and lays them into tiles. Whatever keyed row
// it does not place still shows, in a More tile at the foot, so a setting
// added in Rust is never lost to a layout that has not heard of it yet.

import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/settings.dart';
import 'settings_controller.dart';

/// A URL in the default browser, or on the clipboard when there is no opener
/// to run.
Future<void> openExternal(BuildContext context, String url) async {
  try {
    if (Platform.isWindows) {
      await Process.start('cmd', ['/c', 'start', '', url],
          mode: ProcessStartMode.detached);
    } else {
      await Process.start(Platform.isMacOS ? 'open' : 'xdg-open', [url],
          mode: ProcessStartMode.detached);
    }
  } catch (_) {
    await Clipboard.setData(ClipboardData(text: url));
    if (context.mounted) {
      ScaffoldMessenger.maybeOf(context)?.showSnackBar(
        SnackBar(content: Text('Could not open a browser. $url copied')),
      );
    }
  }
}

/// "18 KB", "1.2 GB".
String humanBytes(num b) {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  var v = b.toDouble();
  var i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return '${v >= 10 || i == 0 ? v.round() : v.toStringAsFixed(1)} ${units[i]}';
}

/// A home-relative path the way a person writes it.
String tildePath(String p) {
  final home = Platform.environment['HOME'] ?? '';
  return home.isNotEmpty && p.startsWith(home) ? '~${p.substring(home.length)}' : p;
}

/// One tab's rows, looked up by key — or by label, for the keyless readings.
/// Every key asked for is remembered, and [rest] is what nobody asked for.
class Rows {
  Rows(this.all);

  final List<SettingItem> all;
  final Set<String> _used = {};

  SettingItem? key(String k) {
    _used.add(k);
    for (final r in all) {
      if (r.key == k) return r;
    }
    return null;
  }

  SettingItem? label(String l) {
    for (final r in all) {
      if (r.label == l) return r;
    }
    return null;
  }

  /// Rows the tab draws some other way (its head's button, a model card).
  void use(Iterable<String> keys) => _used.addAll(keys);

  /// Only meaningful once the tab has asked for every key it places.
  List<SettingItem> get rest => [
        for (final r in all)
          if (r.key.isNotEmpty && r.kind != 'header' && !_used.contains(r.key))
            r,
      ];
}

// ── the page ────────────────────────────────────────────────────────────────

/// A tab's scrolling page: its head, then its blocks, on the same gutter as
/// You & Home.
class SettingsPageBody extends StatelessWidget {
  const SettingsPageBody({super.key, required this.head, required this.children});

  final Widget head;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
        builder: (context, box) {
          final g = math.max(16.0, box.maxWidth * 0.02);
          return ListView(
            padding: EdgeInsets.fromLTRB(g, 18, g, 28),
            children: [
              head,
              for (final c in children) ...[const SizedBox(height: 14), c],
            ],
          );
        },
      );
}

/// The tab's glyph, name and one line, with its own actions on the right.
class SettingsHead extends StatelessWidget {
  const SettingsHead({
    super.key,
    required this.icon,
    required this.tint,
    required this.title,
    required this.note,
    this.tint2,
    this.actions = const [],
  });

  /// Name, glyph and colour from the rail's list, so the two never disagree.
  factory SettingsHead.forTab(
    String id, {
    required String note,
    IconData? icon,
    Color? tint2,
    List<Widget> actions = const [],
  }) {
    final tab = kSettingsTabs.firstWhere((x) => x.id == id);
    return SettingsHead(
      icon: icon ?? tab.icon,
      tint: tab.tint,
      title: tab.label,
      note: note,
      tint2: tint2,
      actions: actions,
    );
  }

  final IconData icon;
  final Color tint;

  /// The gradient's far end. Defaults to [tint] sunk toward ink.
  final Color? tint2;
  final String title;
  final String note;
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final skinned = context.skin.control(active: true, tint: tint, radius: 13);
    return Row(
      children: [
        Container(
          width: 42,
          height: 42,
          decoration: skinned ??
              BoxDecoration(
                gradient: LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: [
                    tint,
                    tint2 ?? Color.lerp(tint, const Color(0xFF1A1033), 0.55)!,
                  ],
                ),
                borderRadius: const BorderRadius.all(Radius.circular(13)),
              ),
          child: Icon(context.skin.icon(icon),
              size: 21, color: skinned == null ? Colors.white : tint),
        ),
        const SizedBox(width: 12),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(title,
                  style: TextStyle(
                      fontSize: 22, fontWeight: FontWeight.w800, color: t.text)),
              Text(note, style: TextStyle(fontSize: 12.5, color: t.textDim)),
            ],
          ),
        ),
        for (final a in actions) ...[const SizedBox(width: 8), a],
      ],
    );
  }
}

/// One tile: a tinted glyph, a title and a line under it, whatever sits at the
/// right of the header, then the controls. Every block on every tab is this
/// shape, which is what makes a grid of them read as one panel.
class SettingsTile extends StatelessWidget {
  const SettingsTile({
    super.key,
    required this.icon,
    required this.tint,
    required this.title,
    required this.note,
    this.child,
    this.trailing = const [],
    this.fill = false,
    this.danger = false,
  });

  final IconData icon;
  final Color tint;
  final String title;
  final String note;

  /// Null for a tile that is all header (a folded section).
  final Widget? child;
  final List<Widget> trailing;

  /// The controls take the tile's remaining height. Needs a bounded height.
  final bool fill;

  /// The edge in the error colour: this tile destroys something.
  final bool danger;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final body = child;
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: context.skin.surface(SurfaceRole.card, radius: 18) ??
          BoxDecoration(
            color: t.panel2,
            borderRadius: BorderRadius.circular(18),
            border: Border.all(
                color: danger
                    ? Tokens.error.withValues(alpha: 0.35)
                    : t.outline),
          ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Container(
                width: 34,
                height: 34,
                decoration: context.skin
                        .control(active: true, tint: tint, radius: 11) ??
                    BoxDecoration(
                      color: tint.withValues(alpha: 0.16),
                      borderRadius: BorderRadius.circular(11),
                    ),
                child: Icon(context.skin.icon(icon), size: 17, color: tint),
              ),
              const SizedBox(width: 11),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 14,
                            fontWeight: FontWeight.w700,
                            color: t.text)),
                    // An empty note is no line, not a blank one.
                    if (note.isNotEmpty)
                      Text(note,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 11.5, color: t.textDim)),
                  ],
                ),
              ),
              for (final w in trailing) ...[const SizedBox(width: 8), w],
            ],
          ),
          if (body != null) ...[
            const SizedBox(height: 14),
            if (fill) Expanded(child: body) else body,
          ],
        ],
      ),
    );
  }
}

/// A tile and how many of the grid's six columns it takes.
typedef TileSpan = ({int span, Widget child});

/// Tiles on six columns, packed into rows left to right; every row is as tall
/// as its tallest tile. One column on a narrow window.
///
/// Rows measure with [IntrinsicHeight], so nothing inside a tile may use a
/// LayoutBuilder.
class TileGrid extends StatelessWidget {
  const TileGrid(this.tiles, {super.key});

  final List<TileSpan> tiles;

  static const double gap = 14;

  static int _sum(List<TileSpan> r) => r.fold(0, (a, x) => a + x.span);

  @override
  Widget build(BuildContext context) => LayoutBuilder(
        builder: (context, box) {
          if (box.maxWidth < 760) {
            return Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (var i = 0; i < tiles.length; i++) ...[
                  if (i > 0) const SizedBox(height: gap),
                  // Measured here too, so a tile that fills its height has
                  // one to fill at every width.
                  IntrinsicHeight(child: tiles[i].child),
                ],
              ],
            );
          }
          final rows = <List<TileSpan>>[];
          var filled = 6;
          for (final x in tiles) {
            if (filled + x.span > 6) {
              rows.add([]);
              filled = 0;
            }
            rows.last.add(x);
            filled += x.span;
          }
          return Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (var r = 0; r < rows.length; r++) ...[
                if (r > 0) const SizedBox(height: gap),
                IntrinsicHeight(
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      for (var i = 0; i < rows[r].length; i++) ...[
                        if (i > 0) const SizedBox(width: gap),
                        Expanded(flex: rows[r][i].span, child: rows[r][i].child),
                      ],
                      // A row its tiles do not fill keeps their widths rather
                      // than stretching them across the gap.
                      if (_sum(rows[r]) < 6) Spacer(flex: 6 - _sum(rows[r])),
                    ],
                  ),
                ),
              ],
            ],
          );
        },
      );
}

/// The figures across the top of a tab: a small caps label, the number, and
/// what the number means.
class StatStrip extends StatelessWidget {
  const StatStrip(this.stats, {super.key});

  final List<({String label, String value, String note})> stats;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget card(({String label, String value, String note}) s) => Container(
          padding: const EdgeInsets.fromLTRB(16, 14, 16, 14),
          decoration: context.skin.surface(SurfaceRole.card, radius: 16) ??
              BoxDecoration(
                color: t.panel2,
                borderRadius: BorderRadius.circular(16),
                border: Border.all(color: t.outline),
              ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(s.label.toUpperCase(),
                  style: TextStyle(
                      fontSize: 10.5,
                      fontWeight: FontWeight.w600,
                      letterSpacing: 0.6,
                      color: t.textDim)),
              const SizedBox(height: 3),
              Text(s.value,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 22,
                      fontWeight: FontWeight.w800,
                      color: t.text,
                      fontFeatures: const [FontFeature.tabularFigures()])),
              Text(s.note,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11.5, color: t.textDim)),
            ],
          ),
        );
    return LayoutBuilder(
      builder: (context, box) {
        final per = box.maxWidth >= 700 ? stats.length : 2;
        return Column(
          children: [
            for (var i = 0; i < stats.length; i += per) ...[
              if (i > 0) const SizedBox(height: 14),
              IntrinsicHeight(
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    for (var j = i; j < math.min(i + per, stats.length); j++) ...[
                      if (j > i) const SizedBox(width: 14),
                      Expanded(child: card(stats[j])),
                    ],
                  ],
                ),
              ),
            ],
          ],
        );
      },
    );
  }
}

// ── rows ────────────────────────────────────────────────────────────────────

/// Rows one under the other with a hairline between each.
class Lines extends StatelessWidget {
  const Lines(this.children, {super.key});

  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (var i = 0; i < children.length; i++) ...[
          if (i > 0) Divider(height: 1, thickness: 1, color: t.outline),
          children[i],
        ],
      ],
    );
  }
}

/// A title, a line under it, and a control — at the right, or with [below]
/// under the words where the control needs the width.
class SettingLine extends StatelessWidget {
  const SettingLine({
    super.key,
    required this.title,
    this.note = '',
    this.trailing,
    this.below = false,
  });

  final String title;
  final String note;
  final Widget? trailing;
  final bool below;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final words = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(title,
            style: TextStyle(
                fontSize: 13, fontWeight: FontWeight.w600, color: t.text)),
        if (note.isNotEmpty) ...[
          const SizedBox(height: 2),
          Text(note, style: TextStyle(fontSize: 11.5, color: t.textDim)),
        ],
      ],
    );
    final c = trailing;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 10),
      child: below
          ? Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [words, if (c != null) ...[const SizedBox(height: 9), c]],
            )
          : Row(
              children: [
                Expanded(child: words),
                if (c != null) ...[const SizedBox(width: 14), c],
              ],
            ),
    );
  }
}

/// A Rust row as a [SettingLine], with the control its kind calls for.
class RowLine extends StatelessWidget {
  const RowLine({
    super.key,
    required this.row,
    required this.controller,
    this.below = false,
  });

  final SettingItem row;
  final SettingsController controller;
  final bool below;

  @override
  Widget build(BuildContext context) => SettingLine(
        title: row.label,
        note: row.desc,
        below: below,
        trailing: RowControl(row: row, controller: controller, below: below),
      );
}

/// The six kinds are the whole vocabulary: a switch, a text box, a choice, a
/// button, a reading, or a reading with its button.
class RowControl extends StatelessWidget {
  const RowControl({
    super.key,
    required this.row,
    required this.controller,
    this.below = false,
  });

  final SettingItem row;
  final SettingsController controller;
  final bool below;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final r = row;
    final c = controller;
    return switch (r.kind) {
      'toggle' => SettingSwitch(
          on: r.on_,
          onChanged: (v) => c.send(SettingsCmd.toggle(key: r.key, on_: v)),
        ),
      'text' => FieldBox(
          value: r.value,
          width: below ? null : 240,
          onSubmit: (v) => c.send(SettingsCmd.setText(key: r.key, value: v)),
        ),
      'choice' => Seg(
          options: r.options,
          value: r.value,
          onPick: (v) => c.send(SettingsCmd.setText(key: r.key, value: v)),
        ),
      'action' => SmallBtn(
          label: r.value,
          busy: r.state == 'busy',
          onTap: () => c.sendAction(r.key),
        ),
      'status-action' => Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            StateChip(r.value, tint: stateTint(r.state, t)),
            const SizedBox(width: 10),
            SmallBtn(
              label: r.btn,
              onTap: r.state == 'busy' ? null : () => c.sendAction(r.key),
            ),
          ],
        ),
      _ => StateChip(r.value, tint: stateTint(r.state, t)),
    };
  }
}

/// Keyed rows the tab did not place, so none of them go missing.
class MoreTile extends StatelessWidget {
  const MoreTile({super.key, required this.rows, required this.controller});

  final List<SettingItem> rows;
  final SettingsController controller;

  @override
  Widget build(BuildContext context) => SettingsTile(
        icon: Icons.tune,
        tint: Tokens.secSettings,
        title: 'More',
        note: 'Settings this page does not group yet',
        child: Lines([
          for (final r in rows) RowLine(row: r, controller: controller),
        ]),
      );
}

// ── controls ────────────────────────────────────────────────────────────────

class SettingSwitch extends StatelessWidget {
  const SettingSwitch({super.key, required this.on, required this.onChanged});

  final bool on;
  final ValueChanged<bool>? onChanged;

  @override
  Widget build(BuildContext context) => Switch(
        value: on,
        onChanged: onChanged,
        materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
        activeThumbColor: Colors.white,
        activeTrackColor: Tokens.brand,
      );
}

/// A short list of choices as one row of buttons. [full] spreads them over
/// the width; otherwise they wrap.
class Seg extends StatelessWidget {
  const Seg({
    super.key,
    required this.options,
    required this.value,
    required this.onPick,
    this.labels,
    this.full = false,
  });

  final List<String> options;
  final String value;
  final ValueChanged<String> onPick;

  /// What each option reads as, when that is not the option itself.
  final List<String>? labels;
  final bool full;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget button(int i) {
      final active = options[i] == value;
      return Material(
        color: active ? t.panel : Colors.transparent,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(7),
          side: BorderSide(
              color: active ? t.outlineStrong : Colors.transparent),
        ),
        child: InkWell(
          borderRadius: BorderRadius.circular(7),
          onTap: active ? null : () => onPick(options[i]),
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
            child: Text(labels?[i] ?? options[i],
                textAlign: TextAlign.center,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                    color: active ? t.text : t.textDim)),
          ),
        ),
      );
    }

    return Container(
      padding: const EdgeInsets.all(3),
      decoration: BoxDecoration(
        color: t.bg,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: t.outline),
      ),
      child: full
          ? Row(
              children: [
                for (var i = 0; i < options.length; i++) ...[
                  if (i > 0) const SizedBox(width: 2),
                  Expanded(child: button(i)),
                ],
              ],
            )
          : Wrap(
              spacing: 2,
              runSpacing: 2,
              children: [for (var i = 0; i < options.length; i++) button(i)],
            ),
    );
  }
}

class SmallBtn extends StatelessWidget {
  const SmallBtn({
    super.key,
    required this.label,
    this.onTap,
    this.icon,
    this.primary = false,
    this.danger = false,
    this.ghost = false,
    this.busy = false,
    this.large = false,
    this.tooltip,
  });

  final String label;

  /// Null draws it disabled.
  final VoidCallback? onTap;
  final IconData? icon;
  final bool primary;
  final bool danger;
  final bool ghost;

  /// A spinner in place of the icon, and no taps.
  final bool busy;

  /// Head-of-page size.
  final bool large;
  final String? tooltip;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final ink = primary
        ? Colors.white
        : danger
            ? Tokens.error
            : ghost
                ? t.textDim
                : t.text;
    final r = BorderRadius.circular(large ? 10 : 8);
    final enabled = onTap != null && !busy;
    final b = Opacity(
      opacity: enabled || busy ? 1 : 0.45,
      child: Material(
        color: primary
            ? Tokens.brand
            : danger
                ? Tokens.error.withValues(alpha: 0.08)
                : ghost
                    ? Colors.transparent
                    : t.bg,
        shape: RoundedRectangleBorder(
          borderRadius: r,
          side: BorderSide(
            color: primary
                ? Tokens.brand
                : danger
                    ? Tokens.error.withValues(alpha: 0.45)
                    : ghost
                        ? Colors.transparent
                        : t.outlineStrong,
          ),
        ),
        child: InkWell(
          borderRadius: r,
          onTap: enabled ? onTap : null,
          child: Container(
            height: large ? 34 : 28,
            padding: EdgeInsets.symmetric(horizontal: large ? 13 : 10),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (busy)
                  SizedBox(
                    width: 12,
                    height: 12,
                    child: CircularProgressIndicator(strokeWidth: 2, color: ink),
                  )
                else if (icon != null)
                  Icon(icon, size: 14, color: ink),
                if (busy || icon != null) const SizedBox(width: 7),
                Text(label,
                    style: TextStyle(
                        fontSize: large ? 12.5 : 11.5,
                        fontWeight: FontWeight.w600,
                        color: ink)),
              ],
            ),
          ),
        ),
      ),
    );
    final tip = tooltip;
    return tip == null ? b : Tooltip(message: tip, child: b);
  }
}

/// A reading as a pill with a dot: Installed, Missing, Set.
class StateChip extends StatelessWidget {
  const StateChip(this.label, {super.key, this.tint});

  final String label;

  /// Null is the muted grey.
  final Color? tint;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final k = tint ?? t.textDim;
    return Container(
      height: 22,
      padding: const EdgeInsets.symmetric(horizontal: 8),
      decoration: BoxDecoration(
        color: k.withValues(alpha: 0.14),
        borderRadius: BorderRadius.circular(11),
        border: Border.all(color: k.withValues(alpha: 0.35)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 6,
            height: 6,
            decoration: BoxDecoration(color: k, shape: BoxShape.circle),
          ),
          const SizedBox(width: 6),
          Text(label,
              style: TextStyle(
                  fontSize: 11, fontWeight: FontWeight.w600, color: k)),
        ],
      ),
    );
  }
}

/// A row's `state` as a colour: "ok" | "warn" | "error" | "busy" | muted.
Color stateTint(String state, Tokens t) => switch (state) {
      'ok' => Tokens.ok,
      'warn' => Tokens.warn,
      'error' => Tokens.error,
      'busy' => Tokens.brand,
      _ => t.textDim,
    };

/// A text box that writes on Enter or on its Save button — never per
/// keystroke, which would be a settings-file write each time. [secret] masks
/// it behind an eye.
class FieldBox extends StatefulWidget {
  const FieldBox({
    super.key,
    required this.value,
    required this.onSubmit,
    this.hint = '',
    this.secret = false,
    this.width,
    this.trailing = const [],
  });

  final String value;
  final ValueChanged<String> onSubmit;
  final String hint;
  final bool secret;

  /// Null takes the width it is given; it must be given one.
  final double? width;
  final List<Widget> trailing;

  @override
  State<FieldBox> createState() => _FieldBoxState();
}

class _FieldBoxState extends State<FieldBox> {
  late final TextEditingController _text =
      TextEditingController(text: widget.value);
  final FocusNode _focus = FocusNode();
  bool _shown = false;

  @override
  void initState() {
    super.initState();
    _text.addListener(_reread);
    _focus.addListener(_reread);
  }

  void _reread() {
    if (mounted) setState(() {});
  }

  @override
  void didUpdateWidget(FieldBox old) {
    super.didUpdateWidget(old);
    // A refresh that did not change this value must not move the caret out
    // from under someone still typing in it.
    if (widget.value != old.value && widget.value != _text.text) {
      _text.text = widget.value;
    }
  }

  @override
  void dispose() {
    _text.dispose();
    _focus.dispose();
    super.dispose();
  }

  bool get _dirty => _text.text.trim() != widget.value.trim();

  void _submit() {
    if (_dirty) widget.onSubmit(_text.text.trim());
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      width: widget.width,
      height: 36,
      padding: const EdgeInsets.fromLTRB(11, 0, 4, 0),
      decoration: BoxDecoration(
        color: t.bg,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(
            color: _focus.hasFocus ? Tokens.brand : t.outlineStrong),
      ),
      child: Row(
        children: [
          Expanded(
            child: TextField(
              controller: _text,
              focusNode: _focus,
              obscureText: widget.secret && !_shown,
              onSubmitted: (_) => _submit(),
              style: TextStyle(fontSize: 12.5, color: t.text),
              decoration: InputDecoration.collapsed(
                hintText: widget.hint,
                hintStyle: TextStyle(fontSize: 12.5, color: t.textDim),
              ),
            ),
          ),
          if (widget.secret)
            IconButton(
              tooltip: _shown ? 'Hide' : 'Show',
              visualDensity: VisualDensity.compact,
              iconSize: 16,
              onPressed: () => setState(() => _shown = !_shown),
              icon: Icon(
                  _shown
                      ? Icons.visibility_off_outlined
                      : Icons.visibility_outlined,
                  color: t.textDim),
            ),
          if (_dirty) ...[
            const SizedBox(width: 4),
            SmallBtn(label: 'Save', primary: true, onTap: _submit),
          ],
          for (final w in widget.trailing) ...[const SizedBox(width: 4), w],
        ],
      ),
    );
  }
}
