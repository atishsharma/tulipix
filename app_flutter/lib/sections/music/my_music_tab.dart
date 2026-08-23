// My Music — the first and largest of the five tabs.
//
// Nine sub-tabs: Home's rails and listening figures, the paged songs list,
// four browse grids (albums, artists, genres, playlists), the watched-folder
// list, favourites and history. Opening any card lands on the detail page,
// which is one file over.

import 'package:flutter/material.dart';

import '../../design/pick.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'detail_page.dart';
import 'downloader_tab.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';
import 'music_widgets.dart';

class MyMusicTab extends StatelessWidget {
  const MyMusicTab({super.key, required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    if (st == null) return const Center(child: CircularProgressIndicator());
    if (st.detailOpen) return DetailPage(controller: controller);

    return Column(
      children: [
        _SubTabs(controller: controller),
        Expanded(child: _body(context, st)),
      ],
    );
  }

  Widget _body(BuildContext context, MusicState st) {
    // An empty library is the one state that beats every sub-tab: none of them
    // can show anything, and the fix is the same from all nine.
    if (st.trackCount == 0 && st.roots.isEmpty) {
      return MusicEmpty(
        icon: Icons.library_music_outlined,
        title: 'No music yet',
        body: 'Add a folder of audio files and Tulipix will read their tags, '
            'group them into albums and artists, and keep watching it.',
        action: ('Add a folder', () => _addFolder(context, controller)),
      );
    }
    return switch (st.libTab) {
      'home' => _Home(controller: controller, st: st),
      'downloader' => const DownloaderTab(),
      'songs' || 'favorites' || 'history' => _SongsList(
          controller: controller,
          st: st,
          paged: st.libTab != 'history',
        ),
      'folders' => _Folders(controller: controller, st: st),
      _ => _Browse(controller: controller, st: st),
    };
  }
}

Future<void> _addFolder(BuildContext context, MusicController c) async {
  final path = await pickDirectory();
  if (path == null) return;
  await c.send(MusicCmd.addFolder(path: path));
}

class _SubTabs extends StatelessWidget {
  const _SubTabs({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final active = controller.libTab;
    return SizedBox(
      height: 52,
      child: Row(
        children: [
          Expanded(
            child: ListView(
              scrollDirection: Axis.horizontal,
              padding: const EdgeInsets.symmetric(horizontal: 20),
              children: [
                for (final tab in libTabs)
                  Padding(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 4, vertical: 9),
                    child: MusicChip(
                      label: tab.label,
                      icon: tab.icon,
                      active: active == tab.id,
                      onTap: () =>
                          controller.send(MusicCmd.setLibTab(name: tab.id)),
                    ),
                  ),
              ],
            ),
          ),
          if (active == 'songs') _SongSort(controller: controller),
          IconButton(
            tooltip: 'Rescan watched folders',
            icon: const Icon(Icons.refresh),
            onPressed: () => controller.send(const MusicCmd.scan()),
          ),
          IconButton(
            // Separate from the scan: the walk is cheap and the ffprobe pass
            // over an untagged library is minutes.
            tooltip: 'Read tags for anything untagged',
            icon: const Icon(Icons.label_outline),
            onPressed: () => controller.send(const MusicCmd.readTags()),
          ),
          IconButton(
            tooltip: 'Audio settings',
            icon: const Icon(Icons.tune),
            onPressed: () => audioSettings(context, controller),
          ),
          IconButton(
            tooltip: 'Add a folder',
            icon: const Icon(Icons.create_new_folder_outlined),
            onPressed: () => _addFolder(context, controller),
          ),
          const SizedBox(width: 12),
          Container(width: 1, height: 24, color: t.outline),
          const SizedBox(width: 12),
        ],
      ),
    );
  }
}

class _SongSort extends StatelessWidget {
  const _SongSort({required this.controller});

  final MusicController controller;

