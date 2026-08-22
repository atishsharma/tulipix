// Podcasts — Home, Subscribed and Downloads, plus the show page.
//
// Subscribing is one HTTP GET and one call into the domain crate's feed
// parser; everything after that is rows in podcasts.db. Playing an episode
// takes over the same player bar My Music was using, which is the whole point
// of the five tabs being one section.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';
import 'music_widgets.dart';

class PodcastsTab extends StatelessWidget {
  const PodcastsTab({super.key, required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    if (st == null) return const Center(child: CircularProgressIndicator());
    if (st.podDetailOpen) return _ShowPage(controller: controller, st: st);

    return Column(
      children: [
        _Header(controller: controller, st: st),
        Expanded(
          child: switch (st.podTab) {
            'downloads' => _Downloads(controller: controller, st: st),
            'subscribed' => _Subscribed(controller: controller, st: st),
            _ => _Home(controller: controller, st: st),
          },
        ),
      ],
    );
  }
}

class _Header extends StatelessWidget {
  const _Header({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      height: 52,
      child: Row(
        children: [
          const SizedBox(width: 20),
          for (final tab in const [
            ('home', 'Home'),
            ('subscribed', 'Subscribed'),
            ('downloads', 'Downloads'),
          ])
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 4),
              child: MusicChip(
                label: tab.$2,
                active: st.podTab == tab.$1,
                tint: const Color(0xFFF97316),
                tint2: const Color(0xFFEC4899),
                badge: tab.$1 == 'subscribed' ? '${st.podTotal}' : null,
                onTap: () => controller.send(MusicCmd.podSetTab(name: tab.$1)),
              ),
            ),
          const SizedBox(width: 16),
          if (st.podCategories.length > 1)
            SizedBox(
              width: 180,
              child: DropdownButton<String>(
                value: st.podCategories.contains(st.podCat)
                    ? st.podCat
                    : st.podCategories.first,
                isExpanded: true,
                underline: const SizedBox.shrink(),
                items: [
                  for (final c in st.podCategories)
                    DropdownMenuItem(value: c, child: Text(c)),
                ],
                onChanged: (v) => v == null
                    ? null
                    : controller.send(MusicCmd.podSetCat(name: v)),
              ),
            ),
          const Spacer(),
          if (st.podQueue.isNotEmpty)
            TextButton.icon(
              icon: const Icon(Icons.playlist_play, size: 16),
              label: Text('Queue · ${st.podQueue.length}'),
              onPressed: () => _showQueue(context, controller, st),
            ),
          TextButton.icon(
            icon: const Icon(Icons.refresh, size: 16),
            label: const Text('Refresh all'),
            onPressed: () => controller.send(const MusicCmd.podRefresh()),
          ),
          FilledButton.icon(
            icon: const Icon(Icons.add, size: 16),
            label: const Text('Add feed'),
            onPressed: () => _addFeed(context, controller),
          ),
          PopupMenuButton<String>(
            tooltip: 'More',
            onSelected: (v) => _more(context, controller, v),
            itemBuilder: (_) => const [
              PopupMenuItem(value: 'import', child: Text('Import OPML…')),
              PopupMenuItem(value: 'export', child: Text('Export OPML…')),
              PopupMenuItem(
                  value: 'reset', child: Text('Delete every subscription…')),
            ],
          ),
          const SizedBox(width: 12),
          Container(width: 1, height: 24, color: t.outline),
          const SizedBox(width: 12),
        ],
      ),
    );
  }
}

