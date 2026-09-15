// Stream's other four surfaces: Downloads, History, Bookmarks, and the Stream
// Setting modal — plus the cast picker, which is a modal over the detail pane.
//
// All five read the same snapshot the search view does; they are destinations
// in the header, not separate sections.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/videos.dart';
import 'videos_controller.dart';
import 'videos_stream.dart' show PickRow;
import 'videos_widgets.dart';

/// The header every sub-page shares: back, home, an icon, a title, a count.
class _PageHeader extends StatelessWidget {
  const _PageHeader({
    required this.icon,
    required this.title,
    required this.hue,
    required this.onBack,
    required this.controller,
    this.trailing = const [],
    this.count = '',
  });

  final IconData icon;
  final String title;
  final Color hue;
  final String count;
  final VoidCallback onBack;
  final VideosController controller;
  final List<Widget> trailing;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 66,
      color: t.panel,
      padding: const EdgeInsets.symmetric(horizontal: 28),
      child: Row(
        children: [
          PlexButton(
            icon: Icons.arrow_back,
            label: 'Back',
            hue: cBack,
            filled: true,
            compact: true,
            onTap: onBack,
          ),
          const SizedBox(width: 12),
          IconBtn(
            icon: Icons.home_outlined,
            tip: 'Back to the Stream home',
            hue: cInfo,
            onTap: () {
              onBack();
              controller.send(const VideosCmd.streamHome());
            },
          ),
          const SizedBox(width: 12),
          Icon(icon, size: 20, color: hue),
          const SizedBox(width: 10),
          Text(title,
              style: TextStyle(
                  fontSize: 22, fontWeight: FontWeight.w800, color: t.nInk)),
          const SizedBox(width: 12),
          if (count.isNotEmpty)
            Text(count, style: TextStyle(fontSize: 12, color: t.nInk3)),
          const Spacer(),
          ...trailing,
        ],
      ),
    );
  }
}

// ------------------------------------------------------------- downloads ----

class StreamDownloadsPage extends StatelessWidget {
  const StreamDownloadsPage({
    super.key,
    required this.controller,
    required this.stream,
    required this.onBack,
  });

  final VideosController controller;
  final StreamView stream;
  final VoidCallback onBack;

  Future<bool> _confirm(
      BuildContext context, String title, String body, String action) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(title),
        content: Text(body),
        actions: [
          TextButton(
              onPressed: () => Navigator.pop(ctx, false),
              child: const Text('Cancel')),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: cErr),
            onPressed: () => Navigator.pop(ctx, true),
            child: Text(action),
          ),
        ],
      ),
    );
    return ok ?? false;
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        _PageHeader(
          icon: Icons.download,
          title: 'Downloads',
          hue: cDl,
          controller: controller,
          onBack: onBack,
          count: stream.dlPending > 0
              ? '${stream.dlPending} in the queue'
              : '${stream.downloads.length} items',
          trailing: [
            if (stream.dlPending > 0) ...[
              PlexButton(
                compact: true,
                icon: Icons.close,
                label: 'Stop all',
                hue: cErr,
                onTap: () async {
                  if (await _confirm(
                      context,
                      'Stop every download?',
                      'Anything still running is cancelled. Finished files '
                          'stay where they are.',
                      'Stop all')) {
                    await controller
                        .send(const VideosCmd.streamDownloadsCancelAll());
                  }
                },
              ),
              const SizedBox(width: 8),
            ],
            PlexButton(
              compact: true,
              icon: Icons.delete_outline,
              label: 'Clear',
              hue: cBack,
              onTap: () async {
                if (await _confirm(context, 'Clear the ledger?',
                    'The rows go; the files on disk stay.', 'Clear')) {
                  await controller.send(const VideosCmd.streamDownloadsClear());
                }
              },
            ),
          ],
        ),
        Expanded(
          child: stream.downloads.isEmpty
              ? const VideosEmpty(
                  icon: Icons.download,
                  title: 'Nothing downloaded yet',
                  message:
                      'Open a title and press Download, or queue a whole season.',
                )
              : ListView.separated(
                  padding: const EdgeInsets.fromLTRB(28, 14, 28, 14),
                  itemCount: stream.downloads.length,
                  separatorBuilder: (_, __) => const SizedBox(height: 6),
                  itemBuilder: (_, i) => _DownloadRowView(
                    row: stream.downloads[i],
                    live: controller.liveProgress[stream.downloads[i].id],
                    controller: controller,
                    onDelete: () async {
                      final row = stream.downloads[i];
                      if (await _confirm(
                          context,
                          'Delete the file?',
                          '“${row.title}” is removed from disk. This cannot '
                              'be undone.',
                          'Delete')) {
                        await controller.send(
                            VideosCmd.streamDownloadDeleteFile(id: row.id));
                      }
                    },
                  ),
                ),
        ),
      ],
    );
  }
}

