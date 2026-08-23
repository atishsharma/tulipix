// YouTube — search, subscriptions, playlists, the cache and the downloads.
//
// Audio only, and deliberately: playback lands on the same deck the other four
// tabs use, so a video here behaves like a track — it goes in the same bar,
// stops the same album, and honours the same volume. Watching video is the
// Videos section's job and it has its own player, over the window.
//
// Every listing is yt-dlp. The Piped backend the Slint build can be switched
// to is a Settings choice, and Settings belongs to the shell (phase 04).

import 'package:flutter/material.dart';

import '../../design/pick.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';
import 'music_widgets.dart';

class YoutubeTab extends StatefulWidget {
  const YoutubeTab({super.key, required this.controller});

  final MusicController controller;

  @override
  State<YoutubeTab> createState() => _YoutubeTabState();
}

class _YoutubeTabState extends State<YoutubeTab> {
  final _query = TextEditingController();

  @override
  void dispose() {
    _query.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final st = c.state;
    if (st == null) return const Center(child: CircularProgressIndicator());
    if (st.ytChannelOpen) return _ChannelPage(controller: c, st: st);
    if (st.ytPlaylistOpen) return _PlaylistPage(controller: c, st: st);

    return Column(
      children: [
        SizedBox(
          height: 52,
          child: Row(
            children: [
              const SizedBox(width: 20),
              for (final tab in const [
                ('home', 'Home'),
                ('subscriptions', 'Subscriptions'),
                ('playlists', 'Playlists'),
                ('cached', 'Cached'),
                ('downloads', 'Downloads'),
              ])
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 4),
                  child: MusicChip(
                    label: tab.$2,
                    active: st.ytTab == tab.$1,
                    tint: const Color(0xFFF43F5E),
                    tint2: const Color(0xFFF97316),
                    badge: switch (tab.$1) {
                      'subscriptions' when st.ytSubCount > 0 =>
                        '${st.ytSubCount}',
                      'downloads' when st.ytDlTotal > 0 => '${st.ytDlTotal}',
                      _ => null,
                    },
                    onTap: () => c.send(MusicCmd.ytSetTab(name: tab.$1)),
                  ),
                ),
              const SizedBox(width: 16),
              SizedBox(
                width: 300,
                child: TextField(
                  controller: _query,
                  decoration: const InputDecoration(
                    isDense: true,
                    prefixIcon: Icon(Icons.search, size: 18),
                    hintText: 'Search YouTube',
                    border: OutlineInputBorder(),
                  ),
                  onSubmitted: (q) => c.send(MusicCmd.ytSearch(query: q)),
                ),
              ),
              const Spacer(),
              if (st.ytStatus.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.only(right: 16),
                  child: Text(st.ytStatus,
                      style:
                          TextStyle(fontSize: 12, color: context.tokens.nInk2)),
                ),
            ],
          ),
        ),
        Expanded(child: _body(context, c, st)),
      ],
    );
  }

  Widget _body(BuildContext context, MusicController c, MusicState st) =>
      switch (st.ytTab) {
        'subscriptions' => _Subscriptions(controller: c, st: st),
        'playlists' => _Playlists(controller: c, st: st),
        'cached' => Column(
            children: [
              _ClearBar(
                label: 'Empty the cache',
                enabled: st.ytCached.isNotEmpty,
                onPressed: () async {
                  final ok = await confirm(
                    context,
                    title: 'Empty the cache?',
                    body: 'The cached audio files are deleted. Anything you '
                        'play again is simply pulled down again.',
                    action: 'Empty',
                  );
                  if (ok) await c.send(const MusicCmd.ytClearCached());
                },
              ),
              Expanded(
                child: _VideoList(
                  controller: c,
                  videos: st.ytCached,
                  empty: 'Nothing cached yet',
                  emptyBody: 'Anything you play is pulled down in the '
                      'background, so the second listen is local.',
                  onRemove: (v) =>
                      c.send(MusicCmd.ytRemoveCached(videoId: v.videoId)),
                ),
              ),
            ],
          ),
        'downloads' => Column(
            children: [
              _ClearBar(
                label: 'Delete all downloads',
                enabled: st.ytDownloads.isNotEmpty,
                onPressed: () async {
                  final ok = await confirm(
                    context,
                    title: 'Delete every download?',
                    body: 'These are the permanent copies, not the cache. '
                        'They go from disk.',
                    action: 'Delete all',
                  );
                  if (ok) await c.send(const MusicCmd.ytClearDownloads());
                },
              ),
              Expanded(
                child: _VideoList(
                  controller: c,
                  videos: st.ytDownloads,
                  empty: 'No downloads',
                  emptyBody: 'Download keeps a permanent copy under your data '
                      'directory; the cache above is disposable.',
                  local: true,
                  onRemove: (v) =>
                      c.send(MusicCmd.ytRemoveDownload(videoId: v.videoId)),
                ),
              ),
              Pager(
                page: st.ytDlPage,
                pages: st.ytDlPages,
                onGo: (p) => c.send(MusicCmd.ytSetDlPage(page: p)),
              ),
            ],
          ),
        _ => _Home(controller: c, st: st),
      };
}

