// The Finances shell: seven tabs, one snapshot, one modal at a time.
//
// The tabs carry no counts. Each tab's own filter row carries them, and a
// number on a tab is a number you have to read past to find the name of the
// thing you wanted. The badge for what needs you stays on the rail, which is
// where a thing that needs you belongs.

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/tokens.dart';
import '../../shell/section_tabs.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/finances.dart';
import 'finances_accounts.dart';
import 'finances_controller.dart';
import 'finances_ledger.dart';
import 'finances_overview.dart';
import 'finances_planning.dart';
import 'finances_recurring.dart';
import 'finances_sheet.dart';

class FinancesPage extends StatefulWidget {
  const FinancesPage({super.key});

  @override
  State<FinancesPage> createState() => _FinancesPageState();
}

class _FinancesPageState extends State<FinancesPage> {
  final FinancesController _c = FinancesController();

  @override
  void initState() {
    super.initState();
    // The first refresh also runs the recurrence engine and, if they are a day
    // old, the exchange rates — the app may have been left running across a
    // date boundary, and a bill that became due at midnight has to be here now.
    _c.refresh();
    // "Open bill" from Papers: the ledger, with that transaction's sheet up.
    ShellController.instance.onOpen(Section.finances, (raw) async {
      final a = openArg(raw);
      final id = int.tryParse(a.arg);
      if (a.verb != 'txn' || id == null) return;
      await _c.send(const FinancesCmd.setTab(tab: 'txns'));
      await _c.send(FinancesCmd.txnOpen(id: id));
    });
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        final st = _c.state;
        return ColoredBox(
          color: t.nCanvas,
          child: Column(
            children: [
              _Header(c: _c, st: st),
              if (_c.notice != null) _NoticeBanner(c: _c),
              if (_c.error != null) _ErrorBanner(c: _c),
              Expanded(
                child: st == null
                    ? FirstLoad(error: _c.error, onRetry: _c.refresh)
                    : Stack(
                        children: [
                          _Body(c: _c, st: st),
                          if (st.sheet.isNotEmpty)
                            Positioned.fill(
                                child: FinancesSheet(c: _c, st: st)),
                          if (_c.busy)
                            const Positioned(
                              top: 0,
                              left: 0,
                              right: 0,
                              child: LinearProgressIndicator(minHeight: 2),
                            ),
                        ],
                      ),
              ),
            ],
          ),
        );
      },
    );
  }
}

class _Body extends StatelessWidget {
  const _Body({required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    return switch (st.tab) {
      'txns' => LedgerTab(c: c, st: st),
      'accounts' => AccountsTab(c: c, st: st),
      'planning' => PlanningTab(c: c, st: st),
      'bills' => BillsTab(c: c, st: st),
      'subs' => SubsTab(c: c, st: st),
      'dues' => LendingTab(c: c, st: st),
      _ => OverviewTab(c: c, st: st),
    };
  }
}

class _Header extends StatelessWidget {
  const _Header({required this.c, required this.st});

  final FinancesController c;
  final FinancesState? st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tab = st?.tab ?? 'overview';
    return Container(
      padding: const EdgeInsets.fromLTRB(20, 14, 20, 0),
      decoration: BoxDecoration(
        color: t.panel,
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const Icon(Icons.savings_outlined,
                  color: Tokens.secFinances, size: 22),
              const SizedBox(width: 10),
              Text('Finances',
                  style: TextStyle(
                      fontSize: 20,
                      fontWeight: FontWeight.w800,
                      color: t.nInk)),
              if (st != null && st!.badge > 0) ...[
                const SizedBox(width: 12),
                Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 9, vertical: 3),
                  decoration: BoxDecoration(
                    color: (st!.badgeOverdue ? Tokens.error : Tokens.warn)
                        .withValues(alpha: 0.16),
                    borderRadius: BorderRadius.circular(999),
                  ),
                  child: Text(
                    st!.badgeOverdue
                        ? '${st!.badge} due — some already late'
                        : '${st!.badge} coming up',
                    style: TextStyle(
                      fontSize: 11,
                      fontWeight: FontWeight.w700,
                      color: st!.badgeOverdue ? Tokens.error : Tokens.warn,
                    ),
                  ),
                ),
              ],
              const Spacer(),
              // Always here, as Slint's is: "Sample data" in the warning
              // colour while invented money is on screen, "Add sample data"
              // otherwise. Either opens the sheet that adds it back or clears
              // the section, as many times as you like. It used to show only
              // while samples were present, so after one clear the way back
              // was gone.
              if (st != null)
                Padding(
                  padding: const EdgeInsets.only(right: 8),
                  child: _SampleDataBtn(c: c, demo: st!.demo),
                ),
              IconButton(
                tooltip: 'Exchange rates',
                iconSize: 19,
                onPressed: () =>
                    c.send(const FinancesCmd.openSheet(kind: 'rates', id: 0)),
                icon: const Icon(Icons.currency_exchange),
              ),
              IconButton(
                tooltip: 'Refresh',
                iconSize: 19,
                onPressed: c.busy ? null : () => c.refresh(),
                icon: const Icon(Icons.refresh),
              ),
            ],
          ),
          const SizedBox(height: 10),
          SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: Row(
              children: [
                for (final f in keepTabs('finances', finTabs, (f) => f.id,
                    active: (f) => tab == f.id))
                  Padding(
                    padding: const EdgeInsets.only(right: 8),
                    child: _TabButton(
                      tab: f,
                      active: tab == f.id,
                      onTap: () => c.send(FinancesCmd.setTab(tab: f.id)),
                    ),
                  ),
              ],
            ),
          ),
          const SizedBox(height: 2),
        ],
      ),
    );
  }
}

