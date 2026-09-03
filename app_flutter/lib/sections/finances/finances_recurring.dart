// Bills and Subscriptions: two tabs over one table.
//
// A bill and a subscription are the same row in the database — a recurrence
// with a cycle, an amount and a next date. What separates them is what the
// amount means. A subscription's is fixed and known in advance; a bill's is a
// guess until it arrives, which is why an estimate is drawn dimmer than a fact
// and why a recurrence with no fixed amount can never post itself.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/finances.dart';
import 'finances_controller.dart';
import 'finances_overview.dart' show ObligationRow;
import 'finances_widgets.dart';

class BillsTab extends StatelessWidget {
  const BillsTab({super.key, required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.fromLTRB(20, 14, 20, 28),
      children: [
        MonthStepper(
          label: st.billsPeriod,
          onStep: (d) => c.send(FinancesCmd.billsStep(delta: d)),
          trailing: Wrap(
            spacing: 8,
            children: [
              OutlinedButton.icon(
                onPressed: () =>
                    c.send(const FinancesCmd.openSheet(kind: 'one-off', id: 0)),
                icon: const Icon(Icons.event_outlined, size: 16),
                label: const Text('One-off'),
              ),
              FilledButton.icon(
                style: FilledButton.styleFrom(
                  backgroundColor: Tokens.secFinances,
                  foregroundColor: const Color(0xFF1A2E05),
                ),
                onPressed: () =>
                    c.send(const FinancesCmd.openSheet(kind: 'bill', id: 0)),
                icon: const Icon(Icons.add, size: 17),
                label: const Text('Add a bill'),
              ),
            ],
          ),
        ),
        const SizedBox(height: 12),
        StatStrip(stats: st.billStats),
        const SizedBox(height: 14),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            for (final f in [
              (id: 'all', label: 'Everything', n: st.billsAll),
              (id: 'needs', label: 'Needs you', n: st.billsNeeds),
              (id: 'auto', label: 'Posts itself', n: st.billsAuto),
              (id: 'paid', label: 'Settled', n: st.billsPaid),
              (id: 'oneoff', label: 'One-offs', n: st.billsOneoff),
            ])
              CountChip(
                label: f.label,
                count: f.n,
                active: st.billsFilter == f.id,
                tint: f.id == 'needs' ? Tokens.warn : null,
                onTap: () => c.send(FinancesCmd.setBillsFilter(filter: f.id)),
              ),
          ],
        ),
        const SizedBox(height: 12),
        FinCard(
          title: 'Dated in this month',
          sub: 'Widened backwards, so a bill from last month that is still '
              'unpaid stays in front of you rather than falling off a calendar '
              'boundary.',
          child: st.bills.isEmpty
              ? const EmptyNote(
                  icon: Icons.receipt_long_outlined,
                  title: 'Nothing due in that month under that filter.')
              : Column(
                  children: [
                    for (final o in st.bills) ObligationRow(c: c, row: o),
                  ],
                ),
        ),
        const SizedBox(height: 14),
        FinCard(
          title: 'What repeats',
          sub:
              'The templates the dated rows above are posted from. Editing one '
              'changes what happens next month, not what already happened.',
          child: st.billTemplates.isEmpty
              ? const EmptyNote(
                  icon: Icons.repeat, title: 'No repeating bills set up yet.')
              : Column(
                  children: [
                    for (final r in st.billTemplates)
                      RecurRowTile(c: c, row: r, showMoney: false),
                  ],
                ),
        ),
        if (st.demo) ...[
          const SizedBox(height: 14),
          Text(
            'Some of these are sample data.',
            style: TextStyle(fontSize: 11, color: t.nInk3),
          ),
        ],
      ],
    );
  }
}

