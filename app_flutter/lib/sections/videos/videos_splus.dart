// Stream Plus: the anime lane and the second provider stack. Six surfaces —
// home shelves, search, detail, library & history, downloads and settings —
// switched by the row of destinations the shell draws above this.
//
// Its own poster cache is the Stream tab's, on purpose; so is its mpv. What is
// not shared is the catalogue, the tables or any of these preferences.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/videos.dart';
import 'videos_controller.dart';
import 'videos_widgets.dart';

const double _pad = 24;

class VideosSplus extends StatefulWidget {
  const VideosSplus({
    super.key,
    required this.controller,
    required this.splus,
  });

  final VideosController controller;
  final SplusView splus;

  @override
  State<VideosSplus> createState() => _VideosSplusState();
}

class _VideosSplusState extends State<VideosSplus> {
  final TextEditingController _query = TextEditingController();

  @override
  void dispose() {
    _query.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = widget.splus;
    return Column(
      children: [
        _Destinations(controller: widget.controller, splus: s),
        if (s.status.isNotEmpty)
          Container(
            height: 30,
            width: double.infinity,
            alignment: Alignment.centerLeft,
            padding: const EdgeInsets.symmetric(horizontal: _pad),
            color: t.panel2,
            child: Text(s.status,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12, color: t.nInk3)),
          ),
        if (s.busy) const LinearProgressIndicator(minHeight: 2, value: null),
        Expanded(
          child: switch (s.view) {
            'search' =>
              _Search(controller: widget.controller, splus: s, query: _query),
            'detail' => _Detail(controller: widget.controller, splus: s),
            'library' => _Library(controller: widget.controller, splus: s),
            'downloads' => _Downloads(controller: widget.controller, splus: s),
            'settings' => _Settings(controller: widget.controller, splus: s),
            _ => _Home(controller: widget.controller, splus: s),
          },
        ),
      ],
    );
  }
}

class _Destinations extends StatelessWidget {
  const _Destinations({required this.controller, required this.splus});

  final VideosController controller;
  final SplusView splus;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final view = splus.view;
    return Container(
      height: 52,
      color: t.panel2,
      padding: const EdgeInsets.symmetric(horizontal: _pad),
      child: Row(
        children: [
          VideoTab(
            icon: Icons.home_outlined,
            label: 'Home',
            active: view == 'home',
            onTap: () {
              controller.send(const VideosCmd.splusSetView(view: 'home'));
              controller.send(const VideosCmd.splusHomeLoad());
            },
          ),
          const SizedBox(width: 8),
          VideoTab(
            icon: Icons.search,
            label: 'Search',
            active: view == 'search' || view == 'detail',
            onTap: () =>
                controller.send(const VideosCmd.splusSetView(view: 'search')),
          ),
          const SizedBox(width: 8),
          VideoTab(
            hue: cSave,
            icon: Icons.bookmark_outline,
            label: 'Library & History',
            active: view == 'library',
            onTap: () {
              controller.send(const VideosCmd.splusSetView(view: 'library'));
              controller.send(const VideosCmd.splusLibraryLoad());
            },
          ),
          const SizedBox(width: 8),
          VideoTab(
            hue: cDl,
            icon: Icons.download,
            label: splus.dlActive > 0
                ? 'Downloads (${splus.dlActive})'
                : 'Downloads',
            active: view == 'downloads',
            onTap: () {
              controller.send(const VideosCmd.splusSetView(view: 'downloads'));
              controller.send(const VideosCmd.splusDownloadsLoad());
            },
          ),
          const SizedBox(width: 8),
          VideoTab(
            hue: cInfo,
            icon: Icons.settings,
            label: 'Settings',
            active: view == 'settings',
            onTap: () {
              controller.send(const VideosCmd.splusSetView(view: 'settings'));
              controller.send(const VideosCmd.splusSettingsLoad());
            },
          ),
          const Spacer(),
          // Live source health, so a dead provider is visible without opening
          // Settings.
          for (final h in splus.health.where((h) => h.enabled).take(4))
            Padding(
              padding: const EdgeInsets.only(left: 6),
              child: SortChip(
                label: h.label,
                active: true,
                hue: h.ok ? cPlay : (h.sunk ? cSave : cBack),
                onTap: () {
                  controller
                      .send(const VideosCmd.splusSetView(view: 'settings'));
                  controller.send(const VideosCmd.splusSettingsLoad());
                },
              ),
            ),
        ],
      ),
    );
  }
}

/// The shelf heading: a label, an optional note on the right.
class _ShelfHead extends StatelessWidget {
  const _ShelfHead(this.label, {this.note = ''});

  final String label;
  final String note;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: Row(
        children: [
          Container(
            width: 4,
            height: 18,
            decoration: BoxDecoration(
                color: Tokens.secVideos,
                borderRadius: BorderRadius.circular(2)),
          ),
          const SizedBox(width: 8),
          Text(label,
              style: TextStyle(
                  fontSize: 16, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(width: 10),
          Expanded(
            child: Text(note,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 11.5, color: t.nInk3)),
          ),
        ],
      ),
    );
  }
}

class _Poster extends StatelessWidget {
  const _Poster({required this.card, required this.onTap});

  final SplusCard card;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      width: 150,
      child: InkWell(
        onTap: card.blocked ? null : onTap,
        borderRadius: BorderRadius.circular(8),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            Stack(
              children: [
                Artwork(path: card.blocked ? '' : card.poster),
                if (card.blocked)
                  const Positioned.fill(
                    child: Center(
                        child: Icon(Icons.lock_outline,
                            size: 28, color: Colors.white70)),
                  ),
                if (card.badge.isNotEmpty)
                  Positioned(
                    top: 6,
                    left: 6,
                    child: OverlayPill(
                      text: card.badge,
                      background:
                          card.badge == 'MOVIE' ? cBadgeMovie : cBadgeSeries,
                    ),
                  ),
                if (card.saved)
                  const Positioned(
                    top: 6,
                    right: 6,
                    child: OverlayPill(text: '★', background: cSave),
                  ),
                if (card.progress > 0)
                  Positioned(
                    left: 0,
                    right: 0,
                    bottom: 0,
                    child: ProgressStrip(value: card.progress, hue: cPlay),
                  ),
              ],
            ),
            const SizedBox(height: 8),
            Text(card.title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 12, fontWeight: FontWeight.w600, color: t.nInk)),
            if (card.note.isNotEmpty)
              Text(card.note,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 10, color: t.nInk3)),
          ],
        ),
      ),
    );
  }
}

