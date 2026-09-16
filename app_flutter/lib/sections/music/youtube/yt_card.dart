// The YouTube tab's cards: a video as a 16:9 tile, and a saved playlist.
//
// A video card answers "how do you want this" without a menu: hovering or
// focusing it shows Audio, Video and Download over the picture. The title
// plays it the way the tab always has, as audio; the channel name opens the
// channel when the bridge knows which one it is.

import 'package:flutter/material.dart';

import '../../../design/clock.dart';
import '../../../design/tokens.dart';
import '../../../src/rust/api/music.dart';
import '../music_controller.dart';
import '../music_dialogs.dart' show musicFilledStyle;
import '../music_widgets.dart';
import 'yt_details.dart';
import 'yt_dialogs.dart';
import 'yt_format_sheet.dart';

/// Cards on a paged YouTube page: three rows of six.
const int ytPerPage = 18;

/// Download buttons: green, filled, the same on every page.
ButtonStyle ytDownloadStyle() => musicFilledStyle(fill: Tokens.ok).copyWith(
      foregroundColor: const WidgetStatePropertyAll(Colors.white),
    );

/// The YouTube tab's rose.
const Color ytRose = Color(0xFFF43F5E);

/// Bytes as the tab prints them: one decimal under 100 MB, whole above, GB
/// past a gigabyte.
String ytSize(num bytes) {
  if (bytes >= 1024 * 1024 * 1024) {
    return '${(bytes / (1024 * 1024 * 1024)).toStringAsFixed(2)} GB';
  }
  final mb = bytes / (1024 * 1024);
  return mb >= 100 ? '${mb.round()} MB' : '${mb.toStringAsFixed(1)} MB';
}

/// `meta` split into its view count ("1.2M views") and the rest: the count
/// goes on the picture, the rest under the title.
(String, String) ytSplitViews(String meta) {
  final parts = meta.split(' · ');
  final views = parts.where((p) => p.endsWith(' views')).firstOrNull ?? '';
  return (views, parts.where((p) => p != views).join(' · '));
}

/// What an offline copy holds: "audio" or "video", "" for none.
String ytOfflineKind(YtVideo v) => v.offline.split(':').first;

/// Whether that copy is a download, kept for good, rather than the cache.
bool ytDownloaded(YtVideo v) => v.offline.endsWith(':dl');

/// A file the in-app player can show a picture from.
bool ytIsVideoFile(String path) => const {'mp4', 'mkv', 'webm', 'mov', 'm4v'}
    .contains(path.split('.').last.toLowerCase());

/// Watch from disk when the video is a file there, otherwise at the saved
/// default (`yt_prefs::watch_pref`). Picking a specific stream is the format
/// panel's job.
Future<void> watchVideo(BuildContext context, MusicController c, YtVideo v) =>
    c.send(ytIsVideoFile(v.mediaPath)
        ? MusicCmd.ytWatchLocal(mediaPath: v.mediaPath, videoId: v.videoId)
        : MusicCmd.ytWatch(videoId: v.videoId, formatId: ''));

/// Play as audio: from disk when there is a file, streamed otherwise.
Future<void> playYtAudio(MusicController c, YtVideo v) => c.send(
      v.mediaPath.isNotEmpty
          ? MusicCmd.ytPlayLocal(mediaPath: v.mediaPath, videoId: v.videoId)
          : MusicCmd.ytPlay(videoId: v.videoId, formatId: ''),
    );

class YtVideoCard extends StatefulWidget {
  const YtVideoCard({
    super.key,
    required this.controller,
    required this.video,
    this.extraMenu = const [],
    this.subtitle,
  });

  final MusicController controller;
  final YtVideo video;

  /// The line after the channel, in place of the video's own `meta`.
  final String? subtitle;

  /// Items the page adds to the card's menu, after its own.
  final List<(String, VoidCallback)> extraMenu;

  @override
  State<YtVideoCard> createState() => _YtVideoCardState();
}

