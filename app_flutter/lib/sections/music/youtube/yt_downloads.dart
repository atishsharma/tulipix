// YouTube › Downloads: what is downloading, and what has been kept.
//
// The job rows live here and are shared: the strip over every other tab shows
// the running job and a count, this page shows each one. Finished downloads
// are files in a folder the user chose, so every row says where, and how big.

import 'dart:io';

import 'package:flutter/material.dart';

import '../../../design/tokens.dart';
import '../../../src/rust/api/music.dart';
import '../music_controller.dart';
import '../music_dialogs.dart';
import '../music_widgets.dart';
import 'yt_card.dart';

class YtDownloads extends StatefulWidget {
  const YtDownloads({super.key, required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  State<YtDownloads> createState() => _YtDownloadsState();
}

class _YtDownloadsState extends State<YtDownloads> {
  Future<YtStorage>? _storage;
  String _storageKey = '';

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = widget.controller;
    final st = widget.st;
    // Re-total only when the list changes, not on every progress tick.
    final key = '${st.ytDlTotal}|${st.ytJobs.length}';
    if (key != _storageKey) {
      _storageKey = key;
      _storage = musicYtStorage();
    }
    final p = st.ytDlPrefs;

    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 12, 24, 32),
      children: [
        DecoratedBox(
          decoration: BoxDecoration(
            color: t.nCard,
            border: Border.all(color: t.nHair),
            borderRadius: BorderRadius.circular(Tokens.radiusLg),
          ),
          child: Padding(
            padding: const EdgeInsets.fromLTRB(16, 12, 16, 12),
            child: Wrap(
              spacing: 14,
              runSpacing: 8,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                Text('Next download',
                    style: TextStyle(
                        fontSize: 13, fontWeight: FontWeight.w600, color: t.nInk)),
                _Pill(icon: Icons.headphones_outlined, text: p.audioPreset.toUpperCase()),
                _Pill(icon: Icons.movie_outlined, text: p.videoPreset.toUpperCase()),
                Text(p.template,
                    style: TextStyle(
                        fontSize: 11, color: t.nInk2, fontFeatures: const [FontFeature.tabularFigures()])),
                FutureBuilder<YtStorage>(
                  future: _storage,
                  builder: (context, snap) {
                    final s = snap.data;
                    if (s == null) return const SizedBox.shrink();
                    return Text(
                      '${ytSize(s.downloadsAudioBytes + s.downloadsVideoBytes)} on disk',
                      style: TextStyle(fontSize: 12, color: t.nInk3),
                    );
                  },
                ),
              ],
            ),
          ),
        ),
        if (st.ytJobs.isNotEmpty) ...[
          _Heading(text: 'In progress', trailing: '${st.ytJobs.length}'),
          YtJobs(controller: c, st: st, all: true),
        ],
        _Heading(
          text: 'Finished',
          trailing: st.ytDlTotal > 0 ? '${st.ytDlTotal}' : null,
          actions: [
            for (final m in const [('new', 'Newest'), ('old', 'Oldest'), ('az', 'A–Z')])
              SortChip(
                label: m.$2,
                active: st.ytDlSort == m.$1,
                onTap: () => c.send(MusicCmd.ytSetDlSort(mode: m.$1)),
              ),
            TextButton.icon(
              icon: const Icon(Icons.delete_sweep_outlined, size: 16),
              label: const Text('Delete all'),
              onPressed: st.ytDownloads.isEmpty
                  ? null
                  : () async {
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
          ],
        ),
        if (st.ytDownloads.isEmpty)
          const MusicEmpty(
            icon: Icons.download_outlined,
            title: 'No downloads',
            body: 'Pick Download… on any video to keep a file in the format '
                'you want. The cache is disposable; these are not.',
          )
        else
          // The list already has the page's side padding.
          YtGrid(padding: 0, children: [
            for (final v in st.ytDownloads) _DownloadCard(controller: c, video: v),
          ]),
      ],
    );
  }
}

class _Pill extends StatelessWidget {
  const _Pill({required this.icon, required this.text});

  final IconData icon;
  final String text;

  @override
  Widget build(BuildContext context) => DecoratedBox(
        decoration: BoxDecoration(
          color: ytRose.withValues(alpha: 0.12),
          borderRadius: BorderRadius.circular(6),
        ),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(icon, size: 13, color: ytRose),
              const SizedBox(width: 5),
              Text(text,
                  style: const TextStyle(
                      fontSize: 11, fontWeight: FontWeight.w600, color: ytRose)),
            ],
          ),
        ),
      );
}

