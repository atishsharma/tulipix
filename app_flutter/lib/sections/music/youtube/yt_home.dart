// YouTube › Home: where you left off, what is new, and what plays offline.
//
// Order is the order of intent. Resuming something half-watched is the most
// likely reason to open the tab, so it is the hero; new uploads from the
// channels you follow come next; saved playlists and offline copies are the
// shelves under them. Searches have their own view (yt_results.dart).

import 'package:flutter/material.dart';

import '../../../design/clock.dart';
import '../../../design/tokens.dart';
import '../../../src/rust/api/music.dart';
import '../music_controller.dart';
import '../music_dialogs.dart';
import '../music_widgets.dart';
import 'yt_card.dart';
import 'yt_format_sheet.dart';

class YtHome extends StatelessWidget {
  const YtHome({super.key, required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    // Offline shelf: downloads first (they are kept on purpose), then the
    // cache, one card per video.
    final offline = <YtVideo>[];
    for (final v in [...st.ytDownloads, ...st.ytCached]) {
      if (offline.length == 6) break;
      if (!offline.any((o) => o.videoId == v.videoId)) offline.add(v);
    }
    final hero = st.ytContinue.firstOrNull;
    if (hero == null &&
        st.ytRecommended.isEmpty &&
        offline.isEmpty &&
        st.ytHomeSubs.isEmpty &&
        st.ytPlaylists.isEmpty) {
      return const MusicEmpty(
        icon: Icons.smart_display_outlined,
        title: 'Search for something',
        body: 'Play it as audio through the section\'s player, or watch it. '
            'Subscribe to channels and their new videos show up here. '
            'Needs yt-dlp on PATH or bundled in tools/.',
      );
    }

    return ListView(
      padding: const EdgeInsets.only(bottom: 32),
      children: [
        if (hero != null || st.ytHomeSubs.isNotEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 16, 24, 0),
            child: LayoutBuilder(builder: (context, box) {
              final side = _Side(controller: c, st: st);
              if (hero == null) return side;
              final big = _Hero(controller: c, video: hero);
              if (box.maxWidth < 900) {
                return Column(
                    children: [big, const SizedBox(height: 16), side]);
              }
              return IntrinsicHeight(
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Expanded(flex: 5, child: big),
                    const SizedBox(width: 16),
                    Expanded(flex: 3, child: side),
                  ],
                ),
              );
            }),
          ),
        if (st.ytRecommended.isNotEmpty) ...[
          // Nothing pinned and no subscriptions: Rust fills the shelf with
          // trending music instead.
          if (st.ytHomeChannels.isEmpty && st.ytHomeSubs.isEmpty)
            const _Head(
              title: 'Trending music',
              hint: 'Hindi · Punjabi · English, most watched this week',
            )
          else
            _Head(
              title: 'New from your channels',
              hint: st.ytHomeChannels.isEmpty
                  ? 'from your most-followed subscriptions'
                  : 'from your pinned channels',
              action: (
                'Subscriptions',
                () => c.send(const MusicCmd.ytSetTab(name: 'subscriptions'))
              ),
            ),
          YtVideoGrid(controller: c, videos: st.ytRecommended),
        ],
        if (st.ytPlaylists.isNotEmpty) ...[
          _Head(
            title: 'Saved playlists',
            hint: '${st.ytPlaylists.length} playlists',
            action: (
              'See all',
              () => c.send(const MusicCmd.ytSetTab(name: 'playlists'))
            ),
          ),
          YtGrid(children: [
            for (final p in st.ytPlaylists.take(6))
              YtPlaylistCard(
                controller: c,
                playlist: p,
                playing: st.ytPlayingPlId == p.id,
              ),
          ]),
        ],
        if (offline.isNotEmpty) ...[
          _Head(
            title: 'Plays offline',
            hint: 'already on this disk',
            action: (
              'Cached',
              () => c.send(const MusicCmd.ytSetTab(name: 'cached'))
            ),
          ),
          YtVideoGrid(controller: c, videos: offline),
        ],
      ],
    );
  }
}

class _Head extends StatelessWidget {
  const _Head({required this.title, this.hint, this.action});