// ------------------------------------------------------------------ home ----

class _Home extends StatefulWidget {
  const _Home({required this.controller, required this.splus});

  final VideosController controller;
  final SplusView splus;

  @override
  State<_Home> createState() => _HomeState();
}

class _HomeState extends State<_Home> {
  int _featured = 0;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = widget.splus;
    if (s.featured.isEmpty && s.trending.isEmpty && !s.busy) {
      return VideosEmpty(
        icon: Icons.auto_awesome_outlined,
        title: 'Nothing loaded yet',
        message: 'AniList is where the shelves come from. Press Home again '
            'once there is a connection.',
        action: PlexButton(
          label: 'Load shelves',
          filled: true,
          onTap: () => widget.controller.send(const VideosCmd.splusHomeLoad()),
        ),
      );
    }
    final hero = s.featured.isEmpty
        ? null
        : s.featured[_featured.clamp(0, s.featured.length - 1)];

    return ListView(
      padding: const EdgeInsets.fromLTRB(_pad, 20, _pad, 24),
      children: [
        if (hero != null) ...[
          _ShelfHead('Featured',
              note: '${_featured + 1} / ${s.featured.length}'),
          Container(
            height: 246,
            padding: const EdgeInsets.all(24),
            decoration: BoxDecoration(
              color: t.panel,
              borderRadius: BorderRadius.circular(13),
              border: Border.all(color: t.nHair),
            ),
            child: Row(
              children: [
                Artwork(path: hero.poster, width: 132, height: 198, radius: 9),
                const SizedBox(width: 22),
                Expanded(
                  child: Column(
                    mainAxisAlignment: MainAxisAlignment.center,
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(hero.title,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 25,
                              fontWeight: FontWeight.w800,
                              color: t.nInk)),
                      const SizedBox(height: 7),
                      Text(hero.note,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 13, color: t.nInk2)),
                      const SizedBox(height: 16),
                      Row(
                        children: [
                          PlexButton(
                            icon: Icons.play_arrow,
                            label: 'Open',
                            hue: cPlay,
                            filled: true,
                            compact: true,
                            onTap: () => widget.controller
                                .send(VideosCmd.splusOpenCard(key: hero.key)),
                          ),
                          const SizedBox(width: 10),
                          IconBtn(
                              icon: Icons.chevron_left,
                              size: 32,
                              onTap: () => setState(() => _featured =
                                  (_featured - 1 + s.featured.length) %
                                      s.featured.length)),
                          const SizedBox(width: 6),
                          IconBtn(
                              icon: Icons.chevron_right,
                              size: 32,
                              onTap: () => setState(() => _featured =
                                  (_featured + 1) % s.featured.length)),
                        ],
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(height: 22),
        ],
        if (s.continueRows.isNotEmpty) ...[
          _ShelfHead('Continue watching',
              note: '${s.continueRows.length} titles'),
          SizedBox(
            height: 92,
            child: ListView.separated(
              scrollDirection: Axis.horizontal,
              itemCount: s.continueRows.length,
              separatorBuilder: (_, __) => const SizedBox(width: 13),
              itemBuilder: (_, i) => _ContinueCard(
                row: s.continueRows[i],
                onTap: () => widget.controller.send(
                    VideosCmd.splusHistoryPlay(key: s.continueRows[i].key)),
              ),
            ),
          ),
          const SizedBox(height: 22),
        ],
        if (s.trending.isNotEmpty) ...[
          const _ShelfHead('Trending'),
          _Rail(cards: s.trending, controller: widget.controller),
          const SizedBox(height: 22),
        ],
        if (s.seasonal.isNotEmpty) ...[
          _ShelfHead('This season', note: s.seasonLabel),
          _Rail(cards: s.seasonal, controller: widget.controller),
        ],
      ],
    );
  }
}

class _Rail extends StatelessWidget {
  const _Rail({required this.cards, required this.controller});

  final List<SplusCard> cards;
  final VideosController controller;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: 258,
      child: ListView.separated(
        scrollDirection: Axis.horizontal,
        itemCount: cards.length,
        separatorBuilder: (_, __) => const SizedBox(width: 14),
        itemBuilder: (_, i) => _Poster(
          card: cards[i],
          onTap: () =>
              controller.send(VideosCmd.splusOpenCard(key: cards[i].key)),
        ),
      ),
    );
  }
}

class _ContinueCard extends StatelessWidget {
  const _ContinueCard({required this.row, required this.onTap});

