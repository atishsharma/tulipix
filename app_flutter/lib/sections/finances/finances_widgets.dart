// The pieces every Finances tab is built out of.
//
// Seven tabs share a stat strip, a card, a filter row and a table shell. In the
// Slint build those were seven near-identical component declarations; here they
// are one each, which is most of why the port is shorter than the original.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/finances.dart';
import 'finances_controller.dart';

/// The panel every block on every tab sits in.
class FinCard extends StatelessWidget {
  const FinCard({
    super.key,
    required this.child,
    this.title = '',
    this.sub = '',
    this.trailing,
    this.padding = const EdgeInsets.all(16),
  });

  final Widget child;
  final String title;
  final String sub;
  final Widget? trailing;
  final EdgeInsets padding;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        border: Border.all(color: t.nHair),
      ),
      padding: padding,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          if (title.isNotEmpty || trailing != null) ...[
            Row(
              children: [
                if (title.isNotEmpty)
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Text(title,
                            style: TextStyle(
                                fontSize: 14,
                                fontWeight: FontWeight.w700,
                                color: t.nInk)),
                        if (sub.isNotEmpty)
                          Padding(
                            padding: const EdgeInsets.only(top: 2),
                            child: Text(sub,
                                style: TextStyle(fontSize: 11, color: t.nInk2)),
                          ),
                      ],
                    ),
                  )
                else
                  const Spacer(),
                if (trailing != null) trailing!,
              ],
            ),
            const SizedBox(height: 12),
          ],
          child,
        ],
      ),
    );
  }
}

/// One figure with its label, and — when the section has something to say about
/// it — a subtitle and a delta. `action` is what clicking it does, and an empty
/// action makes the card inert rather than a button that goes nowhere.
class StatCard extends StatelessWidget {
  const StatCard({super.key, required this.stat, this.onTap, this.width = 190});

  final FinStat stat;
  final VoidCallback? onTap;
  final double width;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tone = toneColour(context, stat.tone);
    final card = Container(
      width: width,
      padding: const EdgeInsets.fromLTRB(14, 12, 14, 12),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        border: Border.all(color: t.nHair),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(stat.label.toUpperCase(),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 10,
                  letterSpacing: 0.6,
                  fontWeight: FontWeight.w600,
                  color: t.nInk2)),
          const SizedBox(height: 6),
          Text(stat.value,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 20, fontWeight: FontWeight.w800, color: tone)),
          if (stat.sub.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Text(stat.sub,
                  maxLines: 2, style: TextStyle(fontSize: 11, color: t.nInk2)),
            ),
          if (stat.delta.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(stat.deltaUp ? Icons.arrow_upward : Icons.arrow_downward,
                      size: 12, color: stat.deltaUp ? Tokens.error : Tokens.ok),
                  const SizedBox(width: 3),
                  Text(stat.delta,
                      style: TextStyle(
                          fontSize: 11,
                          color: stat.deltaUp ? Tokens.error : Tokens.ok)),
                ],
              ),
            ),
        ],
      ),
    );
    if (onTap == null || stat.action.isEmpty) return card;
    return InkWell(
      borderRadius: BorderRadius.circular(Tokens.radiusMd),
      onTap: onTap,
      child: card,
    );
  }
}

/// A row of stat cards that wraps rather than scrolls: on a narrow window the
/// figures should stack, not hide behind a scrollbar nobody looks for.
class StatStrip extends StatelessWidget {
  const StatStrip({super.key, required this.stats, this.onAction});

  final List<FinStat> stats;
  final void Function(String action)? onAction;

  @override
  Widget build(BuildContext context) {
    if (stats.isEmpty) return const SizedBox.shrink();
    return Wrap(
      spacing: 10,
      runSpacing: 10,
      children: [
        for (final s in stats)
          StatCard(
            stat: s,
            onTap: s.action.isEmpty || onAction == null
                ? null
                : () => onAction!(s.action),
          ),
      ],
    );
  }
}

/// A label-and-figure list: the cash-flow walk, the month statement, the
/// position summary. One widget rather than four near-identical ones.
class KvList extends StatelessWidget {
  const KvList({super.key, required this.rows, this.dense = false});

  final List<FinKv> rows;
  final bool dense;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final r in rows)
          Padding(
            padding: EdgeInsets.symmetric(vertical: dense ? 3 : 5),
            child: Row(
              children: [
                Expanded(
                  child: Text(r.label,
                      style: TextStyle(
                          fontSize: 12,
                          fontWeight:
                              r.strong ? FontWeight.w700 : FontWeight.w400,
                          color: r.strong ? t.nInk : t.nInk2)),
                ),
                Text(r.value,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight:
                            r.strong ? FontWeight.w800 : FontWeight.w600,
                        color: toneColour(context, r.tone))),
              ],
            ),
          ),
      ],
    );
  }
}

/// A filter chip that carries its own count. Counts live on the filter row
/// rather than on the tabs: a number on a tab is a number you read past to find
/// the name of the thing you wanted.
class CountChip extends StatelessWidget {
  const CountChip({
    super.key,
    required this.label,
    required this.count,
    required this.active,
    required this.onTap,
    this.tint,
  });

