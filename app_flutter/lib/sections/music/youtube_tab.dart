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
              // What is on the deck right now, as a picture. The Slint build
              // calls this Watch current; it is the fastest path from "this
              // song is good" to seeing the video.
              if (c.now?.mode == 'youtube' && !st.ytWatching)
                TextButton.icon(
                  icon: const Icon(Icons.smart_display_outlined, size: 16),
                  label: const Text('Watch this'),
                  onPressed: () => c.send(const MusicCmd.ytWatchCurrent()),
                ),
              if (st.ytWatching)
                TextButton.icon(
                  icon: const Icon(Icons.close, size: 16),
                  label: const Text('Close the picture'),
                  onPressed: () => c.send(const MusicCmd.ytStopWatching()),
                ),
              // Which backend does the listing. Piped is faster and rate-limits
              // less; yt-dlp always works. "auto" tries Piped and falls back.
              PopupMenuButton<String>(
                tooltip: 'Listing backend: ${st.ytFetcher}',
                icon: const Icon(Icons.cloud_outlined, size: 18),
                initialValue: st.ytFetcher,
                onSelected: (v) => c.send(MusicCmd.ytSetFetcher(name: v)),
                itemBuilder: (_) => const [
                  PopupMenuItem(value: 'auto', child: Text('Auto')),
                  PopupMenuItem(value: 'piped', child: Text('Piped')),
                  PopupMenuItem(value: 'ytdlp', child: Text('yt-dlp')),
                ],
              ),
              IconButton(
                iconSize: 18,
                tooltip: st.ytHomeConnect
                    ? 'Plain outlines on the Home rails'
                    : 'Gradient outlines on the Home rails',
                icon: Icon(st.ytHomeConnect
                    ? Icons.gradient
                    : Icons.gradient_outlined),
                onPressed: () => c.send(const MusicCmd.ytToggleHomeConnect()),
              ),
              const SizedBox(width: 8),
            ],
          ),
        ),
        if (st.ytFetchBusy)
          _FetchBar(label: st.ytFetchMsg, frac: st.ytFetchFrac),
        if (st.ytJobs.isNotEmpty) _DlJobs(st: st),
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
              Row(
                children: [
                  const SizedBox(width: 20),
                  for (final m in const [
                    ('new', 'Newest'),
                    ('old', 'Oldest'),
                    ('az', 'A–Z'),
                  ])
                    Padding(
                      padding: const EdgeInsets.only(right: 6),
                      child: SortChip(
                        label: m.$2,
                        active: st.ytDlSort == m.$1,
                        onTap: () => c.send(MusicCmd.ytSetDlSort(mode: m.$1)),
                      ),
                    ),
                ],
              ),
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

/// The counts refresh — one yt-dlp spawn per channel, so forty channels is a
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

/// What yt-dlp is doing. A download used to be a button press with no visible
/// consequence until a file appeared in another tab minutes later.
class _DlJobs extends StatelessWidget {
  const _DlJobs({required this.st});

  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final head = st.ytJobs.first;
    final queued = st.ytJobs.length - 1;
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 0, 20, 8),
      child: Row(
        children: [
          const Icon(Icons.downloading, size: 16, color: Color(0xFFF43F5E)),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  queued > 0
                      ? '${head.title}   ·   $queued waiting'
                      : head.title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11, color: t.nInk2),
                ),
                const SizedBox(height: 4),
                ClipRRect(
                  borderRadius: BorderRadius.circular(2),
                  child: LinearProgressIndicator(
                    // Indeterminate until yt-dlp's first percent line: it
                    // spends the opening seconds resolving formats.
                    value: head.frac <= 0 ? null : head.frac.clamp(0.0, 1.0),
                    minHeight: 3,
                    backgroundColor: t.nHair,
                    valueColor: const AlwaysStoppedAnimation(Color(0xFFF43F5E)),
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 10),
          Text('${(head.frac * 100).round()}%',
              style: TextStyle(fontSize: 11, color: t.nInk2)),
        ],
      ),
    );
  }
}

