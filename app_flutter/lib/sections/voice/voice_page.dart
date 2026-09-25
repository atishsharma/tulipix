// The Voice section — docs/mockups/NewSections/voice-deck.html.
//
// Four tabs over one snapshot: Notes (the list and the open note, with its
// player, transcript, summary and tasks), Ask (every word ever said, searched),
// Tasks and Setup. Files dropped anywhere on the page come in as notes.

import 'package:desktop_drop/desktop_drop.dart';
import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/section_tabs.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/dialog.dart';
import '../../src/rust/api/voice.dart';
import '../kitchen/kitchen_page.dart' show Quiet, Strip, askLine, cardDeco, confirm;
import '../papers/papers_page.dart' show TabPill;
import 'voice_controller.dart';

const Color _rec = Color(0xFFEF4444);

class VoicePage extends StatefulWidget {
  const VoicePage({super.key, required this.visible});

  final bool visible;

  @override
  State<VoicePage> createState() => _VoicePageState();
}

class _VoicePageState extends State<VoicePage> {
  final VoiceController _c = VoiceController();
  final VoicePlayer _player = VoicePlayer();
  bool _over = false;

  @override
  void initState() {
    super.initState();
    _c.refresh();
  }

  @override
  void didUpdateWidget(VoicePage old) {
    super.didUpdateWidget(old);
    if (widget.visible && !old.visible) _c.refresh();
    if (!widget.visible && old.visible) _c.pause();
  }

  @override
  void dispose() {
    _c.dispose();
    _player.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        final st = _c.state;
        return DropTarget(
          onDragEntered: (_) => setState(() => _over = true),
          onDragExited: (_) => setState(() => _over = false),
          onDragDone: (d) {
            setState(() => _over = false);
            final paths = [for (final f in d.files) f.path];
            if (paths.isNotEmpty) _c.send(VoiceCmd.addFiles(paths: paths));
          },
          child: Stack(
            children: [
              ColoredBox(
                color: t.nCanvas,
                child: Column(
                  children: [
                    _Header(c: _c, st: st),
                    if (_c.busy || (st?.working ?? 0) > 0)
                      const LinearProgressIndicator(minHeight: 2, color: kVoice)
                    else
                      const SizedBox(height: 2),
                    if (st != null && st.recording) _Recording(c: _c, st: st),
                    if (_c.notice.isNotEmpty)
                      Strip(
                        icon: Icons.check_circle_outline,
                        tint: kVoice,
                        text: _c.notice,
                        onClose: _c.dismissNotice,
                      ),
                    if (_c.error != null && st != null)
                      Strip(
                        icon: Icons.error_outline,
                        tint: Tokens.error,
                        text: plainError(_c.error!),
                        onClose: _c.clearError,
                      ),
                    if (st != null && !st.setup.ready && st.tab != 'setup')
                      Strip(
                        icon: Icons.info_outline,
                        tint: Tokens.warn,
                        text: 'Notes are saved, but can\'t be transcribed yet: '
                            '${st.setup.why}',
                        onClose: () =>
                            _c.send(const VoiceCmd.setTab(tab: 'setup')),
                      ),
                    Expanded(
                      child: st == null
                          ? FirstLoad(error: _c.error, onRetry: _c.refresh)
                          : switch (st.tab) {
                              'ask' => _Ask(c: _c, st: st, player: _player),
                              'tasks' => _Tasks(c: _c, st: st),
                              'setup' => _Setup(c: _c, st: st),
                              _ => _Notes(c: _c, st: st, player: _player),
                            },
                    ),
                  ],
                ),
              ),
              if (_over)
                Positioned.fill(
                  child: IgnorePointer(
                    child: Container(
                      margin: const EdgeInsets.all(12),
                      decoration: BoxDecoration(
                        color: kVoice.withValues(alpha: 0.10),
                        borderRadius: BorderRadius.circular(18),
                        border: Border.all(color: kVoice, width: 2),
                      ),
                      alignment: Alignment.center,
                      child: const Text('Drop to transcribe',
                          style: TextStyle(
                              fontSize: 20,
                              fontWeight: FontWeight.w700,
                              color: kVoice)),
                    ),
                  ),
                ),
            ],
          ),
        );
      },
    );
  }
}

Future<void> _chooseFiles(VoiceController c) async {
  final paths = await dialogPickFiles(
    title: 'Transcribe recordings',
    initial: '',
    label: 'Audio and video',
    extensions: kVoiceExtensions,
  );
  if (paths.isNotEmpty) await c.send(VoiceCmd.addFiles(paths: paths));
}

// ------------------------------------------------------------------ header --

class _Header extends StatefulWidget {
  const _Header({required this.c, required this.st});

  final VoiceController c;
  final VoiceState? st;

  @override
  State<_Header> createState() => _HeaderState();
}

class _HeaderState extends State<_Header> {
  final TextEditingController _q = TextEditingController();