  final String title;
  final String? hint;
  final (String, VoidCallback)? action;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 28, 24, 12),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.baseline,
        textBaseline: TextBaseline.alphabetic,
        children: [
          Text(title,
              style: TextStyle(
                  fontSize: 17,
                  fontWeight: FontWeight.w700,
                  letterSpacing: -0.3,
                  color: t.nInk)),
          if (hint != null) ...[
            const SizedBox(width: 10),
            Text(hint!, style: TextStyle(fontSize: 12, color: t.nInk3)),
          ],
          const Spacer(),
          if (action != null)
            TextButton(onPressed: action!.$2, child: Text(action!.$1)),
        ],
      ),
    );
  }
}

/// The most recent part-watched video, large: resume it as audio, watch it
/// from the same second, or keep it.
class _Hero extends StatelessWidget {
  const _Hero({required this.controller, required this.video});

  final MusicController controller;
  final YtVideo video;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final v = video;
    final at = v.progress * v.duration;
    return ConstrainedBox(
      constraints: const BoxConstraints(minHeight: 300),
      child: ClipRRect(
        borderRadius: BorderRadius.circular(Tokens.radiusLg),
        child: Stack(
          fit: StackFit.passthrough,
          children: [
            Positioned.fill(
              child: MusicArt(
                controller: c,
                kind: 'yt',
                artKey: v.thumb,
                direct: v.thumb.startsWith('http') ? null : v.thumb,
                size: 640,
                radius: 0,
                fallback: Icons.smart_display,
              ),
            ),
            // Artwork is whatever the uploader chose, so the words sit on a
            // scrim of their own rather than on the theme.
            const Positioned.fill(
              child: DecoratedBox(
                decoration: BoxDecoration(
                  gradient: LinearGradient(
                    begin: Alignment.topCenter,
                    end: Alignment.bottomCenter,
                    stops: [0.25, 1],
                    colors: [Color(0x00060412), Color(0xDD060412)],
                  ),
                ),
              ),
            ),
            Padding(
              padding: const EdgeInsets.all(26),
              child: Column(
                mainAxisAlignment: MainAxisAlignment.end,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Text(
                    'PICK UP WHERE YOU LEFT OFF',
                    style: TextStyle(
                        fontSize: 11,
                        letterSpacing: 1.8,
                        fontWeight: FontWeight.w600,
                        color: Colors.white70),
                  ),
                  const SizedBox(height: 10),
                  ConstrainedBox(
                    constraints: const BoxConstraints(maxWidth: 560),
                    child: Text(
                      v.title,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                          fontSize: 30,
                          height: 1.08,
                          letterSpacing: -0.8,
                          fontWeight: FontWeight.w800,
                          color: Colors.white),
                    ),
                  ),
                  const SizedBox(height: 8),
                  Text(
                    '${v.channel}  ·  ${fmtClock(at)} of ${fmtClock(v.duration.toDouble())}',
                    style: const TextStyle(
                        fontSize: 13,
                        color: Colors.white70,
                        fontFeatures: [FontFeature.tabularFigures()]),
                  ),
                  const SizedBox(height: 12),
                  ConstrainedBox(
                    constraints: const BoxConstraints(maxWidth: 420),
                    child: ClipRRect(
                      borderRadius: BorderRadius.circular(2),
                      child: LinearProgressIndicator(
                        value: v.progress.clamp(0.0, 1.0),
                        minHeight: 4,
                        backgroundColor: Colors.white24,
                        valueColor: const AlwaysStoppedAnimation(Colors.white),
                      ),
                    ),
                  ),
                  const SizedBox(height: 16),
                  Wrap(
                    spacing: 8,
                    runSpacing: 8,
                    children: [
                      FilledButton.icon(
                        style: musicFilledStyle(fill: ytRose),
                        icon: const Icon(Icons.play_arrow_rounded, size: 18),
                        label: const Text('Resume audio'),
                        onPressed: () => playYtAudio(c, v),
                      ),
                      _GlassButton(
                        icon: Icons.smart_display_outlined,
                        label: 'Watch from ${fmtClock(at)}',
                        onTap: () => watchVideo(context, c, v),
                      ),
                      _GlassButton(
                        icon: Icons.download_rounded,
                        label: 'Download',
                        onTap: () => showFormatSheet(context, c, v,
                            mode: FormatMode.download),
                      ),
                    ],
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

class _GlassButton extends StatelessWidget {
  const _GlassButton({
    required this.icon,
    required this.label,
    required this.onTap,
  });

  final IconData icon;
  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => OutlinedButton.icon(
        style: OutlinedButton.styleFrom(
          foregroundColor: Colors.white,
          backgroundColor: Colors.white.withValues(alpha: 0.12),
          side: const BorderSide(color: Colors.white38),
          textStyle: const TextStyle(
              fontFamily: Tokens.fontFamily,
              fontSize: 13,
              fontWeight: FontWeight.w600),
        ),
        icon: Icon(icon, size: 18),
        label: Text(label),
        onPressed: onTap,
      );
}

/// Beside the hero: the rest of Continue, and the channel rail.
class _Side extends StatelessWidget {
  const _Side({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = controller;
    // Four in all: the big card, and three beside it.
    final rest = st.ytContinue.skip(1).take(3).toList();
    Widget panel(String title, Widget child,
            {(String, VoidCallback)? action}) =>
        DecoratedBox(
          decoration: BoxDecoration(
            color: t.nCard,
            border: Border.all(color: t.nHair),
            borderRadius: BorderRadius.circular(Tokens.radiusLg),
          ),
          child: Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 12, 14),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Text(title,
                        style: TextStyle(
                            fontSize: 13,
                            fontWeight: FontWeight.w600,
                            color: t.nInk2)),
                    const Spacer(),
                    if (action != null)
                      TextButton(onPressed: action.$2, child: Text(action.$1)),
                  ],
                ),
                const SizedBox(height: 6),
                child,
              ],
            ),
          ),
        );

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (rest.isNotEmpty)
          panel(
            'Continue',
            Column(children: [
              for (final v in rest) _ContinueRow(controller: c, video: v),
            ]),
          ),
        if (rest.isNotEmpty && st.ytHomeSubs.isNotEmpty)
          const SizedBox(height: 16),
        if (st.ytHomeSubs.isNotEmpty)
          panel(
            st.ytHomeChannels.isEmpty ? 'Channels' : 'Pinned channels',
            Wrap(
              spacing: 10,
              runSpacing: 12,
              children: [
                for (final s in st.ytHomeSubs.take(8))
                  _ChannelChip(
                    controller: c,
                    sub: s,
                    pinned: st.ytHomeChannels.contains(s.channelId),
                  ),
              ],
            ),
            action: (
              'All',
              () => c.send(const MusicCmd.ytSetTab(name: 'subscriptions'))
            ),
          ),
      ],
    );
  }
}

