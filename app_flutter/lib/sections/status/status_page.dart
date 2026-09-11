// Status — the same dashboard the loopback HTML page draws, as a native
// section.
//
// Both exist on purpose. `crates/tulipix-status` serves a self-contained HTML
// page over loopback; this draws the identical snapshot inside the window. They
// read the *same* `collect()` JSON — there is no second set of queries and no
// second set of numbers — so the only thing that differs is the rendering.
//
// Two things the Slint page had to do by hand are gone here. The card grid
// wraps by itself instead of placing children by index arithmetic, and the
// seven-day sparkline is painted from its points rather than from two SVG path
// strings built in Rust. Both were Slint working around a missing primitive.

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/tokens.dart';
import '../../design/skin.dart';
import '../../src/rust/api/status.dart';
import 'status_controller.dart';
import 'status_dialogs.dart';

class StatusPage extends StatefulWidget {
  const StatusPage({super.key, required this.visible});

  /// The rail keeps every section alive, so the page has to be told when it is
  /// actually being looked at: `collect()` queries ten databases and a 2-second
  /// tick for a page nobody is on is exactly the work the slow tick avoids.
  final bool visible;

  @override
  State<StatusPage> createState() => _StatusPageState();
}

class _StatusPageState extends State<StatusPage> {
  final StatusController _c = StatusController();

  @override
  void initState() {
    super.initState();
    // Not straight: `initState` runs inside a build, and `send` notifies before
    // it awaits. Asking on the next frame keeps the first refresh out of the
    // build phase.
    WidgetsBinding.instance
        .addPostFrameCallback((_) => _c.watched = widget.visible);
  }

  @override
  void didUpdateWidget(StatusPage old) {
    super.didUpdateWidget(old);
    _c.watched = widget.visible;
  }

  @override
  void dispose() {
    _c.watched = false;
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
          color: t.bg,
          child: st == null
              ? FirstLoad(error: _c.error, onRetry: _c.refresh)
              : Column(
                  children: [
                    if (_c.error != null) _Banner(controller: _c),
                    if (_c.notice.isNotEmpty) _Notice(controller: _c),
                    Expanded(child: _Body(controller: _c, state: st)),
                  ],
                ),
        );
      },
    );
  }
}

class _Banner extends StatelessWidget {
  const _Banner({required this.controller});

  final StatusController controller;

  @override
  Widget build(BuildContext context) => Material(
        color: Tokens.error.withValues(alpha: 0.12),
        child: ListTile(
          dense: true,
          leading: const Icon(Icons.error_outline, color: Tokens.error),
          title: Text('${controller.error}',
              style: const TextStyle(fontSize: 12.5, color: Tokens.error)),
          trailing: IconButton(
            icon: const Icon(Icons.close, size: 18),
            onPressed: controller.clearError,
          ),
        ),
      );
}

class _Notice extends StatelessWidget {
  const _Notice({required this.controller});

  final StatusController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: Tokens.secStatus.withValues(alpha: 0.12),
      child: ListTile(
        dense: true,
        leading: const Icon(Icons.info_outline, color: Tokens.secStatus),
        title: Text(controller.notice,
            style: TextStyle(fontSize: 12.5, color: t.text)),
        trailing: IconButton(
          icon: const Icon(Icons.close, size: 18),
          onPressed: controller.clearNotice,
        ),
      ),
    );
  }
}

class _Body extends StatelessWidget {
  const _Body({required this.controller, required this.state});

  final StatusController controller;
  final StatusState state;

  @override
  Widget build(BuildContext context) => ListView(
        padding: const EdgeInsets.all(20),
        children: [
          _Header(controller: controller, state: state),
          const SizedBox(height: 16),
          _MetricStrip(controller: controller, state: state),
          const SizedBox(height: 16),
          _Cards(controller: controller, state: state),
          const SizedBox(height: 16),
          _BottomLists(state: state),
        ],
      );
}

// ── the card shell ──────────────────────────────────────────────────────────

/// A panel with a coloured top edge. The stripe is what makes a grid of ten
/// identical boxes readable at a glance.
class Panel extends StatelessWidget {
  const Panel({
    super.key,
    required this.child,
    this.tint = Tokens.secStatus,
    this.stripe = true,
    this.onTap,
  });

