// Planning: what to do next.
//
// One month selector over the whole tab, because the envelopes and the due
// dates are always the same month — which was the point of merging Budgets and
// Calendar into it. Everything on the dashboard is a summary; the detail lives
// behind five drill-downs, so reading the page never costs you the page.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/finances.dart';
import 'finances_charts.dart';
import 'finances_controller.dart';
import 'finances_overview.dart' show ObligationRow;
import 'finances_recurring.dart' show RecurRowTile;
import 'finances_widgets.dart';

class PlanningTab extends StatelessWidget {
  const PlanningTab({super.key, required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    return Stack(
      children: [
        ListView(
          padding: const EdgeInsets.fromLTRB(20, 14, 20, 28),
          children: [
            MonthStepper(
              label: st.budgetPeriod,
              onStep: (d) => c.send(FinancesCmd.planStep(delta: d)),
              onGoto: (o) => c.send(FinancesCmd.planGoto(offset: o)),
              trailing: FilledButton.icon(
                style: FilledButton.styleFrom(
                  backgroundColor: Tokens.secFinances,
                  foregroundColor: const Color(0xFF1A2E05),
                ),
                onPressed: () =>
                    c.send(const FinancesCmd.openSheet(kind: 'budget', id: 0)),
                icon: const Icon(Icons.add, size: 17),
                label: const Text('Set an envelope'),
              ),
            ),
            const SizedBox(height: 12),
            StatStrip(stats: st.planHealth),
            const SizedBox(height: 14),
            _CashFlowBar(c: c, st: st),
            const SizedBox(height: 14),
            if (st.attention.isNotEmpty) ...[
              FinCard(
                title: 'Wants you',
                child: Column(
                  children: [
                    for (final f in st.attention)
                      Padding(
                        padding: const EdgeInsets.only(bottom: 8),
                        child: FlagCard(
                            flag: f,
                            onAction: (a) =>
                                c.send(FinancesCmd.flagAction(action: a))),
                      ),
                  ],
                ),
              ),
              const SizedBox(height: 14),
            ],
            LayoutBuilder(
              builder: (context, box) {
                final narrow = box.maxWidth < 940;
                final left = Column(
                  children: [
                    _TimelineCard(c: c, st: st),
                    const SizedBox(height: 14),
                    _EnvelopesCard(c: c, st: st),
                  ],
                );
                final right = Column(
                  children: [
                    _HabitsCard(c: c, st: st),
                    const SizedBox(height: 14),
                    _PredictionsCard(st: st),
                    const SizedBox(height: 14),
                    _RecosCard(c: c, st: st),
                  ],
                );
                if (narrow) {
                  return Column(children: [left, const SizedBox(height: 14), right]);
                }
                return Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Expanded(flex: 3, child: left),
                    const SizedBox(width: 14),
                    Expanded(flex: 2, child: right),
                  ],
                );
              },
            ),
            const SizedBox(height: 14),
            _DrillRow(c: c),
          ],
        ),
        if (st.planModal.isNotEmpty) PlanModal(c: c, st: st),
      ],
    );
  }
}

