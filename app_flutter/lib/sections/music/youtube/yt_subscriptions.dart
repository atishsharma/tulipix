// YouTube › Subscriptions: the channels on the left, what they uploaded on the
// right: every channel's newest video, or one channel's 30 newest.
//
// "New" is what a channel listed above the video that topped it when you last
// looked: a listing's dates are approximate ("3 days ago"), its order is not.
// Picking a channel shows its cached listing at once, fetches a fresh one
// behind it, and counts it as seen. Channels not refreshed for 12 hours are
// checked in the background once a session; Check for new videos (in the
// tab's menu) refreshes every channel. Neither marks any of them seen.

import 'package:flutter/material.dart';

import '../../../design/tokens.dart';
import '../../../src/rust/api/music.dart';
import '../music_controller.dart';
import '../music_dialogs.dart';
import '../music_widgets.dart';
import 'yt_card.dart';
import 'yt_format_sheet.dart';

/// Below this the list and the feed take turns instead of sharing the width.
const double _sideBySide = 860;

/// Videos per page of the feed, five across: All channels (one video a
/// channel), and one channel's 30 newest.
const int _perPageAll = 15, _perPageChannel = 15;

class YtSubscriptions extends StatefulWidget {
  const YtSubscriptions(
      {super.key, required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  State<YtSubscriptions> createState() => _YtSubscriptionsState();
}

class _YtSubscriptionsState extends State<YtSubscriptions> {
  /// Narrow windows only: the feed is showing rather than the list.
  bool _feed = false;

  /// The feed's page, 0-based. Back to the first on every selection.
  int _page = 0;

  void _select(String channelId) {
    widget.controller.send(MusicCmd.ytSubsSelect(channelId: channelId));
    setState(() {
      _feed = true;
      _page = 0;
    });
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return LayoutBuilder(builder: (context, box) {
      final wide = box.maxWidth >= _sideBySide;
      final list = _ChannelList(
        controller: widget.controller,
        st: widget.st,
        onSelect: _select,
      );
      final feed = _Feed(
        controller: widget.controller,
        st: widget.st,
        page: _page,
        onPage: (p) => setState(() => _page = p),
        onBack: wide ? null : () => setState(() => _feed = false),
      );
      if (!wide) return _feed ? feed : list;
      return Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SizedBox(width: 340, child: list),
          VerticalDivider(width: 1, color: t.nHair),
          Expanded(child: feed),
        ],
      );
    });
  }
}

/// 1234 → 1.2K, 4200000 → 4.2M.
String _compact(int n) {
  String one(double v, String unit) =>
      '${v >= 10 ? v.round() : v.toStringAsFixed(1)}$unit';
  if (n >= 1000000) return one(n / 1000000, 'M');
  if (n >= 1000) return one(n / 1000, 'K');
  return '$n';
}

class _ChannelList extends StatelessWidget {
  const _ChannelList({
    required this.controller,
    required this.st,
    required this.onSelect,
  });

