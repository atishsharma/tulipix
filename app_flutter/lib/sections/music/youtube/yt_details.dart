// A video's page, as a panel: views, likes, when it went up, the description,
// and its chapters, which jump there when it is the one playing.
//
// It reads the same saved `yt-dlp -J` answer the format panel does, so a video
// already played or looked at opens at once.

import 'package:flutter/material.dart';

import '../../../design/tokens.dart';
import '../../../src/rust/api/music.dart';
import '../music_controller.dart';
import '../music_dialogs.dart';
import 'yt_card.dart';
import 'yt_format_sheet.dart';

Future<void> showYtDetails(
    BuildContext context, MusicController c, YtVideo video) {
  final info = musicYtFormats(videoId: video.videoId);
  return showDialog<void>(
    context: context,
    builder: (ctx) => Dialog(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 720, maxHeight: 680),
        child: FutureBuilder<YtStreamInfo>(
          future: info,
          builder: (ctx, snap) => _Details(
            host: context,
            controller: c,
            video: video,
            info: snap.data,
            error: snap.hasError ? '${snap.error}' : null,
          ),
        ),
      ),
    ),
  );
}

const _months = [
  'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun',
  'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
];

/// `20240312` → `12 Mar 2024`.
String _date(String d) {
  if (d.length != 8) return '';
  final m = int.tryParse(d.substring(4, 6)) ?? 0;
  if (m < 1 || m > 12) return '';
  return '${int.parse(d.substring(6))} ${_months[m - 1]} ${d.substring(0, 4)}';
}

String _count(int n, String unit) {
  String one(double v, String s) =>
      '${v >= 10 ? v.round() : v.toStringAsFixed(1)}$s';
  final text = n >= 1000000000
      ? one(n / 1e9, 'B')
      : n >= 1000000
          ? one(n / 1e6, 'M')
          : n >= 1000
              ? one(n / 1e3, 'K')
              : '$n';
  return '$text $unit';
}

class _Details extends StatelessWidget {
  const _Details({
    required this.host,
    required this.controller,
    required this.video,
    required this.info,
    required this.error,
  });

