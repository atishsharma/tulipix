// Status — every section's health, what is running, and what changed today
// (docs/mockups/settings-tabs.html, the Status tab).
//
// Both this and the loopback HTML page exist on purpose, and they read the
// *same* `collect()` JSON — there is no second set of queries and no second
// set of numbers — so only the drawing differs. Top to bottom: one health
// band with the headline figures and the week's growth, a card per section
// that opens its details in a drawer, then the scans, today, the timeline and
// the metrics, each of which explains itself when tapped.

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/status.dart';
import '../settings/settings_kit.dart';
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

  /// The card the drawer shows, by key — `collect()` is free to reorder — and
  /// whether it is open. The key outlives a close, so the drawer still has
  /// something to draw while it slides away.
  String? _shown;
  bool _open = false;

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

  /// A card, or a figure standing for one: open its details, or shut them if
  /// they are the ones showing.
  void _toggle(String key) => setState(() {
        if (_open && _shown == key) {
          _open = false;
        } else {
          _shown = key;
          _open = true;
        }
      });

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
                    Expanded(
                      child: Stack(
                        children: [
                          Positioned.fill(child: _body(st)),
                          // Under the drawer, over the page: a click anywhere
                          // outside the drawer closes it.
                          Positioned.fill(
                            child: IgnorePointer(
                              ignoring: !_open,
                              child: GestureDetector(
                                behavior: HitTestBehavior.opaque,
                                onTap: () => setState(() => _open = false),
                                child: AnimatedOpacity(
                                  opacity: _open ? 1 : 0,
                                  duration: t.reduceMotion
                                      ? Duration.zero
                                      : const Duration(milliseconds: 200),
                                  child: const ColoredBox(
                                      color: Color(0x33000000)),
                                ),
                              ),
                            ),
                          ),
                          _drawer(st),
                        ],
                      ),
                    ),
                  ],
                ),
        );
      },
    );
  }

  Widget _body(StatusState st) {
    final t = context.tokens;
    return SettingsPageBody(
      head: SettingsHead.forTab(
        'status',
        note: 'Every section\'s health, what is running, and what changed '
            'today',
        actions: [
          if (st.updated.isNotEmpty)
            Text('Updated ${st.updated}',
                style: TextStyle(fontSize: 11, color: t.textDim)),
          SmallBtn(
            label: _c.rescanning ? 'Rescanning…' : 'Rescan',
            icon: Icons.refresh,
            large: true,
            primary: true,
            busy: _c.rescanning,
            onTap: _c.rescan,
          ),
        ],
      ),
      children: [
        _Health(state: st, onCard: _toggle),
        _SectionCards(state: st, open: _open ? _shown : null, onOpen: _toggle),
        TileGrid([
          (span: 3, child: _Scans(state: st, rescanning: _c.rescanning)),
          (span: 3, child: _Today(state: st)),
          (span: 3, child: _Timeline(state: st)),
          (
            span: 3,
            child: _System(
              state: st,
              onMetric: (i) => openMetric(context, _c, st, i),
            ),
          ),
        ]),
      ],
    );
  }

  Widget _drawer(StatusState st) {
    StCard? card;
    for (final x in st.cards) {
      if (x.key == _shown) card = x;
    }
    final shown = card;
    final open = _open && shown != null;
    return Positioned(
      top: 0,
      right: 0,
      bottom: 0,
      width: 380,
      child: IgnorePointer(
        ignoring: !open,
        child: AnimatedSlide(
          offset: open ? Offset.zero : const Offset(1.05, 0),
          duration: context.tokens.reduceMotion
              ? Duration.zero
              : const Duration(milliseconds: 280),
          curve: Curves.easeOutCubic,
          child: shown == null
              ? const SizedBox.shrink()
              : _Drawer(
                  card: shown,
                  onClose: () => setState(() => _open = false),
                ),
        ),
      ),
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

// ── small shared pieces (the popups use these too) ──────────────────────────

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

/// A tinted rounded glyph.
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

/// A popup or drawer's ground: its panel colour laid over an opaque base, so
/// nothing behind it shows through whatever the theme's panel alpha is.
Color solidPanel(Tokens t) => Color.alphaBlend(
      t.panel2,
      Color.alphaBlend(t.bg, t.dark ? Colors.black : Colors.white),
    );

/// One `key: value` line. `cls` is the value's tint, carried verbatim from the
/// snapshot: "g" good, "o" attention, "rd" bad, "mut" muted, "" plain.
///
/// Two equal columns, the value centred in its own: every value in a list
/// then sits on one line down the middle, whatever the length of its label.
class KvRow extends StatelessWidget {
  const KvRow({super.key, required this.row});

  final StRow row;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 3),
      child: Row(
        children: [
          Expanded(
            child: Text(row.k,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12, color: t.textDim)),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Text(row.v,
                textAlign: TextAlign.center,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: 12,
                  fontWeight:
                      row.cls == 'mut' ? FontWeight.w500 : FontWeight.w600,
                  color: clsColor(row.cls, t),
                )),
          ),
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

/// The caption every details block wears.
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

// ── the health band ─────────────────────────────────────────────────────────

/// The verdict, the three figures that matter, and the week's growth. The
/// verdict opens System and the job figures open Tools: "1 failed" is the
/// start of a question, and the answer is a click away.
class _Health extends StatelessWidget {
  const _Health({required this.state, required this.onCard});

  final StatusState state;
  final ValueChanged<String> onCard;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = state;
    final tint = levelColor(st.level);
    final verdict = InkWell(
      borderRadius: BorderRadius.circular(10),
      onTap: () => onCard('system'),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 14,
            height: 14,
            decoration: BoxDecoration(
              color: tint,
              shape: BoxShape.circle,
              boxShadow: [
                BoxShadow(
                    color: tint.withValues(alpha: 0.55),
                    blurRadius: 10,
                    spreadRadius: 1),
              ],
            ),
          ),
          const SizedBox(width: 14),
          Flexible(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(st.headline,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 20,
                        fontWeight: FontWeight.w800,
                        color: t.text)),
                Text(st.note,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 12, color: t.textDim)),
              ],
            ),
          ),
        ],
      ),
    );
    final figs = [
      _Fig(
        label: 'Running',
        value: '${st.jobsRunning} ${st.jobsRunning == '1' ? 'job' : 'jobs'}',
        onTap: () => onCard('tools'),
      ),
      _Fig(
        label: 'Failed',
        value: st.jobsFailed,
        bad: st.jobsFailed != '0',
        onTap: () => onCard('tools'),
      ),
      _Fig(label: 'Library', value: st.libraryItems),
    ];
    final spark = Column(
      crossAxisAlignment: CrossAxisAlignment.end,
      mainAxisSize: MainAxisSize.min,
      children: [
        Text('LIBRARY GROWTH · 7 DAYS · +${st.libraryWeek}',
            style: TextStyle(
                fontSize: 10.5,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.6,
                color: t.textDim)),
        const SizedBox(height: 4),
        SizedBox(
          width: 180,
          height: 48,
          child: CustomPaint(painter: _SparkPainter(st.spark, tint)),
        ),
      ],
    );
    return Container(
      decoration: context.skin.surface(SurfaceRole.card, radius: 18) ??
          BoxDecoration(
            color: t.panel2,
            borderRadius: BorderRadius.circular(18),
            border: Border.all(color: tint.withValues(alpha: 0.3)),
          ),
      child: Container(
        padding: const EdgeInsets.fromLTRB(20, 18, 20, 18),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(18),
          gradient: RadialGradient(
            center: Alignment.topLeft,
            radius: 1.6,
            colors: [tint.withValues(alpha: 0.14), tint.withValues(alpha: 0)],
          ),
        ),
        child: LayoutBuilder(
          builder: (context, box) {
            if (box.maxWidth < 820) {
              return Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  verdict,
                  const SizedBox(height: 16),
                  Wrap(
                    runSpacing: 12,
                    crossAxisAlignment: WrapCrossAlignment.end,
                    children: [
                      ...figs,
                      Padding(
                          padding: const EdgeInsets.only(left: 22),
                          child: spark),
                    ],
                  ),
                ],
              );
            }
            return Row(
              children: [
                Expanded(
                  child: Align(alignment: Alignment.centerLeft, child: verdict),
                ),
                ...figs,
                const SizedBox(width: 22),
                spark,
              ],
            );
          },
        ),
      ),
    );
  }
}

