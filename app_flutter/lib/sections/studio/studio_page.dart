// The Studio section — docs/mockups/NewSections/studio-deck.html.
//
// Create (ideas, and the sources a project can start from: trips in Places,
// albums, people, dates), Projects, and a project open: the photos picked for
// it, its length, shape and song, and Render. Places' trip page sends a trip
// here with "Make a movie".

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/section_tabs.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/studio.dart';
import '../journal/journal_page.dart' show PhotoThumb;
import '../kitchen/kitchen_page.dart' show Grid, Quiet, Strip, askLine, cardDeco, confirm;
import '../papers/papers_page.dart' show TabPill;
import 'studio_controller.dart';

class StudioPage extends StatefulWidget {
  const StudioPage({super.key, required this.visible});

  final bool visible;

  @override
  State<StudioPage> createState() => _StudioPageState();
}

class _StudioPageState extends State<StudioPage> {
  final StudioController _c = StudioController();

  @override
  void initState() {
    super.initState();
    _c.refresh();
    // "Make a movie" on a trip in Places.
    ShellController.instance.onOpen(Section.studio, (raw) async {
      final a = openArg(raw);
      if (a.verb != 'trip' || a.arg.isEmpty) return;
      await _c.send(StudioCmd.create(
        title: '',
        kind: 'movie',
        source: 'trip:${a.arg}',
        start: 0,
        end: 0,
        length: 'medium',
        shape: 'wide',
      ));
    });
  }

  @override
  void didUpdateWidget(StudioPage old) {
    super.didUpdateWidget(old);
    if (widget.visible && !old.visible) _c.refresh(quiet: true);
    if (!widget.visible && old.visible) _c.pause();
  }

  @override
  void dispose() {
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
              if (_c.busy || (st?.busy ?? false))
                const LinearProgressIndicator(minHeight: 2, color: kStudio)
              else
                const SizedBox(height: 2),
              if (_c.notice.isNotEmpty)
                Strip(
                  icon: Icons.check_circle_outline,
                  tint: kStudio,
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
              if (st != null && !st.ready)
                Strip(
                  icon: Icons.info_outline,
                  tint: Tokens.warn,
                  text: 'ffmpeg is missing, and Studio renders with it.',
                  onClose: _c.refresh,
                ),
              Expanded(
                child: st == null
                    ? FirstLoad(error: _c.error, onRetry: _c.refresh)
                    : st.open != null
                        ? _Editor(c: _c, st: st, p: st.open!)
                        : st.tab == 'projects'
                            ? _Projects(c: _c, st: st)
                            : _Create(c: _c, st: st),
              ),
            ],
          ),
        );
      },
    );
  }
}

class _Header extends StatelessWidget {
  const _Header({required this.c, required this.st});

  final StudioController c;
  final StudioState? st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tab = st?.open != null ? '' : st?.tab ?? 'create';
    final r = context.skin.controlRadius ?? 10;
    final rendering = st?.projects
            .where((p) => p.state == 'rendering' || p.state == 'queued')
            .length ??
        0;
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
              color: kStudio.withValues(alpha: 0.17),
              borderRadius: BorderRadius.circular(10),
            ),
            child: const Icon(Icons.movie_creation_outlined,
                size: 18, color: kStudio),
          ),
          const SizedBox(width: 10),
          Text('Studio',
              style: TextStyle(
                  fontSize: 19, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(width: 14),
          Container(
            padding: const EdgeInsets.all(3),
            decoration: BoxDecoration(
              color: t.nChip,
              borderRadius: BorderRadius.circular(r + 3),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                for (final f in keepTabs('studio', studioTabs, (f) => f.id,
                    active: (f) => tab == f.id))
                  TabPill(
                    label: f.label,
                    on: f.id == tab,
                    count: f.id == 'projects' ? rendering : 0,
                    radius: r,
                    tint: kStudio,
                    onTap: () => c.send(StudioCmd.setTab(tab: f.id)),
                  ),
              ],
            ),
          ),
          const Spacer(),
          if (st != null)
            Flexible(
              child: Text('Saves to ${st!.folder}',
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 12, color: t.nInk3)),
            ),
        ],
      ),
    );
  }
}

// ------------------------------------------------------------------ create --