  @override
  void dispose() {
    _q.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = widget.c;
    final st = widget.st;
    final tab = st?.tab ?? 'notes';
    final r = context.skin.controlRadius ?? 10;
    final recording = st?.recording ?? false;
    return Container(
      height: 64,
      padding: const EdgeInsets.fromLTRB(22, 0, 20, 0),
      decoration: BoxDecoration(
        color: t.panel,
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          Container(
            width: 32,
            height: 32,
            decoration: BoxDecoration(
              color: kVoice.withValues(alpha: 0.17),
              borderRadius: BorderRadius.circular(10),
            ),
            child: const Icon(Icons.mic_none_outlined, size: 18, color: kVoice),
          ),
          const SizedBox(width: 10),
          Text('Voice',
              style: TextStyle(
                  fontSize: 19, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(width: 14),
          Expanded(
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: Container(
                padding: const EdgeInsets.all(3),
                decoration: BoxDecoration(
                  color: t.nChip,
                  borderRadius: BorderRadius.circular(r + 3),
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    for (final f in keepTabs('voice', voiceTabs, (f) => f.id,
                        active: (f) => tab == f.id))
                      TabPill(
                        label: f.label,
                        on: f.id == tab,
                        count: switch (f.id) {
                          'tasks' => st?.tasksOpen ?? 0,
                          _ => 0,
                        },
                        radius: r,
                        tint: kVoice,
                        onTap: () => c.send(VoiceCmd.setTab(tab: f.id)),
                      ),
                  ],
                ),
              ),
            ),
          ),
          const SizedBox(width: 12),
          SizedBox(
            width: 250,
            height: 38,
            child: TextField(
              controller: _q,
              style: TextStyle(fontSize: 13, color: t.nInk),
              textInputAction: TextInputAction.search,
              onChanged: (_) => setState(() {}),
              onSubmitted: (v) => c.send(VoiceCmd.search(text: v)),
              decoration: InputDecoration(
                isDense: true,
                hintText: 'Find where I talked about…',
                hintStyle: TextStyle(fontSize: 13, color: t.nInk3),
                prefixIcon: Icon(Icons.search, size: 17, color: t.nInk3),
                suffixIcon: _q.text.isEmpty
                    ? null
                    : IconButton(
                        iconSize: 15,
                        tooltip: 'Clear',
                        icon: const Icon(Icons.close),
                        onPressed: () {
                          _q.clear();
                          c.send(const VoiceCmd.search(text: ''));
                        },
                      ),
                filled: true,
                fillColor: t.nChip,
                contentPadding: const EdgeInsets.symmetric(vertical: 10),
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(r),
                  borderSide: BorderSide.none,
                ),
              ),
            ),
          ),
          const SizedBox(width: 10),
          FilledButton.icon(
            style: FilledButton.styleFrom(backgroundColor: _rec),
            icon: Icon(recording ? Icons.stop_rounded : Icons.mic, size: 18),
            label: Text(recording
                ? 'Stop ${clockOf(st?.recSecs ?? 0)}'
                : 'Record'),
            onPressed: () => c.send(recording
                ? const VoiceCmd.stop()
                : const VoiceCmd.record()),
          ),
        ],
      ),
    );
  }
}

/// The strip under the header while recording: the clock, the bookmarks, and
/// the two things you can do.
class _Recording extends StatelessWidget {
  const _Recording({required this.c, required this.st});

  final VoiceController c;
  final VoiceState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.fromLTRB(22, 10, 20, 10),
      color: _rec.withValues(alpha: 0.10),
      child: Row(
        children: [
          const _Pulse(),
          const SizedBox(width: 12),
          Text('Recording · ${clockOf(st.recSecs)}',
              style: TextStyle(
                  fontWeight: FontWeight.w700,
                  color: t.nInk,
                  fontFeatures: const [FontFeature.tabularFigures()])),
          if (st.recMarks > 0) ...[
            const SizedBox(width: 12),
            Text(plural(st.recMarks.toInt(), 'bookmark'),
                style: TextStyle(fontSize: 12.5, color: t.nInk2)),
          ],
          const Spacer(),
          OutlinedButton.icon(
            icon: const Icon(Icons.bookmark_add_outlined, size: 17),
            label: const Text('Bookmark this moment'),
            onPressed: () => c.send(const VoiceCmd.mark(), quiet: true),
          ),
          const SizedBox(width: 8),
          FilledButton.icon(
            style: FilledButton.styleFrom(backgroundColor: _rec),
            icon: const Icon(Icons.stop_rounded, size: 18),
            label: const Text('Stop and save'),
            onPressed: () => c.send(const VoiceCmd.stop()),
          ),
        ],
      ),
    );
  }
}

class _Pulse extends StatefulWidget {
  const _Pulse();

  @override
  State<_Pulse> createState() => _PulseState();
}

class _PulseState extends State<_Pulse> with SingleTickerProviderStateMixin {
  late final AnimationController _a = AnimationController(
      vsync: this, duration: const Duration(milliseconds: 1200))
    ..repeat(reverse: true);

  @override
  void dispose() {
    _a.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _a,
      builder: (context, _) => Container(
        width: 12,
        height: 12,
        decoration: BoxDecoration(
          color: _rec,
          shape: BoxShape.circle,
          boxShadow: [
            BoxShadow(
                color: _rec.withValues(alpha: 0.35 * (1 - _a.value)),
                spreadRadius: 3 + 5 * _a.value),
          ],
        ),
      ),
    );
  }
}

// ------------------------------------------------------------------- notes --

class _Notes extends StatelessWidget {
  const _Notes({required this.c, required this.st, required this.player});