/// Import, export and the reset. OPML is how a podcast library moves between
/// apps, and both directions go through the domain crate's parser so a file
/// written here opens in the Slint build and anywhere else.
Future<void> _more(
    BuildContext context, MusicController c, String choice) async {
  switch (choice) {
    case 'import':
      final path = await promptPath(
        context,
        title: 'Import OPML',
        label: 'Path to the .opml file',
        hint: '/home/you/Downloads/podcasts.opml',
        confirm: 'Import',
      );
      if (path != null) await c.send(MusicCmd.podOpmlImport(path: path));
    case 'export':
      final path = await promptPath(
        context,
        title: 'Export OPML',
        label: 'Where to write it',
        hint: '/home/you/podcasts.opml',
        confirm: 'Export',
      );
      if (path != null) await c.send(MusicCmd.podOpmlExport(path: path));
    case 'reset':
      final ok = await confirm(
        context,
        title: 'Delete every subscription?',
        body: 'Every show, every stored episode and every saved audio file '
            'goes. Export an OPML first if you want the list back.',
        action: 'Delete everything',
      );
      if (ok) await c.send(const MusicCmd.podResetAll());
  }
}

/// The episode queue, which is the podcast half of what the player bar's
/// Queue panel is for library tracks. Separate because the two hold different
/// things — `items` rows on one side, episode ids on the other.
Future<void> _showQueue(
    BuildContext context, MusicController c, MusicState st) async {
  await showDialog<void>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Episode queue'),
      content: SizedBox(
        width: 520,
        height: 360,
        child: ListView(
          children: [
            for (final e in st.podQueue)
              ListTile(
                dense: true,
                title:
                    Text(e.title, maxLines: 1, overflow: TextOverflow.ellipsis),
                subtitle:
                    Text(e.show_, maxLines: 1, overflow: TextOverflow.ellipsis),
                onTap: () {
                  c.send(MusicCmd.podPlay(episodeId: e.id));
                  Navigator.pop(ctx);
                },
                trailing: IconButton(
                  icon: const Icon(Icons.close, size: 16),
                  tooltip: 'Remove',
                  onPressed: () =>
                      c.send(MusicCmd.podQueueRemove(episodeId: e.id)),
                ),
              ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () {
            c.send(const MusicCmd.podQueueClear());
            Navigator.pop(ctx);
          },
          child: const Text('Clear'),
        ),
        FilledButton(
            onPressed: () => Navigator.pop(ctx), child: const Text('Done')),
      ],
    ),
  );
}

Future<void> _addFeed(BuildContext context, MusicController c) async {
  final text = TextEditingController();
  final url = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Add a podcast'),
      content: SizedBox(
        width: 420,
        child: TextField(
          controller: text,
          autofocus: true,
          decoration: const InputDecoration(
            labelText: 'RSS feed URL',
            hintText: 'https://example.com/feed.xml',
          ),
          onSubmitted: (v) => Navigator.pop(ctx, v),
        ),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx), child: const Text('Cancel')),
        FilledButton(
          onPressed: () => Navigator.pop(ctx, text.text),
          child: const Text('Subscribe'),
        ),
      ],
    ),
  );
  final trimmed = url?.trim() ?? '';
  if (trimmed.isNotEmpty) await c.send(MusicCmd.podSubscribe(url: trimmed));
}

class _Home extends StatelessWidget {
  const _Home({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    if (st.podShows.isEmpty) {
      return MusicEmpty(
        icon: Icons.podcasts_outlined,
        title: 'No podcasts yet',
        body: 'Paste a feed URL and Tulipix will pull the show, its artwork '
            'and its back catalogue into podcasts.db.',
        action: ('Add a feed', () => _addFeed(context, controller)),
      );
    }
    return ListView(
      children: [
        Rail(
          title: 'Your shows',
          height: 210,
          action: (
            'See all',
            () => controller.send(const MusicCmd.podSetTab(name: 'subscribed'))
          ),
          children: [
            for (final s in st.podShows)
              MusicCard(
                controller: controller,
                title: s.title,
                subtitle: s.author,
                artKind: 'podcast',
                artKey: s.art,
                fallback: Icons.podcasts,
                badge: s.unplayed > 0 ? '${s.unplayed}' : null,
                onTap: () => controller.send(MusicCmd.podOpen(podcastId: s.id)),
              ),
          ],
        ),
        if (st.podLatest.isNotEmpty) ...[
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 24, 24, 8),
            child: Text('Latest episodes',
                style: TextStyle(
                    fontSize: 15,
                    fontWeight: FontWeight.w700,
                    color: context.tokens.nInk)),
          ),
          for (final e in st.podLatest)
            EpisodeRow(controller: controller, episode: e),
        ],
        const SizedBox(height: 24),
      ],
    );
  }
}

