// The Journal section — docs/NewSections/journal-deck.html.
//
// Four tabs over one snapshot: Today (the day's entries and voice notes, the
// day as the other sections saw it, and a rail with the map, On this day,
// tonight's question and the streak), Calendar, On this day and Insights —
// the last three in journal_views.dart. Every surface is drawn from the
// tokens and the skin, so the four design languages come for free; the one
// liberty is the serif for what you wrote, which is the Books reader's.

import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/section_tabs.dart';
import '../../src/rust/api/dialog.dart';
import '../../src/rust/api/journal.dart';
import '../home/home_shared.dart' show LazyCover;
import 'journal_controller.dart';
import 'journal_editor.dart';
import 'journal_views.dart';

/// The writing face: the Books reader's.
const String kSerif = 'serif';

class JournalPage extends StatefulWidget {
  const JournalPage({super.key, required this.visible});

  /// On screen. Coming back asks again: the other sections may have added to
  /// the day, and the day itself may have changed.
  final bool visible;

  @override
  State<JournalPage> createState() => _JournalPageState();
}

class _JournalPageState extends State<JournalPage> {
  final JournalController _c = JournalController();
  final VoicePlayer _player = VoicePlayer();

  @override
  void initState() {
    super.initState();
    _c.refresh();
  }

  @override
  void didUpdateWidget(JournalPage old) {
    super.didUpdateWidget(old);
    if (widget.visible && !old.visible) _c.refresh();
  }

  @override
  void dispose() {
    _player.dispose();
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        final st = _c.state;
        return ColoredBox(
          color: t.nCanvas,
          child: Column(
            children: [
              _Header(c: _c, st: st),
              if (_c.busy || _c.transcribing != 0)
                const LinearProgressIndicator(minHeight: 2, color: kJournal)
              else
                const SizedBox(height: 2),
              if (_c.notice.isNotEmpty)
                Strip(
                  icon: Icons.check_circle_outline,
                  tint: kJournal,
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
              Expanded(
                child: st == null
                    ? FirstLoad(error: _c.error, onRetry: _c.refresh)
                    : st.query.isNotEmpty
                        ? SearchView(c: _c, st: st)
                        : switch (st.tab) {
                            'calendar' => CalendarView(c: _c, st: st),
                            'otd' => OtdView(c: _c, st: st),
                            'insights' => InsightsView(c: _c, st: st),
                            _ => _TodayView(c: _c, st: st, player: _player),
                          },
              ),
            ],
          ),
        );
      },
    );
  }
}

// ------------------------------------------------------------------ header --

class _Header extends StatefulWidget {
  const _Header({required this.c, required this.st});

  final JournalController c;
  final JournalState? st;

  @override
  State<_Header> createState() => _HeaderState();
}

class _HeaderState extends State<_Header> {
  final TextEditingController _q = TextEditingController();

  @override
  void didUpdateWidget(_Header old) {
    super.didUpdateWidget(old);
    // Cleared by picking a tab or a result: the field follows.
    final q = widget.st?.query ?? '';
    if (q.isEmpty && _q.text.isNotEmpty && old.st?.query != q) _q.clear();
  }

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
    final tab = st?.tab ?? 'today';
    final streak = st?.streak ?? 0;
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
              color: kJournal.withValues(alpha: 0.17),
              borderRadius: BorderRadius.circular(10),
            ),
            child: const Icon(Icons.edit_note, size: 19, color: kJournal),
          ),
          const SizedBox(width: 10),
          Text('Journal',
              style: TextStyle(
                  fontSize: 19, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(width: 14),
          Expanded(
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: _Tabs(
                tabs: keepTabs('journal', journalTabs, (f) => f.id,
                    active: (f) => tab == f.id),
                active: st != null && st.query.isNotEmpty ? '' : tab,
                otd: st?.otd.length ?? 0,
                onTap: (id) => c.send(JournalCmd.setTab(tab: id)),
              ),
            ),
          ),
          const SizedBox(width: 12),
          SizedBox(
            width: 240,
            height: 38,
            child: TextField(
              controller: _q,
              style: TextStyle(fontSize: 13, color: t.nInk),
              textInputAction: TextInputAction.search,
              onChanged: (_) => setState(() {}),
              onSubmitted: (v) => c.send(JournalCmd.search(text: v)),
              decoration: InputDecoration(
                isDense: true,
                hintText: 'Search every entry',
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
                          c.send(const JournalCmd.search(text: ''));
                        },
                      ),
                filled: true,
                fillColor: t.nChip,
                contentPadding: const EdgeInsets.symmetric(vertical: 10),
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(
                      context.skin.controlRadius ?? 10),
                  borderSide: BorderSide.none,
                ),
              ),
            ),
          ),
          const SizedBox(width: 10),
          if (streak > 0) ...[
            Tooltip(
              message: '${plural(streak, 'day')} in a row',
              child: Container(
                height: 30,
                padding: const EdgeInsets.symmetric(horizontal: 10),
                decoration: BoxDecoration(
                  color: kJournal.withValues(alpha: 0.10),
                  borderRadius: BorderRadius.circular(99),
                  border: Border.all(color: kJournal.withValues(alpha: 0.35)),
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const Icon(Icons.local_fire_department_outlined,
                        size: 15, color: kJournal2),
                    const SizedBox(width: 4),
                    Text(plural(streak, 'day'),
                        style: const TextStyle(
                            fontSize: 12,
                            fontWeight: FontWeight.w700,
                            color: kJournal2)),
                  ],
                ),
              ),
            ),
            const SizedBox(width: 10),
          ],
          IconButton(
            tooltip: 'Export as Markdown',
            onPressed: st == null ? null : () => _export(c),
            icon: Icon(Icons.ios_share, size: 19, color: t.nInk2),
          ),
          const SizedBox(width: 4),
          FilledButton.icon(
            style: FilledButton.styleFrom(backgroundColor: kJournal2),
            onPressed: st == null
                ? null
                : () async {
                    if (st.tab != 'today' || st.query.isNotEmpty) {
                      await c.send(const JournalCmd.setTab(tab: 'today'));
                    }
                    await c.send(const JournalCmd.newEntry());
                  },
            icon: const Icon(Icons.add, size: 17),
            label: const Text('New entry'),
          ),
        ],
      ),
    );
  }
}

