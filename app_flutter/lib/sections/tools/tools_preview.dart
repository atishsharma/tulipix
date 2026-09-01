// The right-hand pane of an open tool: what the operation is about to do,
// drawn before anything is queued.
//
// Which preview a tool gets is `OpDef::preview` in Rust, arriving as
// `state.activePreview` — it used to be a lookup table here, which put the
// answer to "what does this tool show you" a bridge away from the tool. The
// content comes from `tools_preview(epoch)`, which reads the disk but never
// spawns anything.
//
// All eight kinds are answered. A rendered artifact arrives as a path into the
// preview cache, which is why this file reads files off disk and holds no bytes
// of its own.

import 'dart:io' show File;

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/tools.dart';
import 'tools_controller.dart';

({IconData icon, String title, String blurb}) _look(String kind) =>
    switch (kind) {
      'image' => (
          icon: Icons.image_outlined,
          title: 'Image preview',
          blurb: 'Before and after on one frame, with a split you can drag.',
        ),
      'video' => (
          icon: Icons.movie_outlined,
          title: 'Video preview',
          blurb: 'A two-second sample at these settings, and a timeline.',
        ),
      'wave' => (
          icon: Icons.graphic_eq,
          title: 'Waveform preview',
          blurb: 'The audio drawn as peaks, with what you are cutting marked.',
        ),
      'pages' => (
          icon: Icons.description_outlined,
          title: 'Page preview',
          blurb:
              'Every page as a thumbnail, kept ones lit and dropped ones struck through.',
        ),
      'cues' => (
          icon: Icons.subtitles_outlined,
          title: 'Cue preview',
          blurb: 'The subtitle lines with their timings, as they will land.',
        ),
      'dryrun' => (
          icon: Icons.checklist_outlined,
          title: 'Dry run',
          blurb:
              'Every file this will touch, and what it becomes — before it touches any of them.',
        ),
      'convert' => (
          icon: Icons.swap_horiz,
          title: 'Conversion preview',
          blurb: 'What goes in and what comes out, side by side.',
        ),
      _ => (
          icon: Icons.article_outlined,
          title: 'Report',
          blurb: 'What this run will report back.',
        ),
    };

/// A row's colour. The words are in the row's own note.
Color _rowTint(String kind, Tokens t) => switch (kind) {
      'add' => Tokens.ok,
      'remove' => Tokens.error,
      'change' => Tokens.warn,
      'warn' => Tokens.error,
      _ => t.nInk3,
    };

IconData _rowGlyph(String kind) => switch (kind) {
      'add' => Icons.add,
      'remove' => Icons.remove,
      'change' => Icons.east,
      'warn' => Icons.warning_amber_rounded,
      _ => Icons.remove,
    };

class PreviewPane extends StatelessWidget {
  const PreviewPane({super.key, required this.controller, required this.state});

  final ToolsController controller;
  final ToolsState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final result = controller.preview;
    final declared = state.activePreview;
    return ColoredBox(
      color: t.nCanvas,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _Head(
            declared: declared,
            result: result,
            busy: controller.previewBusy,
          ),
          Expanded(
            child: _Body(
              declared: declared,
              result: result,
              tint: activeTint(state),
            ),
          ),
          if (result?.estimate != null)
            _EstimateStrip(estimate: result!.estimate!),
        ],
      ),
    );
  }
}

class _Head extends StatelessWidget {
  const _Head({
    required this.declared,
    required this.result,
    required this.busy,
  });

  final String declared;
  final PreviewResult? result;
  final bool busy;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final title = (result?.title.isNotEmpty ?? false)
        ? result!.title
        : _look(declared).title;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          Icon(_look(declared).icon, size: 15, color: t.nInk2),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              title,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 12, fontWeight: FontWeight.w700, color: t.nInk),
            ),
          ),
          // A spinner that appears for 40 ms on every keystroke is noise; this
          // one only ever shows up when the disk is genuinely slow.
          if (busy) ...[
            SizedBox(
              width: 11,
              height: 11,
              child: CircularProgressIndicator(strokeWidth: 1.6, color: t.nInk3),
            ),
            const SizedBox(width: 10),
          ],
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
            decoration: BoxDecoration(
              color: t.nChip,
              borderRadius: BorderRadius.circular(6),
            ),
            child: Text(
              declared,
              style: TextStyle(
                  fontFamily: 'monospace', fontSize: 10.5, color: t.nInk3),
            ),
          ),
        ],
      ),
    );
  }
}