int _secs(DateTime d) => d.millisecondsSinceEpoch ~/ 1000;

/// Ask what to make from a source, then make it.
Future<void> _make(BuildContext context, StudioController c,
    {required String title,
    required String source,
    int start = 0,
    int end = 0}) async {
  final pick = await showDialog<(String, String, String)>(
    context: context,
    builder: (ctx) => SimpleDialog(
      title: Text('Make something from $title'),
      children: [
        for (final (kind, shape, length, label, sub, icon) in const [
          ('movie', 'wide', 'medium', 'Movie', 'About a minute, 16:9, to a song', Icons.movie_outlined),
          ('movie', 'tall', 'short', 'Vertical reel', '30 seconds, 9:16, for a phone', Icons.smartphone),
          ('movie', 'square', 'medium', 'Square movie', 'About a minute, 1:1', Icons.crop_square),
          ('movie', 'wide', 'long', 'Long movie', 'About three minutes, 16:9', Icons.theaters_outlined),
          ('book', 'wide', 'medium', 'Photo book', 'A PDF of up to 48 photos, 21 cm square', Icons.menu_book_outlined),
        ])
          SimpleDialogOption(
            onPressed: () => Navigator.pop(ctx, (kind, shape, length)),
            child: ListTile(
              leading: Icon(icon, color: kStudio),
              title: Text(label),
              subtitle: Text(sub),
            ),
          ),
      ],
    ),
  );
  if (pick == null) return;
  await c.send(StudioCmd.create(
    title: title,
    kind: pick.$1,
    source: source,
    start: start,
    end: end,
    length: pick.$3,
    shape: pick.$2,
  ));
}

class _Create extends StatelessWidget {
  const _Create({required this.c, required this.st});

  final StudioController c;
  final StudioState st;

  Future<void> _dates(BuildContext context) async {
    final now = DateTime.now();
    final range = await showDateRangePicker(
      context: context,
      firstDate: DateTime(1990),
      lastDate: now,
      initialDateRange:
          DateTimeRange(start: DateTime(now.year, now.month, 1), end: now),
    );
    if (range == null || !context.mounted) return;
    final end = DateTime(range.end.year, range.end.month, range.end.day + 1);
    final title =
        '${range.start.day}/${range.start.month} – ${range.end.day}/${range.end.month}/${range.end.year}';
    await _make(context, c,
        title: title,
        source: 'range',
        start: _secs(range.start),
        end: _secs(end) - 1);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final now = DateTime.now();
    Widget section(String title, List<SourcePick> picks) => Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const SizedBox(height: 18),
            Text(title,
                style: TextStyle(
                    fontSize: 15, fontWeight: FontWeight.w700, color: t.nInk)),
            const SizedBox(height: 8),
            Wrap(spacing: 8, runSpacing: 8, children: [
              for (final p in picks)
                ActionChip(
                  label: Text('${p.label} · ${p.sub}'),
                  onPressed: () => _make(context, c,
                      title: p.label,
                      source: p.source,
                      start: p.start,
                      end: p.end),
                ),
            ]),
          ],
        );
    final months = [
      ('This month', DateTime(now.year, now.month, 1), DateTime(now.year, now.month + 1, 1)),
      ('Last month', DateTime(now.year, now.month - 1, 1), DateTime(now.year, now.month, 1)),
      ('This year', DateTime(now.year, 1, 1), DateTime(now.year + 1, 1, 1)),
      ('Last year', DateTime(now.year - 1, 1, 1), DateTime(now.year, 1, 1)),
    ];
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 22, 24, 30),
      children: [
        Text('Made for you',
            style: TextStyle(
                fontSize: 16, fontWeight: FontWeight.w700, color: t.nInk)),
        const SizedBox(height: 10),
        Grid(min: 280, children: [
          for (final i in st.ideas)
            Container(
              padding: const EdgeInsets.all(16),
              decoration: cardDeco(context),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(children: [
                    Icon(
                        i.shape == 'tall'
                            ? Icons.smartphone
                            : Icons.movie_outlined,
                        color: kStudio),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Text(i.title,
                          style: TextStyle(
                              fontSize: 15,
                              fontWeight: FontWeight.w700,
                              color: t.nInk)),
                    ),
                  ]),
                  const SizedBox(height: 6),
                  Text(i.sub,
                      style: TextStyle(fontSize: 12.5, color: t.nInk2)),
                  const SizedBox(height: 12),
                  FilledButton(
                    style: FilledButton.styleFrom(backgroundColor: kStudio),
                    onPressed: () => c.send(StudioCmd.create(
                      title: i.title,
                      kind: i.kind,
                      source: i.source,
                      start: i.start,
                      end: i.end,
                      length: i.length,
                      shape: i.shape,
                    )),
                    child: const Text('Make it'),
                  ),
                ],
              ),
            ),
        ]),
        if (st.trips.isNotEmpty) section('From a trip', st.trips),
        if (st.albums.isNotEmpty) section('From an album', st.albums),
        if (st.people.isNotEmpty) section('Of someone', st.people),
        const SizedBox(height: 18),
        Text('From dates',
            style: TextStyle(
                fontSize: 15, fontWeight: FontWeight.w700, color: t.nInk)),
        const SizedBox(height: 8),
        Wrap(spacing: 8, runSpacing: 8, children: [
          for (final (label, a, b) in months)
            ActionChip(
              label: Text(label),
              onPressed: () => _make(context, c,
                  title: label == 'This year' || label == 'Last year'
                      ? '${a.year}'
                      : '${_monthName(a.month)} ${a.year}',
                  source: 'range',
                  start: _secs(a),
                  end: _secs(b) - 1),
            ),
          ActionChip(
            avatar: const Icon(Icons.date_range, size: 16),
            label: const Text('Choose dates…'),
            onPressed: () => _dates(context),
          ),
        ]),
        if (st.trips.isEmpty)
          Padding(
            padding: const EdgeInsets.only(top: 18),
            child: Text(
                'Trips come from Places: add it in Settings › Sections, keep a '
                'trip, and it shows up here as a travel movie.',
                style: TextStyle(fontSize: 12.5, color: t.nInk3)),
          ),
      ],
    );
  }
}