/// Every entry as Markdown files in a folder, one a day, with kept photos.
Future<void> _export(JournalController c) async {
  final to = await dialogPickFolder(title: 'Export the journal to', initial: '');
  if (to == null || to.isEmpty) return;
  try {
    final n = await journalExport(folder: to, photos: true);
    c.say(n == 0
        ? 'Nothing written yet to export'
        : 'Exported ${plural(n.toInt(), 'day')} as Markdown');
  } catch (e) {
    c.say(plainError(e));
  }
}

class _Tabs extends StatelessWidget {
  const _Tabs({
    required this.tabs,
    required this.active,
    required this.otd,
    required this.onTap,
  });

  final List<JournalTab> tabs;
  final String active;
  final int otd;
  final ValueChanged<String> onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final r = context.skin.controlRadius ?? 10;
    return Container(
      padding: const EdgeInsets.all(3),
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(r + 3),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (final f in tabs)
            _TabButton(
              label: f.label,
              on: f.id == active,
              count: f.id == 'otd' ? otd : 0,
              radius: r,
              onTap: () => onTap(f.id),
            ),
        ],
      ),
    );
  }
}

class _TabButton extends StatelessWidget {
  const _TabButton({
    required this.label,
    required this.on,
    required this.count,
    required this.radius,
    required this.onTap,
  });

  final String label;
  final bool on;
  final int count;
  final double radius;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: on ? kJournal2 : Colors.transparent,
      borderRadius: BorderRadius.circular(radius),
      child: InkWell(
        borderRadius: BorderRadius.circular(radius),
        onTap: onTap,
        child: Container(
          height: 32,
          padding: const EdgeInsets.symmetric(horizontal: 14),
          alignment: Alignment.center,
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(label,
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w600,
                      color: on ? Colors.white : t.nInk2)),
              if (count > 0) ...[
                const SizedBox(width: 7),
                Container(
                  constraints: const BoxConstraints(minWidth: 18),
                  height: 18,
                  padding: const EdgeInsets.symmetric(horizontal: 5),
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: on ? Colors.white : kJournal2,
                    borderRadius: BorderRadius.circular(99),
                  ),
                  child: Text('$count',
                      style: TextStyle(
                          fontSize: 11,
                          fontWeight: FontWeight.w700,
                          color: on ? kJournal2 : Colors.white)),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

// ------------------------------------------------------------------- today --

class _TodayView extends StatelessWidget {
  const _TodayView({required this.c, required this.st, required this.player});

  final JournalController c;
  final JournalState st;
  final VoicePlayer player;

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(builder: (context, box) {
      final wide = box.maxWidth >= 1080;
      final main = <Widget>[
        _DayHead(c: c, st: st),
        const SizedBox(height: 16),
        if (st.entries.isEmpty) _BlankEntry(c: c),
        for (final e in st.entries) ...[
          _EntryCard(
            key: ValueKey(e.id),
            c: c,
            st: st,
            e: e,
            focus: st.focus == e.id,
          ),
          for (final v in e.voice)
            Padding(
              padding: const EdgeInsets.only(top: 10),
              child: _VoiceCard(c: c, v: v, player: player),
            ),
          const SizedBox(height: 14),
        ],
        const SizedBox(height: 10),
        _Gathered(c: c, st: st),
      ];
      final rail = _Rail(c: c, st: st);
      if (!wide) {
        return ListView(
          padding: const EdgeInsets.fromLTRB(24, 22, 24, 40),
          children: [...main, const SizedBox(height: 18), rail],
        );
      }
      return Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(
            child: ListView(
              padding: const EdgeInsets.fromLTRB(26, 22, 18, 40),
              children: main,
            ),
          ),
          SizedBox(
            width: 320,
            child: ListView(
              padding: const EdgeInsets.fromLTRB(4, 22, 24, 40),
              children: [rail],
            ),
          ),
        ],
      );
    });
  }
}

class _DayHead extends StatelessWidget {
  const _DayHead({required this.c, required this.st});

  final JournalController c;
  final JournalState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.end,
      children: [
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(st.dayTitle,
                  style: TextStyle(
                      fontSize: 26,
                      fontWeight: FontWeight.w800,
                      height: 1.15,
                      color: t.nInk)),
              const SizedBox(height: 5),
              Row(
                children: [
                  Icon(
                      st.places.names.isEmpty
                          ? Icons.today_outlined
                          : Icons.place_outlined,
                      size: 14,
                      color: t.nInk3),
                  const SizedBox(width: 5),
                  Flexible(
                    child: Text(
                        [
                          if (st.weather.isNotEmpty) st.weather,
                          st.daySub,
                        ].join(' · '),
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 12.5, color: t.nInk3)),
                  ),
                  IconButton(
                    tooltip: st.weatherOn
                        ? 'Hide the weather'
                        : 'Show each day’s weather — asks open-meteo.com with the date and a rough location',
                    iconSize: 15,
                    visualDensity: VisualDensity.compact,
                    onPressed: () =>
                        c.send(JournalCmd.setWeather(on_: !st.weatherOn)),
                    icon: Icon(
                        st.weatherOn
                            ? Icons.wb_sunny
                            : Icons.wb_sunny_outlined,
                        color: st.weatherOn ? kJournal : t.nInk3),
                  ),
                ],
              ),
            ],
          ),
        ),
        if (!st.isToday)
          TextButton(
            onPressed: () => c.send(JournalCmd.go(day: _todayIso())),
            child: const Text('Today'),
          ),
        _RoundBtn(
          icon: Icons.chevron_left,
          tooltip: 'The day before',
          onTap: () => c.send(const JournalCmd.step(delta: -1)),
        ),
        const SizedBox(width: 6),
        _RoundBtn(
          icon: Icons.chevron_right,
          tooltip: 'The day after',
          onTap: st.isToday ? null : () => c.send(const JournalCmd.step(delta: 1)),
        ),
      ],
    );
  }
}

String _todayIso() {
  final n = DateTime.now();
  return '${n.year}-${'${n.month}'.padLeft(2, '0')}-${'${n.day}'.padLeft(2, '0')}';
}

class _RoundBtn extends StatelessWidget {
  const _RoundBtn({required this.icon, required this.tooltip, this.onTap});