  final SplusHistory row;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(11),
      child: Container(
        width: 300,
        padding: const EdgeInsets.all(11),
        decoration: BoxDecoration(
          color: t.panel,
          borderRadius: BorderRadius.circular(11),
          border: Border.all(color: t.nHair),
        ),
        child: Row(
          children: [
            Artwork(path: row.poster, width: 104, height: 66, radius: 7),
            const SizedBox(width: 13),
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(row.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 14,
                          fontWeight: FontWeight.w700,
                          color: t.nInk)),
                  Text(row.state,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 12, color: t.nInk3)),
                  const SizedBox(height: 6),
                  ClipRRect(
                    borderRadius: BorderRadius.circular(2),
                    child: ProgressStrip(value: row.progress, hue: cPlay),
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

// ---------------------------------------------------------------- search ----

class _Search extends StatelessWidget {
  const _Search({
    required this.controller,
    required this.splus,
    required this.query,
  });

  final VideosController controller;
  final SplusView splus;
  final TextEditingController query;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.fromLTRB(_pad, 20, _pad, 24),
      children: [
        Row(
          children: [
            Expanded(
              child: VideoSearchField(
                controller: query,
                width: double.infinity,
                hint: 'Search for a show, a film, or an anime',
                onChanged: (q) =>
                    controller.sendQuiet(VideosCmd.splusSuggest(query: q)),
                onSubmitted: (q) =>
                    controller.send(VideosCmd.splusSearch(query: q)),
              ),
            ),
            const SizedBox(width: 10),
            for (final lane in const [
              (id: 'anime', label: 'Anime'),
              (id: 'movie', label: 'Movies'),
              (id: 'tv', label: 'TV'),
            ])
              Padding(
                padding: const EdgeInsets.only(left: 6),
                child: SortChip(
                  label: lane.label,
                  active: splus.lane == lane.id,
                  onTap: () =>
                      controller.send(VideosCmd.splusSetLane(lane: lane.id)),
                ),
              ),
          ],
        ),
        if (splus.suggestions.isNotEmpty) ...[
          const SizedBox(height: 8),
          Container(
            decoration: BoxDecoration(
              color: t.modal,
              borderRadius: BorderRadius.circular(11),
              border: Border.all(color: t.nHair),
            ),
            child: Column(
              children: [
                for (final sug in splus.suggestions.take(5))
                  ListTile(
                    dense: true,
                    leading: Icon(Icons.search, size: 13, color: t.nInk3),
                    title: Text(sug,
                        style: TextStyle(fontSize: 13.5, color: t.nInk)),
                    onTap: () {
                      query.text = sug;
                      controller.send(VideosCmd.splusSearch(query: sug));
                    },
                  ),
              ],
            ),
          ),
        ],
        if (splus.recent.isNotEmpty) ...[
          const SizedBox(height: 12),
          Row(
            children: [
              Text('RECENT',
                  style: TextStyle(
                      fontSize: 10.5,
                      fontWeight: FontWeight.w800,
                      letterSpacing: 1.4,
                      color: t.nInk3)),
              const SizedBox(width: 8),
              Expanded(
                child: Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: [
                    for (final r in splus.recent)
                      SortChip(
                        label: r,
                        onTap: () {
                          query.text = r;
                          controller.send(VideosCmd.splusSearch(query: r));
                        },
                      ),
                  ],
                ),
              ),
              SortChip(
                label: 'Clear',
                hue: cErr,
                onTap: () =>
                    controller.send(const VideosCmd.splusRecentClear()),
              ),
            ],
          ),
        ],
        const SizedBox(height: 16),
        _ShelfHead(
          'Results',
          note: splus.busy
              ? 'searching…'
              : (splus.results.isNotEmpty
                  ? '${splus.results.length} of ${splus.resultsTotal}'
                  : ''),
        ),
        if (splus.results.isEmpty && !splus.busy)
          Padding(
            padding: const EdgeInsets.only(top: 40),
            child: Text(
              splus.query.isEmpty
                  ? 'Search for a show, a film, or an anime.'
                  : 'Nothing matched that.',
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 13.5, color: t.nInk3),
            ),
          )
        else
          Wrap(
            spacing: 16,
            runSpacing: 16,
            children: [
              for (final c in splus.results)
                _Poster(
                  card: c,
                  onTap: () =>
                      controller.send(VideosCmd.splusOpenCard(key: c.key)),
                ),
            ],
          ),
        if (splus.resultsMore) ...[
          const SizedBox(height: 16),
          Center(
            child: PlexButton(
              label: 'Load more',
              onTap: () => controller.send(const VideosCmd.splusLoadMore()),
            ),
          ),
        ],
      ],
    );
  }
}

// ---------------------------------------------------------------- detail ----

class _Detail extends StatelessWidget {
  const _Detail({required this.controller, required this.splus});

  final VideosController controller;
  final SplusView splus;