  final String label;
  final int count;
  final bool active;
  final VoidCallback onTap;
  final Color? tint;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = tint ?? Tokens.secFinances;
    return InkWell(
      borderRadius: BorderRadius.circular(999),
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
        decoration: BoxDecoration(
          color: active ? c.withValues(alpha: 0.16) : t.nChip,
          borderRadius: BorderRadius.circular(999),
          border: Border.all(color: active ? c : t.nHair),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(label,
                style: TextStyle(
                    fontSize: 12,
                    fontWeight: active ? FontWeight.w700 : FontWeight.w500,
                    color: active ? c : t.nInk2)),
            if (count >= 0) ...[
              const SizedBox(width: 6),
              Text('$count',
                  style: TextStyle(
                      fontSize: 11,
                      fontWeight: FontWeight.w700,
                      color: active ? c : t.nInk3)),
            ],
          ],
        ),
      ),
    );
  }
}

/// A coloured disc with a letter in it. Accounts, loans and recurrences all get
/// one, because four accounts of the same kind would otherwise get four
/// identical icons.
class HueBadge extends StatelessWidget {
  const HueBadge({
    super.key,
    required this.text,
    required this.argb,
    this.size = 34,
  });

  final String text;
  final int argb;
  final double size;

  @override
  Widget build(BuildContext context) {
    final c = hue(argb);
    return Container(
      width: size,
      height: size,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: c.withValues(alpha: 0.18),
        borderRadius: BorderRadius.circular(size / 3),
        border: Border.all(color: c.withValues(alpha: 0.55)),
      ),
      child: Text(text,
          style: TextStyle(
              fontSize: size * 0.4, fontWeight: FontWeight.w800, color: c)),
    );
  }
}

/// A small dot in a category's colour, next to the category's name.
class CatDot extends StatelessWidget {
  const CatDot({super.key, required this.argb, this.size = 8});

  final int argb;
  final double size;

  @override
  Widget build(BuildContext context) => Container(
        width: size,
        height: size,
        decoration: BoxDecoration(color: hue(argb), shape: BoxShape.circle),
      );
}

/// A proportion bar. Over-budget draws past its own track in the error colour
/// rather than clamping, because a clamped bar says "exactly full" for both
/// "on budget to the rupee" and "double it".
class MeterBar extends StatelessWidget {
  const MeterBar({
    super.key,
    required this.pct,
    this.tint,
    this.over = false,
    this.height = 6,
    this.ghostPct = -1,
  });

  final int pct;
  final Color? tint;
  final bool over;
  final double height;

  /// A second, paler mark on the same track — where the month is projected to
  /// land, against where it is now.
  final int ghostPct;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = over ? Tokens.error : (tint ?? Tokens.secFinances);
    return LayoutBuilder(
      builder: (context, box) {
        final w = box.maxWidth;
        return SizedBox(
          height: height,
          child: Stack(
            children: [
              Container(
                decoration: BoxDecoration(
                  color: t.nHair,
                  borderRadius: BorderRadius.circular(height),
                ),
              ),
              if (ghostPct >= 0)
                Container(
                  width: w * (ghostPct.clamp(0, 100)) / 100,
                  decoration: BoxDecoration(
                    color: c.withValues(alpha: 0.28),
                    borderRadius: BorderRadius.circular(height),
                  ),
                ),
              Container(
                width: w * (pct.clamp(0, 100)) / 100,
                decoration: BoxDecoration(
                  color: c,
                  borderRadius: BorderRadius.circular(height),
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

/// What a tab shows when its list is empty. A designed answer, not a blank
/// panel: an empty state that looks like a bug gets reported as one.
class EmptyNote extends StatelessWidget {
  const EmptyNote({
    super.key,
    required this.icon,
    required this.title,
    this.detail = '',
    this.action,
  });

  final IconData icon;
  final String title;
  final String detail;
  final Widget? action;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 34),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 30, color: t.nInk3),
          const SizedBox(height: 10),
          Text(title,
              textAlign: TextAlign.center,
              style: TextStyle(
                  fontSize: 13, fontWeight: FontWeight.w600, color: t.nInk2)),
          if (detail.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 5),
              child: Text(detail,
                  textAlign: TextAlign.center,
                  style: TextStyle(fontSize: 11, color: t.nInk3)),
            ),
          if (action != null) ...[const SizedBox(height: 14), action!],
        ],
      ),
    );
  }
}

/// One insight, recommendation or thing needing attention. The action label and
/// the action itself both come from Rust, so a card can never offer a button
/// that routes nowhere.
class FlagCard extends StatelessWidget {
  const FlagCard({super.key, required this.flag, this.onAction});

