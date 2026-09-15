// Live TV: the channel grid, the categories sidebar, the now-playing pill and
// its sheet, and the iptv-org playlist picker.
//
// The paging, sorting and filtering all happen in Rust — the snapshot carries
// one page of forty-two cards, already narrowed and already ordered — so this
// file draws what it is given and sends the chips back.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../design/skin.dart';
import '../../src/rust/api/videos.dart';
import 'videos_controller.dart';
import 'videos_widgets.dart';

class VideosLiveTv extends StatelessWidget {
  const VideosLiveTv({
    super.key,
    required this.controller,
    required this.live,
  });

  final VideosController controller;
  final LiveView live;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      children: [
        if (live.total > 0) _SortBar(controller: controller, live: live),
        Expanded(
          child: Row(
            children: [
              Expanded(child: _Grid(controller: controller, live: live)),
              if (live.groups.isNotEmpty)
                _Categories(controller: controller, live: live),
            ],
          ),
        ),
        if (live.status.isNotEmpty)
          Container(
            height: 34,
            width: double.infinity,
            alignment: Alignment.centerLeft,
            padding: const EdgeInsets.symmetric(horizontal: 28),
            color: t.panel,
            child: Text(
              live.status,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(fontSize: 12, color: t.nInk2),
            ),
          ),
      ],
    );
  }
}

class _SortBar extends StatelessWidget {
  const _SortBar({required this.controller, required this.live});

  final VideosController controller;
  final LiveView live;

  void _sort(String key) =>
      controller.send(VideosCmd.liveSetSort(key: key, asc: live.asc));

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 54,
      color: t.panel,
      padding: const EdgeInsets.symmetric(horizontal: 28),
      child: Row(
        children: [
          Text('SORT',
              style: TextStyle(
                  fontSize: 9,
                  fontWeight: FontWeight.w800,
                  letterSpacing: 1.2,
                  color: t.nInk3)),
          const SizedBox(width: 8),
          SortChip(
              label: 'Playlist',
              hue: cInfo,
              active: live.sort == 'default',
              onTap: () => _sort('default')),
          const SizedBox(width: 6),
          SortChip(
              label: 'Name',
              hue: cDl,
              active: live.sort == 'name',
              onTap: () => _sort('name')),
          const SizedBox(width: 6),
          SortChip(
              label: 'Category',
              hue: cBack,
              active: live.sort == 'group',
              onTap: () => _sort('group')),
          const SizedBox(width: 6),
          // Read out of the "(720p)" iptv-org writes into the channel name —
          // the playlists carry it nowhere else.
          SortChip(
              label: 'Quality',
              hue: cPlay,
              active: live.sort == 'res',
              onTap: () => _sort('res')),
          const SizedBox(width: 8),
          IconBtn(
            icon: live.asc ? Icons.arrow_upward : Icons.arrow_downward,
            tip: live.asc ? 'Ascending' : 'Descending',
            size: 30,
            onTap: () => controller
                .send(VideosCmd.liveSetSort(key: live.sort, asc: !live.asc)),
          ),
          const Spacer(),
          Text(
            '${live.matched} ${live.matched == 1 ? 'channel' : 'channels'}'
            '${live.group.isEmpty ? '' : ' in ${live.group}'}',
            style: TextStyle(fontSize: 12, color: t.nInk2),
          ),
          if (live.pages > 1) ...[
            const SizedBox(width: 14),
            IconBtn(
              icon: Icons.chevron_left,
              size: 30,
              onTap: () => live.page > 0
                  ? controller.send(VideosCmd.liveSetPage(page: live.page - 1))
                  : null,
            ),
            const SizedBox(width: 8),
            Text('${live.page + 1} / ${live.pages}',
                style: TextStyle(
                    fontSize: 12, fontWeight: FontWeight.w700, color: t.nInk2)),
            const SizedBox(width: 8),
            IconBtn(
              icon: Icons.chevron_right,
              size: 30,
              onTap: () => live.page + 1 < live.pages
                  ? controller.send(VideosCmd.liveSetPage(page: live.page + 1))
                  : null,
            ),
          ],
        ],
      ),
    );
  }
}

class _Grid extends StatelessWidget {
  const _Grid({required this.controller, required this.live});

  final VideosController controller;
  final LiveView live;