class _Home extends StatelessWidget {
  const _Home({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (st.ytResults.isEmpty &&
        st.ytCached.isEmpty &&
        st.ytSubs.isEmpty &&
        st.ytDownloads.isEmpty) {
      return const MusicEmpty(
        icon: Icons.smart_display_outlined,
        title: 'Search for something',
        body: 'Results play as audio through the same player as the rest of '
            'the section. Needs yt-dlp on PATH or bundled in tools/.',
      );
    }
    return ListView(
      children: [
        if (st.ytRecent.isNotEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 16, 24, 0),
            child: Wrap(
              spacing: 6,
              runSpacing: 6,
              children: [
                for (final q in st.ytRecent)
                  MusicChip(
                    label: q,
                    active: false,
                    onTap: () => controller.send(MusicCmd.ytSearch(query: q)),
                  ),
                TextButton(
                  onPressed: () =>
                      controller.send(const MusicCmd.ytClearRecent()),
                  child: const Text('Clear'),
                ),
              ],
            ),
          ),
        if (st.ytResults.isNotEmpty) ...[
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 20, 24, 8),
            child: Text('Results',
                style: TextStyle(
                    fontSize: 15, fontWeight: FontWeight.w700, color: t.nInk)),
          ),
          for (final v in st.ytResults)
            VideoRow(controller: controller, video: v),
        ],
        if (st.ytCached.isNotEmpty)
          Rail(
            title: 'Cached',
            height: 200,
            action: (
              'See all',
              () => controller.send(const MusicCmd.ytSetTab(name: 'cached'))
            ),
            children: [
              for (final v in st.ytCached.take(10))
                _VideoCard(controller: controller, video: v),
            ],
          ),
        if (st.ytSubs.isNotEmpty)
          Rail(
            title: 'Channels',
            height: 190,
            action: (
              'See all',
              () => controller
                  .send(const MusicCmd.ytSetTab(name: 'subscriptions'))
            ),
            children: [
              for (final s in st.ytSubs.take(10))
                MusicCard(
                  controller: controller,
                  title: s.title,
                  subtitle: s.subs > 0 ? '${s.subs} subscribers' : '',
                  artKind: 'yt',
                  artKey: s.avatar,
                  direct: s.avatar,
                  round: true,
                  fallback: Icons.person,
                  onTap: () => controller
                      .send(MusicCmd.ytOpenChannel(channelId: s.channelId)),
                ),
            ],
          ),
        const SizedBox(height: 24),
      ],
    );
  }
}

class _VideoList extends StatelessWidget {
  const _VideoList({
    required this.controller,
    required this.videos,
    required this.empty,
    required this.emptyBody,
    this.local = false,
    this.onRemove,
  });

  final MusicController controller;
  final List<YtVideo> videos;
  final String empty;
  final String emptyBody;
  final bool local;
  final void Function(YtVideo)? onRemove;

  @override
  Widget build(BuildContext context) {
    if (videos.isEmpty) {
      return MusicEmpty(
        icon: Icons.download_outlined,
        title: empty,
        body: emptyBody,
      );
    }
    return ListView.builder(
      itemCount: videos.length,
      itemBuilder: (_, i) => VideoRow(
        controller: controller,
        video: videos[i],
        local: local,
        onRemove: onRemove == null ? null : () => onRemove!(videos[i]),
      ),
    );
  }
}

class _VideoCard extends StatelessWidget {
  const _VideoCard({required this.controller, required this.video});

  final MusicController controller;
  final YtVideo video;