  final VoiceController c;
  final VoiceState st;
  final VoicePlayer player;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final open = st.open;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Container(
          width: 340,
          decoration:
              BoxDecoration(border: Border(right: BorderSide(color: t.nHair))),
          child: _NoteList(c: c, st: st),
        ),
        Expanded(
          child: open == null
              ? Center(
                  child: Padding(
                    padding: const EdgeInsets.all(28),
                    child: ConstrainedBox(
                      constraints: const BoxConstraints(maxWidth: 460),
                      child: Quiet(
                        icon: Icons.mic_none_outlined,
                        title: st.notes.isEmpty
                            ? 'Say something worth keeping'
                            : 'Pick a note',
                        body: st.notes.isEmpty
                            ? 'Press Record, or drop audio and video files '
                                'here. whisper writes down every word on this '
                                'computer, and finds the tasks you said.'
                            : 'Its words, a summary and the tasks in it open '
                                'here.',
                      ),
                    ),
                  ),
                )
              : _NoteDetail(
                  key: ValueKey(open.id), c: c, n: open, player: player),
        ),
      ],
    );
  }
}

class _NoteList extends StatelessWidget {
  const _NoteList({required this.c, required this.st});

  final VoiceController c;
  final VoiceState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final items = <Widget>[];
    String? group;
    for (final n in st.notes) {
      if (n.day != group) {
        group = n.day;
        items.add(Padding(
          padding: const EdgeInsets.fromLTRB(6, 14, 6, 6),
          child: Text(n.day.toUpperCase(),
              style: TextStyle(
                  fontSize: 11,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 0.7,
                  color: t.nInk3)),
        ));
      }
      items.add(_NoteTile(
        n: n,
        on: st.open?.id == n.id,
        onTap: () => c.send(VoiceCmd.open(id: n.id), quiet: true),
      ));
    }
    return ListView(
      padding: const EdgeInsets.fromLTRB(12, 14, 12, 24),
      children: [
        Wrap(
          spacing: 6,
          runSpacing: 6,
          children: [
            for (final (id, label) in const [
              ('all', 'All'),
              ('starred', 'Starred'),
              ('tasks', 'With tasks'),
            ])
              ChoiceChip(
                label: Text(label),
                selected: st.filter == id,
                selectedColor: kVoice.withValues(alpha: 0.18),
                onSelected: (_) => c.send(VoiceCmd.setFilter(filter: id)),
              ),
          ],
        ),
        const SizedBox(height: 8),
        TextButton.icon(
          icon: const Icon(Icons.upload_file_outlined, size: 17),
          label: const Text('Transcribe audio or video files…'),
          onPressed: () => _chooseFiles(c),
        ),
        if (st.notes.isEmpty)
          Padding(
            padding: const EdgeInsets.all(12),
            child: Text(
                st.filter == 'all'
                    ? 'No notes yet.'
                    : 'Nothing here with this filter.',
                style: TextStyle(color: t.nInk3)),
          ),
        ...items,
      ],
    );
  }
}

class _NoteTile extends StatelessWidget {
  const _NoteTile({required this.n, required this.on, required this.onTap});

  final VoiceNoteRow n;
  final bool on;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final r = context.skin.controlRadius ?? 10;
    final waiting = n.state == 'new' || n.state == 'working';
    return Material(
      color: on ? kVoice.withValues(alpha: 0.14) : Colors.transparent,
      borderRadius: BorderRadius.circular(r),
      child: InkWell(
        borderRadius: BorderRadius.circular(r),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(12, 10, 12, 10),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  if (n.starred) ...[
                    const Icon(Icons.star_rounded,
                        size: 15, color: Color(0xFFF59E0B)),
                    const SizedBox(width: 4),
                  ],
                  Expanded(
                    child: Text(n.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 13.5,
                            fontWeight: FontWeight.w700,
                            color: t.nInk)),
                  ),
                  const SizedBox(width: 8),
                  Text(n.time,
                      style: TextStyle(fontSize: 11.5, color: t.nInk3)),
                ],
              ),
              const SizedBox(height: 3),
              Text(n.snippet,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 12,
                      height: 1.35,
                      fontStyle: waiting ? FontStyle.italic : null,
                      color: n.state == 'failed' ? Tokens.error : t.nInk2)),
              const SizedBox(height: 5),
              Row(
                children: [
                  Icon(
                      n.source == 'file'
                          ? Icons.audio_file_outlined
                          : Icons.schedule,
                      size: 13,
                      color: t.nInk3),
                  const SizedBox(width: 4),
                  Text(n.length,
                      style: TextStyle(fontSize: 11.5, color: t.nInk3)),
                  if (n.tasks > 0) ...[
                    const SizedBox(width: 8),
                    _Tag(plural(n.tasks.toInt(), 'task'), tint: kVoice),
                  ],
                  if (waiting) ...[
                    const SizedBox(width: 8),
                    const SizedBox(
                        width: 11,
                        height: 11,
                        child: CircularProgressIndicator(
                            strokeWidth: 1.6, color: kVoice)),
                  ],
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _Tag extends StatelessWidget {
  const _Tag(this.text, {required this.tint});

  final String text;
  final Color tint;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 2),
      decoration: BoxDecoration(
        color: tint.withValues(alpha: 0.15),
        borderRadius: BorderRadius.circular(6),
      ),
      child: Text(text,
          style: TextStyle(
              fontSize: 11, fontWeight: FontWeight.w700, color: tint)),
    );
  }
}

