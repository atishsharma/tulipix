// The format panel: every stream a video has, and every file it can become.
//
// Play lists picture streams (played with the best sound -- the reason Watch
// is HD rather than the 360p muxed stream it used to get) and sound streams
// alone. Download lists presets with their sizes estimated against this
// video's own streams, the extras yt-dlp can write into the file, and where
// the file goes. Both halves read one `music_yt_formats` answer.

import 'package:flutter/material.dart';

import '../../../platform/pick.dart';
import '../../../design/tokens.dart';
import '../../../src/rust/api/music.dart';
import '../music_controller.dart';
import '../music_dialogs.dart';
import '../music_widgets.dart';

/// The YouTube tab's own rose, as its chips already use.
const Color _yt = Color(0xFFF43F5E);

const String _defaultTemplate =
    '~/Music/YouTube/%(channel,uploader)s/%(title)s.%(ext)s';

enum FormatMode { play, download }

Future<void> showFormatSheet(
  BuildContext context,
  MusicController c,
  YtVideo v, {
  FormatMode mode = FormatMode.play,
  List<YtVideo> batch = const [],
}) {
  final still = context.tokens.reduceMotion;
  return showGeneralDialog(
    context: context,
    barrierDismissible: true,
    barrierLabel: 'Close formats',
    barrierColor: Colors.black38,
    transitionDuration:
        still ? Duration.zero : const Duration(milliseconds: 220),
    pageBuilder: (ctx, _, __) => Align(
      alignment: Alignment.centerRight,
      // A batch is a download: every video gets the spec picked against the
      // first one's streams.
      child: _FormatSheet(
        controller: c,
        video: v,
        mode: batch.length > 1 ? FormatMode.download : mode,
        batch: batch,
      ),
    ),
    transitionBuilder: (ctx, anim, _, child) => SlideTransition(
      position: Tween(begin: const Offset(0.06, 0), end: Offset.zero)
          .animate(CurvedAnimation(parent: anim, curve: Curves.easeOutCubic)),
      child: FadeTransition(opacity: anim, child: child),
    ),
  );
}

String _size(double bytes) {
  if (bytes >= 1024 * 1024 * 1024) {
    return '${(bytes / (1024 * 1024 * 1024)).toStringAsFixed(2)} GB';
  }
  final mb = bytes / (1024 * 1024);
  return mb >= 100 ? '${mb.round()} MB' : '${mb.toStringAsFixed(1)} MB';
}

String _rate(int kbps) => kbps >= 1000
    ? '${(kbps / 1000).toStringAsFixed(1)} Mb/s'
    : '$kbps kb/s';

String _audioName(String codec) => switch (codec) {
      'opus' => 'Opus',
      'mp4a' => 'AAC',
      _ => codec.toUpperCase(),
    };

/// The extension a preset key ends up with, for the "Saves as" example only;
/// yt-dlp decides the real one.
String _extOf(String key) => switch (key) {
      'vorbis' => 'ogg',
      _ when key.startsWith('mp4-') || key.startsWith('av1-') => 'mp4',
      _ when key.startsWith('webm-') => 'webm',
      'mkv-best' => 'mkv',
      _ => key,
    };

class _FormatSheet extends StatefulWidget {
  const _FormatSheet({
    required this.controller,
    required this.video,
    required this.mode,
    this.batch = const [],
  });

  final MusicController controller;
  final YtVideo video;
  final FormatMode mode;

  /// Download all of these, not just [video]. Two or more hides Play.
  final List<YtVideo> batch;

  bool get isBatch => batch.length > 1;

  @override
  State<_FormatSheet> createState() => _FormatSheetState();
}

class _FormatSheetState extends State<_FormatSheet> {
  late Future<YtStreamInfo> _info;
  YtStreamInfo? _loaded;
  late FormatMode _mode = widget.mode;

  // Play
  String? _videoId;
  String? _audioId;
  bool _asVideo = true;
  bool _remember = false;

