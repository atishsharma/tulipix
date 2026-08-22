// The Finances shell: seven tabs, one snapshot, one modal at a time.
//
// The tabs carry no counts. Each tab's own filter row carries them, and a
// number on a tab is a number you have to read past to find the name of the
// thing you wanted. The badge for what needs you stays on the rail, which is
// where a thing that needs you belongs.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
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
                    ? const Center(child: CircularProgressIndicator())
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
              if (st != null && st!.demo)
                Padding(
                  padding: const EdgeInsets.only(right: 8),
                  child: Tooltip(
                    message: 'Some of what is on screen is sample data.',
                    child: Chip(
                      visualDensity: VisualDensity.compact,
                      avatar: const Icon(Icons.science_outlined, size: 14),
                      label: Text(
                          c.demoBusy.isEmpty ? 'Sample data' : c.demoBusy,
                          style: const TextStyle(fontSize: 11)),
                    ),
                  ),
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
                for (final f in finTabs)
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
                Text(n.body,
                    style: TextStyle(fontSize: 11.5, color: t.nInk2)),
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
