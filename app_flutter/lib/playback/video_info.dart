// What is playing, in numbers: the media-info panel behind the ⓘ button.
//
// Everything here is read from mpv on open rather than watched. A codec does
// not change mid-file, and a panel that subscribes to eleven properties to
// redraw text that never moves is a panel that costs something to have open.
// The two figures that do drift — the bitrates — are averages mpv keeps, so a
// snapshot is what they are anyway.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import 'video_controls.dart';

/// One `label: value` line. Empty values are dropped by the section builder, so
/// a file with no video track simply has no video section.
class _Fact {
  const _Fact(this.label, this.value);
  final String label;
  final String value;
}

class _Section {
  const _Section(this.title, this.facts);
  final String title;
  final List<_Fact> facts;
}

Future<void> showMediaInfo(BuildContext context, VideoOps ops) {
  return showDialog<void>(
    context: context,
    builder: (context) => _MediaInfo(ops: ops),
  );
}

/// Reads the lot in one pass.
///
/// Sequential rather than a `Future.wait`: these go over the same libmpv
/// handle, and two dozen concurrent property reads on it buy nothing on a
/// panel that opens once.
Future<List<_Section>> _gather(VideoOps ops) async {
  final player = ops.player;
  final state = player.state;

  Future<String> p(String name) => ops.property(name);

  final container = await p('file-format');
  final size = _bytes(await p('file-size'));

  final vCodec = await p('video-codec');
  final vFormat = await p('video-format');
  final vBitrate = _bitrate(await p('video-bitrate'));
  final pixfmt = await p('video-params/pixelformat');
  final hwdec = await p('hwdec-current');
  // Frames lost since the file opened: by the decoder (too slow to decode)
  // and by the output (decoded, but not shown in time). The second is the
  // one a stutter shows up in.
  final decDrops = await p('decoder-frame-drop-count');
  final voDrops = await p('frame-drop-count');
  var fps = await p('container-fps');
  if (fps.isEmpty) fps = await p('estimated-vf-fps');

  final aCodec = await p('audio-codec');
  final aBitrate = _bitrate(await p('audio-bitrate'));
  final channels = await p('audio-params/hr-channels');
  final rate = await p('audio-params/samplerate');

  final width = state.width;
  final height = state.height;
  final dw = await p('video-params/dw');
  final dh = await p('video-params/dh');

  final subtitle = player.state.track.subtitle;
  final subs = ops.subtitleTracks;

  return [
    _Section('File', [
      _Fact('Title', ops.title),
      _Fact('Location', ops.source),
      _Fact('Container', container),
      _Fact('Size', size),
      _Fact('Duration', fmtDuration(state.duration)),
    ]),
    _Section('Video', [
      _Fact('Codec', vCodec.isEmpty ? vFormat : vCodec),
      _Fact(
        'Resolution',
        width == null || height == null ? '' : '$width × $height',
      ),
      // Only when the display size differs: anamorphic content is stored at one
      // size and meant to be shown at another, and that is the whole reason to
      // print it. For everything else it repeats the line above.
      _Fact('Display size', _display(width, height, dw, dh)),
      _Fact('Frame rate', fps.isEmpty ? '' : '${_round(fps)} fps'),
      _Fact('Bitrate', vBitrate),
      _Fact('Pixel format', pixfmt),
      // The one line here that is about this machine rather than the file.
      // "no" means libmpv is decoding on the CPU, which is the answer worth
      // having when a film stutters.
      _Fact('Hardware decoding', hwdec),
      _Fact(
        'Dropped frames',
        decDrops.isEmpty && voDrops.isEmpty
            ? ''
            : '${voDrops.isEmpty ? '0' : voDrops} shown late · '
                '${decDrops.isEmpty ? '0' : decDrops} decoded late',
      ),
    ]),
    _Section('Audio', [
      _Fact('Codec', aCodec),
      _Fact('Channels', channels),
      _Fact('Sample rate', rate.isEmpty ? '' : '${_round(rate)} Hz'),
      _Fact('Bitrate', aBitrate),
      _Fact('Tracks', '${ops.audioTracks.length}'),
    ]),
    _Section('Subtitles', [
      _Fact(
        'Current',
        subtitle.id == 'no'
            ? 'None'
            : (subtitle.title ?? subtitle.language ?? subtitle.id),
      ),
      _Fact('Tracks', '${subs.length}'),
    ]),
  ];
}

