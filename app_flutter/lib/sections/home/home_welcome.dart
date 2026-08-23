// Welcome — the friendly one.
//
// A big greeting with a rotating emoji, the launch bar, Continue and Recently
// Added side by side, then My Hub as eight tiles across the bottom. Where
// Classic leads with facts, this leads with a hello.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../shell/sidebar.dart';
import '../../src/rust/api/home.dart';
import 'home_controller.dart';
import 'home_widgets.dart';

/// The greeting emoji. The list lives here rather than in Rust because the
/// rotation is a UI clock: one every two minutes.
const List<String> kGreetEmojis = [
  '👋',
  '😊',
  '🌞',
  '✨',
  '🎉',
  '🌻',
  '😄',
  '🌈',
  '🍀',
  '💫',
];

const Duration kEmojiRotate = Duration(minutes: 2);

class WelcomeHome extends StatefulWidget {
  const WelcomeHome({
    super.key,
    required this.controller,
    required this.state,
  });

  final HomeController controller;
  final HomeState state;

  @override
  State<WelcomeHome> createState() => _WelcomeHomeState();
}

class _WelcomeHomeState extends State<WelcomeHome> {
  int _emoji = 0;
  Timer? _tick;

  @override
  void initState() {
    super.initState();
    _tick = Timer.periodic(kEmojiRotate, (_) {
      if (mounted) {
        setState(() => _emoji = (_emoji + 1) % kGreetEmojis.length);
      }
    });
  }

  @override
  void dispose() {
    _tick?.cancel();
    super.dispose();
  }

  bool _on(String card) => widget.state.cards.contains(card);

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.state;
    return ListView(
      padding: const EdgeInsets.all(26),
      children: [
        if (_on('hero')) ...[
          _Hero(state: st, emoji: kGreetEmojis[_emoji]),
          const SizedBox(height: 18),
        ],
        if (_on('quick')) ...[
          const _Card(child: LaunchBar()),
          const SizedBox(height: 14),
        ],
        LayoutBuilder(
          builder: (context, box) {
            final cont = _on('continue')
                ? _Card(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Row(
                          children: [
                            Text('Continue',
                                style: TextStyle(
                                    fontSize: 15,
                                    fontWeight: FontWeight.w800,
                                    color: t.text)),
                            const SizedBox(width: 12),
                            Expanded(
                              child: ContinueTabs(
                                controller: widget.controller,
                                active: st.continueFilter,
                              ),
                            ),
                          ],
                        ),
                        const SizedBox(height: 12),
                        if (st.continueRows.isEmpty)
                          Text(
                              'Nothing in progress yet. Whatever you start '
                              'turns up here.',
                              style:
                                  TextStyle(fontSize: 12.5, color: t.textDim))
                        else
                          for (final r in st.continueRows)
                            Padding(
                              padding: const EdgeInsets.only(bottom: 8),
                              child: ContinueCard(
                                controller: widget.controller,
                                row: r,
                                dense: true,
                              ),
                            ),
                      ],
                    ),
                  )
                : null;
            final recent = _on('library') ? _RecentlyAdded(state: st) : null;
            final both = [cont, recent].whereType<Widget>().toList();
            if (both.isEmpty) return const SizedBox.shrink();
            if (box.maxWidth < 900 || both.length == 1) {
              return Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  for (final w in both)
                    Padding(
                      padding: const EdgeInsets.only(bottom: 14),
                      child: w,
                    ),
                ],
              );
            }
            return Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(child: both[0]),
                const SizedBox(width: 14),
                Expanded(child: both[1]),
              ],
            );
          },
        ),
        const SizedBox(height: 14),
        _Hub(state: st),
        if (_on('player')) ...[
          const SizedBox(height: 14),
          NowPlayingCard(state: st),
        ],
      ],
    );
  }
}

