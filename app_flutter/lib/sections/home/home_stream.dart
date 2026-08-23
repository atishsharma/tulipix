// Stream — the timeline.
//
// One feed of everything that happened, newest first, grouped by NOW / TODAY /
// YESTERDAY / EARLIER, with a filter key at the bottom. Runs of the same kind
// inside a quarter of an hour fold into one row, so adding four hundred photos
// is one line rather than four hundred.
//
// An empty today is not an empty page: the feed reaches further back and says
// so.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../shell/sidebar.dart';
import '../../src/rust/api/home.dart';
import 'home_controller.dart';
import 'home_widgets.dart';

/// The feed key, in the order the Slint page lists it.
const List<({String id, String label, IconData icon})> kFeedFilters = [
  (id: 'all', label: 'Everything', icon: Icons.all_inclusive),
  (id: 'media', label: 'Media', icon: Icons.perm_media_outlined),
  (id: 'money', label: 'Money', icon: Icons.account_balance_wallet_outlined),
  (id: 'devices', label: 'Devices', icon: Icons.devices_outlined),
];

class StreamHome extends StatelessWidget {
  const StreamHome({
    super.key,
    required this.controller,
    required this.state,
  });

  final HomeController controller;
  final HomeState state;

  bool _on(String card) => state.cards.contains(card);

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      children: [
        Expanded(
          child: LayoutBuilder(
            builder: (context, box) {
              final feed = _Feed(state: state);
              final side = _Side(state: state, showPlayer: _on('player'));
              if (box.maxWidth < 940) {
                return ListView(
                  padding: const EdgeInsets.fromLTRB(24, 22, 24, 12),
                  children: [
                    _Head(state: state),
                    const SizedBox(height: 16),
                    side,
                    const SizedBox(height: 16),
                    feed,
                  ],
                );
              }
              return Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Expanded(
                    child: ListView(
                      padding: const EdgeInsets.fromLTRB(24, 22, 12, 12),
                      children: [
                        _Head(state: state),
                        const SizedBox(height: 16),
                        feed,
                      ],
                    ),
                  ),
                  SizedBox(
                    width: 320,
                    child: ListView(
                      padding: const EdgeInsets.fromLTRB(12, 22, 24, 12),
                      children: [side],
                    ),
                  ),
                ],
              );
            },
          ),
        ),
        // The key floats at the foot, Material's bottom bar: icon and label
        // inside one pill, so a key reads as a single thing.
        Container(
          padding: const EdgeInsets.fromLTRB(24, 8, 24, 14),
          color: t.bg,
          child: Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              for (final f in kFeedFilters)
                Padding(
                  padding: const EdgeInsets.only(right: 8),
                  child: HomeChip(
                    label: f.label,
                    icon: f.icon,
                    on: state.feedFilter == f.id,
                    onTap: () =>
                        controller.send(HomeCmd.setFeedFilter(filter: f.id)),
                  ),
                ),
            ],
          ),
        ),
      ],
    );
  }
}

class _Head extends StatelessWidget {
  const _Head({required this.state});

  final HomeState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(state.greeting,
            style: TextStyle(
                fontSize: 24, fontWeight: FontWeight.w800, color: t.text)),
        const SizedBox(height: 3),
        Text('${state.dateLine}  ·  ${state.libraryLine}',
            style: TextStyle(fontSize: 12, color: t.textDim)),
        if (state.feedNote.isNotEmpty) ...[
          const SizedBox(height: 10),
          Text(state.feedNote,
              style: TextStyle(
                  fontSize: 12, fontStyle: FontStyle.italic, color: t.textDim)),
        ],
      ],
    );
  }
}

class _Feed extends StatelessWidget {
  const _Feed({required this.state});

  final HomeState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (state.events.isEmpty) {
      return Container(
        padding: const EdgeInsets.all(24),
        decoration: BoxDecoration(
          color: t.panel,
          borderRadius: BorderRadius.circular(Tokens.radiusMd),
          border: Border.all(color: t.outline),
        ),
        child: Text(
            'Nothing has happened yet. Add a folder, play something, or send a '
            'file — it all turns up here.',
            style: TextStyle(fontSize: 12.5, color: t.textDim)),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        for (final e in state.events) ...[
          if (e.head) ...[
            const SizedBox(height: 14),
            HomeCaption(e.group),
          ],
          _Row(event: e),
        ],
      ],
    );
  }
}

class _Row extends StatelessWidget {
  const _Row({required this.event});