/// Subscribe to a channel you already have the URL for, rather than having to
/// find one of its videos first.
Future<void> _addChannel(BuildContext context, MusicController c) async {
  final text = TextEditingController();
  final url = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Add a channel'),
      content: SizedBox(
        width: 460,
        child: TextField(
          controller: text,
          autofocus: true,
          decoration: const InputDecoration(
            labelText: 'Channel URL or @handle',
            hintText: '@veritasium',
          ),
          onSubmitted: (v) => Navigator.pop(ctx, v),
        ),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx), child: const Text('Cancel')),
        FilledButton(
            onPressed: () => Navigator.pop(ctx, text.text),
            child: const Text('Add')),
      ],
    ),
  );
  final trimmed = url?.trim() ?? '';
  if (trimmed.isNotEmpty) {
    await c.send(MusicCmd.ytAddChannelUrl(url: trimmed));
  }
}

/// Import a YouTube playlist by URL — ids and title in one go.
Future<void> _importPlaylist(BuildContext context, MusicController c) async {
  final text = TextEditingController();
  final url = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Import a playlist'),
      content: SizedBox(
        width: 460,
        child: TextField(
          controller: text,
          autofocus: true,
          decoration: const InputDecoration(
            labelText: 'Playlist URL',
            hintText: 'https://www.youtube.com/playlist?list=…',
          ),
          onSubmitted: (v) => Navigator.pop(ctx, v),
        ),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx), child: const Text('Cancel')),
        FilledButton(
            onPressed: () => Navigator.pop(ctx, text.text),
            child: const Text('Import')),
      ],
    ),
  );
  final trimmed = url?.trim() ?? '';
  if (trimmed.isNotEmpty) {
    await c.send(MusicCmd.ytImportPlaylistUrl(url: trimmed));
  }
}

/// Watch rather than listen. Honours the saved height, asks when there is
/// none — the same preference the download picker writes, because "what
/// quality do you want" is one question however the file is used.
Future<void> watchVideo(
    BuildContext context, MusicController c, YtVideo v) async {
  var res = c.state?.ytDefaultRes ?? kResUnset;
  if (res == kResUnset) {
    final picked = await showDialog<int>(
      context: context,
      builder: (ctx) => SimpleDialog(
        title: const Text('Watch at'),
        children: [
          for (final o in const [
            (1080, '1080p'),
            (720, '720p'),
            (480, '480p'),
            (0, 'Best available'),
          ])
            SimpleDialogOption(
              onPressed: () => Navigator.pop(ctx, o.$1),
              child: Text(o.$2),
            ),
        ],
      ),
    );
    if (picked == null) return;
    res = picked;
  }
  // Audio-only is a download answer, not a watch one.
  await c.send(MusicCmd.ytWatch(videoId: v.videoId, height: res < 0 ? 0 : res));
}

/// The saved default, as the menu label. Null when nothing is saved, so the
/// caller falls back to "Download…" and the picker opens.
String? qualityLabel(int? res) {
  if (res == null || res == kResUnset) return null;
  if (res < 0) return 'Download audio (Opus)';
  if (res == 0) return 'Download best video';
  return 'Download ${res}p';
}

/// `yt_prefs::RES_UNSET` — the value that means "ask every time".
const int kResUnset = -999;

/// The wire spelling `yt_download` expects.
String qualityArg(int res) =>
    res < 0 ? 'audio' : (res == 0 ? 'best' : '${res}p');

/// Download honouring the saved default, asking only when there isn't one.
Future<void> downloadWithQuality(
    BuildContext context, MusicController c, YtVideo v) async {
  final res = c.state?.ytDefaultRes ?? kResUnset;
  if (res == kResUnset) {
    await pickQuality(context, c, v);
    return;
  }
  await c
      .send(MusicCmd.ytDownload(videoId: v.videoId, quality: qualityArg(res)));
}