/// The header's way into sample data: a warning while invented money is on
/// screen, a quiet offer when none is.
class _SampleDataBtn extends StatelessWidget {
  const _SampleDataBtn({required this.c, required this.demo});

  final FinancesController c;
  final bool demo;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final busy = c.demoBusy.isNotEmpty;
    final ink = demo ? Tokens.warn : t.nInk2;
    return Material(
      color: demo ? Tokens.warn.withValues(alpha: 0.12) : Colors.transparent,
      shape: StadiumBorder(
        side: BorderSide(
            color: demo ? Tokens.warn.withValues(alpha: 0.45) : t.nHair),
      ),
      child: InkWell(
        customBorder: const StadiumBorder(),
        onTap: busy ? null : () => _sampleData(context, c, demo: demo),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (busy)
                const SizedBox(
                  width: 12,
                  height: 12,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              else
                Icon(demo ? Icons.warning_amber_rounded : Icons.add,
                    size: 15, color: ink),
              const SizedBox(width: 7),
              Text(
                busy
                    ? c.demoBusy
                    : demo
                        ? 'Sample data'
                        : 'Add sample data',
                style: TextStyle(
                    fontSize: 12, fontWeight: FontWeight.w700, color: ink),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// What the sample data is and the two things to do about it — Slint's
/// explainer, word for word where the words still hold.
///
/// Offered whether or not any is left: adding puts a year of invented money
/// back beside your own, and "Remove all of it" is the section's reset, so
/// both can be done as many times as you like.
Future<void> _sampleData(
  BuildContext context,
  FinancesController c, {
  required bool demo,
}) {
  return showDialog<void>(
    context: context,
    builder: (ctx) {
      final t = ctx.tokens;
      final tint = demo ? Tokens.warn : Tokens.secFinances;
      final body = TextStyle(fontSize: 12.5, height: 1.5, color: t.nInk2);
      return AlertDialog(
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(20),
          // Warn-edged only while there is invented money on screen. With
          // none left this is an offer, not an alert.
          side: BorderSide(
              color: demo ? Tokens.warn.withValues(alpha: 0.35) : t.nHair),
        ),
        title: Row(
          children: [
            Container(
              width: 34,
              height: 34,
              decoration: BoxDecoration(
                color: tint.withValues(alpha: 0.16),
                borderRadius: BorderRadius.circular(11),
              ),
              child: Icon(
                  demo
                      ? Icons.warning_amber_rounded
                      : Icons.account_balance_wallet_outlined,
                  size: 18,
                  color: tint),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Text(
                demo
                    ? 'This is sample data, not your money.'
                    : 'Sample data, if you want it back.',
                style: TextStyle(
                    fontSize: 16, fontWeight: FontWeight.w800, color: t.nInk),
              ),
            ),
          ],
        ),
        content: SizedBox(
          width: 480,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                'The section can seed a plausible year of accounts, spending, '
                'bills, subscriptions, loans, dues and budgets, because nine '
                'empty tabs look exactly like nine broken ones. None of it came '
                'from a bank and none of it is yours.',
                style: body,
              ),
              const SizedBox(height: 10),
              Text(
                demo
                    ? 'It gets out of your way by itself, one part at a time: '
                        'your first real account clears the sample accounts, '
                        'your first real bill clears the sample bills. Nothing '
                        'you enter is ever removed with it.'
                    : 'There is none of it left. Adding it back puts the '
                        'sample accounts, bills, subscriptions, loans, dues and '
                        'budgets in again, and leaves everything you entered '
                        'yourself untouched.',
                style: body,
              ),
              const SizedBox(height: 14),
              // The remove button empties the section outright, so this says
              // so rather than leaving it to be discovered.
              Text(
                'Remove all of it empties the section — every account, '
                'posting, bill and budget, yours as well as the sample\'s.',
                style: TextStyle(
                    fontSize: 11.5,
                    height: 1.45,
                    color: demo ? Tokens.warn : t.nInk2),
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: const Text('Close'),
          ),
          // Offered whenever the sheet is: after the samples have gone it is
          // the section's reset, and hiding it then made it look as though
          // only invented money could be removed.
          OutlinedButton.icon(
            style: OutlinedButton.styleFrom(
              foregroundColor: Tokens.error,
              side: BorderSide(color: Tokens.error.withValues(alpha: 0.5)),
            ),
            onPressed: () {
              Navigator.pop(ctx);
              c.demo(add: false);
            },
            icon: const Icon(Icons.delete_outline, size: 17),
            label: const Text('Remove all of it'),
          ),
          FilledButton.icon(
            style: FilledButton.styleFrom(backgroundColor: Tokens.secFinances),
            onPressed: () {
              Navigator.pop(ctx);
              c.demo(add: true);
            },
            icon: const Icon(Icons.add, size: 17),
            label: const Text('Add sample data'),
          ),
        ],
      );
    },
  );
}

class _TabButton extends StatelessWidget {
  const _TabButton({
    required this.tab,
    required this.active,
    required this.onTap,
  });

  final FinTab tab;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onTap,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 9),
        decoration: BoxDecoration(
          border: Border(
            bottom: BorderSide(
              width: 2,
              color: active ? Tokens.secFinances : Colors.transparent,
            ),
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(tab.icon,
                size: 16, color: active ? Tokens.secFinances : t.nInk3),
            const SizedBox(width: 7),
            Text(tab.label,
                style: TextStyle(
                    fontSize: 12.5,
                    fontWeight: active ? FontWeight.w700 : FontWeight.w500,
                    color: active ? Tokens.secFinances : t.nInk2)),
          ],
        ),
      ),
    );
  }
}