class _DownloadRowView extends StatelessWidget {
  const _DownloadRowView({
    required this.row,
    required this.live,
    required this.controller,
    required this.onDelete,
  });

  final StreamDownloadRow row;
  final ({double progress, String detail})? live;
  final VideosController controller;
  final VoidCallback onDelete;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // A running job's numbers come off the event stream — the snapshot only
    // moves when a command is dispatched, and nothing is being dispatched
    // while bytes are arriving.
    final progress = live?.progress ?? row.progress;
    final detail = live?.detail ?? row.detail;
    final hue = row.failed
        ? cErr
        : row.done
            ? cPlay
            : cDl;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: t.nHair),
      ),
      child: Row(
        children: [
          Container(
            width: 8,
            height: 8,
            decoration: BoxDecoration(color: hue, shape: BoxShape.circle),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(row.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
                const SizedBox(height: 2),
                Text('${row.state} · $detail',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11, color: t.nInk3)),
                if (row.active) ...[
                  const SizedBox(height: 6),
                  ClipRRect(
                    borderRadius: BorderRadius.circular(2),
                    child: LinearProgressIndicator(
                      minHeight: 4,
                      value: progress.clamp(0.0, 1.0),
                      backgroundColor: t.nHair,
                      valueColor: AlwaysStoppedAnimation(hue),
                    ),
                  ),
                ],
              ],
            ),
          ),
          const SizedBox(width: 12),
          if (row.done)
            IconBtn(
                icon: Icons.play_arrow,
                tip: 'Play',
                hue: cPlay,
                size: 30,
                filled: true,
                onTap: () =>
                    controller.send(VideosCmd.streamDownloadPlay(id: row.id))),
          if (row.failed)
            IconBtn(
                icon: Icons.refresh,
                tip: 'Retry',
                hue: cBack,
                size: 30,
                onTap: () =>
                    controller.send(VideosCmd.streamDownloadRetry(id: row.id))),
          const SizedBox(width: 6),
          IconBtn(
              icon: Icons.folder_open,
              tip: 'Show in files',
              hue: cCopy,
              size: 30,
              onTap: () =>
                  controller.send(VideosCmd.streamDownloadReveal(id: row.id))),
          const SizedBox(width: 6),
          IconBtn(
              icon: Icons.visibility_off_outlined,
              tip: 'Forget this row',
              size: 30,
              onTap: () =>
                  controller.send(VideosCmd.streamDownloadForget(id: row.id))),
          const SizedBox(width: 6),
          IconBtn(
              icon: Icons.delete_outline,
              tip: 'Delete the file',
              hue: cErr,
              size: 30,
              onTap: onDelete),
        ],
      ),
    );
  }
}

// --------------------------------------------------------------- history ----

class StreamHistoryPage extends StatelessWidget {
  const StreamHistoryPage({
    super.key,
    required this.controller,
    required this.stream,
    required this.onBack,
  });