  /// The page that opened the panel: what opens after it closes needs a
  /// context that is still there.
  final BuildContext host;
  final MusicController controller;
  final YtVideo video;
  final YtStreamInfo? info;
  final String? error;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = controller;
    final i = info;
    final v = video;
    // On the audio deck: a picture on screen has its own seek bar.
    final playing = c.now?.mode == 'youtube' &&
        c.now?.key == v.videoId &&
        !(c.state?.ytWatching ?? false);
    final title = i != null && i.title.isNotEmpty ? i.title : v.title;
    final link = YtVideo(
      videoId: v.videoId,
      title: title,
      channel: i != null && i.channel.isNotEmpty ? i.channel : v.channel,
      thumb: v.thumb,
      duration: v.duration,
      mediaPath: v.mediaPath,
      meta: v.meta,
      progress: v.progress,
      quality: v.quality,
      channelId: i != null && i.channelId.isNotEmpty ? i.channelId : v.channelId,
      offline: v.offline,
      bytes: v.bytes,
    );
    final facts = [
      if (i != null && i.views >= 0) _count(i.views, 'views'),
      if (i != null && i.likes >= 0) _count(i.likes, 'likes'),
      if (i != null) _date(i.uploadDate),
      if (v.duration > 0) fmtClock(v.duration.toDouble()),
    ].where((x) => x.isNotEmpty).join('  ·  ');

    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              SizedBox(
                width: 192,
                height: 108,
                child: YtThumb(controller: c, video: link),
              ),
              const SizedBox(width: 16),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    // minLines: a SelectableText with only maxLines is that
                    // many lines tall, however short the title.
                    SelectableText(title,
                        minLines: 1,
                        maxLines: 3,
                        style: TextStyle(
                            fontSize: 17,
                            fontWeight: FontWeight.w700,
                            color: t.nInk)),
                    if (link.channel.isNotEmpty)
                      InkWell(
                        // The dialog goes first, or the channel opens under it.
                        onTap: link.channelId.isEmpty
                            ? null
                            : () {
                                Navigator.pop(context);
                                c.send(MusicCmd.ytOpenChannel(
                                    channelId: link.channelId));
                              },
                        child: Text(link.channel,
                            style: TextStyle(
                                fontSize: 13,
                                color: link.channelId.isEmpty
                                    ? t.nInk2
                                    : ytRose)),
                      ),
                    if (facts.isNotEmpty) ...[
                      const SizedBox(height: 4),
                      Text(facts,
                          style: TextStyle(
                              fontSize: 12,
                              color: t.nInk3,
                              fontFeatures: const [
                                FontFeature.tabularFigures()
                              ])),
                    ],
                  ],
                ),
              ),
              IconButton(
                tooltip: 'Close',
                icon: const Icon(Icons.close),
                onPressed: () => Navigator.pop(context),
              ),
            ],
          ),
          const SizedBox(height: 12),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              FilledButton.icon(
                style: musicFilledStyle(fill: ytRose),
                icon: const Icon(Icons.headphones_outlined, size: 16),
                label: const Text('Listen'),
                onPressed: () {
                  Navigator.pop(context);
                  playYtAudio(c, v);
                },
              ),
              OutlinedButton.icon(
                icon: const Icon(Icons.smart_display_outlined, size: 16),
                label: const Text('Watch'),
                onPressed: () {
                  Navigator.pop(context);
                  watchVideo(host, c, v);
                },
              ),
              FilledButton.icon(
                style: ytDownloadStyle(),
                icon: const Icon(Icons.download_rounded, size: 16),
                label: const Text('Download…'),
                onPressed: () {
                  Navigator.pop(context);
                  showFormatSheet(host, c, v, mode: FormatMode.download);
                },
              ),
            ],
          ),
          const SizedBox(height: 12),
          Divider(height: 1, color: t.nHair),
          Flexible(
            child: i == null
                ? Padding(
                    padding: const EdgeInsets.all(24),
                    child: Center(
                      child: error != null
                          ? Text(error!, style: TextStyle(color: t.nInk2))
                          : const CircularProgressIndicator(),
                    ),
                  )
                : ListView(
                    shrinkWrap: true,
                    padding: const EdgeInsets.only(top: 12),
                    children: [
                      if (i.chapters.isNotEmpty) ...[
                        _label(context, 'Chapters'),
                        for (final ch in i.chapters)
                          ListTile(
                            dense: true,
                            visualDensity: VisualDensity.compact,
                            contentPadding: EdgeInsets.zero,
                            leading: SizedBox(
                              width: 56,
                              child: Text(fmtClock(ch.startS),
                                  style: TextStyle(
                                      fontSize: 12,
                                      color: playing ? ytRose : t.nInk2,
                                      fontFeatures: const [
                                        FontFeature.tabularFigures()
                                      ])),
                            ),
                            title: Text(ch.title,
                                style: TextStyle(fontSize: 13, color: t.nInk)),
                            // Only the video playing has a position to move.
                            onTap: playing
                                ? () => c.send(MusicCmd.seek(secs: ch.startS))
                                : null,
                          ),
                        const SizedBox(height: 12),
                      ],
                      _label(context, 'Description'),
                      SelectableText(
                        i.description.isEmpty
                            ? 'No description.'
                            : i.description,
                        style: TextStyle(
                            fontSize: 13, height: 1.45, color: t.nInk2),
                      ),
                    ],
                  ),
          ),
        ],
      ),
    );
  }

  Widget _label(BuildContext context, String text) => Padding(
        padding: const EdgeInsets.only(bottom: 6),
        child: Text(text.toUpperCase(),
            style: TextStyle(
                fontSize: 11,
                letterSpacing: 1.4,
                fontWeight: FontWeight.w600,
                color: context.tokens.nInk3)),
      );
}