class _Fig extends StatelessWidget {
  const _Fig({
    required this.label,
    required this.value,
    this.onTap,
    this.bad = false,
  });

  final String label;
  final String value;
  final VoidCallback? onTap;
  final bool bad;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final body = Container(
      padding: const EdgeInsets.symmetric(horizontal: 22),
      decoration: BoxDecoration(
        border: Border(left: BorderSide(color: t.outline)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(label.toUpperCase(),
              style: TextStyle(
                  fontSize: 10.5,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.6,
                  color: t.textDim)),
          Text(value,
              style: TextStyle(
                  fontSize: 19,
                  fontWeight: FontWeight.w800,
                  color: bad ? Tokens.error : t.text,
                  fontFeatures: const [FontFeature.tabularFigures()])),
        ],
      ),
    );
    final tap = onTap;
    return tap == null ? body : InkWell(onTap: tap, child: body);
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
    Offset at(int i) => Offset(
          i / n * size.width,
          2 + (1 - (points[i] - lo) / span) * (size.height - 6),
        );

    final line = Path()..moveTo(at(0).dx, at(0).dy);
    for (var i = 1; i < points.length; i++) {
      line.lineTo(at(i).dx, at(i).dy);
    }
    final area = Path.from(line)
      ..lineTo(size.width, size.height)
      ..lineTo(0, size.height)
      ..close();

