// Podcasts — Home, New, Subscribed, Trends and Downloads, plus the show page.
//
// Subscribing is one HTTP GET and one call into the domain crate's feed
// parser; everything after that is rows in podcasts.db. Playing an episode
// takes over the same player bar My Music was using, which is the whole point
// of the five tabs being one section.
//
// The shape of the tab is docs/listen-deck.html: resume is one click from
// Home, and every episode action sits on its own row rather than behind a
// menu. The queue is the docked side panel (see side_panel.dart), not a
// dialog, because it is read while something is playing.

import 'package:flutter/material.dart';

import '../../platform/pick.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';
import 'music_widgets.dart';

/// The tab's own gradient — the orange→pink pair ui/page_music.slint gives the
/// podcast sub-tabs, which is not the purple the section chip wears.
const Color kPodTint = Color(0xFFF97316);
const Color kPodTint2 = Color(0xFFEC4899);

/// The four filters every episode list offers, in the order the chips sit.
/// One list, because the New tab and a show page filter the same rows the same
/// way and two orderings would put "Unplayed" in two places.
const List<(String, String)> kEpFilters = [
  ('unplayed', 'Unplayed'),
  ('progress', 'In progress'),
  ('downloaded', 'Downloaded'),
  ('all', 'All'),
];

class PodcastsTab extends StatelessWidget {
  const PodcastsTab({super.key, required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    if (st == null) return const Center(child: CircularProgressIndicator());

    // The info card is a panel over whatever is behind it, not a route: it is
    // something you glance at and dismiss, and losing your place in a grid to
    // read three lines of description is the reason the Slint one is an
    // overlay too. Show notes are no longer one of these — they live in the
    // docked panel now, beside the list they belong to.
    return Stack(
      children: [
        Column(
          children: [
            _SubTabs(controller: controller, st: st),
            Expanded(
              child: st.podDetailOpen
                  ? _ShowPage(
                      key: ValueKey(st.podDetail?.id ?? -1),
                      controller: controller,
                      st: st,
                    )
                  : switch (st.podTab) {
                      'new' => _Inbox(controller: controller, st: st),
                      'downloads' => _Downloads(controller: controller, st: st),
                      'subscribed' =>
                        _Subscribed(controller: controller, st: st),
                      'trends' => _Trends(controller: controller, st: st),
                      _ => _Home(controller: controller, st: st),
                    },
            ),
          ],
        ),
        if (st.podInfo != null)
          _InfoCard(controller: controller, info: st.podInfo!),
      ],
    );
  }
}

// ------------------------------------------------------------ sub-tab row --

class _SubTabs extends StatelessWidget {
  const _SubTabs({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final row = SizedBox(
      height: 52,
      child: Row(
        children: [
          const SizedBox(width: 20),
          for (final tab in [
            ('home', 'Home', Icons.home_outlined, null),
            (
              'new',
              'New',
              Icons.inbox_outlined,
              st.podNewCount > 0 ? '${st.podNewCount}' : null
            ),
            (
              'subscribed',
              'Subscribed',
              Icons.podcasts,
              st.podSubCount > 0 ? '${st.podSubCount}' : null
            ),
            ('trends', 'Trends', Icons.trending_up, null),
            (
              'downloads',
              'Downloads',
              Icons.download_outlined,
              st.podDlCount > 0 ? '${st.podDlCount}' : null
            ),
          ])
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 4),
              child: MusicChip(
                label: tab.$2,
                icon: tab.$3,
                active: st.podTab == tab.$1 && !st.podDetailOpen,
                tint: kPodTint,
                tint2: kPodTint2,
                badge: tab.$4,
                onTap: () {
                  // Just the one command: picking a tab leaves the show page on
                  // the bridge's side. Sending `podBack` alongside this raced
                  // it, and the loser's snapshot was the one that stuck.
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
          const Spacer(),
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
          const SizedBox(width: 8),
          IconButton(
            iconSize: 18,
            tooltip: 'Queue and show notes (Q)',
            isSelected: controller.panel == 'queue',
            icon: const Icon(Icons.queue_music_outlined),
            onPressed: () => controller.setPanel('queue'),
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
                    valueColor: const AlwaysStoppedAnimation(kPodTint),
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

/// A toolbar strip: a row of chips over a list, with room for a search box and
/// whatever bulk actions the list has. Every podcast page but Home has one, so
/// the padding and the wrap live here rather than five times over.
class _Toolbar extends StatelessWidget {
  const _Toolbar({required this.children, this.top = 10});

  final List<Widget> children;
  final double top;

  @override
  Widget build(BuildContext context) => Padding(
        padding: EdgeInsets.fromLTRB(24, top, 24, 4),
        child: Wrap(
          spacing: 6,
          runSpacing: 6,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: children,
        ),
      );
}

/// A heading over a shelf: the title, an aside, and an optional link out.
class _Head extends StatelessWidget {
  const _Head({required this.title, this.hint = '', this.action, this.trailing});

  final String title;
  final String hint;
  final (String, VoidCallback)? action;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 6),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.baseline,
        textBaseline: TextBaseline.alphabetic,
        children: [
          Text(title,
              style: TextStyle(
                  fontSize: 15, fontWeight: FontWeight.w700, color: t.nInk)),
          if (hint.isNotEmpty) ...[
            const SizedBox(width: 10),
            Text(hint, style: TextStyle(fontSize: 11.5, color: t.nInk3)),
          ],
          const Spacer(),
          if (trailing != null) trailing!,
          if (action != null)
            TextButton(onPressed: action!.$2, child: Text(action!.$1)),
        ],
      ),
    );
  }
}

/// The search box every list on this tab carries. Its own State so the caret
/// does not jump on the snapshot the keystroke produced.
class _Search extends StatefulWidget {
  const _Search({
    super.key,
    required this.value,
    required this.hint,
    required this.onChanged,
  });

  final String value;
  final String hint;
  final ValueChanged<String> onChanged;

  @override
  State<_Search> createState() => _SearchState();
}

class _SearchState extends State<_Search> {
  late final TextEditingController _c = TextEditingController(text: widget.value);

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => SizedBox(
        width: 220,
        height: 34,
        child: TextField(
          controller: _c,
          style: const TextStyle(fontSize: 12.5),
          decoration: InputDecoration(
            isDense: true,
            contentPadding: const EdgeInsets.symmetric(vertical: 8),
            prefixIcon: const Icon(Icons.search, size: 16),
            prefixIconConstraints:
                const BoxConstraints(minWidth: 32, minHeight: 32),
            hintText: widget.hint,
            border: const OutlineInputBorder(),
            suffixIcon: _c.text.isEmpty
                ? null
                : IconButton(
                    tooltip: 'Clear the search',
                    icon: const Icon(Icons.close, size: 14),
                    onPressed: () {
                      _c.clear();
                      widget.onChanged('');
                      setState(() {});
                    },
                  ),
          ),
          onChanged: (q) {
            widget.onChanged(q);
            setState(() {});
          },
        ),
      );
}

// ------------------------------------------------------------------- Home --

class _Home extends StatelessWidget {
  const _Home({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    if (st.podShows.isEmpty && st.podHome.isEmpty) {
      return MusicEmpty(
        icon: Icons.podcasts_outlined,
        title: 'No podcasts yet',
        body: 'Paste a feed URL and Tulipix will pull the show, its artwork '
            'and its back catalogue into podcasts.db.',
        action: ('Add a feed', () => addFeed(context, controller)),
      );
    }
    final t = context.tokens;
    // "Your shows" is the shows you PINNED, not the first page of the
    // subscription list. Before, Home and Subscribed drew the same rail from
    // the same rows and Home had no reason to exist.
    final pinned = st.podHome;
    return ListView(
      children: [
        if (st.podContinue.isNotEmpty) ...[
          _Head(
            title: 'Continue listening',
            hint: '${st.podContinue.length} in progress',
            action: (
              'See all',
              () async {
                // Awaited: two dispatches in flight at once race, and the one
                // that lands last is the snapshot the page keeps.
                await controller.send(const MusicCmd.podSetTab(name: 'new'));
                await controller
                    .send(const MusicCmd.podSetInboxFilter(name: 'progress'));
              }
            ),
          ),
          SizedBox(
            height: 92,
            child: ListView.separated(
              scrollDirection: Axis.horizontal,
              padding: const EdgeInsets.symmetric(horizontal: 24),
              itemCount: st.podContinue.length,
              separatorBuilder: (_, __) => const SizedBox(width: 12),
              itemBuilder: (_, i) => _ContinueCard(
                controller: controller,
                episode: st.podContinue[i],
              ),
            ),
          ),
        ],
        if (st.podLatest.isNotEmpty) ...[
          _Head(
            title: 'New from your shows',
            hint: 'newest episode of each',
            trailing: TextButton.icon(
              icon: const Icon(Icons.playlist_play, size: 16),
              label: const Text('Queue all'),
              onPressed: () =>
                  controller.send(const MusicCmd.podQueueAll(podcastId: -1)),
            ),
            action: (
              'See all',
              () => controller.send(const MusicCmd.podSetTab(name: 'new'))
            ),
          ),
          for (final e in st.podLatest)
            EpisodeRow(controller: controller, episode: e),
        ],
        _Head(
          title: 'Your shows',
          hint: 'pinned',
          action: (
            'Subscribed',
            () => controller.send(const MusicCmd.podSetTab(name: 'subscribed'))
          ),
        ),
        if (pinned.isEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 0, 24, 8),
            child: Text(
              'Nothing pinned yet — open Subscribed and pin the shows you '
              'actually follow, and they will live here.',
              style: TextStyle(fontSize: 12, color: t.nInk2),
            ),
          )
        else
          CardGrid(
            inList: true,
            padding: const EdgeInsets.fromLTRB(24, 8, 24, 8),
            min: 190,
            children: [
              for (final s in pinned)
                _ShowCard(controller: controller, show: s),
            ],
          ),
        // Pinning forty shows makes a grid nobody reaches the end of.
        if (st.podHomePages > 1)
          Pager(
            page: st.podHomePage,
            pages: st.podHomePages,
            onGo: (p) => controller.send(MusicCmd.podSetHomePage(page: p)),
          ),
        const SizedBox(height: 24),
      ],
    );
  }
}

/// One part-heard episode on Home's first shelf: art, title, show, time left
/// and how far in. The whole tile is the resume button — the point of the row
/// is that carrying on is one click from opening the section.
class _ContinueCard extends StatelessWidget {
  const _ContinueCard({required this.controller, required this.episode});

  final MusicController controller;
  final Episode episode;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final e = episode;
    final part =
        e.durationS > 0 ? (e.positionS / e.durationS).clamp(0.0, 1.0) : 0.0;
    return SizedBox(
      width: 320,
      child: Material(
        color: t.nCard,
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        child: InkWell(
          borderRadius: BorderRadius.circular(Tokens.radiusMd),
          onTap: () => controller.send(MusicCmd.podPlay(episodeId: e.id)),
          child: Padding(
            padding: const EdgeInsets.all(10),
            child: Row(
              children: [
                MusicArt(
                  controller: controller,
                  kind: 'podcast',
                  artKey: e.art,
                  size: 56,
                  radius: 8,
                  fallback: Icons.podcasts,
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(e.title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 12.5,
                              fontWeight: FontWeight.w600,
                              color: t.nInk)),
                      const SizedBox(height: 2),
                      Text(
                        '${e.show_} · ${fmtMins(e.durationS - e.positionS)} left',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11, color: t.nInk2),
                      ),
                      const SizedBox(height: 8),
                      ClipRRect(
                        borderRadius: BorderRadius.circular(2),
                        child: LinearProgressIndicator(
                          value: part,
                          minHeight: 3,
                          backgroundColor: t.nHair,
                          valueColor: const AlwaysStoppedAnimation(kPodTint),
                        ),
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 6),
                const Icon(Icons.play_arrow, size: 20, color: kPodTint),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// A subscribed show as a tile: the unplayed count is the badge, and the play
/// button on hover starts the newest episode rather than opening the page.
class _ShowCard extends StatelessWidget {
  const _ShowCard({required this.controller, required this.show});

  final MusicController controller;
  final PodcastShow show;

  @override
  Widget build(BuildContext context) => MusicCard(
        controller: controller,
        title: show.title,
        subtitle: [show.author, show.category]
            .where((s) => s.isNotEmpty)
            .join(' · '),
        artKind: 'podcast',
        artKey: show.art,
        fallback: Icons.podcasts,
        badge: show.unplayed > 0 ? '${show.unplayed}' : null,
        onTap: () => controller.send(MusicCmd.podOpen(podcastId: show.id)),
        onPlay: () =>
            controller.send(MusicCmd.podPlayLatest(podcastId: show.id)),
        onMenu: () => _podMenu(context, controller, show),
      );
}

// -------------------------------------------------------------- New tab ----

/// Everything the subscriptions posted in the last three weeks, in one list,
/// with the filters and the two bulk actions that make an inbox an inbox.
class _Inbox extends StatelessWidget {
  const _Inbox({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final counts = st.podInboxCounts;
    final left = st.podInbox.fold<double>(
        0, (n, e) => n + (e.durationS - e.positionS).clamp(0.0, e.durationS));
    return Column(
      children: [
        _Head(
          title: 'New episodes',
          hint: st.podInbox.isEmpty
              ? 'the last three weeks'
              : '${st.podInbox.length} episodes · ${fmtMins(left)}',
        ),
        _Toolbar(
          top: 0,
          children: [
            for (var i = 0; i < kEpFilters.length; i++)
              SortChip(
                label: i < counts.length
                    ? '${kEpFilters[i].$2}  ${counts[i]}'
                    : kEpFilters[i].$2,
                active: st.podInboxFilter == kEpFilters[i].$1,
                onTap: () => controller
                    .send(MusicCmd.podSetInboxFilter(name: kEpFilters[i].$1)),
              ),
            const SizedBox(width: 8),
            TextButton.icon(
              icon: const Icon(Icons.playlist_add, size: 16),
              label: const Text('Queue all'),
              onPressed: st.podInbox.isEmpty
                  ? null
                  : () =>
                      controller.send(const MusicCmd.podQueueAll(podcastId: -1)),
            ),
            TextButton.icon(
              icon: const Icon(Icons.done_all, size: 16),
              label: const Text('Mark all played'),
              onPressed: st.podInbox.isEmpty
                  ? null
                  : () => confirmThen(
                        context,
                        controller,
                        title: 'Mark these played?',
                        body: '${st.podInbox.length} episodes leave the inbox. '
                            'Anything downloaded from them stays on disk '
                            'unless "delete when played" is on.',
                        action: 'Mark played',
                        cmd: const MusicCmd.podMarkAllPlayed(podcastId: -1),
                      ),
            ),
          ],
        ),
        Expanded(
          child: st.podInbox.isEmpty
              ? MusicEmpty(
                  icon: Icons.inbox_outlined,
                  title: 'You are caught up',
                  body: st.podInboxFilter == 'unplayed'
                      ? 'Nothing unplayed in the last three weeks. Refresh the '
                          'feeds, or look at All.'
                      : 'Nothing in the last three weeks matches this filter.',
                  action: st.podInboxFilter == 'all'
                      ? null
                      : (
                          'Show all',
                          () => controller.send(
                              const MusicCmd.podSetInboxFilter(name: 'all'))
                        ),
                )
              : ListView.builder(
                  itemCount: st.podInbox.length,
                  itemBuilder: (_, i) => EpisodeRow(
                    controller: controller,
                    episode: st.podInbox[i],
                  ),
                ),
        ),
        SizedBox(height: 1, child: ColoredBox(color: t.nHair)),
      ],
    );
  }
}

// ----------------------------------------------------------- Subscribed ----

class _Subscribed extends StatelessWidget {
  const _Subscribed({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        _Toolbar(
          children: [
            for (final c in st.podCategories)
              SortChip(
                label: c,
                active: st.podCat == c,
                onTap: () => controller.send(MusicCmd.podSetCat(name: c)),
              ),
            const SizedBox(width: 8),
            for (final m in const [
              ('name', 'Name'),
              ('latest', 'Updated'),
              ('category', 'Category'),
            ])
              SortChip(
                label: m.$2,
                active: st.podHomeSort == m.$1,
                onTap: () =>
                    controller.send(MusicCmd.podSetHomeSort(mode: m.$1)),
              ),
          ],
        ),
        Expanded(
          child: st.podShows.isEmpty
              ? MusicEmpty(
                  icon: Icons.podcasts_outlined,
                  title: 'Nothing here',
                  body: 'Pick another category, clear the search, or add a '
                      'feed.',
                  action: ('Add a feed', () => addFeed(context, controller)),
                )
              : CardGrid(
                  min: 190,
                  children: [
                    for (final s in st.podShows)
                      _ShowCard(controller: controller, show: s),
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

// -------------------------------------------------------------- Downloads --

class _Downloads extends StatelessWidget {
  const _Downloads({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        _Storage(controller: controller, st: st),
        _Toolbar(
          children: [
            for (final m in const [
              ('dl', 'Recently saved'),
              ('new', 'Newest'),
              ('old', 'Oldest'),
            ])
              SortChip(
                label: m.$2,
                active: st.podDlSort == m.$1,
                onTap: () => controller.send(MusicCmd.podSetDlSort(mode: m.$1)),
              ),
            const SizedBox(width: 8),
            TextButton.icon(
              icon: const Icon(Icons.cleaning_services_outlined, size: 16),
              label: const Text('Remove played'),
              onPressed: () => confirmThen(
                context,
                controller,
                title: 'Remove the played downloads?',
                body: 'Every saved file whose episode is marked played is '
                    'deleted from disk. The episodes stay in their feeds.',
                action: 'Remove played',
                cmd: const MusicCmd.podRemovePlayedDownloads(),
              ),
            ),
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
          ],
        ),
        Expanded(
          child: st.podDownloads.isEmpty
              ? const MusicEmpty(
                  icon: Icons.download_outlined,
                  title: 'Nothing saved offline',
                  body: 'Download an episode from a show page and it lands '
                      'here, playable with no network at all.',
                )
              : ListView.builder(
                  itemCount: st.podDownloads.length,
                  itemBuilder: (_, i) => EpisodeRow(
                    controller: controller,
                    episode: st.podDownloads[i],
                  ),
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

/// What the saved episodes take, against the ceiling you set, and the two
/// rules that keep them under it.
class _Storage extends StatelessWidget {
  const _Storage({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  static const List<int> _limits = [0, 2, 6, 12, 32];

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final gb = st.podDlBytes / (1024 * 1024 * 1024);
    final limit = st.podDlLimitGb;
    final frac = limit <= 0 ? 0.0 : (gb / limit).clamp(0.0, 1.0);
    void prefs({int? limitGb, bool? whenPlayed, bool? wifiOnly}) =>
        controller.send(MusicCmd.podDownloadPrefs(
          limitGb: limitGb ?? st.podDlLimitGb,
          whenPlayed: whenPlayed ?? st.podDlWhenPlayed,
          wifiOnly: wifiOnly ?? st.podDlWifiOnly,
        ));
    return Container(
      margin: const EdgeInsets.fromLTRB(24, 12, 24, 0),
      padding: const EdgeInsets.fromLTRB(16, 14, 16, 14),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        border: Border.all(color: t.nHair),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(gb >= 1 ? '${gb.toStringAsFixed(2)} GB' : '${(gb * 1024).round()} MB',
                  style: TextStyle(
                      fontSize: 20,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
              Text(limit <= 0 ? 'no limit set' : 'of a $limit GB limit',
                  style: TextStyle(fontSize: 11.5, color: t.nInk2)),
            ],
          ),
          const SizedBox(width: 20),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                ClipRRect(
                  borderRadius: BorderRadius.circular(4),
                  child: LinearProgressIndicator(
                    value: limit <= 0 ? 0.0 : frac,
                    minHeight: 8,
                    backgroundColor: t.nTile,
                    valueColor: AlwaysStoppedAnimation(
                        frac > 0.9 ? Tokens.error : kPodTint),
                  ),
                ),
                const SizedBox(height: 10),
                Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: [
                    Text('Limit',
                        style: TextStyle(fontSize: 11.5, color: t.nInk3)),
                    for (final v in _limits)
                      SortChip(
                        label: v == 0 ? 'None' : '$v GB',
                        active: limit == v,
                        onTap: () => prefs(limitGb: v),
                      ),
                  ],
                ),
              ],
            ),
          ),
          const SizedBox(width: 20),
          Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              _Switch(
                label: 'Delete when played',
                value: st.podDlWhenPlayed,
                onChanged: (v) => prefs(whenPlayed: v),
              ),
              _Switch(
                label: 'Auto-download on Wi-Fi only',
                value: st.podDlWifiOnly,
                onChanged: (v) => prefs(wifiOnly: v),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

/// A labelled switch at the size the panels here want. Material's SwitchListTile
/// is a 56px row; these sit four to a card.
class _Switch extends StatelessWidget {
  const _Switch({
    required this.label,
    required this.value,
    required this.onChanged,
  });

  final String label;
  final bool value;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      borderRadius: BorderRadius.circular(6),
      onTap: () => onChanged(!value),
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 3, horizontal: 4),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Transform.scale(
              scale: 0.7,
              child: Switch(value: value, onChanged: onChanged),
            ),
            const SizedBox(width: 4),
            Text(label, style: TextStyle(fontSize: 12, color: t.nInk2)),
          ],
        ),
      ),
    );
  }
}

// ----------------------------------------------------------------- Trends --

/// The baked directory. Not a search of the world: three dozen feeds chosen up
/// front, so the page has something to show on a first run when nothing is
/// subscribed — which is the one moment a podcast section is otherwise an
/// empty box with a "paste an RSS URL" prompt in it. The box over the grid
/// searches those, by name, author or category.
class _Trends extends StatelessWidget {
  const _Trends({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    if (st.podTrendsLoading && st.podTrends.isEmpty) {
      return const Center(child: CircularProgressIndicator());
    }
    return Column(
      children: [
        _Toolbar(
          children: [
            for (final m in const [('name', 'Name'), ('category', 'Category')])
              SortChip(
                label: m.$2,
                active: st.podTrendsSort == m.$1,
                onTap: () =>
                    controller.send(MusicCmd.podSetTrendsSort(mode: m.$1)),
              ),
          ],
        ),
        Expanded(
          child: st.podTrends.isEmpty
              ? MusicEmpty(
                  icon: Icons.trending_up,
                  title: st.podQuery.isEmpty
                      ? 'The directory did not load'
                      : 'Nothing matches',
                  body: st.podQuery.isEmpty
                      ? 'Every feed in it was unreachable. It retries whenever '
                          'you come back to this tab.'
                      : 'The directory is three dozen shows, not the whole of '
                          'podcasting. Paste a feed URL for anything else.',
                  action: (
                    'Try again',
                    () => controller.send(const MusicCmd.podTrendsLoad())
                  ),
                )
              : CardGrid(
                  min: 190,
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
        subtitle: trend.subscribed
            ? 'Subscribed'
            : (trend.author.isEmpty ? 'Preview before subscribing' : trend.author),
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

// -------------------------------------------------------------- show page --

class _ShowPage extends StatefulWidget {
  const _ShowPage({
    super.key,
    required this.controller,
    required this.st,
  });

  final MusicController controller;
  final MusicState st;

  @override
  State<_ShowPage> createState() => _ShowPageState();
}

class _ShowPageState extends State<_ShowPage> {
  bool _settings = false;

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    final st = widget.st;
    final t = context.tokens;
    final show = st.podDetail;
    final counts = st.podEpCounts;
    return ListView(
      padding: EdgeInsets.zero,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
          child: Row(
            children: [
              TextButton.icon(
                icon: const Icon(Icons.arrow_back, size: 18),
                label: const Text('Back'),
                onPressed: () => controller.send(const MusicCmd.podBack()),
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
                      if (show.category.isNotEmpty)
                        Text(show.category.toUpperCase(),
                            style: TextStyle(
                                fontSize: 10,
                                letterSpacing: 1.1,
                                fontWeight: FontWeight.w700,
                                color: t.nInk3)),
                      Text(show.title,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 22,
                              fontWeight: FontWeight.w700,
                              color: t.nInk)),
                      if (show.author.isNotEmpty)
                        Text(show.author,
                            style:
                                TextStyle(fontSize: 12.5, color: t.nInk2)),
                      const SizedBox(height: 6),
                      Text(
                        [
                          '${show.episodes} episodes',
                          '${show.unplayed} unplayed',
                          if (show.latest > 0)
                            'Updated ${fmtAgo(show.latest)}',
                        ].join('   ·   '),
                        style: TextStyle(fontSize: 11.5, color: t.nInk3),
                      ),
                      const SizedBox(height: 10),
                      Wrap(
                        spacing: 8,
                        runSpacing: 6,
                        crossAxisAlignment: WrapCrossAlignment.center,
                        children: [
                          FilledButton.icon(
                            icon: const Icon(Icons.play_arrow, size: 18),
                            label: const Text('Play latest'),
                            onPressed: show.episodes == 0
                                ? null
                                : () => controller.send(
                                    MusicCmd.podPlayLatest(podcastId: show.id)),
                          ),
                          OutlinedButton.icon(
                            icon: const Icon(Icons.playlist_add, size: 16),
                            label: const Text('Queue unplayed'),
                            onPressed: show.unplayed == 0
                                ? null
                                : () => controller.send(
                                    MusicCmd.podQueueAll(podcastId: show.id)),
                          ),
                          TextButton.icon(
                            icon: Icon(
                                show.pinned
                                    ? Icons.push_pin
                                    : Icons.push_pin_outlined,
                                size: 16,
                                color: show.pinned ? kPodTint : null),
                            label: Text(
                                show.pinned ? 'Pinned to Home' : 'Pin to Home'),
                            onPressed: () => controller
                                .send(MusicCmd.podToggleHome(podcastId: show.id)),
                          ),
                          TextButton.icon(
                            icon: Icon(
                                _settings ? Icons.tune : Icons.tune_outlined,
                                size: 16),
                            label: const Text('Settings'),
                            onPressed: () =>
                                setState(() => _settings = !_settings),
                          ),
                          TextButton.icon(
                            icon: const Icon(Icons.done_all, size: 16),
                            label: const Text('Mark all played'),
                            onPressed: show.unplayed == 0
                                ? null
                                : () => confirmThen(
                                      context,
                                      controller,
                                      title: 'Mark the whole show played?',
                                      body: '${show.unplayed} unplayed '
                                          'episodes of “${show.title}” are '
                                          'marked played.',
                                      action: 'Mark played',
                                      cmd: MusicCmd.podMarkAllPlayed(
                                          podcastId: show.id),
                                    ),
                          ),
                          IconButton(
                            iconSize: 18,
                            tooltip: 'Refresh this feed',
                            icon: const Icon(Icons.refresh),
                            onPressed: () => controller
                                .send(MusicCmd.podRefreshOne(podcastId: show.id)),
                          ),
                          IconButton(
                            iconSize: 18,
                            tooltip: 'More',
                            icon: const Icon(Icons.more_horiz),
                            onPressed: () => _podMenu(context, controller, show),
                          ),
                        ],
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        if (show != null && show.description.isNotEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 0, 24, 12),
            child: Text(
              show.description,
              maxLines: 4,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(fontSize: 12, height: 1.5, color: t.nInk2),
            ),
          ),
        if (_settings && show != null)
          _ShowSettings(controller: controller, show: show),
        _Toolbar(
          children: [
            for (var i = 0; i < kEpFilters.length; i++)
              SortChip(
                // `pod_ep_counts` is in the chips' own order — unplayed, in
                // progress, downloaded, all — so the number beside a filter is
                // the number of rows that filter would keep.
                label: i < counts.length
                    ? '${kEpFilters[i].$2}  ${counts[i]}'
                    : kEpFilters[i].$2,
                active: st.podEpFilter == kEpFilters[i].$1,
                onTap: () => controller
                    .send(MusicCmd.podSetEpFilter(name: kEpFilters[i].$1)),
              ),
            const SizedBox(width: 8),
            _Search(
              key: ValueKey('show-search-${show?.id ?? -1}'),
              value: st.podEpQuery,
              hint: 'Search this show',
              onChanged: (q) =>
                  controller.send(MusicCmd.podSearchEpisodes(query: q)),
            ),
            const SizedBox(width: 8),
            for (final m in const [('new', 'Newest first'), ('old', 'Oldest first')])
              SortChip(
                label: m.$2,
                active: (st.podEpSort == 'old') == (m.$1 == 'old'),
                onTap: () => controller.send(MusicCmd.podSetEpSort(mode: m.$1)),
              ),
          ],
        ),
        if (st.podEpisodes.isEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 32, 24, 32),
            child: Text(
              st.podEpFilter == 'all' && st.podEpQuery.isEmpty
                  ? 'No episodes stored. Refresh the feed to pull them in.'
                  : 'No episodes match this filter.',
              style: TextStyle(fontSize: 12.5, color: t.nInk2),
            ),
          )
        else
          for (final e in st.podEpisodes)
            EpisodeRow(
              controller: controller,
              episode: e,
              showName: false,
            ),
        Pager(
          page: st.podEpPage,
          pages: st.podEpPages,
          onGo: (p) => controller.send(MusicCmd.podSetEpPage(page: p)),
        ),
        const SizedBox(height: 16),
      ],
    );
  }
}

/// The four things a show can be told, written together: one panel, one save.
/// `speed = 0` is "follow the section", which is why the first chip is Section
/// and not 1.0×.
class _ShowSettings extends StatelessWidget {
  const _ShowSettings({required this.controller, required this.show});

  final MusicController controller;
  final PodcastShow show;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    void save({double? speed, int? intro, bool? auto, int? keep}) =>
        controller.send(MusicCmd.podShowSettings(
          podcastId: show.id,
          speed: speed ?? show.speed,
          skipIntroS: intro ?? show.skipIntroS,
          autoDl: auto ?? show.autoDl,
          keepLast: keep ?? show.keepLast,
        ));
    Widget row(String label, List<Widget> chips) => Padding(
          padding: const EdgeInsets.symmetric(vertical: 5),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              SizedBox(
                width: 150,
                child: Text(label,
                    style: TextStyle(fontSize: 12, color: t.nInk3)),
              ),
              Expanded(
                child: Wrap(
                    spacing: 6,
                    runSpacing: 6,
                    crossAxisAlignment: WrapCrossAlignment.center,
                    children: chips),
              ),
            ],
          ),
        );
    return Container(
      margin: const EdgeInsets.fromLTRB(24, 0, 24, 8),
      padding: const EdgeInsets.fromLTRB(16, 12, 16, 12),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        border: Border.all(color: t.nHair),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          row('Speed for this show', [
            SortChip(
              label: 'Section',
              active: show.speed <= 0,
              onTap: () => save(speed: 0.0),
            ),
            for (final v in const [1.0, 1.2, 1.3, 1.5, 1.8, 2.0])
              SortChip(
                label: '$v×',
                active: (show.speed - v).abs() < 0.01,
                onTap: () => save(speed: v),
              ),
          ]),
          row('Skip intro', [
            for (final v in const [0, 15, 30, 45, 60, 90])
              SortChip(
                label: v == 0 ? 'Off' : '$v s',
                active: show.skipIntroS == v,
                onTap: () => save(intro: v),
              ),
          ]),
          row('New episodes', [
            _Switch(
              label: 'Download automatically',
              value: show.autoDl,
              onChanged: (v) => save(auto: v),
            ),
          ]),
          row('Keep downloaded', [
            for (final v in const [(0, 'All'), (3, 'Last 3'), (5, 'Last 5'), (10, 'Last 10')])
              SortChip(
                label: v.$2,
                active: show.keepLast == v.$1,
                onTap: () => save(keep: v.$1),
              ),
          ]),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------- episode row ----

/// One episode: how far in it is, when it landed, how much is left, and the
/// five things you can do to it without opening anything.
///
/// The actions are on the row rather than behind a menu because every one of
/// them is a one-click decision made while skimming — queueing the next three,
/// burying one you will never play, saving one for a flight.
class EpisodeRow extends StatefulWidget {
  const EpisodeRow({
    super.key,
    required this.controller,
    required this.episode,
    this.showName = true,
  });

  final MusicController controller;
  final Episode episode;

  /// The show's name on the meta line. Off on a show page, where every row
  /// would say the same thing.
  final bool showName;

  @override
  State<EpisodeRow> createState() => _EpisodeRowState();
}

class _EpisodeRowState extends State<EpisodeRow> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final controller = widget.controller;
    final e = widget.episode;
    final st = controller.state;
    final playing =
        controller.now?.mode == 'podcast' && controller.now?.key == '${e.id}';
    final saved = e.downloaded.isNotEmpty;
    final saving = st != null && st.podDlId == e.id;
    final queued = st != null && st.podQueue.any((q) => q.id == e.id);
    final part =
        e.durationS > 0 ? (e.positionS / e.durationS).clamp(0.0, 1.0) : 0.0;
    final fresh = !e.played &&
        e.positionS == 0 &&
        e.published > 0 &&
        DateTime.now()
                .difference(
                    DateTime.fromMillisecondsSinceEpoch(e.published * 1000))
                .inDays <
            7;

    Widget act(String tip, IconData icon, VoidCallback? onTap,
            {bool on = false}) =>
        IconButton(
          iconSize: 18,
          tooltip: tip,
          isSelected: on,
          color: on ? kPodTint : null,
          icon: Icon(icon),
          onPressed: onTap,
        );

    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: Material(
        color: _hovered ? t.nHover : Colors.transparent,
        child: InkWell(
          onTap: () => controller.send(MusicCmd.podPlay(episodeId: e.id)),
          child: Opacity(
            // A played episode stays legible but stops competing: the list is
            // read for what is left, not for what is done.
            opacity: e.played ? 0.55 : 1,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(24, 10, 16, 10),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _PlayDisc(
                    controller: controller,
                    episode: e,
                    playing: playing,
                    progress: e.played ? 1.0 : part,
                  ),
                  const SizedBox(width: 14),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          children: [
                            Flexible(
                              child: Text(
                                [
                                  if (widget.showName && e.show_.isNotEmpty)
                                    e.show_,
                                  if (e.published > 0) fmtAgo(e.published),
                                  if (e.durationS > 0)
                                    e.positionS > 0 && !e.played
                                        ? '${fmtMins(e.durationS - e.positionS)} left'
                                        : fmtMins(e.durationS),
                                ].join('  ·  '),
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style:
                                    TextStyle(fontSize: 11, color: t.nInk2),
                              ),
                            ),
                            if (fresh) const _Tag(text: 'New', tint: kPodTint),
                            if (saved)
                              const _Tag(text: 'Saved', tint: Tokens.ok),
                          ],
                        ),
                        const SizedBox(height: 2),
                        Text(
                          e.title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            fontSize: 13,
                            fontWeight:
                                playing ? FontWeight.w700 : FontWeight.w600,
                            color: playing ? kPodTint : t.nInk,
                          ),
                        ),
                        if (e.description.isNotEmpty)
                          Padding(
                            padding: const EdgeInsets.only(top: 3),
                            child: Text(
                              e.description,
                              maxLines: 2,
                              overflow: TextOverflow.ellipsis,
                              style:
                                  TextStyle(fontSize: 11, color: t.nInk3),
                            ),
                          ),
                        // A save in flight owns the strip: an 80MB episode used
                        // to download with nothing on screen at all until it
                        // appeared.
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
                                    valueColor:
                                        const AlwaysStoppedAnimation(kPodTint),
                                  ),
                                ),
                                const SizedBox(width: 8),
                                Text('${(st.podDlFrac * 100).round()}%',
                                    style: TextStyle(
                                        fontSize: 10, color: t.nInk2)),
                              ],
                            ),
                          ),
                      ],
                    ),
                  ),
                  const SizedBox(width: 8),
                  act(
                    'Play next',
                    Icons.playlist_play,
                    () => controller
                        .send(MusicCmd.podQueueNext(episodeId: e.id)),
                  ),
                  act(
                    queued ? 'In the queue' : 'Add to the queue',
                    queued ? Icons.playlist_add_check : Icons.playlist_add,
                    () => controller.send(queued
                        ? MusicCmd.podQueueRemove(episodeId: e.id)
                        : MusicCmd.podQueueAdd(episodeId: e.id)),
                    on: queued,
                  ),
                  act(
                    saving
                        ? 'Saving…'
                        : (saved ? 'Delete the saved copy' : 'Save for offline'),
                    saving
                        ? Icons.downloading
                        : (saved
                            ? Icons.delete_outline
                            : Icons.download_outlined),
                    saving
                        ? null
                        : () => controller.send(
                              saved
                                  ? MusicCmd.podRemoveDownload(episodeId: e.id)
                                  : MusicCmd.podDownload(episodeId: e.id),
                            ),
                  ),
                  act(
                    e.played ? 'Mark unplayed' : 'Mark played',
                    e.played ? Icons.check_circle : Icons.check_circle_outline,
                    () => controller.send(
                        MusicCmd.podSetPlayed(episodeId: e.id, played: !e.played)),
                    on: e.played,
                  ),
                  act(
                    'Show notes',
                    Icons.notes_outlined,
                    () {
                      controller.send(MusicCmd.podTranscript(episodeId: e.id));
                      // The notes half of the docked panel, which shares the
                      // slot the library's lyrics use.
                      controller.showPanel('lyrics');
                    },
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// The play button with the episode's progress drawn round it — the one place
/// a list can say "you are 40% through this" without spending a row on a bar.
class _PlayDisc extends StatelessWidget {
  const _PlayDisc({
    required this.controller,
    required this.episode,
    required this.playing,
    required this.progress,
  });

  final MusicController controller;
  final Episode episode;
  final bool playing;
  final double progress;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      width: 44,
      height: 44,
      child: Stack(
        alignment: Alignment.center,
        children: [
          if (progress > 0.005)
            SizedBox(
              width: 44,
              height: 44,
              child: CircularProgressIndicator(
                value: progress,
                strokeWidth: 2.5,
                backgroundColor: t.nHair,
                valueColor: const AlwaysStoppedAnimation(kPodTint),
              ),
            ),
          IconButton(
            iconSize: 22,
            tooltip: playing ? 'Pause' : 'Play',
            icon: Icon(playing && controller.tickPlaying
                ? Icons.pause_circle_filled
                : Icons.play_circle_fill),
            color: playing ? kPodTint : t.nInk2,
            onPressed: () => playing
                ? controller.send(const MusicCmd.playPause())
                : controller.send(MusicCmd.podPlay(episodeId: episode.id)),
          ),
        ],
      ),
    );
  }
}

class _Tag extends StatelessWidget {
  const _Tag({required this.text, required this.tint});

  final String text;
  final Color tint;

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.only(left: 6),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
          decoration: BoxDecoration(
            color: tint.withValues(alpha: 0.16),
            borderRadius: BorderRadius.circular(4),
          ),
          child: Text(text,
              style: TextStyle(
                  fontSize: 9.5, fontWeight: FontWeight.w700, color: tint)),
        ),
      );
}

// --------------------------------------------------------------- dialogs ----

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
          ('pin', s.pinned ? 'Unpin from Home' : 'Pin to Home'),
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
                  color: t.modalSolid,
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
                            onPressed: () async {
                              await controller
                                  .send(const MusicCmd.podInfoClose());
                              await controller.send(
                                  MusicCmd.podOpen(podcastId: info.podcastId));
                            },
                          )
                        else ...[
                          // Hearing the newest episode is the honest way to
                          // decide, and it costs nothing: subscribing to find
                          // out and unsubscribing again is the thing this
                          // replaces.
                          OutlinedButton.icon(
                            icon: const Icon(Icons.play_arrow, size: 16),
                            label: const Text('Preview'),
                            onPressed: () => controller.send(
                                MusicCmd.podPreview(feedUrl: info.feedUrl)),
                          ),
                          const SizedBox(width: 8),
                          FilledButton.icon(
                            icon: const Icon(Icons.add, size: 16),
                            label: const Text('Subscribe'),
                            onPressed: () async {
                              await controller.send(MusicCmd.podTrendSubscribe(
                                  feedUrl: info.feedUrl));
                              await controller
                                  .send(const MusicCmd.podInfoClose());
                            },
                          ),
                        ],
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
