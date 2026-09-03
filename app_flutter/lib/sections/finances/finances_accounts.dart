// Accounts, loans and lending.
//
// The five cards over the grid are also its filter: the figure and the rows it
// was added up from are one control, so a card cannot show a total the grid
// under it disagrees with. "Lending, net" routes to its own tab rather than
// filtering the grid, because money lent is not an account — it is a claim, and
// the section keeps that distinction because conflating them is how a tracker
// starts counting the same rupee twice.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/finances.dart';
import 'finances_controller.dart';
import 'finances_widgets.dart';

class AccountsTab extends StatelessWidget {
  const AccountsTab({super.key, required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final showGrid = st.accountFilter != 'loans';
    return ListView(
      padding: const EdgeInsets.fromLTRB(20, 16, 20, 28),
      children: [
        StatStrip(
          stats: st.accountStats,
          onAction: (a) {
            final which = a.split(':').last;
            if (which == 'lending') {
              c.send(const FinancesCmd.setTab(tab: 'dues'));
            } else {
              c.send(FinancesCmd.setAccountFilter(
                  filter: st.accountFilter == which ? 'all' : which));
            }
          },
        ),
        const SizedBox(height: 16),
        Row(
          children: [
            Text('Liquid ${st.liquidTotal}',
                style: TextStyle(
                    fontSize: 12, fontWeight: FontWeight.w600, color: t.nInk)),
            const SizedBox(width: 14),
            Text('Owed ${st.debtTotal}',
                style: TextStyle(fontSize: 12, color: t.nInk2)),
            const Spacer(),
            OutlinedButton.icon(
              onPressed: () =>
                  c.send(const FinancesCmd.openSheet(kind: 'loan', id: 0)),
              icon: const Icon(Icons.account_balance_outlined, size: 16),
              label: const Text('Add a loan'),
            ),
            const SizedBox(width: 8),
            FilledButton.icon(
              style: FilledButton.styleFrom(
                backgroundColor: Tokens.secFinances,
                foregroundColor: const Color(0xFF1A2E05),
              ),
              onPressed: () =>
                  c.send(const FinancesCmd.openSheet(kind: 'account', id: 0)),
              icon: const Icon(Icons.add, size: 17),
              label: const Text('Add an account'),
            ),
          ],
        ),
        const SizedBox(height: 14),
        if (showGrid)
          if (st.accounts.isEmpty)
            const EmptyNote(
              icon: Icons.account_balance_wallet_outlined,
              title: 'No accounts under that filter.',
            )
          else
            LayoutBuilder(
              builder: (context, box) {
                final cols = (box.maxWidth / 330).floor().clamp(1, 4);
                return GridView.builder(
                  shrinkWrap: true,
                  physics: const NeverScrollableScrollPhysics(),
                  itemCount: st.accounts.length,
                  gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
                    crossAxisCount: cols,
                    mainAxisSpacing: 12,
                    crossAxisSpacing: 12,
                    mainAxisExtent: 172,
                  ),
                  itemBuilder: (context, i) =>
                      AccountCard(c: c, row: st.accounts[i]),
                );
              },
            ),
        if (st.loans.isNotEmpty) ...[
          const SizedBox(height: 20),
          Row(
            children: [
              Text('Loans',
                  style: TextStyle(
                      fontSize: 15,
                      fontWeight: FontWeight.w800,
                      color: t.nInk)),
              const SizedBox(width: 10),
              Text('${st.loansTotal} outstanding',
                  style: TextStyle(fontSize: 12, color: t.nInk2)),
            ],
          ),
          const SizedBox(height: 10),
          LayoutBuilder(
            builder: (context, box) {
              final cols = (box.maxWidth / 400).floor().clamp(1, 3);
              return GridView.builder(
                shrinkWrap: true,
                physics: const NeverScrollableScrollPhysics(),
                itemCount: st.loans.length,
                gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
                  crossAxisCount: cols,
                  mainAxisSpacing: 12,
                  crossAxisSpacing: 12,
                  mainAxisExtent: 206,
                ),
                itemBuilder: (context, i) => LoanCard(c: c, row: st.loans[i]),
              );
            },
          ),
        ],
      ],
    );
  }
}