  final Widget child;
  final Color tint;
  final bool stripe;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final body = DecoratedBox(
      decoration: context.skin.surface(SurfaceRole.card, radius: 14) ??
          BoxDecoration(
            color: t.panel,
            borderRadius: BorderRadius.circular(14),
            border: Border.all(color: t.outline),
          ),
      child: ClipRRect(
        borderRadius: BorderRadius.circular(13),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (stripe) Container(height: 3, color: tint),
            // Not `Flexible`: these panels sit in a ListView, where a flex
            // child gets unbounded height and throws. A fixed-height caller
            // (a card, a metric tile) still bounds its own child through the
            // SizedBox around the panel.
            child,
          ],
        ),
      ),
    );
    if (onTap == null) return body;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(onTap: onTap, child: body),
    );
  }
}

/// The health dot.
class Lamp extends StatelessWidget {
  const Lamp({super.key, required this.level, this.size = 8});

  final String level;
  final double size;

  @override
  Widget build(BuildContext context) => Container(
        width: size,
        height: size,
        decoration:
            BoxDecoration(color: levelColor(level), shape: BoxShape.circle),
      );
}

/// A tinted round glyph. Every card, popup header, timeline entry and Today row
/// wears one, so the eye can skip the chrome and read the text.
class Glyph extends StatelessWidget {
  const Glyph({
    super.key,
    required this.icon,
    required this.tint,
    this.size = 38,
    this.inner = 18,
  });

  final IconData icon;
  final Color tint;
  final double size;
  final double inner;

  @override
  Widget build(BuildContext context) => Container(
        width: size,
        height: size,
        alignment: Alignment.center,
        decoration: context.skin
                .control(active: true, tint: tint, radius: size * 0.3) ??
            BoxDecoration(
              color: tint.withValues(alpha: 0.14),
              borderRadius: BorderRadius.circular(size * 0.3),
            ),
        child: Icon(context.skin.icon(icon), size: inner, color: tint),
      );
}

/// One `key: value` line. `cls` is the value's tint, carried verbatim from the
/// snapshot: "g" good, "o" attention, "rd" bad, "mut" muted, "" plain.
class KvRow extends StatelessWidget {
  const KvRow({super.key, required this.row});

  final StRow row;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        children: [
          Flexible(
            child: Text(row.k,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12, color: t.textDim)),
          ),
          const Spacer(),
          Text(row.v,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontSize: 12,
                fontWeight:
                    row.cls == 'mut' ? FontWeight.w500 : FontWeight.w600,
                color: clsColor(row.cls, t),
              )),
        ],
      ),
    );
  }
}

Color clsColor(String cls, Tokens t) => switch (cls) {
      'g' => Tokens.ok,
      'o' => Tokens.warn,
      'rd' => Tokens.error,
      'mut' => t.textDim,
      _ => t.text,
    };

/// A small pill. The status column of every popup table is one of these.
class Pill extends StatelessWidget {
  const Pill({super.key, required this.label, this.cls = ''});

  final String label;
  final String cls;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = switch (cls) {
      'b' => Tokens.error,
      'w' => Tokens.warn,
      'i' => t.textDim,
      _ => Tokens.ok,
    };
    return Container(
      height: 20,
      padding: const EdgeInsets.symmetric(horizontal: 9),
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: tint.withValues(alpha: 0.14),
        borderRadius: BorderRadius.circular(10),
      ),
      child: Text(label,
          style: TextStyle(
              fontSize: 10.5, fontWeight: FontWeight.w700, color: tint)),
    );
  }
}

/// The section-caption every popup block wears.
class Caption extends StatelessWidget {
  const Caption(this.text, {super.key});

  final String text;

  @override
  Widget build(BuildContext context) => Text(
        text,
        style: TextStyle(
          fontSize: 10.5,
          fontWeight: FontWeight.w700,
          letterSpacing: 0.6,
          color: context.tokens.textDim,
        ),
      );
}

