// Cinema — the one that leads with what you were doing.
//
// A full-bleed backdrop of the newest in-progress item with a Resume button on
// it, the launcher pills across the top, the player standing up in a glass
// column on the right, and the Continue strip as a shelf along the bottom. The
// shelf is the hero's tab strip: picking a tile swaps the backdrop, it does not
// open anything. Opening is what the hero's own button is for.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/home.dart';
import 'home_controller.dart';
import 'home_widgets.dart';

/// How long one hub card holds the gap under the player before the next takes
/// its turn. The right column is 300px wide, so they cannot all fit at once.
const Duration kHubRotate = Duration(seconds: 10);

class CinemaHome extends StatefulWidget {
  const CinemaHome({
    super.key,
    required this.controller,
    required this.state,
  });

  final HomeController controller;
  final HomeState state;

  @override
  State<CinemaHome> createState() => _CinemaHomeState();
}

class _CinemaHomeState extends State<CinemaHome> {
  /// Which Continue tile the backdrop is showing.
  int _sel = 0;
  int _hub = 0;
  Timer? _tick;

  @override
  void initState() {
    super.initState();
    _tick = Timer.periodic(kHubRotate, (_) {
      if (mounted) setState(() => _hub = (_hub + 1) % 8);
    });
  }

  @override
  void dispose() {
    _tick?.cancel();
    super.dispose();
  }

  bool _on(String card) => widget.state.cards.contains(card);

  /// The backdrop's subject: the picked Continue row, or the hero the snapshot
  /// resolved when there is nothing in progress.
  HomeContinue? get _selRow {
    final rows = widget.state.continueRows;
    if (rows.isEmpty) return null;
    return rows[_sel.clamp(0, rows.length - 1)];
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.state;
    final row = _selRow;
    final kind = row?.kind ?? st.hero.kind;
    final tint = kind.isEmpty ? Tokens.brand : kindColor(kind);
    return Stack(
      fit: StackFit.expand,
      children: [
        // The backdrop: the subject's own art, washed down far enough that
        // white text sits on it.
        if (row != null)
          Opacity(
            opacity: 0.35,
            child: LazyCover(
              section: kindSection(row.kind),
              id: row.id,
              tint: tint,
            ),
          ),
        DecoratedBox(
          decoration: BoxDecoration(
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [
                t.bg.withValues(alpha: 0.92),
                t.bg.withValues(alpha: 0.75),
                tint.withValues(alpha: 0.12),
              ],
            ),
          ),
        ),
        Padding(
          padding: const EdgeInsets.all(28),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              _TopBar(state: st, quick: _on('quick')),
              const SizedBox(height: 20),
              Expanded(
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Expanded(
                      child: _on('hero')
                          ? _Hero(state: st, row: row, tint: tint)
                          : const SizedBox.shrink(),
                    ),
                    if (_on('player') || _on('hub')) ...[
                      const SizedBox(width: 24),
                      SizedBox(
                        width: 300,
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.stretch,
                          children: [
                            if (_on('player'))
                              NowPlayingCard(state: st, tall: true),
                            if (_on('hub')) ...[
                              const SizedBox(height: 16),
                              _HubCarousel(
                                state: st,
                                index: _hub,
                                onStep: (d) =>
                                    setState(() => _hub = (_hub + d + 8) % 8),
                              ),
                            ],
                          ],
                        ),
                      ),
                    ],
                  ],
                ),
              ),
              if (_on('continue') && st.continueRows.isNotEmpty) ...[
                const SizedBox(height: 20),
                _Shelf(
                  controller: widget.controller,
                  state: st,
                  selected: _sel,
                  onSelect: (i) => setState(() => _sel = i),
                ),
              ],
            ],
          ),
        ),
      ],
    );
  }
}

class _TopBar extends StatelessWidget {
  const _TopBar({required this.state, required this.quick});