  @override
  Widget build(BuildContext context) {
    if (live.pickedCount == 0 && !live.busy) {
      return VideosEmpty(
        icon: Icons.tv,
        title: 'Pick some playlists',
        message: 'iptv-org publishes one list per country, language and '
            'category — there is no single list of everything, so choose the '
            'ones you want and they are merged into one grid.',
        action: VideoTab(
          hue: cPlay,
          icon: Icons.list,
          label: 'Choose playlists',
          active: true,
          onTap: () => openPlaylistPicker(context, controller, live),
        ),
      );
    }
    if (live.busy && live.channels.isEmpty) {
      return const Center(child: CircularProgressIndicator());
    }
    if (live.channels.isEmpty) {
      return VideosEmpty(
        icon: Icons.tv_off_outlined,
        title: live.total == 0 ? 'No channels loaded' : 'Nothing matched',
        message: live.total == 0
            ? 'The chosen playlists came back empty — press Refresh to fetch '
                'them again.'
            : 'Clear the search or pick another category.',
      );
    }
    return LayoutBuilder(
      builder: (context, box) {
        // Capped at seven so a full page is the 7×6 the page size is cut for.
        final cols = (((box.maxWidth - 42) / 196).floor()).clamp(1, 7);
        return GridView.builder(
          padding: const EdgeInsets.fromLTRB(28, 28, 28, 28),
          gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
            crossAxisCount: cols,
            mainAxisSpacing: 14,
            crossAxisSpacing: 14,
            childAspectRatio: 1.05,
          ),
          itemCount: live.channels.length,
          itemBuilder: (_, i) => _ChannelCard(
            channel: live.channels[i],
            playing: live.nowIndex == live.channels[i].index,
            anyPlaying: live.nowName.isNotEmpty,
            onPlay: () => controller
                .send(VideosCmd.livePlay(index: live.channels[i].index)),
          ),
        );
      },
    );
  }
}

/// One channel. A single click plays when nothing is on; once something is
/// running, switching takes a double click, so a stray click on the grid cannot
/// kill a stream that is already playing.
class _ChannelCard extends StatefulWidget {
  const _ChannelCard({
    required this.channel,
    required this.playing,
    required this.anyPlaying,
    required this.onPlay,
  });

  final LiveChannel channel;
  final bool playing;
  final bool anyPlaying;
  final VoidCallback onPlay;

  @override
  State<_ChannelCard> createState() => _ChannelCardState();
}