class _Body extends StatelessWidget {
  const _Body({
    required this.declared,
    required this.result,
    required this.tint,
  });

  final String declared;
  final PreviewResult? result;

  /// The open tool's accent, which only the pane above knows.
  final Color tint;

  @override
  Widget build(BuildContext context) {
    final r = result;
    if (r == null) return _Placeholder(declared: declared);
    switch (r.kind) {
      case 'dryrun':
        return _DryRun(result: r);
      case 'cues':
        return _Cues(result: r);
      case 'pages':
        return _Pages(result: r, tint: tint);
      case 'image':
        return _Rendered(result: r);
      case 'wave':
        return _Wave(result: r, tint: tint);
      case 'waiting':
        return _Waiting(declared: declared, note: r.note);
      default:
        return _Placeholder(declared: declared);
    }
  }
}

/// A preview that exists but has not been given what it needs yet. The note
/// names the missing field, so this is an instruction rather than an apology.
class _Waiting extends StatelessWidget {
  const _Waiting({required this.declared, required this.note});

  final String declared;
  final String note;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 320),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(_look(declared).icon, size: 34, color: t.nInk3),
            const SizedBox(height: 14),
            Text(
              note,
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 12.5, height: 1.6, color: t.nInk2),
            ),
          ],
        ),
      ),
    );
  }
}

/// The kinds that need a render or a probe, which is phase 4.
class _Placeholder extends StatelessWidget {
  const _Placeholder({required this.declared});

  final String declared;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final look = _look(declared);
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 340),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(look.icon, size: 40, color: t.nInk3),
            const SizedBox(height: 14),
            Text(look.title,
                style: TextStyle(
                    fontSize: 14, fontWeight: FontWeight.w700, color: t.nInk2)),
            const SizedBox(height: 8),
            Text(
              look.blurb,
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 12, height: 1.6, color: t.nInk3),
            ),
            const SizedBox(height: 18),
            Text(
              'Not drawn yet — press Run to queue it.',
              textAlign: TextAlign.center,
              style: TextStyle(
                  fontSize: 11, fontStyle: FontStyle.italic, color: t.nInk3),
            ),
          ],
        ),
      ),
    );
  }
}

class _DryRun extends StatelessWidget {
  const _DryRun({required this.result});

  final PreviewResult result;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final rows = result.rows;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (result.note.isNotEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 12, 14, 4),
            child: Text(
              result.note,
              style: TextStyle(fontSize: 11.5, height: 1.5, color: t.nInk2),
            ),
          ),
        Expanded(
          child: rows.isEmpty
              ? Center(
                  child: Text('Nothing to do.',
                      style: TextStyle(fontSize: 12.5, color: t.nInk3)),
                )
              : ListView.builder(
                  padding: const EdgeInsets.fromLTRB(8, 8, 8, 12),
                  itemCount: rows.length + (result.more > 0 ? 1 : 0),
                  itemBuilder: (context, i) {
                    if (i >= rows.length) {
                      return Padding(
                        padding: const EdgeInsets.fromLTRB(10, 10, 10, 0),
                        child: Text(
                          '…and ${result.more} more.',
                          style: TextStyle(
                              fontSize: 11.5,
                              fontStyle: FontStyle.italic,
                              color: t.nInk3),
                        ),
                      );
                    }
                    return _DryRunRow(row: rows[i]);
                  },
                ),
        ),
      ],
    );
  }
}

class _DryRunRow extends StatelessWidget {
  const _DryRunRow({required this.row});