  @override
  Widget build(BuildContext context) => MusicCard(
        controller: controller,
        title: video.title,
        subtitle: video.channel,
        artKind: 'yt',
        artKey: video.thumb,
        direct: video.thumb,
        fallback: Icons.smart_display,
        progress: video.progress,
        badge: video.duration > 0 ? fmtClock(video.duration.toDouble()) : null,
        onTap: () => controller.send(
          video.mediaPath.isNotEmpty
              ? MusicCmd.ytPlayLocal(
                  mediaPath: video.mediaPath, videoId: video.videoId)
              : MusicCmd.ytPlay(videoId: video.videoId),
        ),
      );
}

class VideoRow extends StatelessWidget {
  const VideoRow({
    super.key,
    required this.controller,
    required this.video,
    this.local = false,
    this.onRemove,
  });

  final MusicController controller;
  final YtVideo video;
  final bool local;
  final VoidCallback? onRemove;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final v = video;
    final playing =
        controller.now?.mode == 'youtube' && controller.now?.key == v.videoId;
    return InkWell(
      onTap: () => controller.send(
        local && v.mediaPath.isNotEmpty
            ? MusicCmd.ytPlayLocal(mediaPath: v.mediaPath, videoId: v.videoId)
            : MusicCmd.ytPlay(videoId: v.videoId),
      ),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 8),
        child: Row(
          children: [
            SizedBox(
              width: 120,
              height: 68,
              child: Stack(
                fit: StackFit.expand,
                children: [
                  MusicArt(
                    controller: controller,
                    kind: 'yt',
                    artKey: v.thumb,
                    direct: v.thumb.startsWith('http') ? null : v.thumb,
                    size: 120,
                    radius: 8,
                    fallback: Icons.smart_display,
                  ),
                  if (v.duration > 0)
                    Positioned(
                      right: 4,
                      bottom: 4,
                      child: DecoratedBox(
                        decoration: BoxDecoration(
                          color: Colors.black.withValues(alpha: 0.7),
                          borderRadius: BorderRadius.circular(4),
                        ),
                        child: Padding(
                          padding: const EdgeInsets.symmetric(
                              horizontal: 4, vertical: 1),
                          child: Text(
                            fmtClock(v.duration.toDouble()),
                            style: const TextStyle(
                                fontSize: 10, color: Colors.white),
                          ),
                        ),
                      ),
                    ),
                  if (v.progress > 0)
                    Positioned(
                      left: 0,
                      right: 0,
                      bottom: 0,
                      child: LinearProgressIndicator(
                        value: v.progress.clamp(0.0, 1.0),
                        minHeight: 3,
                        backgroundColor: Colors.black26,
                        valueColor:
                            const AlwaysStoppedAnimation(Color(0xFFF43F5E)),
                      ),
                    ),
                ],
              ),
            ),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    v.title,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      fontSize: 13,
                      fontWeight: playing ? FontWeight.w700 : FontWeight.w600,
                      color: playing ? const Color(0xFFF43F5E) : t.nInk,
                    ),
                  ),
                  Text(
                    [v.channel, v.meta].where((s) => s.isNotEmpty).join(' · '),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11, color: t.nInk2),
                  ),
                ],
              ),
            ),
            PopupMenuButton<String>(
              iconSize: 18,
              tooltip: 'More',
              onSelected: (choice) async {
                if (choice == 'remove') {
                  onRemove?.call();
                  return;
                }
                if (choice == 'playlist') {
                  await _addToYtPlaylist(context, controller, v);
                  return;
                }
                await controller.send(
                    MusicCmd.ytDownload(videoId: v.videoId, quality: choice));
              },
              itemBuilder: (_) => [
                const PopupMenuItem(
                    value: 'playlist', child: Text('Add to playlist…')),
                const PopupMenuItem(
                    value: 'audio', child: Text('Download audio (Opus)')),
                const PopupMenuItem(
                    value: '1080p', child: Text('Download 1080p')),
                const PopupMenuItem(
                    value: '720p', child: Text('Download 720p')),
                if (onRemove != null)
                  const PopupMenuItem(value: 'remove', child: Text('Remove')),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

class _Subscriptions extends StatelessWidget {
  const _Subscriptions({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    if (st.ytSubs.isEmpty) {
      return const MusicEmpty(
        icon: Icons.subscriptions_outlined,
        title: 'No channels',
        body: 'Open a channel from a search result and subscribe, or import '
            "Google Takeout's subscriptions.csv.",
      );
    }
    return Column(
      children: [
        Row(
          children: [
            const Spacer(),
            TextButton.icon(
              icon: const Icon(Icons.upload_file, size: 16),
              label: const Text('Import subscriptions'),
              onPressed: () async {
                final path = await pickFile(
                  label: "Takeout's subscriptions.csv",
                  extensions: ['csv'],
                );
                if (path != null) {
                  await controller.send(MusicCmd.ytImportSubs(path: path));
                }
              },
            ),
            TextButton.icon(
              icon: const Icon(Icons.refresh, size: 16),
              label: const Text('Refresh counts'),
              onPressed: () => controller.send(const MusicCmd.ytRefreshSubs()),
            ),
            const SizedBox(width: 12),
          ],
        ),
        Expanded(
          child: CardGrid(
            min: 160,
            children: [
              for (final s in st.ytSubs)
                MusicCard(
                  controller: controller,
                  title: s.title,
                  subtitle: [
                    if (s.subs > 0) '${s.subs} subs',
                    if (s.videos > 0) '${s.videos} videos',
                  ].join(' · '),
                  artKind: 'yt',
                  artKey: s.avatar,
                  direct: s.avatar,
                  round: true,
                  fallback: Icons.person,
                  badge: s.subscribed ? null : 'unsubbed',
                  onTap: () => controller
                      .send(MusicCmd.ytOpenChannel(channelId: s.channelId)),
                  onMenu: () =>
                      controller.send(MusicCmd.ytUnsub(channelId: s.channelId)),
                ),
            ],
          ),
        ),
        Pager(
          page: st.ytSubsPage,
          pages: st.ytSubsPages,
          onGo: (p) => controller.send(MusicCmd.ytSetSubsPage(page: p)),
        ),
      ],
    );
  }
}

class _Playlists extends StatelessWidget {
  const _Playlists({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        Row(
          children: [
            const Spacer(),
            FilledButton.icon(
              icon: const Icon(Icons.add, size: 16),
              label: const Text('New playlist'),
              onPressed: () => _newPlaylist(context, controller),
            ),
            const SizedBox(width: 12),
          ],
        ),
        Expanded(
          child: st.ytPlaylists.isEmpty
              ? MusicEmpty(
                  icon: Icons.playlist_play,
                  title: 'No playlists',
                  body: 'A YouTube playlist here is local: it holds video ids, '
                      'so it survives the cache being cleared.',
                  action: (
                    'New playlist',
                    () => _newPlaylist(context, controller)
                  ),
                )
              : CardGrid(
                  children: [
                    for (final p in st.ytPlaylists)
                      MusicCard(
                        controller: controller,
                        title: p.name,
                        subtitle: '${p.count} videos',
                        artKind: 'yt',
                        artKey: p.cover,
                        direct: p.cover,
                        fallback: Icons.playlist_play,
                        onTap: () => controller
                            .send(MusicCmd.ytOpenPlaylist(playlistId: p.id)),
                        onMenu: () => controller
                            .send(MusicCmd.ytDeletePlaylist(playlistId: p.id)),
                      ),
                  ],
                ),
        ),
      ],
    );
  }
}

/// A right-aligned destructive action above a list. Two tabs need one and
/// nothing else does, so it stays here rather than in the shared widgets.
class _ClearBar extends StatelessWidget {
  const _ClearBar({
    required this.label,
    required this.enabled,
    required this.onPressed,
  });

  final String label;
  final bool enabled;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) => Row(
        children: [
          const Spacer(),
          TextButton.icon(
            icon: const Icon(Icons.delete_sweep_outlined, size: 16),
            label: Text(label),
            onPressed: enabled ? onPressed : null,
          ),
          const SizedBox(width: 12),
        ],
      );
}

/// Pick one of the local YouTube playlists for this video. The list only sits
/// in the snapshot while that tab is open, so ask for it first.
Future<void> _addToYtPlaylist(
    BuildContext context, MusicController c, YtVideo v) async {
  // Same round trip, same restore: fetching the list must not move the user
  // off the tab they were on.
  final previous = c.state?.ytTab ?? 'home';
  await c.send(const MusicCmd.ytSetTab(name: 'playlists'));
  final playlists = c.state?.ytPlaylists ?? const <YtPlaylist>[];
  await c.send(MusicCmd.ytSetTab(name: previous));
  if (!context.mounted) return;
  if (playlists.isEmpty) {
    await _newPlaylist(context, c);
    return;
  }
  final chosen = await showDialog<int>(
    context: context,
    builder: (ctx) => SimpleDialog(
      title: const Text('Add to playlist'),
      children: [
        for (final p in playlists)
          SimpleDialogOption(
            onPressed: () => Navigator.pop(ctx, p.id),
            child: Text('${p.name}  ·  ${p.count}'),
          ),
      ],
    ),
  );
  if (chosen == null) return;
  await c
      .send(MusicCmd.ytAddToPlaylist(playlistId: chosen, videoId: v.videoId));
}

Future<void> _newPlaylist(BuildContext context, MusicController c) async {
  final text = TextEditingController();
  final name = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('New YouTube playlist'),
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
    await c.send(MusicCmd.ytCreatePlaylist(name: trimmed));
  }
}