  final HomeEvent event;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final section = sectionOf(event.section);
    final accent = event.alarm ? Tokens.error : accentFor(section);
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: () => ShellController.instance.go(section),
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: 6),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              // The gutter prints the clock and the day: the clock alone never
              // said which day a row belonged to once the feed reached past
              // yesterday.
              SizedBox(
                width: 62,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.end,
                  children: [
                    Text(event.at,
                        style: TextStyle(
                            fontSize: 11.5,
                            fontWeight: FontWeight.w700,
                            color: t.text)),
                    Text(event.day,
                        style: TextStyle(fontSize: 9.5, color: t.textDim)),
                  ],
                ),
              ),
              const SizedBox(width: 12),
              Container(
                width: 30,
                height: 30,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: accent.withValues(alpha: 0.16),
                  shape: BoxShape.circle,
                ),
                child: Icon(_glyph(event.kind), size: 15, color: accent),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(event.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 12.5,
                            fontWeight: FontWeight.w600,
                            color: event.alarm ? Tokens.error : t.text)),
                    Text(event.sub,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11, color: t.textDim)),
                  ],
                ),
              ),
              // A pill only where there is a concrete thing to resume — the row
              // itself already opens the section, and two controls doing one
              // job is noise.
              if (event.action.isNotEmpty) ...[
                const SizedBox(width: 10),
                OutlinedButton(
                  onPressed: () => ShellController.instance.go(section),
                  style: OutlinedButton.styleFrom(
                    visualDensity: VisualDensity.compact,
                    side: BorderSide(color: accent.withValues(alpha: 0.5)),
                  ),
                  child: Text(event.action,
                      style: TextStyle(fontSize: 11, color: accent)),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }

  static IconData _glyph(String kind) => switch (kind) {
        'added' => Icons.add,
        'played' => Icons.play_arrow,
        'resumed' => Icons.history,
        'episode' => Icons.podcasts,
        'job' => Icons.build_outlined,
        'sent' => Icons.upload_outlined,
        'received' => Icons.download_outlined,
        'paid' => Icons.account_balance_wallet_outlined,
        _ => Icons.circle_outlined,
      };
}

/// The feed names sections as strings; the shell names them as an enum.
Section sectionOf(String key) => switch (key) {
      'photos' => Section.photos,
      'videos' => Section.videos,
      'music' => Section.music,
      'books' => Section.books,
      'cloud' => Section.cloud,
      'tools' => Section.tools,
      'transfer' => Section.transfer,
      'finances' => Section.finances,
      _ => Section.home,
    };

/// The right column: the player, the money chart and the hub tiles — the
/// standing facts, beside a feed that is all movement.
class _Side extends StatelessWidget {
  const _Side({required this.state, required this.showPlayer});

  final HomeState state;
  final bool showPlayer;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (showPlayer) ...[
          NowPlayingCard(state: state),
          const SizedBox(height: 14),
        ],
        _Spend(state: state),
        const SizedBox(height: 14),
        for (final tile in hubTiles(state).take(4))
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: SizedBox(
              height: 84,
              child: HubTile(
                section: tile.section,
                value: tile.value,
                sub: tile.sub,
              ),
            ),
          ),
      ],
    );
  }
}

/// Twelve months of spend as bar heights. The tallest bar is the year's peak,
/// so the shape says how this month compares rather than what it cost.
class _Spend extends StatelessWidget {
  const _Spend({required this.state});

  final HomeState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (state.finMonths.isEmpty) return const SizedBox.shrink();
    return Container(
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        border: Border.all(color: t.outline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          Row(
            children: [
              Text('SPENDING',
                  style: TextStyle(
                      fontSize: 9.5,
                      fontWeight: FontWeight.w800,
                      letterSpacing: 1.1,
                      color: t.textDim)),
              const Spacer(),
              Text('${state.finMonth} · ${state.finSpent}',
                  style: TextStyle(
                      fontSize: 11,
                      fontWeight: FontWeight.w700,
                      color: t.text)),
            ],
          ),
          const SizedBox(height: 10),
          SizedBox(
            height: 46,
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: [
                for (var i = 0; i < state.finMonths.length; i++) ...[
                  if (i > 0) const SizedBox(width: 3),
                  Expanded(
                    child: Tooltip(
                      message: i < state.finMonthLabels.length
                          ? state.finMonthLabels[i]
                          : '',
                      child: FractionallySizedBox(
                        heightFactor:
                            state.finMonths[i].clamp(0.04, 1.0).toDouble(),
                        alignment: Alignment.bottomCenter,
                        child: DecoratedBox(
                          decoration: BoxDecoration(
                            color: i == state.finMonths.length - 1
                                ? Tokens.secFinances
                                : Tokens.secFinances.withValues(alpha: 0.35),
                            borderRadius: BorderRadius.circular(2),
                          ),
                        ),
                      ),
                    ),
                  ),
                ],
              ],
            ),
          ),
          if (state.finDues.isNotEmpty) ...[
            const SizedBox(height: 12),
            for (final d in state.finDues)
              Padding(
                padding: const EdgeInsets.only(bottom: 4),
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
        ],
      ),
    );
  }
}