/// Where the month's income is committed. Three slices: what leaves whatever
/// you decide, what the envelopes claim, and what no one has claimed. The third
/// is floored at zero — a month whose commitments and envelopes together come
/// to more than its income has nothing free, and a negative slice would draw as
/// a bar running backwards.
class _CashFlowBar extends StatelessWidget {
  const _CashFlowBar({required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return FinCard(
      title: 'This month, committed',
      sub: st.budgetIncome.isEmpty
          ? ''
          : '${st.budgetIncome} came in. ${st.fixedShare}',
      trailing: TextButton(
        onPressed: () =>
            c.send(const FinancesCmd.setPlanModal(which: 'flow')),
        child: const Text('The whole walk'),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SizedBox(
            height: 22,
            child: LayoutBuilder(
              builder: (context, box) {
                final w = box.maxWidth;
                Widget slice(int pct, Color colour, String label) => SizedBox(
                      width: w * pct / 100,
                      child: Container(
                        alignment: Alignment.center,
                        color: colour,
                        child: pct < 8
                            ? null
                            : Text(label,
                                style: const TextStyle(
                                    fontSize: 10,
                                    fontWeight: FontWeight.w700,
                                    color: Color(0xFF1A2E05))),
                      ),
                    );
                return ClipRRect(
                  borderRadius: BorderRadius.circular(6),
                  child: Row(
                    children: [
                      slice(st.fixedPct, const Color(0xFFF472B6), 'fixed'),
                      slice(st.budgetAllocatedPct, const Color(0xFFFACC15),
                          'envelopes'),
                      slice(st.planFreePct, Tokens.secFinances, 'free'),
                      Expanded(child: ColoredBox(color: t.nHair)),
                    ],
                  ),
                );
              },
            ),
          ),
          const SizedBox(height: 10),
          Row(
            children: [
              _Leg(colour: const Color(0xFFF472B6), label: 'Fixed', value: st.fixedTotal),
              _Leg(
                  colour: const Color(0xFFFACC15),
                  label: 'Envelopes',
                  value: st.budgetAllocated),
              _Leg(
                  colour: Tokens.secFinances,
                  label: 'Free',
                  value: st.planFree),
            ],
          ),
          if (st.calWarning.isNotEmpty) ...[
            const SizedBox(height: 10),
            Row(
              children: [
                const Icon(Icons.warning_amber_outlined,
                    size: 15, color: Tokens.warn),
                const SizedBox(width: 7),
                Expanded(
                  child: Text(st.calWarning,
                      style: const TextStyle(
                          fontSize: 11.5, color: Tokens.warn)),
                ),
              ],
            ),
          ],
        ],
      ),
    );
  }
}

class _Leg extends StatelessWidget {
  const _Leg({required this.colour, required this.label, required this.value});

  final Color colour;
  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Expanded(
      child: Row(
        children: [
          Container(
            width: 8,
            height: 8,
            decoration: BoxDecoration(color: colour, shape: BoxShape.circle),
          ),
          const SizedBox(width: 6),
          Text('$label ', style: TextStyle(fontSize: 11, color: t.nInk3)),
          Flexible(
            child: Text(value,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 11.5,
                    fontWeight: FontWeight.w700,
                    color: t.nInk)),
          ),
        ],
      ),
    );
  }
}

/// Everything dated in the month, day by day, with the closing balance at the
/// end of it. Expected income is not added to that closing figure — the same
/// rule the cash-flow walk states, so the two cannot disagree.
class _TimelineCard extends StatelessWidget {
  const _TimelineCard({required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return FinCard(
      title: 'What is dated this month',
      trailing: TextButton(
        onPressed: () =>
            c.send(const FinancesCmd.setPlanModal(which: 'calendar')),
        child: const Text('Calendar'),
      ),
      child: st.timeline.isEmpty
          ? const EmptyNote(
              icon: Icons.event_available_outlined,
              title: 'Nothing dated in this month.')
          : Column(
              children: [
                for (final e in st.timeline)
                  Padding(
                    padding: const EdgeInsets.symmetric(vertical: 5),
                    child: Row(
                      children: [
                        SizedBox(
                          width: 28,
                          child: Text(e.day > 0 ? '${e.day}' : '',
                              style: TextStyle(
                                  fontSize: 11,
                                  fontWeight: FontWeight.w700,
                                  color: t.nInk3)),
                        ),
                        Container(
                          width: 3,
                          height: 22,
                          decoration: BoxDecoration(
                            color: hue(e.hue),
                            borderRadius: BorderRadius.circular(2),
                          ),
                        ),
                        const SizedBox(width: 10),
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            mainAxisSize: MainAxisSize.min,
                            children: [
                              Text(e.name,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: TextStyle(
                                      fontSize: 12,
                                      fontWeight: e.closing
                                          ? FontWeight.w800
                                          : FontWeight.w500,
                                      color: t.nInk)),
                              if (e.sub.isNotEmpty)
                                Text(e.sub,
                                    style: TextStyle(
                                        fontSize: 10, color: t.nInk3)),
                            ],
                          ),
                        ),
                        Text(e.amount,
                            style: TextStyle(
                                fontSize: 12,
                                fontWeight: e.closing
                                    ? FontWeight.w800
                                    : FontWeight.w600,
                                color: e.income ? Tokens.ok : t.nInk)),
                      ],
                    ),
                  ),
              ],
            ),
    );
  }
}