  final PreviewRow row;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = _rowTint(row.kind, t);
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 3),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Padding(
            padding: const EdgeInsets.only(top: 2),
            child: Icon(_rowGlyph(row.kind), size: 13, color: tint),
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Expanded(
                      child: Text(
                        row.left,
                        style: TextStyle(
                          fontFamily: 'monospace',
                          fontSize: 11.5,
                          height: 1.5,
                          color: row.kind == 'warn' ? tint : t.nInk,
                        ),
                      ),
                    ),
                    if (row.right.isNotEmpty) ...[
                      const SizedBox(width: 10),
                      Text(
                        row.right,
                        textAlign: TextAlign.right,
                        style: TextStyle(
                            fontFamily: 'monospace',
                            fontSize: 11.5,
                            height: 1.5,
                            color: t.nInk2),
                      ),
                    ],
                  ],
                ),
                if (row.note.isNotEmpty)
                  Text(
                    row.note,
                    style: TextStyle(fontSize: 10.5, color: tint),
                  ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _Cues extends StatelessWidget {
  const _Cues({required this.result});

  final PreviewResult result;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (result.note.isNotEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 12, 14, 4),
            child: Text(result.note,
                style: TextStyle(fontSize: 11.5, color: t.nInk2)),
          ),
        Expanded(
          child: ListView.builder(
            padding: const EdgeInsets.fromLTRB(12, 8, 12, 12),
            itemCount: result.cues.length + (result.more > 0 ? 1 : 0),
            itemBuilder: (context, i) {
              if (i >= result.cues.length) {
                return Padding(
                  padding: const EdgeInsets.only(top: 10),
                  child: Text('…and ${result.more} more.',
                      style: TextStyle(
                          fontSize: 11.5,
                          fontStyle: FontStyle.italic,
                          color: t.nInk3)),
                );
              }
              final cue = result.cues[i];
              return Padding(
                padding: const EdgeInsets.only(bottom: 10),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      '${_clock(cue.startMs)} → ${_clock(cue.endMs)}',
                      style: TextStyle(
                          fontFamily: 'monospace',
                          fontSize: 10.5,
                          color: t.nInk3),
                    ),
                    const SizedBox(height: 2),
                    Text(cue.text,
                        style: TextStyle(
                            fontSize: 12.5, height: 1.45, color: t.nInk)),
                  ],
                ),
              );
            },
          ),
        ),
      ],
    );
  }
}

/// Every page of a document as a numbered chip: kept ones solid, dropped ones
/// struck through, turned ones carrying the angle.
///
/// No thumbnails. The question people get wrong is what `1-3,5,8-10` means, and
/// that is a numbered strip rather than a picture.
class _Pages extends StatelessWidget {
  const _Pages({required this.result, required this.tint});

  final PreviewResult result;
  final Color tint;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (result.note.isNotEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 12, 14, 6),
            child: Text(result.note,
                style: TextStyle(fontSize: 11.5, color: t.nInk2)),
          ),
        Expanded(
          child: SingleChildScrollView(
            padding: const EdgeInsets.fromLTRB(14, 4, 14, 14),
            child: Wrap(
              spacing: 6,
              runSpacing: 6,
              children: [
                for (final page in result.pages)
                  _PageChip(page: page, tint: tint),
                if (result.more > 0)
                  Padding(
                    padding: const EdgeInsets.only(left: 4, top: 6),
                    child: Text('…and ${result.more} more',
                        style: TextStyle(
                            fontSize: 11,
                            fontStyle: FontStyle.italic,
                            color: t.nInk3)),
                  ),
              ],
            ),
          ),
        ),
      ],
    );
  }
}

class _PageChip extends StatelessWidget {
  const _PageChip({required this.page, required this.tint});

  final PreviewPage page;
  final Color tint;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final kept = page.kept;
    return Container(
      width: 40,
      height: 52,
      decoration: BoxDecoration(
        color: kept ? tint.withValues(alpha: 0.12) : t.nChip,
        borderRadius: BorderRadius.circular(5),
        border: Border.all(
          color: kept ? tint.withValues(alpha: 0.55) : t.nHair,
        ),
      ),
      child: Stack(
        alignment: Alignment.center,
        children: [
          Text(
            '${page.page}',
            style: TextStyle(
              fontFamily: 'monospace',
              fontSize: 12,
              fontWeight: FontWeight.w700,
              color: kept ? t.nInk : t.nInk3,
              decoration: kept ? null : TextDecoration.lineThrough,
              decorationColor: Tokens.error,
              decorationThickness: 2,
            ),
          ),
          if (page.turned != 0)
            Positioned(
              bottom: 4,
              child: Text(
                '${page.turned}°',
                style: TextStyle(
                    fontSize: 8.5, fontWeight: FontWeight.w700, color: tint),
              ),
            ),
        ],
      ),
    );
  }
}