  final IconData icon;
  final String tooltip;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Tooltip(
      message: tooltip,
      child: Material(
        color: t.nCard,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(10),
          side: BorderSide(color: t.nHair),
        ),
        child: InkWell(
          borderRadius: BorderRadius.circular(10),
          onTap: onTap,
          child: SizedBox(
            width: 34,
            height: 34,
            child: Icon(icon,
                size: 19, color: onTap == null ? t.nInk3.withValues(alpha: 0.4) : t.nInk2),
          ),
        ),
      ),
    );
  }
}

/// The day before anything is written: the moods and a place to start.
class _BlankEntry extends StatelessWidget {
  const _BlankEntry({required this.c});

  final JournalController c;

  Future<void> _start({int mood = 0}) async {
    final st = await c.send(const JournalCmd.newEntry());
    if (st == null || mood == 0 || st.focus == 0) return;
    await c.send(JournalCmd.setMood(id: st.focus, mood: mood));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      decoration: cardDeco(context),
      padding: const EdgeInsets.fromLTRB(18, 14, 18, 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          _MoodRow(mood: 0, onPick: (m) => _start(mood: m)),
          const SizedBox(height: 10),
          InkWell(
            borderRadius: BorderRadius.circular(8),
            onTap: _start,
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 18),
              child: Row(
                children: [
                  Expanded(
                    child: Text('Write about the day…',
                        style: TextStyle(
                            fontFamily: kSerif,
                            fontSize: 17,
                            color: t.nInk3)),
                  ),
                  Icon(Icons.edit_outlined, size: 18, color: t.nInk3),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _MoodRow extends StatelessWidget {
  const _MoodRow({required this.mood, required this.onPick});

  final int mood;
  final ValueChanged<int> onPick;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Text('How was today?',
            style: TextStyle(fontSize: 12.5, color: t.nInk3)),
        const SizedBox(width: 10),
        for (var i = 1; i <= 5; i++)
          Tooltip(
            message: moodLabels[i - 1],
            child: InkResponse(
              radius: 18,
              // Pressing the chosen face again clears it.
              onTap: () => onPick(mood == i ? 0 : i),
              child: Container(
                width: 30,
                height: 30,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: mood == i
                      ? moodColor(i)!.withValues(alpha: 0.16)
                      : Colors.transparent,
                ),
                child: Icon(moodIcons[i - 1],
                    size: 20, color: mood == i ? moodColor(i) : t.nInk3),
              ),
            ),
          ),
      ],
    );
  }
}

// ------------------------------------------------------------------- entry --

class _EntryCard extends StatefulWidget {
  const _EntryCard({
    super.key,
    required this.c,
    required this.st,
    required this.e,
    required this.focus,
  });

  final JournalController c;
  final JournalState st;
  final EntryView e;

  /// Just made: put the cursor in it.
  final bool focus;

  @override
  State<_EntryCard> createState() => _EntryCardState();
}

class _EntryCardState extends State<_EntryCard> {
  late final MarkdownController _text = MarkdownController(text: widget.e.body);
  final FocusNode _focus = FocusNode();
  Timer? _save;
  bool _dirty = false;

  /// Seconds recorded, while this entry is the one recording.
  Timer? _tick;
  int _secs = 0;

  bool get _recording => widget.st.recording == widget.e.id;

  @override
  void initState() {
    super.initState();
    if (widget.focus) _grab();
    if (_recording) _startTick();
    // The formatting bar shows while you are writing.
    _focus.addListener(() {
      if (mounted) setState(() {});
    });
  }

  @override
  void didUpdateWidget(_EntryCard old) {
    super.didUpdateWidget(old);
    if (widget.focus && !old.focus) _grab();
    // Written elsewhere (tonight's question into a blank entry): take it,
    // unless there are keystrokes here the bridge has not seen yet.
    if (!_dirty && widget.e.body != _text.text && widget.e.body != old.e.body) {
      _text.text = widget.e.body;
    }
    final was = old.st.recording == old.e.id;
    if (_recording && !was) _startTick();
    if (!_recording && was) _stopTick();
  }