  static const Map<String, String> _modes = {
    'added': 'Date added',
    'title': 'Title',
    'artist': 'Artist',
    'album': 'Album',
    'plays': 'Play count',
    'duration': 'Length',
  };

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    final mode = st?.songSort ?? 'added';
    final dir = st?.songDir ?? 'desc';
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        PopupMenuButton<String>(
          tooltip: 'Sort',
          onSelected: (v) => controller.send(
            // Picking the mode that is already active flips the direction,
            // which is what a sort header does everywhere else.
            v == mode
                ? MusicCmd.setSongSort(
                    mode: mode, dir: dir == 'desc' ? 'asc' : 'desc')
                : MusicCmd.setSongSort(mode: v, dir: dir),
          ),
          itemBuilder: (_) => [
            for (final e in _modes.entries)
              PopupMenuItem(
                value: e.key,
                child: Row(
                  children: [
                    if (e.key == mode)
                      Icon(
                          dir == 'desc'
                              ? Icons.arrow_downward
                              : Icons.arrow_upward,
                          size: 14)
                    else
                      const SizedBox(width: 14),
                    const SizedBox(width: 8),
                    Text(e.value),
                  ],
                ),
              ),
          ],
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                const Icon(Icons.sort, size: 16),
                const SizedBox(width: 6),
                Text(_modes[mode] ?? mode,
                    style: const TextStyle(fontSize: 12)),
              ],
            ),
          ),
        ),
      ],
    );
  }
}

class _Home extends StatelessWidget {
  const _Home({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    return ListView(
      children: [
        _StatsStrip(stats: st.stats, trackCount: st.trackCount),
        _rail('Recently played', st.railRecent, 'recent'),
        _rail('Most played', st.railMost, 'most'),
        _rail('Favourites', st.railLoved, 'loved'),
        _rail('New this week', st.railFresh, 'fresh'),
        if (st.railAlbums.isNotEmpty)
          Rail(
            title: 'Albums',
            height: 210,
            action: (
              'See all',
              () => controller.send(const MusicCmd.setLibTab(name: 'albums'))
            ),
            children: [
              for (final a in st.railAlbums)
                MusicCard(
                  controller: controller,
                  title: a.title,
                  subtitle: a.subtitle,
                  artKind: 'album',
                  artKey: a.key,
                  direct: a.art,
                  onTap: () =>
                      controller.send(MusicCmd.openAlbum(albumId: a.id)),
                ),
            ],
          ),
        if (st.railArtists.isNotEmpty)
          Rail(
            title: 'Artists',
            height: 210,
            action: (
              'See all',
              () => controller.send(const MusicCmd.setLibTab(name: 'artists'))
            ),
            children: [
              for (final a in st.railArtists)
                MusicCard(
                  controller: controller,
                  title: a.title,
                  subtitle: a.subtitle,
                  artKind: 'artist',
                  artKey: a.key,
                  direct: a.art,
                  round: true,
                  fallback: Icons.person,
                  onTap: () =>
                      controller.send(MusicCmd.openArtist(artistId: a.id)),
                ),
            ],
          ),
        const SizedBox(height: 24),
      ],
    );
  }

  Widget _rail(String title, List<Track> tracks, String source) => Rail(
        title: title,
        height: 190,
        children: [
          for (var i = 0; i < tracks.length; i++)
            MusicCard(
              controller: controller,
              title: tracks[i].title,
              subtitle: tracks[i].artist,
              artKind: 'track',
              artKey: '${tracks[i].itemId}',
              direct: tracks[i].art,
              fallback: Icons.music_note,
              onTap: () => controller.playFrom(tracks, i, source),
              onPlay: () => controller.playFrom(tracks, i, source),
            ),
        ],
      );
}

class _StatsStrip extends StatelessWidget {
  const _StatsStrip({required this.stats, required this.trackCount});