// ------------------------------------------------------------- one note ---

class _NoteDetail extends StatelessWidget {
  const _NoteDetail(
      {super.key, required this.c, required this.n, required this.player});

  final VoiceController c;
  final VoiceNote n;
  final VoicePlayer player;

  Future<void> _rename(BuildContext context) async {
    final title =
        await askLine(context, 'Rename', 'What this note is', initial: n.title);
    if (title != null && title.trim().isNotEmpty) {
      await c.send(VoiceCmd.rename(id: n.id, title: title));
    }
  }

  Future<void> _export(String format) async {
    final safe = n.title.replaceAll(RegExp(r'[\\/:*?"<>|]'), '').trim();
    final path = await dialogSaveFile(
      title: 'Export the transcript',
      fileName: '${safe.isEmpty ? 'Voice note' : safe}.$format',
      label: switch (format) {
        'srt' => 'Subtitles',
        'md' => 'Markdown',
        _ => 'Text',
      },
      extensions: [format],
    );
    if (path == null || path.isEmpty) return;
    try {
      await voiceExport(id: n.id, format: format, path: path);
      c.say('Saved $path');
    } catch (e) {
      c.say('Could not save it — ${plainError(e)}');
    }
  }

  Future<void> _delete(BuildContext context) async {
    final ok = await confirm(context, 'Delete this note?',
        '“${n.title}”, its transcript and its tasks go. Imported files stay where they are.');
    if (!ok) return;
    await player.forget(n.id);
    await c.send(VoiceCmd.delete(id: n.id));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final waiting = n.state == 'new' || n.state == 'working';
    final side = [
      _SummaryCard(c: c, n: n),
      const SizedBox(height: 14),
      _NoteTasks(c: c, n: n, player: player),
      const SizedBox(height: 14),
      _UseCard(c: c, n: n, onExport: _export),
    ];
    return LayoutBuilder(builder: (context, box) {
      final wide = box.maxWidth > 860;
      final transcript = waiting
          ? Padding(
              padding: const EdgeInsets.all(24),
              child: Row(children: [
                const SizedBox(
                    width: 18,
                    height: 18,
                    child: CircularProgressIndicator(
                        strokeWidth: 2, color: kVoice)),
                const SizedBox(width: 12),
                Text(
                    n.state == 'working'
                        ? 'whisper is writing it down…'
                        : 'Waiting its turn with whisper…',
                    style: TextStyle(color: t.nInk2)),
              ]),
            )
          : n.state == 'failed'
              ? Padding(
                  padding: const EdgeInsets.all(20),
                  child: Row(children: [
                    const Icon(Icons.error_outline, color: Tokens.error),
                    const SizedBox(width: 10),
                    Expanded(
                        child: Text('Not transcribed — ${n.error}',
                            style: TextStyle(color: t.nInk2))),
                    TextButton(
                      onPressed: () =>
                          c.send(VoiceCmd.retranscribe(id: n.id)),
                      child: const Text('Try again'),
                    ),
                  ]),
                )
              : _Transcript(n: n, player: player);
      return ListView(
        padding: const EdgeInsets.fromLTRB(26, 22, 26, 30),
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    InkWell(
                      onTap: () => _rename(context),
                      child: Text(n.title,
                          style: TextStyle(
                              fontSize: 26,
                              fontWeight: FontWeight.w800,
                              letterSpacing: -0.4,
                              color: t.nInk)),
                    ),
                    const SizedBox(height: 4),
                    Wrap(
                      spacing: 8,
                      runSpacing: 4,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        Text(n.when,
                            style: TextStyle(fontSize: 12.5, color: t.nInk2)),
                        Text('· ${clockOf(n.durationS)}',
                            style: TextStyle(fontSize: 12.5, color: t.nInk2)),
                        if (n.origin.isNotEmpty)
                          _Tag(n.origin, tint: t.nInk3),
                        if (n.inJournal.isNotEmpty)
                          const _Tag('In Journal', tint: Tokens.secJournal),
                      ],
                    ),
                  ],
                ),
              ),
              IconButton(
                tooltip: n.starred ? 'Unstar' : 'Star',
                icon: Icon(
                    n.starred ? Icons.star_rounded : Icons.star_border_rounded,
                    color: n.starred ? const Color(0xFFF59E0B) : null),
                onPressed: () => c.send(VoiceCmd.star(id: n.id), quiet: true),
              ),
              PopupMenuButton<String>(
                tooltip: 'More',
                onSelected: (v) => switch (v) {
                  'rename' => _rename(context),
                  'again' => c.send(VoiceCmd.retranscribe(id: n.id)),
                  'delete' => _delete(context),
                  _ => null,
                },
                itemBuilder: (_) => const [
                  PopupMenuItem(value: 'rename', child: Text('Rename')),
                  PopupMenuItem(
                      value: 'again', child: Text('Transcribe again')),
                  PopupMenuItem(value: 'delete', child: Text('Delete')),
                ],
              ),
            ],
          ),
          const SizedBox(height: 16),
          _PlayerCard(n: n, player: player),
          const SizedBox(height: 16),
          if (wide)
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(
                    child: Container(
                        decoration: cardDeco(context), child: transcript)),
                const SizedBox(width: 16),
                SizedBox(width: 300, child: Column(children: side)),
              ],
            )
          else ...[
            Container(decoration: cardDeco(context), child: transcript),
            const SizedBox(height: 14),
            ...side,
          ],
        ],
      );
    });
  }
}