  Future<void> _copy(BuildContext context) async {
    final url = videosSplusLink();
    if (url.isEmpty) return;
    await Clipboard.setData(ClipboardData(text: url));
    if (!context.mounted) return;
    ScaffoldMessenger.of(context)
        .showSnackBar(const SnackBar(content: Text('Stream link copied.')));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = splus;
    return ListView(
      padding: const EdgeInsets.fromLTRB(_pad, 18, _pad, 24),
      children: [
        Row(
          children: [
            PlexButton(
              compact: true,
              icon: Icons.chevron_left,
              label: 'Results',
              onTap: () => controller.send(const VideosCmd.splusBack()),
            ),
            const SizedBox(width: 8),
            PlexButton(
              compact: true,
              icon: Icons.home_outlined,
              label: 'Home',
              onTap: () =>
                  controller.send(const VideosCmd.splusSetView(view: 'home')),
            ),
          ],
        ),
        const SizedBox(height: 18),
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Artwork(path: s.dCover, width: 186, height: 279, radius: 11),
            const SizedBox(width: 22),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(s.dTitle,
                      style: TextStyle(
                          fontSize: 27,
                          fontWeight: FontWeight.w800,
                          color: t.nInk)),
                  const SizedBox(height: 6),
                  Text(s.dMeta,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 13, color: t.nInk3)),
                  if (s.dAudio.isNotEmpty) ...[
                    const SizedBox(height: 10),
                    _ChipRow(
                      caption: 'AUDIO',
                      chips: s.dAudio,
                      onTap: (id) =>
                          controller.send(VideosCmd.splusSetAudio(id: id)),
                    ),
                  ],
                  if (s.dSeasons.length > 1) ...[
                    const SizedBox(height: 8),
                    _ChipRow(
                      caption: 'SEASON',
                      chips: s.dSeasons,
                      onTap: (id) =>
                          controller.send(VideosCmd.splusSetSeason(id: id)),
                    ),
                  ],
                  if (s.dOverview.isNotEmpty) ...[
                    const SizedBox(height: 10),
                    Text(s.dOverview,
                        style: TextStyle(fontSize: 13.5, color: t.nInk2)),
                  ],
                  const SizedBox(height: 14),
                  Wrap(
                    spacing: 9,
                    runSpacing: 9,
                    children: [
                      PlexButton(
                          icon: Icons.play_arrow,
                          label: 'Play',
                          hue: cPlay,
                          filled: true,
                          compact: true,
                          onTap: () =>
                              controller.send(const VideosCmd.splusPlay())),
                      PlexButton(
                          icon: Icons.download,
                          label: 'Download',
                          hue: cDl,
                          filled: true,
                          compact: true,
                          onTap: () =>
                              controller.send(const VideosCmd.splusDownload())),
                      PlexButton(
                          icon: Icons.playlist_add,
                          label: 'Season',
                          compact: true,
                          onTap: () => controller
                              .send(const VideosCmd.splusDownloadSeason())),
                      PlexButton(
                          icon: Icons.bookmark_outline,
                          label: s.dSaved ? 'Saved' : 'Save',
                          hue: cSave,
                          filled: s.dSaved,
                          compact: true,
                          onTap: () => controller
                              .send(const VideosCmd.splusToggleSave())),
                      PlexButton(
                          icon: Icons.link,
                          label: 'Copy link',
                          compact: true,
                          onTap: () => _copy(context)),
                      PlexButton(
                          icon: Icons.cast,
                          label: 'Cast',
                          compact: true,
                          onTap: () =>
                              controller.send(const VideosCmd.splusCast())),
                      PlexButton(
                          icon: Icons.add,
                          label: 'Add to library',
                          compact: true,
                          onTap: () => controller
                              .send(const VideosCmd.splusAddToLibrary())),
                    ],
                  ),
                ],
              ),
            ),
          ],
        ),
        if (s.dEpisodes.isNotEmpty) ...[
          const SizedBox(height: 22),
          _ShelfHead('Episodes', note: s.dSkipNote),
          SizedBox(
            height: 128,
            child: ListView.separated(
              scrollDirection: Axis.horizontal,
              itemCount: s.dEpisodes.length,
              separatorBuilder: (_, __) => const SizedBox(width: 9),
              itemBuilder: (_, i) => _EpisodeCard(
                episode: s.dEpisodes[i],
                active: s.dEpisodes[i].number == s.dEpisode,
                onTap: () => controller.send(
                    VideosCmd.splusSetEpisode(episode: s.dEpisodes[i].number)),
              ),
            ),
          ),
        ],
        const SizedBox(height: 22),
        _ShelfHead('Sources', note: s.dResolving ? 'resolving…' : s.dResolved),
        Container(
          decoration: BoxDecoration(
            color: t.panel,
            borderRadius: BorderRadius.circular(11),
            border: Border.all(color: t.nHair),
          ),
          child: s.dFiles.isEmpty
              ? Padding(
                  padding: const EdgeInsets.all(18),
                  child: Text(
                    s.dResolving
                        ? 'Asking each source in turn…'
                        : 'No source carried this episode.',
                    textAlign: TextAlign.center,
                    style: TextStyle(fontSize: 13, color: t.nInk3),
                  ),
                )
              : Column(
                  children: [
                    for (final f in s.dFiles)
                      _FileRow(
                        file: f,
                        active: f.index == s.dFile && !f.failed,
                        onPlay: () {
                          controller
                              .send(VideosCmd.splusSetFile(index: f.index));
                          controller.send(const VideosCmd.splusPlay());
                        },
                        onRetry: () =>
                            controller.send(const VideosCmd.splusRetrySource()),
                        onSelect: () => controller
                            .send(VideosCmd.splusSetFile(index: f.index)),
                      ),
                  ],
                ),
        ),
        if (s.dSubs.isNotEmpty) ...[
          const SizedBox(height: 22),
          const _ShelfHead('Subtitles'),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              for (final sub in s.dSubs)
                SortChip(
                  label: sub.label,
                  active: sub.active,
                  onTap: () =>
                      controller.send(VideosCmd.splusSetSub(id: sub.id)),
                ),
            ],
          ),
        ],
      ],
    );
  }
}

class _ChipRow extends StatelessWidget {
  const _ChipRow({
    required this.caption,
    required this.chips,
    required this.onTap,
  });

  final String caption;
  final List<SplusChip> chips;
  final ValueChanged<String> onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      children: [
        Text(caption,
            style: TextStyle(
                fontSize: 10,
                fontWeight: FontWeight.w800,
                letterSpacing: 1.2,
                color: t.nInk3)),
        const SizedBox(width: 8),
        for (final c in chips)
          Padding(
            padding: const EdgeInsets.only(right: 8),
            child: SortChip(
                label: c.label, active: c.active, onTap: () => onTap(c.id)),
          ),
      ],
    );
  }
}

class _EpisodeCard extends StatelessWidget {
  const _EpisodeCard({
    required this.episode,
    required this.active,
    required this.onTap,
  });

  final SplusEpisode episode;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(9),
      child: Container(
        width: 168,
        decoration: BoxDecoration(
          color: t.panel,
          borderRadius: BorderRadius.circular(9),
          border: Border.all(
              color:
                  active ? Tokens.secVideos.withValues(alpha: 0.53) : t.nHair),
        ),
        clipBehavior: Clip.antiAlias,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            SizedBox(
              height: 78,
              child: Stack(
                fit: StackFit.expand,
                children: [
                  Artwork(
                      path: episode.thumb,
                      width: 168,
                      height: 78,
                      radius: 0,
                      fallback: Icons.movie_outlined),
                  if (episode.progress > 0.01)
                    Positioned(
                      left: 0,
                      right: 0,
                      bottom: 0,
                      child: ProgressStrip(value: episode.progress, hue: cPlay),
                    ),
                ],
              ),
            ),
            Padding(
              padding: const EdgeInsets.all(8),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text('E${episode.number}',
                      style: const TextStyle(
                          fontSize: 11,
                          fontWeight: FontWeight.w700,
                          color: Tokens.secVideos)),
                  Text(
                    episode.title.isEmpty
                        ? 'Episode ${episode.number}'
                        : episode.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 12,
                        fontWeight: FontWeight.w600,
                        color: t.nInk),
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

class _FileRow extends StatelessWidget {
  const _FileRow({
    required this.file,
    required this.active,
    required this.onPlay,
    required this.onRetry,
    required this.onSelect,
  });

  final SplusFile file;
  final bool active;
  final VoidCallback onPlay;
  final VoidCallback onRetry;
  final VoidCallback onSelect;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onSelect,
      child: Container(
        height: 60,
        padding: const EdgeInsets.symmetric(horizontal: 14),
        decoration: BoxDecoration(
          color: active
              ? Tokens.secVideos.withValues(alpha: 0.10)
              : Colors.transparent,
          border: Border(bottom: BorderSide(color: t.nHair)),
        ),
        child: Row(
          children: [
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(file.label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: 14,
                        fontWeight: FontWeight.w700,
                        color: file.failed ? t.nInk3 : t.nInk,
                      )),
                  Text(file.failed ? file.note : file.sub,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 12, color: file.failed ? cSave : t.nInk3)),
                ],
              ),
            ),
            const SizedBox(width: 14),
            Text(file.subs,
                style: TextStyle(
                    fontSize: 12, color: file.hasSubs ? cPlay : t.nInk3)),
            const SizedBox(width: 14),
            if (file.failed)
              PlexButton(
                  compact: true,
                  icon: Icons.refresh,
                  label: 'Retry',
                  onTap: onRetry)
            else
              PlexButton(
                  compact: true,
                  icon: Icons.play_arrow,
                  label: 'Play',
                  hue: cPlay,
                  filled: true,
                  onTap: onPlay),
          ],
        ),
      ),
    );
  }
}