  final HomeState state;
  final bool quick;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      children: [
        Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(state.greeting,
                style: TextStyle(
                    fontSize: 20, fontWeight: FontWeight.w800, color: t.text)),
            Text(state.dateLine,
                style: TextStyle(fontSize: 11.5, color: t.textDim)),
          ],
        ),
        const Spacer(),
        if (quick) const Flexible(child: LaunchBar(solid: true)),
      ],
    );
  }
}

class _Hero extends StatelessWidget {
  const _Hero({required this.state, required this.row, required this.tint});

  final HomeState state;
  final HomeContinue? row;
  final Color tint;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final h = state.hero;
    final title = row?.title ?? h.title;
    if (title.isEmpty) {
      return Column(
        mainAxisAlignment: MainAxisAlignment.center,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('Nothing in progress',
              style: TextStyle(
                  fontSize: 40, fontWeight: FontWeight.w800, color: t.text)),
          const SizedBox(height: 8),
          Text('Start a film, a book or an episode and it lands here.',
              style: TextStyle(fontSize: 14, color: t.textDim)),
        ],
      );
    }
    final kicker = row == null
        ? h.kicker
        : switch (row!.kind) {
            'video' => 'CONTINUE WATCHING',
            'book' => 'CONTINUE READING',
            _ => 'CONTINUE LISTENING',
          };
    final meta = row == null
        ? h.meta
        : row!.author.isEmpty
            ? row!.sub
            : '${row!.author} · ${row!.sub}';
    final frac = row?.frac ?? h.frac;
    return Column(
      mainAxisAlignment: MainAxisAlignment.center,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(kicker,
            style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w800,
                letterSpacing: 1.4,
                color: tint)),
        const SizedBox(height: 8),
        Text(title,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
                fontSize: 46, fontWeight: FontWeight.w800, color: t.text)),
        const SizedBox(height: 8),
        Text(meta, style: TextStyle(fontSize: 14, color: t.textDim)),
        if (frac >= 0) ...[
          const SizedBox(height: 14),
          SizedBox(
            width: 360,
            child: ClipRRect(
              borderRadius: BorderRadius.circular(3),
              child: LinearProgressIndicator(
                value: frac,
                minHeight: 5,
                backgroundColor: t.outline,
                color: tint,
              ),
            ),
          ),
        ],
        const SizedBox(height: 20),
        Row(
          children: [
            FilledButton.icon(
              onPressed: () =>
                  ShellController.instance.go(kindSection(row?.kind ?? h.kind)),
              icon: const Icon(Icons.play_arrow, size: 18),
              label: const Text('Resume'),
              style: FilledButton.styleFrom(backgroundColor: tint),
            ),
            const SizedBox(width: 10),
            _Money(state: state),
          ],
        ),
      ],
    );
  }
}

/// The money panel. Three obligations, overdue first — a fourth row would push
/// the shelf off the page.
class _Money extends StatelessWidget {
  const _Money({required this.state});