  final VideosController controller;
  final StreamView stream;
  final VoidCallback onBack;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      children: [
        _PageHeader(
          icon: Icons.history,
          title: 'History',
          hue: Tokens.secVideos,
          controller: controller,
          onBack: onBack,
          count: '${stream.history.length} titles',
          trailing: [
            PlexButton(
              icon: Icons.delete_outline,
              label: 'Clear',
              hue: cErr,
              compact: true,
              onTap: () async {
                final ok = await showDialog<bool>(
                  context: context,
                  builder: (ctx) => AlertDialog(
                    title: const Text('Clear the history?'),
                    content: const Text(
                        'Every past session goes, along with the resume '
                        'points they carry.'),
                    actions: [
                      TextButton(
                          onPressed: () => Navigator.pop(ctx, false),
                          child: const Text('Cancel')),
                      FilledButton(
                        style: FilledButton.styleFrom(backgroundColor: cErr),
                        onPressed: () => Navigator.pop(ctx, true),
                        child: const Text('Clear'),
                      ),
                    ],
                  ),
                );
                if (ok ?? false) {
                  await controller.send(const VideosCmd.streamHistoryClear());
                }
              },
            ),
          ],
        ),
        Expanded(
          child: stream.history.isEmpty
              ? const VideosEmpty(
                  icon: Icons.history_toggle_off,
                  title: 'Nothing watched yet',
                )
              : ListView.separated(
                  padding: const EdgeInsets.fromLTRB(28, 14, 28, 14),
                  itemCount: stream.history.length,
                  separatorBuilder: (_, __) => const SizedBox(height: 6),
                  itemBuilder: (_, i) => _HistoryRowView(
                    row: stream.history[i],
                    controller: controller,
                    page: stream.historyPage,
                  ),
                ),
        ),
        if (stream.historyPages > 1)
          Container(
            height: 52,
            color: t.panel,
            child: Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                IconBtn(
                  icon: Icons.chevron_left,
                  size: 30,
                  onTap: () => stream.historyPage > 0
                      ? controller.send(VideosCmd.streamHistoryLoad(
                          page: stream.historyPage - 1))
                      : null,
                ),
                const SizedBox(width: 10),
                Text('${stream.historyPage + 1} / ${stream.historyPages}',
                    style: TextStyle(
                        fontSize: 12,
                        fontWeight: FontWeight.w700,
                        color: t.nInk2)),
                const SizedBox(width: 10),
                IconBtn(
                  icon: Icons.chevron_right,
                  size: 30,
                  onTap: () => stream.historyPage + 1 < stream.historyPages
                      ? controller.send(VideosCmd.streamHistoryLoad(
                          page: stream.historyPage + 1))
                      : null,
                ),
              ],
            ),
          ),
      ],
    );
  }
}

class _HistoryRowView extends StatelessWidget {
  const _HistoryRowView({
    required this.row,
    required this.controller,
    required this.page,
  });

  final StreamHistoryRow row;
  final VideosController controller;
  final int page;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: t.nHair),
      ),
      child: Row(
        children: [
          Artwork(path: row.poster, width: 40, height: 60, radius: 5),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(row.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
                Text('${row.kind} · ${row.state}',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11, color: t.nInk3)),
                if (row.progress > 0 && !row.finished) ...[
                  const SizedBox(height: 5),
                  ClipRRect(
                    borderRadius: BorderRadius.circular(2),
                    child: ProgressStrip(value: row.progress),
                  ),
                ],
              ],
            ),
          ),
          const SizedBox(width: 12),
          Text(row.ago, style: TextStyle(fontSize: 11, color: t.nInk3)),
          const SizedBox(width: 12),
          IconBtn(
            icon: Icons.play_arrow,
            tip: 'Resume',
            hue: cPlay,
            size: 30,
            filled: true,
            // Re-resolves rather than replaying a URL from last week — a
            // provider's link has almost certainly expired.
            onTap: () => controller.send(VideosCmd.streamHistoryPlay(
                id: row.id, season: row.season, episode: row.episode)),
          ),
          const SizedBox(width: 6),
          IconBtn(
              icon: Icons.open_in_new,
              tip: 'Open the title',
              hue: cInfo,
              size: 30,
              onTap: () =>
                  controller.send(VideosCmd.streamOpenResume(id: row.id))),
          const SizedBox(width: 6),
          IconBtn(
              icon: Icons.close,
              tip: 'Remove',
              hue: cErr,
              size: 30,
              onTap: () => controller
                  .send(VideosCmd.streamHistoryRemove(id: row.id, page: page))),
        ],
      ),
    );
  }
}

