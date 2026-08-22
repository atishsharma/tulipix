// Overview: what happened this month.
//
// The dashboard, and the first-run door that stands in for it. An Overview of
// nine zeroes is not a dashboard, it is a wall — and it does not say which of
// the three ways in is the cheap one. The importer is: one statement gives the
// ledger, the recurring bills and the subscriptions in a single pass.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/finances.dart';
import 'finances_charts.dart';
import 'finances_controller.dart';
import 'finances_widgets.dart';

class OverviewTab extends StatelessWidget {
  const OverviewTab({super.key, required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    if (st.empty) return FirstRun(c: c, st: st);
    return ListView(
      padding: const EdgeInsets.fromLTRB(20, 16, 20, 28),
      children: [
        StatStrip(stats: st.stats, onAction: (a) => _act(c, a)),
        const SizedBox(height: 16),
        LayoutBuilder(
          builder: (context, box) {
            final narrow = box.maxWidth < 940;
            final ring = _SpendCard(c: c, st: st);
            final health = _HealthCard(st: st);
            if (narrow) {
              return Column(
                children: [ring, const SizedBox(height: 14), health],
              );
            }
            return Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(flex: 3, child: ring),
                const SizedBox(width: 14),
                Expanded(flex: 2, child: health),
              ],
            );
          },
        ),
        const SizedBox(height: 14),
        FinCard(
          title: 'The last twelve months',
          sub: 'Spending only. Click a bar to move the ring above it.',
          child: MonthBars(
            months: st.months,
            picked: st.monthPick,
            onPick: (i) => c.send(FinancesCmd.pickMonth(index: i)),
          ),
        ),
        const SizedBox(height: 14),
        if (st.needsYou.isNotEmpty) ...[
          FinCard(
            title: 'Needs you',
            sub: 'Dated inside the next fortnight, or already late.',
            child: Column(
              children: [
                for (final o in st.needsYou)
                  ObligationRow(c: c, row: o, compact: true),
              ],
            ),
          ),
          const SizedBox(height: 14),
        ],
        FinCard(
          title: 'Recent',
          sub: 'Unfiltered — the Transactions tab decides what it shows, not '
              'what "recent" means here.',
          trailing: TextButton(
            onPressed: () => c.send(const FinancesCmd.setTab(tab: 'txns')),
            child: const Text('Open the ledger'),
          ),
          child: st.recent.isEmpty
              ? const EmptyNote(
                  icon: Icons.receipt_long_outlined,
                  title: 'Nothing recorded yet.')
              : Column(
                  children: [
                    for (final r in st.recent) TxnRowTile(c: c, row: r),
                  ],
                ),
        ),
        if (st.demo) ...[
          const SizedBox(height: 14),
          DemoBanner(c: c),
        ],
      ],
    );
  }
}

/// Stat cards carry an action encoded as `kind:id`, so a card can never offer a
/// route that does not exist.
void _act(FinancesController c, String action) {
  final kind = action.split(':').first;
  switch (kind) {
    case 'acct':
      c.send(const FinancesCmd.setTab(tab: 'accounts'));
    case 'bills':
      c.send(const FinancesCmd.setTab(tab: 'bills'));
    case 'subs':
      c.send(const FinancesCmd.setTab(tab: 'subs'));
    case 'dues':
      c.send(const FinancesCmd.setTab(tab: 'dues'));
    case 'txns':
      c.send(const FinancesCmd.setTab(tab: 'txns'));
    case 'plan':
      c.send(const FinancesCmd.setTab(tab: 'planning'));
    default:
      c.send(FinancesCmd.flagAction(action: action));
  }
}