class SubsTab extends StatelessWidget {
  const SubsTab({super.key, required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(20, 14, 20, 0),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              StatStrip(stats: st.subStats),
              const SizedBox(height: 14),
              Wrap(
                spacing: 8,
                runSpacing: 8,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  for (final f in [
                    (id: 'all', label: 'Everything', n: st.subsAll),
                    (id: 'active', label: 'Active', n: st.subsActive),
                    (id: 'paused', label: 'Paused', n: st.subsPaused),
                    (id: 'cancelled', label: 'Cancelled', n: st.subsCancelled),
                    (id: 'removed', label: 'Removed', n: st.subsRemoved),
                  ])
                    CountChip(
                      label: f.label,
                      count: f.n,
                      active: st.subsFilter == f.id,
                      onTap: () =>
                          c.send(FinancesCmd.setSubsFilter(filter: f.id)),
                    ),
                  LabelPicker(
                    value: st.subsCategory,
                    options: st.subsCategories,
                    icon: Icons.label_outline,
                    onChanged: (v) => c.send(FinancesCmd.subsCategory(name: v)),
                  ),
                  LabelPicker(
                    value: st.subsCurrency,
                    options: st.subsCurrencies,
                    width: 145,
                    icon: Icons.currency_exchange,
                    onChanged: (v) => c.send(FinancesCmd.subsCurrency(code: v)),
                  ),
                  OutlinedButton.icon(
                    onPressed: () => c.send(
                        const FinancesCmd.openSheet(kind: 'detect', id: 0)),
                    icon: const Icon(Icons.auto_awesome_outlined, size: 16),
                    label: const Text('Find missing ones'),
                  ),
                  FilledButton.icon(
                    style: FilledButton.styleFrom(
                      backgroundColor: Tokens.secFinances,
                      foregroundColor: const Color(0xFF1A2E05),
                    ),
                    onPressed: () =>
                        c.send(const FinancesCmd.openSheet(kind: 'sub', id: 0)),
                    icon: const Icon(Icons.add, size: 17),
                    label: const Text('Add one'),
                  ),
                ],
              ),
              const SizedBox(height: 10),
              Row(
                children: [
                  Text('${st.subsYearly} a year, at the current prices',
                      style: TextStyle(
                          fontSize: 12,
                          fontWeight: FontWeight.w600,
                          color: t.nInk2)),
                ],
              ),
              const SizedBox(height: 8),
              Row(
                children: [
                  const SizedBox(width: 44),
                  SortHeader(
                      label: 'Name',
                      column: 'name',
                      active: st.subsSort,
                      desc: st.subsDesc,
                      onSort: (col) =>
                          c.send(FinancesCmd.subsSortBy(column: col)),
                      flex: 4),
                  SortHeader(
                      label: 'Cycle',
                      column: 'cycle',
                      active: st.subsSort,
                      desc: st.subsDesc,
                      onSort: (col) =>
                          c.send(FinancesCmd.subsSortBy(column: col)),
                      width: 100),
                  SortHeader(
                      label: 'Next',
                      column: 'next',
                      active: st.subsSort,
                      desc: st.subsDesc,
                      onSort: (col) =>
                          c.send(FinancesCmd.subsSortBy(column: col)),
                      width: 118),
                  SortHeader(
                      label: 'A month',
                      column: 'monthly',
                      active: st.subsSort,
                      desc: st.subsDesc,
                      align: TextAlign.right,
                      onSort: (col) =>
                          c.send(FinancesCmd.subsSortBy(column: col)),
                      width: 100),
                  SortHeader(
                      label: 'A year',
                      column: 'yearly',
                      active: st.subsSort,
                      desc: st.subsDesc,
                      align: TextAlign.right,
                      onSort: (col) =>
                          c.send(FinancesCmd.subsSortBy(column: col)),
                      width: 100),
                  const SizedBox(width: 96),
                ],
              ),
            ],
          ),
        ),
        Divider(height: 12, color: t.nHair),
        Expanded(
          child: st.subs.isEmpty
              ? const EmptyNote(
                  icon: Icons.repeat,
                  title: 'Nothing under that filter.',
                  detail: '"Find missing ones" reads the ledger for charges '
                      'that look like a subscription nobody set up.')
              : ListView.builder(
                  padding: const EdgeInsets.symmetric(horizontal: 20),
                  itemCount: st.subs.length,
                  itemBuilder: (context, i) =>
                      RecurRowTile(c: c, row: st.subs[i]),
                ),
        ),
      ],
    );
  }
}

/// One recurrence. Shared by both tabs, because they are one table.
class RecurRowTile extends StatelessWidget {
  const RecurRowTile({
    super.key,
    required this.c,
    required this.row,
    this.showMoney = true,
  });