// --------------------------------------------------------------- library ----

class _Library extends StatelessWidget {
  const _Library({required this.controller, required this.splus});

  final VideosController controller;
  final SplusView splus;

  @override
  Widget build(BuildContext context) {
    final s = splus;
    final saved = s.libTab == 'saved';
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(_pad, 16, _pad, 10),
          child: Row(
            children: [
              SortChip(
                  label: 'Saved',
                  active: saved,
                  onTap: () => controller
                      .send(const VideosCmd.splusSetLibTab(tab: 'saved'))),
              const SizedBox(width: 8),
              SortChip(
                  label: 'History',
                  active: !saved,
                  onTap: () => controller
                      .send(const VideosCmd.splusSetLibTab(tab: 'history'))),
              const Spacer(),
              for (final k in const [
                (id: 'added', label: 'Recently added'),
                (id: 'watched', label: 'Recently watched'),
                (id: 'title', label: 'Title'),
                (id: 'year', label: 'Year'),
                (id: 'progress', label: 'Progress'),
              ])
                Padding(
                  padding: const EdgeInsets.only(left: 6),
                  child: SortChip(
                    label: k.label,
                    active: s.sort == k.id,
                    onTap: () =>
                        controller.send(VideosCmd.splusSetSort(key: k.id)),
                  ),
                ),
            ],
          ),
        ),
        Expanded(
          child: saved
              ? (s.saved.isEmpty
                  ? const VideosEmpty(
                      icon: Icons.bookmark_border,
                      title: 'Nothing saved yet',
                      message: 'The bookmark on a title puts it here.',
                    )
                  : ListView(
                      padding: const EdgeInsets.fromLTRB(_pad, 0, _pad, 24),
                      children: [
                        _ShelfHead('Saved', note: '${s.saved.length} titles'),
                        Wrap(
                          spacing: 16,
                          runSpacing: 16,
                          children: [
                            for (final c in s.saved)
                              _SavedPoster(
                                card: c,
                                onOpen: () => controller
                                    .send(VideosCmd.splusOpenCard(key: c.key)),
                                onRemove: () => controller.send(
                                    VideosCmd.splusSavedRemove(key: c.key)),
                              ),
                          ],
                        ),
                      ],
                    ))
              : (s.history.isEmpty
                  ? const VideosEmpty(
                      icon: Icons.history_toggle_off,
                      title: 'Nothing watched yet',
                    )
                  : ListView(
                      padding: const EdgeInsets.fromLTRB(_pad, 0, _pad, 24),
                      children: [
                        _ShelfHead('History',
                            note: 'newest first · ${s.history.length} entries'),
                        for (final h in s.history)
                          _HistoryRow(row: h, controller: controller),
                        const SizedBox(height: 14),
                        Align(
                          alignment: Alignment.centerRight,
                          child: PlexButton(
                            icon: Icons.delete_outline,
                            label: 'Clear history',
                            hue: cErr,
                            compact: true,
                            onTap: () async {
                              final ok = await showDialog<bool>(
                                context: context,
                                builder: (ctx) => AlertDialog(
                                  title: const Text('Clear the history?'),
                                  content: const Text(
                                      'Every session goes, along with the '
                                      'resume points they carry.'),
                                  actions: [
                                    TextButton(
                                        onPressed: () =>
                                            Navigator.pop(ctx, false),
                                        child: const Text('Cancel')),
                                    FilledButton(
                                      style: FilledButton.styleFrom(
                                          backgroundColor: cErr),
                                      onPressed: () => Navigator.pop(ctx, true),
                                      child: const Text('Clear'),
                                    ),
                                  ],
                                ),
                              );
                              if (ok ?? false) {
                                await controller
                                    .send(const VideosCmd.splusHistoryClear());
                              }
                            },
                          ),
                        ),
                      ],
                    )),
        ),
      ],
    );
  }
}

class _SavedPoster extends StatelessWidget {
  const _SavedPoster({
    required this.card,
    required this.onOpen,
    required this.onRemove,
  });

  final SplusCard card;
  final VoidCallback onOpen;
  final VoidCallback onRemove;

  @override
  Widget build(BuildContext context) {
    return Stack(
      children: [
        _Poster(card: card, onTap: onOpen),
        Positioned(
          top: 4,
          right: 4,
          child: IconBtn(
              icon: Icons.close,
              tip: 'Remove',
              hue: cErr,
              size: 24,
              onTap: onRemove),
        ),
      ],
    );
  }
}

class _HistoryRow extends StatelessWidget {
  const _HistoryRow({required this.row, required this.controller});