  void _grab() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      _focus.requestFocus();
      _text.selection = TextSelection.collapsed(offset: _text.text.length);
    });
  }

  void _startTick() {
    _secs = 0;
    _tick?.cancel();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) {
      if (mounted) setState(() => _secs++);
    });
  }

  void _stopTick() {
    _tick?.cancel();
    _tick = null;
  }

  void _changed(String _) {
    _dirty = true;
    _save?.cancel();
    _save = Timer(const Duration(milliseconds: 700), _flush);
    setState(() {});
  }

  Future<void> _flush() async {
    if (!_dirty) return;
    _dirty = false;
    await widget.c.send(
        JournalCmd.saveBody(id: widget.e.id, body: _text.text),
        quiet: true);
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    _save?.cancel();
    _stopTick();
    // Straight to the bridge: the controller may be going away with us, and
    // the words must not.
    if (_dirty) {
      journalDispatch(
              cmd: JournalCmd.saveBody(id: widget.e.id, body: _text.text))
          .ignore();
    }
    _text.dispose();
    _focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = widget.c;
    final e = widget.e;
    final other = widget.st.recording != 0 && !_recording;
    return Container(
      decoration: cardDeco(context),
      padding: const EdgeInsets.fromLTRB(18, 12, 12, 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Flexible(
                child: SingleChildScrollView(
                  scrollDirection: Axis.horizontal,
                  child: _MoodRow(
                    mood: e.mood,
                    onPick: (m) =>
                        c.send(JournalCmd.setMood(id: e.id, mood: m)),
                  ),
                ),
              ),
              const Spacer(),
              const _Private(),
              PopupMenuButton<String>(
                tooltip: 'More',
                iconSize: 18,
                icon: Icon(Icons.more_horiz, color: t.nInk3),
                onSelected: (v) async {
                  if (v != 'delete') return;
                  if (await _confirmDelete(context)) {
                    await c.send(JournalCmd.deleteEntry(id: e.id));
                  }
                },
                itemBuilder: (_) => const [
                  PopupMenuItem(value: 'delete', child: Text('Delete entry')),
                ],
              ),
            ],
          ),
          if (_focus.hasFocus)
            Padding(
              padding: const EdgeInsets.only(top: 6),
              child: MarkdownBar(text: _text, onEdit: () => _changed('')),
            ),
          TextField(
            controller: _text,
            focusNode: _focus,
            onChanged: _changed,
            onTapOutside: (_) => _flush(),
            minLines: 3,
            maxLines: null,
            keyboardType: TextInputType.multiline,
            style: TextStyle(
                fontFamily: kSerif, fontSize: 16.5, height: 1.65, color: t.nInk),
            decoration: InputDecoration(
              border: InputBorder.none,
              isDense: true,
              contentPadding: const EdgeInsets.fromLTRB(0, 10, 6, 10),
              hintText: 'Write about the day…',
              hintStyle: TextStyle(
                  fontFamily: kSerif, fontSize: 16.5, color: t.nInk3),
            ),
          ),
          if (e.photos.isNotEmpty) ...[
            const SizedBox(height: 6),
            SizedBox(
              height: 72,
              child: ListView.separated(
                scrollDirection: Axis.horizontal,
                itemCount: e.photos.length,
                separatorBuilder: (_, __) => const SizedBox(width: 8),
                itemBuilder: (_, i) =>
                    PhotoThumb(id: e.photos[i].toInt(), size: 72),
              ),
            ),
            const SizedBox(height: 8),
          ],
          Divider(height: 14, color: t.nHair),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              _Chip(
                icon: Icons.photo_outlined,
                label: e.photos.isEmpty ? 'Photo' : plural(e.photos.length, 'photo'),
                onTap: widget.st.dayPhotos.isEmpty
                    ? null
                    : () => showPhotoPicker(context, c, e.id),
                tooltip: widget.st.dayPhotos.isEmpty
                    ? 'No photos were taken this day'
                    : 'Keep photos from the day in this entry',
              ),
              _Chip(
                icon: Icons.place_outlined,
                label: e.place.isEmpty ? 'Place' : e.place,
                onTap: () => _place(context),
              ),
              for (final tag in e.tags)
                _Chip(
                  icon: Icons.sell_outlined,
                  label: '#$tag',
                  tint: kJournal2,
                  onDelete: () =>
                      c.send(JournalCmd.removeTag(id: e.id, tag: tag)),
                ),
              _Chip(
                icon: Icons.add,
                label: 'Tag',
                onTap: () => _tag(context),
              ),
            ],
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              const Spacer(),
              Text(
                _dirty ? 'saving…' : '${plural(e.words, 'word')} · saved',
                style: TextStyle(fontSize: 11.5, color: t.nInk3),
              ),
              const SizedBox(width: 10),
              if (_recording) ...[
                IconButton(
                  tooltip: 'Throw this recording away',
                  iconSize: 17,
                  onPressed: () => c.send(const JournalCmd.recordCancel()),
                  icon: Icon(Icons.close, color: t.nInk3),
                ),
                FilledButton.icon(
                  style: FilledButton.styleFrom(
                      backgroundColor: Tokens.error,
                      visualDensity: VisualDensity.compact),
                  onPressed: c.stopRecording,
                  icon: const Icon(Icons.stop, size: 16),
                  label: Text('Stop  ${clock(_secs.toDouble())}'),
                ),
              ] else
                OutlinedButton.icon(
                  style: OutlinedButton.styleFrom(
                    foregroundColor: Tokens.error,
                    visualDensity: VisualDensity.compact,
                    side: BorderSide(
                        color: Tokens.error.withValues(alpha: 0.4)),
                  ),
                  onPressed: other
                      ? null
                      : () => c.send(JournalCmd.recordStart(id: e.id)),
                  icon: const Icon(Icons.mic_none, size: 16),
                  label: const Text('Record'),
                ),
            ],
          ),
        ],
      ),
    );
  }

  Future<void> _place(BuildContext context) async {
    final v = await askText(
      context,
      title: 'Where were you?',
      hint: 'Harbourside, Lisbon, home…',
      initial: widget.e.place,
      suggestions: widget.st.knownPlaces,
    );
    if (v == null) return;
    await widget.c.send(JournalCmd.setPlace(id: widget.e.id, place: v));
  }

  Future<void> _tag(BuildContext context) async {
    final v = await askText(
      context,
      title: 'Add a tag',
      hint: 'walks, work, lisbon…',
      suggestions: [
        for (final k in widget.st.knownTags)
          if (!widget.e.tags.contains(k)) k,
      ],
    );
    if (v == null || v.trim().isEmpty) return;
    await widget.c.send(JournalCmd.addTag(id: widget.e.id, tag: v));
  }
}

Future<bool> _confirmDelete(BuildContext context) async {
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Delete this entry?'),
      content: const Text(
          'Its words, mood, tags and voice notes go. Nothing in the other sections is touched.'),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Keep')),
        FilledButton(
          style: FilledButton.styleFrom(backgroundColor: Tokens.error),
          onPressed: () => Navigator.pop(ctx, true),
          child: const Text('Delete'),
        ),
      ],
    ),
  );
  return ok ?? false;
}

class _Private extends StatelessWidget {
  const _Private();

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: 'Kept in journal.db on this computer, and encrypted with the '
          'rest when Settings › Security has encryption on.',
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
        decoration: BoxDecoration(
          color: Tokens.ok.withValues(alpha: 0.10),
          borderRadius: BorderRadius.circular(6),
        ),
        child: const Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.lock_outline, size: 12, color: Tokens.ok),
            SizedBox(width: 4),
            Text('Private · on this computer',
                style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                    color: Tokens.ok)),
          ],
        ),
      ),
    );
  }
}

class _Chip extends StatelessWidget {
  const _Chip({
    required this.icon,
    required this.label,
    this.onTap,
    this.onDelete,
    this.tint,
    this.tooltip,
  });