class _YtVideoCardState extends State<YtVideoCard> {
  bool _hover = false;
  bool _focus = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = widget.controller;
    final v = widget.video;
    final playing = c.now?.mode == 'youtube' && c.now?.key == v.videoId;
    final (views, rest) = ytSplitViews(v.meta);
    final meta = widget.subtitle ??
        (rest.isNotEmpty ? rest : (v.meta.isEmpty ? v.quality : ''));
    return FocusableActionDetector(
      onShowHoverHighlight: (x) => setState(() => _hover = x),
      onShowFocusHighlight: (x) => setState(() => _focus = x),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          AspectRatio(
            aspectRatio: 16 / 9,
            child: GestureDetector(
              onTap: () => playYtAudio(c, v),
              child: YtThumb(
                controller: c,
                video: v,
                radius: Tokens.radiusMd,
                views: views,
                overlay: AnimatedOpacity(
                  opacity: _hover || _focus ? 1 : 0,
                  duration: t.reduceMotion
                      ? Duration.zero
                      : const Duration(milliseconds: 150),
                  child: Align(
                    alignment: Alignment.bottomLeft,
                    child: Padding(
                      padding: const EdgeInsets.all(8),
                      child: Wrap(
                        spacing: 6,
                        runSpacing: 6,
                        children: [
                          _QuickButton(
                            icon: Icons.headphones_outlined,
                            label: 'Audio',
                            onTap: () => playYtAudio(c, v),
                          ),
                          _QuickButton(
                            icon: Icons.smart_display_outlined,
                            label: 'Video',
                            onTap: () => watchVideo(context, c, v),
                          ),
                          _QuickButton(
                            icon: Icons.download_rounded,
                            tooltip: 'Download…',
                            onTap: () => showFormatSheet(context, c, v,
                                mode: FormatMode.download),
                          ),
                        ],
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
          const SizedBox(height: 8),
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    InkWell(
                      onTap: () => playYtAudio(c, v),
                      child: Text(
                        v.title,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          fontSize: 13,
                          height: 1.3,
                          fontWeight: FontWeight.w600,
                          color: playing ? ytRose : t.nInk,
                        ),
                      ),
                    ),
                    const SizedBox(height: 2),
                    Row(
                      children: [
                        Flexible(
                          child: YtChannelLink(controller: c, video: v),
                        ),
                        if (meta.isNotEmpty)
                          Flexible(
                            child: Text(
                              '  ·  $meta',
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(fontSize: 11, color: t.nInk3),
                            ),
                          ),
                      ],
                    ),
                  ],
                ),
              ),
              _CardMenu(controller: c, video: v, extra: widget.extraMenu),
            ],
          ),
        ],
      ),
    );
  }
}

/// A video's channel name that opens the channel page: a pointer and an
/// underline on hover, so it reads as the link it is. Plain text when the
/// channel is not known.
class YtChannelLink extends StatefulWidget {
  const YtChannelLink({
    super.key,
    required this.controller,
    required this.video,
    this.fontSize = 11,
  });

  final MusicController controller;
  final YtVideo video;
  final double fontSize;

  @override
  State<YtChannelLink> createState() => _YtChannelLinkState();
}

class _YtChannelLinkState extends State<YtChannelLink> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final v = widget.video;
    final linked = v.channelId.isNotEmpty;
    final text = Text(
      v.channel,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: TextStyle(
        fontSize: widget.fontSize,
        color: _hover ? ytRose : t.nInk2,
        decoration: _hover ? TextDecoration.underline : null,
        decorationColor: ytRose,
      ),
    );
    if (!linked) return text;
    return Tooltip(
      message: 'Open ${v.channel}',
      waitDuration: const Duration(milliseconds: 600),
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          behavior: HitTestBehavior.opaque,
          onTap: () => widget.controller
              .send(MusicCmd.ytOpenChannel(channelId: v.channelId)),
          child: text,
        ),
      ),
    );
  }
}

/// A video's picture with the badges every surface shows: duration, the watch
/// bar, and whether it plays offline. `overlay` sits on top of all of them.
class YtThumb extends StatelessWidget {
  const YtThumb({
    super.key,
    required this.controller,
    required this.video,
    this.radius = Tokens.radiusSm,
    this.views = '',
    this.overlay,
  });

  final MusicController controller;
  final YtVideo video;
  final double radius;

  /// "1.2M views", in the top right corner; "" for none.
  final String views;
  final Widget? overlay;

  @override
  Widget build(BuildContext context) {
    final v = video;
    return ClipRRect(
      borderRadius: BorderRadius.circular(radius),
      child: Stack(
        fit: StackFit.expand,
        children: [
          MusicArt(
            controller: controller,
            kind: 'yt',
            artKey: v.thumb,
            direct: v.thumb.startsWith('http') ? null : v.thumb,
            size: 320,
            radius: 0,
            fallback: Icons.smart_display,
          ),
          if (v.offline.isNotEmpty)
            Positioned(
              left: 8,
              top: 8,
              child: _Badge(
                icon: ytOfflineKind(v) == 'video'
                    ? Icons.movie_outlined
                    : Icons.headphones_outlined,
                text: ytDownloaded(v) ? 'DOWNLOADED' : 'CACHED',
              ),
            ),
          if (views.isNotEmpty)
            Positioned(
              right: 8,
              top: 8,
              child: _Badge(text: views),
            ),
          if (v.duration > 0)
            Positioned(
              right: 8,
              bottom: 8,
              child: _Badge(text: fmtClock(v.duration.toDouble())),
            ),
          if (v.progress > 0)
            Positioned(
              left: 0,
              right: 0,
              bottom: 0,
              child: LinearProgressIndicator(
                value: v.progress.clamp(0.0, 1.0),
                minHeight: 3,
                backgroundColor: Colors.white24,
                valueColor: const AlwaysStoppedAnimation(ytRose),
              ),
            ),
          if (overlay != null) overlay!,
        ],
      ),
    );
  }
}