String _monthName(int m) => const [
      'January', 'February', 'March', 'April', 'May', 'June', 'July', //
      'August', 'September', 'October', 'November', 'December',
    ][(m - 1) % 12];

// ---------------------------------------------------------------- projects --

class _Projects extends StatelessWidget {
  const _Projects({required this.c, required this.st});

  final StudioController c;
  final StudioState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (st.projects.isEmpty) {
      return Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 460),
          child: const Quiet(
            icon: Icons.movie_creation_outlined,
            title: 'Nothing made yet',
            body: 'Start from an idea or a source on Create. Each project keeps '
                'its recipe, so it can be rendered again in another shape.',
          ),
        ),
      );
    }
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 22, 24, 30),
      children: [
        for (final p in st.projects)
          Container(
            margin: const EdgeInsets.only(bottom: 10),
            padding: const EdgeInsets.all(12),
            decoration: cardDeco(context),
            child: Row(children: [
              SizedBox(
                width: 96,
                height: 60,
                child: p.cover > 0
                    ? PhotoThumb(id: p.cover.toInt(), size: 0)
                    : Icon(Icons.image_outlined, color: t.nInk3),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: InkWell(
                  onTap: () => c.send(StudioCmd.open(id: p.id)),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(p.title,
                          style: TextStyle(
                              fontWeight: FontWeight.w700, color: t.nInk)),
                      const SizedBox(height: 3),
                      Text(p.sub,
                          style: TextStyle(fontSize: 12.5, color: t.nInk2)),
                      if (p.state == 'rendering' || p.state == 'queued') ...[
                        const SizedBox(height: 6),
                        LinearProgressIndicator(
                          value: p.state == 'queued' ? null : p.progress,
                          color: kStudio,
                        ),
                      ] else if (p.state == 'failed')
                        const Text('Did not render — open it to see why',
                            style: TextStyle(
                                fontSize: 12, color: Tokens.error)),
                    ],
                  ),
                ),
              ),
              if (p.state == 'done') ...[
                IconButton(
                  tooltip: 'Open',
                  icon: const Icon(Icons.play_circle_outline),
                  onPressed: () =>
                      c.send(StudioCmd.showFile(id: p.id, folder: false)),
                ),
                IconButton(
                  tooltip: 'Show in its folder',
                  icon: const Icon(Icons.folder_open_outlined),
                  onPressed: () =>
                      c.send(StudioCmd.showFile(id: p.id, folder: true)),
                ),
              ],
              IconButton(
                tooltip: 'Delete the project',
                icon: const Icon(Icons.delete_outline),
                onPressed: p.state == 'rendering'
                    ? null
                    : () async {
                        final ok = await confirm(context, 'Delete this project?',
                            'The recipe goes; a movie or book already made stays where it is.');
                        if (ok) await c.send(StudioCmd.delete(id: p.id));
                      },
              ),
            ]),
          ),
      ],
    );
  }
}