    canvas.drawPath(
      area,
      Paint()
        ..shader = LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [tint.withValues(alpha: 0.35), tint.withValues(alpha: 0)],
        ).createShader(Offset.zero & size),
    );
    canvas.drawPath(
      line,
      Paint()
        ..color = tint
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2,
    );
    canvas.drawCircle(at(n), 3.5, Paint()..color = tint);
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

// ── the section cards ───────────────────────────────────────────────────────

class _SectionCards extends StatelessWidget {
  const _SectionCards({
    required this.state,
    required this.open,
    required this.onOpen,
  });

  final StatusState state;

  /// The key of the card whose drawer is open.
  final String? open;
  final ValueChanged<String> onOpen;

  @override
  Widget build(BuildContext context) {
    final cards = state.cards;
    if (cards.isEmpty) return const SizedBox.shrink();
    return LayoutBuilder(
      builder: (context, box) {
        final per = (box.maxWidth / 200).floor().clamp(2, 5).toInt();
        return Column(
          children: [
            for (var i = 0; i < cards.length; i += per) ...[
              if (i > 0) const SizedBox(height: 10),
              IntrinsicHeight(
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    for (var j = i; j < i + per; j++) ...[
                      if (j > i) const SizedBox(width: 10),
                      Expanded(
                        child: j < cards.length
                            ? _SectionCard(
                                card: cards[j],
                                selected: cards[j].key == open,
                                onTap: () => onOpen(cards[j].key),
                              )
                            : const SizedBox.shrink(),
                      ),
                    ],
                  ],
                ),
              ),
            ],
          ],
        );
      },
    );
  }
}

/// A section at a glance: its figure, its state, and what is wrong when
/// something is. The first of its details' figures is the one on the face.
class _SectionCard extends StatelessWidget {
  const _SectionCard({
    required this.card,
    required this.selected,
    required this.onTap,
  });

  final StCard card;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = accentOf(card.accent);
    final (fig, unit) = card.kpis.isNotEmpty
        ? (card.kpis.first.v, card.kpis.first.k)
        : card.rows.isNotEmpty
            ? (card.rows.first.v, card.rows.first.k)
            : ('', '');
    final r = BorderRadius.circular(14);
    return Container(
      clipBehavior: Clip.antiAlias,
      decoration: context.skin.surface(SurfaceRole.card, radius: 14) ??
          BoxDecoration(
            color: t.panel2,
            borderRadius: r,
            border: Border.all(color: t.outline),
          ),
      foregroundDecoration: selected
          ? BoxDecoration(
              borderRadius: r,
              border:
                  Border.all(color: tint.withValues(alpha: 0.75), width: 1.5),
            )
          : null,
      child: Material(
        color: Colors.transparent,
        child: InkWell(
          onTap: onTap,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Container(height: 3, color: tint),
              Padding(
                padding: const EdgeInsets.fromLTRB(12, 10, 12, 12),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Icon(context.skin.icon(iconForKey(card.key)),
                            size: 16, color: tint),
                        const SizedBox(width: 8),
                        Expanded(
                          child: Text(card.name,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                  fontSize: 13,
                                  fontWeight: FontWeight.w700,
                                  color: t.text)),
                        ),
                        Lamp(level: card.level),
                      ],
                    ),
                    if (fig.isNotEmpty) ...[
                      const SizedBox(height: 8),
                      Row(
                        crossAxisAlignment: CrossAxisAlignment.baseline,
                        textBaseline: TextBaseline.alphabetic,
                        children: [
                          Flexible(
                            child: Text(fig,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(
                                    fontSize: 18,
                                    fontWeight: FontWeight.w800,
                                    color: t.text)),
                          ),
                          const SizedBox(width: 6),
                          Flexible(
                            child: Text(unit,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style:
                                    TextStyle(fontSize: 11, color: t.textDim)),
                          ),
                        ],
                      ),
                    ],
                    const SizedBox(height: 4),
                    Text(card.status,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11.5, color: t.textDim)),
                    if (card.why.isNotEmpty) ...[
                      const SizedBox(height: 6),
                      Text(card.why,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                              fontSize: 11, color: Tokens.error)),
                    ],
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