/// Text over artwork: always light on a dark chip, whatever the theme, because
/// the picture under it is whatever the uploader chose.
class _Badge extends StatelessWidget {
  const _Badge({required this.text, this.icon});

  final String text;
  final IconData? icon;

  @override
  Widget build(BuildContext context) => DecoratedBox(
        decoration: BoxDecoration(
          color: Colors.black.withValues(alpha: 0.72),
          borderRadius: BorderRadius.circular(5),
        ),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (icon != null) ...[
                Icon(icon, size: 11, color: Colors.white),
                const SizedBox(width: 4),
              ],
              Text(
                text,
                style: const TextStyle(
                  fontSize: 10,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 0.3,
                  color: Colors.white,
                  fontFeatures: [FontFeature.tabularFigures()],
                ),
              ),
            ],
          ),
        ),
      );
}

class _QuickButton extends StatelessWidget {
  const _QuickButton({
    required this.icon,
    required this.onTap,
    this.label,
    this.tooltip,
  });

  final IconData icon;
  final String? label;
  final String? tooltip;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final button = Material(
      color: Colors.black.withValues(alpha: 0.62),
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(8),
        side: const BorderSide(color: Colors.white24),
      ),
      child: InkWell(
        borderRadius: BorderRadius.circular(8),
        hoverColor: ytRose.withValues(alpha: 0.85),
        onTap: onTap,
        child: Padding(
          padding: EdgeInsets.symmetric(
              horizontal: label == null ? 7 : 9, vertical: 6),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(icon, size: 14, color: Colors.white),
              if (label != null) ...[
                const SizedBox(width: 5),
                Text(label!,
                    style: const TextStyle(
                        fontSize: 11,
                        fontWeight: FontWeight.w600,
                        color: Colors.white)),
              ],
            ],
          ),
        ),
      ),
    );
    return tooltip == null ? button : Tooltip(message: tooltip!, child: button);
  }
}

class _CardMenu extends StatelessWidget {
  const _CardMenu({
    required this.controller,
    required this.video,
    this.extra = const [],
  });

  final MusicController controller;
  final YtVideo video;
  final List<(String, VoidCallback)> extra;

  @override
  Widget build(BuildContext context) => PopupMenuButton<String>(
        iconSize: 18,
        padding: EdgeInsets.zero,
        tooltip: 'More',
        onSelected: (choice) async {
          final c = controller;
          final v = video;
          switch (choice) {
            case 'details':
              await showYtDetails(context, c, v);
            case 'next':
              await c.send(MusicCmd.ytQueueAdd(videoId: v.videoId, next: true));
            case 'queue':
              await c.send(MusicCmd.ytQueueAdd(videoId: v.videoId, next: false));
            case 'watched':
              await c.send(MusicCmd.ytMarkWatched(
                  videoId: v.videoId, watched: v.progress < 1.0));
            case 'formats':
              await showFormatSheet(context, c, v);
            case 'download':
              await showFormatSheet(context, c, v, mode: FormatMode.download);
            case 'playlist':
              await addToYtPlaylist(context, c, v);
            case 'channel':
              await c.send(MusicCmd.ytOpenChannel(channelId: v.channelId));
            default:
              if (choice.startsWith('x:')) extra[int.parse(choice.substring(2))].$2();
          }
        },
        itemBuilder: (_) => [
          const PopupMenuItem(value: 'details', child: Text('Details…')),
          const PopupMenuItem(value: 'next', child: Text('Play next')),
          const PopupMenuItem(value: 'queue', child: Text('Add to queue')),
          PopupMenuItem(
              value: 'watched',
              child: Text(video.progress >= 1.0
                  ? 'Mark as unwatched'
                  : 'Mark as watched')),
          const PopupMenuDivider(),
          const PopupMenuItem(value: 'formats', child: Text('Formats…')),
          const PopupMenuItem(value: 'download', child: Text('Download…')),
          const PopupMenuItem(
              value: 'playlist', child: Text('Add to playlist…')),
          if (video.channelId.isNotEmpty)
            const PopupMenuItem(value: 'channel', child: Text('Open channel')),
          if (extra.isNotEmpty) const PopupMenuDivider(),
          for (final (i, (label, _)) in extra.indexed)
            PopupMenuItem(value: 'x:$i', child: Text(label)),
        ],
      );
}

