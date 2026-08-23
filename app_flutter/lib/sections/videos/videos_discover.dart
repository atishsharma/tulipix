// Discover: four TMDB rails — trending films, trending shows, what is coming
// and what is on the air.
//
// The cards do not open anything. They do not in the Slint build either
// (`card-clicked(id) => { }` on all four rails); Discover is a browse surface
// over the scraper's feed, and the detail view it would need belongs to a
// catalogue this tab does not have.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/videos.dart';
import 'videos_controller.dart';
import 'videos_library.dart' show RailTitle;
import 'videos_widgets.dart';

class VideosDiscover extends StatelessWidget {
  const VideosDiscover({
    super.key,
    required this.controller,
    required this.state,
  });

  final VideosController controller;
  final VideosState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final empty = state.discoverTrendingMovies.isEmpty &&
        state.discoverTrendingShows.isEmpty &&
        state.discoverUpcoming.isEmpty &&
        state.discoverOnAir.isEmpty;

    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(32, 16, 32, 16),
          child: Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: [
              Text(
                'TMDB TRENDING & UPCOMING',
                style: TextStyle(
                  fontSize: 11,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 1.5,
                  color: t.nInk3,
                ),
              ),
              Row(
                children: [
                  if (state.discoverBusy) ...[
                    const SizedBox(
                        width: 14,
                        height: 14,
                        child: CircularProgressIndicator(strokeWidth: 2)),
                    const SizedBox(width: 10),
                  ],
                  VideoTab(
                    hue: cDl,
                    icon: Icons.refresh,
                    label: 'Refresh',
                    onTap: () =>
                        controller.send(const VideosCmd.refreshDiscover()),
                  ),
                ],
              ),
            ],
          ),
        ),
        Expanded(
          child: empty
              ? VideosEmpty(
                  icon: Icons.explore_outlined,
                  title: state.discoverBusy ? 'Fetching…' : 'Nothing to show',
                  message: state.discoverBusy
                      ? ''
                      : 'Discover needs a TMDB API key in Settings, and a '
                          'connection. Press Refresh once it has one.',
                )
              : ListView(
                  padding: const EdgeInsets.only(bottom: 32),
                  children: [
                    _Rail(
                        label: 'Trending Movies',
                        icon: Icons.local_fire_department,
                        items: state.discoverTrendingMovies),
                    const _Hair(),
                    _Rail(
                        label: 'Trending TV Shows',
                        icon: Icons.tv,
                        items: state.discoverTrendingShows),
                    const _Hair(),
                    _Rail(
                        label: 'Upcoming Movies',
                        icon: Icons.movie_outlined,
                        items: state.discoverUpcoming),
                    const _Hair(),
                    _Rail(
                        label: 'On The Air',
                        icon: Icons.rss_feed,
                        items: state.discoverOnAir),
                  ],
                ),
        ),
      ],
    );
  }
}

class _Hair extends StatelessWidget {
  const _Hair();

  @override
  Widget build(BuildContext context) => Divider(
        height: 1,
        thickness: 1,
        color: context.tokens.nHair,
      );
}

class _Rail extends StatelessWidget {
  const _Rail({
    required this.label,
    required this.icon,
    required this.items,
  });

  final String label;
  final IconData icon;
  final List<DiscoverCard> items;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (items.isEmpty) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.fromLTRB(28, 18, 28, 0),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              RailTitle(label),
              const SizedBox(width: 10),
              Icon(icon, size: 16, color: t.nInk2),
              const SizedBox(width: 10),
              Text('${items.length}',
                  style: TextStyle(fontSize: 12, color: t.nInk3)),
            ],
          ),
          const SizedBox(height: 14),
          SizedBox(
            height: 250,
            child: ListView.separated(
              scrollDirection: Axis.horizontal,
              itemCount: items.length,
              separatorBuilder: (_, __) => const SizedBox(width: 14),
              itemBuilder: (_, i) => _Card(card: items[i]),
            ),
          ),
        ],
      ),
    );
  }
}

class _Card extends StatelessWidget {
  const _Card({required this.card});

  final DiscoverCard card;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      width: 130,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Stack(
            children: [
              Artwork(path: card.poster, width: 130, height: 195),
              if (card.vote > 0)
                Positioned(
                  top: 6,
                  right: 6,
                  child: OverlayPill(
                    text: card.vote.toStringAsFixed(1),
                    ink: Tokens.secVideos,
                  ),
                ),
              Positioned(
                top: 6,
                left: 6,
                child: OverlayPill(
                  text: card.isMovie ? 'MOVIE' : 'SHOW',
                  background: card.isMovie ? cBadgeMovie : cBadgeSeries,
                ),
              ),
              if (card.releaseDate.isNotEmpty)
                Positioned(
                  left: 0,
                  right: 0,
                  bottom: 0,
                  child: Container(
                    height: 22,
                    alignment: Alignment.center,
                    color: const Color(0xCC000000),
                    child: Text(
                      card.releaseDate,
                      style: const TextStyle(
                          fontSize: 9,
                          fontWeight: FontWeight.w600,
                          color: Colors.white),
                    ),
                  ),
                ),
            ],
          ),
          const SizedBox(height: 6),
          Text(
            card.title,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
                fontSize: 10, fontWeight: FontWeight.w600, color: t.nInk2),
          ),
          if (card.year > 0)
            Text('${card.year}', style: TextStyle(fontSize: 9, color: t.nInk3)),
        ],
      ),
    );
  }
}