// ------------------------------------------------------------- bookmarks ----

/// Nine to a page, in a 3×3 grid, exactly as the Slint page cuts it.
const int _bmPerPage = 9;

class StreamBookmarksPage extends StatefulWidget {
  const StreamBookmarksPage({
    super.key,
    required this.controller,
    required this.stream,
    required this.onBack,
    required this.onOpen,
  });

  final VideosController controller;
  final StreamView stream;
  final VoidCallback onBack;
  final ValueChanged<String> onOpen;

  @override
  State<StreamBookmarksPage> createState() => _StreamBookmarksPageState();
}

class _StreamBookmarksPageState extends State<StreamBookmarksPage> {
  int _page = 0;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = widget.stream;
    final pages = (s.bookmarks.length / _bmPerPage).ceil().clamp(1, 9999);
    final page = _page.clamp(0, pages - 1);
    final slice = s.bookmarks
        .skip(page * _bmPerPage)
        .take(_bmPerPage)
        .toList(growable: false);

    void sort(String key) {
      setState(() => _page = 0);
      widget.controller
          .send(VideosCmd.streamBookmarksSort(key: key, asc: s.bmAsc));
    }

    return Column(
      children: [
        _PageHeader(
          icon: Icons.bookmark,
          title: 'Bookmarks',
          hue: Tokens.secVideos,
          controller: widget.controller,
          onBack: widget.onBack,
          count:
              '${s.bookmarks.length} title${s.bookmarks.length == 1 ? '' : 's'}',
          trailing: [
            Text('SORT',
                style: TextStyle(
                    fontSize: 9,
                    fontWeight: FontWeight.w800,
                    letterSpacing: 1.2,
                    color: t.nInk3)),
            const SizedBox(width: 8),
            SortChip(
                label: 'Date',
                hue: cInfo,
                active: s.bmSort == 'date',
                onTap: () => sort('date')),
            const SizedBox(width: 6),
            SortChip(
                label: 'Name',
                hue: cDl,
                active: s.bmSort == 'name',
                onTap: () => sort('name')),
            const SizedBox(width: 6),
            SortChip(
                label: 'Type',
                hue: cBack,
                active: s.bmSort == 'type',
                onTap: () => sort('type')),
            const SizedBox(width: 8),
            IconBtn(
              icon: s.bmAsc ? Icons.arrow_upward : Icons.arrow_downward,
              size: 30,
              onTap: () {
                setState(() => _page = 0);
                widget.controller.send(VideosCmd.streamBookmarksSort(
                    key: s.bmSort, asc: !s.bmAsc));
              },
            ),
            if (pages > 1) ...[
              const SizedBox(width: 14),
              IconBtn(
                  icon: Icons.chevron_left,
                  size: 30,
                  onTap: () =>
                      setState(() => _page = (page - 1).clamp(0, pages - 1))),
              const SizedBox(width: 8),
              Text('${page + 1} / $pages',
                  style: TextStyle(
                      fontSize: 12,
                      fontWeight: FontWeight.w700,
                      color: t.nInk2)),
              const SizedBox(width: 8),
              IconBtn(
                  icon: Icons.chevron_right,
                  size: 30,
                  onTap: () =>
                      setState(() => _page = (page + 1).clamp(0, pages - 1))),
            ],
          ],
        ),
        Expanded(
          child: s.bookmarks.isEmpty
              ? const VideosEmpty(
                  icon: Icons.bookmark_border,
                  title: 'No bookmarks yet',
                  message: 'Open a title and press the bookmark button to '
                      'save it here.',
                )
              : GridView.count(
                  padding: const EdgeInsets.fromLTRB(44, 24, 44, 24),
                  crossAxisCount: 3,
                  mainAxisSpacing: 18,
                  crossAxisSpacing: 18,
                  childAspectRatio: 2.1,
                  children: [
                    for (final card in slice)
                      _BookmarkCard(
                        card: card,
                        onOpen: () => widget.onOpen(card.id),
                        onRemove: () => _confirmRemove(context, card),
                      ),
                  ],
                ),
        ),
      ],
    );
  }

  Future<void> _confirmRemove(BuildContext context, StreamCard card) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Remove the bookmark?'),
        content: Text('“${card.title}” goes off the list.'),
        actions: [
          TextButton(
              onPressed: () => Navigator.pop(ctx, false),
              child: const Text('Cancel')),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: cErr),
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('Remove'),
          ),
        ],
      ),
    );
    if (ok ?? false) {
      await widget.controller.send(VideosCmd.streamBookmarkRemove(id: card.id));
    }
  }
}

