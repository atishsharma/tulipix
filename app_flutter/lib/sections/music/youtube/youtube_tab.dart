// YouTube: the tab's shell. A header (tabs, the pager of a paged page, a
// settings menu), the download strip, and whichever page is open, a channel or
// a playlist included: the tab row stays over those, and a tab is a way out of
// them. Search and the count live in the
// Music header, like every other section's.
//
// Playback is the section's: audio goes on the deck every tab shares, and a
// video opens in the in-app player, switched from the player bar.

import 'package:flutter/material.dart';

import '../../../platform/pick.dart';
import '../../../design/tokens.dart';
import '../../../src/rust/api/music.dart';
import '../music_controller.dart';
import '../music_widgets.dart';
import 'yt_cached.dart';
import 'yt_card.dart';
import 'yt_channel.dart';
import 'yt_dialogs.dart';
import 'yt_downloads.dart';
import 'yt_history.dart';
import 'yt_home.dart';
import 'yt_playlists.dart';
import 'yt_results.dart';
import 'yt_subscriptions.dart';

class YoutubeTab extends StatefulWidget {
  const YoutubeTab({super.key, required this.controller});

  final MusicController controller;

  @override
  State<YoutubeTab> createState() => _YoutubeTabState();
}

class _YoutubeTabState extends State<YoutubeTab> {
  /// Cached, History and Playlists page here rather than in the bridge: the whole list is
  /// already in the snapshot. Downloads pages in the bridge.
  final Map<String, int> _pages = {};

  /// Cached's All / Audio / Video, here because the pager counts what it keeps.
  String _cachedKind = 'all';

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final st = c.state;
    if (st == null) return const Center(child: CircularProgressIndicator());

    // The pager in the header, for the three pages of cards.
    final onPage = !st.ytChannelOpen && !st.ytPlaylistOpen;
    int pagesOf(int n) => ((n / ytPerPage).ceil()).clamp(1, 1 << 30);
    final (int, int, void Function(int)) paging = switch (st.ytTab) {
      'downloads' when onPage => (
          st.ytDlPage,
          st.ytDlPages,
          (int p) => c.send(MusicCmd.ytSetDlPage(page: p)),
        ),
      'cached' || 'history' || 'playlists' when onPage => () {
          final tab = st.ytTab;
          final n = switch (tab) {
            'cached' => ytCachedOfKind(st, _cachedKind).length,
            'history' => st.ytHistory.length,
            _ => st.ytPlaylists.length,
          };
          final pages = pagesOf(n);
          // A filter or a removal can leave the page past the end.
          final page = (_pages[tab] ?? 0).clamp(0, pages - 1);
          return (page, pages, (int p) => setState(() => _pages[tab] = p));
        }(),
      _ => (0, 1, (int _) {}),
    };
    final (page, pages, onGo) = paging;

    return Column(
      children: [
        SizedBox(
          height: 52,
          child: Row(
            children: [
              // The tabs scroll rather than push the right-hand controls off
              // the row's end.
              Expanded(
                child: SingleChildScrollView(
                  scrollDirection: Axis.horizontal,
                  padding: const EdgeInsets.only(left: 20),
                  child: Row(
                    children: [
                      for (final tab in const [
                        ('home', 'Home'),
                        ('subscriptions', 'Subscriptions'),
                        ('playlists', 'Playlists'),
                        ('history', 'History'),
                        ('cached', 'Cached'),
                        ('downloads', 'Downloads'),
                      ])
                        Padding(
                          padding: const EdgeInsets.symmetric(horizontal: 4),
                          child: MusicChip(
                            label: tab.$2,
                            active: st.ytTab == tab.$1,
                            tint: ytRose,
                            tint2: const Color(0xFFF97316),
                            onTap: () =>
                                c.send(MusicCmd.ytSetTab(name: tab.$1)),
                          ),
                        ),
                    ],
                  ),
                ),
              ),
              // `yt_busy` covers the requests that have no progress to report;
              // without it a slow search is indistinguishable from a dead one.
              if (st.ytBusy)
                Padding(
                  padding: const EdgeInsets.only(left: 12, right: 12),
                  child: SizedBox(
                    width: 14,
                    height: 14,
                    child: CircularProgressIndicator(
                      strokeWidth: 2,
                      valueColor: AlwaysStoppedAnimation(context.tokens.nInk2),
                    ),
                  ),
                ),
              if (onPage && st.ytTab == 'history')
                Padding(
                  padding: const EdgeInsets.only(left: 8, right: 12),
                  child: YtHistoryActions(controller: c, st: st),
                ),
              if (onPage && st.ytTab == 'playlists')
                Padding(
                  padding: const EdgeInsets.only(left: 8, right: 12),
                  child: YtPlaylistsActions(controller: c),
                ),
              if (pages > 1)
                Padding(
                  padding: const EdgeInsets.only(left: 4, right: 8),
                  child: Pager(
                      page: page, pages: pages, onGo: onGo, compact: true),
                ),
              _Menu(controller: c, st: st),
              const SizedBox(width: 8),
            ],
          ),
        ),
        if (st.ytFetchBusy)
          _FetchBar(label: st.ytFetchMsg, frac: st.ytFetchFrac),
        // The Downloads page lists every job itself.
        if (st.ytJobs.isNotEmpty &&
            (st.ytTab != 'downloads' || st.ytChannelOpen || st.ytPlaylistOpen))
          YtJobs(controller: c, st: st),
        Expanded(
          child: switch (st.ytTab) {
            _ when st.ytChannelOpen => YtChannel(controller: c, st: st),
            _ when st.ytPlaylistOpen => YtPlaylistPage(controller: c, st: st),
            'subscriptions' => YtSubscriptions(controller: c, st: st),
            'playlists' => YtPlaylists(controller: c, st: st, page: page),
            'history' => YtHistory(controller: c, st: st, page: page),
            'cached' => YtCached(
                controller: c,
                st: st,
                kind: _cachedKind,
                page: page,
                onKind: (k) => setState(() {
                  _cachedKind = k;
                  _pages['cached'] = 0;
                }),
              ),
            'downloads' => YtDownloads(controller: c, st: st),
            'results' => YtResults(controller: c, st: st),
            _ => YtHome(controller: c, st: st),
          },
        ),
      ],
    );
  }
}