  final IconData icon;
  final String label;
  final VoidCallback? onTap;
  final VoidCallback? onDelete;
  final Color? tint;
  final String? tooltip;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final ink = tint ?? t.nInk2;
    final chip = Material(
      color: tint?.withValues(alpha: 0.10) ?? t.nChip,
      borderRadius: BorderRadius.circular(8),
      child: InkWell(
        borderRadius: BorderRadius.circular(8),
        onTap: onTap,
        child: Padding(
          padding: EdgeInsets.fromLTRB(9, 5, onDelete == null ? 10 : 4, 5),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(icon,
                  size: 14,
                  color: onTap == null && onDelete == null
                      ? ink.withValues(alpha: 0.4)
                      : ink),
              const SizedBox(width: 5),
              ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 200),
                child: Text(label,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 12,
                        fontWeight: FontWeight.w600,
                        color: onTap == null && onDelete == null
                            ? ink.withValues(alpha: 0.4)
                            : ink)),
              ),
              if (onDelete != null)
                InkResponse(
                  radius: 12,
                  onTap: onDelete,
                  child: Padding(
                    padding: const EdgeInsets.only(left: 4),
                    child: Icon(Icons.close, size: 13, color: ink),
                  ),
                ),
            ],
          ),
        ),
      ),
    );
    return tooltip == null ? chip : Tooltip(message: tooltip!, child: chip);
  }
}

// ------------------------------------------------------------------- voice --

class _VoiceCard extends StatelessWidget {
  const _VoiceCard({required this.c, required this.v, required this.player});

  final JournalController c;
  final VoiceView v;
  final VoicePlayer player;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final working = c.transcribing == v.id;
    return AnimatedBuilder(
      animation: player,
      builder: (context, _) {
        final mine = player.current == v.id;
        final on = mine && player.playing;
        return Container(
          decoration: cardDeco(context),
          padding: const EdgeInsets.fromLTRB(14, 12, 10, 12),
          child: Row(
            children: [
              Material(
                color: kJournal2,
                shape: const CircleBorder(),
                child: InkWell(
                  customBorder: const CircleBorder(),
                  onTap: () => player.toggle(v),
                  child: SizedBox(
                    width: 38,
                    height: 38,
                    child: Icon(on ? Icons.pause : Icons.play_arrow,
                        color: Colors.white, size: 22),
                  ),
                ),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Expanded(
                          child: SizedBox(
                            height: 26,
                            child: CustomPaint(
                              painter: _WavePainter(
                                peaks: v.peaks.toList(),
                                done: mine ? player.fraction : 0,
                                on: kJournal2.withValues(alpha: 0.75),
                                off: t.nInk3.withValues(alpha: 0.25),
                              ),
                            ),
                          ),
                        ),
                        const SizedBox(width: 10),
                        Text(clock(v.durationS),
                            style: TextStyle(
                                fontSize: 12,
                                fontWeight: FontWeight.w700,
                                color: t.nInk)),
                      ],
                    ),
                    const SizedBox(height: 6),
                    Row(
                      children: [
                        Expanded(
                          child: Text(
                            working
                                ? 'Transcribing on this computer…'
                                : v.transcript.isNotEmpty
                                    ? '“${v.transcript}”'
                                    : v.state == 'failed'
                                        ? 'Not transcribed.'
                                        : 'Not transcribed yet.',
                            style: TextStyle(
                                fontFamily:
                                    v.transcript.isNotEmpty ? kSerif : null,
                                fontSize: 13,
                                height: 1.45,
                                fontStyle: v.transcript.isNotEmpty
                                    ? FontStyle.italic
                                    : null,
                                color: t.nInk2),
                          ),
                        ),
                        const SizedBox(width: 8),
                        if (v.transcript.isNotEmpty)
                          const _Pill(
                              icon: Icons.graphic_eq,
                              text: 'Transcribed here')
                        else if (!working)
                          TextButton(
                            onPressed: c.transcribing != 0
                                ? null
                                : () => c.transcribe(v.id),
                            child: const Text('Transcribe'),
                          ),
                      ],
                    ),
                  ],
                ),
              ),
              PopupMenuButton<String>(
                tooltip: 'More',
                iconSize: 17,
                icon: Icon(Icons.more_vert, color: t.nInk3),
                onSelected: (_) =>
                    c.send(JournalCmd.deleteVoice(id: v.id)),
                itemBuilder: (_) => const [
                  PopupMenuItem(
                      value: 'delete', child: Text('Delete voice note')),
                ],
              ),
            ],
          ),
        );
      },
    );
  }
}

class _WavePainter extends CustomPainter {
  _WavePainter({
    required this.peaks,
    required this.done,
    required this.on,
    required this.off,
  });

  final List<double> peaks;
  final double done;
  final Color on;
  final Color off;

  @override
  void paint(Canvas canvas, Size size) {
    if (peaks.isEmpty) return;
    const gap = 3.0;
    final w = math.max(2.0, (size.width - gap * (peaks.length - 1)) / peaks.length);
    for (var i = 0; i < peaks.length; i++) {
      final h = math.max(3.0, peaks[i] * size.height);
      final x = i * (w + gap);
      final played = (i + 0.5) / peaks.length <= done;
      canvas.drawRRect(
        RRect.fromRectAndRadius(
          Rect.fromLTWH(x, (size.height - h) / 2, w, h),
          const Radius.circular(2),
        ),
        Paint()..color = played || done == 0 ? on : off,
      );
    }
  }

  @override
  bool shouldRepaint(_WavePainter old) =>
      old.done != done || old.peaks != peaks || old.on != on;
}

class _Pill extends StatelessWidget {
  const _Pill({required this.icon, required this.text});

  final IconData icon;
  final String text;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
      decoration: BoxDecoration(
        color: kJournal.withValues(alpha: 0.13),
        borderRadius: BorderRadius.circular(6),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 12, color: kJournal2),
          const SizedBox(width: 4),
          Text(text,
              style: const TextStyle(
                  fontSize: 11,
                  fontWeight: FontWeight.w600,
                  color: kJournal2)),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------- gathered --

class _Gathered extends StatelessWidget {
  const _Gathered({required this.c, required this.st});

  final JournalController c;
  final JournalState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final rows = st.gathered;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const H2(
          title: 'Your day, gathered',
          hint: 'from the other sections · switch off what you’d rather not keep',
        ),
        const SizedBox(height: 10),
        Container(
          decoration: cardDeco(context),
          padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 4),
          child: rows.isEmpty
              ? Padding(
                  padding: const EdgeInsets.symmetric(vertical: 18),
                  child: Text(
                    'Nothing from Photos, Music, Videos, Books or Finances for this day yet.',
                    style: TextStyle(fontSize: 13, color: t.nInk3),
                  ),
                )
              : Column(
                  children: [
                    for (var i = 0; i < rows.length; i++) ...[
                      if (i > 0) Divider(height: 1, color: t.nHair),
                      _GatherRowView(c: c, r: rows[i]),
                    ],
                  ],
                ),
        ),
      ],
    );
  }
}

