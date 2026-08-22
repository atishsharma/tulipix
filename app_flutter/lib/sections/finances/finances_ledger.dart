// Transactions: every row, filtered six ways and paged.
//
// The filters live in Rust because they are state, not arguments: the month
// filter has to survive paying a bill on another tab. Every one of them narrows
// in SQL rather than after paging — filtering a page that was already cut to 25
// rows would show four of them and call it page one of fifty.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/finances.dart';
import 'finances_controller.dart';
import 'finances_overview.dart' show TxnRowTile;
import 'finances_widgets.dart';

class LedgerTab extends StatefulWidget {
  const LedgerTab({super.key, required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  State<LedgerTab> createState() => _LedgerTabState();
}

class _LedgerTabState extends State<LedgerTab> {
  late final TextEditingController _search =
      TextEditingController(text: widget.st.txnSearch);

  @override
  void didUpdateWidget(LedgerTab old) {
    super.didUpdateWidget(old);
    // Only when Rust changed it — Find missing, for instance, clears every
    // filter including this one.
    if (widget.st.txnSearch != old.st.txnSearch &&
        widget.st.txnSearch != _search.text) {
      _search.text = widget.st.txnSearch;
    }
  }

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = widget.c;
    final st = widget.st;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(20, 14, 20, 8),
          child: Wrap(
            spacing: 8,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              SizedBox(
                width: 230,
                child: TextField(
                  controller: _search,
                  decoration: const InputDecoration(
                    isDense: true,
                    hintText: 'Search descriptions…',
                    prefixIcon: Icon(Icons.search, size: 17),
                    border: OutlineInputBorder(),
                  ),
                  style: TextStyle(fontSize: 12, color: t.nInk),
                  onSubmitted: (v) =>
                      c.send(FinancesCmd.txnSearch(text: v.trim())),
                ),
              ),
              LabelPicker(
                value: st.txnAccount,
                options: st.accountNames,
                icon: Icons.account_balance_outlined,
                onChanged: (v) => c.send(FinancesCmd.txnAccount(name: v)),
              ),
              LabelPicker(
                value: st.txnCategory,
                options: st.categoryNames,
                icon: Icons.label_outline,
                onChanged: (v) => c.send(FinancesCmd.txnCategory(name: v)),
              ),
              LabelPicker(
                value: st.txnKind,
                options: const ['All', 'Expense', 'Income', 'Transfer'],
                width: 130,
                onChanged: (v) => c.send(FinancesCmd.txnKindFilter(kind: v)),
              ),
              LabelPicker(
                value: st.txnPeriod,
                options: st.txnPeriods,
                width: 140,
                icon: Icons.calendar_month_outlined,
                onChanged: (v) => c.send(FinancesCmd.txnPeriod(period: v)),
              ),
              LabelPicker(
                value: st.txnSource,
                options: st.sourceNames,
                width: 140,
                onChanged: (v) => c.send(FinancesCmd.txnSource(source: v)),
              ),
              FilledButton.icon(
                style: FilledButton.styleFrom(
                  backgroundColor: Tokens.secFinances,
                  foregroundColor: const Color(0xFF1A2E05),
                ),
                onPressed: () =>
                    c.send(const FinancesCmd.openSheet(kind: 'txn', id: 0)),
                icon: const Icon(Icons.add, size: 17),
                label: const Text('Record one'),
              ),
              IconButton(
                tooltip: 'Read a receipt',
                iconSize: 19,
                onPressed: () => c.send(const FinancesCmd.scanReceipt()),
                icon: const Icon(Icons.document_scanner_outlined),
              ),
              IconButton(
                tooltip: 'Import a statement',
                iconSize: 19,
                onPressed: () => c.send(const FinancesCmd.importPick()),
                icon: const Icon(Icons.upload_file_outlined),
              ),
            ],
          ),
        ),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 20),
          child: Row(
            children: [
              Text('${st.txnTotal} rows',
                  style: TextStyle(fontSize: 11.5, color: t.nInk2)),
              const SizedBox(width: 16),
              _Total(label: 'Out', value: st.txnSpent, tone: 'flat'),
              const SizedBox(width: 12),
              _Total(label: 'In', value: st.txnIncome, tone: 'ok'),
              const Spacer(),
              if (!st.runningShown)
                Tooltip(
                  message:
                      'Pick one account and sort by date to see a running '
                      'balance. Down a mixed list it would be a running total '
                      'of unrelated rows.',
                  child: Icon(Icons.info_outline, size: 15, color: t.nInk3),
                ),
            ],
          ),
        ),
        const SizedBox(height: 6),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 24),
          child: Row(
            children: [
              SortHeader(
                  label: 'Date',
                  column: 'date',
                  active: st.txnSort,
                  desc: st.txnDesc,
                  onSort: (col) => c.send(FinancesCmd.txnSortBy(column: col)),
                  width: 78),
              SortHeader(
                  label: 'Description',
                  column: 'description',
                  active: st.txnSort,
                  desc: st.txnDesc,
                  onSort: (col) => c.send(FinancesCmd.txnSortBy(column: col)),
                  flex: 4),
              SortHeader(
                  label: 'Category',
                  column: 'category',
                  active: st.txnSort,
                  desc: st.txnDesc,
                  onSort: (col) => c.send(FinancesCmd.txnSortBy(column: col)),
                  flex: 2),
              SortHeader(
                  label: 'Account',
                  column: 'account',
                  active: st.txnSort,
                  desc: st.txnDesc,
                  onSort: (col) => c.send(FinancesCmd.txnSortBy(column: col)),
                  flex: 2),
              SortHeader(
                  label: 'Amount',
                  column: 'amount',
                  active: st.txnSort,
                  desc: st.txnDesc,
                  align: TextAlign.right,
                  onSort: (col) => c.send(FinancesCmd.txnSortBy(column: col)),
                  width: 104),
              if (st.runningShown)
                SizedBox(
                  width: 104,
                  child: Text('Balance',
                      textAlign: TextAlign.right,
                      style: TextStyle(
                          fontSize: 11,
                          fontWeight: FontWeight.w600,
                          color: t.nInk2)),
                ),
            ],
          ),
        ),
        Divider(height: 1, color: t.nHair),
        Expanded(
          child: st.txns.isEmpty
              ? const EmptyNote(
                  icon: Icons.filter_alt_off_outlined,
                  title: 'Nothing matches those filters.',
                  detail: 'Widen one of them, or record something.')
              : ListView.builder(
                  padding: const EdgeInsets.symmetric(horizontal: 20),
                  itemCount: st.txns.length,
                  itemBuilder: (context, i) => TxnRowTile(
                    c: c,
                    row: st.txns[i],
                    showRunning: st.runningShown,
                  ),
                ),
        ),
        if (st.txnPages > 1)
          Container(
            padding: const EdgeInsets.symmetric(vertical: 8),
            decoration:
                BoxDecoration(border: Border(top: BorderSide(color: t.nHair))),
            child: Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                IconButton(
                  iconSize: 18,
                  onPressed: st.txnPage <= 0
                      ? null
                      : () => c.send(FinancesCmd.txnGoto(page: st.txnPage - 1)),
                  icon: const Icon(Icons.chevron_left),
                ),
                Text('${st.txnPage + 1} / ${st.txnPages}',
                    style: TextStyle(fontSize: 12, color: t.nInk2)),
                IconButton(
                  iconSize: 18,
                  onPressed: st.txnPage + 1 >= st.txnPages
                      ? null
                      : () => c.send(FinancesCmd.txnGoto(page: st.txnPage + 1)),
                  icon: const Icon(Icons.chevron_right),
                ),
              ],
            ),
          ),
      ],
    );
  }
}

class _Total extends StatelessWidget {
  const _Total({required this.label, required this.value, required this.tone});

  final String label;
  final String value;
  final String tone;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Text('$label ', style: TextStyle(fontSize: 11, color: t.nInk3)),
        Text(value,
            style: TextStyle(
                fontSize: 12,
                fontWeight: FontWeight.w700,
                color: toneColour(context, tone))),
      ],
    );
  }
}