  // Download
  late bool _fileIsAudio;
  late String _audioPreset;
  late String _videoPreset;
  late bool _cover, _tags, _chapters, _sponsorblock, _subs;
  late final TextEditingController _template;

  @override
  void initState() {
    super.initState();
    final p = widget.controller.state?.ytDlPrefs;
    _audioPreset = p?.audioPreset ?? 'opus';
    _videoPreset = p?.videoPreset ?? 'mp4-1080';
    _fileIsAudio = true;
    _cover = p?.cover ?? true;
    _tags = p?.tags ?? true;
    _chapters = p?.chapters ?? true;
    _sponsorblock = p?.sponsorblock ?? false;
    _subs = p?.subs ?? false;
    _template = TextEditingController(
        text: (p?.template.isNotEmpty ?? false) ? p!.template : _defaultTemplate);
    _template.addListener(() => setState(() {}));
    _load();
  }

  @override
  void dispose() {
    _template.dispose();
    super.dispose();
  }

  void _load() {
    _info = musicYtFormats(videoId: widget.video.videoId).then((i) {
      if (mounted) setState(() => _preselect(i));
      return i;
    });
  }

  /// The row the watch default would have played, so opening the panel and
  /// pressing Play changes nothing unless you pick something.
  void _preselect(YtStreamInfo i) {
    _loaded = i;
    final capped = i.video
        .where((f) => i.watchHeight == 0 || f.height <= i.watchHeight)
        .toList();
    final pool = capped.isEmpty ? i.video : capped;
    // `any` is the bridge's "cheapest to decode": H.264, then VP9, then the
    // best there is -- the same order Watch uses without the panel. A cap
    // above 1080p, where there is no H.264, takes the taller picture first.
    final order = i.watchCodec == 'any' ? const ['avc1', 'vp09'] : [i.watchCodec];
    final tall = i.watchCodec == 'any' && i.watchHeight > 1080
        ? [
            ...pool.where((f) => f.height > 1080 && f.codec == 'vp09'),
            ...pool.where((f) => f.height > 1080),
          ]
        : const <YtVideoFormat>[];
    final match = [
      ...tall,
      for (final codec in order) ...pool.where((f) => f.codec == codec),
    ];
    _videoId = (match.isNotEmpty ? match.first : pool.firstOrNull)?.id;
    _audioId = i.audio.firstOrNull?.id;
    _asVideo = _videoId != null;
  }

  String get _presetKey => _fileIsAudio ? _audioPreset : _videoPreset;

  YtPreset? get _preset =>
      _loaded?.presets.where((p) => p.key == _presetKey).firstOrNull;

  double _playEstimate(YtStreamInfo i) {
    final a = i.audio.where((f) => f.id == _audioId).firstOrNull;
    double bytesOf(int bytes, int kbps) =>
        bytes >= 0 ? bytes.toDouble() : i.durationS * kbps * 1000 / 8;
    final audio = a == null ? 0.0 : bytesOf(a.bytes, a.kbps);
    if (!_asVideo) return audio;
    final v = i.video.where((f) => f.id == _videoId).firstOrNull;
    return audio + (v == null ? 0.0 : bytesOf(v.bytes, v.kbps));
  }

  Future<void> _play() async {
    final c = widget.controller;
    final id = widget.video.videoId;
    final i = _loaded;
    if (_asVideo && i != null) {
      final f = i.video.firstWhere((x) => x.id == _videoId);
      if (_remember) {
        await c.send(MusicCmd.ytSetWatchPref(height: f.height, codec: f.codec));
      }
      await c.send(MusicCmd.ytWatch(videoId: id, formatId: f.id));
    } else {
      await c.send(MusicCmd.ytPlay(videoId: id, formatId: _audioId ?? ''));
    }
    if (mounted) Navigator.pop(context);
  }