  final HomeState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (state.finDues.isEmpty) return const SizedBox.shrink();
    return Container(
      width: 274,
      padding: const EdgeInsets.all(13),
      decoration: BoxDecoration(
        color: t.glass,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: t.glassBorder),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          Row(
            children: [
              Text('NEEDS YOU',
                  style: TextStyle(
                      fontSize: 9.5,
                      fontWeight: FontWeight.w800,
                      letterSpacing: 1.1,
                      color: t.textDim)),
              const Spacer(),
              Text('${state.finMonth} · ${state.finSpent}',
                  style: TextStyle(fontSize: 10.5, color: t.textDim)),
            ],
          ),
          const SizedBox(height: 8),
          for (final d in state.finDues)
            Padding(
              padding: const EdgeInsets.only(bottom: 5),
              child: Row(
                children: [
                  Expanded(
                    child: Text(d.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11.5, color: t.text)),
                  ),
                  Text(d.due,
                      style: TextStyle(
                          fontSize: 10.5,
                          color: d.late_ ? Tokens.error : t.textDim)),
                  const SizedBox(width: 8),
                  Text(d.amount,
                      style: TextStyle(
                          fontSize: 11.5,
                          fontWeight: FontWeight.w700,
                          color: d.late_ ? Tokens.error : t.text)),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// My Hub, one card at a time: Classic shows all eight side by side, but this
/// column is 300px wide, so they take turns.
class _HubCarousel extends StatelessWidget {
  const _HubCarousel({
    required this.state,
    required this.index,
    required this.onStep,
  });

  final HomeState state;
  final int index;
  final ValueChanged<int> onStep;

  @override
  Widget build(BuildContext context) {
    final tiles = hubTiles(state);
    final tile = tiles[index % tiles.length];
    return Row(
      children: [
        _Arrow(icon: Icons.chevron_left, onTap: () => onStep(-1)),
        const SizedBox(width: 8),
        Expanded(
          child: SizedBox(
            height: 132,
            child: HubTile(
              section: tile.section,
              value: tile.value,
              sub: tile.sub,
              tall: true,
            ),
          ),
        ),
        const SizedBox(width: 8),
        _Arrow(icon: Icons.chevron_right, onTap: () => onStep(1)),
      ],
    );
  }
}

class _Arrow extends StatelessWidget {
  const _Arrow({required this.icon, required this.onTap});

  final IconData icon;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: t.glass,
      shape: const CircleBorder(),
      child: InkWell(
        customBorder: const CircleBorder(),
        onTap: onTap,
        child: SizedBox(
          width: 28,
          height: 28,
          child: Icon(icon, size: 16, color: t.textDim),
        ),
      ),
    );
  }
}

/// The Continue shelf. Tabs divide the row evenly, so nothing has to scroll
/// however many things are in progress — capped, because one tab half a page
/// wide is a banner, not a tab.
class _Shelf extends StatelessWidget {
  const _Shelf({
    required this.controller,
    required this.state,
    required this.selected,
    required this.onSelect,
  });

  final HomeController controller;
  final HomeState state;
  final int selected;
  final ValueChanged<int> onSelect;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        HomeCaption('CONTINUE',
            trailing: ContinueTabs(
                controller: controller, active: state.continueFilter)),
        SizedBox(
          height: 96,
          child: LayoutBuilder(
            builder: (context, box) {
              final n = state.continueRows.length;
              final w = ((box.maxWidth - (n - 1) * 8) / n).clamp(0.0, 320.0);
              return Row(
                children: [
                  for (var i = 0; i < n; i++) ...[
                    if (i > 0) const SizedBox(width: 8),
                    SizedBox(
                      width: w,
                      child: _Tile(
                        row: state.continueRows[i],
                        active: i == selected,
                        onTap: () => onSelect(i),
                      ),
                    ),
                  ],
                ],
              );
            },
          ),
        ),
      ],
    );
  }
}

class _Tile extends StatelessWidget {
  const _Tile({
    required this.row,
    required this.active,
    required this.onTap,
  });

  final HomeContinue row;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = kindColor(row.kind);
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 130),
          padding: const EdgeInsets.all(9),
          decoration: BoxDecoration(
            color: active ? tint.withValues(alpha: 0.18) : t.glass,
            borderRadius: BorderRadius.circular(12),
            border: Border.all(
                color:
                    active ? tint.withValues(alpha: 0.55) : Colors.transparent),
          ),
          child: Row(
            children: [
              ClipRRect(
                borderRadius: BorderRadius.circular(6),
                child: SizedBox(
                  width: 40,
                  height: 54,
                  child: LazyCover(
                    section: kindSection(row.kind),
                    id: row.id,
                    tint: tint,
                    icon: kindIcon(row.kind),
                  ),
                ),
              ),
              const SizedBox(width: 9),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(row.title,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 11.5,
                            fontWeight: FontWeight.w700,
                            color: t.text)),
                    const SizedBox(height: 2),
                    Text(row.sub,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 10.5, color: t.textDim)),
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