class _EnvelopesCard extends StatelessWidget {
  const _EnvelopesCard({required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final top = st.budgets.where((b) => !b.unset).take(6).toList();
    return FinCard(
      title: 'Envelopes',
      sub: '${st.envelopesSpent} spent of ${st.budgetAllocated} set aside',
      trailing: TextButton(
        onPressed: () =>
            c.send(const FinancesCmd.setPlanModal(which: 'budgets')),
        child: const Text('All of them'),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (top.isEmpty)
            const EmptyNote(
              icon: Icons.mail_outline,
              title: 'No envelopes set for this month.',
              detail: 'An envelope is a cap you choose, not one the app '
                  'invents from last month.',
            )
          else
            for (final b in top) EnvelopeRow(c: c, row: b),
          if (st.budgetUnbudgeted.isNotEmpty) ...[
            Divider(height: 18, color: t.nHair),
            Row(
              children: [
                const Icon(Icons.help_outline, size: 15, color: Tokens.warn),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                      '${st.budgetUnbudgeted} ${st.budgetUnbudgetedSub}',
                      style: TextStyle(fontSize: 11.5, color: t.nInk2)),
                ),
              ],
            ),
          ],
        ],
      ),
    );
  }
}

/// One envelope. The projected mark is a paler bar behind the real one: where
/// the month is heading, against where it is now.
class EnvelopeRow extends StatelessWidget {
  const EnvelopeRow({super.key, required this.c, required this.row});

  final FinancesController c;
  final FinBudgetRow row;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: () =>
          c.send(FinancesCmd.openSheet(kind: 'budget', id: row.categoryId)),
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 7),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Expanded(
                  child: Row(
                    children: [
                      Flexible(
                        child: Text(row.name,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontSize: 12,
                                fontWeight: FontWeight.w600,
                                color: t.nInk)),
                      ),
                      if (row.rollover && row.carried.isNotEmpty)
                        Padding(
                          padding: const EdgeInsets.only(left: 6),
                          child: Tooltip(
                            message: 'Carried in from last month',
                            child: Text('+${row.carried}',
                                style: const TextStyle(
                                    fontSize: 10, color: Tokens.ok)),
                          ),
                        ),
                    ],
                  ),
                ),
                Text('${row.spent} / ${row.budget}',
                    style: TextStyle(
                        fontSize: 11.5,
                        fontWeight: FontWeight.w600,
                        color: row.over ? Tokens.error : t.nInk2)),
              ],
            ),
            const SizedBox(height: 5),
            MeterBar(
              pct: row.usedPct,
              ghostPct: row.projectedPct,
              over: row.over,
              tint: row.offPace ? Tokens.warn : Tokens.secFinances,
            ),
            if (row.detail.isNotEmpty)
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: Text(row.detail,
                    style: TextStyle(
                        fontSize: 10,
                        color: row.over ? Tokens.error : t.nInk3)),
              ),
          ],
        ),
      ),
    );
  }
}

