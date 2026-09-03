// The section's five drawings.
//
// The Slint build emitted SVG path commands for the donut from Rust, because
// Slint has no trigonometry and a ring cannot be drawn without it. Canvas has
// `arcTo`, so a slice crosses as nothing but where it starts and how much of
// the circle it covers, and the geometry — some forty lines of it — is gone.
// This is the one place the port is allowed to be shorter than the original.

import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/finances.dart';
import 'finances_controller.dart';
import 'finances_widgets.dart';

/// Spending by category, as a ring with the total in the middle.
class SpendRing extends StatelessWidget {
  const SpendRing({
    super.key,
    required this.slices,
    required this.total,
    required this.caption,
    this.size = 190,
    this.thickness = 26,
  });

  final List<FinCatSlice> slices;
  final String total;
  final String caption;
  final double size;
  final double thickness;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      width: size,
      height: size,
      child: Stack(
        alignment: Alignment.center,
        children: [
          CustomPaint(
            size: Size.square(size),
            painter: _RingPainter(
              slices: slices,
              thickness: thickness,
              track: t.nHair,
            ),
          ),
          Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(total,
                  style: TextStyle(
                      fontSize: 20,
                      fontWeight: FontWeight.w800,
                      color: t.nInk)),
              if (caption.isNotEmpty)
                Text(caption, style: TextStyle(fontSize: 10.5, color: t.nInk2)),
            ],
          ),
        ],
      ),
    );
  }
}

class _RingPainter extends CustomPainter {
  _RingPainter({
    required this.slices,
    required this.thickness,
    required this.track,
  });

  final List<FinCatSlice> slices;
  final double thickness;
  final Color track;

  @override
  void paint(Canvas canvas, Size size) {
    final r = (size.shortestSide - thickness) / 2;
    final centre = Offset(size.width / 2, size.height / 2);
    final rect = Rect.fromCircle(center: centre, radius: r);
    final base = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = thickness
      ..color = track;
    canvas.drawCircle(centre, r, base);

    // Twelve o'clock, clockwise — the direction a reader expects a share to
    // grow, and the direction the Slint build drew it in.
    const start = -math.pi / 2;
    // A hairline of the background between slices, so two adjacent categories
    // of similar hue still read as two.
    const gap = 0.012;
    for (final s in slices) {
      if (s.pct <= 0) continue;
      final from = start + s.start / 100 * 2 * math.pi;
      final sweep = s.pct / 100 * 2 * math.pi;
      canvas.drawArc(
        rect,
        from + gap / 2,
        math.max(sweep - gap, 0.004),
        false,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = thickness
          ..strokeCap = StrokeCap.butt
          ..color = hue(s.hue),
      );
    }
  }

  @override
  bool shouldRepaint(_RingPainter old) =>
      old.slices != slices || old.thickness != thickness || old.track != track;
}

/// The ring's key. Its own widget because the ring is square and the key is
/// not: side by side on a wide card, stacked on a narrow one.
class SliceLegend extends StatelessWidget {
  const SliceLegend({super.key, required this.slices});

  final List<FinCatSlice> slices;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final s in slices)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 3),
            child: Row(
              children: [
                CatDot(argb: s.hue, size: 9),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(s.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 12, color: t.nInk2)),
                ),
                Text('${s.pct}%',
                    style: TextStyle(
                        fontSize: 11,
                        fontWeight: FontWeight.w600,
                        color: t.nInk3)),
                const SizedBox(width: 10),
                SizedBox(
                  width: 84,
                  child: Text(s.amount,
                      textAlign: TextAlign.right,
                      style: TextStyle(
                          fontSize: 12,
                          fontWeight: FontWeight.w600,
                          color: t.nInk)),
                ),
              ],
            ),
          ),
      ],
    );
  }
}

/// The twelve-month bar strip, which is also the Overview's month picker:
/// clicking a bar retargets the ring above it.
class MonthBars extends StatelessWidget {
  const MonthBars({
    super.key,
    required this.months,
    required this.picked,
    required this.onPick,
    this.height = 92,
  });