class _PlayerCard extends StatelessWidget {
  const _PlayerCard({required this.n, required this.player});

  final VoiceNote n;
  final VoicePlayer player;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.fromLTRB(18, 16, 18, 12),
      decoration: cardDeco(context),
      child: AnimatedBuilder(
        animation: player,
        builder: (context, _) {
          final mine = player.current == n.id;
          final at = mine ? player.positionMs / 1000 : 0.0;
          final canPlay = n.path.isNotEmpty;
          return Column(
            children: [
              LayoutBuilder(builder: (context, box) {
                return GestureDetector(
                  onTapDown: canPlay
                      ? (d) => player.seek(
                          n,
                          (d.localPosition.dx /
                                  box.maxWidth *
                                  n.durationS *
                                  1000)
                              .round())
                      : null,
                  child: SizedBox(
                    height: 64,
                    width: double.infinity,
                    child: CustomPaint(
                      painter: _Wave(
                        peaks: n.peaks,
                        done: player.fraction(n),
                        marks: [
                          for (final m in n.marks)
                            n.durationS > 0 ? m.toInt() / (n.durationS * 1000) : 0.0
                        ],
                        on: kVoice,
                        off: t.nHair,
                        mark: const Color(0xFFF59E0B),
                      ),
                    ),
                  ),
                );
              }),
              const SizedBox(height: 10),
              Row(
                children: [
                  IconButton(
                    tooltip: 'Back 10 seconds',
                    icon: const Icon(Icons.replay_10),
                    onPressed: mine ? () => player.skip(-10000) : null,
                  ),
                  IconButton.filled(
                    style: IconButton.styleFrom(backgroundColor: kVoice),
                    iconSize: 28,
                    tooltip: canPlay ? 'Play' : 'Not ready to play yet',
                    icon: Icon(mine && player.playing
                        ? Icons.pause_rounded
                        : Icons.play_arrow_rounded),
                    onPressed: canPlay ? () => player.toggle(n) : null,
                  ),
                  IconButton(
                    tooltip: 'Forward 10 seconds',
                    icon: const Icon(Icons.forward_10),
                    onPressed: mine ? () => player.skip(10000) : null,
                  ),
                  const SizedBox(width: 8),
                  Text('${clockOf(at)} / ${clockOf(n.durationS)}',
                      style: TextStyle(
                          fontSize: 12.5,
                          color: t.nInk2,
                          fontFeatures: const [FontFeature.tabularFigures()])),
                  const Spacer(),
                  for (final r in const [1.0, 1.5, 2.0])
                    Padding(
                      padding: const EdgeInsets.only(left: 4),
                      child: ChoiceChip(
                        label: Text(r == 1.5 ? '1.5×' : '${r.toInt()}×'),
                        selected: player.rate == r,
                        selectedColor: kVoice.withValues(alpha: 0.18),
                        onSelected: (_) => player.setRate(r),
                      ),
                    ),
                ],
              ),
              if (n.marks.isNotEmpty)
                Align(
                  alignment: Alignment.centerLeft,
                  child: Wrap(
                    spacing: 6,
                    children: [
                      for (final m in n.marks)
                        ActionChip(
                          avatar: const Icon(Icons.bookmark,
                              size: 14, color: Color(0xFFF59E0B)),
                          label: Text(clockOf(m.toInt() / 1000)),
                          onPressed: () => player.seek(n, m.toInt()),
                        ),
                    ],
                  ),
                ),
            ],
          );
        },
      ),
    );
  }
}

class _Wave extends CustomPainter {
  _Wave({
    required this.peaks,
    required this.done,
    required this.marks,
    required this.on,
    required this.off,
    required this.mark,
  });

  final List<double> peaks;
  final double done;
  final List<double> marks;
  final Color on;
  final Color off;
  final Color mark;

  @override
  void paint(Canvas canvas, Size size) {
    final bars = peaks.isEmpty ? List.filled(80, 0.08) : peaks;
    final w = size.width / bars.length;
    for (var i = 0; i < bars.length; i++) {
      final h = bars[i].clamp(0.06, 1.0).toDouble() * size.height;
      final played = (i + 0.5) / bars.length <= done;
      canvas.drawRRect(
        RRect.fromRectAndRadius(
          Rect.fromLTWH(i * w + w * 0.15, (size.height - h) / 2, w * 0.7, h),
          const Radius.circular(2),
        ),
        Paint()..color = played ? on : off,
      );
    }
    final p = Paint()
      ..color = mark
      ..strokeWidth = 2;
    for (final m in marks) {
      final x = m.clamp(0.0, 1.0).toDouble() * size.width;
      canvas.drawLine(Offset(x, 0), Offset(x, size.height), p);
    }
  }

  @override
  bool shouldRepaint(_Wave old) =>
      old.done != done || old.peaks != peaks || old.off != off;
}

class _Transcript extends StatelessWidget {
  const _Transcript({required this.n, required this.player});