// ------------------------------------------------------------------ editor --

class _Editor extends StatelessWidget {
  const _Editor({required this.c, required this.st, required this.p});

  final StudioController c;
  final StudioState st;
  final ProjectView p;

  Future<void> _rename(BuildContext context) async {
    final title =
        await askLine(context, 'Rename', 'What it is called', initial: p.title);
    if (title != null && title.trim().isNotEmpty) {
      await c.send(StudioCmd.rename(id: p.id, title: title));
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final movie = p.kind == 'movie';
    final working = p.state == 'rendering' || p.state == 'queued';
    Widget chips(String label, List<(String, String)> opts, String on,
            void Function(String) pick) =>
        Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(label.toUpperCase(),
                style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w700,
                    letterSpacing: 0.7,
                    color: t.nInk3)),
            const SizedBox(height: 6),
            Wrap(spacing: 6, runSpacing: 6, children: [
              for (final (id, name) in opts)
                ChoiceChip(
                  label: Text(name),
                  selected: on == id,
                  selectedColor: kStudio.withValues(alpha: 0.18),
                  onSelected: working ? null : (_) => pick(id),
                ),
            ]),
            const SizedBox(height: 14),
          ],
        );
    final side = Container(
      padding: const EdgeInsets.all(16),
      decoration: cardDeco(context),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (movie) ...[
            chips(
                'Length',
                const [
                  ('short', '30 s'),
                  ('medium', '1 min'),
                  ('long', '3 min'),
                  ('song', 'The song'),
                ],
                p.length,
                (v) => c.send(StudioCmd.setLength(id: p.id, length: v))),
            chips(
                'Shape',
                const [('wide', '16:9'), ('tall', '9:16'), ('square', '1:1')],
                p.shape,
                (v) => c.send(StudioCmd.setShape(id: p.id, shape: v))),
            Text('SONG',
                style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w700,
                    letterSpacing: 0.7,
                    color: t.nInk3)),
            const SizedBox(height: 6),
            DropdownButton<int>(
              isExpanded: true,
              value: [0, for (final s in st.songs) s.id.toInt()]
                      .contains(p.musicId.toInt())
                  ? p.musicId.toInt()
                  : 0,
              items: [
                const DropdownMenuItem(value: 0, child: Text('No music')),
                for (final s in st.songs)
                  DropdownMenuItem(
                    value: s.id.toInt(),
                    child: Text(
                        '${s.title}${s.artist.isEmpty ? '' : ' · ${s.artist}'} · ${s.length}',
                        overflow: TextOverflow.ellipsis),
                  ),
              ],
              onChanged: working
                  ? null
                  : (v) => c.send(StudioCmd.setMusic(id: p.id, musicId: v ?? 0)),
            ),
            SwitchListTile(
              contentPadding: EdgeInsets.zero,
              value: p.beat,
              activeThumbColor: kStudio,
              title: const Text('Cut on the beat'),
              subtitle: const Text('Uses the tempo Music measured'),
              onChanged: working
                  ? null
                  : (v) => c.send(StudioCmd.setBeat(id: p.id, beat: v)),
            ),
          ],
          Text(p.plan, style: TextStyle(fontSize: 13, color: t.nInk2)),
          const SizedBox(height: 14),
          if (working) ...[
            LinearProgressIndicator(
                value: p.state == 'queued' ? null : p.progress,
                color: kStudio),
            const SizedBox(height: 8),
            Row(children: [
              Expanded(
                child: Text(
                    p.state == 'queued'
                        ? 'Waiting for another project'
                        : '${(p.progress * 100).round()}% rendered',
                    style: TextStyle(fontSize: 12.5, color: t.nInk2)),
              ),
              TextButton(
                onPressed: () => c.send(StudioCmd.cancel(id: p.id)),
                child: const Text('Cancel'),
              ),
            ]),
          ] else
            FilledButton.icon(
              style: FilledButton.styleFrom(
                  backgroundColor: kStudio,
                  minimumSize: const Size.fromHeight(44)),
              icon: Icon(movie ? Icons.movie_outlined : Icons.picture_as_pdf,
                  size: 18),
              label: Text(p.state == 'done'
                  ? 'Render again'
                  : movie
                      ? 'Render the movie'
                      : 'Make the PDF'),
              onPressed: p.picks.length < 2 || !st.ready
                  ? null
                  : () => c.send(StudioCmd.render(id: p.id)),
            ),
          if (p.state == 'done') ...[
            const SizedBox(height: 10),
            Row(children: [
              Expanded(
                child: OutlinedButton.icon(
                  icon: const Icon(Icons.play_circle_outline, size: 18),
                  label: const Text('Open'),
                  onPressed: () =>
                      c.send(StudioCmd.showFile(id: p.id, folder: false)),
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: OutlinedButton.icon(
                  icon: const Icon(Icons.folder_open_outlined, size: 18),
                  label: const Text('Folder'),
                  onPressed: () =>
                      c.send(StudioCmd.showFile(id: p.id, folder: true)),
                ),
              ),
            ]),
          ],
          if (p.state == 'failed' && p.error.isNotEmpty) ...[
            const SizedBox(height: 10),
            Text(p.error,
                style: const TextStyle(fontSize: 12.5, color: Tokens.error)),
          ],
        ],
      ),
    );
    final picks = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(children: [
          Expanded(
            child: Text(
                '${plural(p.picks.length, 'photo')} picked of ${p.available} · '
                'sharp moments, no repeats, every day covered',
                style: TextStyle(fontSize: 12.5, color: t.nInk2)),
          ),
          TextButton.icon(
            icon: const Icon(Icons.refresh, size: 16),
            label: const Text('Pick again'),
            onPressed:
                working ? null : () => c.send(StudioCmd.repick(id: p.id)),
          ),
        ]),
        const SizedBox(height: 8),
        Wrap(spacing: 6, runSpacing: 6, children: [
          for (final id in p.picks)
            Stack(children: [
              PhotoThumb(id: id.toInt(), size: 96),
              if (!working)
                Positioned(
                  top: 3,
                  right: 3,
                  child: InkWell(
                    onTap: () => c.send(
                        StudioCmd.removePick(id: p.id, item: id.toInt()),
                        quiet: true),
                    child: Container(
                      padding: const EdgeInsets.all(2),
                      decoration: const BoxDecoration(
                          color: Colors.black54, shape: BoxShape.circle),
                      child:
                          const Icon(Icons.close, size: 14, color: Colors.white),
                    ),
                  ),
                ),
            ]),
        ]),
      ],
    );
    return LayoutBuilder(builder: (context, box) {
      final wide = box.maxWidth > 900;
      return ListView(
        padding: const EdgeInsets.fromLTRB(24, 16, 24, 30),
        children: [
          Row(children: [
            TextButton.icon(
              icon: const Icon(Icons.arrow_back, size: 17),
              label: const Text('Back'),
              onPressed: () => c.send(const StudioCmd.close()),
            ),
          ]),
          InkWell(
            onTap: () => _rename(context),
            child: Text(p.title,
                style: TextStyle(
                    fontSize: 26,
                    fontWeight: FontWeight.w800,
                    letterSpacing: -0.4,
                    color: t.nInk)),
          ),
          const SizedBox(height: 4),
          Text(
              [
                movie ? 'Movie' : 'Photo book',
                p.sourceLabel,
                if (p.musicLabel.isNotEmpty && movie) p.musicLabel,
              ].join(' · '),
              style: TextStyle(fontSize: 13, color: t.nInk2)),
          const SizedBox(height: 18),
          if (wide)
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(child: picks),
                const SizedBox(width: 18),
                SizedBox(width: 320, child: side),
              ],
            )
          else ...[
            side,
            const SizedBox(height: 16),
            picks,
          ],
        ],
      );
    });
  }
}