  final SplusHistory row;
  final VideosController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 72,
      padding: const EdgeInsets.symmetric(horizontal: 14),
      decoration: BoxDecoration(
        color: t.panel,
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          Artwork(path: row.poster, width: 56, height: 36, radius: 6),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(row.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 14,
                        fontWeight: FontWeight.w600,
                        color: t.nInk)),
                Text('${row.kind} · ${row.detail}',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 12, color: t.nInk3)),
              ],
            ),
          ),
          SizedBox(
            width: 130,
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(row.state,
                    style: TextStyle(
                        fontSize: 12, color: row.finished ? cPlay : t.nInk2)),
                const SizedBox(height: 4),
                ClipRRect(
                  borderRadius: BorderRadius.circular(2),
                  child: ProgressStrip(
                      value: row.progress, hue: row.finished ? cPlay : cDl),
                ),
              ],
            ),
          ),
          SizedBox(
            width: 100,
            child: Text(row.ago,
                textAlign: TextAlign.right,
                style: TextStyle(fontSize: 12, color: t.nInk3)),
          ),
          const SizedBox(width: 10),
          IconBtn(
              icon: Icons.play_arrow,
              tip: 'Resume',
              hue: cPlay,
              size: 30,
              filled: true,
              onTap: () =>
                  controller.send(VideosCmd.splusHistoryPlay(key: row.key))),
          const SizedBox(width: 6),
          IconBtn(
              icon: Icons.close,
              tip: 'Remove',
              hue: cErr,
              size: 30,
              onTap: () =>
                  controller.send(VideosCmd.splusHistoryRemove(key: row.key))),
        ],
      ),
    );
  }
}

// ------------------------------------------------------------- downloads ----

class _Downloads extends StatelessWidget {
  const _Downloads({required this.controller, required this.splus});

  final VideosController controller;
  final SplusView splus;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = splus;
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(_pad, 16, _pad, 10),
          child: Row(
            children: [
              Text(
                s.dlActive > 0
                    ? '${s.dlActive} running · ${s.dlRate}'
                    : 'Nothing downloading',
                style: TextStyle(fontSize: 12.5, color: t.nInk2),
              ),
              const Spacer(),
              PlexButton(
                  compact: true,
                  label: 'Cancel all',
                  onTap: () =>
                      controller.send(const VideosCmd.splusDlCancelAll())),
              const SizedBox(width: 8),
              PlexButton(
                  compact: true,
                  label: 'Clear finished',
                  onTap: () => controller.send(const VideosCmd.splusDlClear())),
            ],
          ),
        ),
        Expanded(
          child: s.downloads.isEmpty
              ? const VideosEmpty(
                  icon: Icons.download,
                  title: 'Nothing queued',
                  message: 'Download on a title puts it here.',
                )
              : ListView.builder(
                  padding: const EdgeInsets.fromLTRB(_pad, 0, _pad, 24),
                  itemCount: s.downloads.length,
                  itemBuilder: (_, i) => _SplusDownloadRow(
                      row: s.downloads[i], controller: controller),
                ),
        ),
      ],
    );
  }
}

class _SplusDownloadRow extends StatelessWidget {
  const _SplusDownloadRow({required this.row, required this.controller});

  final SplusDownload row;
  final VideosController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final hue = row.failed
        ? cSave
        : row.done
            ? cPlay
            : (row.active ? cDl : t.nInk3);
    return Container(
      height: 74,
      padding: const EdgeInsets.symmetric(horizontal: 14),
      decoration: BoxDecoration(
        color: t.panel,
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          Expanded(
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(row.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 14,
                        fontWeight: FontWeight.w600,
                        color: t.nInk)),
                Text(row.detail,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 12, color: row.failed ? cSave : t.nInk3)),
              ],
            ),
          ),
          SizedBox(
            width: 200,
            child: ClipRRect(
              borderRadius: BorderRadius.circular(2),
              child: ProgressStrip(value: row.progress, hue: hue),
            ),
          ),
          const SizedBox(width: 14),
          SizedBox(
            width: 104,
            child: Container(
              height: 24,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                  color: hue, borderRadius: BorderRadius.circular(5)),
              child: Text(row.state,
                  style: const TextStyle(
                      fontSize: 10.5,
                      fontWeight: FontWeight.w800,
                      letterSpacing: 0.6,
                      color: Colors.white)),
            ),
          ),
          const SizedBox(width: 10),
          if (row.done) ...[
            IconBtn(
                icon: Icons.play_arrow,
                tip: 'Play',
                hue: cPlay,
                size: 30,
                filled: true,
                onTap: () =>
                    controller.send(VideosCmd.splusDlPlay(id: row.id))),
            const SizedBox(width: 6),
            IconBtn(
                icon: Icons.folder_open,
                tip: 'Show in files',
                size: 30,
                onTap: () =>
                    controller.send(VideosCmd.splusDlReveal(id: row.id))),
            const SizedBox(width: 6),
            IconBtn(
                icon: Icons.delete_outline,
                tip: 'Delete the file',
                hue: cErr,
                size: 30,
                onTap: () =>
                    controller.send(VideosCmd.splusDlDelete(id: row.id))),
          ],
          if (row.failed)
            IconBtn(
                icon: Icons.refresh,
                tip: 'Retry',
                hue: cBack,
                size: 30,
                onTap: () =>
                    controller.send(VideosCmd.splusDlRetry(id: row.id))),
          if (!row.done && !row.failed)
            IconBtn(
                icon: Icons.close,
                tip: 'Cancel',
                hue: cErr,
                size: 30,
                onTap: () =>
                    controller.send(VideosCmd.splusDlCancel(id: row.id))),
        ],
      ),
    );
  }
}

// -------------------------------------------------------------- settings ----

class _Settings extends StatelessWidget {
  const _Settings({required this.controller, required this.splus});

  final VideosController controller;
  final SplusView splus;

