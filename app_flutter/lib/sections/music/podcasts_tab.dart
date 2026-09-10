// Podcasts — Home, Subscribed and Downloads, plus the show page.
//
// Subscribing is one HTTP GET and one call into the domain crate's feed
// parser; everything after that is rows in podcasts.db. Playing an episode
// takes over the same player bar My Music was using, which is the whole point
// of the five tabs being one section.

import 'package:flutter/material.dart';

import '../../design/pick.dart';
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

    // The info card and the show notes are panels over whatever is behind
    // them, not routes: both are things you glance at and dismiss, and losing
    // your place in a grid to read three lines of description is the reason
    // the Slint ones are overlays too.
    return Stack(
      children: [
        if (st.podDetailOpen)
          _ShowPage(controller: controller, st: st)
        else
          Column(
            children: [
              // Keyed on the tab: each tab keeps its own filter, so the box
              // has to be rebuilt with that tab's text when you switch.
              _Header(
                key: ValueKey(st.podTab),
                controller: controller,
                st: st,
              ),
              Expanded(
                child: switch (st.podTab) {
                  'downloads' => _Downloads(controller: controller, st: st),
                  'subscribed' => _Subscribed(controller: controller, st: st),
                  'trends' => _Trends(controller: controller, st: st),
                  _ => _Home(controller: controller, st: st),
                },
              ),
            ],
          ),
        if (st.podInfo != null)
          _InfoCard(controller: controller, info: st.podInfo!),
        if (st.podTranscriptTitle.isNotEmpty)
          _TranscriptPanel(controller: controller, st: st),
      ],
    );
  }
}