/// The quality sheet: the same five the Slint picker offers, plus the tick
/// that turns one of them into the default. `force` reopens it even when a
/// default is saved, which is how you change your mind about one.
Future<void> pickQuality(
  BuildContext context,
  MusicController c,
  YtVideo v, {
  bool force = false,
}) async {
  final saved = c.state?.ytDefaultRes ?? kResUnset;
  var remember = false;
  final picked = await showDialog<int>(
    context: context,
    builder: (ctx) => StatefulBuilder(
      builder: (ctx, setLocal) => SimpleDialog(
        title: Text(v.title, maxLines: 2, overflow: TextOverflow.ellipsis),
        children: [
          for (final o in const [
            (1080, '1080p'),
            (720, '720p'),
            (480, '480p'),
            (0, 'Best video'),
            (-1, 'Audio only (Opus)'),
          ])
            SimpleDialogOption(
              onPressed: () => Navigator.pop(ctx, o.$1),
              child: Row(
                children: [
                  Expanded(child: Text(o.$2)),
                  if (saved == o.$1) const Icon(Icons.check, size: 16),
                ],
              ),
            ),
          const Divider(height: 12),
          CheckboxListTile(
            dense: true,
            value: remember,
            onChanged: (x) => setLocal(() => remember = x ?? false),
            title: const Text('Remember this choice'),
          ),
          if (saved != kResUnset)
            SimpleDialogOption(
              onPressed: () {
                c.send(const MusicCmd.ytResetVideoPrefs());
                Navigator.pop(ctx);
              },
              child: const Text('Forget the saved default'),
            ),
        ],
      ),
    ),
  );
  if (picked == null) return;
  if (remember) {
    await c.send(MusicCmd.ytSetDefaultRes(height: picked));
  }
  await c.send(
      MusicCmd.ytDownload(videoId: v.videoId, quality: qualityArg(picked)));
}