/// One card's details, over the right edge of the page.
class _Drawer extends StatelessWidget {
  const _Drawer({required this.card, required this.onClose});

  final StCard card;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: solidPanel(t),
      surfaceTintColor: Colors.transparent,
      elevation: 16,
      child: DecoratedBox(
        decoration: BoxDecoration(
          border: Border(left: BorderSide(color: t.outlineStrong)),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(18, 18, 10, 14),
              child: Row(
                children: [
                  Glyph(
                      icon: iconForKey(card.key),
                      tint: accentOf(card.accent),
                      size: 36,
                      inner: 17),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(card.name,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontSize: 17,
                                fontWeight: FontWeight.w800,
                                color: t.text)),
                        Row(
                          children: [
                            Lamp(level: card.level),
                            const SizedBox(width: 6),
                            Expanded(
                              child: Text(card.status,
                                  overflow: TextOverflow.ellipsis,
                                  style: TextStyle(
                                      fontSize: 12, color: t.textDim)),
                            ),
                          ],
                        ),
                      ],
                    ),
                  ),
                  IconButton(
                    tooltip: 'Close',
                    icon: const Icon(Icons.close, size: 18),
                    onPressed: onClose,
                  ),
                ],
              ),
            ),
            Divider(height: 1, color: t.outline),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.fromLTRB(18, 16, 18, 18),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    if (card.rows.isNotEmpty) ...[
                      const Caption('NOW'),
                      const SizedBox(height: 6),
                      for (final r in card.rows) KvRow(row: r),
                      const SizedBox(height: 16),
                    ],
                    CardDetail(card: card),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ── the four tiles ──────────────────────────────────────────────────────────

class _Scans extends StatelessWidget {
  const _Scans({required this.state, required this.rescanning});

  final StatusState state;
  final bool rescanning;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final scans = state.scan;
    final active = scans.where((s) => s.active).length;
    final running = state.scanRunning || rescanning;
    return SettingsTile(
      icon: Icons.sync,
      tint: Tokens.brand,
      title: 'Scans',
      note: 'Each section reading its folders',
      trailing: [
        StateChip(
          running ? (active > 0 ? '$active running' : 'Running') : 'All done',
          tint: running ? Tokens.brand : Tokens.ok,
        ),
      ],
      child: scans.isEmpty
          ? Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(
                    rescanning
                        ? 'Rescanning every watched folder…'
                        : 'Nothing is scanning. Rescan reads every watched '
                            'folder again.',
                    style: TextStyle(fontSize: 12, color: t.textDim)),
                if (rescanning) ...[
                  const SizedBox(height: 10),
                  ClipRRect(
                    borderRadius: BorderRadius.circular(3),
                    child: LinearProgressIndicator(
                      minHeight: 6,
                      backgroundColor: t.outline,
                      color: Tokens.brand,
                    ),
                  ),
                ],
              ],
            )
          : Column(children: [for (final s in scans) _ScanLine(scan: s)]),
    );
  }
}

class _ScanLine extends StatelessWidget {
  const _ScanLine({required this.scan});

