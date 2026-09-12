// The pieces ui/page_transfer.slint builds the page out of: the card, the two
// button shapes, the bordered box a short list lives in, the read-only value
// well, the tinted footer strip, and the sortable column head.
//
// They live together because they are one visual family — every border on the
// page is drawn from a tint passed in, which is what says which card a panel
// belongs to before its label is read.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../design/skin.dart';

/// The section's own hues. `Theme.transfer-accent` and the five box tints the
/// page assigns in its property block, transcribed rather than reinvented.
class Tint {
  const Tint._();

  static const accent = Tokens.secTransfer; // orange — the section
  static const accent2 = Color(0xFFFBBF24); // amber
  static const send = Color(0xFF6366F1); // indigo — giving
  static const recv = Tokens.ok; // green  — getting
  static const pair = Color(0xFFF59E0B); // amber  — who is let in
  static const hist = Color(0xFF06B6D4); // cyan   — what already happened
  static const help = Color(0xFF64748B); // slate  — neither, so neutral

  /// The ledger's six column pills. One family — cyan through violet, all at
  /// roughly the same saturation — so six different fills read as a set rather
  /// than six unrelated badges.
  static const columns = <Color>[
    Color(0xFF06B6D4),
    Color(0xFF0EA5E9),
    Color(0xFF6366F1),
    Color(0xFF14B8A6),
    Color(0xFF3B82F6),
    Color(0xFF8B5CF6),
  ];
}

/// `Surface.wash` — a tint at low alpha over the panel.
Color wash(Color c, double a) => c.withValues(alpha: a);

/// `Surface.edge` — the same tint at border strength.
Color edge(Color c, double a) => c.withValues(alpha: a);

/// One of the three cards, or the full-width ledger below them.
class TCard extends StatelessWidget {
  const TCard({
    super.key,
    required this.title,
    required this.subtitle,
    required this.icon,
    required this.tint,
    required this.children,
    this.action,
    this.actionIcon,
    this.actionEnabled = true,
    this.onAction,
    this.height,
  });

  final String title;
  final String subtitle;
  final IconData icon;
  final Color tint;
  final List<Widget> children;

  /// An optional control pinned to the right of the title row.
  final String? action;
  final IconData? actionIcon;
  final bool actionEnabled;
  final VoidCallback? onAction;

  /// Null lets the card size to its content — what the ledger does. The three
  /// across the top pin it, so none of them moves when a file is chosen.
  final double? height;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final body = Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Container(
              width: 38,
              height: 38,
              decoration: context.skin
                      .control(active: true, tint: tint, radius: 11) ??
                  BoxDecoration(
                    color: wash(tint, 0.16),
                    borderRadius: BorderRadius.circular(11),
                  ),
              child: Icon(context.skin.icon(icon), size: 19, color: tint),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(
                    title,
                    style: TextStyle(
                      fontFamily: context.skin.fontFamily ?? Tokens.fontFamily,
                      fontSize: 16,
                      fontWeight: FontWeight.w700,
                      color: t.text,
                    ),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    subtitle,
                    style: TextStyle(fontSize: 11.5, color: t.textDim),
                  ),
                ],
              ),
            ),
            if (action != null && action!.isNotEmpty)
              Opacity(
                opacity: actionEnabled ? 1 : 0.4,
                child: ActionBtn(
                  label: action!,
                  icon: actionIcon,
                  tint: tint,
                  onTap: actionEnabled ? onAction : null,
                ),
              ),
          ],
        ),
        const SizedBox(height: 14),
        ...children,
      ],
    );
    return Container(
      height: height,
      padding: const EdgeInsets.all(16),
      decoration: context.skin
              .surface(SurfaceRole.card, radius: Tokens.radiusMd) ??
          BoxDecoration(
            color: t.panel,
            borderRadius: BorderRadius.circular(Tokens.radiusMd),
            border: Border.all(color: t.outline),
          ),
      child: body,
    );
  }
}