class _Subscribed extends StatelessWidget {
  const _Subscribed({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    if (st.podShows.isEmpty) {
      return MusicEmpty(
        icon: Icons.podcasts_outlined,
        title: 'Nothing in this category',
        body: 'Pick another category, or add a feed.',
        action: ('Add a feed', () => _addFeed(context, controller)),
      );
    }
    return Column(
      children: [
        Expanded(
          child: CardGrid(
            children: [
              for (final s in st.podShows)
                MusicCard(
                  controller: controller,
                  title: s.title,
                  subtitle: '${s.episodes} episodes',
                  artKind: 'podcast',
                  artKey: s.art,
                  fallback: Icons.podcasts,
                  badge: s.unplayed > 0 ? '${s.unplayed}' : null,
                  onTap: () =>
                      controller.send(MusicCmd.podOpen(podcastId: s.id)),
                  onMenu: () =>
                      controller.send(MusicCmd.podUnsubscribe(podcastId: s.id)),
                ),
            ],
          ),
        ),
        Pager(
          page: st.podPage,
          pages: st.podPages,
          onGo: (p) => controller.send(MusicCmd.podSetPage(page: p)),
        ),
      ],
    );
  }
}

class _Downloads extends StatelessWidget {
  const _Downloads({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    if (st.podDownloads.isEmpty) {
      return const MusicEmpty(
        icon: Icons.download_outlined,
        title: 'Nothing saved offline',
        body: 'Download an episode from a show page and it lands here, '
            'playable with no network at all.',
      );
    }
    return Column(
      children: [
        Row(
          children: [
            const Spacer(),
            TextButton.icon(
              icon: const Icon(Icons.delete_sweep_outlined, size: 16),
              label: const Text('Delete all'),
              onPressed: () =>
                  controller.send(const MusicCmd.podClearDownloads()),
            ),
            const SizedBox(width: 12),
          ],
        ),
        Expanded(
          child: ListView(
            children: [
              for (final e in st.podDownloads)
                EpisodeRow(controller: controller, episode: e),
            ],
          ),
        ),
        Pager(
          page: st.podPage,
          pages: st.podPages,
          onGo: (p) => controller.send(MusicCmd.podSetPage(page: p)),
        ),
      ],
    );
  }
}

class _ShowPage extends StatelessWidget {
  const _ShowPage({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final show = st.podDetail;
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
          child: Row(
            children: [
              IconButton(
                icon: const Icon(Icons.arrow_back),
                tooltip: 'Back',
                onPressed: () => controller.send(const MusicCmd.podBack()),
              ),
              const Spacer(),
              TextButton.icon(
                icon: const Icon(Icons.refresh, size: 16),
                label: const Text('Refresh'),
                onPressed: show == null
                    ? null
                    : () => controller
                        .send(MusicCmd.podRefreshOne(podcastId: show.id)),
              ),
              TextButton.icon(
                icon: const Icon(Icons.link_off, size: 16),
                label: const Text('Unsubscribe'),
                onPressed: show == null
                    ? null
                    : () => controller
                        .send(MusicCmd.podUnsubscribe(podcastId: show.id)),
              ),
            ],
          ),
        ),
        if (show != null)
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 8, 24, 12),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                MusicArt(
                  controller: controller,
                  kind: 'podcast',
                  artKey: show.art,
                  size: 128,
                  radius: 12,
                  fallback: Icons.podcasts,
                ),
                const SizedBox(width: 20),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(show.title,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 22,
                              fontWeight: FontWeight.w700,
                              color: t.nInk)),
                      Text(
                        [
                          show.author,
                          show.category,
                          '${show.episodes} episodes'
                        ].where((s) => s.isNotEmpty).join(' · '),
                        style: TextStyle(fontSize: 12, color: t.nInk2),
                      ),
                      const SizedBox(height: 8),
                      Text(
                        show.description,
                        maxLines: 4,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 12, color: t.nInk2),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        Row(
          children: [
            const SizedBox(width: 24),
            MusicChip(
              label: 'Newest first',
              active: st.podEpSort != 'old',
              tint: const Color(0xFFF97316),
              tint2: const Color(0xFFEC4899),
              onTap: () =>
                  controller.send(const MusicCmd.podSetEpSort(mode: 'new')),
            ),
            const SizedBox(width: 6),
            MusicChip(
              label: 'Oldest first',
              active: st.podEpSort == 'old',
              tint: const Color(0xFFF97316),
              tint2: const Color(0xFFEC4899),
              onTap: () =>
                  controller.send(const MusicCmd.podSetEpSort(mode: 'old')),
            ),
          ],
        ),
        Expanded(
          child: st.podEpisodes.isEmpty
              ? const MusicEmpty(
                  icon: Icons.inbox_outlined,
                  title: 'No episodes stored',
                  body: 'Refresh the feed to pull them in.',
                )
              : ListView(
                  children: [
                    for (final e in st.podEpisodes)
                      EpisodeRow(controller: controller, episode: e),
                  ],
                ),
        ),
        Pager(
          page: st.podEpPage,
          pages: st.podEpPages,
          onGo: (p) => controller.send(MusicCmd.podSetEpPage(page: p)),
        ),
      ],
    );
  }
}

