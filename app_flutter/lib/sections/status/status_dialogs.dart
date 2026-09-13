// What Status opens: one section card in full, which the page's drawer draws,
// and one metric explained, which is still a popup.
//
// The library popup that used to live here is gone. Its folders are the
// Libraries tab, its rescan is the page's own button, and its reset moved to
// Backup & Data, next to the backups that make it safe to press.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/status.dart';
import 'status_controller.dart';
import 'status_page.dart';

/// The shell a popup wears: a stripe, a glyph, a title, a close cross, a
/// scrolling body and a foot of buttons.
Future<void> _sheet(
  BuildContext context, {
  required Color tint,
  required IconData icon,
  required String title,
  required String subtitle,
  required Widget body,
  required List<Widget> Function(BuildContext) foot,
}) {
  final t = context.tokens;
  return showDialog<void>(
    context: context,
    builder: (context) => Dialog(
      // Solid: the light theme's panel is see-through, and a popup over the
      // page's own figures is unreadable if it is.
      backgroundColor: solidPanel(t),
      surfaceTintColor: Colors.transparent,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 640, maxHeight: 720),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(height: 3, color: tint),
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 16, 8, 16),
              child: Row(
                children: [
                  Glyph(icon: icon, tint: tint),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(title,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontSize: 16,
                                fontWeight: FontWeight.w800,
                                color: t.text)),
                        const SizedBox(height: 3),
                        Text(subtitle,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 11.5, color: t.textDim)),
                      ],
                    ),
                  ),
                  IconButton(
                    icon: const Icon(Icons.close, size: 16),
                    onPressed: () => Navigator.of(context).pop(),
                  ),
                ],
              ),
            ),
            Divider(height: 1, color: t.outline),
            Flexible(
              child: SingleChildScrollView(
                padding: const EdgeInsets.all(16),
                child: body,
              ),
            ),
            Divider(height: 1, color: t.outline),
            Padding(
              padding: const EdgeInsets.all(13),
              child: Row(children: foot(context)),
            ),
          ],
        ),
      ),
    ),
  );
}

// ── one section card ────────────────────────────────────────────────────────

/// One section card in full: what is wrong, its figures, and its groups.
class CardDetail extends StatelessWidget {
  const CardDetail({super.key, required this.card});

  final StCard card;

  @override
  Widget build(BuildContext context) => Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (card.why.isNotEmpty) ...[
            Container(
              padding: const EdgeInsets.all(10),
              decoration: BoxDecoration(
                color: Tokens.error.withValues(alpha: 0.09),
                borderRadius: BorderRadius.circular(9),
              ),
              child: Text(card.why,
                  style: const TextStyle(fontSize: 12, color: Tokens.error)),
            ),
            const SizedBox(height: 14),
          ],
          if (card.kpis.isNotEmpty) ...[
            Row(
              children: [
                for (var i = 0; i < card.kpis.length; i++) ...[
                  if (i > 0) const SizedBox(width: 10),
                  Expanded(child: _Kpi(kpi: card.kpis[i])),
                ],
              ],
            ),
            const SizedBox(height: 14),
          ],
          for (final g in card.groups) ...[
            _Group(group: g),
            const SizedBox(height: 14),
          ],
        ],
      );
}

class _Kpi extends StatelessWidget {
  const _Kpi({required this.kpi});

  final StKpi kpi;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(11),
      decoration: BoxDecoration(
        color: t.panel2,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: t.outline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(kpi.v,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 17, fontWeight: FontWeight.w800, color: t.text)),
          Text(kpi.k,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 10.5,
                  fontWeight: FontWeight.w600,
                  color: t.textDim)),
        ],
      ),
    );
  }
}

/// A row list *or* a table, never both — `cols` being non-empty is what says
/// which, the same test the HTML page makes. A table's first column is its
/// name and reads from the left; every column after it is centred, header
/// and cells alike, as the row lists' values are.
class _Group extends StatelessWidget {
  const _Group({required this.group});

  final StGroup group;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Caption(group.h),
        const SizedBox(height: 6),
        for (final r in group.rows) KvRow(row: r),
        if (group.cols.isNotEmpty) ...[
          Row(
            children: [
              for (var i = 0; i < group.cols.length; i++)
                Expanded(
                  child: Text(group.cols[i],
                      textAlign: i == 0 ? TextAlign.left : TextAlign.center,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 10,
                          fontWeight: FontWeight.w700,
                          letterSpacing: 0.5,
                          color: t.textDim)),
                ),
            ],
          ),
          const SizedBox(height: 6),
          Divider(height: 1, color: t.outline),
          if (group.table.isEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 7),
              child: Text('Nothing right now.',
                  style: TextStyle(fontSize: 12, color: t.textDim)),
            ),
          for (final tr in group.table)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 7),
              child: Row(
                children: [
                  for (var i = 0; i < tr.cells.length; i++)
                    Expanded(
                      child: Align(
                        alignment:
                            i == 0 ? Alignment.centerLeft : Alignment.center,
                        child: tr.cells[i].tag
                            ? Pill(
                                label: tr.cells[i].text,
                                cls: tr.cells[i].cls)
                            : Text(tr.cells[i].text,
                                textAlign: i == 0
                                    ? TextAlign.left
                                    : TextAlign.center,
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(
                                    fontSize: 12,
                                    fontWeight: tr.cells[i].cls == 'mut'
                                        ? FontWeight.w500
                                        : FontWeight.w600,
                                    color: clsColor(tr.cells[i].cls, t))),
                      ),
                    ),
                ],
              ),
            ),
        ],
      ],
    );
  }
}

// ── one metric, explained ───────────────────────────────────────────────────

Future<void> openMetric(
  BuildContext context,
  StatusController controller,
  StatusState state,
  int index,
) {
  if (index < 0 || index >= state.metrics.length) return Future.value();
  final m = state.metrics[index];
  return _sheet(
    context,
    tint: accentOf(m.accent),
    icon: Icons.info_outline,
    title: m.label,
    subtitle: '${m.value} · ${m.note}',
    body: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const Caption('WHAT IT IS'),
        const SizedBox(height: 5),
        _Para(m.helpIs),
        const SizedBox(height: 14),
        const Caption('WHY IT IS HERE'),
        const SizedBox(height: 5),
        _Para(m.helpFor),
        if (m.rows.isNotEmpty) ...[
          const SizedBox(height: 14),
          const Caption('RIGHT NOW'),
          const SizedBox(height: 5),
          for (final r in m.rows) KvRow(row: r),
        ],
      ],
    ),
    foot: (context) => [
      const Spacer(),
      Btn(
        label: 'Close',
        tint: Tokens.secSettings,
        onTap: () => Navigator.of(context).pop(),
      ),
    ],
  );
}

class _Para extends StatelessWidget {
  const _Para(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Text(
        text,
        style: TextStyle(fontSize: 12.5, color: context.tokens.text),
      );
}