/// A soft panel. Welcome's whole page is these, which is what makes it read as
/// a set of things offered rather than a dashboard reporting.
class _Card extends StatelessWidget {
  const _Card({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(20),
        border: Border.all(color: t.outline),
      ),
      child: child,
    );
  }
}

class _Hero extends StatelessWidget {
  const _Hero({required this.state, required this.emoji});

  final HomeState state;
  final String emoji;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return _Card(
      child: Row(
        children: [
          Text(emoji, style: const TextStyle(fontSize: 44)),
          const SizedBox(width: 18),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(state.greeting,
                    style: TextStyle(
                        fontSize: 28,
                        fontWeight: FontWeight.w800,
                        color: t.text)),
                const SizedBox(height: 3),
                Text('Your hub for everything that matters.',
                    style: TextStyle(fontSize: 13, color: t.textDim)),
                const SizedBox(height: 10),
                Text('${state.dateLine}  ·  ${state.libraryLine}',
                    style: TextStyle(fontSize: 12, color: t.textDim)),
              ],
            ),
          ),
          const SizedBox(width: 18),
          SizedBox(width: 300, child: QuoteBlock(quote: state.quote)),
        ],
      ),
    );
  }
}

/// Recently Added, one medium at a time. Tabs rather than four stacked shelves:
/// the card sits beside Continue and has one shelf's worth of height.
class _RecentlyAdded extends StatefulWidget {
  const _RecentlyAdded({required this.state});

  final HomeState state;

  @override
  State<_RecentlyAdded> createState() => _RecentlyAddedState();
}

class _RecentlyAddedState extends State<_RecentlyAdded> {
  Section _tab = Section.photos;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.state;
    final tabs = <({Section section, List<HomeTile> tiles})>[
      (section: Section.photos, tiles: st.recentPhotos),
      (section: Section.videos, tiles: st.recentVideos),
      (section: Section.music, tiles: st.recentSongs),
      (section: Section.books, tiles: st.recentBooks),
    ].where((x) => x.tiles.isNotEmpty).toList();
    if (tabs.isEmpty) {
      return _Card(
        child: Text(
            'Nothing indexed yet. Add a folder in Settings › Libraries.',
            style: TextStyle(fontSize: 12.5, color: t.textDim)),
      );
    }
    final active =
        tabs.firstWhere((x) => x.section == _tab, orElse: () => tabs.first);
    return _Card(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          Row(
            children: [
              Text('Recently Added',
                  style: TextStyle(
                      fontSize: 15,
                      fontWeight: FontWeight.w800,
                      color: t.text)),
              const SizedBox(width: 12),
              Expanded(
                child: Wrap(
                  spacing: 7,
                  children: [
                    for (final x in tabs)
                      HomeChip(
                        label: kSectionMeta[x.section]!.label,
                        on: active.section == x.section,
                        accent: accentFor(x.section),
                        onTap: () => setState(() => _tab = x.section),
                      ),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 12),
          Shelf(
            title: '',
            section: active.section,
            tiles: active.tiles,
          ),
        ],
      ),
    );
  }
}

class _Hub extends StatelessWidget {
  const _Hub({required this.state});

  final HomeState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tiles = hubTiles(state);
    return _Card(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text('My Hub',
              style: TextStyle(
                  fontSize: 15, fontWeight: FontWeight.w800, color: t.text)),
          const SizedBox(height: 12),
          LayoutBuilder(
            builder: (context, box) {
              // Eight across on a wide window, folding to four and then two.
              final cols = (box.maxWidth / 150).floor().clamp(2, 8);
              final w = (box.maxWidth - (cols - 1) * 10) / cols;
              return Wrap(
                spacing: 10,
                runSpacing: 10,
                children: [
                  for (final tile in tiles)
                    SizedBox(
                      width: w,
                      height: 132,
                      child: HubTile(
                        section: tile.section,
                        value: tile.value,
                        sub: tile.sub,
                        tall: true,
                      ),
                    ),
                ],
              );
            },
          ),
        ],
      ),
    );
  }
}