  final MusicController controller;
  final MusicState st;
  final ValueChanged<String> onSelect;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = controller;
    final newOnPage = st.ytSubs.fold<int>(0, (n, s) => n + s.newVideos);
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 14, 16, 8),
          child: Wrap(
            spacing: 6,
            runSpacing: 6,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              for (final m in const [
                ('subscribers', 'Followers'),
                ('name', 'Name'),
              ])
                SortChip(
                  label: m.$2,
                  active: st.ytSubsSort == m.$1,
                  dir: st.ytSubsSort == m.$1
                      ? (st.ytSubsDir == 'desc' ? -1 : 1)
                      : 0,
                  onTap: () => st.ytSubsSort == m.$1
                      ? c.send(const MusicCmd.ytToggleSubsDir())
                      : c.send(MusicCmd.ytSetSubsSort(mode: m.$1)),
                ),
              // Channels you unsubscribed from stay in the table -- a Takeout
              // import brings in hundreds and unsubscribing is how you thin it
              // out, so getting back to one has to be possible.
              MusicChip(
                label:
                    st.ytSubsFilter == 'unsub' ? 'Unsubscribed' : 'Subscribed',
                icon: st.ytSubsFilter == 'unsub'
                    ? Icons.person_off_outlined
                    : Icons.how_to_reg,
                active: st.ytSubsFilter == 'unsub',
                minWidth: 0,
                tint: ytRose,
                tint2: const Color(0xFFF97316),
                onTap: () => c.send(const MusicCmd.ytToggleSubsFilter()),
              ),
            ],
          ),
        ),
        if (st.ytSubs.isEmpty)
          Expanded(
            child: MusicEmpty(
              icon: Icons.subscriptions_outlined,
              title: st.ytSubsFilter == 'unsub'
                  ? 'Nothing unsubscribed'
                  : 'No channels',
              body: 'Subscribe from a channel page, or use the ⋮ menu to add '
                  "one by URL or import Google Takeout's subscriptions.csv.",
            ),
          )
        else
          Expanded(
            child: ListView(
              padding: const EdgeInsets.symmetric(horizontal: 8),
              children: [
                _ChannelRow(
                  selected: st.ytSubsSel.isEmpty,
                  leading: Container(
                    width: 36,
                    height: 36,
                    decoration: BoxDecoration(
                      color: t.nChip,
                      shape: BoxShape.circle,
                    ),
                    child: Icon(Icons.subscriptions_outlined,
                        size: 18, color: t.nInk2),
                  ),
                  title: 'All channels',
                  subtitle: '${st.ytSubCount} subscribed',
                  fresh: newOnPage,
                  onTap: () => onSelect(''),
                ),
                for (final s in st.ytSubs)
                  _ChannelRow(
                    selected: st.ytSubsSel == s.channelId,
                    leading: MusicArt(
                      controller: c,
                      kind: 'yt',
                      artKey: s.avatar,
                      direct: s.avatar,
                      size: 36,
                      radius: 18,
                      fallback: Icons.person,
                    ),
                    title: s.title,
                    subtitle:
                        s.subs > 0 ? '${_compact(s.subs)} subscribers' : '',
                    fresh: s.newVideos,
                    pinned: st.ytHomeChannels.contains(s.channelId),
                    onTap: () => onSelect(s.channelId),
                    menu: _menu(s),
                  ),
              ],
            ),
          ),
        Pager(
          page: st.ytSubsPage,
          pages: st.ytSubsPages,
          onGo: (p) => c.send(MusicCmd.ytSetSubsPage(page: p)),
        ),
      ],
    );
  }

  Widget _menu(YtSub s) {
    final c = controller;
    final pinned = st.ytHomeChannels.contains(s.channelId);
    return PopupMenuButton<String>(
      tooltip: 'More',
      iconSize: 16,
      onSelected: (choice) => c.send(switch (choice) {
        'open' => MusicCmd.ytOpenChannel(channelId: s.channelId),
        'pin' => pinned
            ? MusicCmd.ytUnpinHome(channelId: s.channelId)
            : MusicCmd.ytPinHome(channelId: s.channelId),
        _ => s.subscribed
            ? MusicCmd.ytUnsub(channelId: s.channelId)
            : MusicCmd.ytSubscribe(channelId: s.channelId, title: s.title),
      }),
      itemBuilder: (_) => [
        const PopupMenuItem(value: 'open', child: Text('Open channel')),
        PopupMenuItem(
            value: 'pin',
            child: Text(pinned ? 'Unpin from Home' : 'Pin to Home')),
        PopupMenuItem(
            value: 'sub',
            child: Text(s.subscribed ? 'Unsubscribe' : 'Subscribe again')),
      ],
    );
  }
}

class _ChannelRow extends StatelessWidget {
  const _ChannelRow({
    required this.selected,
    required this.leading,
    required this.title,
    required this.subtitle,
    required this.fresh,
    required this.onTap,
    this.pinned = false,
    this.menu,
  });