/// One episode: art, title, date and length, and the three things you can do
/// to it — play, save offline, queue.
class EpisodeRow extends StatelessWidget {
  const EpisodeRow({
    super.key,
    required this.controller,
    required this.episode,
  });

  final MusicController controller;
  final Episode episode;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final e = episode;
    final playing =
        controller.now?.mode == 'podcast' && controller.now?.key == '${e.id}';
    final saved = e.downloaded.isNotEmpty;
    return InkWell(
      onTap: () => controller.send(MusicCmd.podPlay(episodeId: e.id)),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 10),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            MusicArt(
              controller: controller,
              kind: 'podcast',
              artKey: e.art,
              size: 56,
              radius: 8,
              fallback: Icons.podcasts,
            ),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    e.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      fontSize: 13,
                      fontWeight: playing ? FontWeight.w700 : FontWeight.w600,
                      color: playing ? Tokens.secMusic : t.nInk,
                    ),
                  ),
                  Text(
                    [
                      e.show_,
                      fmtDate(e.published),
                      if (e.durationS > 0) fmtClock(e.durationS),
                      if (saved) 'saved',
                    ].where((s) => s.isNotEmpty).join(' · '),
                    style: TextStyle(fontSize: 11, color: t.nInk2),
                  ),
                  if (e.description.isNotEmpty)
                    Padding(
                      padding: const EdgeInsets.only(top: 4),
                      child: Text(
                        e.description,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11, color: t.nInk2),
                      ),
                    ),
                  // Where you got to last time. Only drawn once there is
                  // something to draw — a bar at zero is noise on every row.
                  if (e.positionS > 0 && e.durationS > 0)
                    Padding(
                      padding: const EdgeInsets.only(top: 6),
                      child: LinearProgressIndicator(
                        value: (e.positionS / e.durationS).clamp(0.0, 1.0),
                        minHeight: 2,
                        backgroundColor: t.nHair,
                        valueColor:
                            const AlwaysStoppedAnimation(Tokens.secMusic),
                      ),
                    ),
                ],
              ),
            ),
            IconButton(
              iconSize: 18,
              tooltip: 'Add to the episode queue',
              icon: const Icon(Icons.playlist_add),
              onPressed: () =>
                  controller.send(MusicCmd.podQueueAdd(episodeId: e.id)),
            ),
            IconButton(
              iconSize: 18,
              tooltip: saved ? 'Delete the saved copy' : 'Save for offline',
              icon:
                  Icon(saved ? Icons.delete_outline : Icons.download_outlined),
              onPressed: () => controller.send(
                saved
                    ? MusicCmd.podRemoveDownload(episodeId: e.id)
                    : MusicCmd.podDownload(episodeId: e.id),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