  final FinFlag flag;
  final void Function(String action)? onAction;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final look = severityLook(flag.severity);
    return Container(
      padding: const EdgeInsets.fromLTRB(14, 12, 12, 12),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        border: Border.all(color: look.colour.withValues(alpha: 0.4)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(look.icon, size: 18, color: look.colour),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(flag.title,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
                if (flag.detail.isNotEmpty)
                  Padding(
                    padding: const EdgeInsets.only(top: 3),
                    child: Text(flag.detail,
                        style: TextStyle(fontSize: 11.5, color: t.nInk2)),
                  ),
              ],
            ),
          ),
          if (flag.actionLabel.isNotEmpty && onAction != null) ...[
            const SizedBox(width: 8),
            TextButton(
              onPressed: () => onAction!(flag.action),
              child:
                  Text(flag.actionLabel, style: const TextStyle(fontSize: 12)),
            ),
          ],
        ],
      ),
    );
  }
}

/// A month stepper: back, the month's name, forward, and the three shortcuts
/// beside them. `onGoto` takes an offset from the month today is in rather than
/// a period string — "next month" has to keep meaning next month after midnight
/// on the 31st.
class MonthStepper extends StatelessWidget {
  const MonthStepper({
    super.key,
    required this.label,
    required this.onStep,
    this.onGoto,
    this.trailing,
  });

  final String label;
  final void Function(int delta) onStep;
  final void Function(int offset)? onGoto;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      children: [
        IconButton(
          iconSize: 18,
          tooltip: 'Previous month',
          onPressed: () => onStep(-1),
          icon: const Icon(Icons.chevron_left),
        ),
        SizedBox(
          width: 150,
          child: Text(label,
              textAlign: TextAlign.center,
              style: TextStyle(
                  fontSize: 14, fontWeight: FontWeight.w700, color: t.nInk)),
        ),
        IconButton(
          iconSize: 18,
          tooltip: 'Next month',
          onPressed: () => onStep(1),
          icon: const Icon(Icons.chevron_right),
        ),
        if (onGoto != null) ...[
          const SizedBox(width: 8),
          for (final g in const [(-1, 'Last'), (0, 'This'), (1, 'Next')])
            Padding(
              padding: const EdgeInsets.only(right: 6),
              child: OutlinedButton(
                style: OutlinedButton.styleFrom(
                  visualDensity: VisualDensity.compact,
                  padding: const EdgeInsets.symmetric(horizontal: 10),
                ),
                onPressed: () => onGoto!(g.$1),
                child: Text(g.$2, style: const TextStyle(fontSize: 11)),
              ),
            ),
        ],
        const Spacer(),
        if (trailing != null) trailing!,
      ],
    );
  }
}

/// A dropdown over a list of labels. Every filter in the section is one of
/// these, and every one of them carries its own "all" sentinel as the first
/// entry, so "no filter" is a value rather than a null.
class LabelPicker extends StatelessWidget {
  const LabelPicker({
    super.key,
    required this.value,
    required this.options,
    required this.onChanged,
    this.width = 170,
    this.icon,
  });

  final String value;
  final List<String> options;
  final ValueChanged<String> onChanged;
  final double width;
  final IconData? icon;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // A value the list no longer holds would throw rather than fall back, so
    // the current value is admitted to its own list.
    final items = options.contains(value) ? options : [value, ...options];
    return SizedBox(
      width: width,
      child: DropdownButtonFormField<String>(
        initialValue: value,
        isDense: true,
        isExpanded: true,
        decoration: InputDecoration(
          isDense: true,
          contentPadding:
              const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
          prefixIcon: icon == null ? null : Icon(icon, size: 16),
          prefixIconConstraints:
              const BoxConstraints(minWidth: 30, minHeight: 20),
          border: const OutlineInputBorder(),
        ),
        style: TextStyle(fontSize: 12, color: t.nInk),
        items: [
          for (final o in items)
            DropdownMenuItem(
              value: o,
              child: Text(o, overflow: TextOverflow.ellipsis),
            ),
        ],
        onChanged: (v) {
          if (v != null) onChanged(v);
        },
      ),
    );
  }
}

/// A table header cell that sorts. Clicking the sorted column flips it; a
/// different column starts descending, which is what a reader of a money table
/// wants.
class SortHeader extends StatelessWidget {
  const SortHeader({
    super.key,
    required this.label,
    required this.column,
    required this.active,
    required this.desc,
    required this.onSort,
    this.align = TextAlign.left,
    this.flex = 1,
    this.width,
  });

  final String label;
  final String column;
  final String active;
  final bool desc;
  final void Function(String column) onSort;
  final TextAlign align;
  final int flex;
  final double? width;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final on = active == column;
    final cell = InkWell(
      onTap: () => onSort(column),
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 6),
        child: Row(
          mainAxisAlignment: align == TextAlign.right
              ? MainAxisAlignment.end
              : MainAxisAlignment.start,
          children: [
            Flexible(
              child: Text(label,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 11,
                      fontWeight: on ? FontWeight.w700 : FontWeight.w600,
                      color: on ? Tokens.secFinances : t.nInk2)),
            ),
            if (on)
              Icon(desc ? Icons.arrow_drop_down : Icons.arrow_drop_up,
                  size: 16, color: Tokens.secFinances),
          ],
        ),
      ),
    );
    if (width != null) return SizedBox(width: width, child: cell);
    return Expanded(flex: flex, child: cell);
  }
}