class AccountCard extends StatelessWidget {
  const AccountCard({super.key, required this.c, required this.row});

  final FinancesController c;
  final FinAccountRow row;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Opacity(
      opacity: row.closed ? 0.55 : 1,
      child: Container(
        padding: const EdgeInsets.all(14),
        decoration: BoxDecoration(
          color: t.nCard,
          borderRadius: BorderRadius.circular(Tokens.radiusMd),
          border: Border.all(color: t.nHair),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                HueBadge(text: row.badge, argb: row.hue),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(row.name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 13,
                              fontWeight: FontWeight.w700,
                              color: t.nInk)),
                      Text(row.closed ? '${row.kind} · closed' : row.kind,
                          style: TextStyle(fontSize: 10.5, color: t.nInk3)),
                    ],
                  ),
                ),
                _Menu(c: c, row: row),
              ],
            ),
            const SizedBox(height: 10),
            Text(row.balance,
                style: TextStyle(
                    fontSize: 19,
                    fontWeight: FontWeight.w800,
                    color: row.negative ? Tokens.error : t.nInk)),
            if (row.sub.isNotEmpty)
              Text(row.sub, style: TextStyle(fontSize: 10.5, color: t.nInk2)),
            if (row.utilPct > 0) ...[
              const SizedBox(height: 8),
              MeterBar(pct: row.utilPct, over: row.utilPct > 80),
              const SizedBox(height: 4),
              Text(row.limitNote,
                  style: TextStyle(fontSize: 10, color: t.nInk3)),
            ],
            const Spacer(),
            Row(
              children: [
                if (row.drift.isNotEmpty)
                  Expanded(
                    child: Tooltip(
                      message:
                          'What the ledger says, against what you last counted.',
                      child: Text(row.drift,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 10.5,
                              fontWeight: FontWeight.w600,
                              color: row.driftBad ? Tokens.error : t.nInk3)),
                    ),
                  )
                else
                  Expanded(
                    child: Text(
                        row.reconciled.isEmpty
                            ? '${row.txnCount} rows'
                            : 'Counted ${row.reconciled}',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 10.5, color: t.nInk3)),
                  ),
                if (row.inMonth.isNotEmpty)
                  Text('↓${row.inMonth}',
                      style: const TextStyle(fontSize: 10.5, color: Tokens.ok)),
                if (row.outMonth.isNotEmpty)
                  Padding(
                    padding: const EdgeInsets.only(left: 6),
                    child: Text('↑${row.outMonth}',
                        style: TextStyle(fontSize: 10.5, color: t.nInk2)),
                  ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

class _Menu extends StatelessWidget {
  const _Menu({required this.c, required this.row});

  final FinancesController c;
  final FinAccountRow row;

  @override
  Widget build(BuildContext context) {
    final card = row.kind == 'card';
    return PopupMenuButton<String>(
      iconSize: 17,
      tooltip: 'More',
      onSelected: (v) {
        switch (v) {
          case 'edit':
            c.send(FinancesCmd.openSheet(kind: 'account', id: row.id));
          case 'reconcile':
            c.send(FinancesCmd.accountReconcile(id: row.id));
          case 'recount':
            c.send(FinancesCmd.accountRecount(id: row.id));
          case 'paycard':
            c.send(FinancesCmd.accountPayCard(id: row.id));
          case 'close':
            c.send(FinancesCmd.accountClose(id: row.id, closed: !row.closed));
          case 'delete':
            c.send(FinancesCmd.accountDelete(id: row.id));
        }
      },
      itemBuilder: (context) => [
        const PopupMenuItem(value: 'edit', child: Text('Edit')),
        // A recount is a reconcile — the same form, the same posting. The word
        // is different because the physical act is: nobody reconciles a wallet,
        // they count what is in it.
        PopupMenuItem(
          value: row.kind == 'cash' ? 'recount' : 'reconcile',
          child: Text(row.kind == 'cash' ? 'Count what is in it' : 'Reconcile'),
        ),
        if (card)
          const PopupMenuItem(
            value: 'paycard',
            // Paying a card is a transfer, never an expense: the money was
            // already counted when it was spent on the card.
            child: Text('Pay it off (a transfer)'),
          ),
        PopupMenuItem(
          value: 'close',
          child: Text(row.closed ? 'Reopen it' : 'Close it'),
        ),
        const PopupMenuDivider(),
        const PopupMenuItem(value: 'delete', child: Text('Delete')),
      ],
    );
  }
}

class LoanCard extends StatelessWidget {
  const LoanCard({super.key, required this.c, required this.row});

  final FinancesController c;
  final FinLoanRow row;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        border: Border.all(color: t.nHair),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              HueBadge(text: row.badge, argb: row.hue),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(row.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 13,
                            fontWeight: FontWeight.w700,
                            color: t.nInk)),
                    Text(
                        row.interestFree
                            ? 'Interest-free'
                            : '${row.rate} · ${row.emi} a month',
                        style: TextStyle(fontSize: 10.5, color: t.nInk3)),
                  ],
                ),
              ),
              Text(row.balance,
                  style: TextStyle(
                      fontSize: 16,
                      fontWeight: FontWeight.w800,
                      color: t.nInk)),
            ],
          ),
          const SizedBox(height: 10),
          MeterBar(pct: row.progressPct, tint: Tokens.secFinances),
          const SizedBox(height: 5),
          Row(
            children: [
              Text(
                  '${row.tenureMonths - row.remainingMonths} of '
                  '${row.tenureMonths} paid',
                  style: TextStyle(fontSize: 10.5, color: t.nInk2)),
              const Spacer(),
              if (row.closes.isNotEmpty)
                Text('closes ${row.closes}',
                    style: TextStyle(fontSize: 10.5, color: t.nInk3)),
            ],
          ),
          const SizedBox(height: 8),
          // The split is what makes an EMI worth looking at: early on, most of
          // it is interest, and the bar above says nothing about that.
          if (!row.interestFree)
            Row(
              children: [
                Text('This EMI: ',
                    style: TextStyle(fontSize: 10.5, color: t.nInk3)),
                Text(row.emiPrincipal,
                    style: const TextStyle(
                        fontSize: 10.5,
                        fontWeight: FontWeight.w600,
                        color: Tokens.ok)),
                Text(' principal · ',
                    style: TextStyle(fontSize: 10.5, color: t.nInk3)),
                Text(row.emiInterest,
                    style: const TextStyle(
                        fontSize: 10.5,
                        fontWeight: FontWeight.w600,
                        color: Tokens.error)),
                Text(' interest',
                    style: TextStyle(fontSize: 10.5, color: t.nInk3)),
              ],
            ),
          const Spacer(),
          Row(
            children: [
              if (row.interestLeft.isNotEmpty)
                Expanded(
                  child: Text('${row.interestLeft} of interest still to pay',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 10.5, color: t.nInk3)),
                )
              else
                const Spacer(),
              TextButton(
                onPressed: () =>
                    c.send(FinancesCmd.loanSchedule(id: row.accountId)),
                child: const Text('Schedule', style: TextStyle(fontSize: 11.5)),
              ),
              TextButton(
                onPressed: () =>
                    c.send(FinancesCmd.loanPrepay(id: row.accountId)),
                child: const Text('Prepay?', style: TextStyle(fontSize: 11.5)),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

// ----------------------------------------------------------------- lending ---

/// Money lent and money borrowed. Not spending, and never posted as such — a
/// due opens a transfer into a virtual holding account, so lending a friend
/// ₹5,000 moves it rather than burning it. That is the second of the two rules
/// that keep this section's totals honest.
class LendingTab extends StatelessWidget {
  const LendingTab({super.key, required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    return ListView(
      padding: const EdgeInsets.fromLTRB(20, 16, 20, 28),
      children: [
        StatStrip(stats: st.dueStats),
        const SizedBox(height: 14),
        Row(
          children: [
            Wrap(
              spacing: 8,
              children: [
                for (final f in [
                  (id: 'all', label: 'Everything', n: st.duesAll),
                  (id: 'to-me', label: 'Owed to me', n: st.duesToMe),
                  (id: 'i-owe', label: 'I owe', n: st.duesIOwe),
                  (id: 'closed', label: 'Closed', n: st.duesClosed),
                ])
                  CountChip(
                    label: f.label,
                    count: f.n,
                    active: st.duesFilter == f.id,
                    onTap: () =>
                        c.send(FinancesCmd.setDuesFilter(filter: f.id)),
                  ),
              ],
            ),
            const Spacer(),
            FilledButton.icon(
              style: FilledButton.styleFrom(
                backgroundColor: Tokens.secFinances,
                foregroundColor: const Color(0xFF1A2E05),
              ),
              onPressed: () =>
                  c.send(const FinancesCmd.openSheet(kind: 'due', id: 0)),
              icon: const Icon(Icons.add, size: 17),
              label: const Text('Record one'),
            ),
          ],
        ),
        const SizedBox(height: 12),
        FinCard(
          title: 'Open',
          child: st.dues.isEmpty
              ? const EmptyNote(
                  icon: Icons.people_outline,
                  title: 'Nothing outstanding under that filter.')
              : Column(
                  children: [
                    for (final d in st.dues) DueRowTile(c: c, row: d),
                  ],
                ),
        ),
        if (st.duesSettled.isNotEmpty) ...[
          const SizedBox(height: 14),
          FinCard(
            title: 'Settled or written off',
            sub:
                'The last six months. Its own card rather than only behind the '
                'Closed filter: the useful question is whether this gets paid '
                'back, and that needs the history next to the open rows.',
            child: Column(
              children: [
                for (final d in st.duesSettled)
                  DueRowTile(c: c, row: d, closed: true),
              ],
            ),
          ),
        ],
      ],
    );
  }
}

class DueRowTile extends StatelessWidget {
  const DueRowTile({
    super.key,
    required this.c,
    required this.row,
    this.closed = false,
  });

  final FinancesController c;
  final FinDueRow row;
  final bool closed;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final toMe = row.direction == 'to-me';
    return Container(
      padding: const EdgeInsets.symmetric(vertical: 9),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Opacity(
        opacity: closed ? 0.6 : 1,
        child: Row(
          children: [
            Icon(toMe ? Icons.call_received : Icons.call_made,
                size: 16, color: toMe ? Tokens.ok : Tokens.warn),
            const SizedBox(width: 10),
            Expanded(
              flex: 3,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(row.person,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 12.5,
                          fontWeight: FontWeight.w600,
                          color: t.nInk)),
                  if (row.note.isNotEmpty)
                    Text(row.note,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 10.5, color: t.nInk3)),
                ],
              ),
            ),
            Expanded(
              flex: 2,
              child: Text(
                  closed
                      ? '${row.status}${row.settled.isEmpty ? '' : ' ${row.settled}'}'
                      : 'since ${row.opened} · ${row.ageDays}d',
                  style: TextStyle(fontSize: 11, color: t.nInk2)),
            ),
            SizedBox(
              width: 112,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.end,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(row.amount,
                      style: TextStyle(
                          fontSize: 12.5,
                          fontWeight: FontWeight.w700,
                          color: toMe ? Tokens.ok : t.nInk)),
                  // A part payment has to show what it started as, or the row
                  // reads as though the whole debt were smaller than it was.
                  if (row.partPaid && row.original.isNotEmpty)
                    Text('of ${row.original}',
                        style: TextStyle(fontSize: 10, color: t.nInk3)),
                ],
              ),
            ),
            if (!closed) ...[
              IconButton(
                iconSize: 17,
                tooltip: 'Settle it',
                onPressed: () => c.send(FinancesCmd.dueSettle(id: row.id)),
                icon: const Icon(Icons.check_circle_outline),
              ),
              PopupMenuButton<String>(
                iconSize: 17,
                tooltip: 'More',
                onSelected: (v) => v == 'writeoff'
                    ? c.send(FinancesCmd.dueWriteOff(id: row.id))
                    : c.send(FinancesCmd.dueDelete(id: row.id)),
                itemBuilder: (context) => const [
                  PopupMenuItem(
                      value: 'writeoff',
                      child: Text('Write it off — not coming back')),
                  PopupMenuDivider(),
                  PopupMenuItem(value: 'delete', child: Text('Delete')),
                ],
              ),
            ] else
              const SizedBox(width: 88),
          ],
        ),
      ),
    );
  }
}