/// A solid, filled button. The page's one primary shape.
class ActionBtn extends StatelessWidget {
  const ActionBtn({
    super.key,
    required this.label,
    required this.tint,
    this.icon,
    this.onTap,
    this.width,
    this.height = 34,
  });

  final String label;
  final Color tint;
  final IconData? icon;
  final VoidCallback? onTap;
  final double? width;
  final double height;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: width,
      height: height,
      child: FilledButton.icon(
        onPressed: onTap,
        icon: icon == null ? null : Icon(icon, size: 15),
        label: Text(
          label,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: const TextStyle(fontSize: 12.5, fontWeight: FontWeight.w700),
        ),
        style: FilledButton.styleFrom(
          backgroundColor: tint,
          foregroundColor: Colors.white,
          padding: const EdgeInsets.symmetric(horizontal: 13),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(height / 2),
          ),
        ),
      ),
    );
  }
}

/// The destructive twin, for the buttons that throw data away.
class DangerBtn extends StatelessWidget {
  const DangerBtn({
    super.key,
    required this.label,
    this.icon,
    this.onTap,
    this.width,
    this.height = 34,
  });

  final String label;
  final IconData? icon;
  final VoidCallback? onTap;
  final double? width;
  final double height;

  @override
  Widget build(BuildContext context) => ActionBtn(
        label: label,
        tint: Tokens.error,
        icon: icon,
        onTap: onTap,
        width: width,
        height: height,
      );
}

/// The small filled button per-row actions use.
class RowBtn extends StatelessWidget {
  const RowBtn({super.key, required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return TextButton(
      onPressed: onTap,
      style: TextButton.styleFrom(
        minimumSize: const Size(0, 26),
        padding: const EdgeInsets.symmetric(horizontal: 10),
        // Without these a TextButton is 48px tall whatever its minimumSize
        // says — the tap target is padded out around it — and 48px is most of
        // the height a card's label row has to give.
        visualDensity: VisualDensity.compact,
        tapTargetSize: MaterialTapTargetSize.shrinkWrap,
        foregroundColor: t.textDim,
        backgroundColor: t.panel2,
        shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(8)),
      ),
      child: Text(
        label,
        style: const TextStyle(fontSize: 11, fontWeight: FontWeight.w600),
      ),
    );
  }
}

/// The bordered box a short list lives in.
///
/// Always drawn, empty or not: a list that appears only once it has something
/// in it moves everything below it the moment a file is chosen, and the empty
/// box is where the eye already is. `rows` is how many fit before it scrolls.
class Outline extends StatelessWidget {
  const Outline({
    super.key,
    required this.label,
    required this.tint,
    required this.empty,
    required this.isEmpty,
    required this.children,
    this.note = '',
    this.rowH = 52,
    this.rows = 2,
    this.action,
    this.onAction,
    this.trailing,
  });

  final String label;
  final Color tint;
  final String empty;
  final bool isEmpty;
  final List<Widget> children;
  final String note;
  final double rowH;
  final int rows;
  final String? action;
  final VoidCallback? onAction;

  /// A control that belongs in the title row rather than beside the label —
  /// Send's "who is this for?" picker is the only one.
  final Widget? trailing;

