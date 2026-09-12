// Timing the words by hand — tap along and the line takes the playhead.
//
// The half of `np.p4.music.lyrics-sync` that lives on this side. The Rust
// module owns the two things worth getting exactly right: the constant-offset
// shift, and the `[mm:ss.xx]` other readers expect. Everything here is the
// interaction, because tapping along is a thing you do to a song that is
// playing and only this side has the playhead.
//
// Reached from the Lyrics panel, and only when the track has words with no
// times on them — that is the whole situation this solves. LRCLIB has the
// synced version of most things; this is for the rest.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';

/// `[mm:ss.xx]`, the same spelling `lyrics_sync::fmt_ts` writes.
String fmtStamp(int ms) {
  final cs = (ms ~/ 10) % 100;
  final s = (ms ~/ 1000) % 60;
  final m = ms ~/ 60000;
  return '${m.toString().padLeft(2, '0')}:'
      '${s.toString().padLeft(2, '0')}.'
      '${cs.toString().padLeft(2, '0')}';
}

/// Open the editor over whatever is playing.
Future<void> timeLyrics(BuildContext context, MusicController c) async {
  final st = c.state;
  final itemId = st?.now.itemId ?? 0;
  final plain = st?.lyricsPlain ?? '';
  if (itemId == 0 || plain.trim().isEmpty) return;
  await showDialog<void>(
    context: context,
    builder: (_) => _Timer(controller: c, itemId: itemId, plain: plain),
  );
}

class _Timer extends StatefulWidget {
  const _Timer({
    required this.controller,
    required this.itemId,
    required this.plain,
  });

  final MusicController controller;
  final int itemId;
  final String plain;

  @override
  State<_Timer> createState() => _TimerState();
}

class _Line {
  _Line(this.text);
  final String text;

  /// Null until stamped.
  int? ms;
}

class _TimerState extends State<_Timer> {
  late final List<_Line> _lines;
  final FocusNode _keys = FocusNode();

  /// The line the next tap lands on. Not "the first unstamped one": going back
  /// to fix a line you fumbled should leave you there, moving forward from it,
  /// which is how a second pass actually goes.
  int _cursor = 0;

  /// Applied to every stamped line on save. A tap is always a little late —
  /// you hear the line, then you press — so this starts negative rather than
  /// at zero, and pretending otherwise would make every file drift the same
  /// way.
  int _shiftMs = -150;

  bool _saving = false;

  @override
  void initState() {
    super.initState();
    _lines = widget.plain
        .split('\n')
        .map((l) => _Line(l.trim()))
        .toList(growable: false);
  }

  @override
  void dispose() {
    _keys.dispose();
    super.dispose();
  }

  int get _nowMs => (widget.controller.tickPos * 1000).round().clamp(0, 1 << 30);

  /// Blank lines are the gaps between verses. They stay in the file — the
  /// shape of the words is part of them — but nothing is sung there, so the
  /// cursor walks past rather than asking you to time silence.
  int _nextTimeable(int from) {
    var i = from;
    while (i < _lines.length && _lines[i].text.isEmpty) {
      i++;
    }
    return i;
  }

  void _stamp() {
    final i = _nextTimeable(_cursor);
    if (i >= _lines.length) return;
    setState(() {
      _lines[i].ms = _nowMs;
      _cursor = _nextTimeable(i + 1);
    });
  }

  void _back() {
    // Step back to the last stamped line and un-stamp it. The undo people
    // actually want here is "I pressed too early", which is one line, not the
    // whole pass.
    setState(() {
      for (var i = _lines.length - 1; i >= 0; i--) {
        if (_lines[i].ms != null) {
          _lines[i].ms = null;
          _cursor = i;
          return;
        }
      }
      _cursor = 0;
    });
  }