  void _flag(String key, bool on) =>
      controller.send(VideosCmd.splusSetFlag(key: key, on_: on));
  void _num(String key, int value) =>
      controller.send(VideosCmd.splusSetNum(key: key, value: value));
  void _text(String key, String value) =>
      controller.send(VideosCmd.splusSetText(key: key, value: value));

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = splus;
    return ListView(
      padding: const EdgeInsets.fromLTRB(_pad, 20, _pad, 26),
      children: [
        const _ShelfHead('Sources',
            note: 'tried top to bottom · three failures in a row sinks one '
                'for an hour'),
        _Card(
          child: Column(
            children: [
              for (var i = 0; i < s.health.length; i++)
                _SourceRow(
                  index: i,
                  health: s.health[i],
                  controller: controller,
                ),
              const SizedBox(height: 10),
              Row(
                children: [
                  PlexButton(
                      compact: true,
                      icon: Icons.network_check,
                      label: 'Test all',
                      onTap: () =>
                          controller.send(const VideosCmd.splusSourceTest())),
                  const SizedBox(width: 9),
                  PlexButton(
                      compact: true,
                      icon: Icons.restart_alt,
                      label: 'Reset',
                      onTap: () =>
                          controller.send(const VideosCmd.splusSourceReset())),
                  const SizedBox(width: 9),
                  PlexButton(
                      compact: true,
                      icon: Icons.dns_outlined,
                      label: 'Servers…',
                      onTap: () => openSplusServers(context, controller)),
                ],
              ),
            ],
          ),
        ),
        const SizedBox(height: 20),
        const _ShelfHead('Anime',
            note: 'AniList metadata · TMDB numbering correction'),
        _Card(
          child: Column(
            children: [
              _PickerRow(
                label: 'Preferred audio',
                options: const [
                  (id: 'sub_then_dub', label: 'Sub first'),
                  (id: 'dub_then_sub', label: 'Dub first'),
                ],
                selected: s.audioPref,
                onTap: (id) => _text('audio_pref', id),
              ),
              StreamToggle(
                label: 'Skip opening and ending',
                hint: 'AniSkip timestamps, keyed by MyAnimeList id',
                value: s.skipOpEd,
                onChanged: (v) => _flag('skip_op_ed', v),
              ),
              _NumRow(
                label: 'Autoplay next episode',
                hint: 'countdown before the next one starts',
                values: const [0, 5, 10, 20],
                labels: const ['Off', '5 s', '10 s', '20 s'],
                selected: s.autoplaySecs,
                onTap: (v) => _num('autoplay_seconds', v),
              ),
              _NumRow(
                label: 'Mark watched at',
                hint: 'progress past this point counts as finished',
                values: const [80, 85, 90, 95],
                labels: const ['80%', '85%', '90%', '95%'],
                selected: s.watchedPct,
                onTap: (v) => _num('watched_threshold', v),
              ),
              StreamToggle(
                label: 'Hide episodes you have finished',
                hint: 'the strip starts at the first unwatched one',
                value: s.hideFinished,
                onChanged: (v) => _flag('hide_finished', v),
              ),
            ],
          ),
        ),
        const SizedBox(height: 20),
        const _ShelfHead('Subtitles',
            note: "two more sources behind the video section's subtitle "
                'search'),
        _Card(
          child: Column(
            children: [
              StreamToggle(
                label: 'Wyzie',
                hint: 'no key needed',
                value: s.subsWyzie,
                onChanged: (v) => _flag('subs_wyzie', v),
              ),
              StreamToggle(
                label: 'SubDL',
                hint: 'API key stored in the system keychain',
                value: s.subsSubdl,
                onChanged: (v) => _flag('subs_subdl', v),
              ),
              _PickerRow(
                label: 'Preferred language',
                options: const [
                  (id: 'en', label: 'en'),
                  (id: 'de', label: 'de'),
                  (id: 'es', label: 'es'),
                  (id: 'fr', label: 'fr'),
                  (id: 'ja', label: 'ja'),
                ],
                selected: s.subsLang,
                onTap: (id) => _text('subs_lang', id),
              ),
              StreamToggle(
                label: 'Download subtitles with the file',
                hint: 'written beside the video as .srt',
                value: s.subsWithFile,
                onChanged: (v) => _flag('subs_with_file', v),
              ),
            ],
          ),
        ),
        const SizedBox(height: 20),
        const _ShelfHead('Player & downloads'),
        _Card(
          child: Column(
            children: [
              _NumRow(
                label: 'Preferred quality',
                hint: 'picks the closest available, never higher',
                values: const [480, 720, 1080, 2160],
                labels: const ['480p', '720p', '1080p', '2160p'],
                selected: s.quality,
                onTap: (v) => _num('quality', v),
              ),
              StreamToggle(
                label: 'Download into the library',
                hint: 'finished files appear under Videos → Library',
                value: s.intoLibrary,
                onChanged: (v) => _flag('into_library', v),
              ),
              Row(
                children: [
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text('Download folder',
                            style: TextStyle(
                                fontSize: 13.5,
                                fontWeight: FontWeight.w600,
                                color: t.nInk)),
                        Text(s.downloadDir,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 11.5, color: t.nInk3)),
                      ],
                    ),
                  ),
                  PlexButton(
                    compact: true,
                    label: 'Change',
                    onTap: () => _pickDir(context, s.downloadDir),
                  ),
                ],
              ),
              const SizedBox(height: 10),
              _NumRow(
                label: 'Simultaneous downloads',
                hint: 'more slots is not faster on one host',
                values: const [1, 2, 3, 4],
                labels: const ['1', '2', '3', '4'],
                selected: s.slots,
                onTap: (v) => _num('slots', v),
              ),
            ],
          ),
        ),
        const SizedBox(height: 20),
        const _ShelfHead('Notifications'),
        _Card(
          child: Column(
            children: [
              StreamToggle(
                label: 'Download finished',
                hint: 'one per file, none for a season batch',
                value: s.notifyDownload,
                onChanged: (v) => _flag('notify_download', v),
              ),
              StreamToggle(
                label: 'New episode of a saved show',
                hint: 'checked when the app is idle, at most once an hour',
                value: s.notifyEpisode,
                onChanged: (v) => _flag('notify_episode', v),
              ),
            ],
          ),
        ),
      ],
    );
  }

  /// Typed rather than a native chooser: a cdylib has no window to parent a
  /// GTK dialog to, so the path comes in as text — the same shape the library's
  /// "Add folder" uses.
  Future<void> _pickDir(BuildContext context, String current) async {
    final text = TextEditingController(text: current);
    final path = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Download folder'),
        content: TextField(
          controller: text,
          autofocus: true,
          onSubmitted: (v) => Navigator.pop(ctx, v),
        ),
        actions: [
          TextButton(
              onPressed: () => Navigator.pop(ctx), child: const Text('Cancel')),
          FilledButton(
              onPressed: () => Navigator.pop(ctx, text.text),
              child: const Text('Use it')),
        ],
      ),
    );
    text.dispose();
    if (path == null || path.trim().isEmpty) return;
    await controller.send(VideosCmd.splusSetDownloadDir(path: path.trim()));
  }
}