  final VoiceNote n;
  final VoicePlayer player;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: player,
      builder: (context, _) {
        final mine = player.current == n.id;
        final pos = player.positionMs;
        return Padding(
          padding: const EdgeInsets.fromLTRB(8, 10, 8, 10),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              for (final s in n.segments)
                InkWell(
                  borderRadius: BorderRadius.circular(8),
                  onTap: () => player.seek(n, s.startMs.toInt()),
                  child: Container(
                    padding: const EdgeInsets.fromLTRB(10, 7, 10, 7),
                    decoration: BoxDecoration(
                      color: mine && pos >= s.startMs && pos < s.endMs
                          ? kVoice.withValues(alpha: 0.14)
                          : null,
                      borderRadius: BorderRadius.circular(8),
                    ),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        SizedBox(
                          width: 52,
                          child: Padding(
                            padding: const EdgeInsets.only(top: 3),
                            child: Text(s.at,
                                style: TextStyle(
                                    fontSize: 12,
                                    color: t.nInk3,
                                    fontFeatures: const [
                                      FontFeature.tabularFigures()
                                    ])),
                          ),
                        ),
                        Expanded(
                          child: Text(s.text,
                              style: TextStyle(
                                  fontSize: 14.5,
                                  height: 1.55,
                                  color: mine && pos >= s.endMs
                                      ? t.nInk
                                      : t.nInk2)),
                        ),
                      ],
                    ),
                  ),
                ),
              Padding(
                padding: const EdgeInsets.fromLTRB(10, 8, 10, 0),
                child: Text(
                    'Click a line to hear it. Transcribed on this computer.',
                    style: TextStyle(fontSize: 11.5, color: t.nInk3)),
              ),
            ],
          ),
        );
      },
    );
  }
}

class _Card extends StatelessWidget {
  const _Card({required this.icon, required this.title, required this.child, this.trailing});

  final IconData icon;
  final String title;
  final Widget child;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(16, 14, 16, 16),
      decoration: cardDeco(context),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(children: [
            Icon(icon, size: 17, color: kVoice),
            const SizedBox(width: 8),
            Expanded(
              child: Text(title,
                  style: TextStyle(
                      fontSize: 14,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
            ),
            if (trailing != null) trailing!,
          ]),
          const SizedBox(height: 10),
          child,
        ],
      ),
    );
  }
}

class _SummaryCard extends StatelessWidget {
  const _SummaryCard({required this.c, required this.n});

  final VoiceController c;
  final VoiceNote n;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final server = c.state?.setup.summarizer ?? '';
    return _Card(
      icon: Icons.auto_awesome_outlined,
      title: 'Summary',
      trailing: n.summaryBy.isNotEmpty ? _Tag(n.summaryBy, tint: t.nInk3) : null,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
              n.summary.isEmpty
                  ? (n.state == 'done'
                      ? 'Too short to summarise from its words alone.'
                      : 'Once it is transcribed.')
                  : n.summary.join(' '),
              style: TextStyle(fontSize: 13, height: 1.5, color: t.nInk2)),
          if (server.isNotEmpty && n.state == 'done') ...[
            const SizedBox(height: 8),
            TextButton.icon(
              icon: const Icon(Icons.refresh, size: 16),
              label: Text(n.summaryBy.isEmpty
                  ? 'Summarise with your model'
                  : 'Summarise again'),
              onPressed: () => c.send(VoiceCmd.summarise(id: n.id)),
            ),
          ],
        ],
      ),
    );
  }
}

class _NoteTasks extends StatelessWidget {
  const _NoteTasks({required this.c, required this.n, required this.player});

  final VoiceController c;
  final VoiceNote n;
  final VoicePlayer player;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return _Card(
      icon: Icons.checklist_rtl,
      title: 'Tasks said',
      trailing: n.tasks.isEmpty
          ? null
          : _Tag('${n.tasks.where((x) => !x.done).length}', tint: kVoice),
      child: n.tasks.isEmpty
          ? Text(
              'None heard. “Remind me to…”, “I need to…” and “buy…” become tasks.',
              style: TextStyle(fontSize: 12.5, color: t.nInk3))
          : Column(
              children: [
                for (final x in n.tasks)
                  _TaskLine(
                    x: x,
                    showNote: false,
                    onToggle: () =>
                        c.send(VoiceCmd.toggleTask(id: x.id), quiet: true),
                    onJump: () => player.seek(n, x.atMs.toInt()),
                  ),
              ],
            ),
    );
  }
}

class _UseCard extends StatelessWidget {
  const _UseCard({required this.c, required this.n, required this.onExport});

  final VoiceController c;
  final VoiceNote n;
  final Future<void> Function(String format) onExport;

  @override
  Widget build(BuildContext context) {
    final done = n.state == 'done';
    return _Card(
      icon: Icons.ios_share,
      title: 'Use it',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          OutlinedButton.icon(
            icon: const Icon(Icons.edit_note, size: 18),
            label: Text(n.inJournal.isEmpty
                ? 'Make a Journal entry'
                : 'In Journal · ${n.inJournal}'),
            onPressed: !done
                ? null
                : n.inJournal.isEmpty
                    ? () => c.send(VoiceCmd.toJournal(id: n.id))
                    : () => ShellController.instance.go(Section.journal),
          ),
          const SizedBox(height: 8),
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [
              for (final (f, label) in const [
                ('txt', 'Text'),
                ('md', 'Markdown'),
                ('srt', 'Subtitles'),
              ])
                ActionChip(
                  avatar: const Icon(Icons.download_outlined, size: 15),
                  label: Text(label),
                  onPressed: done ? () => onExport(f) : null,
                ),
            ],
          ),
        ],
      ),
    );
  }
}