  final Stats stats;
  final int trackCount;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget cell(String label, String value) => Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(value,
                  style: TextStyle(
                      fontSize: 22,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
              Text(label, style: TextStyle(fontSize: 11, color: t.nInk2)),
            ],
          ),
        );
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 4),
      child: Container(
        padding: const EdgeInsets.all(20),
        decoration: BoxDecoration(
          color: t.nCard,
          borderRadius: BorderRadius.circular(Tokens.radiusMd),
          border: Border.all(color: t.nHair),
        ),
        child: Row(
          children: [
            cell('tracks', '$trackCount'),
            cell('this week', stats.week),
            cell('all time', stats.total),
            cell('top genre', stats.genre),
            cell('streak', stats.streak),
          ],
        ),
      ),
    );
  }
}

class _SongsList extends StatelessWidget {
  const _SongsList({
    required this.controller,
    required this.st,
    required this.paged,
  });

  final MusicController controller;
  final MusicState st;
  final bool paged;

  @override
  Widget build(BuildContext context) {
    final songs = st.songs;
    if (songs.isEmpty) {
      return MusicEmpty(
        icon: st.libTab == 'favorites'
            ? Icons.favorite_border
            : st.libTab == 'history'
                ? Icons.history
                : Icons.music_note_outlined,
        title: st.query.isNotEmpty
            ? 'Nothing matches “${st.query}”'
            : st.libTab == 'favorites'
                ? 'No favourites yet'
                : st.libTab == 'history'
                    ? 'Nothing played yet'
                    : 'No songs',
        body: st.query.isNotEmpty
            ? 'Try a shorter search, or clear it to see the whole library.'
            : 'Tracks show here once a watched folder has been scanned.',
        action: st.query.isNotEmpty
            ? (
                'Clear search',
                () => controller.send(const MusicCmd.search(query: ''))
              )
            : null,
      );
    }
    return Column(
      children: [
        _ListActions(
            controller: controller, tracks: songs, total: st.songTotal),
        Expanded(
          child: ListView.builder(
            itemCount: songs.length,
            itemBuilder: (_, i) => TrackRow(
              controller: controller,
              track: songs[i],
              index: i + (paged ? st.songPage * 100 : 0),
              onPlay: () => controller.playFrom(songs, i, st.libTab),
              onQueue: () => controller.queueAll([songs[i]]),
            ),
          ),
        ),
        if (paged)
          Pager(
            page: st.songPage,
            pages: st.songPages,
            onGo: (p) => controller.send(MusicCmd.setSongPage(page: p)),
          ),
        if (st.libTab == 'history')
          Padding(
            padding: const EdgeInsets.only(bottom: 12),
            child: TextButton.icon(
              icon: const Icon(Icons.delete_outline, size: 16),
              label: const Text('Clear history'),
              onPressed: () => controller.send(const MusicCmd.clearHistory()),
            ),
          ),
      ],
    );
  }
}

class _ListActions extends StatelessWidget {
  const _ListActions({
    required this.controller,
    required this.tracks,
    required this.total,
  });

  final MusicController controller;
  final List<Track> tracks;
  final int total;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 4, 16, 8),
      child: Row(
        children: [
          FilledButton.icon(
            icon: const Icon(Icons.play_arrow, size: 18),
            label: const Text('Play all'),
            onPressed: () => controller.playFrom(tracks, 0, 'list'),
          ),
          const SizedBox(width: 8),
          OutlinedButton.icon(
            icon: const Icon(Icons.shuffle, size: 18),
            label: const Text('Shuffle'),
            onPressed: () async {
              final st = controller.state;
              if (st != null && !st.shuffle) {
                await controller.send(const MusicCmd.toggleShuffle());
              }
              await controller.playFrom(tracks, 0, 'list');
            },
          ),
          const SizedBox(width: 8),
          OutlinedButton.icon(
            icon: const Icon(Icons.queue_music, size: 18),
            label: const Text('Queue'),
            onPressed: () => controller.queueAll(tracks),
          ),
          const Spacer(),
          Text('$total tracks', style: TextStyle(fontSize: 12, color: t.nInk2)),
        ],
      ),
    );
  }
}