class _ContinueRow extends StatelessWidget {
  const _ContinueRow({required this.controller, required this.video});

  final MusicController controller;
  final YtVideo video;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final v = video;
    final left = (1 - v.progress) * v.duration;
    return InkWell(
      borderRadius: BorderRadius.circular(Tokens.radiusSm),
      onTap: () => playYtAudio(controller, v),
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 5),
        child: Row(
          children: [
            SizedBox(
              width: 96,
              height: 54,
              child: YtThumb(controller: controller, video: v),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(v.title,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 12,
                          fontWeight: FontWeight.w600,
                          color: t.nInk)),
                  Text('${fmtClock(left)} left',
                      style: TextStyle(
                          fontSize: 11,
                          color: t.nInk3,
                          fontFeatures: const [FontFeature.tabularFigures()])),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ChannelChip extends StatelessWidget {
  const _ChannelChip({
    required this.controller,
    required this.sub,
    required this.pinned,
  });

  final MusicController controller;
  final YtSub sub;
  final bool pinned;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = controller;
    final s = sub;
    return Tooltip(
      message: pinned
          ? '${s.title} · right-click to unpin'
          : '${s.title} · right-click to pin',
      child: InkWell(
        borderRadius: BorderRadius.circular(12),
        onTap: () => c.send(MusicCmd.ytOpenChannel(channelId: s.channelId)),
        onSecondaryTap: () => c.send(pinned
            ? MusicCmd.ytUnpinHome(channelId: s.channelId)
            : MusicCmd.ytPinHome(channelId: s.channelId)),
        child: SizedBox(
          width: 64,
          child: Column(
            children: [
              MusicArt(
                controller: c,
                kind: 'yt',
                artKey: s.avatar,
                direct: s.avatar,
                size: 48,
                radius: 24,
                fallback: Icons.person,
              ),
              const SizedBox(height: 6),
              Text(s.title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  textAlign: TextAlign.center,
                  style: TextStyle(fontSize: 11, color: t.nInk2)),
            ],
          ),
        ),
      ),
    );
  }
}
