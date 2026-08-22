// The album / artist / genre / playlist / folder page.
//
// One widget for five kinds, because they are the same page: a header with
// art and a title, a Play-all and a Shuffle, and the tracks. The artist is the
// only one that is two lists rather than one — its albums sit above its
// tracks — and that is the only branch here.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_widgets.dart';

class DetailPage extends StatelessWidget {
  const DetailPage({super.key, required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    if (st == null) return const SizedBox.shrink();
    final t = context.tokens;
    final tracks = st.detailTracks;

    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
          child: Row(
            children: [
              IconButton(
                icon: const Icon(Icons.arrow_back),
                tooltip: 'Back',
                onPressed: () => controller.send(const MusicCmd.closeDetail()),
              ),
              const SizedBox(width: 8),
              Text(
                st.detailKind[0].toUpperCase() + st.detailKind.substring(1),
                style: TextStyle(fontSize: 12, color: t.nInk2),
              ),
            ],
          ),
        ),
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 8, 24, 16),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              MusicArt(
                controller: controller,
                kind: st.detailKind,
                artKey: st.detailKind == 'album' || st.detailKind == 'artist'
                    ? '${st.detailId}'
                    : st.detailKey,
                direct: st.detailArt,
                size: 148,
                radius: st.detailKind == 'artist' ? 74 : 12,
                fallback: switch (st.detailKind) {
                  'artist' => Icons.person,
                  'playlist' => Icons.queue_music_outlined,
                  'folder' => Icons.folder,
                  'genre' => Icons.category_outlined,
                  _ => Icons.album_outlined,
                },
              ),
              const SizedBox(width: 20),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      st.detailTitle,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 26,
                          fontWeight: FontWeight.w700,
                          color: t.nInk),
                    ),
                    const SizedBox(height: 4),
                    Text(
                      st.detailSubtitle,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 13, color: t.nInk2),
                    ),
                    const SizedBox(height: 14),
                    Row(
                      children: [
                        FilledButton.icon(
                          icon: const Icon(Icons.play_arrow, size: 18),
                          label: const Text('Play'),
                          onPressed: tracks.isEmpty
                              ? null
                              : () =>
                                  controller.playFrom(tracks, 0, st.detailKind),
                        ),
                        const SizedBox(width: 8),
                        OutlinedButton.icon(
                          icon: const Icon(Icons.shuffle, size: 18),
                          label: const Text('Shuffle'),
                          onPressed: tracks.isEmpty
                              ? null
                              : () async {
                                  if (!st.shuffle) {
                                    await controller
                                        .send(const MusicCmd.toggleShuffle());
                                  }
                                  await controller.playFrom(
                                      tracks, 0, st.detailKind);
                                },
                        ),
                        const SizedBox(width: 8),
                        OutlinedButton.icon(
                          icon: const Icon(Icons.queue_music, size: 18),
                          label: const Text('Queue'),
                          onPressed: tracks.isEmpty
                              ? null
                              : () => controller.queueAll(tracks),
                        ),
                        if (st.detailKind == 'playlist') ...[
                          const SizedBox(width: 8),
                          OutlinedButton.icon(
                            icon: const Icon(Icons.delete_outline, size: 18),
                            label: const Text('Delete'),
                            onPressed: () => controller.send(
                              MusicCmd.playlistDelete(playlistId: st.detailId),
                            ),
                          ),
                        ],
                      ],
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
        Expanded(
          child: tracks.isEmpty && st.detailAlbums.isEmpty
              ? const MusicEmpty(
                  icon: Icons.music_off_outlined,
                  title: 'Nothing in here',
                  body: 'Every track this belonged to has been removed or is '
                      'missing from disk.',
                )
              : ListView(
                  children: [
                    if (st.detailAlbums.isNotEmpty) ...[
                      Rail(
                        title: 'Albums',
                        height: 210,
                        children: [
                          for (final a in st.detailAlbums)
                            MusicCard(
                              controller: controller,
                              title: a.title,
                              subtitle: a.subtitle,
                              artKind: 'album',
                              artKey: a.key,
                              direct: a.art,
                              onTap: () => controller
                                  .send(MusicCmd.openAlbum(albumId: a.id)),
                            ),
                        ],
                      ),
                      const SizedBox(height: 12),
                    ],
                    // A playlist has an order the user chose, so its rows are
                    // draggable; an album's order is the album's and dragging
                    // it would mean nothing.
                    if (st.detailKind == 'playlist')
                      ReorderableListView.builder(
                        shrinkWrap: true,
                        physics: const NeverScrollableScrollPhysics(),
                        itemCount: tracks.length,
                        // onReorderItem, unlike the deprecated onReorder,
                        // already accounts for the lifted row, so `to` is the
                        // destination the store should store.
                        onReorderItem: (from, to) =>
                            controller.send(MusicCmd.playlistMove(
                          playlistId: st.detailId,
                          from: from,
                          to: to,
                        )),
                        itemBuilder: (_, i) => TrackRow(
                          key: ValueKey(tracks[i].itemId),
                          controller: controller,
                          track: tracks[i],
                          index: i,
                          onPlay: () =>
                              controller.playFrom(tracks, i, st.detailKind),
                          onQueue: () => controller.queueAll([tracks[i]]),
                          onRemove: () =>
                              controller.send(MusicCmd.playlistRemove(
                            playlistId: st.detailId,
                            itemId: tracks[i].itemId,
                          )),
                        ),
                      )
                    else
                      for (var i = 0; i < tracks.length; i++)
                        TrackRow(
                          controller: controller,
                          track: tracks[i],
                          index: i,
                          // An album's own art is the header's; repeating it on
                          // every row is twelve copies of one picture.
                          showArt: st.detailKind != 'album',
                          onPlay: () =>
                              controller.playFrom(tracks, i, st.detailKind),
                          onQueue: () => controller.queueAll([tracks[i]]),
                        ),
                    const SizedBox(height: 24),
                  ],
                ),
        ),
      ],
    );
  }
}
