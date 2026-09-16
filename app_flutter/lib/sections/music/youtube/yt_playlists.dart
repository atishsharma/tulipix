// YouTube › Playlists: the saved playlists as cards, and one playlist as pages
// of video cards, eighteen at a time.
//
// A playlist here is local: it holds video ids, so it survives the cache being
// cleared. The Offline badge says which of its videos would still play
// without the network.

import 'dart:math' show Random;

import 'package:flutter/material.dart';

import '../../../design/clock.dart';
import '../../../design/tokens.dart';
import '../../../src/rust/api/music.dart';
import '../music_controller.dart';
import '../music_dialogs.dart';
import '../music_widgets.dart';
import 'yt_card.dart';
import 'yt_dialogs.dart';
import 'yt_format_sheet.dart';

class YtPlaylists extends StatelessWidget {
  const YtPlaylists({
    super.key,
    required this.controller,
    required this.st,
    required this.page,
  });

  final MusicController controller;
  final MusicState st;

  /// Paged from the tab's header, which also holds New and Import.
  final int page;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    if (st.ytPlaylists.isEmpty) {
      return MusicEmpty(
        icon: Icons.playlist_play,
        title: 'No playlists',
        body: 'Make one, or import a YouTube playlist from its link. '
            'Add videos from any card\'s menu.',
        action: ('New playlist', () => newYtPlaylist(context, c)),
      );
    }
    return ListView(
      padding: const EdgeInsets.only(top: 12, bottom: 32),
      children: [
        YtGrid(children: [
          for (final p in st.ytPlaylists.skip(page * ytPerPage).take(ytPerPage))
            YtPlaylistCard(
              controller: c,
              playlist: p,
              playing: st.ytPlayingPlId == p.id,
            ),
        ]),
      ],
    );
  }
}

/// The Playlists tab's buttons, for the tab header.
class YtPlaylistsActions extends StatelessWidget {
  const YtPlaylistsActions({super.key, required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) => Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          FilledButton.icon(
            style: musicFilledStyle(fill: ytRose),
            icon: const Icon(Icons.add, size: 16),
            label: const Text('New playlist'),
            onPressed: () => newYtPlaylist(context, controller),
          ),
          const SizedBox(width: 6),
          TextButton.icon(
            style: musicQuietStyle(context),
            icon: const Icon(Icons.link, size: 16),
            label: const Text('Import from link'),
            onPressed: () => importYtPlaylist(context, controller),
          ),
        ],
      );
}