class _Heading extends StatelessWidget {
  const _Heading({required this.text, this.trailing, this.actions = const []});

  final String text;
  final String? trailing;
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.fromLTRB(0, 24, 0, 10),
      child: Wrap(
        spacing: 8,
        runSpacing: 6,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          Text(text,
              style: TextStyle(
                  fontSize: 17,
                  fontWeight: FontWeight.w700,
                  letterSpacing: -0.3,
                  color: t.nInk)),
          if (trailing != null)
            Text(trailing!, style: TextStyle(fontSize: 12, color: t.nInk3)),
          const SizedBox(width: 12),
          ...actions,
        ],
      ),
    );
  }
}

/// A kept file as a card: what it was kept as and how big on the line under
/// the title, the folder and deleting it in the menu.
class _DownloadCard extends StatelessWidget {
  const _DownloadCard({required this.controller, required this.video});

  final MusicController controller;
  final YtVideo video;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final v = video;
    final onDisk = v.bytes >= 0;
    return YtVideoCard(
      controller: c,
      video: v,
      subtitle: [
        if (v.quality.isNotEmpty) v.quality,
        onDisk ? ytSize(v.bytes) : 'File missing',
      ].join(' · '),
      extraMenu: [
        if (onDisk)
          (
            'Show in folder',
            () => c.send(
                MusicCmd.ytShowInFolder(path: File(v.mediaPath).parent.path))
          ),
        (
          'Delete the file',
          () async {
            final ok = await confirm(
              context,
              title: 'Delete this download?',
              body: '“${v.title}”${v.quality.isEmpty ? '' : ' (${v.quality})'} goes from disk.',
              action: 'Delete',
            );
            // By file: the same video can be kept at several qualities.
            if (ok) await c.send(MusicCmd.ytRemoveDownload(mediaPath: v.mediaPath));
          }
        ),
      ],
    );
  }
}

/// What yt-dlp is doing. A download used to be a button press with no visible
/// consequence until a file appeared in another tab minutes later.
class YtJobs extends StatelessWidget {
  const YtJobs({
    super.key,
    required this.controller,
    required this.st,
    this.all = false,
  });

  final MusicController controller;
  final MusicState st;

  /// Every job with its own row (the Downloads page), rather than the running
  /// one with a count of the rest (the strip over every other tab).
  final bool all;

  @override
  Widget build(BuildContext context) {
    final failed = st.ytJobs.where((j) => j.error.isNotEmpty).toList();
    final live = st.ytJobs.where((j) => j.error.isEmpty).toList();
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 0, 20, 8),
      child: Column(
        children: [
          if (all)
            for (final j in live)
              Padding(
                padding: const EdgeInsets.only(bottom: 8),
                child: _JobRow(controller: controller, job: j, waiting: 0),
              )
          else if (live.isNotEmpty)
            _JobRow(
                controller: controller,
                job: live.first,
                waiting: live.length - 1),
          for (final j in failed)
            _FailedJobRow(controller: controller, job: j),
        ],
      ),
    );
  }
}

String _mb(num bytes) => '${(bytes / (1024 * 1024)).toStringAsFixed(1)} MB';

String _left(int s) => s >= 3600
    ? '${s ~/ 3600}:${(s % 3600 ~/ 60).toString().padLeft(2, '0')}:${(s % 60).toString().padLeft(2, '0')}'
    : '${s ~/ 60}:${(s % 60).toString().padLeft(2, '0')}';

class _JobRow extends StatelessWidget {
  const _JobRow(
      {required this.controller, required this.job, required this.waiting});