class _HabitsCard extends StatelessWidget {
  const _HabitsCard({required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return FinCard(
      title: 'How this has been going',
      child: Column(
        children: [
          for (final h in st.habits)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 6),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Text(h.label,
                            style: TextStyle(fontSize: 12, color: t.nInk2)),
                      ),
                      Text(h.value,
                          style: TextStyle(
                              fontSize: 13,
                              fontWeight: FontWeight.w700,
                              color: hue(h.hue))),
                      // A habit's action names one of Planning's own
                      // drill-downs, never a tab: sending the reader to another
                      // tab to see four rows would cost them this page.
                      if (h.actionLabel.isNotEmpty)
                        TextButton(
                          style: TextButton.styleFrom(
                            visualDensity: VisualDensity.compact,
                            padding: const EdgeInsets.symmetric(horizontal: 6),
                          ),
                          onPressed: () => c.send(
                              FinancesCmd.setPlanModal(which: h.action)),
                          child: Text(h.actionLabel,
                              style: const TextStyle(fontSize: 11)),
                        ),
                    ],
                  ),
                  if (h.bar) ...[
                    const SizedBox(height: 5),
                    MeterBar(pct: h.pct, tint: hue(h.hue)),
                  ],
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// When each envelope is projected to run out, from the shape of the month so
/// far. A projection, drawn as one — the note says what it is measuring.
class _PredictionsCard extends StatelessWidget {
  const _PredictionsCard({required this.st});

  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (st.predictions.isEmpty) return const SizedBox.shrink();
    return FinCard(
      title: 'At this pace',
      child: Column(
        children: [
          for (final p in st.predictions)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 7),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Row(
                    children: [
                      CatDot(argb: p.hue),
                      const SizedBox(width: 8),
                      Expanded(
                        child: Text(p.name,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 12, color: t.nInk)),
                      ),
                      Text(p.finish,
                          style: TextStyle(
                              fontSize: 11.5,
                              fontWeight: FontWeight.w700,
                              color: p.soon ? Tokens.error : t.nInk2)),
                    ],
                  ),
                  const SizedBox(height: 4),
                  BurnDown(points: p.points, tint: hue(p.hue)),
                  if (p.note.isNotEmpty)
                    Padding(
                      padding: const EdgeInsets.only(top: 3),
                      child: Text(p.note,
                          style: TextStyle(fontSize: 10, color: t.nInk3)),
                    ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

class _RecosCard extends StatelessWidget {
  const _RecosCard({required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    if (st.recos.isEmpty) return const SizedBox.shrink();
    return FinCard(
      title: 'Worth knowing',
      child: Column(
        children: [
          for (final f in st.recos)
            Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: FlagCard(
                  flag: f,
                  onAction: (a) => c.send(FinancesCmd.flagAction(action: a))),
            ),
        ],
      ),
    );
  }
}

/// The five drill-downs, as a row of doors. Everything above is a summary; this
/// is where the lists live.
class _DrillRow extends StatelessWidget {
  const _DrillRow({required this.c});

  final FinancesController c;

  @override
  Widget build(BuildContext context) {
    return Wrap(
      spacing: 10,
      runSpacing: 10,
      children: [
        for (final d in const [
          (id: 'budgets', label: 'Every envelope', icon: Icons.mail_outline),
          (id: 'calendar', label: 'Calendar', icon: Icons.calendar_month_outlined),
          (id: 'flow', label: 'The cash-flow walk', icon: Icons.timeline),
          (id: 'review', label: 'This month, reviewed', icon: Icons.fact_check_outlined),
          (id: 'trends', label: 'Trends & savings', icon: Icons.show_chart),
        ])
          OutlinedButton.icon(
            onPressed: () => c.send(FinancesCmd.setPlanModal(which: d.id)),
            icon: Icon(d.icon, size: 16),
            label: Text(d.label),
          ),
      ],
    );
  }
}

// ------------------------------------------------------------ drill-downs ---

class PlanModal extends StatelessWidget {
  const PlanModal({super.key, required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final title = switch (st.planModal) {
      'budgets' => 'Every envelope — ${st.budgetPeriod}',
      'calendar' => st.calLabel,
      'flow' => 'Where the money goes',
      'review' => '${st.budgetPeriod}, reviewed',
      'trends' => 'Trends & savings',
      'subs' => 'Subscriptions that have gone up',
      _ => '',
    };
    return Positioned.fill(
      child: Stack(
        children: [
          GestureDetector(
            onTap: () => c.send(const FinancesCmd.setPlanModal(which: '')),
            child: ColoredBox(color: Colors.black.withValues(alpha: 0.45)),
          ),
          Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 880, maxHeight: 720),
              child: Material(
                color: t.modal,
                borderRadius: BorderRadius.circular(Tokens.radiusLg),
                clipBehavior: Clip.antiAlias,
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Padding(
                      padding: const EdgeInsets.fromLTRB(20, 16, 10, 10),
                      child: Row(
                        children: [
                          Expanded(
                            child: Text(title,
                                style: TextStyle(
                                    fontSize: 17,
                                    fontWeight: FontWeight.w800,
                                    color: t.nInk)),
                          ),
                          if (st.planModal == 'calendar')
                            SegmentedButton<String>(
                              style: const ButtonStyle(
                                  visualDensity: VisualDensity.compact),
                              segments: const [
                                ButtonSegment(value: 'month', label: Text('Month')),
                                ButtonSegment(value: 'year', label: Text('Year')),
                              ],
                              selected: {st.calView},
                              onSelectionChanged: (s) => c.send(
                                  FinancesCmd.setCalView(view: s.first)),
                            ),
                          IconButton(
                            iconSize: 20,
                            onPressed: () => c.send(
                                const FinancesCmd.setPlanModal(which: '')),
                            icon: const Icon(Icons.close),
                          ),
                        ],
                      ),
                    ),
                    Flexible(
                      child: SingleChildScrollView(
                        padding: const EdgeInsets.fromLTRB(20, 0, 20, 20),
                        child: _ModalBody(c: c, st: st),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _ModalBody extends StatelessWidget {
  const _ModalBody({required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    return switch (st.planModal) {
      'budgets' => _AllEnvelopes(c: c, st: st),
      'calendar' => _CalendarBody(c: c, st: st),
      'flow' => _FlowBody(st: st),
      'review' => _ReviewBody(st: st),
      'trends' => _TrendsBody(st: st),
      'subs' => Column(
          children: [
            for (final r in st.subs.where((s) => s.hikePct != 0))
              RecurRowTile(c: c, row: r),
          ],
        ),
      _ => const SizedBox.shrink(),
    };
  }
}

class _AllEnvelopes extends StatelessWidget {
  const _AllEnvelopes({required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            OutlinedButton.icon(
              onPressed: () =>
                  c.send(const FinancesCmd.budgetCopyForward()),
              icon: const Icon(Icons.content_copy_outlined, size: 15),
              label: const Text('Copy last month\'s'),
            ),
            const Spacer(),
            Text('${st.budgetUnallocated} unallocated',
                style: TextStyle(fontSize: 11.5, color: t.nInk2)),
          ],
        ),
        const SizedBox(height: 10),
        for (final b in st.budgets)
          Row(
            children: [
              Expanded(child: EnvelopeRow(c: c, row: b)),
              if (!b.unset)
                IconButton(
                  iconSize: 16,
                  tooltip: 'Remove the envelope',
                  onPressed: () => c.send(
                      FinancesCmd.budgetRemove(categoryId: b.categoryId)),
                  icon: const Icon(Icons.close),
                ),
            ],
          ),
        const SizedBox(height: 16),
        Text('What leaves whatever you decide',
            style: TextStyle(
                fontSize: 13, fontWeight: FontWeight.w700, color: t.nInk)),
        const SizedBox(height: 4),
        Text(
            'EMIs, bills and subscriptions. Shown so the page adds up: you '
            'cannot overspend a bill, you can only fail to pay it.',
            style: TextStyle(fontSize: 11, color: t.nInk3)),
        const SizedBox(height: 8),
        KvList(rows: st.fixedItems, dense: true),
        const SizedBox(height: 12),
        if (st.envelopeHistory.isNotEmpty) ...[
          Text('Six months',
              style: TextStyle(
                  fontSize: 13, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(height: 8),
          _HistoryGrid(st: st),
        ],
      ],
    );
  }
}

class _HistoryGrid extends StatelessWidget {
  const _HistoryGrid({required this.st});

  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SingleChildScrollView(
      scrollDirection: Axis.horizontal,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const SizedBox(width: 150),
              for (final m in st.disciplineMonths)
                SizedBox(
                  width: 74,
                  child: Text(m,
                      textAlign: TextAlign.center,
                      style: TextStyle(
                          fontSize: 10,
                          fontWeight: FontWeight.w600,
                          color: t.nInk3)),
                ),
              const SizedBox(width: 84),
            ],
          ),
          const SizedBox(height: 4),
          for (final h in st.envelopeHistory)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 3),
              child: Row(
                children: [
                  SizedBox(
                    width: 150,
                    child: Text(h.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11.5, color: t.nInk)),
                  ),
                  for (var i = 0; i < h.cells.length; i++)
                    SizedBox(
                      width: 74,
                      child: Text(h.cells[i],
                          textAlign: TextAlign.center,
                          style: TextStyle(
                              fontSize: 11,
                              color: i < h.over.length && h.over[i]
                                  ? Tokens.error
                                  : t.nInk2)),
                    ),
                  SizedBox(
                    width: 84,
                    child: Text(h.avgDelta,
                        textAlign: TextAlign.right,
                        style: TextStyle(
                            fontSize: 11,
                            fontWeight: FontWeight.w600,
                            color: h.avgOver ? Tokens.error : Tokens.ok)),
                  ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

class _CalendarBody extends StatelessWidget {
  const _CalendarBody({required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        MonthStepper(
          label: st.calLabel,
          onStep: (d) => c.send(FinancesCmd.calStep(delta: d)),
        ),
        const SizedBox(height: 10),
        if (st.calView == 'year')
          CalendarYear(months: st.calMonths)
        else
          CalendarGrid(
            days: st.calDays,
            focus: st.calFocus,
            // The grid's own day selection is a Dart concern only in the Slint
            // build; here the agenda under it lists the whole month, so a tap
            // scrolls rather than re-queries.
            onPick: (_) {},
          ),
        const SizedBox(height: 14),
        if (st.calHeaviest.isNotEmpty) ...[
          FinCard(
            title: st.calHeaviestLabel,
            sub: st.calHeaviestSub,
            trailing: Text(st.calHeaviest,
                style: TextStyle(
                    fontSize: 15,
                    fontWeight: FontWeight.w800,
                    color: t.nInk)),
            child: Column(
              children: [
                for (final o in st.calHeaviestItems)
                  ObligationRow(c: c, row: o, compact: true),
              ],
            ),
          ),
          const SizedBox(height: 12),
        ],
        if (st.calLowPoint.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(bottom: 12),
            child: Row(
              children: [
                const Icon(Icons.south_east, size: 15, color: Tokens.warn),
                const SizedBox(width: 8),
                Text(st.calLowPoint,
                    style: const TextStyle(
                        fontSize: 11.5, color: Tokens.warn)),
              ],
            ),
          ),
        FinCard(
          title: 'Everything dated in the month',
          child: st.calAgenda.isEmpty
              ? const EmptyNote(
                  icon: Icons.event_busy_outlined,
                  title: 'Nothing dated in this month.')
              : Column(
                  children: [
                    for (final o in st.calAgenda) ObligationRow(c: c, row: o),
                  ],
                ),
        ),
      ],
    );
  }
}

class _FlowBody extends StatelessWidget {
  const _FlowBody({required this.st});

  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
            'What is in the bank, what leaves, where it bottoms out, what '
            'lands, where it closes.',
            style: TextStyle(fontSize: 11.5, color: t.nInk2)),
        const SizedBox(height: 10),
        KvList(rows: st.calFlow),
        const SizedBox(height: 18),
        Text('The month on its own terms',
            style: TextStyle(
                fontSize: 13, fontWeight: FontWeight.w700, color: t.nInk)),
        const SizedBox(height: 4),
        Text(
            'What came in, less what has a claim on it. Separate from the walk '
            'above, which measures a bank balance and so carries every month '
            'before this one inside it.',
            style: TextStyle(fontSize: 11, color: t.nInk3)),
        const SizedBox(height: 8),
        KvList(rows: st.planStatement),
        const SizedBox(height: 18),
        Text('Where this month\'s income is committed',
            style: TextStyle(
                fontSize: 13, fontWeight: FontWeight.w700, color: t.nInk)),
        const SizedBox(height: 8),
        SliceLegend(slices: st.commitments),
      ],
    );
  }
}

class _ReviewBody extends StatelessWidget {
  const _ReviewBody({required this.st});

  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        StatStrip(stats: st.review),
        const SizedBox(height: 18),
        Text('Budget against actual',
            style: TextStyle(
                fontSize: 13, fontWeight: FontWeight.w700, color: t.nInk)),
        const SizedBox(height: 8),
        for (final d in st.discipline)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 5),
            child: Row(
              children: [
                SizedBox(
                  width: 90,
                  child: Text(d.period,
                      style: TextStyle(fontSize: 11.5, color: t.nInk2)),
                ),
                Expanded(
                  child: MeterBar(pct: d.pct, over: !d.within),
                ),
                const SizedBox(width: 10),
                SizedBox(
                  width: 190,
                  child: Text('${d.spent} of ${d.budget}',
                      textAlign: TextAlign.right,
                      style: TextStyle(
                          fontSize: 11.5,
                          color: d.within ? t.nInk2 : Tokens.error)),
                ),
              ],
            ),
          ),
      ],
    );
  }
}

class _TrendsBody extends StatelessWidget {
  const _TrendsBody({required this.st});

  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        StatStrip(stats: st.insightStats),
        const SizedBox(height: 18),
        Text('Savings rate',
            style: TextStyle(
                fontSize: 13, fontWeight: FontWeight.w700, color: t.nInk)),
        const SizedBox(height: 4),
        Text(
            'A month with no income at all is drawn hollow rather than at '
            'zero: "unknown" and "saved nothing" are not the same claim.',
            style: TextStyle(fontSize: 11, color: t.nInk3)),
        const SizedBox(height: 8),
        SavingsStrip(months: st.savings),
        const SizedBox(height: 10),
        KvList(rows: st.savingsSummary, dense: true),
        const SizedBox(height: 18),
        if (st.trends.isNotEmpty) ...[
          for (final s in st.trends) ...[
            Text(s.name,
                style: TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w700,
                    color: t.nInk2)),
            const SizedBox(height: 6),
            SavingsStrip(months: s.months, height: 60),
            const SizedBox(height: 14),
          ],
        ],
        if (st.trendPies.isNotEmpty) ...[
          Text('Six months of spending',
              style: TextStyle(
                  fontSize: 13, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(height: 8),
          SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: Row(
              children: [
                for (final p in st.trendPies)
                  Padding(
                    padding: const EdgeInsets.only(right: 14),
                    child: Column(
                      children: [
                        SpendRing(
                          slices: p.slices,
                          total: p.total,
                          caption: '',
                          size: 108,
                          thickness: 16,
                        ),
                        const SizedBox(height: 5),
                        Text(p.label,
                            style:
                                TextStyle(fontSize: 11, color: t.nInk2)),
                      ],
                    ),
                  ),
              ],
            ),
          ),
          const SizedBox(height: 18),
        ],
        Text('Looking forward',
            style: TextStyle(
                fontSize: 13, fontWeight: FontWeight.w700, color: t.nInk)),
        const SizedBox(height: 8),
        KvList(rows: st.projection),
        if (st.projectionNote.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(top: 6),
            child: Text(st.projectionNote,
                style: TextStyle(fontSize: 10.5, color: t.nInk3)),
          ),
        const SizedBox(height: 18),
        if (st.flags.isNotEmpty) ...[
          Text('What stands out',
              style: TextStyle(
                  fontSize: 13, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(height: 8),
          for (final f in st.flags)
            Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: FlagCard(flag: f),
            ),
        ],
      ],
    );
  }
}