/// A rendered frame. With a `before` it is a split you can drag; without one
/// it is simply the result, which is right for a thumbnail or a contact sheet.
class _Rendered extends StatefulWidget {
  const _Rendered({required this.result});

  final PreviewResult result;

  @override
  State<_Rendered> createState() => _RenderedState();
}

class _RenderedState extends State<_Rendered> {
  double _split = 0.5;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final r = widget.result;
    // A path is content-keyed, so a new setting is a new file and Flutter's
    // image cache cannot hand back the last one.
    final after = Image.file(File(r.image), fit: BoxFit.contain,
        key: ValueKey(r.image), gaplessPlayback: true,
        errorBuilder: (context, error, stack) => const SizedBox.shrink());

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(
          child: Padding(
            padding: const EdgeInsets.all(14),
            child: r.before.isEmpty
                ? after
                : LayoutBuilder(
                    builder: (context, box) => GestureDetector(
                      behavior: HitTestBehavior.opaque,
                      onHorizontalDragUpdate: (d) => setState(() {
                        _split = (_split + d.delta.dx / box.maxWidth)
                            .clamp(0.0, 1.0);
                      }),
                      child: Stack(
                        fit: StackFit.expand,
                        children: [
                          after,
                          // The untouched frame, clipped to the left of the
                          // handle. Both sides are the same picture at the
                          // same size, so the seam lines up.
                          ClipRect(
                            clipper: _LeftOf(_split),
                            child: Image.file(File(r.before),
                                fit: BoxFit.contain,
                                key: ValueKey(r.before),
                                gaplessPlayback: true,
                                errorBuilder: (context, error, stack) =>
                                    const SizedBox.shrink()),
                          ),
                          Positioned(
                            left: box.maxWidth * _split - 1,
                            top: 0,
                            bottom: 0,
                            child: Container(width: 2, color: t.nInk),
                          ),
                          Positioned(
                            left: box.maxWidth * _split - 15,
                            top: box.maxHeight / 2 - 15,
                            child: Container(
                              width: 30,
                              height: 30,
                              decoration: BoxDecoration(
                                color: t.nInk,
                                shape: BoxShape.circle,
                              ),
                              child: Icon(Icons.drag_indicator,
                                  size: 15, color: t.nCanvas),
                            ),
                          ),
                          Positioned(
                            left: 6,
                            top: 6,
                            child: _Chip(text: 'Before', tint: t.nInk),
                          ),
                          Positioned(
                            right: 6,
                            top: 6,
                            child: _Chip(text: 'After', tint: t.nInk),
                          ),
                        ],
                      ),
                    ),
                  ),
          ),
        ),
        if (r.note.isNotEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 0, 14, 12),
            child: Text(r.note,
                textAlign: TextAlign.center,
                style: TextStyle(fontSize: 11, color: t.nInk3)),
          ),
      ],
    );
  }
}

class _LeftOf extends CustomClipper<Rect> {
  const _LeftOf(this.fraction);

  final double fraction;

  @override
  Rect getClip(Size size) => Rect.fromLTWH(0, 0, size.width * fraction, size.height);

  @override
  bool shouldReclip(_LeftOf old) => old.fraction != fraction;
}

class _Chip extends StatelessWidget {
  const _Chip({required this.text, required this.tint});

  final String text;
  final Color tint;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 2),
      decoration: BoxDecoration(
        color: tint.withValues(alpha: 0.75),
        borderRadius: BorderRadius.circular(5),
      ),
      child: Text(text,
          style: TextStyle(
              fontSize: 9.5,
              letterSpacing: 0.6,
              fontWeight: FontWeight.w700,
              color: t.nCanvas)),
    );
  }
}

class _Wave extends StatelessWidget {
  const _Wave({required this.result, required this.tint});

  final PreviewResult result;
  final Color tint;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (result.peaks.isEmpty) {
      return const _Waiting(
          declared: 'wave', note: 'Nothing audible in that file.');
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(
          child: Padding(
            padding: const EdgeInsets.fromLTRB(14, 20, 14, 8),
            child: CustomPaint(
              painter: _WavePainter(
                peaks: result.peaks,
                tint: tint,
                axis: t.nHair,
              ),
              child: const SizedBox.expand(),
            ),
          ),
        ),
        if (result.note.isNotEmpty)
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 0, 14, 12),
            child: Text(result.note,
                textAlign: TextAlign.center,
                style: TextStyle(fontSize: 11, color: t.nInk3)),
          ),
      ],
    );
  }
}