  List<Widget> _controls() => [
        if (trailing != null) Flexible(child: trailing!),
        if (action != null && action!.isNotEmpty) ...[
          const SizedBox(width: 8),
          RowBtn(label: action!, onTap: onAction ?? () {}),
        ],
      ];

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      decoration: context.skin.surface(SurfaceRole.card, radius: 12) ??
          BoxDecoration(
            borderRadius: BorderRadius.circular(12),
            color: wash(tint, 0.05),
            border: Border.all(color: edge(tint, 0.42), width: 1.5),
          ),
      padding: const EdgeInsets.all(10),
      child: LayoutBuilder(
        builder: (context, box) {
          final stacked = box.maxWidth < _stackControls;
          return Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            mainAxisSize: MainAxisSize.min,
            children: [
              Row(
                children: [
                  // The label and its count give way before the controls do: a
                  // clipped "PAIRED DEVICES" is readable, a clipped button is not.
                  Flexible(
                    child: Text(
                      label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: 10,
                        letterSpacing: 1.3,
                        fontWeight: FontWeight.w700,
                        color: t.textDim,
                      ),
                    ),
                  ),
                  // The count is the first thing to go: on a narrow card the
                  // controls beside it are the ones that have to survive.
                  if (note.isNotEmpty && box.maxWidth > _noteRoom) ...[
                    const SizedBox(width: 8),
                    Flexible(
                      child: Text(
                        note,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          fontSize: 10.5,
                          color: tint,
                          fontWeight: FontWeight.w700,
                        ),
                      ),
                    ),
                  ],
                  const Spacer(),
                  // Wide enough: the controls sit on the label's line. Narrow, and
                  // they drop to a line of their own — squeezing a dropdown that
                  // has a device name in it produces something nobody can read.
                  if (!stacked) ..._controls(),
                ],
              ),
              if (stacked && _controls().isNotEmpty) ...[
                const SizedBox(height: 6),
                Row(
                    mainAxisAlignment: MainAxisAlignment.end,
                    children: _controls()),
              ],
              const SizedBox(height: 8),
              // `rows` rows tall when there is room for that many, and no taller:
              // a box that grows with its contents moves the whole card under it.
              // Flexible rather than a bare SizedBox because the card is pinned to
              // one height and the panel above this one is not — on a narrow
              // column there is sometimes less than two rows left, and a box that
              // insists on its full height in that case overflows instead of
              // giving way.
              Flexible(
                child: SizedBox(
                  height: rowH * rows,
                  child: isEmpty
                      ? Center(
                          child: Padding(
                            padding: const EdgeInsets.symmetric(horizontal: 10),
                            child: Text(
                              empty,
                              textAlign: TextAlign.center,
                              style: TextStyle(fontSize: 11, color: t.textDim),
                            ),
                          ),
                        )
                      : ListView.separated(
                          padding: EdgeInsets.zero,
                          itemCount: children.length,
                          separatorBuilder: (_, __) =>
                              const SizedBox(height: 4),
                          itemBuilder: (_, i) => children[i],
                        ),
                ),
              ),
            ],
          );
        },
      ),
    );
  }
}

/// Under this the label row cannot hold its count as well as its controls.
const _noteRoom = 250.0;

/// Under this the controls need a line of their own.
const _stackControls = 340.0;

/// A card panel that centres its content when there is room for it and scrolls
/// when there is not.
///
/// The three cards are pinned to one height so none of them moves when a file
/// is chosen. That makes every panel inside them a fixed budget, and a budget
/// is something content can exceed — two buttons that wrap onto a second row on
/// a narrow column are enough. Scrolling is the answer that neither clips the
/// text nor lets the card grow.
class PanelBody extends StatelessWidget {
  const PanelBody({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
        builder: (context, box) => SingleChildScrollView(
          child: ConstrainedBox(
            constraints: BoxConstraints(
              minHeight: box.maxHeight.isFinite ? box.maxHeight : 0,
            ),
            child: child,
          ),
        ),
      );
}

/// A read-only field the value can be selected out of, with an optional action
/// pinned inside the well rather than beside it.
class ValueField extends StatelessWidget {
  const ValueField({
    super.key,
    required this.value,
    this.size = 13,
    this.tone,
    this.weight = FontWeight.w600,
    this.action,
    this.onAction,
  });