class _Browse extends StatelessWidget {
  const _Browse({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    if (st.cards.isEmpty) {
      return MusicEmpty(
        icon: Icons.grid_view_outlined,
        title: 'Nothing here',
        body: st.libTab == 'playlists'
            ? 'Create a playlist, or let Tulipix build a smart one from what '
                'you have loved and played.'
            : 'This grid fills in once tracks have been tagged.',
        action: st.libTab == 'playlists'
            ? ('New playlist', () => _newPlaylist(context, controller))
            : null,
      );
    }
    return Column(
      children: [
        _BrowseActions(controller: controller, st: st),
        Expanded(
          child: CardGrid(
            children: [
              for (final c in st.cards) _card(c),
            ],
          ),
        ),
        Pager(
          page: st.browsePage,
          pages: st.browsePages,
          onGo: (p) => controller.send(MusicCmd.setBrowsePage(page: p)),
        ),
      ],
    );
  }

  Widget _card(BrowseCard c) => switch (st.libTab) {
        'artists' => MusicCard(
            controller: controller,
            title: c.title,
            subtitle: c.subtitle,
            artKind: 'artist',
            artKey: c.key,
            direct: c.art,
            round: true,
            fallback: Icons.person,
            onTap: () => controller.send(MusicCmd.openArtist(artistId: c.id)),
          ),
        'genres' => MusicCard(
            controller: controller,
            title: c.title,
            subtitle: c.subtitle,
            artKind: 'genre',
            artKey: c.key,
            fallback: Icons.category_outlined,
            onTap: () => controller.send(MusicCmd.openGenre(name: c.key)),
          ),
        'playlists' => MusicCard(
            controller: controller,
            title: c.title,
            subtitle: c.subtitle,
            artKind: 'playlist',
            artKey: c.key,
            fallback: Icons.queue_music_outlined,
            onTap: () =>
                controller.send(MusicCmd.openPlaylist(playlistId: c.id)),
            onMenu: () =>
                controller.send(MusicCmd.playlistDelete(playlistId: c.id)),
          ),
        _ => MusicCard(
            controller: controller,
            title: c.title,
            subtitle: c.subtitle,
            artKind: 'album',
            artKey: c.key,
            direct: c.art,
            onTap: () => controller.send(MusicCmd.openAlbum(albumId: c.id)),
          ),
      };
}

class _BrowseActions extends StatelessWidget {
  const _BrowseActions({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 8, 24, 0),
      child: Row(
        children: [
          Text('${st.cardTotal}',
              style: TextStyle(
                  fontSize: 13, fontWeight: FontWeight.w600, color: t.nInk)),
          const SizedBox(width: 6),
          Text(st.libTab, style: TextStyle(fontSize: 13, color: t.nInk2)),
          const Spacer(),
          if (st.libTab == 'playlists') ...[
            TextButton.icon(
              icon: const Icon(Icons.auto_awesome, size: 16),
              label: const Text('Smart: Loved'),
              onPressed: () => controller
                  .send(const MusicCmd.playlistCreateSmart(kind: 'loved')),
            ),
            TextButton.icon(
              icon: const Icon(Icons.auto_awesome, size: 16),
              label: const Text('Smart: Recent'),
              onPressed: () => controller
                  .send(const MusicCmd.playlistCreateSmart(kind: 'recent')),
            ),
            FilledButton.icon(
              icon: const Icon(Icons.add, size: 16),
              label: const Text('New'),
              onPressed: () => _newPlaylist(context, controller),
            ),
          ] else
            _SortToggle(controller: controller, st: st),
        ],
      ),
    );
  }
}