class _TaskLine extends StatelessWidget {
  const _TaskLine({
    required this.x,
    required this.showNote,
    required this.onToggle,
    this.onJump,
    this.onDelete,
  });

  final VoiceTask x;
  final bool showNote;
  final VoidCallback onToggle;
  final VoidCallback? onJump;
  final VoidCallback? onDelete;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final sub = [
      if (x.dueLabel.isNotEmpty) x.dueLabel,
      if (showNote) '“${x.noteTitle}” ${x.at}' else x.at,
    ].join(' · ');
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Checkbox(
            value: x.done,
            activeColor: kVoice,
            visualDensity: VisualDensity.compact,
            onChanged: (_) => onToggle(),
          ),
          const SizedBox(width: 4),
          Expanded(
            child: InkWell(
              onTap: onJump,
              child: Padding(
                padding: const EdgeInsets.only(top: 6),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(x.text,
                        style: TextStyle(
                            fontSize: 13.5,
                            fontWeight: FontWeight.w600,
                            color: x.done ? t.nInk3 : t.nInk,
                            decoration:
                                x.done ? TextDecoration.lineThrough : null)),
                    const SizedBox(height: 2),
                    Text(sub,
                        style: TextStyle(
                            fontSize: 11.5,
                            color: x.overdue ? Tokens.error : t.nInk3)),
                  ],
                ),
              ),
            ),
          ),
          if (onDelete != null)
            IconButton(
              tooltip: 'Remove',
              iconSize: 17,
              icon: Icon(Icons.close, color: t.nInk3),
              onPressed: onDelete,
            ),
        ],
      ),
    );
  }
}

// --------------------------------------------------------------------- ask --

Future<void> _openAt(
    VoiceController c, VoicePlayer player, int noteId, int ms) async {
  final st = await c.send(VoiceCmd.open(id: noteId));
  final n = st?.open;
  if (n != null && n.id == noteId) await player.seek(n, ms);
}

class _Ask extends StatelessWidget {
  const _Ask({required this.c, required this.st, required this.player});

  final VoiceController c;
  final VoiceState st;
  final VoicePlayer player;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (st.query.isEmpty) {
      return Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 480),
          child: const Quiet(
            icon: Icons.manage_search,
            title: 'Find where you said it',
            body: 'Type in the search box above. Every word of every note is '
                'searched, and a result plays from the moment you said it.',
          ),
        ),
      );
    }
    return ListView(
      padding: const EdgeInsets.fromLTRB(26, 22, 26, 30),
      children: [
        Text(
            st.hits.isEmpty
                ? 'Nothing said matches “${st.query}”.'
                : '${plural(st.hits.length, 'moment')} matching “${st.query}”',
            style: TextStyle(fontSize: 13, color: t.nInk2)),
        const SizedBox(height: 12),
        for (final h in st.hits)
          Padding(
            padding: const EdgeInsets.only(bottom: 10),
            child: Material(
              color: Colors.transparent,
              child: InkWell(
                borderRadius: BorderRadius.circular(14),
                onTap: () =>
                    _openAt(c, player, h.noteId.toInt(), h.startMs.toInt()),
                child: Container(
                  padding: const EdgeInsets.fromLTRB(16, 13, 16, 13),
                  decoration: cardDeco(context),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Row(children: [
                        Expanded(
                          child: Text(h.title,
                              style: TextStyle(
                                  fontWeight: FontWeight.w700,
                                  color: t.nInk)),
                        ),
                        Text(h.day,
                            style: TextStyle(fontSize: 12, color: t.nInk3)),
                      ]),
                      const SizedBox(height: 6),
                      Text.rich(
                        TextSpan(children: [
                          TextSpan(
                              text: '▶ ${h.at}  ',
                              style: const TextStyle(
                                  fontWeight: FontWeight.w700,
                                  color: kVoice)),
                          ..._marked(h.text, t.nInk),
                        ]),
                        style: TextStyle(
                            fontSize: 13.5, height: 1.5, color: t.nInk2),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
      ],
    );
  }
}

/// The snippet with what matched — between « and » — in bold.
List<TextSpan> _marked(String text, Color strong) {
  final out = <TextSpan>[];
  var rest = text;
  while (true) {
    final a = rest.indexOf('«');
    final b = a < 0 ? -1 : rest.indexOf('»', a);
    if (a < 0 || b < 0) {
      out.add(TextSpan(text: rest));
      return out;
    }
    out.add(TextSpan(text: rest.substring(0, a)));
    out.add(TextSpan(
        text: rest.substring(a + 1, b),
        style: TextStyle(fontWeight: FontWeight.w700, color: strong)));
    rest = rest.substring(b + 1);
  }
}

// ------------------------------------------------------------------- tasks --

class _Tasks extends StatelessWidget {
  const _Tasks({required this.c, required this.st});

  final VoiceController c;
  final VoiceState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (st.tasks.isEmpty) {
      return Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 480),
          child: const Quiet(
            icon: Icons.checklist_rtl,
            title: 'No tasks yet',
            body: 'Say “remind me to…”, “I need to…” or “buy…” in a note, and '
                'it lands here with its day, if you said one.',
          ),
        ),
      );
    }
    final dated = st.tasks.any((x) => !x.done && x.due.isNotEmpty);
    return ListView(
      padding: const EdgeInsets.fromLTRB(26, 22, 26, 30),
      children: [
        Row(children: [
          Expanded(
            child: Text(
                '${plural(st.tasksOpen.toInt(), 'task')} to do, found in your notes',
                style: TextStyle(fontSize: 13, color: t.nInk2)),
          ),
          OutlinedButton.icon(
            icon: const Icon(Icons.event_outlined, size: 17),
            label: const Text('Add dated ones to my calendar'),
            onPressed: dated
                ? () => c.send(const VoiceCmd.tasksToCalendar())
                : null,
          ),
        ]),
        const SizedBox(height: 12),
        Container(
          padding: const EdgeInsets.fromLTRB(10, 6, 10, 6),
          decoration: cardDeco(context),
          child: Column(
            children: [
              for (final x in st.tasks)
                _TaskLine(
                  x: x,
                  showNote: true,
                  onToggle: () =>
                      c.send(VoiceCmd.toggleTask(id: x.id), quiet: true),
                  onJump: () => c.send(VoiceCmd.open(id: x.noteId)),
                  onDelete: () =>
                      c.send(VoiceCmd.deleteTask(id: x.id), quiet: true),
                ),
            ],
          ),
        ),
      ],
    );
  }
}