class _ChannelCardState extends State<_ChannelCard> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final ch = widget.channel;
    // Logos are overwhelmingly white-on-transparent PNGs, so the backdrop is
    // what decides whether they read at all.
    final well = t.dark ? const Color(0xFF3A3F48) : const Color(0xFF9BA3B0);
    final wellInk = t.dark
        ? Colors.white.withValues(alpha: 0.55)
        : Colors.black.withValues(alpha: 0.45);

    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.anyPlaying ? null : widget.onPlay,
        onDoubleTap:
            widget.anyPlaying && !widget.playing ? widget.onPlay : null,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 120),
          padding: const EdgeInsets.all(10),
          // The channel on air is latched in the section's colour; the rest
          // are the skin's cards.
          decoration: (widget.playing
                  ? context.skin.control(
                      active: true, tint: Tokens.secVideos, radius: 10)
                  : context.skin.surface(SurfaceRole.card, radius: 10)) ??
              BoxDecoration(
            color: _hover ? t.panel2 : t.panel,
            borderRadius: BorderRadius.circular(10),
            border: Border.all(
              color: widget.playing
                  ? Tokens.secVideos
                  : (_hover
                      ? Tokens.secVideos.withValues(alpha: 0.27)
                      : t.nHair),
              width: widget.playing ? 2 : 1,
            ),
          ),
          child: Column(
            children: [
              Expanded(
                child: Stack(
                  children: [
                    Positioned.fill(
                      child: Container(
                        decoration: BoxDecoration(
                          color: well,
                          borderRadius: BorderRadius.circular(8),
                        ),
                        clipBehavior: Clip.antiAlias,
                        padding: const EdgeInsets.all(8),
                        child: ch.logo.isEmpty
                            ? Center(
                                child: Text(
                                  ch.name,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  textAlign: TextAlign.center,
                                  style: TextStyle(
                                      fontSize: 20,
                                      fontWeight: FontWeight.w800,
                                      color: wellInk),
                                ),
                              )
                            : Artwork(
                                path: ch.logo,
                                width: double.infinity,
                                height: double.infinity,
                                radius: 0,
                                fallback: Icons.radio,
                              ),
                      ),
                    ),
                    if (widget.playing)
                      const Positioned(
                        top: 6,
                        left: 6,
                        child: OverlayPill(
                            text: 'ON AIR', background: Tokens.secVideos),
                      ),
                    // Without this the card looks broken: a single click does
                    // nothing while another channel is on.
                    if (widget.anyPlaying && !widget.playing && _hover)
                      const Positioned(
                        left: 0,
                        right: 0,
                        bottom: 6,
                        child: Center(
                          child: OverlayPill(
                              text: 'Double-click to switch',
                              background: Color(0xB0000000)),
                        ),
                      ),
                  ],
                ),
              ),
              const SizedBox(height: 6),
              Text(
                ch.name,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                textAlign: TextAlign.center,
                style: TextStyle(
                    fontSize: 12.5, fontWeight: FontWeight.w700, color: t.nInk),
              ),
              Text(
                ch.group.isEmpty ? 'Live' : ch.group,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 10.5, color: t.nInk3),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _Categories extends StatelessWidget {
  const _Categories({required this.controller, required this.live});

  final VideosController controller;
  final LiveView live;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      width: 236,
      color: t.panel,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 18, 12, 10),
            child: Row(
              children: [
                Icon(Icons.category_outlined, size: 15, color: t.nInk2),
                const SizedBox(width: 8),
                Text('CATEGORIES',
                    style: TextStyle(
                        fontSize: 10,
                        fontWeight: FontWeight.w800,
                        letterSpacing: 1.1,
                        color: t.nInk3)),
              ],
            ),
          ),
          Expanded(
            child: ListView(
              padding: const EdgeInsets.symmetric(horizontal: 10),
              children: [
                _GroupRow(
                  label: 'All channels',
                  count: live.total,
                  active: live.group.isEmpty,
                  onTap: () =>
                      controller.send(const VideosCmd.liveSetGroup(group: '')),
                ),
                for (final g in live.groups)
                  _GroupRow(
                    label: g.name,
                    count: g.count,
                    active: live.group == g.name,
                    onTap: () =>
                        controller.send(VideosCmd.liveSetGroup(group: g.name)),
                  ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _GroupRow extends StatelessWidget {
  const _GroupRow({
    required this.label,
    required this.count,
    required this.active,
    required this.onTap,
  });

  final String label;
  final int count;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(8),
      child: Container(
        height: 34,
        padding: const EdgeInsets.symmetric(horizontal: 10),
        decoration: (active
                ? context.skin.control(
                    active: true, tint: Tokens.secVideos, radius: 8)
                : null) ??
            BoxDecoration(
              color: active
                  ? Tokens.secVideos.withValues(alpha: 0.16)
                  : Colors.transparent,
              borderRadius: BorderRadius.circular(8),
            ),
        child: Row(
          children: [
            Expanded(
              child: Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: 12.5,
                  fontWeight: active ? FontWeight.w700 : FontWeight.w500,
                  color: active ? Tokens.secVideos : t.nInk,
                ),
              ),
            ),
            if (count >= 0)
              Text('$count', style: TextStyle(fontSize: 11, color: t.nInk3)),
          ],
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------- modals ----

/// The now-playing sheet: what mpv was handed, and what the playlist says.
Future<void> openNowPlaying(
  BuildContext context,
  VideosController controller,
  LiveView live,
) async {
  await controller.send(const VideosCmd.liveInfoOpened());
  if (!context.mounted) return;
  await showDialog<void>(
    context: context,
    builder: (ctx) => AnimatedBuilder(
      animation: controller,
      builder: (ctx, _) {
        final l = controller.state?.live ?? live;
        final t = ctx.tokens;
        return AlertDialog(
          backgroundColor: t.modalSolid,
          title: Row(
            children: [
              Container(
                width: 8,
                height: 8,
                decoration: const BoxDecoration(
                    color: Color(0xFFEF4444), shape: BoxShape.circle),
              ),
              const SizedBox(width: 8),
              Text('ON AIR',
                  style: TextStyle(
                      fontSize: 10,
                      fontWeight: FontWeight.w800,
                      letterSpacing: 1.2,
                      color: t.nInk3)),
            ],
          ),
          content: SizedBox(
            width: 480,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(l.nowName,
                    style: TextStyle(
                        fontSize: 18,
                        fontWeight: FontWeight.w800,
                        color: t.nInk)),
                const SizedBox(height: 14),
                _InfoRow(label: 'CHANNEL ID', value: l.nowId),
                _InfoRow(
                    label: 'CATEGORY',
                    value: l.nowGroup.isEmpty ? 'Live' : l.nowGroup),
                // What the playlist claims, not what ffprobe found: probing a
                // live HLS stream mpv already holds open means a second
                // connection and a wait, for a number the name already carries.
                _InfoRow(label: 'QUALITY', value: l.nowRes),
                _InfoRow(label: 'CACHE', value: l.nowCache),
                _InfoRow(label: 'STREAM', value: l.nowUrl),
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
  );
}

class _InfoRow extends StatelessWidget {
  const _InfoRow({required this.label, required this.value});

  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(bottom: 10),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 92,
            child: Text(label,
                style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w800,
                    letterSpacing: 0.6,
                    color: t.nInk3)),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: SelectableText(value.isEmpty ? '—' : value,
                style: TextStyle(fontSize: 12, color: t.nInk)),
          ),
        ],
      ),
    );
  }
}

/// The iptv-org picker. Everything ticked is fetched and merged into one grid;
/// the lists are cached for a day.
Future<void> openPlaylistPicker(
  BuildContext context,
  VideosController controller,
  LiveView live,
) async {
  final filter = TextEditingController(text: live.configQuery);
  await controller.send(VideosCmd.liveConfigLoad(
      tab: live.configTab.isEmpty ? 'country' : live.configTab,
      query: filter.text));
  if (!context.mounted) {
    filter.dispose();
    return;
  }
  await showDialog<void>(
    context: context,
    builder: (ctx) => AnimatedBuilder(
      animation: controller,
      builder: (ctx, _) {
        final l = controller.state?.live ?? live;
        final t = ctx.tokens;
        void reload(String tab) => controller
            .send(VideosCmd.liveConfigLoad(tab: tab, query: filter.text));

        return AlertDialog(
          backgroundColor: t.modalSolid,
          title: const Text('Live TV playlists'),
          content: SizedBox(
            width: 520,
            height: 520,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(
                  'Everything ticked is fetched and merged into one grid. '
                  'Lists are cached for a day.',
                  style: TextStyle(fontSize: 11, color: t.nInk3),
                ),
                const SizedBox(height: 12),
                Row(
                  children: [
                    VideoTab(
                        hue: cInfo,
                        label: 'Country',
                        active: l.configTab == 'country',
                        onTap: () => reload('country')),
                    const SizedBox(width: 6),
                    VideoTab(
                        hue: cDl,
                        label: 'Language',
                        active: l.configTab == 'language',
                        onTap: () => reload('language')),
                    const SizedBox(width: 6),
                    VideoTab(
                        hue: cBack,
                        label: 'Category',
                        active: l.configTab == 'category',
                        onTap: () => reload('category')),
                  ],
                ),
                const SizedBox(height: 12),
                VideoSearchField(
                  controller: filter,
                  width: double.infinity,
                  hint: 'Filter this list',
                  onChanged: (_) => reload(l.configTab),
                ),
                const SizedBox(height: 12),
                Expanded(
                  child: l.playlists.isEmpty
                      ? Center(
                          child: Text(
                            'Nothing matched. Clear the filter to see the '
                            'whole list.',
                            textAlign: TextAlign.center,
                            style: TextStyle(fontSize: 12, color: t.nInk3),
                          ),
                        )
                      : ListView.builder(
                          itemCount: l.playlists.length,
                          itemBuilder: (_, i) {
                            final p = l.playlists[i];
                            return CheckboxListTile(
                              dense: true,
                              value: p.picked,
                              activeColor: Tokens.secVideos,
                              controlAffinity: ListTileControlAffinity.leading,
                              title: Text(p.name,
                                  style:
                                      TextStyle(fontSize: 12.5, color: t.nInk)),
                              onChanged: (_) => controller.send(
                                  VideosCmd.liveTogglePlaylist(url: p.url)),
                            );
                          },
                        ),
                ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () =>
                  controller.send(const VideosCmd.liveConfigClear()),
              child: const Text('Clear all'),
            ),
            TextButton(
                onPressed: () => Navigator.pop(ctx),
                child: const Text('Close')),
            FilledButton(
              onPressed: l.pickedCount == 0
                  ? null
                  : () {
                      controller.send(const VideosCmd.liveConfigSave());
                      Navigator.pop(ctx);
                    },
              child: Text(l.pickedCount == 0
                  ? 'Nothing chosen'
                  : 'Load ${l.pickedCount} '
                      'list${l.pickedCount == 1 ? '' : 's'}'),
            ),
          ],
        );
      },
    ),
  );
  filter.dispose();
}