String _display(int? w, int? h, String dw, String dh) {
  final x = int.tryParse(dw);
  final y = int.tryParse(dh);
  if (x == null || y == null) return '';
  if (x == w && y == h) return '';
  return '$x × $y';
}

String _round(String value) {
  final n = double.tryParse(value);
  if (n == null) return value;
  return n == n.roundToDouble() ? '${n.round()}' : n.toStringAsFixed(3);
}

/// mpv reports sizes in bytes and bitrates in bits per second.
String _bytes(String value) {
  final n = int.tryParse(value);
  if (n == null || n <= 0) return '';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  var v = n.toDouble();
  var unit = 0;
  while (v >= 1024 && unit < units.length - 1) {
    v /= 1024;
    unit++;
  }
  return '${v.toStringAsFixed(unit == 0 ? 0 : 1)} ${units[unit]}';
}

String _bitrate(String value) {
  final n = double.tryParse(value);
  if (n == null || n <= 0) return '';
  if (n >= 1000000) return '${(n / 1000000).toStringAsFixed(2)} Mb/s';
  return '${(n / 1000).round()} kb/s';
}

class _MediaInfo extends StatefulWidget {
  const _MediaInfo({required this.ops});

  final VideoOps ops;

  @override
  State<_MediaInfo> createState() => _MediaInfoState();
}

class _MediaInfoState extends State<_MediaInfo> {
  // Held in the state, not built in `build`: a FutureBuilder handed a future
  // made inline re-reads every property on every rebuild, and this dialog
  // rebuilds when the snackbar below opens.
  late final Future<List<_Section>> _facts = _gather(widget.ops);

  void _copy(List<_Section> sections) {
    final out = StringBuffer();
    for (final section in sections) {
      final rows = section.facts.where((f) => f.value.trim().isNotEmpty);
      if (rows.isEmpty) continue;
      out.writeln(section.title);
      for (final f in rows) {
        out.writeln('  ${f.label}: ${f.value}');
      }
    }
    Clipboard.setData(ClipboardData(text: out.toString()));
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Media info'),
      content: SizedBox(
        width: 520,
        height: 460,
        child: FutureBuilder<List<_Section>>(
          future: _facts,
          builder: (context, snap) {
            final sections = snap.data;
            if (sections == null) {
              return const Center(child: CircularProgressIndicator());
            }
            return ListView(
              children: [
                for (final section in sections) ..._rows(context, section),
              ],
            );
          },
        ),
      ),
      actions: [
        FutureBuilder<List<_Section>>(
          future: _facts,
          builder: (context, snap) => TextButton(
            onPressed: snap.data == null ? null : () => _copy(snap.data!),
            child: const Text('Copy'),
          ),
        ),
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Close'),
        ),
      ],
    );
  }

  List<Widget> _rows(BuildContext context, _Section section) {
    final facts = section.facts.where((f) => f.value.trim().isNotEmpty);
    if (facts.isEmpty) return const [];
    final muted = Theme.of(context).textTheme.bodySmall?.color;
    return [
      Padding(
        padding: const EdgeInsets.fromLTRB(0, 14, 0, 6),
        child: Text(
          section.title.toUpperCase(),
          style: TextStyle(
            fontSize: 11,
            letterSpacing: 1.2,
            fontWeight: FontWeight.w700,
            color: muted,
          ),
        ),
      ),
      for (final fact in facts)
        Padding(
          padding: const EdgeInsets.symmetric(vertical: 3),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              SizedBox(
                width: 150,
                child: Text(
                  fact.label,
                  style: TextStyle(fontSize: 13, color: muted),
                ),
              ),
              // Selectable because the one field anybody wants out of a panel
              // like this is the path, and it is usually longer than the row.
              Expanded(
                child: SelectableText(
                  fact.value,
                  style: const TextStyle(fontSize: 13),
                ),
              ),
            ],
          ),
        ),
    ];
  }
}