  final bool selected;
  final Widget leading;
  final String title;
  final String subtitle;
  final int fresh;
  final bool pinned;
  final VoidCallback onTap;
  final Widget? menu;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 1),
      child: Material(
        color: selected ? ytRose.withValues(alpha: 0.12) : Colors.transparent,
        borderRadius: BorderRadius.circular(Tokens.radiusSm),
        child: InkWell(
          borderRadius: BorderRadius.circular(Tokens.radiusSm),
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(8, 6, 0, 6),
            child: Row(
              children: [
                leading,
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 13,
                              fontWeight:
                                  fresh > 0 ? FontWeight.w700 : FontWeight.w500,
                              color: selected ? ytRose : t.nInk)),
                      if (subtitle.isNotEmpty)
                        Text(subtitle,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 11, color: t.nInk3)),
                    ],
                  ),
                ),
                if (fresh > 0)
                  Container(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 7, vertical: 2),
                    decoration: BoxDecoration(
                      color: ytRose,
                      borderRadius: BorderRadius.circular(9),
                    ),
                    child: Text('$fresh new',
                        style: const TextStyle(
                            fontSize: 10,
                            fontWeight: FontWeight.w700,
                            color: Colors.white)),
                  )
                else if (pinned)
                  Tooltip(
                    message: 'Pinned to Home',
                    child: Icon(Icons.push_pin, size: 14, color: t.nInk3),
                  ),
                menu ?? const SizedBox(width: 8),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _Feed extends StatelessWidget {
  const _Feed({
    required this.controller,
    required this.st,
    required this.page,
    required this.onPage,
    this.onBack,
  });

  final MusicController controller;
  final MusicState st;
  final int page;
  final ValueChanged<int> onPage;
  final VoidCallback? onBack;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = controller;
    final all = st.ytSubsSel.isEmpty;
    final feed = st.ytSubsFeed;
    final newCount = st.ytSubsFeedNew.toInt();
    final perPage = all ? _perPageAll : _perPageChannel;
    final pages = (feed.length / perPage).ceil();
    // The feed can shrink under the page (a refresh, an unsubscribe).
    final at = pages == 0 ? 0 : page.clamp(0, pages - 1);
    final start = at * perPage;
    final shown = feed.skip(start).take(perPage).toList();
    // New videos lead the feed, so on any page they are the first few rows.
    final freshHere = (newCount - start).clamp(0, shown.length);
    final fresh = shown.take(freshHere).toList();
    final earlier = shown.skip(freshHere).toList();
    final sub = st.ytSubs.where((s) => s.channelId == st.ytSubsSel).firstOrNull;
    final title = all
        ? 'All channels'
        : sub?.title ?? feed.firstOrNull?.channel ?? 'Channel';

    return ListView(
      padding: const EdgeInsets.only(bottom: 32),
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 14, 24, 4),
          child: Row(
            children: [
              Expanded(
                child: Wrap(
                  spacing: 10,
                  runSpacing: 10,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: [
                    if (onBack != null)
                      IconButton(
                        tooltip: 'Channels',
                        icon: const Icon(Icons.arrow_back),
                        onPressed: onBack,
                      ),
                    Padding(
                      padding: EdgeInsets.only(left: onBack == null ? 8 : 0),
                      child: Text(title,
                          style: TextStyle(
                              fontSize: 20,
                              fontWeight: FontWeight.w700,
                              letterSpacing: -0.4,
                              color: t.nInk)),
                    ),
                    if (!all)
                      TextButton.icon(
                        style: musicQuietStyle(context),
                        icon: const Icon(Icons.open_in_new, size: 16),
                        label: const Text('Open channel'),
                        onPressed: () => c.send(
                            MusicCmd.ytOpenChannel(channelId: st.ytSubsSel)),
                      ),
                  ],
                ),
              ),
              if (pages > 1) ...[
                const SizedBox(width: 12),
                Text('${feed.length} videos',
                    style: TextStyle(fontSize: 12, color: t.nInk3)),
                const SizedBox(width: 12),
                Pager(page: at, pages: pages, onGo: onPage, compact: true),
              ],
              // All channels only: picking one channel already fetches its
              // listing. The fetch bar shows the check running.
              if (all) ...[
                const SizedBox(width: 12),
                TextButton.icon(
                  style: musicQuietStyle(context),
                  icon: const Icon(Icons.sync, size: 16),
                  label: const Text('Check for new videos'),
                  onPressed: st.ytFetchBusy
                      ? null
                      : () => c.send(const MusicCmd.ytCheckNew()),
                ),
              ],
              if (feed.isNotEmpty) ...[
                SizedBox(width: all ? 6 : 12),
                FilledButton.icon(
                  style: musicFilledStyle(fill: ytRose),
                  icon: const Icon(Icons.play_arrow_rounded, size: 18),
                  label: Text(fresh.isEmpty ? 'Play all' : 'Play new'),
                  onPressed: () => c.send(MusicCmd.ytPlayAll(
                    videoIds: [
                      for (final v in fresh.isEmpty ? feed : fresh) v.videoId
                    ],
                  )),
                ),
                const SizedBox(width: 6),
                FilledButton.icon(
                  style: ytDownloadStyle(),
                  icon: const Icon(Icons.download_rounded, size: 16),
                  label: const Text('Download all'),
                  onPressed: () => showFormatSheet(context, c, feed.first,
                      mode: FormatMode.download, batch: feed),
                ),
              ],
            ],
          ),
        ),
        if (feed.isEmpty)
          Padding(
            padding: const EdgeInsets.all(24),
            child: Text(
              all
                  ? 'Nothing listed yet. Pick a channel to load its uploads, '
                      'or use Check for new videos in the ⋮ menu.'
                  : 'Fetching this channel\'s uploads with yt-dlp…',
              style: TextStyle(fontSize: 13, color: t.nInk2),
            ),
          ),
        if (fresh.isNotEmpty) ...[
          _label(context, 'New · $newCount'),
          YtVideoGrid(controller: c, videos: fresh, maxColumns: 5),
        ],
        if (earlier.isNotEmpty) ...[
          if (fresh.isNotEmpty || all)
            _label(context, all ? 'Latest from each channel' : 'Earlier'),
          if (fresh.isEmpty && !all) const SizedBox(height: 12),
          YtVideoGrid(controller: c, videos: earlier, maxColumns: 5),
        ],
      ],
    );
  }

  Widget _label(BuildContext context, String text) => Padding(
        padding: const EdgeInsets.fromLTRB(24, 20, 24, 10),
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