class _BookmarkCard extends StatelessWidget {
  const _BookmarkCard({
    required this.card,
    required this.onOpen,
    required this.onRemove,
  });

  final StreamCard card;
  final VoidCallback onOpen;
  final VoidCallback onRemove;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onOpen,
      borderRadius: BorderRadius.circular(12),
      child: Container(
        padding: const EdgeInsets.all(10),
        decoration: BoxDecoration(
          color: t.panel,
          borderRadius: BorderRadius.circular(12),
          border: Border.all(color: t.nHair),
        ),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Artwork(
              path: card.poster,
              width: 90,
              height: 135,
              radius: 6,
              fallback: card.isSeries ? Icons.tv : Icons.movie_outlined,
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Text(card.title,
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontSize: 14,
                                fontWeight: FontWeight.w800,
                                color: t.nInk)),
                      ),
                      IconBtn(
                          icon: Icons.close,
                          tip: 'Remove',
                          hue: cErr,
                          size: 26,
                          onTap: onRemove),
                    ],
                  ),
                  const SizedBox(height: 4),
                  OverlayPill(
                    text: card.isSeries ? 'SERIES' : 'MOVIE',
                    background: card.isSeries ? cBadgeSeries : cBadgeMovie,
                  ),
                  const SizedBox(height: 6),
                  if (card.meta.isNotEmpty)
                    Text(card.meta,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11, color: t.nInk2)),
                  const Spacer(),
                  if (card.seasons > 0)
                    Text(
                      '${card.seasons} new '
                      'episode${card.seasons == 1 ? '' : 's'}',
                      style: const TextStyle(
                          fontSize: 11,
                          fontWeight: FontWeight.w700,
                          color: cPlay),
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

// ----------------------------------------------------------------- cast ----

/// The DLNA renderers found on this network. A resolved stream is already a
/// public URL, so nothing is served locally to make this work.
Future<void> openCastPicker(
    BuildContext context, VideosController controller) async {
  await controller.send(const VideosCmd.streamCastDiscover());
  if (!context.mounted) return;
  await showDialog<void>(
    context: context,
    builder: (ctx) => AnimatedBuilder(
      animation: controller,
      builder: (ctx, _) {
        final s = controller.state?.stream;
        final t = ctx.tokens;
        return AlertDialog(
          backgroundColor: t.modalSolid,
          title: const Row(
            children: [
              Icon(Icons.cast, size: 18, color: Tokens.secVideos),
              SizedBox(width: 10),
              Text('Cast to'),
            ],
          ),
          content: SizedBox(
            width: 340,
            height: 260,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(
                  'DLNA renderers on this network play the stream directly.',
                  style: TextStyle(fontSize: 11, color: t.nInk3),
                ),
                const SizedBox(height: 12),
                Expanded(
                  child: (s?.castDevices ?? const []).isEmpty
                      ? Center(
                          child: Text(
                            (s?.busy ?? false)
                                ? 'Looking…'
                                : 'Nothing found yet — press Scan.',
                            style: TextStyle(fontSize: 12, color: t.nInk3),
                          ),
                        )
                      : ListView(
                          children: [
                            for (final dev in s!.castDevices)
                              PickRow(
                                label: dev,
                                active: dev == s.castTarget,
                                onTap: () {
                                  controller.send(
                                      VideosCmd.streamCastTo(device: dev));
                                  Navigator.pop(ctx);
                                },
                              ),
                          ],
                        ),
                ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () =>
                  controller.send(const VideosCmd.streamCastDiscover()),
              child: const Text('Scan'),
            ),
            if (s?.castActive ?? false)
              TextButton(
                onPressed: () {
                  controller.send(const VideosCmd.streamCastStop());
                  Navigator.pop(ctx);
                },
                child: const Text('Stop casting'),
              ),
            TextButton(
                onPressed: () => Navigator.pop(ctx),
                child: const Text('Close')),
          ],
        );
      },
    ),
  );
}

// -------------------------------------------------------------- settings ----

/// Four tabs over one modal: where to fetch from, what to sign with, which
/// catalogue, and how it plays.
Future<void> openStreamSettings(
    BuildContext context, VideosController controller) async {
  await controller.send(const VideosCmd.streamHostsLoad());
  if (!context.mounted) return;
  final s0 = controller.state?.stream;
  final hosts = TextEditingController(text: s0?.hostsText ?? '');
  final key = TextEditingController(text: s0?.keyText ?? '');
  final fourk = TextEditingController(text: s0?.fourkBase ?? '');
  var tab = 'hosts';

  await showDialog<void>(
    context: context,
    builder: (ctx) => StatefulBuilder(
      builder: (ctx, setLocal) => AnimatedBuilder(
        animation: controller,
        builder: (ctx, _) {
          final s = controller.state?.stream;
          final t = ctx.tokens;
          if (s == null) return const SizedBox.shrink();
          return AlertDialog(
            backgroundColor: t.modalSolid,
            title: const Text('Stream Setting'),
            content: SizedBox(
              width: 500,
              height: 430,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Row(
                    children: [
                      for (final id in const [
                        'hosts',
                        'key',
                        'source',
                        'player'
                      ])
                        Padding(
                          padding: const EdgeInsets.only(right: 6),
                          child: VideoTab(
                            label: id[0].toUpperCase() + id.substring(1),
                            active: tab == id,
                            hue: Tokens.secVideos,
                            onTap: () {
                              setLocal(() => tab = id);
                              if (id == 'source') {
                                controller
                                    .send(const VideosCmd.streamSourceLoad());
                              }
                            },
                          ),
                        ),
                    ],
                  ),
                  const SizedBox(height: 14),
                  Expanded(
                    child: switch (tab) {
                      'hosts' => _HostsTab(
                          controller: controller, stream: s, text: hosts),
                      'key' =>
                        _KeyTab(controller: controller, stream: s, text: key),
                      'source' => _SourceTab(
                          controller: controller, stream: s, text: fourk),
                      _ => _PlayerTab(controller: controller, stream: s),
                    },
                  ),
                ],
              ),
            ),
            actions: [
              TextButton(
                  onPressed: () => Navigator.pop(ctx),
                  child: const Text('Close')),
            ],
          );
        },
      ),
    ),
  );
  hosts.dispose();
  key.dispose();
  fourk.dispose();
}