class _GatherRowView extends StatelessWidget {
  const _GatherRowView({required this.c, required this.r});

  final JournalController c;
  final GatherRow r;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final look = sourceLook(r.source);
    return Opacity(
      opacity: r.shown ? 1 : 0.45,
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 10),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SizedBox(
              width: 46,
              child: Padding(
                padding: const EdgeInsets.only(top: 4),
                child: Text(r.clock,
                    style: TextStyle(
                        fontSize: 12,
                        color: t.nInk3,
                        fontFeatures: const [FontFeature.tabularFigures()])),
              ),
            ),
            Container(
              width: 26,
              height: 26,
              decoration: BoxDecoration(
                color: look.tint.withValues(alpha: 0.14),
                borderRadius: BorderRadius.circular(7),
              ),
              child: Icon(look.icon, size: 15, color: look.tint),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(look.label,
                      style: TextStyle(fontSize: 11.5, color: t.nInk3)),
                  const SizedBox(height: 2),
                  Text(r.title,
                      style: TextStyle(
                          fontSize: 13.5,
                          fontWeight: FontWeight.w500,
                          color: t.nInk)),
                  if (r.ids.isNotEmpty && r.shown) ...[
                    const SizedBox(height: 10),
                    Wrap(
                      spacing: 6,
                      runSpacing: 6,
                      children: [
                        for (final id in ints(r.ids)) PhotoThumb(id: id, size: 50)
                      ],
                    ),
                  ],
                ],
              ),
            ),
            Switch(
              value: r.shown,
              activeTrackColor: kJournal2,
              onChanged: (v) =>
                  c.send(JournalCmd.setShown(source: r.source, shown: v)),
            ),
          ],
        ),
      ),
    );
  }
}

// -------------------------------------------------------------------- rail --

class _Rail extends StatelessWidget {
  const _Rail({required this.c, required this.st});

  final JournalController c;
  final JournalState st;

  @override
  Widget build(BuildContext context) {
    final p = st.places;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (p.stops > 0 || p.names.isNotEmpty) ...[
          _PlacesCard(c: c, p: p),
          const SizedBox(height: 14),
        ],
        if (st.otd.isNotEmpty) ...[
          _OtdMini(c: c, card: st.otd.first),
          const SizedBox(height: 14),
        ],
        _Question(c: c, prompt: st.prompt),
        const SizedBox(height: 14),
        _Streak(st: st),
      ],
    );
  }
}

class _PlacesCard extends StatelessWidget {
  const _PlacesCard({required this.c, required this.p});