class _WavePainter extends CustomPainter {
  _WavePainter({required this.peaks, required this.tint, required this.axis});

  final List<double> peaks;
  final Color tint;
  final Color axis;

  @override
  void paint(Canvas canvas, Size size) {
    final mid = size.height / 2;
    canvas.drawLine(Offset(0, mid), Offset(size.width, mid),
        Paint()..color = axis..strokeWidth = 1);

    // One bar per pixel column, taking the loudest peak that falls in it, so
    // the shape does not change with the width of the pane.
    final columns = size.width.floor().clamp(1, peaks.length);
    final per = peaks.length / columns;
    final paint = Paint()
      ..color = tint
      ..strokeWidth = 1
      ..strokeCap = StrokeCap.round;
    for (var x = 0; x < columns; x++) {
      var peak = 0.0;
      for (var i = (x * per).floor(); i < ((x + 1) * per).ceil() && i < peaks.length; i++) {
        if (peaks[i] > peak) peak = peaks[i];
      }
      final half = (peak * mid).clamp(0.5, mid);
      final dx = x * size.width / columns;
      canvas.drawLine(Offset(dx, mid - half), Offset(dx, mid + half), paint);
    }
  }

  @override
  bool shouldRepaint(_WavePainter old) =>
      old.peaks != peaks || old.tint != tint;
}

/// The footer. It says "Estimated" because it is a ratio table and one file
/// size, and the job's own numbers replace it when it finishes.
class _EstimateStrip extends StatelessWidget {
  const _EstimateStrip({required this.estimate});

  final PreviewEstimate estimate;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final delta = estimate.src == 0
        ? 0
        : ((estimate.out - estimate.src) * 100 / estimate.src).round();
    final smaller = delta <= 0;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 9),
      decoration: BoxDecoration(
        color: t.nChip,
        border: Border(top: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          Text('Estimated',
              style: TextStyle(
                  fontSize: 9.5,
                  letterSpacing: 0.8,
                  fontWeight: FontWeight.w700,
                  color: t.nInk3)),
          const SizedBox(width: 12),
          Text(
            '${_bytes(estimate.src)} → ${_bytes(estimate.out)}',
            style: TextStyle(
                fontFamily: 'monospace', fontSize: 11.5, color: t.nInk),
          ),
          const Spacer(),
          if (estimate.secs > 1) ...[
            Text(
              _duration(estimate.secs),
              style: TextStyle(
                  fontFamily: 'monospace', fontSize: 11.5, color: t.nInk3),
            ),
            const SizedBox(width: 12),
          ],
          Text(
            '${smaller ? '−' : '+'}${delta.abs()}%',
            style: TextStyle(
              fontFamily: 'monospace',
              fontSize: 11.5,
              fontWeight: FontWeight.w700,
              color: smaller ? Tokens.ok : Tokens.warn,
            ),
          ),
        ],
      ),
    );
  }
}

String _bytes(int n) {
  const units = ['B', 'KB', 'MB', 'GB'];
  var v = n.toDouble();
  var u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return u == 0 ? '$n B' : '${v.toStringAsFixed(1)} ${units[u]}';
}

/// Rough is the point: an encode estimate to the second would be a lie with a
/// decimal place on it.
String _duration(double secs) {
  if (secs < 90) return '~${secs.round()} s';
  if (secs < 5400) return '~${(secs / 60).round()} min';
  return '~${(secs / 3600).toStringAsFixed(1)} h';
}

String _clock(int ms) {
  final total = ms ~/ 1000;
  final h = total ~/ 3600;
  final m = (total ~/ 60) % 60;
  final s = total % 60;
  final mm = m.toString().padLeft(2, '0');
  final ss = s.toString().padLeft(2, '0');
  return h > 0 ? '$h:$mm:$ss' : '$m:$ss';
}

/// The tint the open tool wears, taken from its own row where the snapshot
/// still has it and from the open tab otherwise.
Color activeTint(ToolsState state) {
  for (final op in state.ops) {
    if (op.kind == state.activeOp) return tintFor(op.category);
  }
  return tintFor(state.category);
}