  Future<void> _save() async {
    final stamped = _lines.where((l) => l.ms != null).length;
    if (stamped == 0) {
      final ok = await confirm(
        context,
        title: 'Save with no times?',
        body: 'Nothing has been stamped yet. Saving now stores the words as '
            'plain lyrics, which is what they already are.',
        action: 'Save anyway',
        danger: false,
      );
      if (!ok) return;
    }
    setState(() => _saving = true);
    try {
      final n = await musicSaveLrc(
        itemId: widget.itemId,
        lines: _lines
            .map((l) => StampedLine(atMs: l.ms ?? -1, text: l.text))
            .toList(),
        shiftMs: _shiftMs,
      );
      if (!mounted) return;
      Navigator.of(context).pop();
      // Re-read: the panel is drawing the unsynced version right now.
      await widget.controller.refresh();
      debugPrint('lyrics: stored $n timed lines');
    } catch (e) {
      if (!mounted) return;
      setState(() => _saving = false);
      await confirm(
        context,
        title: 'Could not save',
        body: '$e',
        action: 'Close',
        danger: false,
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = widget.controller;
    final done = _lines.where((l) => l.ms != null).length;
    final total = _lines.where((l) => l.text.isNotEmpty).length;

    return Dialog(
      backgroundColor: t.panel.withValues(alpha: 1),
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(18),
        side: BorderSide(color: t.outline),
      ),
      child: CallbackShortcuts(
        bindings: {
          // Space is the tap. It is also play/pause everywhere else in the
          // section, which is exactly why this dialog takes focus: while you
          // are timing, the space bar means "now".
          const SingleActivator(LogicalKeyboardKey.space): _stamp,
          const SingleActivator(LogicalKeyboardKey.enter): _stamp,
          const SingleActivator(LogicalKeyboardKey.backspace): _back,
        },
        child: Focus(
          focusNode: _keys,
          autofocus: true,
          child: SizedBox(
            width: 620,
            height: 660,
            child: Padding(
              padding: const EdgeInsets.all(22),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  _head(t, c, done, total),
                  const SizedBox(height: 12),
                  Expanded(child: _list(t)),
                  const SizedBox(height: 12),
                  _foot(t),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _head(Tokens t, MusicController c, int done, int total) => Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  'Time these words',
                  style: TextStyle(
                    fontFamily: Tokens.fontFamily,
                    fontSize: 16,
                    fontWeight: FontWeight.w800,
                    color: t.nInk,
                  ),
                ),
                Text(
                  '$done of $total timed  ·  space or click stamps the next '
                  'line, backspace takes one back',
                  style: TextStyle(fontSize: 11, color: t.nInk3),
                ),
              ],
            ),
          ),
          // The playhead, live. The whole job is matching this number to a
          // line, so it is the biggest thing in the header.
          AnimatedBuilder(
            animation: c.live,
            builder: (_, __) => Text(
              fmtStamp(_nowMs),
              style: const TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: 22,
                fontWeight: FontWeight.w800,
                fontFeatures: [FontFeature.tabularFigures()],
                color: Tokens.secMusic,
              ),
            ),
          ),
          const SizedBox(width: 12),
          AnimatedBuilder(
            animation: c,
            builder: (_, __) => IconButton(
              tooltip: c.tickPlaying ? 'Pause' : 'Play',
              icon: Icon(c.tickPlaying ? Icons.pause : Icons.play_arrow),
              onPressed: () => c.send(const MusicCmd.playPause()),
            ),
          ),
          IconButton(
            tooltip: 'Back 5 seconds',
            icon: const Icon(Icons.replay_5),
            onPressed: () => c.nudge(-5),
          ),
        ],
      );

  Widget _list(Tokens t) {
    final next = _nextTimeable(_cursor);
    return ListView.builder(
      itemCount: _lines.length,
      itemBuilder: (context, i) {
        final l = _lines[i];
        final isNext = i == next;
        final blank = l.text.isEmpty;
        return InkWell(
          // Clicking a line stamps *it*, wherever the cursor was. Fixing one
          // line in the middle of a pass should not mean starting again.
          onTap: blank
              ? null
              : () => setState(() {
                    l.ms = _nowMs;
                    _cursor = _nextTimeable(i + 1);
                  }),
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 7),
            decoration: BoxDecoration(
              color: isNext ? Tokens.secMusic.withValues(alpha: 0.14) : null,
              border: Border(
                left: BorderSide(
                  width: 2,
                  color: isNext ? Tokens.secMusic : Colors.transparent,
                ),
              ),
            ),
            child: Row(
              children: [
                SizedBox(
                  width: 78,
                  child: Text(
                    l.ms == null ? '—' : fmtStamp(l.ms!),
                    style: TextStyle(
                      fontSize: 12,
                      fontFeatures: const [FontFeature.tabularFigures()],
                      color: l.ms == null ? t.nInk3 : Tokens.secMusic,
                      fontWeight:
                          l.ms == null ? FontWeight.w400 : FontWeight.w700,
                    ),
                  ),
                ),
                Expanded(
                  child: Text(
                    blank ? '' : l.text,
                    style: TextStyle(
                      fontSize: 13,
                      color: blank ? t.nInk3 : t.nInk,
                      fontStyle: blank ? FontStyle.italic : null,
                    ),
                  ),
                ),
                if (l.ms != null)
                  IconButton(
                    iconSize: 15,
                    visualDensity: VisualDensity.compact,
                    tooltip: 'Clear this time',
                    icon: const Icon(Icons.close),
                    onPressed: () => setState(() => l.ms = null),
                  ),
              ],
            ),
          ),
        );
      },
    );
  }

  Widget _foot(Tokens t) => Row(
        children: [
          Tooltip(
            message: 'Every stamp moves by this much when saved. A tap lands '
                'after the line starts, so a small negative number is usually '
                'right.',
            child: Text('Nudge all  ${_shiftMs}ms',
                style: TextStyle(fontSize: 12, color: t.nInk2)),
          ),
          const SizedBox(width: 10),
          _Step(label: '−50', onTap: () => setState(() => _shiftMs -= 50)),
          const SizedBox(width: 6),
          _Step(label: '+50', onTap: () => setState(() => _shiftMs += 50)),
          const Spacer(),
          TextButton(
            style: musicQuietStyle(context),
            onPressed: _saving ? null : () => Navigator.of(context).pop(),
            child: const Text('Cancel'),
          ),
          const SizedBox(width: 8),
          FilledButton(
            style: musicFilledStyle(),
            onPressed: _saving ? null : _save,
            child: Text(_saving ? 'Saving…' : 'Save to library'),
          ),
        ],
      );
}

class _Step extends StatelessWidget {
  const _Step({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(6),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
        decoration: BoxDecoration(
          color: t.nChip,
          borderRadius: BorderRadius.circular(6),
          border: Border.all(color: t.nHair),
        ),
        child: Text(label, style: TextStyle(fontSize: 11, color: t.nInk2)),
      ),
    );
  }
}