  final JournalController c;
  final PlacesView p;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final head = p.stops > 0 ? plural(p.stops, 'place') : 'Places';
    final line = [
      if (p.km >= 0.1) '${p.km.toStringAsFixed(1)} km between them',
      ...p.names,
    ].join(' · ');
    return Container(
      clipBehavior: Clip.antiAlias,
      decoration: cardDeco(context),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (p.stops > 0)
            SizedBox(
              height: 130,
              child: CustomPaint(
                painter: _MapPainter(
                  points: [for (final m in p.points) Offset(m.x, m.y)],
                  labels: p.labels,
                  labelStyle: TextStyle(
                      fontSize: 10.5,
                      fontWeight: FontWeight.w600,
                      color: t.nInk2),
                  ground: kJournal.withValues(alpha: t.dark ? 0.08 : 0.06),
                  road: t.nInk3.withValues(alpha: 0.18),
                  water: const Color(0xFF38BDF8).withValues(alpha: 0.35),
                  route: kJournal2,
                  pinRing: t.nCard,
                ),
              ),
            ),
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 10, 14, 12),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(head,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
                if (line.isNotEmpty) ...[
                  const SizedBox(height: 3),
                  Text(line,
                      style: TextStyle(
                          fontSize: 12, height: 1.4, color: t.nInk2)),
                ],
                if (p.stops > 0 && p.names.isEmpty) ...[
                  const SizedBox(height: 3),
                  Text('From where the day’s photos were taken',
                      style: TextStyle(fontSize: 11.5, color: t.nInk3)),
                ],
                if (p.canName)
                  Align(
                    alignment: Alignment.centerLeft,
                    child: TextButton.icon(
                      style: TextButton.styleFrom(
                          padding: EdgeInsets.zero,
                          visualDensity: VisualDensity.compact),
                      onPressed: () =>
                          c.send(const JournalCmd.getPlaceNames()),
                      icon: const Icon(Icons.travel_explore, size: 15),
                      label: const Text('Name the towns · a 3 MB download, once'),
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

/// The day's stops as plain shapes, as the deck draws them: a ground, two
/// roads and a river for texture (not geography), and the route between
/// the pins in the order they were visited.
///
/// ponytail: no tiles. An offline tile set would make the lines real; the
/// towns come from GeoNames once it is fetched.
class _MapPainter extends CustomPainter {
  _MapPainter({
    required this.points,
    required this.labels,
    required this.labelStyle,
    required this.ground,
    required this.road,
    required this.water,
    required this.route,
    required this.pinRing,
  });

  final List<Offset> points;

  /// A town per pin; only a change of town is written, so a day in one city
  /// says its name once.
  final List<String> labels;
  final TextStyle labelStyle;
  final Color ground;
  final Color road;
  final Color water;
  final Color route;
  final Color pinRing;

  @override
  void paint(Canvas canvas, Size size) {
    final w = size.width, h = size.height;
    canvas.drawRect(Offset.zero & size, Paint()..color = ground);
    final roads = Paint()
      ..color = road
      ..strokeWidth = 2.5
      ..style = PaintingStyle.stroke;
    canvas.drawLine(Offset(0, h * 0.28), Offset(w, h * 0.18), roads);
    canvas.drawLine(Offset(w * 0.62, 0), Offset(w * 0.56, h), roads);
    canvas.drawPath(
      Path()
        ..moveTo(0, h * 0.86)
        ..quadraticBezierTo(w * 0.35, h * 0.62, w * 0.6, h * 0.8)
        ..quadraticBezierTo(w * 0.85, h * 0.95, w, h * 0.55),
      Paint()
        ..color = water
        ..strokeWidth = 9
        ..style = PaintingStyle.stroke,
    );
    final at = [for (final p in points) Offset(p.dx * w, p.dy * h)];
    final dash = Paint()
      ..color = route
      ..strokeWidth = 2.2
      ..strokeCap = StrokeCap.round;
    for (var i = 0; i + 1 < at.length; i++) {
      final a = at[i], b = at[i + 1];
      final len = (b - a).distance;
      if (len == 0) continue;
      final dir = (b - a) / len;
      for (var d = 0.0; d < len; d += 9) {
        canvas.drawLine(a + dir * d, a + dir * math.min(d + 5, len), dash);
      }
    }
    for (final p in at) {
      canvas.drawCircle(p, 7, Paint()..color = pinRing);
      canvas.drawCircle(p, 5, Paint()..color = route);
    }
    var last = '';
    for (var i = 0; i < at.length && i < labels.length; i++) {
      final name = labels[i];
      if (name.isEmpty || name == last) continue;
      last = name;
      final tp = TextPainter(
        text: TextSpan(text: name, style: labelStyle),
        textDirection: TextDirection.ltr,
        maxLines: 1,
        ellipsis: '…',
      )..layout(maxWidth: w * 0.45);
      var o = at[i] + const Offset(9, -7);
      if (o.dx + tp.width > w - 4) o = at[i] + Offset(-9 - tp.width, -7);
      o = Offset(o.dx, o.dy.clamp(2.0, h - tp.height - 2));
      final pad = Rect.fromLTWH(o.dx - 3, o.dy - 1, tp.width + 6, tp.height + 2);
      canvas.drawRRect(RRect.fromRectAndRadius(pad, const Radius.circular(4)),
          Paint()..color = pinRing.withValues(alpha: 0.85));
      tp.paint(canvas, o);
    }
  }

  @override
  bool shouldRepaint(_MapPainter old) =>
      old.points != points ||
      old.labels != labels ||
      old.ground != ground ||
      old.route != route;
}

class _OtdMini extends StatelessWidget {
  const _OtdMini({required this.c, required this.card});

  final JournalController c;
  final OtdCard card;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      clipBehavior: Clip.antiAlias,
      decoration: cardDeco(context),
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Text('On this day',
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
              const Spacer(),
              OutlinedButton(
                style: OutlinedButton.styleFrom(
                    visualDensity: VisualDensity.compact),
                onPressed: () =>
                    c.send(const JournalCmd.setTab(tab: 'otd')),
                child: const Text('All'),
              ),
            ],
          ),
          const SizedBox(height: 8),
          ClipRRect(
            borderRadius: BorderRadius.circular(10),
            child: SizedBox(
              height: 96,
              child: Stack(
                fit: StackFit.expand,
                children: [
                  if (card.photos.isNotEmpty)
                    PhotoThumb(id: card.photos.first.toInt(), size: 0)
                  else
                    const DecoratedBox(
                      decoration: BoxDecoration(
                        gradient: LinearGradient(
                          colors: [Color(0xFFFCD34D), kJournal2],
                          begin: Alignment.topLeft,
                          end: Alignment.bottomRight,
                        ),
                      ),
                    ),
                  Positioned(
                    left: 8,
                    top: 8,
                    child: YearBadge(text: '${card.year}'),
                  ),
                ],
              ),
            ),
          ),
          const SizedBox(height: 8),
          InkWell(
            onTap: () => card.written
                ? c.send(JournalCmd.go(day: card.day))
                : c.send(JournalCmd.writeOn(day: card.day)),
            child: Text(
              card.written ? '“${card.text}”' : card.text,
              maxLines: 4,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontFamily: kSerif,
                  fontSize: 14,
                  height: 1.45,
                  color: t.nInk2),
            ),
          ),
        ],
      ),
    );
  }
}

class _Question extends StatelessWidget {
  const _Question({required this.c, required this.prompt});