  final MusicController controller;
  final YtJob job;
  final int waiting;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final j = job;
    final downloading =
        !j.paused && (j.stage.isEmpty || j.stage == 'Downloading');
    final facts = <String>[
      j.label,
      if (j.paused) 'Paused' else if (j.stage.isNotEmpty) j.stage,
      if (j.paused && j.totalBytes > 0)
        '${_mb(j.totalBytes * j.frac)} of ${_mb(j.totalBytes)}',
      if (downloading && j.totalBytes > 0)
        '${_mb(j.totalBytes * j.frac)} / ${_mb(j.totalBytes)}',
      if (downloading && j.speedBps > 0) '${_mb(j.speedBps)}/s',
      if (downloading && j.etaS >= 0) '${_left(j.etaS)} left',
      if (waiting > 0) '$waiting waiting',
    ];
    return Row(
      children: [
        const Icon(Icons.downloading, size: 16, color: Color(0xFFF43F5E)),
        const SizedBox(width: 8),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                j.title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 12, fontWeight: FontWeight.w600, color: t.nInk),
              ),
              Text(
                facts.join('  ·  '),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: 11,
                  color: t.nInk2,
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
              const SizedBox(height: 4),
              ClipRRect(
                borderRadius: BorderRadius.circular(3),
                child: j.paused
                    ? LinearProgressIndicator(
                        value: j.frac.clamp(0.0, 1.0),
                        minHeight: 3,
                        backgroundColor: t.nHair,
                        valueColor: AlwaysStoppedAnimation(t.nInk3),
                      )
                    : downloading
                    ? LinearProgressIndicator(
                        // Indeterminate while yt-dlp resolves formats.
                        value: j.frac <= 0 ? null : j.frac.clamp(0.0, 1.0),
                        minHeight: 3,
                        backgroundColor: t.nHair,
                        valueColor:
                            const AlwaysStoppedAnimation(Color(0xFFF43F5E)),
                      )
                    // Merging, converting, tagging: work with no percent.
                    : SizedBox(
                        height: 6,
                        width: double.infinity,
                        child: _Stripes(ground: t.nHair),
                      ),
              ),
            ],
          ),
        ),
        const SizedBox(width: 10),
        if (downloading)
          Text('${(j.frac * 100).round()}%',
              style: TextStyle(fontSize: 11, color: t.nInk2)),
        // Pause keeps the partial file; Resume carries on from it. Merging
        // and tagging are too close to done to be worth stopping.
        if (j.paused) ...[
          TextButton(
            onPressed: () =>
                controller.send(MusicCmd.ytResumeJob(videoId: j.videoId)),
            child: const Text('Resume'),
          ),
          TextButton(
            style: musicQuietStyle(context),
            onPressed: () =>
                controller.send(MusicCmd.ytDismissJob(videoId: j.videoId)),
            child: const Text('Dismiss'),
          ),
        ] else if (downloading && j.running)
          IconButton(
            tooltip: 'Pause',
            iconSize: 18,
            icon: const Icon(Icons.pause_rounded),
            onPressed: () =>
                controller.send(MusicCmd.ytPauseJob(videoId: j.videoId)),
          ),
      ],
    );
  }
}

/// Moving stripes: running, and no fraction to pretend to. Still under reduced
/// motion.
class _Stripes extends StatefulWidget {
  const _Stripes({required this.ground});

  final Color ground;

  @override
  State<_Stripes> createState() => _StripesState();
}

class _StripesState extends State<_Stripes>
    with SingleTickerProviderStateMixin {
  late final AnimationController _a = AnimationController(
      vsync: this, duration: const Duration(milliseconds: 700));

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (MediaQuery.disableAnimationsOf(context)) {
      _a.stop();
    } else if (!_a.isAnimating) {
      _a.repeat();
    }
  }

  @override
  void dispose() {
    _a.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => CustomPaint(
        painter: _StripePainter(_a, widget.ground),
      );
}

class _StripePainter extends CustomPainter {
  _StripePainter(this.t, this.ground) : super(repaint: t);

  final Animation<double> t;
  final Color ground;

  static const _period = 12.0;
  static const _color = Color(0xFFF43F5E);

  @override
  void paint(Canvas canvas, Size size) {
    final box = Offset.zero & size;
    canvas.drawRect(box, Paint()..color = ground);
    canvas.save();
    canvas.clipRect(box);
    final stripe = Paint()
      ..color = _color
      ..strokeWidth = _period / 2;
    final h = size.height;
    for (var x = -h - _period + t.value * _period;
        x < size.width + h;
        x += _period) {
      canvas.drawLine(Offset(x, h), Offset(x + h, 0), stripe);
    }
    canvas.restore();
  }

  @override
  bool shouldRepaint(_StripePainter old) => old.ground != ground;
}

class _FailedJobRow extends StatelessWidget {
  const _FailedJobRow({required this.controller, required this.job});

  final MusicController controller;
  final YtJob job;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(top: 6),
      child: Row(
        children: [
          const Icon(Icons.error_outline, size: 16, color: Tokens.error),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text('${job.title}  ·  ${job.label}',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 12,
                        fontWeight: FontWeight.w600,
                        color: t.nInk)),
                Text(job.error,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11, color: t.nInk2)),
              ],
            ),
          ),
          TextButton(
            onPressed: () =>
                controller.send(MusicCmd.ytRetryJob(videoId: job.videoId)),
            child: const Text('Retry'),
          ),
          TextButton(
            style: musicQuietStyle(context),
            onPressed: () =>
                controller.send(MusicCmd.ytDismissJob(videoId: job.videoId)),
            child: const Text('Dismiss'),
          ),
        ],
      ),
    );
  }
}