class _SpendCard extends StatelessWidget {
  const _SpendCard({required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    return FinCard(
      title: 'Where it went',
      sub: st.monthLabel,
      child: st.slices.isEmpty
          ? const EmptyNote(
              icon: Icons.donut_large_outlined,
              title: 'No spending recorded in that month.')
          : LayoutBuilder(
              builder: (context, box) {
                final ring = SpendRing(
                  slices: st.slices,
                  total: st.spentTotal,
                  caption: 'spent',
                );
                final legend = SliceLegend(slices: st.slices);
                if (box.maxWidth < 420) {
                  return Column(
                    children: [
                      Center(child: ring),
                      const SizedBox(height: 12),
                      legend,
                    ],
                  );
                }
                return Row(
                  crossAxisAlignment: CrossAxisAlignment.center,
                  children: [
                    ring,
                    const SizedBox(width: 18),
                    Expanded(child: legend),
                  ],
                );
              },
            ),
    );
  }
}

/// The score, and the parts it is made of. A part with nothing to measure says
/// so rather than scoring zero: "unmeasured" and "bad" are different claims,
/// and a section about money is not allowed to blur them.
class _HealthCard extends StatelessWidget {
  const _HealthCard({required this.st});

  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return FinCard(
      title: 'Financial health',
      sub: st.healthBand,
      trailing: Text(st.healthScore,
          style: TextStyle(
              fontSize: 18, fontWeight: FontWeight.w800, color: t.nInk)),
      child: Column(
        children: [
          for (final p in st.healthParts)
            Padding(
              padding: const EdgeInsets.only(bottom: 10),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Text(p.label,
                            style: TextStyle(
                                fontSize: 12,
                                color: p.measured ? t.nInk : t.nInk3)),
                      ),
                      Text(p.points,
                          style: TextStyle(
                              fontSize: 12,
                              fontWeight: FontWeight.w700,
                              color: toneColour(context, p.tone))),
                    ],
                  ),
                  const SizedBox(height: 5),
                  if (p.measured)
                    MeterBar(pct: p.pct, tint: toneColour(context, p.tone))
                  else
                    Container(height: 6, color: Colors.transparent),
                  if (p.detail.isNotEmpty)
                    Padding(
                      padding: const EdgeInsets.only(top: 4),
                      child: Text(p.detail,
                          style: TextStyle(fontSize: 10.5, color: t.nInk3)),
                    ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// Nothing entered yet. Three ways in, cheapest first.
class FirstRun extends StatelessWidget {
  const FirstRun({super.key, required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Center(
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(28),
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 720),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const Icon(Icons.savings_outlined,
                  size: 40, color: Tokens.secFinances),
              const SizedBox(height: 14),
              Text('Nothing here yet.',
                  style: TextStyle(
                      fontSize: 22,
                      fontWeight: FontWeight.w800,
                      color: t.nInk)),
              const SizedBox(height: 8),
              Text(
                'Three ways to start. The first is the cheap one: a single '
                'statement gives the ledger, the bills that repeat and the '
                'subscriptions hiding inside them, all in one pass.',
                textAlign: TextAlign.center,
                style: TextStyle(fontSize: 13, height: 1.5, color: t.nInk2),
              ),
              const SizedBox(height: 22),
              Wrap(
                spacing: 12,
                runSpacing: 12,
                alignment: WrapAlignment.center,
                children: [
                  _Door(
                    icon: Icons.upload_file_outlined,
                    title: 'Import a statement',
                    detail: 'CSV, OFX, PDF or a spreadsheet.',
                    primary: true,
                    onTap: () => c.send(const FinancesCmd.importPick()),
                  ),
                  _Door(
                    icon: Icons.account_balance_outlined,
                    title: 'Add an account',
                    detail: 'A bank, a wallet, a card, cash in a drawer.',
                    onTap: () => c.send(
                        const FinancesCmd.openSheet(kind: 'account', id: 0)),
                  ),
                  _Door(
                    icon: Icons.science_outlined,
                    title: 'Try it with sample data',
                    detail: 'A made-up year. Removable in one click.',
                    onTap: () => c.demo(add: true),
                  ),
                ],
              ),
              if (c.demoBusy.isNotEmpty) ...[
                const SizedBox(height: 18),
                Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const SizedBox(
                        width: 14,
                        height: 14,
                        child: CircularProgressIndicator(strokeWidth: 2)),
                    const SizedBox(width: 9),
                    Text(c.demoBusy,
                        style: TextStyle(fontSize: 12, color: t.nInk2)),
                  ],
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

class _Door extends StatelessWidget {
  const _Door({
    required this.icon,
    required this.title,
    required this.detail,
    required this.onTap,
    this.primary = false,
  });

  final IconData icon;
  final String title;
  final String detail;
  final VoidCallback onTap;
  final bool primary;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      width: 212,
      child: InkWell(
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        onTap: onTap,
        child: Container(
          padding: const EdgeInsets.all(16),
          decoration: BoxDecoration(
            color: primary
                ? Tokens.secFinances.withValues(alpha: 0.1)
                : t.nCard,
            borderRadius: BorderRadius.circular(Tokens.radiusMd),
            border: Border.all(
                color: primary ? Tokens.secFinances : t.nHair),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(icon,
                  size: 22,
                  color: primary ? Tokens.secFinances : t.nInk2),
              const SizedBox(height: 10),
              Text(title,
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
              const SizedBox(height: 4),
              Text(detail,
                  style: TextStyle(fontSize: 11, height: 1.35, color: t.nInk2)),
            ],
          ),
        ),
      ),
    );
  }
}

/// Sample data is on screen. Never hidden: invented money that does not
/// announce itself is the one thing this section must not do.
class DemoBanner extends StatelessWidget {
  const DemoBanner({super.key, required this.c});

  final FinancesController c;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.fromLTRB(14, 11, 10, 11),
      decoration: BoxDecoration(
        color: Tokens.warn.withValues(alpha: 0.1),
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        border: Border.all(color: Tokens.warn.withValues(alpha: 0.45)),
      ),
      child: Row(
        children: [
          const Icon(Icons.science_outlined, size: 17, color: Tokens.warn),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              'Some of what is on screen is sample data, not yours.',
              style: TextStyle(fontSize: 12, color: t.nInk),
            ),
          ),
          if (c.demoBusy.isNotEmpty)
            Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                const SizedBox(
                    width: 13,
                    height: 13,
                    child: CircularProgressIndicator(strokeWidth: 2)),
                const SizedBox(width: 8),
                Text(c.demoBusy,
                    style: TextStyle(fontSize: 11.5, color: t.nInk2)),
              ],
            )
          else
            TextButton(
              onPressed: () => c.demo(add: false),
              child: const Text('Remove all of it'),
            ),
        ],
      ),
    );
  }
}

