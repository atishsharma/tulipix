// Classic — the default Home.
//
// A greeting with the library totals, the day's quote, the CONTINUE strip, the
// eight section tiles, and the recently-added shelves. The quiet one: it says
// what is in the app and takes you there, and it leads with nothing.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/home.dart';
import 'home_controller.dart';
import 'home_widgets.dart';

class ClassicHome extends StatelessWidget {
  const ClassicHome({
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
    return ListView(
      padding: const EdgeInsets.all(24),
      children: [
        if (_on('hero')) ...[
          LayoutBuilder(
            builder: (context, box) {
              final greeting = Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(state.greeting,
                      style: TextStyle(
                          fontSize: 30,
                          fontWeight: FontWeight.w800,
                          color: t.text)),
                  const SizedBox(height: 4),
                  Text('${state.dateLine}  ·  ${state.libraryLine}',
                      style: TextStyle(fontSize: 13, color: t.textDim)),
                ],
              );
              if (!_on('player')) return greeting;
              final player = NowPlayingCard(state: state);
              if (box.maxWidth < 900) {
                return Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [greeting, const SizedBox(height: 16), player],
                );
              }
              return Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Expanded(child: greeting),
                  const SizedBox(width: 20),
                  SizedBox(width: 360, child: player),
                ],
              );
            },
          ),
          const SizedBox(height: 16),
          QuoteBlock(quote: state.quote),
          const SizedBox(height: 20),
        ],
        if (_on('quick')) ...[
          const LaunchBar(),
          const SizedBox(height: 20),
        ],
        if (_on('continue')) ...[
          HomeCaption('CONTINUE',
              trailing: ContinueTabs(
                  controller: controller, active: state.continueFilter)),
          if (state.continueRows.isEmpty)
            Container(
              padding: const EdgeInsets.all(20),
              decoration: BoxDecoration(
                color: t.panel,
                borderRadius: BorderRadius.circular(Tokens.radiusMd),
                border: Border.all(color: t.outline),
              ),
              child: Text(
                  'Nothing in progress. Start a film, a book or an episode '
                  'and it turns up here.',
                  style: TextStyle(fontSize: 12.5, color: t.textDim)),
            )
          else
            LayoutBuilder(
              builder: (context, box) {
                final cols = (box.maxWidth / 260).floor().clamp(1, 4);
                final w = (box.maxWidth - (cols - 1) * 12) / cols;
                return Wrap(
                  spacing: 12,
                  runSpacing: 12,
                  children: [
                    for (final r in state.continueRows)
                      SizedBox(
                        width: w,
                        child: ContinueCard(controller: controller, row: r),
                      ),
                  ],
                );
              },
            ),
          const SizedBox(height: 20),
        ],
        LayoutBuilder(
          builder: (context, box) {
            final tiles =
                hubTiles(state).where((x) => _on(_cardOf(x.section))).toList();
            if (tiles.isEmpty) return const SizedBox.shrink();
            final cols = (box.maxWidth / 210).floor().clamp(2, 4);
            final w = (box.maxWidth - (cols - 1) * 12) / cols;
            return Wrap(
              spacing: 12,
              runSpacing: 12,
              children: [
                for (final tile in tiles)
                  SizedBox(
                    width: w,
                    height: 96,
                    child: HubTile(
                      section: tile.section,
                      value: tile.value,
                      sub: tile.sub,
                    ),
                  ),
              ],
            );
          },
        ),
        const SizedBox(height: 20),
        if (state.recentPhotos.isNotEmpty && _on('photos'))
          Shelf(
              title: 'RECENT PHOTOS',
              section: Section.photos,
              tiles: state.recentPhotos),
        if (state.recentVideos.isNotEmpty && _on('videos'))
          Shelf(
              title: 'RECENT VIDEOS',
              section: Section.videos,
              tiles: state.recentVideos),
        if (state.recentSongs.isNotEmpty && _on('music'))
          Shelf(
              title: 'RECENT MUSIC',
              section: Section.music,
              tiles: state.recentSongs),
        if (state.recentBooks.isNotEmpty && _on('books'))
          Shelf(
              title: 'RECENT BOOKS',
              section: Section.books,
              tiles: state.recentBooks),
        if (state.remotes.isNotEmpty && _on('cloud')) ...[
          const HomeCaption('CLOUD REMOTES'),
          for (final r in state.remotes)
            ListTile(
              dense: true,
              leading: const Icon(Icons.cloud_outlined,
                  size: 18, color: Tokens.secCloud),
              title:
                  Text(r.name, style: TextStyle(fontSize: 12.5, color: t.text)),
              subtitle: Text(r.backend,
                  style: TextStyle(fontSize: 11, color: t.textDim)),
              onTap: () => ShellController.instance.go(Section.cloud),
            ),
        ],
      ],
    );
  }
}

/// A hub tile's card key — the switch in Settings that hides it.
String _cardOf(Section s) => switch (s) {
      Section.photos => 'photos',
      Section.videos => 'videos',
      Section.music => 'music',
      Section.books => 'books',
      Section.cloud => 'cloud',
      Section.tools => 'tools',
      Section.transfer => 'transfer',
      _ => 'finances',
    };