class _HostsTab extends StatelessWidget {
  const _HostsTab({
    required this.controller,
    required this.stream,
    required this.text,
  });

  final VideosController controller;
  final StreamView stream;
  final TextEditingController text;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text('One server per line · tried top to bottom',
            style: TextStyle(fontSize: 11, color: t.nInk3)),
        const SizedBox(height: 8),
        Expanded(
          child: TextField(
            controller: text,
            maxLines: null,
            expands: true,
            style: const TextStyle(fontSize: 13, fontFamily: 'monospace'),
            decoration: const InputDecoration(
                border: OutlineInputBorder(), isDense: true),
          ),
        ),
        if (stream.hostsError.isNotEmpty) ...[
          const SizedBox(height: 8),
          Text(stream.hostsError,
              style: const TextStyle(fontSize: 12, color: cErr)),
        ],
        const SizedBox(height: 10),
        Row(
          children: [
            PlexButton(
              compact: true,
              icon: Icons.save_outlined,
              label: 'Save',
              hue: cPlay,
              filled: true,
              onTap: () =>
                  controller.send(VideosCmd.streamHostsSave(text: text.text)),
            ),
            const SizedBox(width: 10),
            PlexButton(
              compact: true,
              icon: Icons.restart_alt,
              label: 'Reset',
              hue: cBack,
              onTap: () => controller
                  .send(const VideosCmd.streamHostsReset())
                  .then((_) => text.text =
                      controller.state?.stream.hostsText ?? text.text),
            ),
            const SizedBox(width: 10),
            // A dead server is only known to be dead once it times out, so
            // this takes a few seconds.
            PlexButton(
              compact: true,
              icon: Icons.network_check,
              label: stream.hostsChecking ? 'Testing…' : 'Test',
              hue: cCopy,
              onTap: () =>
                  controller.send(VideosCmd.streamHostsCheck(text: text.text)),
            ),
          ],
        ),
        if (stream.hostsSummary.isNotEmpty) ...[
          const SizedBox(height: 10),
          Text(stream.hostsSummary,
              style: TextStyle(
                  fontSize: 12, fontWeight: FontWeight.w600, color: t.nInk2)),
        ],
        if (stream.hostHealth.isNotEmpty) ...[
          const SizedBox(height: 8),
          SizedBox(
            height: 90,
            child: ListView(
              children: [
                for (final h in stream.hostHealth)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 4),
                    child: Row(
                      children: [
                        Container(
                          width: 7,
                          height: 7,
                          decoration: BoxDecoration(
                              color: h.ok ? cPlay : cErr,
                              shape: BoxShape.circle),
                        ),
                        const SizedBox(width: 8),
                        Expanded(
                          child: Text(h.host,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                  fontSize: 11,
                                  fontWeight: FontWeight.w600,
                                  color: t.nInk)),
                        ),
                        Text(h.note,
                            style: TextStyle(
                                fontSize: 11,
                                fontWeight: FontWeight.w700,
                                color: h.ok ? t.nInk2 : t.nInk3)),
                      ],
                    ),
                  ),
              ],
            ),
          ),
        ],
      ],
    );
  }
}