class _Header extends StatefulWidget {
  const _Header({super.key, required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  State<_Header> createState() => _HeaderState();
}

class _HeaderState extends State<_Header> {
  late final TextEditingController _search =
      TextEditingController(text: widget.st.podQuery);

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
    final row = SizedBox(
      height: 52,
      child: Row(
        children: [
          const SizedBox(width: 20),
          for (final tab in const [
            ('home', 'Home', Icons.home_outlined),
            ('trends', 'Trends', Icons.trending_up),
            ('subscribed', 'Subscribed', Icons.podcasts),
            ('downloads', 'Downloads', Icons.download_outlined),
          ])
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 4),
              child: MusicChip(
                label: tab.$2,
                icon: tab.$3,
                active: st.podTab == tab.$1,
                tint: const Color(0xFFF97316),
                tint2: const Color(0xFFEC4899),
                badge: tab.$1 == 'subscribed' ? '${st.podTotal}' : null,
                onTap: () {
                  controller.send(MusicCmd.podSetTab(name: tab.$1));
                  // Building the directory is a network walk over three dozen
                  // feeds. The bridge caches it for the session, so this is a
                  // no-op on every visit after the first.
                  if (tab.$1 == 'trends') {
                    controller.send(const MusicCmd.podTrendsLoad());
                  }
                },
              ),
            ),
          const SizedBox(width: 16),
          _TabSort(controller: controller, st: st),
          const SizedBox(width: 10),
          SizedBox(
            width: 190,
            child: TextField(
              controller: _search,
              decoration: InputDecoration(
                isDense: true,
                prefixIcon: const Icon(Icons.search, size: 16),
                hintText: switch (st.podTab) {
                  'downloads' => 'Filter saved',
                  'trends' => 'Filter the directory',
                  _ => 'Filter shows',
                },
                border: const OutlineInputBorder(),
                suffixIcon: st.podQuery.isEmpty
                    ? null
                    : IconButton(
                        icon: const Icon(Icons.close, size: 14),
                        onPressed: () {
                          _search.clear();
                          controller.send(const MusicCmd.podSearch(query: ''));
                        },
                      ),
              ),
              onChanged: (q) => controller.send(MusicCmd.podSearch(query: q)),
            ),
          ),
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
          // Speed, cycled rather than a slider: podcast listening is a small
          // set of speeds you pick once, and Slint's header chip cycles the
          // same six.
          TextButton(
            onPressed: () {
              const steps = [1.0, 1.25, 1.5, 1.75, 2.0, 0.75];
              final i = steps.indexWhere((v) => (v - st.podSpeed).abs() < 0.01);
              final next = steps[(i + 1) % steps.length];
              controller.send(MusicCmd.podSetSpeed(speed: next));
            },
            child: Text('${st.podSpeed.toStringAsFixed(2)}×'),
          ),
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
            onPressed: () => addFeed(context, controller),
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
    // Refreshing forty feeds, or storing a six-hundred-episode back catalogue,
    // takes long enough that a button which only greys out reads as broken.
    //
    // A save in flight counts as busy too: the row it belongs to names the
    // episode, but that row is on one tab and one page, and the download runs
    // wherever you go. `pod_dl_title` is what makes this strip say *what* is
    // downloading rather than only how far in it is.
    final saving = st.podDlId >= 0 && st.podDlTitle.isNotEmpty;
    if (!st.podBusy && !saving) return row;
    final frac = st.podBusy ? st.podFrac : st.podDlFrac;
    final label = st.podBusy ? st.podStatus : 'Saving · ${st.podDlTitle}';
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        row,
        Padding(
          padding: const EdgeInsets.fromLTRB(20, 0, 20, 6),
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
              Flexible(
                child: Text(
                  label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11, color: t.nInk2),
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// The sort control for whichever tab is open. Each list sorts by something
/// different -- shows by name, downloads by when they were saved -- so one
/// shared chip row would have had to hide most of itself anyway.
class _TabSort extends StatelessWidget {
  const _TabSort({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final Map<String, String> modes;
    final String active;
    final void Function(String) send;
    switch (st.podTab) {
      case 'home':
        modes = const {
          'name': 'Name',
          'category': 'Category',
          'latest': 'Newest'
        };
        active = st.podHomeSort;
        send = (m) => controller.send(MusicCmd.podSetHomeSort(mode: m));
      case 'trends':
        modes = const {'name': 'Name', 'category': 'Category'};
        active = st.podTrendsSort;
        send = (m) => controller.send(MusicCmd.podSetTrendsSort(mode: m));
      case 'downloads':
        modes = const {
          'dl': 'Recently saved',
          'new': 'Newest',
          'old': 'Oldest'
        };
        active = st.podDlSort;
        send = (m) => controller.send(MusicCmd.podSetDlSort(mode: m));
      default:
        return const SizedBox.shrink();
    }
    return Row(
      children: [
        for (final e in modes.entries)
          Padding(
            padding: const EdgeInsets.only(right: 6),
            child: SortChip(
              label: e.value,
              active: active == e.key,
              onTap: () => send(e.key),
            ),
          ),
      ],
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
      final path = await pickFile(label: 'OPML', extensions: ['opml', 'xml']);
      if (path != null) await c.send(MusicCmd.podOpmlImport(path: path));
    case 'export':
      // A save location rather than a folder: the file does not exist yet, and
      // what it is called is part of what is being chosen.
      final path = await pickSaveLocation(
        suggestedName: 'podcasts.opml',
        label: 'OPML',
        extensions: ['opml'],
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

/// Subscribe by RSS URL. The header's `+ Add` while Podcasts is open.
Future<void> addFeed(BuildContext context, MusicController c) async {
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
  text.dispose();
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
        action: ('Add a feed', () => addFeed(context, controller)),
      );
    }
    // "Your shows" is the shows you PINNED, not the first page of the
    // subscription list. Before, Home and Subscribed drew the same rail from
    // the same rows and Home had no reason to exist.
    final pinned = st.podHome;
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
            for (final s in pinned)
              MusicCard(
                controller: controller,
                title: s.title,
                subtitle: s.author,
                artKind: 'podcast',
                artKey: s.art,
                fallback: Icons.podcasts,
                badge: s.unplayed > 0 ? '${s.unplayed}' : null,
                onTap: () => controller.send(MusicCmd.podOpen(podcastId: s.id)),
                onMenu: () => _podMenu(context, controller, s),
              ),
          ],
        ),
        if (pinned.isEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 0, 24, 8),
            child: Text(
              'Nothing pinned yet — open Subscribed and pin the shows you '
              'actually follow, and they will live here.',
              style: TextStyle(fontSize: 12, color: context.tokens.nInk2),
            ),
          ),
        // Pinning forty shows makes a rail nobody reaches the end of.
        if (st.podHomePages > 1)
          Pager(
            page: st.podHomePage,
            pages: st.podHomePages,
            onGo: (p) => controller.send(MusicCmd.podSetHomePage(page: p)),
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
        action: ('Add a feed', () => addFeed(context, controller)),
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
                  onMenu: () => _podMenu(context, controller, s),
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
              onPressed: () => confirmThen(
                context,
                controller,
                title: 'Delete every downloaded episode?',
                body: 'The audio files are removed from disk. The episodes '
                    'stay in their feeds and can be downloaded again.',
                action: 'Delete all',
                cmd: const MusicCmd.podClearDownloads(),
              ),
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

/// The baked directory. Not a search: thirty-six feeds chosen up front, so the
/// page has something to show on a first run when nothing is subscribed --
/// which is the one moment a podcast section is otherwise an empty box with an
/// "paste an RSS URL" prompt in it.
class _Trends extends StatelessWidget {
  const _Trends({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    if (st.podTrendsLoading && st.podTrends.isEmpty) {
      return const Center(child: CircularProgressIndicator());
    }
    if (st.podTrends.isEmpty) {
      return MusicEmpty(
        icon: Icons.trending_up,
        title: 'The directory did not load',
        body: 'Every feed in it was unreachable. It retries whenever you come '
            'back to this tab.',
        action: (
          'Try again',
          () => controller.send(const MusicCmd.podTrendsLoad())
        ),
      );
    }
    return Column(
      children: [
        Expanded(
          child: CardGrid(
            children: [
              for (final tr in st.podTrends)
                _TrendCard(controller: controller, trend: tr),
            ],
          ),
        ),
        Pager(
          page: st.podTrendsPage,
          pages: st.podTrendsPages,
          onGo: (p) => controller.send(MusicCmd.podSetTrendsPage(page: p)),
        ),
      ],
    );
  }
}

class _TrendCard extends StatelessWidget {
  const _TrendCard({required this.controller, required this.trend});

  final MusicController controller;
  final PodTrend trend;

  @override
  Widget build(BuildContext context) => MusicCard(
        controller: controller,
        title: trend.title,
        subtitle: trend.author.isEmpty ? 'Podcast' : trend.author,
        artKind: 'podcast',
        artKey: trend.art,
        fallback: Icons.podcasts,
        badge: trend.subscribed ? '✓' : null,
        // Tapping a card you have not subscribed to opens the info card, not a
        // subscription: the whole point of a directory is deciding.
        onTap: () => controller.send(
          MusicCmd.podInfoOpen(podcastId: -1, feedUrl: trend.feedUrl),
        ),
        onPlay: trend.subscribed
            ? null
            : () => controller
                .send(MusicCmd.podTrendSubscribe(feedUrl: trend.feedUrl)),
      );
}

/// Everything you can do to a subscribed show from a card.
Future<void> _podMenu(
    BuildContext context, MusicController c, PodcastShow s) async {
  final choice = await showDialog<String>(
    context: context,
    builder: (ctx) => SimpleDialog(
      title: Text(s.title, maxLines: 2, overflow: TextOverflow.ellipsis),
      children: [
        for (final o in [
          ('info', 'Show info'),
          ('pin', 'Pin to Home / unpin'),
          ('art', 'Choose artwork…'),
          ('refresh', 'Refresh this feed'),
          ('unsub', 'Unsubscribe…'),
        ])
          SimpleDialogOption(
            onPressed: () => Navigator.pop(ctx, o.$1),
            child: Text(o.$2),
          ),
      ],
    ),
  );
  if (choice == null) return;
  switch (choice) {
    case 'info':
      await c.send(MusicCmd.podInfoOpen(podcastId: s.id, feedUrl: s.feedUrl));
    case 'pin':
      await c.send(MusicCmd.podToggleHome(podcastId: s.id));
    case 'art':
      final path = await pickFile(
        label: 'Images',
        extensions: ['png', 'jpg', 'jpeg', 'webp', 'bmp'],
      );
      if (path != null) {
        await c.send(MusicCmd.podSetThumb(podcastId: s.id, path: path));
      }
    case 'refresh':
      await c.send(MusicCmd.podRefreshOne(podcastId: s.id));
    case 'unsub':
      if (!context.mounted) return;
      await confirmThen(
        context,
        c,
        title: 'Unsubscribe from this show?',
        body: '“${s.title}” and its episode list go. Anything you have '
            'downloaded from it stays on disk.',
        action: 'Unsubscribe',
        cmd: MusicCmd.podUnsubscribe(podcastId: s.id),
      );
  }
}

/// The read-only card behind a Trends tile, and behind "Show info" on a
/// subscription. Never loads the episode list -- it answers "what is this
/// show", and pulling a back catalogue to answer that is what made the first
/// version of this slow.
class _InfoCard extends StatelessWidget {
  const _InfoCard({required this.controller, required this.info});

  final MusicController controller;
  final PodInfo info;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Positioned.fill(
      child: ColoredBox(
        color: Colors.black54,
        child: GestureDetector(
          onTap: () => controller.send(const MusicCmd.podInfoClose()),
          child: Center(
            child: GestureDetector(
              onTap: () {},
              child: Container(
                width: 620,
                constraints: const BoxConstraints(maxHeight: 520),
                padding: const EdgeInsets.all(24),
                decoration: BoxDecoration(
                  color: t.modal,
                  borderRadius: BorderRadius.circular(Tokens.radiusLg),
                  border: Border.all(color: t.outline),
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        MusicArt(
                          controller: controller,
                          kind: 'podcast',
                          artKey: info.art,
                          size: 112,
                          radius: 12,
                          fallback: Icons.podcasts,
                        ),
                        const SizedBox(width: 18),
                        Expanded(
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              Text(info.title,
                                  maxLines: 2,
                                  overflow: TextOverflow.ellipsis,
                                  style: TextStyle(
                                      fontSize: 20,
                                      fontWeight: FontWeight.w700,
                                      color: t.nInk)),
                              const SizedBox(height: 4),
                              Text(
                                [
                                  info.author,
                                  info.category,
                                  if (info.episodes > 0)
                                    '${info.episodes} episodes',
                                  if (info.latest > 0) fmtDate(info.latest),
                                ].where((s) => s.isNotEmpty).join('  ·  '),
                                style: TextStyle(fontSize: 12, color: t.nInk2),
                              ),
                            ],
                          ),
                        ),
                        IconButton(
                          icon: const Icon(Icons.close),
                          tooltip: 'Close',
                          onPressed: () =>
                              controller.send(const MusicCmd.podInfoClose()),
                        ),
                      ],
                    ),
                    const SizedBox(height: 16),
                    Flexible(
                      child: SingleChildScrollView(
                        child: Text(
                          info.description.isEmpty
                              ? 'This feed carries no description.'
                              : info.description,
                          style: TextStyle(
                              fontSize: 12, height: 1.5, color: t.nInk2),
                        ),
                      ),
                    ),
                    const SizedBox(height: 18),
                    Row(
                      children: [
                        if (info.subscribed)
                          TextButton.icon(
                            icon: const Icon(Icons.label_outline, size: 16),
                            label: const Text('Category…'),
                            onPressed: () =>
                                _editCategory(context, controller, info),
                          ),
                        const Spacer(),
                        if (info.subscribed)
                          FilledButton.icon(
                            icon: const Icon(Icons.open_in_new, size: 16),
                            label: const Text('Open show'),
                            onPressed: () {
                              controller.send(const MusicCmd.podInfoClose());
                              controller.send(
                                  MusicCmd.podOpen(podcastId: info.podcastId));
                            },
                          )
                        else
                          FilledButton.icon(
                            icon: const Icon(Icons.add, size: 16),
                            label: const Text('Subscribe'),
                            onPressed: () {
                              controller.send(MusicCmd.podTrendSubscribe(
                                  feedUrl: info.feedUrl));
                              controller.send(const MusicCmd.podInfoClose());
                            },
                          ),
                      ],
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// The category is the only editable field on the card: it drives the filter
/// on Subscribed, and feeds routinely declare something useless or nothing.
Future<void> _editCategory(
    BuildContext context, MusicController c, PodInfo info) async {
  final text = TextEditingController(text: info.category);
  final next = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Category'),
      content: SizedBox(
        width: 320,
        child: TextField(
          controller: text,
          autofocus: true,
          decoration: const InputDecoration(hintText: 'News, Comedy, …'),
          onSubmitted: (v) => Navigator.pop(ctx, v),
        ),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx), child: const Text('Cancel')),
        FilledButton(
            onPressed: () => Navigator.pop(ctx, text.text),
            child: const Text('Save')),
      ],
    ),
  );
  text.dispose();
  if (next != null) {
    await c.send(MusicCmd.podInfoSetCategory(name: next.trim()));
  }
}

/// Show notes. A side panel rather than a dialog because it is read while the
/// episode plays, and the transport has to stay reachable.
class _TranscriptPanel extends StatelessWidget {
  const _TranscriptPanel({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Align(
      alignment: Alignment.centerRight,
      child: Container(
        width: 380,
        margin: const EdgeInsets.all(12),
        padding: const EdgeInsets.all(18),
        decoration: BoxDecoration(
          color: t.modal,
          borderRadius: BorderRadius.circular(Tokens.radiusLg),
          border: Border.all(color: t.outline),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text(st.podTranscriptTitle,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 14,
                          fontWeight: FontWeight.w700,
                          color: t.nInk)),
                ),
                IconButton(
                  icon: const Icon(Icons.close, size: 18),
                  tooltip: 'Close',
                  onPressed: () =>
                      controller.send(const MusicCmd.podTranscriptClose()),
                ),
              ],
            ),
            const SizedBox(height: 10),
            Expanded(
              child: SingleChildScrollView(
                child: SelectableText(
                  st.podTranscriptText,
                  style: TextStyle(fontSize: 12, height: 1.6, color: t.nInk2),
                ),
              ),
            ),
          ],
        ),
      ),
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
              // The same five actions the card menu has, because a show page
              // is where you are when you decide to pin or re-art one.
              TextButton.icon(
                icon: const Icon(Icons.push_pin_outlined, size: 16),
                label: const Text('Pin to Home'),
                onPressed: show == null
                    ? null
                    : () => controller
                        .send(MusicCmd.podToggleHome(podcastId: show.id)),
              ),
              TextButton.icon(
                icon: const Icon(Icons.image_outlined, size: 16),
                label: const Text('Artwork'),
                onPressed: show == null
                    ? null
                    : () async {
                        final path = await pickFile(
                          label: 'Images',
                          extensions: ['png', 'jpg', 'jpeg', 'webp', 'bmp'],
                        );
                        if (path != null) {
                          await controller.send(MusicCmd.podSetThumb(
                              podcastId: show.id, path: path));
                        }
                      },
              ),
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
                    : () => confirmThen(
                          context,
                          controller,
                          title: 'Unsubscribe from this show?',
                          body: '“${show.title}” and its episode list go. '
                              'Anything downloaded from it stays on disk.',
                          action: 'Unsubscribe',
                          cmd: MusicCmd.podUnsubscribe(podcastId: show.id),
                        ),
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
    final st = controller.state;
    final saving = st != null && st.podDlId == e.id;
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
                  // A save in flight owns the strip: an 80MB episode used to
                  // download with nothing on screen at all until it appeared.
                  if (saving)
                    Padding(
                      padding: const EdgeInsets.only(top: 6),
                      child: Row(
                        children: [
                          Expanded(
                            child: LinearProgressIndicator(
                              value: st.podDlFrac <= 0
                                  ? null
                                  : st.podDlFrac.clamp(0.0, 1.0),
                              minHeight: 3,
                              backgroundColor: t.nHair,
                              valueColor: const AlwaysStoppedAnimation(
                                  Color(0xFFF97316)),
                            ),
                          ),
                          const SizedBox(width: 8),
                          Text('${(st.podDlFrac * 100).round()}%',
                              style: TextStyle(fontSize: 10, color: t.nInk2)),
                        ],
                      ),
                    )
                  // Where you got to last time. Only drawn once there is
                  // something to draw — a bar at zero is noise on every row.
                  else if (e.positionS > 0 && e.durationS > 0)
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
            // Show notes. Feeds carry them on nearly every episode and they are
            // the only place a chapter list or a link ever appears, so the two
            // lines of preview above are not the whole of it.
            IconButton(
              iconSize: 18,
              tooltip: 'Show notes',
              icon: const Icon(Icons.notes_outlined),
              onPressed: () =>
                  controller.send(MusicCmd.podTranscript(episodeId: e.id)),
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
              tooltip: saving
                  ? 'Saving…'
                  : (saved ? 'Delete the saved copy' : 'Save for offline'),
              icon: Icon(saving
                  ? Icons.downloading
                  : (saved ? Icons.delete_outline : Icons.download_outlined)),
              onPressed: saving
                  ? null
                  : () => controller.send(
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