class Btn extends StatelessWidget {
  const Btn({
    super.key,
    required this.label,
    required this.onTap,
    this.icon,
    this.tint = Tokens.brand,
    this.filled = true,
    this.enabled = true,
  });

  final String label;
  final VoidCallback onTap;
  final IconData? icon;
  final Color tint;
  final bool filled;
  final bool enabled;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Opacity(
      opacity: enabled ? 1 : 0.5,
      child: SizedBox(
        height: 36,
        child: filled
            ? FilledButton.icon(
                onPressed: enabled ? onTap : null,
                icon: icon == null ? null : Icon(icon, size: 14),
                label: Text(label,
                    style: const TextStyle(
                        fontSize: 12.5, fontWeight: FontWeight.w700)),
                style: FilledButton.styleFrom(backgroundColor: tint),
              )
            : OutlinedButton.icon(
                onPressed: enabled ? onTap : null,
                icon: icon == null ? null : Icon(icon, size: 14, color: tint),
                label: Text(label,
                    style: TextStyle(
                        fontSize: 12.5,
                        fontWeight: FontWeight.w700,
                        color: t.text)),
                style: OutlinedButton.styleFrom(
                    side: BorderSide(color: t.outline)),
              ),
      ),
    );
  }
}

// ── header ──────────────────────────────────────────────────────────────────

class _Header extends StatelessWidget {
  const _Header({required this.controller, required this.state});

  final StatusController controller;
  final StatusState state;

  @override
  Widget build(BuildContext context) {
    final st = state;
    return Panel(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(18, 18, 18, 18),
        child: LayoutBuilder(
          builder: (context, box) {
            final wide = box.maxWidth > 820;
            final left = Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              mainAxisSize: MainAxisSize.min,
              children: [
                _Verdict(state: st),
                const SizedBox(height: 16),
                _Figures(controller: controller, state: st),
              ],
            );
            final right = _LibraryCard(controller: controller, state: st);
            if (!wide) {
              return Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [left, const SizedBox(height: 16), right],
              );
            }
            // The two columns are the same height, which needs one measuring
            // pass: the panel is in a ListView and has no height of its own.
            return IntrinsicHeight(
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Expanded(child: left),
                  const SizedBox(width: 18),
                  SizedBox(width: 292, child: right),
                ],
              ),
            );
          },
        ),
      ),
    );
  }
}

class _Verdict extends StatelessWidget {
  const _Verdict({required this.state});

  final StatusState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final problem = state.level == 'problem';
    final tint = problem
        ? Tokens.error
        : state.level == 'busy'
            ? const Color(0xFF10B981)
            : Tokens.ok;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Container(
          width: 54,
          height: 54,
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: tint.withValues(alpha: 0.16),
            shape: BoxShape.circle,
          ),
          child: Icon(problem ? Icons.warning_amber_rounded : Icons.check,
              size: 26, color: tint),
        ),
        const SizedBox(width: 14),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(state.headline,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 21,
                      fontWeight: FontWeight.w800,
                      color: t.text)),
              const SizedBox(height: 4),
              Text(state.note,
                  style: TextStyle(fontSize: 12.5, color: t.textDim)),
              if (state.updated.isNotEmpty) ...[
                const SizedBox(height: 4),
                Text('updated ${state.updated}',
                    style: TextStyle(fontSize: 11, color: t.textDim)),
              ],
            ],
          ),
        ),
      ],
    );
  }
}

/// The three figures. Each opens the card that produced it — "3 failed jobs" is
/// the beginning of a question, and the answer is one click away.
class _Figures extends StatelessWidget {
  const _Figures({required this.controller, required this.state});

  final StatusController controller;
  final StatusState state;

  @override
  Widget build(BuildContext context) {
    final tiles = <Widget>[
      _Figure(
        value: state.sectionsOnline,
        label: 'Sections Online',
        tint: const Color(0xFF22C55E),
        icon: Icons.check,
        onTap: () => openCard(context, controller, state, state.systemIdx),
      ),
      _Figure(
        value: state.jobsRunning,
        label: 'Jobs Running',
        tint: const Color(0xFF6366F1),
        icon: Icons.schedule,
        onTap: () => openCard(context, controller, state, state.toolsIdx),
      ),
      _Figure(
        value: state.jobsFailed,
        label: 'Failed Jobs',
        tint: const Color(0xFFEF4444),
        icon: Icons.warning_amber_rounded,
        onTap: () => openCard(context, controller, state, state.toolsIdx),
      ),
    ];
    return Row(
      children: [
        for (var i = 0; i < tiles.length; i++) ...[
          if (i > 0) const SizedBox(width: 10),
          Expanded(child: tiles[i]),
        ],
      ],
    );
  }
}

