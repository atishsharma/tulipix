// YouTube › a channel: who it is and what it uploaded, newest first, 12 at a
// time.
//
// Search is the Music header's: on this page it searches the channel.
//
// The banner is the channel's own when it has one (fetched with the listing
// and kept on disk), and otherwise its avatar scaled up and blurred into a
// wash. It scrolls away with the page rather than holding 190 px of a short
// window.

import 'dart:ui' show ImageFilter;

import 'package:flutter/material.dart';

import '../../../design/tokens.dart';
import '../../../src/rust/api/music.dart';
import '../music_controller.dart';
import '../music_dialogs.dart';
import '../music_widgets.dart';
import 'yt_card.dart';
import 'yt_format_sheet.dart';
import '../../../design/decode.dart';

/// Cards per page, and videos per yt-dlp block (`CHANNEL_PAGE` in Rust): one
/// page is one fetch, kept small so a channel does not keep yt-dlp busy.
const int _perPage = 12;

class YtChannel extends StatefulWidget {
  const YtChannel({super.key, required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  State<YtChannel> createState() => _YtChannelState();
}

class _YtChannelState extends State<YtChannel> {
  /// 0-based. A page past what is loaded is the next block, on its way.
  int _page = 0;

  @override
  void didUpdateWidget(YtChannel old) {
    super.didUpdateWidget(old);
    if (old.st.ytChannelId != widget.st.ytChannelId ||
        old.st.ytChannelQuery != widget.st.ytChannelQuery) {
      _page = 0;
    }
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final st = widget.st;
    final searching = st.ytChannelQuery.isNotEmpty;
    // A search replaces the listing while it is active; clearing the box
    // brings the listing back, which is why the two lists are kept apart.
    final videos = searching ? st.ytChannelResults : st.ytChannelVideos;
    final pinned = st.ytHomeChannels.contains(st.ytChannelId);
    // Only the listing has a next block: the search is a fixed twenty hits.
    final more = !searching && st.ytChannelHasNext;
    final loaded = (videos.length / _perPage).ceil().clamp(1, 1 << 30);
    final page = _page.clamp(0, loaded - 1);
    final shown = videos.skip(page * _perPage).take(_perPage).toList();

    return ListView(
      padding: const EdgeInsets.only(bottom: 32),
      children: [
        _Banner(
          controller: c,
          st: st,
          pinned: pinned,
          // In the banner, left of pin and Subscribe.
          actions: [
            if (videos.isNotEmpty) ...[
              FilledButton.icon(
                style: musicFilledStyle(fill: ytRose),
                icon: const Icon(Icons.play_arrow_rounded, size: 18),
                label: const Text('Play all'),
                onPressed: () => c.send(MusicCmd.ytPlayAll(
                    videoIds: [for (final v in videos) v.videoId])),
              ),
              FilledButton.icon(
                style: ytDownloadStyle(),
                icon: const Icon(Icons.download_rounded, size: 16),
                label: Text('Download all ${videos.length}'),
                onPressed: () => showFormatSheet(context, c, videos.first,
                    mode: FormatMode.download, batch: videos),
              ),
            ],
            // One page further than is loaded while YouTube has more:
            // going there fetches the next block, and it shows on arrival.
            Pager(
              page: page,
              pages: more ? loaded + 1 : loaded,
              compact: true,
              onGo: (p) {
                if (p >= loaded) {
                  c.send(const MusicCmd.ytChannelLoadMore());
                }
                setState(() => _page = p);
              },
            ),
          ],
        ),
        const SizedBox(height: 12),
        if (videos.isEmpty)
          SizedBox(
            height: 280,
            child: MusicEmpty(
              icon: searching ? Icons.search_off : Icons.videocam_off_outlined,
              title:
                  searching ? 'No matches in this channel' : 'Nothing to list',
              body: searching
                  ? 'Try fewer words, or clear the box to see the whole channel.'
                  : 'yt-dlp could not reach this channel, or it has no public '
                      'uploads.',
            ),
          )
        else
          YtVideoGrid(controller: c, videos: shown),
      ],
    );
  }
}

class _Banner extends StatelessWidget {
  const _Banner({
    required this.controller,
    required this.st,
    required this.pinned,
    required this.actions,
  });

  final MusicController controller;
  final MusicState st;
  final bool pinned;

  /// Play all, Download all and the pager.
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = controller;
    Widget avatar(double size) => MusicArt(
          controller: c,
          kind: 'yt',
          artKey: st.ytChannelAvatar,
          direct: st.ytChannelAvatar,
          size: size,
          radius: size / 2,
          fallback: Icons.person,
        );

    return SizedBox(
      height: 190,
      child: Stack(
        fit: StackFit.expand,
        children: [
          if (st.ytChannelBanner.isNotEmpty)
            FileArt(
              st.ytChannelBanner,
              errorBuilder: (_, __, ___) => const SizedBox.shrink(),
            )
          else
            RepaintBoundary(
              child: ClipRect(
                child: ImageFiltered(
                  imageFilter: ImageFilter.blur(sigmaX: 40, sigmaY: 40),
                  child: Transform.scale(
                    scale: 4,
                    child: Opacity(opacity: 0.55, child: avatar(160)),
                  ),
                ),
              ),
            ),
          // Into the page's own ground, so the banner has no edge.
          DecoratedBox(
            decoration: BoxDecoration(
              gradient: LinearGradient(
                begin: Alignment.topCenter,
                end: Alignment.bottomCenter,
                colors: [t.nCanvas.withValues(alpha: 0), t.nCanvas],
              ),
            ),
          ),
          Positioned(
            left: 12,
            top: 8,
            child: IconButton(
              icon: const Icon(Icons.arrow_back),
              tooltip: 'Back',
              onPressed: () => c.send(const MusicCmd.ytChannelBack()),
            ),
          ),
          Positioned(
            left: 24,
            right: 24,
            bottom: 8,
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: [
                DecoratedBox(
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    border: Border.all(color: t.nCanvas, width: 3),
                  ),
                  child: avatar(88),
                ),
                const SizedBox(width: 16),
                Expanded(
                  flex: 2,
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(st.ytChannelTitle,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 26,
                              fontWeight: FontWeight.w800,
                              letterSpacing: -0.6,
                              color: t.nInk)),
                      // "@handle · 4.2M subscribers".
                      if (st.ytChannelSub.isNotEmpty)
                        Text(st.ytChannelSub,
                            style: TextStyle(fontSize: 12, color: t.nInk2)),
                      const SizedBox(height: 6),
                    ],
                  ),
                ),
                const SizedBox(width: 12),
                // The page's buttons in one line with Subscribe; a narrow
                // window wraps them upward, the banner being bottom-anchored.
                // Expanded, not Flexible: a loose Flexible shrinks the Wrap to
                // its buttons and leaves the spare width after them, so `end`
                // had nothing to push against and the row sat mid-banner.
                Expanded(
                  flex: 3,
                  child: Wrap(
                    alignment: WrapAlignment.end,
                    crossAxisAlignment: WrapCrossAlignment.center,
                    spacing: 8,
                    runSpacing: 8,
                    children: [
                      ...actions,
                      IconButton(
                        tooltip:
                            pinned ? 'Unpin from Home' : 'Pin to the Home rail',
                        icon: Icon(
                            pinned ? Icons.push_pin : Icons.push_pin_outlined,
                            size: 18),
                        onPressed: () => c.send(pinned
                            ? MusicCmd.ytUnpinHome(channelId: st.ytChannelId)
                            : MusicCmd.ytPinHome(channelId: st.ytChannelId)),
                      ),
                      if (st.ytChannelSubscribed)
                        FilledButton.icon(
                          style: musicFilledStyle(fill: const Color(0xFFDC2626))
                              .copyWith(
                                  foregroundColor: const WidgetStatePropertyAll(
                                      Colors.white)),
                          icon: const Icon(Icons.check, size: 16),
                          label: const Text('Subscribed'),
                          onPressed: () => c.send(
                              MusicCmd.ytUnsub(channelId: st.ytChannelId)),
                        )
                      else
                        FilledButton.icon(
                          style: musicFilledStyle(fill: ytRose),
                          icon: const Icon(Icons.add, size: 16),
                          label: const Text('Subscribe'),
                          onPressed: () => c.send(MusicCmd.ytSubscribe(
                            channelId: st.ytChannelId,
                            title: st.ytChannelTitle,
                          )),
                        ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