class _KeyTab extends StatelessWidget {
  const _KeyTab({
    required this.controller,
    required this.stream,
    required this.text,
  });

  final VideosController controller;
  final StreamView stream;
  final TextEditingController text;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          'Only change this if every server started failing after a MovieBox '
          'update. Paste the new key and save.',
          style: TextStyle(fontSize: 11, color: t.nInk3),
        ),
        const SizedBox(height: 10),
        TextField(
          controller: text,
          style: const TextStyle(fontSize: 13, fontFamily: 'monospace'),
          decoration: const InputDecoration(
              border: OutlineInputBorder(), isDense: true),
        ),
        if (stream.keyError.isNotEmpty) ...[
          const SizedBox(height: 8),
          Text(stream.keyError,
              style: const TextStyle(fontSize: 12, color: cErr)),
        ],
        const SizedBox(height: 12),
        Row(
          children: [
            PlexButton(
              icon: Icons.save_outlined,
              label: 'Save key',
              hue: cPlay,
              filled: true,
              compact: true,
              onTap: () =>
                  controller.send(VideosCmd.streamKeySave(key: text.text)),
            ),
            const SizedBox(width: 10),
            PlexButton(
              icon: Icons.restart_alt,
              label: 'Reset',
              hue: cBack,
              compact: true,
              onTap: () => controller
                  .send(const VideosCmd.streamKeyReset())
                  .then((_) => text.text =
                      controller.state?.stream.keyText ?? text.text),
            ),
          ],
        ),
      ],
    );
  }
}