  final FinancesController c;
  final FinRecurRow row;
  final bool showMoney;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final live = row.status == 'active';
    return Container(
      padding: const EdgeInsets.symmetric(vertical: 8),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Opacity(
        opacity: live ? 1 : 0.55,
        child: Row(
          children: [
            HueBadge(text: row.badge, argb: row.hue, size: 32),
            const SizedBox(width: 12),
            Expanded(
              flex: 4,
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
                      // A price that went up quietly is the only thing here
                      // worth interrupting a row for.
                      if (row.hikePct != 0)
                        Padding(
                          padding: const EdgeInsets.only(left: 6),
                          child: Tooltip(
                            message: 'Was ${row.hikeFrom}',
                            child: Container(
                              padding: const EdgeInsets.symmetric(
                                  horizontal: 5, vertical: 1),
                              decoration: BoxDecoration(
                                color: Tokens.warn.withValues(alpha: 0.16),
                                borderRadius: BorderRadius.circular(4),
                              ),
                              child: Text('+${row.hikePct}%',
                                  style: const TextStyle(
                                      fontSize: 9.5,
                                      fontWeight: FontWeight.w700,
                                      color: Tokens.warn)),
                            ),
                          ),
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
                  Row(
                    children: [
                      CatDot(argb: row.catHue, size: 6),
                      const SizedBox(width: 5),
                      Flexible(
                        child: Text(
                            '${row.category} · ${row.account}'
                            '${row.plan.isEmpty ? '' : ' · ${row.plan}'}',
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 10.5, color: t.nInk3)),
                      ),
                    ],
                  ),
                ],
              ),
            ),
            SizedBox(
              width: 100,
              child: Text(row.cycle,
                  style: TextStyle(fontSize: 11.5, color: t.nInk2)),
            ),
            SizedBox(
              width: 118,
              child: Text(live ? row.nextShort : row.status,
                  style: TextStyle(
                      fontSize: 11.5,
                      color: live && row.days < 0 ? Tokens.error : t.nInk2)),
            ),
            if (showMoney) ...[
              SizedBox(
                width: 100,
                child: Text(row.monthly,
                    textAlign: TextAlign.right,
                    style: TextStyle(
                        fontSize: 12,
                        fontWeight: FontWeight.w600,
                        color: t.nInk)),
              ),
              SizedBox(
                width: 100,
                child: Text(row.yearly,
                    textAlign: TextAlign.right,
                    style: TextStyle(fontSize: 11.5, color: t.nInk2)),
              ),
            ] else
              SizedBox(
                width: 100,
                child: Text(row.amount,
                    textAlign: TextAlign.right,
                    style: TextStyle(
                        fontSize: 12,
                        fontWeight: FontWeight.w600,
                        // An estimate is not a fact and is not drawn as one.
                        color: row.isEstimate ? t.nInk2 : t.nInk)),
              ),
            SizedBox(
              width: 96,
              child: Row(
                mainAxisAlignment: MainAxisAlignment.end,
                children: [
                  if (showMoney)
                    IconButton(
                      iconSize: 16,
                      tooltip: 'What it has cost',
                      onPressed: () => c.send(
                          FinancesCmd.openSheet(kind: 'prices', id: row.id)),
                      icon: const Icon(Icons.show_chart),
                    ),
                  PopupMenuButton<String>(
                    iconSize: 17,
                    tooltip: 'More',
                    onSelected: (v) {
                      switch (v) {
                        case 'edit':
                          c.send(FinancesCmd.openSheet(
                              kind: row.kind == 'subscription' ? 'sub' : 'bill',
                              id: row.id));
                        case 'delete':
                          c.send(FinancesCmd.recurDelete(id: row.id));
                        default:
                          c.send(
                              FinancesCmd.recurStatus(id: row.id, status: v));
                      }
                    },
                    itemBuilder: (context) => [
                      const PopupMenuItem(value: 'edit', child: Text('Edit')),
                      if (row.status != 'active')
                        const PopupMenuItem(
                            value: 'active', child: Text('Make it active')),
                      if (row.status != 'paused')
                        const PopupMenuItem(
                            value: 'paused', child: Text('Pause it')),
                      if (row.status != 'cancelled')
                        const PopupMenuItem(
                            value: 'cancelled', child: Text('Cancelled')),
                      const PopupMenuDivider(),
                      const PopupMenuItem(
                          value: 'delete', child: Text('Delete')),
                    ],
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}