class _Figure extends StatelessWidget {
  const _Figure({
    required this.value,
    required this.label,
    required this.tint,
    required this.icon,
    required this.onTap,
  });

  final String value;
  final String label;
  final Color tint;
  final IconData icon;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onTap,
        child: Container(
          constraints: const BoxConstraints(minHeight: 76),
          padding: const EdgeInsets.all(13),
          decoration: context.skin.surface(SurfaceRole.card, radius: 11) ??
              BoxDecoration(
                color: tint.withValues(alpha: 0.08),
                borderRadius: BorderRadius.circular(11),
                border: Border.all(color: tint.withValues(alpha: 0.22)),
              ),
          child: Row(
            children: [
              Glyph(icon: icon, tint: tint, size: 36, inner: 17),
              const SizedBox(width: 11),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    Text(value,
                        style: TextStyle(
                            fontSize: 21,
                            fontWeight: FontWeight.w800,
                            color: t.text)),
                    Text(label,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 11,
                            fontWeight: FontWeight.w600,
                            color: t.textDim)),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// How big the library is, how it moved this week, and the one button that acts
/// on all of it.
class _LibraryCard extends StatelessWidget {
  const _LibraryCard({required this.controller, required this.state});

  final StatusController controller;
  final StatusState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: context.skin.surface(SurfaceRole.card, radius: 12) ??
          BoxDecoration(
            color: t.panel2,
            borderRadius: BorderRadius.circular(12),
            border: Border.all(color: t.outline),
          ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(state.libraryBytes,
              style: TextStyle(
                  fontSize: 27, fontWeight: FontWeight.w800, color: t.text)),
          Text('${state.libraryItems} items indexed',
              style: TextStyle(fontSize: 12, color: t.textDim)),
          const SizedBox(height: 6),
          Align(
            alignment: Alignment.centerLeft,
            child: Container(
              height: 22,
              padding: const EdgeInsets.symmetric(horizontal: 10),
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: Tokens.secStatus.withValues(alpha: 0.14),
                borderRadius: BorderRadius.circular(11),
              ),
              child: Text('+${state.libraryWeek} this week',
                  style: const TextStyle(
                      fontSize: 11,
                      fontWeight: FontWeight.w700,
                      color: Tokens.secStatus)),
            ),
          ),
          const SizedBox(height: 8),
          SizedBox(
            height: 54,
            child: CustomPaint(
              painter: _SparkPainter(state.spark, Tokens.secStatus),
              size: Size.infinite,
            ),
          ),
          const SizedBox(height: 10),
          SizedBox(
            width: double.infinity,
            child: Btn(
              label: 'Rescan Libraries',
              icon: Icons.refresh,
              tint: Tokens.secStatus,
              onTap: () => openLibrary(context, controller, state),
            ),
          ),
        ],
      ),
    );
  }
}

/// Seven days of library growth. Fewer than two readings is not a line, and a
/// flat week is a straight one rather than a division by zero.
class _SparkPainter extends CustomPainter {
  const _SparkPainter(this.points, this.tint);

  final List<double> points;
  final Color tint;

  @override
  void paint(Canvas canvas, Size size) {
    if (points.length < 2) return;
    final lo = points.reduce((a, b) => a < b ? a : b);
    final hi = points.reduce((a, b) => a > b ? a : b);
    final span = hi > lo ? hi - lo : 1.0;
    final n = points.length - 1;

    final line = Path();
    for (var i = 0; i < points.length; i++) {
      final x = i / n * size.width;
      final y = size.height * (92 - (points[i] - lo) / span * 84) / 100;
      i == 0 ? line.moveTo(x, y) : line.lineTo(x, y);
    }
    final area = Path.from(line)
      ..lineTo(size.width, size.height)
      ..lineTo(0, size.height)
      ..close();

    canvas.drawPath(area, Paint()..color = tint.withValues(alpha: 0.22));
    canvas.drawPath(
      line,
      Paint()
        ..color = tint
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2,
    );
  }