// ------------------------------------------------------------------- setup --

class _Setup extends StatelessWidget {
  const _Setup({required this.c, required this.st});

  final VoiceController c;
  final VoiceState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = st.setup;
    TextStyle dim() => TextStyle(fontSize: 12.5, color: t.nInk2);
    Widget line(String label, Widget value) => Padding(
          padding: const EdgeInsets.symmetric(vertical: 6),
          child: Row(children: [
            Expanded(
                child: Text(label,
                    style: TextStyle(fontSize: 13, color: t.nInk))),
            Flexible(child: value),
          ]),
        );
    final mb = s.bytes / (1024 * 1024);
    return ListView(
      padding: const EdgeInsets.fromLTRB(26, 22, 26, 30),
      children: [
        _Card(
          icon: Icons.memory,
          title: 'Transcription',
          child: Column(children: [
            line(
              'whisper, on this computer',
              s.ready
                  ? _Tag('Ready · ${s.model}', tint: Tokens.ok)
                  : _Tag(s.why, tint: Tokens.error),
            ),
            if (!s.ready)
              Align(
                alignment: Alignment.centerLeft,
                child: TextButton(
                  onPressed: () =>
                      ShellController.instance.goTab(Section.settings, 'ai'),
                  child: const Text('Get a speech model in Settings › AI Features'),
                ),
              ),
            line(
              'Language',
              Wrap(spacing: 6, children: [
                for (final (id, label) in const [
                  ('', 'As Settings'),
                  ('auto', 'Detect'),
                  ('en', 'English'),
                  ('hi', 'हिन्दी'),
                ])
                  ChoiceChip(
                    label: Text(label),
                    selected: s.lang == id,
                    selectedColor: kVoice.withValues(alpha: 0.18),
                    onSelected: (_) => c.send(VoiceCmd.setLang(lang: id)),
                  ),
              ]),
            ),
            Align(
              alignment: Alignment.centerLeft,
              child: Text(
                  'Each note is transcribed after it is saved, one at a time, '
                  'so recording never waits for whisper.',
                  style: dim()),
            ),
          ]),
        ),
        const SizedBox(height: 14),
        _Card(
          icon: Icons.auto_awesome_outlined,
          title: 'Summaries',
          child: Column(children: [
            line(
              'Your model server, set up in Feeds',
              s.summarizer.isEmpty
                  ? _Tag('None · summaries are picked from the words',
                      tint: t.nInk3)
                  : _Tag(
                      '${s.summarizerModel.isEmpty ? 'model' : s.summarizerModel} · ${s.summarizer}',
                      tint: Tokens.ok),
            ),
            if (s.summarizer.isEmpty)
              Align(
                alignment: Alignment.centerLeft,
                child: TextButton(
                  onPressed: () =>
                      ShellController.instance.go(Section.feeds),
                  child: const Text('Set one up in Feeds'),
                ),
              ),
          ]),
        ),
        const SizedBox(height: 14),
        _Card(
          icon: Icons.folder_outlined,
          title: 'Storage',
          child: Column(children: [
            line('Notes', Text('${s.notes}', style: dim())),
            line(
                'On disk',
                Text(mb >= 1024
                    ? '${(mb / 1024).toStringAsFixed(1)} GB'
                    : '${mb.toStringAsFixed(mb < 10 ? 1 : 0)} MB',
                    style: dim())),
            line('Folder', SelectableText(s.folder, style: dim())),
            Align(
              alignment: Alignment.centerLeft,
              child: Text(
                  'Kept as 16 kHz WAV, about 115 MB an hour. Everything stays '
                  'on this computer.',
                  style: dim()),
            ),
          ]),
        ),
      ],
    );
  }
}