/// Everything the tab does that is not a page: adding and importing, the two
/// slow refreshes, and the listing backend.
class _Menu extends StatelessWidget {
  const _Menu({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    return PopupMenuButton<String>(
      tooltip: 'YouTube settings',
      icon: const Icon(Icons.settings_outlined, size: 20),
      onSelected: (choice) async {
        switch (choice) {
          case 'channel':
            await addYtChannel(context, c);
          case 'playlist':
            await importYtPlaylist(context, c);
          case 'takeout':
            final path = await pickFile(
              label: "Takeout's subscriptions.csv",
              extensions: ['csv'],
            );
            if (path != null) await c.send(MusicCmd.ytImportSubs(path: path));
          case 'new':
            await c.send(const MusicCmd.ytCheckNew());
          case 'counts':
            await c.send(const MusicCmd.ytRefreshSubs());
          case 'hide':
            await c.send(const MusicCmd.ytToggleHideWatched());
          case 'sponsor':
            await c.send(const MusicCmd.ytToggleSponsorSkip());
          case 'captions':
            await setYtCaptionLang(context, c);
          default:
            // Which backend does the listing. Piped is faster and
            // rate-limits less; yt-dlp always works; auto tries Piped first.
            await c.send(MusicCmd.ytSetFetcher(name: choice.substring(6)));
        }
      },
      itemBuilder: (_) => [
        const PopupMenuItem(
          value: 'channel',
          child: ListTile(
              dense: true,
              leading: Icon(Icons.person_add_alt, size: 18),
              title: Text('Add a channel…')),
        ),
        const PopupMenuItem(
          value: 'playlist',
          child: ListTile(
              dense: true,
              leading: Icon(Icons.playlist_add, size: 18),
              title: Text('Import a playlist…')),
        ),
        const PopupMenuItem(
          value: 'takeout',
          child: ListTile(
              dense: true,
              leading: Icon(Icons.upload_file, size: 18),
              title: Text('Import subscriptions…')),
        ),
        const PopupMenuDivider(),
        const PopupMenuItem(
          value: 'new',
          child: ListTile(
              dense: true,
              leading: Icon(Icons.fiber_new_outlined, size: 18),
              title: Text('Check for new videos')),
        ),
        const PopupMenuItem(
          value: 'counts',
          child: ListTile(
              dense: true,
              leading: Icon(Icons.refresh, size: 18),
              title: Text('Refresh subscriber counts')),
        ),
        const PopupMenuDivider(),
        for (final (key, label) in const [
          ('auto', 'Listings: Auto'),
          ('piped', 'Listings: Piped'),
          ('ytdlp', 'Listings: yt-dlp'),
        ])
          CheckedPopupMenuItem(
            value: 'fetch:$key',
            checked: st.ytFetcher == key,
            child: Text(label),
          ),
        const PopupMenuDivider(),
        CheckedPopupMenuItem(
          value: 'hide',
          checked: st.ytHideWatched,
          child: const Text('Hide watched videos'),
        ),
        CheckedPopupMenuItem(
          value: 'sponsor',
          checked: st.ytSponsorSkip,
          child: const Text('Skip sponsor segments'),
        ),
        PopupMenuItem(
          value: 'captions',
          child: ListTile(
              dense: true,
              leading: const Icon(Icons.closed_caption_outlined, size: 18),
              title: const Text('Caption language…'),
              trailing: Text(st.ytCaptionLang)),
        ),
      ],
    );
  }
}

/// The two refreshes are one yt-dlp spawn per channel, so forty channels is a
/// minute of nothing without this.
class _FetchBar extends StatelessWidget {
  const _FetchBar({required this.label, required this.frac});

  final String label;
  final double frac;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 0, 20, 8),
      child: Row(
        children: [
          Expanded(
            child: ClipRRect(
              borderRadius: BorderRadius.circular(2),
              child: LinearProgressIndicator(
                value: frac <= 0 ? null : frac.clamp(0.0, 1.0),
                minHeight: 3,
                backgroundColor: t.nHair,
                valueColor: const AlwaysStoppedAnimation(Color(0xFFF97316)),
              ),
            ),
          ),
          const SizedBox(width: 10),
          Text(label, style: TextStyle(fontSize: 11, color: t.nInk2)),
        ],
      ),
    );
  }
}