// ------------------------------------------------------------- shared rows ---

/// One thing that is owed but has not moved yet. Shared by the Overview, the
/// Bills tab and the calendar's agenda, because they are the same row.
class ObligationRow extends StatelessWidget {
  const ObligationRow({
    super.key,
    required this.c,
    required this.row,
    this.compact = false,
  });

  final FinancesController c;
  final FinObligationRow row;
  final bool compact;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final open = row.status == 'due' || row.status == 'overdue';
    final late = row.days < 0 && open;
    return Container(
      padding: const EdgeInsets.symmetric(vertical: 8),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          CatDot(argb: row.catHue),
          const SizedBox(width: 10),
          Expanded(
            flex: 3,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Row(
                  children: [
                    Flexible(
                      child: Text(row.name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 12.5,
                              fontWeight: FontWeight.w600,
                              color: t.nInk)),
                    ),
                    if (row.autoPost)
                      const Padding(
                        padding: EdgeInsets.only(left: 6),
                        child: Tooltip(
                          message: 'Posts itself on the day',
                          child: Icon(Icons.bolt,
                              size: 13, color: Tokens.secFinances),
                        ),
                      ),
                  ],
                ),
                if (!compact)
                  Text('${row.category} · ${row.account}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 10.5, color: t.nInk3)),
              ],
            ),
          ),
          Expanded(
            flex: 2,
            child: Text(
              open ? dueIn(row.days) : row.status,
              style: TextStyle(
                  fontSize: 11.5,
                  fontWeight: late ? FontWeight.w700 : FontWeight.w400,
                  color: late ? Tokens.error : t.nInk2),
            ),
          ),
          SizedBox(
            width: 108,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.end,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(row.actual.isEmpty ? row.estimate : row.actual,
                    style: TextStyle(
                        fontSize: 12.5,
                        fontWeight: FontWeight.w700,
                        // An estimate is drawn dimmer than a fact, because it
                        // is one. A variable bill's figure is the mean of the
                        // last three and nothing more.
                        color: row.isEstimate && row.actual.isEmpty
                            ? t.nInk2
                            : t.nInk)),
                if (row.variance.isNotEmpty)
                  Text(row.variance,
                      style: TextStyle(
                          fontSize: 10,
                          color: row.over ? Tokens.error : Tokens.ok)),
              ],
            ),
          ),
          if (open) ...[
            const SizedBox(width: 4),
            IconButton(
              iconSize: 17,
              tooltip: 'Record the payment',
              onPressed: () =>
                  c.send(FinancesCmd.payObligation(id: row.id)),
              icon: const Icon(Icons.check_circle_outline),
            ),
            IconButton(
              iconSize: 17,
              tooltip: 'Skip this one',
              onPressed: () =>
                  c.send(FinancesCmd.skipObligation(id: row.id)),
              icon: const Icon(Icons.skip_next_outlined),
            ),
          ] else
            IconButton(
              iconSize: 17,
              tooltip: 'Undo — put it back to due',
              onPressed: () =>
                  c.send(FinancesCmd.unpayObligation(id: row.id)),
              icon: const Icon(Icons.undo),
            ),
        ],
      ),
    );
  }
}