  final List<FinMonthBar> months;
  final int picked;
  final void Function(int index) onPick;
  final double height;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (months.isEmpty) {
      return SizedBox(
        height: height,
        child: Center(
          child: Text('No months with any spending in them yet.',
              style: TextStyle(fontSize: 11.5, color: t.nInk3)),
        ),
      );
    }
    // -1 means "the latest", which is the bar the chart highlights.
    final on = picked < 0 ? months.length - 1 : picked;
    return SizedBox(
      height: height,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          for (var i = 0; i < months.length; i++)
            Expanded(
              child: Tooltip(
                message: '${months[i].long}  ${months[i].expense}',
                child: InkWell(
                  onTap: () => onPick(i),
                  borderRadius: BorderRadius.circular(6),
                  child: Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 2),
                    child: Column(
                      mainAxisAlignment: MainAxisAlignment.end,
                      children: [
                        Expanded(
                          child: FractionallySizedBox(
                            alignment: Alignment.bottomCenter,
                            heightFactor:
                                (months[i].expensePct.clamp(2, 100)) / 100,
                            child: Container(
                              decoration: BoxDecoration(
                                color: i == on
                                    ? Tokens.secFinances
                                    : Tokens.secFinances
                                        .withValues(alpha: 0.32),
                                borderRadius: const BorderRadius.vertical(
                                    top: Radius.circular(4)),
                              ),
                            ),
                          ),
                        ),
                        const SizedBox(height: 5),
                        Text(months[i].label,
                            style: TextStyle(
                                fontSize: 9.5,
                                fontWeight:
                                    i == on ? FontWeight.w700 : FontWeight.w400,
                                color: i == on ? t.nInk : t.nInk3)),
                      ],
                    ),
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// The month grid. Each day carries a count, a total and up to a handful of
/// pips — one per thing due that day, in that thing's own colour.
///
/// `pips` is a plain list here. In Slint it had to be four fixed slots, because
/// a `[color]` inside a struct is an `Rc` and cannot cross into the event loop.
class CalendarGrid extends StatelessWidget {
  const CalendarGrid({
    super.key,
    required this.days,
    required this.focus,
    required this.onPick,
  });

  final List<FinCalDay> days;
  final int focus;
  final void Function(int day) onPick;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    const names = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        Row(
          children: [
            for (final n in names)
              Expanded(
                child: Text(n,
                    textAlign: TextAlign.center,
                    style: TextStyle(
                        fontSize: 10,
                        fontWeight: FontWeight.w600,
                        color: t.nInk3)),
              ),
          ],
        ),
        const SizedBox(height: 6),
        GridView.builder(
          shrinkWrap: true,
          physics: const NeverScrollableScrollPhysics(),
          itemCount: days.length,
          gridDelegate: const SliverGridDelegateWithFixedCrossAxisCount(
            crossAxisCount: 7,
            mainAxisSpacing: 4,
            crossAxisSpacing: 4,
            childAspectRatio: 1.15,
          ),
          itemBuilder: (context, i) {
            final d = days[i];
            final selected = d.inMonth && d.day == focus;
            return InkWell(
              borderRadius: BorderRadius.circular(8),
              onTap: d.inMonth ? () => onPick(d.day) : null,
              child: Container(
                padding: const EdgeInsets.all(4),
                decoration: BoxDecoration(
                  color: !d.inMonth
                      ? Colors.transparent
                      : selected
                          ? Tokens.secFinances.withValues(alpha: 0.14)
                          : t.nTile,
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(
                    color: d.overdue
                        ? Tokens.error
                        : d.isToday
                            ? Tokens.secFinances
                            : selected
                                ? Tokens.secFinances.withValues(alpha: 0.6)
                                : Colors.transparent,
                  ),
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text('${d.day}',
                        style: TextStyle(
                            fontSize: 11,
                            fontWeight:
                                d.isToday ? FontWeight.w800 : FontWeight.w500,
                            color: !d.inMonth
                                ? t.nInk3.withValues(alpha: 0.4)
                                : d.isToday
                                    ? Tokens.secFinances
                                    : t.nInk2)),
                    const Spacer(),
                    if (d.pips.isNotEmpty)
                      Wrap(
                        spacing: 2,
                        runSpacing: 2,
                        children: [
                          for (final p in d.pips.take(6))
                            CatDot(argb: p, size: 5),
                        ],
                      ),
                    if (d.amount.isNotEmpty)
                      Text(d.amount,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 9,
                              fontWeight: FontWeight.w600,
                              color: d.moneyOut ? t.nInk : Tokens.ok)),
                  ],
                ),
              ),
            );
          },
        ),
      ],
    );
  }
}

/// The twelve-month summary the Year toggle shows instead of the grid.
class CalendarYear extends StatelessWidget {
  const CalendarYear({super.key, required this.months});