  @override
  bool shouldRepaint(_SparkPainter old) =>
      old.tint != tint || !_same(old.points, points);

  static bool _same(List<double> a, List<double> b) {
    if (a.length != b.length) return false;
    for (var i = 0; i < a.length; i++) {
      if (a[i] != b[i]) return false;
    }
    return true;
  }
}

// ── metric strip ────────────────────────────────────────────────────────────

class _MetricStrip extends StatelessWidget {
  const _MetricStrip({required this.controller, required this.state});

  final StatusController controller;
  final StatusState state;

  @override
  Widget build(BuildContext context) {
    if (state.metrics.isEmpty) return const SizedBox.shrink();
    return LayoutBuilder(
      builder: (context, box) {
        // Five across on a normal window, fewer as it narrows — the Slint page
        // put them all on one row and let them squeeze.
        final cols =
            (box.maxWidth / 190).floor().clamp(1, state.metrics.length);
        final w = (box.maxWidth - (cols - 1) * 12) / cols;
        return Wrap(
          spacing: 12,
          runSpacing: 12,
          children: [
            for (var i = 0; i < state.metrics.length; i++)
              SizedBox(
                width: w,
                height: 104,
                child: _MetricTile(
                  metric: state.metrics[i],
                  onTap: () => openMetric(context, controller, state, i),
                ),
              ),
          ],
        );
      },
    );
  }
}

class _MetricTile extends StatelessWidget {
  const _MetricTile({required this.metric, required this.onTap});

  final StMetric metric;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = accentOf(metric.accent);
    return Panel(
      tint: tint,
      onTap: onTap,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(13, 13, 13, 13),
        child: Stack(
          children: [
            Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(metric.label,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 10.5,
                        fontWeight: FontWeight.w700,
                        letterSpacing: 0.4,
                        color: t.textDim)),
                const SizedBox(height: 7),
                Text(metric.value,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 18,
                        fontWeight: FontWeight.w700,
                        color: t.text)),
                Text(metric.note,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11, color: t.textDim)),
              ],
            ),
            // The "?" badge. It is the affordance: without it a tile that opens
            // an explanation looks like a tile that does not.
            Positioned(
              right: 0,
              top: 0,
              child: Container(
                width: 17,
                height: 17,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: tint.withValues(alpha: 0.14),
                  shape: BoxShape.circle,
                ),
                child: Text('?',
                    style: TextStyle(
                        fontSize: 10.5,
                        fontWeight: FontWeight.w800,
                        color: tint)),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ── the section cards ───────────────────────────────────────────────────────

class _Cards extends StatelessWidget {
  const _Cards({required this.controller, required this.state});

  final StatusController controller;
  final StatusState state;

  @override
  Widget build(BuildContext context) {
    if (state.cards.isEmpty) return const SizedBox.shrink();
    return LayoutBuilder(
      builder: (context, box) {
        final cols = (box.maxWidth / 232).floor().clamp(1, 5);
        final w = (box.maxWidth - (cols - 1) * 12) / cols;
        return Wrap(
          spacing: 12,
          runSpacing: 12,
          children: [
            for (var i = 0; i < state.cards.length; i++)
              SizedBox(
                width: w,
                height: 268,
                child: _Card(
                  card: state.cards[i],
                  onOpen: () => openCard(context, controller, state, i),
                ),
              ),
          ],
        );
      },
    );
  }
}

class _Card extends StatelessWidget {
  const _Card({required this.card, required this.onOpen});

  final StCard card;
  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = accentOf(card.accent);
    return Panel(
      tint: tint,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(14, 14, 14, 14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Glyph(icon: iconForKey(card.key), tint: tint),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      Text(card.name,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 13.5,
                              fontWeight: FontWeight.w700,
                              color: t.text)),
                      const SizedBox(height: 3),
                      Row(
                        children: [
                          Lamp(level: card.level),
                          const SizedBox(width: 6),
                          Expanded(
                            child: Text(card.status,
                                overflow: TextOverflow.ellipsis,
                                style:
                                    TextStyle(fontSize: 11, color: t.textDim)),
                          ),
                        ],
                      ),
                    ],
                  ),
                ),
              ],
            ),
            const SizedBox(height: 10),
            // The rows take the slack, which is what lands every card's button
            // on the same line across the row.
            Expanded(
              child: ListView(
                padding: EdgeInsets.zero,
                physics: const NeverScrollableScrollPhysics(),
                children: [for (final r in card.rows) KvRow(row: r)],
              ),
            ),
            // The one line that says what is wrong, rather than that something
            // is. Only drawn when there is one.
            if (card.why.isNotEmpty) ...[
              Container(
                padding: const EdgeInsets.all(8),
                decoration: BoxDecoration(
                  color: Tokens.error.withValues(alpha: 0.09),
                  borderRadius: BorderRadius.circular(8),
                ),
                child: Text(card.why,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(fontSize: 11, color: Tokens.error)),
              ),
              const SizedBox(height: 8),
            ],
            Btn(
              label: card.more.isNotEmpty ? card.more : 'View Details',
              tint: tint,
              filled: false,
              onTap: onOpen,
            ),
          ],
        ),
      ),
    );
  }
}

