// YouTube › Results: what a search, or a pasted video or playlist link, found.
//
// The videos come from the bridge's search (YouTube, or the library when the
// search was offline-only). Channels and playlists that match come from the
// library, the same lookup the search box's suggestions use, so a search for
// a channel you follow shows that channel first rather than burying it among
// uploads that mention it.

import 'package:flutter/material.dart';

import '../../../design/tokens.dart';
import '../../../src/rust/api/music.dart';
import '../music_controller.dart';
import '../music_widgets.dart';
import 'yt_card.dart';

class YtResults extends StatefulWidget {
  const YtResults({super.key, required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  State<YtResults> createState() => _YtResultsState();
}

enum _Filter { all, videos, channels, playlists }

class _YtResultsState extends State<YtResults> {
  _Filter _filter = _Filter.all;
  String _forQuery = '';
  Future<YtSuggestLocal>? _library;

  /// Library matches for the query on screen, asked once per query rather
  /// than on every snapshot.
  Future<YtSuggestLocal>? _libraryFor(String q, bool offline) {
    if (q.isEmpty) return null;
    if (q != _forQuery || _library == null) {
      _forQuery = q;
      _library = musicYtSuggestLocal(query: q, offlineOnly: offline);
    }
    return _library;
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = widget.controller;
    final st = widget.st;
    final q = st.ytSearchQ;
    final library = _libraryFor(q, st.ytSearchOffline);

    return FutureBuilder<YtSuggestLocal>(
      future: library,
      builder: (context, snap) {
        final channels = snap.data?.channels ?? const <YtSub>[];
        final playlists = snap.data?.playlists ?? const <YtPlaylist>[];
        bool show(_Filter f) => _filter == _Filter.all || _filter == f;
        final nothing = st.ytResults.isEmpty &&
            channels.isEmpty &&
            playlists.isEmpty &&
            !st.ytBusy;

        return ListView(
          padding: const EdgeInsets.only(bottom: 32),
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(24, 20, 24, 4),
              child: Wrap(
                spacing: 12,
                runSpacing: 10,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  Text(
                    q.isEmpty ? 'From the link' : 'Results for “$q”',
                    style: TextStyle(
                        fontSize: 20,
                        fontWeight: FontWeight.w700,
                        letterSpacing: -0.4,
                        color: t.nInk),
                  ),
                  Text(
                    [
                      '${st.ytResults.length} videos',
                      if (st.ytSearchOffline) 'offline only',
                    ].join('  ·  '),
                    style: TextStyle(fontSize: 12, color: t.nInk3),
                  ),
                ],
              ),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(24, 10, 24, 0),
              child: Wrap(
                spacing: 6,
                runSpacing: 6,
                children: [
                  for (final (f, label) in const [
                    (_Filter.all, 'All'),
                    (_Filter.videos, 'Videos'),
                    (_Filter.channels, 'Channels'),
                    (_Filter.playlists, 'Playlists'),
                  ])
                    MusicChip(
                      label: label,
                      active: _filter == f,
                      tint: ytRose,
                      tint2: const Color(0xFFF97316),
                      onTap: () => setState(() => _filter = f),
                    ),
                ],
              ),
            ),
            if (nothing)
              Padding(
                padding: const EdgeInsets.all(24),
                child: Text(
                  st.ytStatus.isNotEmpty
                      ? st.ytStatus
                      : 'Nothing matched. Try fewer words, or search everywhere '
                          'if this was an offline search.',
                  style: TextStyle(fontSize: 13, color: t.nInk2),
                ),
              ),
            if (show(_Filter.channels) && channels.isNotEmpty) ...[
              _label(context, 'Channels you follow'),
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: 24),
                child: Wrap(
                  spacing: 12,
                  runSpacing: 12,
                  children: [
                    for (final s in channels) _ChannelTile(controller: c, sub: s),
                  ],
                ),
              ),
            ],
            if (show(_Filter.playlists) && playlists.isNotEmpty) ...[
              _label(context, 'Your playlists'),
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: 24),
                child: Wrap(
                  spacing: 16,
                  runSpacing: 16,
                  children: [
                    for (final p in playlists)
                      SizedBox(
                        width: 220,
                        child: YtPlaylistCard(
                          controller: c,
                          playlist: p,
                          playing: st.ytPlayingPlId == p.id,
                        ),
                      ),
                  ],
                ),
              ),
            ],
            if (show(_Filter.videos) && st.ytResults.isNotEmpty) ...[
              if (_filter == _Filter.all &&
                  (channels.isNotEmpty || playlists.isNotEmpty))
                _label(context, 'Videos'),
              const SizedBox(height: 12),
              YtVideoGrid(controller: c, videos: st.ytResults),
              if (st.ytResultsMore)
                Padding(
                  padding: const EdgeInsets.fromLTRB(24, 16, 24, 0),
                  child: Align(
                    alignment: Alignment.centerLeft,
                    child: TextButton.icon(
                      icon: const Icon(Icons.expand_more, size: 18),
                      label: const Text('Load more'),
                      onPressed: () => c.send(const MusicCmd.ytSearchMore()),
                    ),
                  ),
                ),
            ],
          ],
        );
      },
    );
  }

  Widget _label(BuildContext context, String text) => Padding(
        padding: const EdgeInsets.fromLTRB(24, 24, 24, 10),
        child: Text(
          text.toUpperCase(),
          style: TextStyle(
              fontSize: 11,
              letterSpacing: 1.4,
              fontWeight: FontWeight.w600,
              color: context.tokens.nInk3),
        ),
      );
}

class _ChannelTile extends StatelessWidget {
  const _ChannelTile({required this.controller, required this.sub});

  final MusicController controller;
  final YtSub sub;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = sub;
    return InkWell(
      borderRadius: BorderRadius.circular(Tokens.radiusMd),
      onTap: () => controller.send(MusicCmd.ytOpenChannel(channelId: s.channelId)),
      child: Container(
        width: 280,
        padding: const EdgeInsets.all(12),
        decoration: BoxDecoration(
          color: t.nCard,
          border: Border.all(color: t.nHair),
          borderRadius: BorderRadius.circular(Tokens.radiusMd),
        ),
        child: Row(
          children: [
            MusicArt(
              controller: controller,
              kind: 'yt',
              artKey: s.avatar,
              direct: s.avatar,
              size: 48,
              radius: 24,
              fallback: Icons.person,
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(s.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 14,
                          fontWeight: FontWeight.w600,
                          color: t.nInk)),
                  Text(
                    [
                      if (s.subs > 0) '${s.subs} subscribers',
                      if (s.videos > 0) '${s.videos} videos',
                    ].join('  ·  '),
                    style: TextStyle(fontSize: 11, color: t.nInk3),
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