/// One ledger row. Shared by the Overview's short list and the Transactions
/// table, so a transaction reads the same in both places.
class TxnRowTile extends StatelessWidget {
  const TxnRowTile({
    super.key,
    required this.c,
    required this.row,
    this.showRunning = false,
  });

  final FinancesController c;
  final FinTxnRow row;
  final bool showRunning;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: () => c.send(FinancesCmd.txnOpen(id: row.id)),
      child: Container(
        padding: const EdgeInsets.symmetric(vertical: 8, horizontal: 4),
        decoration: BoxDecoration(
          border: Border(bottom: BorderSide(color: t.nHair)),
        ),
        child: Row(
          children: [
            SizedBox(
              width: 78,
              child: Text(row.date,
                  style: TextStyle(fontSize: 11.5, color: t.nInk3)),
            ),
            Expanded(
              flex: 4,
              child: Text(row.description,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 12.5, color: t.nInk)),
            ),
            Expanded(
              flex: 2,
              child: Row(
                children: [
                  CatDot(argb: row.catHue, size: 7),
                  const SizedBox(width: 6),
                  Flexible(
                    child: Text(row.category,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11, color: t.nInk2)),
                  ),
                ],
              ),
            ),
            Expanded(
              flex: 2,
              child: Text(row.account,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11, color: t.nInk2)),
            ),
            SizedBox(
              width: 104,
              child: Text(row.amount,
                  textAlign: TextAlign.right,
                  style: TextStyle(
                      fontSize: 12.5,
                      fontWeight: FontWeight.w700,
                      color: amountColour(context, row.kind))),
            ),
            if (showRunning)
              SizedBox(
                width: 104,
                child: Text(row.running,
                    textAlign: TextAlign.right,
                    style: TextStyle(fontSize: 11.5, color: t.nInk3)),
              ),
          ],
        ),
      ),
    );
  }
}