class _Card extends StatelessWidget {
  const _Card({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(11),
        border: Border.all(color: t.nHair),
      ),
      child: child,
    );
  }
}

class _SourceRow extends StatelessWidget {
  const _SourceRow({
    required this.index,
    required this.health,
    required this.controller,
  });

  final int index;
  final SplusHealth health;
  final VideosController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 46,
      margin: const EdgeInsets.only(bottom: 6),
      padding: const EdgeInsets.symmetric(horizontal: 12),
      decoration: BoxDecoration(
        color: t.panel2,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: t.nHair),
      ),
      child: Row(
        children: [
          SizedBox(
            width: 16,
            child: Text('${index + 1}',
                style: const TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w700,
                    color: Tokens.secVideos)),
          ),
          const SizedBox(width: 11),
          Expanded(
            child: Text(health.label,
                style: TextStyle(
                    fontSize: 13.5,
                    fontWeight: FontWeight.w600,
                    color: t.nInk)),
          ),
          if (health.note.isNotEmpty)
            SortChip(
              label: health.note,
              active: true,
              hue: health.ok ? cPlay : (health.sunk ? cSave : cBack),
              onTap: () {},
            ),
          const SizedBox(width: 8),
          IconBtn(
              icon: Icons.keyboard_arrow_up,
              tip: 'Move up',
              size: 28,
              onTap: () => controller.send(
                  VideosCmd.splusMoveSource(id: health.source, delta: -1))),
          const SizedBox(width: 4),
          IconBtn(
              icon: Icons.keyboard_arrow_down,
              tip: 'Move down',
              size: 28,
              onTap: () => controller.send(
                  VideosCmd.splusMoveSource(id: health.source, delta: 1))),
          const SizedBox(width: 8),
          Switch(
            value: health.enabled,
            activeThumbColor: Tokens.secVideos,
            onChanged: (v) => controller.send(
                VideosCmd.splusSetSourceEnabled(id: health.source, on_: v)),
          ),
        ],
      ),
    );
  }
}

class _PickerRow extends StatelessWidget {
  const _PickerRow({
    required this.label,
    required this.options,
    required this.selected,
    required this.onTap,
  });

  final String label;
  final List<({String id, String label})> options;
  final String selected;
  final ValueChanged<String> onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 8),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(label,
                    style: TextStyle(
                        fontSize: 13.5,
                        fontWeight: FontWeight.w600,
                        color: t.nInk)),
              ],
            ),
          ),
          for (final o in options)
            Padding(
              padding: const EdgeInsets.only(left: 8),
              child: SortChip(
                  label: o.label,
                  active: selected == o.id,
                  onTap: () => onTap(o.id)),
            ),
        ],
      ),
    );
  }
}

class _NumRow extends StatelessWidget {
  const _NumRow({
    required this.label,
    required this.values,
    required this.labels,
    required this.selected,
    required this.onTap,
    this.hint,
  });

  final String label;
  final String? hint;
  final List<int> values;
  final List<String> labels;
  final int selected;
  final ValueChanged<int> onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 8),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(label,
                    style: TextStyle(
                        fontSize: 13.5,
                        fontWeight: FontWeight.w600,
                        color: t.nInk)),
                if (hint != null)
                  Text(hint!, style: TextStyle(fontSize: 11.5, color: t.nInk3)),
              ],
            ),
          ),
          for (var i = 0; i < values.length; i++)
            Padding(
              padding: const EdgeInsets.only(left: 8),
              child: SortChip(
                label: labels[i],
                active: selected == values[i],
                onTap: () => onTap(values[i]),
              ),
            ),
        ],
      ),
    );
  }
}

/// The extra-hosts editor. One origin per line; the resolver tries them in the
/// order the Sources list is in, not this one.
Future<void> openSplusServers(
    BuildContext context, VideosController controller) async {
  await controller.send(const VideosCmd.splusServersOpened());
  if (!context.mounted) return;
  final text =
      TextEditingController(text: controller.state?.splus.hostsText ?? '');
  await showDialog<void>(
    context: context,
    builder: (ctx) => AnimatedBuilder(
      animation: controller,
      builder: (ctx, _) {
        final s = controller.state?.splus;
        final t = ctx.tokens;
        return AlertDialog(
          backgroundColor: t.modal,
          title: const Text('Servers'),
          content: SizedBox(
            width: 500,
            height: 340,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text('One origin per line. Blank restores the built-in list.',
                    style: TextStyle(fontSize: 11, color: t.nInk3)),
                const SizedBox(height: 10),
                Expanded(
                  child: TextField(
                    controller: text,
                    maxLines: null,
                    expands: true,
                    style:
                        const TextStyle(fontSize: 13, fontFamily: 'monospace'),
                    decoration: const InputDecoration(
                        border: OutlineInputBorder(), isDense: true),
                  ),
                ),
                if (s?.hostsSaved ?? false) ...[
                  const SizedBox(height: 8),
                  const Text('Applied ✓',
                      style: TextStyle(
                          fontSize: 12,
                          fontWeight: FontWeight.w700,
                          color: cPlay)),
                ],
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => controller
                  .send(const VideosCmd.splusHostsReset())
                  .then((_) =>
                      text.text = controller.state?.splus.hostsText ?? ''),
              child: const Text('Reset'),
            ),
            TextButton(
              onPressed: () =>
                  controller.send(const VideosCmd.splusHostsCheck()),
              child: Text((s?.hostsChecking ?? false) ? 'Testing…' : 'Test'),
            ),
            FilledButton(
              onPressed: () =>
                  controller.send(VideosCmd.splusHostsSave(text: text.text)),
              child: const Text('Save'),
            ),
            TextButton(
                onPressed: () => Navigator.pop(ctx),
                child: const Text('Close')),
          ],
        );
      },
    ),
  );
  text.dispose();
}