  final String value;
  final double size;
  final Color? tone;
  final FontWeight weight;
  final String? action;
  final VoidCallback? onAction;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      constraints: const BoxConstraints(minHeight: 38),
      padding: const EdgeInsets.symmetric(horizontal: 11, vertical: 6),
      // A value to read or copy out: the skin's well.
      decoration: context.skin.surface(SurfaceRole.well, radius: 9) ??
          BoxDecoration(
            color: t.panel2,
            borderRadius: BorderRadius.circular(9),
            border: Border.all(color: t.outline),
          ),
      // The action sits inside the well beside the value, until the well is
      // too narrow to hold both — which happens on a phone, and on the
      // Connection card whenever the QR plate has eaten most of the column.
      // Then it drops underneath rather than pushing the value off the edge.
      child: LayoutBuilder(
        builder: (context, box) {
          final text = SelectableText(
            value,
            maxLines: 1,
            style: TextStyle(
              fontSize: size,
              fontWeight: weight,
              color: tone ?? t.text,
            ),
          );
          final act = action;
          if (act == null || act.isEmpty) return text;
          if (box.maxWidth < _actionInline) {
            return Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              mainAxisSize: MainAxisSize.min,
              children: [
                text,
                const SizedBox(height: 6),
                RowBtn(label: act, onTap: onAction ?? () {}),
              ],
            );
          }
          return Row(
            children: [
              Expanded(child: text),
              RowBtn(label: act, onTap: onAction ?? () {}),
            ],
          );
        },
      ),
    );
  }
}

/// Below this, a value and a button do not both fit on one line of a well.
const _actionInline = 150.0;

/// The tinted strip a card ends on.
class NoteStrip extends StatelessWidget {
  const NoteStrip({
    super.key,
    required this.title,
    required this.body,
    required this.icon,
    required this.tint,
  });

  final String title;
  final String body;
  final IconData icon;
  final Color tint;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: wash(tint, 0.08),
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: edge(tint, 0.3)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon, size: 15, color: tint),
          const SizedBox(width: 9),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  title,
                  style: TextStyle(
                    fontSize: 11.5,
                    fontWeight: FontWeight.w700,
                    color: t.text,
                  ),
                ),
                const SizedBox(height: 2),
                Text(body, style: TextStyle(fontSize: 10.5, color: t.textDim)),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// One section of the Help panel.
class HelpBlock extends StatelessWidget {
  const HelpBlock({
    super.key,
    required this.title,
    required this.body,
    required this.icon,
    required this.tint,
  });

  final String title;
  final String body;
  final IconData icon;
  final Color tint;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(13),
      decoration: BoxDecoration(
        color: wash(tint, 0.06),
        borderRadius: BorderRadius.circular(11),
        border: Border.all(color: edge(tint, 0.3)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(icon, size: 16, color: tint),
              const SizedBox(width: 8),
              Text(
                title,
                style: TextStyle(
                  fontFamily: context.skin.fontFamily ?? Tokens.fontFamily,
                  fontSize: 13.5,
                  fontWeight: FontWeight.w700,
                  color: t.text,
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          // The advice is written with hard line breaks and, on Linux, a shell
          // command in it. SelectableText because a rule nobody can copy is a
          // rule that gets retyped wrong.
          SelectableText(
            body,
            style: TextStyle(fontSize: 11.5, height: 1.5, color: t.textDim),
          ),
        ],
      ),
    );
  }
}

/// One sortable column header.
class SortHead extends StatelessWidget {
  const SortHead({
    super.key,
    required this.label,
    required this.index,
    required this.active,
    required this.desc,
    required this.tint,
    required this.onTap,
    this.width,
  });

  final String label;
  final int index;
  final int active;
  final bool desc;
  final Color tint;
  final VoidCallback onTap;
  final double? width;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final on = active == index;
    final head = InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(7),
      child: Container(
        height: 24,
        padding: const EdgeInsets.symmetric(horizontal: 8),
        decoration: BoxDecoration(
          color: on ? tint : wash(tint, 0.14),
          borderRadius: BorderRadius.circular(7),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Flexible(
              child: Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: 10.5,
                  fontWeight: FontWeight.w700,
                  color: on ? Colors.white : t.text,
                ),
              ),
            ),
            const SizedBox(width: 4),
            // Idle columns point down — that is what clicking them does.
            Icon(
              on && !desc ? Icons.arrow_upward : Icons.arrow_downward,
              size: 11,
              color: on ? Colors.white : t.textDim,
            ),
          ],
        ),
      ),
    );
    return width == null
        ? head
        : SizedBox(width: width, child: Center(child: head));
  }
}