class _ChannelPage extends StatelessWidget {
  const _ChannelPage({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
          child: Row(
            children: [
              IconButton(
                icon: const Icon(Icons.arrow_back),
                tooltip: 'Back',
                onPressed: () =>
                    controller.send(const MusicCmd.ytChannelBack()),
              ),
              const SizedBox(width: 8),
              MusicArt(
                controller: controller,
                kind: 'yt',
                artKey: st.ytChannelAvatar,
                direct: st.ytChannelAvatar,
                size: 40,
                radius: 20,
                fallback: Icons.person,
              ),
              const SizedBox(width: 12),
              Text(st.ytChannelTitle,
                  style: TextStyle(
                      fontSize: 17,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
              const SizedBox(width: 16),
              MusicChip(
                label: 'Latest',
                active: st.ytChannelMode != 'popular',
                tint: const Color(0xFFF43F5E),
                tint2: const Color(0xFFF97316),
                onTap: () => controller
                    .send(const MusicCmd.ytChannelMode(mode: 'latest')),
              ),
              const SizedBox(width: 6),
              MusicChip(
                label: 'Popular',
                active: st.ytChannelMode == 'popular',
                tint: const Color(0xFFF43F5E),
                tint2: const Color(0xFFF97316),
                onTap: () => controller
                    .send(const MusicCmd.ytChannelMode(mode: 'popular')),
              ),
              const Spacer(),
              if (st.ytChannelSubscribed)
                OutlinedButton.icon(
                  icon: const Icon(Icons.check, size: 16),
                  label: const Text('Subscribed'),
                  onPressed: () => controller
                      .send(MusicCmd.ytUnsub(channelId: st.ytChannelId)),
                )
              else
                FilledButton.icon(
                  icon: const Icon(Icons.add, size: 16),
                  label: const Text('Subscribe'),
                  onPressed: () => controller.send(MusicCmd.ytSubscribe(
                    channelId: st.ytChannelId,
                    title: st.ytChannelTitle,
                  )),
                ),
              const SizedBox(width: 12),
            ],
          ),
        ),
        Expanded(
          child: st.ytChannelVideos.isEmpty
              ? const MusicEmpty(
                  icon: Icons.videocam_off_outlined,
                  title: 'Nothing to list',
                  body: 'yt-dlp could not reach this channel, or it has no '
                      'public uploads.',
                )
              : ListView(
                  children: [
                    for (final v in st.ytChannelVideos)
                      VideoRow(controller: controller, video: v),
                  ],
                ),
        ),
      ],
    );
  }
}