  final List<FinCalMonth> months;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final m in months)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 4),
            child: Row(
              children: [
                SizedBox(
                  width: 78,
                  child: Text(m.label,
                      style: TextStyle(
                          fontSize: 12,
                          fontWeight:
                              m.current ? FontWeight.w800 : FontWeight.w500,
                          color: m.current ? Tokens.secFinances : t.nInk2)),
                ),
                Expanded(child: MeterBarShim(pct: m.pct, on: m.current)),
                const SizedBox(width: 10),
                SizedBox(
                  width: 96,
                  child: Text(m.out,
                      textAlign: TextAlign.right,
                      style: TextStyle(
                          fontSize: 12,
                          fontWeight: FontWeight.w600,
                          color: t.nInk)),
                ),
                const SizedBox(width: 10),
                SizedBox(
                  width: 44,
                  child: Text(m.count == 0 ? '' : '${m.count}',
                      textAlign: TextAlign.right,
                      style: TextStyle(fontSize: 11, color: t.nInk3)),
                ),
              ],
            ),
          ),
      ],
    );
  }
}

/// A bar inside a chart row. Separate from `MeterBar` in the widgets file so the
/// charts do not depend on it for one call.
class MeterBarShim extends StatelessWidget {
  const MeterBarShim({super.key, required this.pct, this.on = false});

  final int pct;
  final bool on;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return LayoutBuilder(
      builder: (context, box) => SizedBox(
        height: 8,
        child: Stack(
          children: [
            Container(
              decoration: BoxDecoration(
                  color: t.nHair, borderRadius: BorderRadius.circular(8)),
            ),
            Container(
              width: box.maxWidth * pct.clamp(0, 100) / 100,
              decoration: BoxDecoration(
                color: on
                    ? Tokens.secFinances
                    : Tokens.secFinances.withValues(alpha: 0.4),
                borderRadius: BorderRadius.circular(8),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A savings-rate strip: one column per month, the current one lit. Months with
/// no income at all are drawn hollow rather than at zero — "unknown" and "saved
/// nothing" are not the same claim, and this section does not conflate them.
class SavingsStrip extends StatelessWidget {
  const SavingsStrip({super.key, required this.months, this.height = 78});

  final List<FinSavingsMonth> months;
  final double height;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (months.isEmpty) return const SizedBox.shrink();
    return SizedBox(
      height: height,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          for (final m in months)
            Expanded(
              child: Tooltip(
                message: m.known
                    ? '${m.label}: ${m.pct}%  ${m.amount}'
                    : '${m.label}: no income recorded',
                child: Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 3),
                  child: Column(
                    mainAxisAlignment: MainAxisAlignment.end,
                    children: [
                      Expanded(
                        child: FractionallySizedBox(
                          alignment: Alignment.bottomCenter,
                          heightFactor:
                              m.known ? (m.pct.clamp(2, 100)) / 100 : 1.0,
                          child: Container(
                            decoration: BoxDecoration(
                              color: !m.known
                                  ? Colors.transparent
                                  : m.current
                                      ? Tokens.ok
                                      : Tokens.ok.withValues(alpha: 0.35),
                              border: m.known
                                  ? null
                                  : Border.all(
                                      color: t.nHair, style: BorderStyle.solid),
                              borderRadius: const BorderRadius.vertical(
                                  top: Radius.circular(4)),
                            ),
                          ),
                        ),
                      ),
                      const SizedBox(height: 4),
                      Text(m.label,
                          style: TextStyle(fontSize: 9.5, color: t.nInk3)),
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

/// An envelope's burn-down: where the month's spending has got to, day by day,
/// against the straight line an even month would draw.
class BurnDown extends StatelessWidget {
  const BurnDown({super.key, required this.points, required this.tint});

  final List<int> points;
  final Color tint;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      height: 34,
      child: CustomPaint(
        size: Size.infinite,
        painter: _BurnPainter(points: points, tint: tint, pace: t.nHair),
      ),
    );
  }
}

class _BurnPainter extends CustomPainter {
  _BurnPainter({required this.points, required this.tint, required this.pace});

  final List<int> points;
  final Color tint;
  final Color pace;

  @override
  void paint(Canvas canvas, Size size) {
    if (points.length < 2) return;
    // The even-spend line, for the eye to measure the real one against.
    canvas.drawLine(
      Offset(0, size.height),
      Offset(size.width, 0),
      Paint()
        ..color = pace
        ..strokeWidth = 1,
    );
    final path = Path();
    for (var i = 0; i < points.length; i++) {
      final x = size.width * i / (points.length - 1);
      final y = size.height * (1 - points[i].clamp(0, 100) / 100);
      if (i == 0) {
        path.moveTo(x, y);
      } else {
        path.lineTo(x, y);
      }
    }
    canvas.drawPath(
      path,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2
        ..strokeJoin = StrokeJoin.round
        ..color = tint,
    );
  }

  @override
  bool shouldRepaint(_BurnPainter old) =>
      old.points != points || old.tint != tint || old.pace != pace;
}