class _SortToggle extends StatelessWidget {
  const _SortToggle({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    void set(String mode) => controller.send(
          st.browseSort == mode
              ? MusicCmd.setBrowseSort(
                  mode: mode, dir: st.browseDir == 'asc' ? 'desc' : 'asc')
              : MusicCmd.setBrowseSort(mode: mode, dir: st.browseDir),
        );
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        MusicChip(
          label: 'Name',
          active: st.browseSort == 'name',
          onTap: () => set('name'),
        ),
        const SizedBox(width: 6),
        MusicChip(
          label: 'Count',
          active: st.browseSort == 'count',
          onTap: () => set('count'),
        ),
        IconButton(
          iconSize: 16,
          icon: Icon(st.browseDir == 'asc'
              ? Icons.arrow_upward
              : Icons.arrow_downward),
          onPressed: () => set(st.browseSort),
        ),
      ],
    );
  }
}

Future<void> _newPlaylist(BuildContext context, MusicController c) async {
  final text = TextEditingController();
  final name = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('New playlist'),
      content: TextField(
        controller: text,
        autofocus: true,
        decoration: const InputDecoration(labelText: 'Name'),
        onSubmitted: (v) => Navigator.pop(ctx, v),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx), child: const Text('Cancel')),
        FilledButton(
          onPressed: () => Navigator.pop(ctx, text.text),
          child: const Text('Create'),
        ),
      ],
    ),
  );
  final trimmed = name?.trim() ?? '';
  if (trimmed.isNotEmpty) {
    await c.send(MusicCmd.playlistCreate(name: trimmed));
  }
}

class _Folders extends StatelessWidget {
  const _Folders({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.all(24),
      children: [
        Text('Watched folders',
            style: TextStyle(
                fontSize: 15, fontWeight: FontWeight.w700, color: t.nInk)),
        const SizedBox(height: 4),
        Text(
          'Tulipix reads these on every scan. Removing one leaves the files '
          'alone — only the watch goes away.',
          style: TextStyle(fontSize: 12, color: t.nInk2),
        ),
        const SizedBox(height: 12),
        for (final root in st.roots)
          Container(
            margin: const EdgeInsets.only(bottom: 8),
            decoration: BoxDecoration(
              color: t.nCard,
              borderRadius: BorderRadius.circular(Tokens.radiusSm),
              border: Border.all(color: t.nHair),
            ),
            child: ListTile(
              leading: const Icon(Icons.folder_outlined),
              title: Text(root.title),
              subtitle: Text(root.subtitle,
                  maxLines: 1, overflow: TextOverflow.ellipsis),
              trailing: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text('${root.count} tracks',
                      style: TextStyle(fontSize: 12, color: t.nInk2)),
                  IconButton(
                    tooltip: 'Stop watching',
                    icon: const Icon(Icons.close, size: 18),
                    onPressed: () =>
                        controller.send(MusicCmd.removeRoot(path: root.key)),
                  ),
                ],
              ),
            ),
          ),
        const SizedBox(height: 24),
        Text('Folders with music in them',
            style: TextStyle(
                fontSize: 15, fontWeight: FontWeight.w700, color: t.nInk)),
        const SizedBox(height: 12),
        for (final f in st.cards)
          ListTile(
            leading: const Icon(Icons.folder, size: 20),
            title: Text(f.title),
            subtitle:
                Text(f.subtitle, maxLines: 1, overflow: TextOverflow.ellipsis),
            trailing: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text('${f.count}',
                    style: TextStyle(fontSize: 12, color: t.nInk2)),
                PopupMenuButton<String>(
                  iconSize: 18,
                  tooltip: 'More',
                  onSelected: (v) => controller.send(
                    MusicCmd.bookFlagFolder(folder: f.key, on_: v == 'book'),
                  ),
                  itemBuilder: (_) => const [
                    // Flagging is a column update, not a move: unflagging puts
                    // the tracks straight back into My Music.
                    PopupMenuItem(
                        value: 'book', child: Text('Mark as an audiobook')),
                    PopupMenuItem(
                        value: 'music', child: Text('Back to My Music')),
                  ],
                ),
              ],
            ),
            onTap: () => controller.send(MusicCmd.openFolder(path: f.key)),
          ),
      ],
    );
  }
}