class _PlaylistPage extends StatelessWidget {
  const _PlaylistPage({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
          child: Row(
            children: [
              IconButton(
                icon: const Icon(Icons.arrow_back),
                tooltip: 'Back',
                onPressed: () =>
                    controller.send(const MusicCmd.ytPlaylistBack()),
              ),
              const SizedBox(width: 8),
              Text(st.ytPlaylistTitle,
                  style: TextStyle(
                      fontSize: 17,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
              const Spacer(),
              Text('${st.ytPlaylistVideos.length} videos',
                  style: TextStyle(fontSize: 12, color: t.nInk2)),
              const SizedBox(width: 16),
            ],
          ),
        ),
        Expanded(
          child: st.ytPlaylistVideos.isEmpty
              ? const MusicEmpty(
                  icon: Icons.playlist_remove,
                  title: 'Empty playlist',
                  body: 'Add videos to it from the menu on any search result.',
                )
              : ListView(
                  children: [
                    for (final v in st.ytPlaylistVideos)
                      VideoRow(
                        controller: controller,
                        video: v,
                        onRemove: () => controller.send(
                          MusicCmd.ytRemoveFromPlaylist(
                            playlistId: st.ytPlaylistId,
                            videoId: v.videoId,
                          ),
                        ),
                      ),
                  ],
                ),
        ),
      ],
    );
  }
}