// ── the two bottom lists ────────────────────────────────────────────────────

class _BottomLists extends StatelessWidget {
  const _BottomLists({required this.state});

  final StatusState state;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
        builder: (context, box) {
          final timeline = _Timeline(state: state);
          final today = _Today(state: state);
          if (box.maxWidth < 720) {
            return Column(
              children: [timeline, const SizedBox(height: 12), today],
            );
          }
          return Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(child: timeline),
              const SizedBox(width: 12),
              Expanded(child: today),
            ],
          );
        },
      );
}

class _Timeline extends StatelessWidget {
  const _Timeline({required this.state});

  final StatusState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Panel(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(14, 14, 14, 14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text('Activity Timeline',
                      style: TextStyle(
                          fontSize: 13.5,
                          fontWeight: FontWeight.w700,
                          color: t.text)),
                ),
                const Pill(label: 'Recent', cls: 'i'),
              ],
            ),
            const SizedBox(height: 10),
            if (state.timeline.isEmpty)
              Text('Nothing yet.',
                  style: TextStyle(fontSize: 12, color: t.textDim)),
            for (final e in state.timeline)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 5),
                child: Row(
                  children: [
                    SizedBox(
                      width: 52,
                      child: Text(e.at,
                          style: TextStyle(
                              fontSize: 11,
                              fontWeight: FontWeight.w600,
                              color: t.textDim)),
                    ),
                    const SizedBox(width: 10),
                    Glyph(
                        icon: Icons.check,
                        tint: accentOf(e.accent),
                        size: 22,
                        inner: 11),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(e.title,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                  fontSize: 12,
                                  fontWeight: FontWeight.w600,
                                  color: t.text)),
                          Text(e.desc,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(fontSize: 11, color: t.textDim)),
                        ],
                      ),
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

class _Today extends StatelessWidget {
  const _Today({required this.state});

  final StatusState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Panel(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(14, 14, 14, 14),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            Text('Today',
                style: TextStyle(
                    fontSize: 13.5,
                    fontWeight: FontWeight.w700,
                    color: t.text)),
            const SizedBox(height: 10),
            if (state.today.isEmpty)
              Text('Nothing yet.',
                  style: TextStyle(fontSize: 12, color: t.textDim)),
            for (final x in state.today)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 5),
                child: Row(
                  children: [
                    Glyph(
                        icon: Icons.bolt,
                        tint: accentOf(x.accent),
                        size: 22,
                        inner: 11),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Text(x.label,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 12, color: t.text)),
                    ),
                    Text(x.value,
                        style: TextStyle(
                            fontSize: 12,
                            fontWeight: FontWeight.w700,
                            color: accentOf(x.accent))),
                  ],
                ),
              ),
          ],
        ),
      ),
    );
  }
}