  Future<void> _download() async {
    final spec = YtDownloadSpec(
      preset: _presetKey,
      cover: _cover,
      tags: _tags,
      chapters: _chapters,
      sponsorblock: _sponsorblock,
      subs: _subs,
      template: _template.text.trim(),
    );
    // The job row takes over from here; the panel has nothing left to say.
    Navigator.pop(context);
    for (final v in widget.isBatch ? widget.batch : [widget.video]) {
      await widget.controller
          .send(MusicCmd.ytDownload(videoId: v.videoId, spec: spec));
    }
  }

  Future<void> _pickFolder() async {
    final dir = await pickDirectory(title: 'Save downloads to');
    if (dir == null) return;
    _template.text = '$dir/%(channel,uploader)s/%(title)s.%(ext)s';
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final screen = MediaQuery.sizeOf(context);
    final v = widget.video;
    return Material(
      color: t.modalSolid,
      elevation: 12,
      child: SizedBox(
        width: screen.width < 600 ? screen.width : 540,
        height: screen.height,
        child: Column(
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(20, 20, 12, 12),
              child: Row(
                children: [
                  SizedBox(
                    width: 128,
                    height: 72,
                    child: MusicArt(
                      controller: widget.controller,
                      kind: 'yt',
                      artKey: v.thumb,
                      direct: v.thumb.startsWith('http') ? null : v.thumb,
                      size: 128,
                      radius: 10,
                      fallback: Icons.smart_display,
                    ),
                  ),
                  const SizedBox(width: 14),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                            widget.isBatch
                                ? 'Download ${widget.batch.length} videos'
                                : v.title,
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontSize: 15,
                                fontWeight: FontWeight.w700,
                                color: t.nInk)),
                        const SizedBox(height: 3),
                        Text(v.channel,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 12, color: t.nInk2)),
                      ],
                    ),
                  ),
                  IconButton(
                    tooltip: 'Close',
                    icon: const Icon(Icons.close, size: 20),
                    onPressed: () => Navigator.pop(context),
                  ),
                ],
              ),
            ),
            if (!widget.isBatch)
              Padding(
                padding: const EdgeInsets.fromLTRB(20, 0, 20, 12),
                child: _Segment(
                  items: const [
                    (FormatMode.play, Icons.play_arrow_rounded, 'Play'),
                    (FormatMode.download, Icons.download_rounded, 'Download'),
                  ],
                  selected: _mode,
                  onPick: (m) => setState(() => _mode = m),
                ),
              ),
            Divider(height: 1, color: t.nHair),
            Expanded(
              child: FutureBuilder<YtStreamInfo>(
                future: _info,
                builder: (context, snap) {
                  if (snap.hasError) return _error(context, snap.error!);
                  final i = snap.data;
                  if (i == null) return _loading(context);
                  return _mode == FormatMode.play
                      ? _playBody(context, i)
                      : _downloadBody(context, i);
                },
              ),
            ),
            if (_loaded != null)
              _mode == FormatMode.play
                  ? _playFooter(context, _loaded!)
                  : _downloadFooter(context),
          ],
        ),
      ),
    );
  }

  // ------------------------------------------------------------------ play --

  Widget _playBody(BuildContext context, YtStreamInfo i) => ListView(
        padding: const EdgeInsets.all(20),
        children: [
          if (i.video.isNotEmpty) ...[
            _groupLabel(context, Icons.smart_display_outlined,
                'Watch as video', 'picture + best sound'),
            _group(context, [
              for (final f in i.video)
                _Row(
                  selected: _asVideo && _videoId == f.id,
                  quality: '${f.height}p${f.fps > 30 ? f.fps : ''}',
                  detail:
                      'itag ${f.id} · ${f.codec} · ${f.ext}${f.hdr ? ' · HDR' : ''}',
                  trailing: _rate(f.kbps),
                  onTap: () => setState(() {
                    _asVideo = true;
                    _videoId = f.id;
                  }),
                ),
            ]),
            const SizedBox(height: 20),
          ],
          if (i.audio.isNotEmpty) ...[
            _groupLabel(context, Icons.headphones_outlined, 'Listen as audio',
                'no picture fetched'),
            _group(context, [
              for (final f in i.audio)
                _Row(
                  selected: !_asVideo && _audioId == f.id,
                  quality: _audioName(f.codec),
                  detail:
                      'itag ${f.id} · ${f.ext} · ${(f.rateHz / 1000).toStringAsFixed(1)} kHz',
                  trailing: _rate(f.kbps),
                  onTap: () => setState(() {
                    _asVideo = false;
                    _audioId = f.id;
                  }),
                ),
            ]),
          ],
        ],
      );

  Widget _playFooter(BuildContext context, YtStreamInfo i) {
    final t = context.tokens;
    return _Footer(children: [
      if (_asVideo)
        Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Checkbox(
              value: _remember,
              activeColor: _yt,
              onChanged: (x) => setState(() => _remember = x ?? false),
            ),
            Text('Make default',
                style: TextStyle(fontSize: 12, color: t.nInk2)),
          ],
        ),
      const SizedBox(width: 8),
      Expanded(
        child: Text(
          '≈ ${_size(_playEstimate(i))} for the whole video',
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          textAlign: TextAlign.end,
          style: TextStyle(fontSize: 12, color: t.nInk3),
        ),
      ),
      const SizedBox(width: 12),
      FilledButton.icon(
        style: musicFilledStyle(fill: _yt),
        icon: const Icon(Icons.play_arrow_rounded, size: 18),
        label: Text(_asVideo ? 'Play video' : 'Play audio'),
        onPressed: (_asVideo ? _videoId : _audioId) == null ? null : _play,
      ),
    ]);
  }

  // -------------------------------------------------------------- download --

  Widget _downloadBody(BuildContext context, YtStreamInfo i) {
    final t = context.tokens;
    final preset = _preset;
    final presets = i.presets.where((p) => p.audio == _fileIsAudio).toList();
    final chapters = i.chapters.length;
    final example = _template.text
        .replaceAll('%(channel,uploader)s', widget.video.channel)
        .replaceAll('%(channel)s', widget.video.channel)
        .replaceAll('%(title)s', widget.video.title)
        .replaceAll('%(ext)s', _extOf(_presetKey));
    return ListView(
      padding: const EdgeInsets.all(20),
      children: [
        _Segment(
          items: const [
            (true, Icons.headphones_outlined, 'Audio file'),
            (false, Icons.movie_outlined, 'Video file'),
          ],
          selected: _fileIsAudio,
          onPick: (a) => setState(() => _fileIsAudio = a),
        ),
        const SizedBox(height: 18),
        _groupLabel(context, Icons.tune, 'Format', ''),
        LayoutBuilder(builder: (context, box) {
          final cols = box.maxWidth < 380 ? 2 : 3;
          final w = (box.maxWidth - 8 * (cols - 1)) / cols;
          return Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              for (final p in presets)
                SizedBox(
                  width: w,
                  child: _PresetTile(
                    preset: p,
                    selected: p.key == _presetKey,
                    onTap: () => setState(() {
                      if (_fileIsAudio) {
                        _audioPreset = p.key;
                      } else {
                        _videoPreset = p.key;
                      }
                    }),
                  ),
                ),
            ],
          );
        }),
        if (preset != null && preset.note.isNotEmpty) ...[
          const SizedBox(height: 10),
          DecoratedBox(
            decoration: BoxDecoration(
              color: t.nChip,
              borderRadius: BorderRadius.circular(Tokens.radiusSm),
            ),
            child: Padding(
              padding: const EdgeInsets.all(10),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  const Icon(Icons.info_outline, size: 15, color: Tokens.warn),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(preset.note,
                        style: TextStyle(fontSize: 12, color: t.nInk2)),
                  ),
                ],
              ),
            ),
          ),
        ],
        const SizedBox(height: 20),
        _groupLabel(context, Icons.auto_awesome_outlined, 'Extras', ''),
        _Toggle(
          title: 'Cover art',
          hint: preset?.embedsCover ?? true
              ? 'The video thumbnail, as the file\'s artwork'
              : 'This format cannot carry a cover',
          value: _cover,
          enabled: preset?.embedsCover ?? true,
          onChanged: (x) => setState(() => _cover = x),
        ),
        _Toggle(
          title: 'Tags',
          hint: 'Title, channel and upload date written into the file',
          value: _tags,
          onChanged: (x) => setState(() => _tags = x),
        ),
        _Toggle(
          title: 'Chapters',
          hint: chapters > 0
              ? '$chapters chapter${chapters == 1 ? '' : 's'} in this video'
              : 'None in this video',
          value: _chapters,
          enabled: chapters > 0,
          onChanged: (x) => setState(() => _chapters = x),
        ),
        _Toggle(
          title: 'Cut sponsor segments',
          hint: 'Sponsor, self-promotion and intros, from SponsorBlock',
          value: _sponsorblock,
          onChanged: (x) => setState(() => _sponsorblock = x),
        ),
        _Toggle(
          title: 'Subtitles',
          hint: _fileIsAudio
              ? 'Video files only'
              : 'English, embedded in the video file',
          value: _subs,
          enabled: !_fileIsAudio,
          onChanged: (x) => setState(() => _subs = x),
        ),
        const SizedBox(height: 20),
        _groupLabel(context, Icons.folder_outlined, 'Save to', ''),
        Row(
          children: [
            Expanded(
              child: TextField(
                controller: _template,
                style: TextStyle(fontSize: 12, color: t.nInk),
                decoration: const InputDecoration(
                  isDense: true,
                  border: OutlineInputBorder(),
                ),
              ),
            ),
            const SizedBox(width: 6),
            IconButton(
              tooltip: 'Choose a folder',
              icon: const Icon(Icons.folder_open_outlined, size: 20),
              onPressed: _pickFolder,
            ),
          ],
        ),
        const SizedBox(height: 6),
        Text('Saves as $example',
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(fontSize: 11, color: t.nInk3)),
      ],
    );
  }

  /// The preset's size for every video in the batch: this video's bytes per
  /// second of its length, over each one's length. A video of unknown length
  /// counts as this one.
  double? _batchBytes(YtPreset preset) {
    final i = _loaded;
    if (i == null || preset.bytes < 0 || i.durationS <= 0) return null;
    final perSecond = preset.bytes / i.durationS;
    return widget.batch.fold<double>(
        0,
        (n, v) =>
            n + (v.duration > 0 ? v.duration * perSecond : preset.bytes.toDouble()));
  }

  Widget _downloadFooter(BuildContext context) {
    final t = context.tokens;
    final preset = _preset;
    final bytes = preset == null || preset.bytes < 0
        ? null
        : widget.isBatch
            ? _batchBytes(preset)
            : preset.bytes.toDouble();
    final size = bytes == null ? '' : _size(bytes);
    return _Footer(children: [
      Expanded(
        child: Text(
          preset == null
              ? ''
              : '${preset.name} · ${preset.detail}'
                  '${size.isEmpty ? '' : '   ≈ $size'}'
                  '${widget.isBatch && size.isNotEmpty ? ' for ${widget.batch.length} videos' : ''}',
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(fontSize: 12, color: t.nInk2),
        ),
      ),
      const SizedBox(width: 12),
      FilledButton.icon(
        style: musicFilledStyle(fill: _yt),
        icon: const Icon(Icons.download_rounded, size: 18),
        label: Text(widget.isBatch
            ? 'Download ${widget.batch.length}${size.isEmpty ? '' : ' · ≈ $size'}'
            : size.isEmpty
                ? 'Download'
                : 'Download · $size'),
        onPressed: _download,
      ),
    ]);
  }

  // ---------------------------------------------------------------- shared --

  Widget _groupLabel(
          BuildContext context, IconData icon, String title, String hint) =>
      Padding(
        padding: const EdgeInsets.only(bottom: 8),
        child: Row(
          children: [
            Icon(icon, size: 15, color: context.tokens.nInk2),
            const SizedBox(width: 6),
            Text(title.toUpperCase(),
                style: TextStyle(
                    fontSize: 11,
                    letterSpacing: 1.2,
                    fontWeight: FontWeight.w600,
                    color: context.tokens.nInk2)),
            const SizedBox(width: 8),
            Text(hint,
                style: TextStyle(fontSize: 11, color: context.tokens.nInk3)),
          ],
        ),
      );

  Widget _group(BuildContext context, List<Widget> rows) => DecoratedBox(
        decoration: BoxDecoration(
          border: Border.all(color: context.tokens.nHair),
          borderRadius: BorderRadius.circular(Tokens.radiusMd),
        ),
        child: ClipRRect(
          borderRadius: BorderRadius.circular(Tokens.radiusMd),
          child: Column(children: rows),
        ),
      );

  Widget _loading(BuildContext context) => ListView(
        padding: const EdgeInsets.all(20),
        children: [
          for (var n = 0; n < 7; n++)
            Container(
              height: 40,
              margin: const EdgeInsets.only(bottom: 6),
              decoration: BoxDecoration(
                color: context.tokens.nChip,
                borderRadius: BorderRadius.circular(Tokens.radiusSm),
              ),
            ),
          const SizedBox(height: 8),
          Text('Asking yt-dlp which streams this video has…',
              style: TextStyle(fontSize: 12, color: context.tokens.nInk3)),
        ],
      );

  Widget _error(BuildContext context, Object error) {
    final t = context.tokens;
    final c = widget.controller;
    final id = widget.video.videoId;
    final download = _mode == FormatMode.download;
    return Padding(
      padding: const EdgeInsets.all(20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text("Couldn't list this video's formats",
              style: TextStyle(
                  fontSize: 14, fontWeight: FontWeight.w600, color: t.nInk)),
          const SizedBox(height: 6),
          Text('$error', style: TextStyle(fontSize: 12, color: t.nInk2)),
          const SizedBox(height: 16),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              FilledButton(
                style: musicFilledStyle(fill: _yt),
                onPressed: download
                    // Sizes and chapters are unknown, but the saved defaults
                    // do not need them.
                    ? _download
                    : () async {
                        await c.send(
                            MusicCmd.ytPlay(videoId: id, formatId: ''));
                        if (context.mounted) Navigator.pop(context);
                      },
                child: Text(download
                    ? 'Download as ${_extOf(_presetKey).toUpperCase()} anyway'
                    : 'Play best audio anyway'),
              ),
              TextButton(
                style: musicQuietStyle(context),
                onPressed: () => setState(_load),
                child: const Text('Try again'),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class _Footer extends StatelessWidget {
  const _Footer({required this.children});

  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return DecoratedBox(
      decoration: BoxDecoration(
        color: t.nCard,
        border: Border(top: BorderSide(color: t.nHair)),
      ),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(20, 12, 20, 12),
        child: Row(children: children),
      ),
    );
  }
}

/// Two or three equal choices in a well: Play / Download, Audio / Video.
class _Segment<T> extends StatelessWidget {
  const _Segment({
    required this.items,
    required this.selected,
    required this.onPick,
  });

  final List<(T, IconData, String)> items;
  final T selected;
  final ValueChanged<T> onPick;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return DecoratedBox(
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(12),
      ),
      child: Padding(
        padding: const EdgeInsets.all(4),
        child: Row(
          children: [
            for (final (value, icon, label) in items)
              Expanded(
                child: Semantics(
                  selected: value == selected,
                  button: true,
                  child: InkWell(
                    borderRadius: BorderRadius.circular(9),
                    onTap: () => onPick(value),
                    child: Container(
                      padding: const EdgeInsets.symmetric(vertical: 9),
                      decoration: value == selected
                          ? BoxDecoration(
                              color: t.modalSolid,
                              borderRadius: BorderRadius.circular(9),
                              boxShadow: const [
                                BoxShadow(
                                    color: Color(0x22000000),
                                    blurRadius: 6,
                                    offset: Offset(0, 1)),
                              ],
                            )
                          : null,
                      child: Row(
                        mainAxisAlignment: MainAxisAlignment.center,
                        children: [
                          Icon(icon,
                              size: 16,
                              color: value == selected ? t.nInk : t.nInk2),
                          const SizedBox(width: 6),
                          Text(label,
                              style: TextStyle(
                                  fontSize: 13,
                                  fontWeight: FontWeight.w600,
                                  color:
                                      value == selected ? t.nInk : t.nInk2)),
                        ],
                      ),
                    ),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

class _PresetTile extends StatelessWidget {
  const _PresetTile({
    required this.preset,
    required this.selected,
    required this.onTap,
  });

  final YtPreset preset;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    const tabular = [FontFeature.tabularFigures()];
    return Semantics(
      selected: selected,
      button: true,
      child: InkWell(
        borderRadius: BorderRadius.circular(12),
        onTap: onTap,
        child: Container(
          padding: const EdgeInsets.fromLTRB(12, 10, 12, 10),
          decoration: BoxDecoration(
            color: selected ? _yt.withValues(alpha: 0.08) : t.nCard,
            borderRadius: BorderRadius.circular(12),
            border: Border.all(
                color: selected ? _yt : t.nHair, width: selected ? 1.5 : 1),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(preset.name,
                  style: TextStyle(
                      fontSize: 15, fontWeight: FontWeight.w700, color: t.nInk)),
              const SizedBox(height: 2),
              Text(preset.detail,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11, color: t.nInk3)),
              Text(
                  preset.bytes < 0
                      ? 'size unknown'
                      : '≈ ${_size(preset.bytes.toDouble())}',
                  style: TextStyle(
                      fontSize: 11, color: t.nInk2, fontFeatures: tabular)),
            ],
          ),
        ),
      ),
    );
  }
}

class _Toggle extends StatelessWidget {
  const _Toggle({
    required this.title,
    required this.hint,
    required this.value,
    required this.onChanged,
    this.enabled = true,
  });

  final String title;
  final String hint;
  final bool value;
  final bool enabled;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Opacity(
      opacity: enabled ? 1 : 0.45,
      child: MergeSemantics(
        child: InkWell(
          onTap: enabled ? () => onChanged(!value) : null,
          child: Padding(
            padding: const EdgeInsets.symmetric(vertical: 6),
            child: Row(
              children: [
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(title,
                          style: TextStyle(fontSize: 13, color: t.nInk)),
                      Text(hint,
                          style: TextStyle(fontSize: 11, color: t.nInk3)),
                    ],
                  ),
                ),
                Switch(
                  value: enabled && value,
                  activeTrackColor: _yt,
                  onChanged: enabled ? onChanged : null,
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _Row extends StatelessWidget {
  const _Row({
    required this.selected,
    required this.quality,
    required this.detail,
    required this.trailing,
    required this.onTap,
  });

  final bool selected;
  final String quality;
  final String detail;
  final String trailing;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    const tabular = [FontFeature.tabularFigures()];
    return Semantics(
      selected: selected,
      button: true,
      child: InkWell(
        onTap: onTap,
        child: Container(
          color: selected ? _yt.withValues(alpha: 0.10) : null,
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
          child: Row(
            children: [
              Icon(
                selected
                    ? Icons.radio_button_checked
                    : Icons.radio_button_unchecked,
                size: 16,
                color: selected ? _yt : t.nInk3,
              ),
              const SizedBox(width: 12),
              SizedBox(
                width: 72,
                child: Text(quality,
                    style: TextStyle(
                        fontSize: 14,
                        fontWeight: FontWeight.w700,
                        color: t.nInk,
                        fontFeatures: tabular)),
              ),
              Expanded(
                child: Text(detail,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11, color: t.nInk3)),
              ),
              const SizedBox(width: 8),
              Text(trailing,
                  style: TextStyle(
                      fontSize: 12, color: t.nInk2, fontFeatures: tabular)),
            ],
          ),
        ),
      ),
    );
  }
}