class _Home extends StatelessWidget {
  const _Home({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (st.ytResults.isEmpty &&
        st.ytRecommended.isEmpty &&
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
                  onPressed: () => confirmThen(
                    context,
                    controller,
                    title: 'Clear recent searches?',
                    body: 'The list of what you have searched for is '
                        'forgotten. Nothing downloaded is touched.',
                    action: 'Clear',
                    cmd: const MusicCmd.ytClearRecent(),
                  ),
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
          if (st.ytResultsMore)
            Padding(
              padding: const EdgeInsets.fromLTRB(24, 8, 24, 0),
              child: Align(
                alignment: Alignment.centerLeft,
                child: TextButton.icon(
                  icon: const Icon(Icons.expand_more, size: 18),
                  label: const Text('Load more'),
                  onPressed: () =>
                      controller.send(const MusicCmd.ytSearchMore()),
                ),
              ),
            ),
        ] else if (st.ytRecommended.isNotEmpty) ...[
          // Not a search: the newest thing from each channel you follow. This
          // is what makes Home a feed rather than a blank search box.
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 20, 24, 8),
            child: Text('New from your channels',
                style: TextStyle(
                    fontSize: 15, fontWeight: FontWeight.w700, color: t.nInk)),
          ),
          for (final v in st.ytRecommended)
            VideoRow(controller: controller, video: v),
        ],
        if (st.ytCached.isNotEmpty)
          Rail(
            title: 'Cached',
            height: 200,
            connect: st.ytHomeConnect,
            action: (
              'See all',
              () => controller.send(const MusicCmd.ytSetTab(name: 'cached'))
            ),
            children: [
              for (final v in st.ytCached.take(10))
                _VideoCard(controller: controller, video: v),
            ],
          ),
        // The rail is what you pinned; until you pin anything the bridge fills
        // it with your most-followed channels, so it is never empty for the
        // want of a feature nobody has found yet.
        if (st.ytHomeSubs.isNotEmpty)
          Rail(
            title: st.ytHomeChannels.isEmpty ? 'Channels' : 'Pinned channels',
            height: 190,
            connect: st.ytHomeConnect,
            action: (
              'See all',
              () => controller
                  .send(const MusicCmd.ytSetTab(name: 'subscriptions'))
            ),
            children: [
              for (final s in st.ytHomeSubs)
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
                  onMenu: () => controller.send(
                    st.ytHomeChannels.contains(s.channelId)
                        ? MusicCmd.ytUnpinHome(channelId: s.channelId)
                        : MusicCmd.ytPinHome(channelId: s.channelId),
                  ),
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
            // One click, not a menu dive: watching is what half of YouTube is
            // for and the audio deck is the other half.
            IconButton(
              iconSize: 18,
              tooltip: 'Watch the video',
              icon: const Icon(Icons.smart_display_outlined),
              onPressed: () => watchVideo(context, controller, v),
            ),
            PopupMenuButton<String>(
              iconSize: 18,
              tooltip: 'More',
              onSelected: (choice) async {
                switch (choice) {
                  case 'remove':
                    onRemove?.call();
                  case 'playlist':
                    await _addToYtPlaylist(context, controller, v);
                  case 'download':
                    await downloadWithQuality(context, controller, v);
                  case 'download-ask':
                    await pickQuality(context, controller, v, force: true);
                  case 'watch':
                    await watchVideo(context, controller, v);
                }
              },
              itemBuilder: (_) => [
                const PopupMenuItem(
                    value: 'watch', child: Text('Watch the video')),
                const PopupMenuItem(
                    value: 'playlist', child: Text('Add to playlist…')),
                PopupMenuItem(
                  value: 'download',
                  child: Text(qualityLabel(controller.state?.ytDefaultRes) ??
                      'Download…'),
                ),
                // Always reachable, so a saved default is never a trap.
                const PopupMenuItem(
                    value: 'download-ask', child: Text('Download at…')),
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
    return Column(
      children: [
        Row(
          children: [
            const SizedBox(width: 20),
            for (final m in const [
              ('subscribers', 'Followers'),
              ('videos', 'Videos'),
              ('name', 'Name'),
            ])
              Padding(
                padding: const EdgeInsets.only(right: 6),
                child: SortChip(
                  label: m.$2,
                  active: st.ytSubsSort == m.$1,
                  dir: st.ytSubsSort == m.$1
                      ? (st.ytSubsDir == 'desc' ? -1 : 1)
                      : 0,
                  onTap: () => st.ytSubsSort == m.$1
                      ? controller.send(const MusicCmd.ytToggleSubsDir())
                      : controller.send(MusicCmd.ytSetSubsSort(mode: m.$1)),
                ),
              ),
            const SizedBox(width: 10),
            // Channels you unsubscribed from stay in the table -- a Takeout
            // import brings in hundreds and unsubscribing is how you thin it
            // out, so getting back to one has to be possible.
            MusicChip(
              label: st.ytSubsFilter == 'unsub' ? 'Unsubscribed' : 'Subscribed',
              icon: st.ytSubsFilter == 'unsub'
                  ? Icons.person_off_outlined
                  : Icons.how_to_reg,
              active: st.ytSubsFilter == 'unsub',
              tint: const Color(0xFFF43F5E),
              tint2: const Color(0xFFF97316),
              onTap: () => controller.send(const MusicCmd.ytToggleSubsFilter()),
            ),
            const Spacer(),
            TextButton.icon(
              icon: const Icon(Icons.person_add_alt, size: 16),
              label: const Text('Add by URL'),
              onPressed: () => _addChannel(context, controller),
            ),
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
        if (st.ytSubs.isEmpty)
          Expanded(
            child: MusicEmpty(
              icon: Icons.subscriptions_outlined,
              title: st.ytSubsFilter == 'unsub'
                  ? 'Nothing unsubscribed'
                  : 'No channels',
              body: 'Open a channel from a search result and subscribe, add '
                  "one by URL, or import Google Takeout's subscriptions.csv.",
            ),
          )
        else
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
                    onMenu: () => controller.send(
                      s.subscribed
                          ? MusicCmd.ytUnsub(channelId: s.channelId)
                          : MusicCmd.ytSubscribe(
                              channelId: s.channelId, title: s.title),
                    ),
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
            TextButton.icon(
              icon: const Icon(Icons.link, size: 16),
              label: const Text('Import from URL'),
              onPressed: () => _importPlaylist(context, controller),
            ),
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
                        // The card that is playing says so, which is the only
                        // way to tell which playlist the deck is walking.
                        subtitle: st.ytPlayingPlId == p.id
                            ? '▶  playing  ·  ${p.count} videos'
                            : '${p.count} videos',
                        artKind: 'yt',
                        artKey: p.cover,
                        direct: p.cover,
                        fallback: Icons.playlist_play,
                        badge: st.ytPlayingPlId == p.id ? '▶' : null,
                        onTap: () => controller
                            .send(MusicCmd.ytOpenPlaylist(playlistId: p.id)),
                        // Play-all straight off the card, as Slint's
                        // `yt-playlist-play-all-id` does.
                        onPlay: () => controller
                            .send(MusicCmd.ytPlaylistPlayAll(playlistId: p.id)),
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

class _ChannelPage extends StatefulWidget {
  const _ChannelPage({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  State<_ChannelPage> createState() => _ChannelPageState();
}

class _ChannelPageState extends State<_ChannelPage> {
  final _search = TextEditingController();

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    final st = widget.st;
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
              Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  Text(st.ytChannelTitle,
                      style: TextStyle(
                          fontSize: 17,
                          fontWeight: FontWeight.w700,
                          color: t.nInk)),
                  // "142 videos · 4.2M subscribers" -- the two numbers that
                  // say whether a channel is worth following, which the port
                  // had nowhere to put.
                  if (st.ytChannelSub.isNotEmpty)
                    Text(st.ytChannelSub,
                        style: TextStyle(fontSize: 11, color: t.nInk2)),
                ],
              ),
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
              SizedBox(
                width: 220,
                child: TextField(
                  controller: _search,
                  decoration: InputDecoration(
                    isDense: true,
                    prefixIcon: const Icon(Icons.search, size: 16),
                    hintText: 'Search this channel',
                    border: const OutlineInputBorder(),
                    suffixIcon: st.ytChannelQuery.isEmpty
                        ? null
                        : IconButton(
                            icon: const Icon(Icons.close, size: 14),
                            onPressed: () {
                              _search.clear();
                              controller.send(
                                  const MusicCmd.ytChannelSearch(query: ''));
                            },
                          ),
                  ),
                  onSubmitted: (q) =>
                      controller.send(MusicCmd.ytChannelSearch(query: q)),
                ),
              ),
              const SizedBox(width: 10),
              IconButton(
                iconSize: 18,
                tooltip: st.ytHomeChannels.contains(st.ytChannelId)
                    ? 'Unpin from Home'
                    : 'Pin to the Home rail',
                icon: Icon(st.ytHomeChannels.contains(st.ytChannelId)
                    ? Icons.push_pin
                    : Icons.push_pin_outlined),
                onPressed: () => controller.send(
                  st.ytHomeChannels.contains(st.ytChannelId)
                      ? MusicCmd.ytUnpinHome(channelId: st.ytChannelId)
                      : MusicCmd.ytPinHome(channelId: st.ytChannelId),
                ),
              ),
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
        Expanded(child: _channelBody(st, controller)),
      ],
    );
  }

  /// A search replaces the listing while it is active; clearing the box brings
  /// the listing back, which is why the two lists are kept apart in state.
  Widget _channelBody(MusicState st, MusicController controller) {
    final searching = st.ytChannelQuery.isNotEmpty;
    final videos = searching ? st.ytChannelResults : st.ytChannelVideos;
    if (videos.isEmpty) {
      return MusicEmpty(
        icon: searching ? Icons.search_off : Icons.videocam_off_outlined,
        title: searching ? 'No matches in this channel' : 'Nothing to list',
        body: searching
            ? 'Try fewer words, or clear the box to see the whole channel.'
            : 'yt-dlp could not reach this channel, or it has no public '
                'uploads.',
      );
    }
    return ListView(
      children: [
        for (final v in videos) VideoRow(controller: controller, video: v),
        // Only on the listing: the search is a fixed twenty hits, so there is
        // no further page to ask for.
        if (!searching && st.ytChannelHasNext)
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 8, 24, 24),
            child: Align(
              alignment: Alignment.centerLeft,
              child: TextButton.icon(
                icon: const Icon(Icons.expand_more, size: 18),
                label: Text('Load more  ·  ${videos.length} so far'),
                onPressed: () =>
                    controller.send(const MusicCmd.ytChannelLoadMore()),
              ),
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
              const SizedBox(width: 16),
              for (final m in const [
                ('default', 'Playlist order'),
                ('title', 'Title'),
                ('duration', 'Length'),
              ])
                Padding(
                  padding: const EdgeInsets.only(right: 6),
                  child: SortChip(
                    label: m.$2,
                    active: st.ytPlaylistSort == m.$1,
                    onTap: () =>
                        controller.send(MusicCmd.ytSetPlaylistSort(mode: m.$1)),
                  ),
                ),
              const Spacer(),
              FilledButton.icon(
                icon: const Icon(Icons.play_arrow, size: 18),
                label: const Text('Play all'),
                onPressed: st.ytPlaylistVideos.isEmpty
                    ? null
                    : () => controller.send(MusicCmd.ytPlaylistPlayAll(
                          playlistId: st.ytPlaylistId,
                        )),
              ),
              const SizedBox(width: 12),
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