class _SourceTab extends StatelessWidget {
  const _SourceTab({
    required this.controller,
    required this.stream,
    required this.text,
  });

  final VideosController controller;
  final StreamView stream;
  final TextEditingController text;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text('4KHDHub address · https:// only',
            style: TextStyle(fontSize: 11, color: t.nInk3)),
        const SizedBox(height: 8),
        TextField(
          controller: text,
          style: const TextStyle(fontSize: 13, fontFamily: 'monospace'),
          decoration: InputDecoration(
            border: const OutlineInputBorder(),
            isDense: true,
            hintText: stream.fourkDefault,
          ),
        ),
        if (stream.fourkError.isNotEmpty) ...[
          const SizedBox(height: 8),
          Text(stream.fourkError,
              style: const TextStyle(fontSize: 12, color: cErr)),
        ],
        const SizedBox(height: 12),
        Row(
          children: [
            PlexButton(
              compact: true,
              icon: Icons.save_outlined,
              label: 'Save',
              hue: cPlay,
              filled: true,
              onTap: () =>
                  controller.send(VideosCmd.streamFourkSave(url: text.text)),
            ),
            const SizedBox(width: 10),
            PlexButton(
              compact: true,
              icon: Icons.restart_alt,
              label: 'Reset',
              hue: cBack,
              onTap: () => controller
                  .send(const VideosCmd.streamFourkReset())
                  .then((_) =>
                      text.text = controller.state?.stream.fourkBase ?? ''),
            ),
          ],
        ),
        const SizedBox(height: 14),
        Text(
          'MovieBox is a signed API and needs no address here — its servers '
          'are on the Hosts tab. 4KHDHub is a scraped site: if the page '
          'layout changes, results stop until the port catches up.',
          style: TextStyle(fontSize: 11, color: t.nInk3),
        ),
      ],
    );
  }
}

class _PlayerTab extends StatelessWidget {
  const _PlayerTab({required this.controller, required this.stream});

  final VideosController controller;
  final StreamView stream;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      children: [
        StreamToggle(
          label: 'Autoplay next episode',
          value: stream.autoplay,
          onChanged: (v) =>
              controller.send(VideosCmd.streamSetAutoplay(on_: v)),
        ),
        const SizedBox(height: 6),
        StreamToggle(
          label: 'Download into the library',
          hint: 'Otherwise downloads land in the Downloads folder.',
          value: stream.toLibrary,
          onChanged: (v) =>
              controller.send(VideosCmd.streamSetToLibrary(on_: v)),
        ),
        const SizedBox(height: 16),
        Text('Subtitles',
            style: TextStyle(
                fontSize: 12, fontWeight: FontWeight.w700, color: t.nInk2)),
        const SizedBox(height: 8),
        Row(
          children: [
            StepperPill(
              label: 'Size ${(stream.subScale * 100).round()}%',
              onMinus: () => controller.send(
                  VideosCmd.streamSetSubScale(value: stream.subScale - 0.1)),
              onPlus: () => controller.send(
                  VideosCmd.streamSetSubScale(value: stream.subScale + 0.1)),
            ),
            const SizedBox(width: 10),
            StepperPill(
              label: 'Offset ${stream.subDelay.toStringAsFixed(1)}s',
              onMinus: () => controller.send(
                  VideosCmd.streamSetSubDelay(value: stream.subDelay - 0.5)),
              onPlus: () => controller.send(
                  VideosCmd.streamSetSubDelay(value: stream.subDelay + 0.5)),
            ),
          ],
        ),
        const SizedBox(height: 10),
        Text('Subtitle size and timing apply the next time something plays.',
            style: TextStyle(fontSize: 11, color: t.nInk3)),
      ],
    );
  }
}