class YtPlaylistPage extends StatefulWidget {
  const YtPlaylistPage({super.key, required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  State<YtPlaylistPage> createState() => _YtPlaylistPageState();
}

class _YtPlaylistPageState extends State<YtPlaylistPage> {
  int _page = 0;

  MusicState get st => widget.st;
  MusicController get c => widget.controller;

  @override
  void didUpdateWidget(YtPlaylistPage old) {
    super.didUpdateWidget(old);
    // Another playlist, or another order: back to its first page.
    if (old.st.ytPlaylistId != st.ytPlaylistId ||
        old.st.ytPlaylistSort != st.ytPlaylistSort) {
      _page = 0;
    }
  }

  Future<void> _delete(BuildContext context) async {
    final ok = await confirm(
      context,
      title: 'Delete “${st.ytPlaylistTitle}”?',
      body: 'The playlist goes; the videos, and any copies on disk, stay.',
      action: 'Delete',
    );
    if (!ok) return;
    await c.send(MusicCmd.ytDeletePlaylist(playlistId: st.ytPlaylistId));
    await c.send(const MusicCmd.ytPlaylistBack());
  }

  /// The YouTube playlist this one was imported from; "" for one made here.
  String get _sourceUrl =>
      st.ytPlaylists
          .where((p) => p.id == st.ytPlaylistId)
          .firstOrNull
          ?.sourceUrl ??
      '';

  /// Move a video one place in the playlist's own order.
  void _move(List<YtVideo> videos, int i, int by) {
    final to = i + by;
    if (to < 0 || to >= videos.length) return;
    // `before` names the video it lands in front of; "" is the end.
    final rest = [...videos]..removeAt(i);
    c.send(MusicCmd.ytMovePlaylistItem(
      playlistId: st.ytPlaylistId,
      videoId: videos[i].videoId,
      before: to < rest.length ? rest[to].videoId : '',
    ));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final videos = st.ytPlaylistVideos;
    final total = videos.fold<int>(0, (n, v) => n + v.duration.toInt());
    final downloaded = videos.where(ytDownloaded).length;
    final cached =
        videos.where((v) => v.offline.isNotEmpty && !ytDownloaded(v)).length;
    final pages = (videos.length / ytPerPage).ceil().clamp(1, 1 << 30);
    final page = _page.clamp(0, pages - 1);
    final from = page * ytPerPage;
    final shown = videos.skip(from).take(ytPerPage).toList();
    final (sortKey, desc) = st.ytPlaylistSort.endsWith(':desc')
        ? (st.ytPlaylistSort.substring(0, st.ytPlaylistSort.length - 5), true)
        : (st.ytPlaylistSort, false);
    // Moving only means something in the playlist's own order.
    final canMove = st.ytPlaylistSort == 'default';

    return ListView(
      padding: const EdgeInsets.only(bottom: 32),
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 12, 24, 16),
          child: Row(
            children: [
              IconButton(
                icon: const Icon(Icons.arrow_back),
                tooltip: 'Back',
                onPressed: () => c.send(const MusicCmd.ytPlaylistBack()),
              ),
              const SizedBox(width: 6),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(st.ytPlaylistTitle,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 20,
                            fontWeight: FontWeight.w700,
                            letterSpacing: -0.4,
                            color: t.nInk)),
                    Text(
                      [
                        '${videos.length} videos',
                        if (total > 0) fmtClock(total.toDouble()),
                        if (downloaded > 0) '$downloaded downloaded',
                        if (cached > 0) '$cached cached',
                      ].join('  ·  '),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 12, color: t.nInk3),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 12),
              // Pinned right: Expanded makes the Wrap span its share, `end`
              // pushes the buttons over, and a narrow window wraps them.
              Expanded(
                flex: 2,
                child: Wrap(
                  alignment: WrapAlignment.end,
                  spacing: 8,
                  runSpacing: 8,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: [
                    FilledButton.icon(
                      style: musicFilledStyle(fill: ytRose),
                      icon: const Icon(Icons.play_arrow_rounded, size: 18),
                      label: const Text('Play'),
                      onPressed: videos.isEmpty
                          ? null
                          : () => c.send(MusicCmd.ytPlaylistPlayAll(
                              playlistId: st.ytPlaylistId)),
                    ),
                    TextButton.icon(
                      style: musicQuietStyle(context),
                      icon: const Icon(Icons.shuffle, size: 16),
                      label: const Text('Shuffle'),
                      onPressed: videos.isEmpty
                          ? null
                          : () => c.send(MusicCmd.ytPlayAll(
                              videoIds: [for (final v in videos) v.videoId]
                                ..shuffle(Random()))),
                    ),
                    FilledButton.icon(
                      style: ytDownloadStyle(),
                      icon: const Icon(Icons.download_rounded, size: 16),
                      label: const Text('Download all'),
                      onPressed: videos.isEmpty
                          ? null
                          : () => showFormatSheet(context, c, videos.first,
                              mode: FormatMode.download, batch: videos),
                    ),
                    // Tap a sort to use it; tap it again to turn it around.
                    for (final m in const [
                      ('default', 'Playlist order'),
                      ('title', 'Title'),
                      ('duration', 'Length'),
                    ])
                      SortChip(
                        label: m.$2,
                        active: sortKey == m.$1,
                        dir: sortKey == m.$1 ? (desc ? 2 : 1) : 0,
                        onTap: () => c.send(MusicCmd.ytSetPlaylistSort(
                            mode: sortKey == m.$1 && !desc
                                ? '${m.$1}:desc'
                                : m.$1)),
                      ),
                    if (_sourceUrl.isNotEmpty)
                      IconButton(
                        tooltip: 'Get new videos from YouTube',
                        icon: const Icon(Icons.sync, size: 18),
                        onPressed: () => c.send(MusicCmd.ytSyncPlaylist(
                            playlistId: st.ytPlaylistId)),
                      ),
                    IconButton(
                      tooltip: 'Rename',
                      icon: const Icon(Icons.edit_outlined, size: 18),
                      onPressed: () => renameYtPlaylist(context, c,
                          st.ytPlaylistId.toInt(), st.ytPlaylistTitle),
                    ),
                    IconButton(
                      tooltip: 'Delete playlist',
                      icon: const Icon(Icons.delete_outline, size: 18),
                      onPressed: () => _delete(context),
                    ),
                  ],
                ),
              ),
              if (pages > 1)
                Padding(
                  padding: const EdgeInsets.only(left: 12),
                  child: Pager(
                    page: page,
                    pages: pages,
                    compact: true,
                    onGo: (p) => setState(() => _page = p),
                  ),
                ),
            ],
          ),
        ),
        if (videos.isEmpty)
          const SizedBox(
            height: 280,
            child: MusicEmpty(
              icon: Icons.playlist_remove,
              title: 'Empty playlist',
              body: 'Add videos to it from the menu on any card.',
            ),
          )
        else
          YtGrid(children: [
            for (final (k, v) in shown.indexed)
              YtVideoCard(
                key: ValueKey('pl-${v.videoId}'),
                controller: c,
                video: v,
                extraMenu: [
                  if (canMove && from + k > 0)
                    ('Move earlier', () => _move(videos, from + k, -1)),
                  if (canMove && from + k < videos.length - 1)
                    ('Move later', () => _move(videos, from + k, 1)),
                  (
                    'Remove from playlist',
                    () => c.send(MusicCmd.ytRemoveFromPlaylist(
                          playlistId: st.ytPlaylistId,
                          videoId: v.videoId,
                        ))
                  ),
                ],
              ),
          ]),
      ],
    );
  }
}