/// A saved playlist: its cover fanned three deep, the count on the front one.
class YtPlaylistCard extends StatelessWidget {
  const YtPlaylistCard({
    super.key,
    required this.controller,
    required this.playlist,
    required this.playing,
  });

  final MusicController controller;
  final YtPlaylist playlist;
  final bool playing;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final p = playlist;
    final c = controller;
    // A cover is YouTube's own still until the first video is cached.
    final direct = p.cover.startsWith('http') ? null : p.cover;
    Widget cover(double inset, double opacity) => Positioned.fill(
          left: inset,
          right: inset,
          top: 0,
          bottom: inset,
          child: Opacity(
            opacity: opacity,
            child: ClipRRect(
              borderRadius: BorderRadius.circular(Tokens.radiusMd),
              child: MusicArt(
                controller: c,
                kind: 'yt',
                artKey: p.cover,
                direct: direct,
                size: 320,
                radius: 0,
                fallback: Icons.playlist_play,
              ),
            ),
          ),
        );
    return InkWell(
      borderRadius: BorderRadius.circular(Tokens.radiusMd),
      onTap: () => c.send(MusicCmd.ytOpenPlaylist(playlistId: p.id)),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          AspectRatio(
            aspectRatio: 16 / 10,
            child: Stack(
              children: [
                cover(20, 0.35),
                cover(10, 0.6),
                Positioned.fill(
                  top: 10,
                  child: ClipRRect(
                    borderRadius: BorderRadius.circular(Tokens.radiusMd),
                    child: Stack(
                      fit: StackFit.expand,
                      children: [
                        MusicArt(
                          controller: c,
                          kind: 'yt',
                          artKey: p.cover,
                          direct: direct,
                          size: 320,
                          radius: 0,
                          fallback: Icons.playlist_play,
                        ),
                        Positioned(
                          right: 8,
                          bottom: 8,
                          child: _Badge(
                            icon: Icons.playlist_play,
                            text: '${p.count}',
                          ),
                        ),
                        Positioned(
                          left: 8,
                          bottom: 8,
                          child: _QuickButton(
                            icon: Icons.play_arrow_rounded,
                            label: playing ? 'Playing' : 'Play',
                            onTap: () => c.send(
                                MusicCmd.ytPlaylistPlayAll(playlistId: p.id)),
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(height: 8),
          Text(p.name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 13,
                  fontWeight: FontWeight.w600,
                  color: playing ? ytRose : t.nInk)),
          Text(
            p.sourceUrl.isEmpty
                ? 'Local · ${p.count} videos'
                : 'Imported · ${p.count} videos',
            style: TextStyle(fontSize: 11, color: t.nInk3),
          ),
        ],
      ),
    );
  }
}

/// Cards in as many columns of at least 200 px as the width holds, up to
/// [maxColumns].
class YtGrid extends StatelessWidget {
  const YtGrid({
    super.key,
    required this.children,
    this.padding = 24,
    this.maxColumns = 6,
  });

  final List<Widget> children;
  final int maxColumns;

  /// Left and right, for a page whose list is not already padded.
  final double padding;

  @override
  Widget build(BuildContext context) => Padding(
        padding: EdgeInsets.symmetric(horizontal: padding),
        child: LayoutBuilder(builder: (context, box) {
          const gap = 18.0;
          final cols = (box.maxWidth / 200).floor().clamp(1, maxColumns);
          final w = (box.maxWidth - gap * (cols - 1)) / cols;
          return Wrap(
            spacing: gap,
            runSpacing: 22,
            children: [
              for (final child in children) SizedBox(width: w, child: child),
            ],
          );
        }),
      );
}

class YtVideoGrid extends StatelessWidget {
  const YtVideoGrid({
    super.key,
    required this.controller,
    required this.videos,
    this.maxColumns = 6,
  });

  final MusicController controller;
  final List<YtVideo> videos;
  final int maxColumns;

  @override
  Widget build(BuildContext context) => YtGrid(maxColumns: maxColumns, children: [
        for (final v in videos) YtVideoCard(controller: controller, video: v),
      ]);
}