/// What the section noticed on the way in — one banner, dismissable, never
/// re-raised. Re-entering the section is the user already looking at it, and a
/// banner over the thing it is telling you about is noise.
class _NoticeBanner extends StatelessWidget {
  const _NoticeBanner({required this.c});

  final FinancesController c;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final n = c.notice!;
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(20, 10, 8, 10),
      color: Tokens.warn.withValues(alpha: 0.12),
      child: Row(
        children: [
          const Icon(Icons.notifications_active_outlined,
              size: 17, color: Tokens.warn),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(n.title,
                    style: TextStyle(
                        fontSize: 12.5,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
                Text(n.body, style: TextStyle(fontSize: 11.5, color: t.nInk2)),
              ],
            ),
          ),
          TextButton(
            onPressed: () {
              c.dismissNotice();
              c.send(const FinancesCmd.setTab(tab: 'bills'));
            },
            child: const Text('Open Bills'),
          ),
          IconButton(
            iconSize: 17,
            onPressed: c.dismissNotice,
            icon: const Icon(Icons.close),
          ),
        ],
      ),
    );
  }
}

class _ErrorBanner extends StatelessWidget {
  const _ErrorBanner({required this.c});

  final FinancesController c;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(20, 9, 8, 9),
      color: Tokens.error.withValues(alpha: 0.12),
      child: Row(
        children: [
          const Icon(Icons.error_outline, size: 17, color: Tokens.error),
          const SizedBox(width: 10),
          Expanded(
            child: Text('${c.error}',
                style: const TextStyle(fontSize: 12, color: Tokens.error)),
          ),
          IconButton(
            iconSize: 17,
            onPressed: c.clearError,
            icon: const Icon(Icons.close),
          ),
        ],
      ),
    );
  }
}