  final JournalController c;
  final String prompt;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final r = context.skin.panelRadius ?? 14;
    return Container(
      padding: const EdgeInsets.fromLTRB(14, 12, 14, 14),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(r),
        border: Border.all(color: kJournal.withValues(alpha: 0.25)),
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [
            kJournal.withValues(alpha: t.dark ? 0.14 : 0.10),
            kJournal2.withValues(alpha: t.dark ? 0.06 : 0.04),
          ],
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('A question for tonight',
              style: TextStyle(fontSize: 11.5, color: t.nInk3)),
          const SizedBox(height: 4),
          Text(prompt,
              style: TextStyle(
                  fontFamily: kSerif,
                  fontSize: 17,
                  height: 1.3,
                  fontWeight: FontWeight.w700,
                  color: t.nInk)),
          const SizedBox(height: 10),
          Row(
            children: [
              FilledButton(
                style: FilledButton.styleFrom(
                    backgroundColor: kJournal2,
                    visualDensity: VisualDensity.compact),
                onPressed: () => c.send(const JournalCmd.answer()),
                child: const Text('Answer'),
              ),
              const SizedBox(width: 8),
              OutlinedButton(
                style: OutlinedButton.styleFrom(
                    visualDensity: VisualDensity.compact),
                onPressed: () => c.send(const JournalCmd.nextPrompt()),
                child: const Text('Another'),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class _Streak extends StatelessWidget {
  const _Streak({required this.st});

  final JournalState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final head = st.streak > 0
        ? '${plural(st.streak, 'day')} in a row'
        : 'Write today to start a streak';
    final sub = st.longest > 0
        ? 'Longest: ${plural(st.longest, 'day')}${st.longestWhen.isEmpty ? '' : ', ${st.longestWhen}'}'
        : 'A day counts once anything is written';
    return Container(
      decoration: cardDeco(context),
      padding: const EdgeInsets.all(12),
      child: Row(
        children: [
          Container(
            width: 36,
            height: 36,
            decoration: BoxDecoration(
              color: kJournal.withValues(alpha: 0.14),
              borderRadius: BorderRadius.circular(10),
            ),
            child: const Icon(Icons.local_fire_department_outlined,
                size: 20, color: kJournal2),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(head,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
                const SizedBox(height: 2),
                Text(sub, style: TextStyle(fontSize: 12, color: t.nInk3)),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

// ----------------------------------------------------------------- dialogs --

/// The day's photos, to keep in an entry or let go of. Each tap is saved at
/// once; the grid follows the controller.
Future<void> showPhotoPicker(
    BuildContext context, JournalController c, int entryId) {
  return showDialog<void>(
    context: context,
    builder: (ctx) => AnimatedBuilder(
      animation: c,
      builder: (ctx, _) {
        final st = c.state;
        if (st == null) return const SizedBox.shrink();
        final kept = <int>{
          for (final e in st.entries)
            if (e.id == entryId) ...ints(e.photos),
        };
        final t = ctx.tokens;
        return AlertDialog(
          title: const Text('Keep photos in this entry'),
          content: SizedBox(
            width: 560,
            height: 380,
            child: GridView.count(
              crossAxisCount: 5,
              mainAxisSpacing: 8,
              crossAxisSpacing: 8,
              children: [
                for (final id in ints(st.dayPhotos))
                  InkWell(
                    borderRadius: BorderRadius.circular(8),
                    onTap: () => c.send(JournalCmd.keepPhoto(
                        id: entryId, photo: id, keep: !kept.contains(id))),
                    child: Stack(
                      fit: StackFit.expand,
                      children: [
                        PhotoThumb(id: id, size: 0),
                        if (kept.contains(id))
                          Container(
                            decoration: BoxDecoration(
                              borderRadius: BorderRadius.circular(8),
                              border: Border.all(color: kJournal2, width: 3),
                            ),
                            alignment: Alignment.topRight,
                            padding: const EdgeInsets.all(4),
                            child: const Icon(Icons.check_circle,
                                size: 20, color: kJournal2),
                          ),
                      ],
                    ),
                  ),
              ],
            ),
          ),
          actions: [
            Text('${kept.length} kept',
                style: TextStyle(fontSize: 12.5, color: t.nInk3)),
            FilledButton(
              style: FilledButton.styleFrom(backgroundColor: kJournal2),
              onPressed: () => Navigator.pop(ctx),
              child: const Text('Done'),
            ),
          ],
        );
      },
    ),
  );
}

/// One line of text, with earlier answers as chips. Null when cancelled.
Future<String?> askText(
  BuildContext context, {
  required String title,
  required String hint,
  String initial = '',
  List<String> suggestions = const [],
}) {
  final ctl = TextEditingController(text: initial);
  return showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(title),
      content: SizedBox(
        width: 380,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            TextField(
              controller: ctl,
              autofocus: true,
              decoration: InputDecoration(hintText: hint),
              onSubmitted: (v) => Navigator.pop(ctx, v),
            ),
            if (suggestions.isNotEmpty) ...[
              const SizedBox(height: 14),
              Wrap(
                spacing: 6,
                runSpacing: 6,
                children: [
                  for (final s in suggestions)
                    ActionChip(
                      label: Text(s),
                      onPressed: () => Navigator.pop(ctx, s),
                    ),
                ],
              ),
            ],
          ],
        ),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: const Text('Cancel')),
        FilledButton(
          style: FilledButton.styleFrom(backgroundColor: kJournal2),
          onPressed: () => Navigator.pop(ctx, ctl.text),
          child: const Text('Save'),
        ),
      ],
    ),
  );
}

// ------------------------------------------------------------------ shared --

Decoration cardDeco(BuildContext context, {double? radius}) {
  final t = context.tokens;
  final r = radius ?? context.skin.panelRadius ?? 14;
  return context.skin.surface(SurfaceRole.card, radius: r) ??
      BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(r),
        border: Border.all(color: t.nHair),
      );
}

/// A photo from Photos by id, cropped square. [size] 0 fills its box.
class PhotoThumb extends StatelessWidget {
  const PhotoThumb({super.key, required this.id, required this.size});

  final int id;
  final double size;

  @override
  Widget build(BuildContext context) {
    final img = ClipRRect(
      borderRadius: BorderRadius.circular(size == 0 ? 0 : 8),
      child: LazyCover(
        section: Section.photos,
        id: id,
        tint: Tokens.secPhotos,
        icon: Icons.photo_outlined,
        alignment: Alignment.topCenter,
      ),
    );
    return size == 0 ? img : SizedBox(width: size, height: size, child: img);
  }
}

class YearBadge extends StatelessWidget {
  const YearBadge({super.key, required this.text});

  final String text;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
      decoration: BoxDecoration(
        color: Colors.black.withValues(alpha: 0.45),
        borderRadius: BorderRadius.circular(6),
      ),
      child: Text(text,
          style: const TextStyle(
              fontSize: 11, fontWeight: FontWeight.w700, color: Colors.white)),
    );
  }
}

class H2 extends StatelessWidget {
  const H2({super.key, required this.title, this.hint = '', this.size = 15.5});

  final String title;
  final String hint;
  final double size;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Text.rich(
      TextSpan(children: [
        TextSpan(
            text: title,
            style: TextStyle(
                fontSize: size, fontWeight: FontWeight.w700, color: t.nInk)),
        if (hint.isNotEmpty)
          TextSpan(
              text: '   $hint',
              style: TextStyle(
                  fontSize: 12, fontWeight: FontWeight.w500, color: t.nInk3)),
      ]),
    );
  }
}

class Strip extends StatelessWidget {
  const Strip({
    super.key,
    required this.icon,
    required this.tint,
    required this.text,
    required this.onClose,
  });

  final IconData icon;
  final Color tint;
  final String text;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(22, 6, 8, 6),
      color: tint.withValues(alpha: 0.10),
      child: Row(
        children: [
          Icon(icon, size: 16, color: tint),
          const SizedBox(width: 10),
          Expanded(
            child: Text(text,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12.5, color: t.nInk)),
          ),
          IconButton(
            iconSize: 16,
            tooltip: 'Dismiss',
            onPressed: onClose,
            icon: const Icon(Icons.close),
          ),
        ],
      ),
    );
  }
}