  final StScan scan;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = scan;
    final note = s.waiting
        ? 'counting files…'
        : !s.active && s.done == s.total
            ? 'up to date'
            : '${s.done} / ${s.total}'
                '${s.failed != '0' ? ' · ${s.failed} failed' : ''}';
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 5),
      child: Row(
        children: [
          SizedBox(
            width: 90,
            child: Text(s.section,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 12.5, fontWeight: FontWeight.w600, color: t.text)),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: ClipRRect(
              borderRadius: BorderRadius.circular(3),
              child: LinearProgressIndicator(
                // A scan still counting its files has nothing honest to show.
                value: s.waiting ? null : s.pct.clamp(0, 100) / 100,
                minHeight: 6,
                backgroundColor: t.outline,
                color: s.active ? Tokens.brand : Tokens.ok,
              ),
            ),
          ),
          const SizedBox(width: 12),
          SizedBox(
            width: 110,
            child: Text(note,
                textAlign: TextAlign.right,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 11.5, color: t.textDim)),
          ),
        ],
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
    final today = state.today;
    Widget cell(StToday x) => Container(
          padding: const EdgeInsets.fromLTRB(12, 10, 12, 10),
          decoration: BoxDecoration(
            color: t.bg,
            borderRadius: BorderRadius.circular(12),
            border: Border.all(color: t.outline),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(x.label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11, color: t.textDim)),
              const SizedBox(height: 2),
              Text(x.value,
                  style: TextStyle(
                      fontSize: 18,
                      fontWeight: FontWeight.w800,
                      color: accentOf(x.accent))),
            ],
          ),
        );
    return SettingsTile(
      icon: Icons.today_outlined,
      tint: const Color(0xFF0EA5E9),
      title: 'Today',
      note: 'Since midnight',
      child: today.isEmpty
          ? Text('Nothing yet.',
              style: TextStyle(fontSize: 12, color: t.textDim))
          : Column(
              children: [
                for (var i = 0; i < today.length; i += 2) ...[
                  if (i > 0) const SizedBox(height: 8),
                  IntrinsicHeight(
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        Expanded(child: cell(today[i])),
                        const SizedBox(width: 8),
                        Expanded(
                          child: i + 1 < today.length
                              ? cell(today[i + 1])
                              : const SizedBox.shrink(),
                        ),
                      ],
                    ),
                  ),
                ],
              ],
            ),
    );
  }
}

class _Timeline extends StatelessWidget {
  const _Timeline({required this.state});

  final StatusState state;

  /// The newest few; the browser dashboard keeps the full list.
  static const int _shown = 6;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final events = state.timeline;
    return SettingsTile(
      icon: Icons.history,
      tint: Tokens.warn,
      title: 'What happened',
      note: 'The latest from every section',
      child: events.isEmpty
          ? Text('Nothing yet.',
              style: TextStyle(fontSize: 12, color: t.textDim))
          : Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (final e in events.take(_shown))
                  Padding(
                    padding: const EdgeInsets.symmetric(vertical: 6),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        SizedBox(
                          width: 52,
                          child: Text(e.at,
                              style: TextStyle(
                                  fontSize: 11,
                                  fontWeight: FontWeight.w600,
                                  color: t.textDim)),
                        ),
                        Container(
                          width: 10,
                          height: 10,
                          margin: const EdgeInsets.only(top: 3),
                          decoration: BoxDecoration(
                            color: accentOf(e.accent),
                            shape: BoxShape.circle,
                          ),
                        ),
                        const SizedBox(width: 10),
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              Text(e.title,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: TextStyle(
                                      fontSize: 12.5,
                                      fontWeight: FontWeight.w600,
                                      color: t.text)),
                              Text(e.desc,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: TextStyle(
                                      fontSize: 11.5, color: t.textDim)),
                            ],
                          ),
                        ),
                      ],
                    ),
                  ),
              ],
            ),
    );
  }
}

/// The metrics, one line each. Every one explains itself, because a number
/// nobody can explain is a number nobody trusts.
class _System extends StatelessWidget {
  const _System({required this.state, required this.onMetric});

  final StatusState state;
  final ValueChanged<int> onMetric;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final metrics = state.metrics;
    return SettingsTile(
      icon: Icons.memory,
      tint: const Color(0xFF64748B),
      title: 'System',
      note: 'This session. Open one for what it means',
      child: metrics.isEmpty
          ? Text('Nothing yet.',
              style: TextStyle(fontSize: 12, color: t.textDim))
          : Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (var i = 0; i < metrics.length; i++)
                  Padding(
                    padding:
                        const EdgeInsets.symmetric(vertical: 3, horizontal: 2),
                    child: Row(
                      children: [
                        Expanded(
                          child: Text(metrics[i].label,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style:
                                  TextStyle(fontSize: 12.5, color: t.textDim)),
                        ),
                        const SizedBox(width: 10),
                        Text(metrics[i].value,
                            style: TextStyle(
                                fontSize: 12,
                                fontWeight: FontWeight.w600,
                                color: t.text)),
                        const SizedBox(width: 10),
                        SmallBtn(label: 'Open', onTap: () => onMetric(i)),
                      ],
                    ),
                  ),
              ],
            ),
    );
  }
}
