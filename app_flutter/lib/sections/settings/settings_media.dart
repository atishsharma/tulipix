// Settings › Libraries, Playback, Services & Keys and AI Features, as tiles
// over the rows Rust already sends (docs/mockups/settings-tabs.html).
//
// Every write is the command it always was — a toggle, a text, an action —
// under the same key. The tiles regroup the rows; they do not rename them.

import 'dart:io';
import 'dart:math' as math;

import 'package:desktop_drop/desktop_drop.dart';
import 'package:flutter/material.dart';

import '../../design/pick.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/scrobble.dart';
import '../../src/rust/api/settings.dart';
import '../../src/rust/api/status.dart';
import 'settings_controller.dart';
import 'settings_kit.dart';

String _cap(String s) => s.isEmpty ? s : s[0].toUpperCase() + s.substring(1);

/// 12,304.
String _grouped(int n) {
  final s = n.abs().toString();
  final b = StringBuffer(n < 0 ? '-' : '');
  for (var i = 0; i < s.length; i++) {
    if (i > 0 && (s.length - i) % 3 == 0) b.write(',');
    b.write(s[i]);
  }
  return b.toString();
}

/// "just now", "12 min ago", "3 h ago", "yesterday", "5 days ago".
String _since(int secs) {
  final d = DateTime.now()
      .difference(DateTime.fromMillisecondsSinceEpoch(secs * 1000));
  if (d.inMinutes < 1) return 'just now';
  if (d.inHours < 1) return '${d.inMinutes} min ago';
  if (d.inDays < 1) return '${d.inHours} h ago';
  if (d.inDays == 1) return 'yesterday';
  return '${d.inDays} days ago';
}

/// A small label on a quiet ground: a section under a folder, a measurement.
class _Tag extends StatelessWidget {
  const _Tag(this.text, {this.tint});

  final String text;

  /// A square of colour before the words.
  final Color? tint;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final k = tint;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 5),
      decoration: BoxDecoration(
        color: t.bg,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: t.outline),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (k != null) ...[
            Container(
              width: 8,
              height: 8,
              decoration: BoxDecoration(
                  color: k, borderRadius: BorderRadius.circular(2)),
            ),
            const SizedBox(width: 7),
          ],
          Text(text,
              style: TextStyle(
                  fontSize: 11.5,
                  fontWeight: k != null ? FontWeight.w600 : FontWeight.w500,
                  color: k != null ? t.text : t.textDim)),
        ],
      ),
    );
  }
}

// ── libraries ───────────────────────────────────────────────────────────────

/// The five places a music folder can go, by the key the shared
/// `music_folder_sections.json` stores.
const Map<String, String> _musicShelves = {
  'mymusic': 'My Music',
  'podcasts': 'Podcasts',
  'audiobooks': 'Audiobooks',
  'radio': 'Radio',
  'youtube': 'YouTube',
};

Color _sectionTint(String name) {
  final n = name.toLowerCase();
  if (n.contains('photo')) return Tokens.secPhotos;
  if (n.contains('video')) return Tokens.secVideos;
  if (n.contains('music')) return Tokens.secMusic;
  if (n.contains('book')) return Tokens.secBooks;
  return Tokens.secSettings;
}

class LibrariesTab extends StatefulWidget {
  const LibrariesTab({super.key, required this.controller, required this.state});

  final SettingsController controller;
  final SettingsState state;

  @override
  State<LibrariesTab> createState() => _LibrariesTabState();
}

class _LibrariesTabState extends State<LibrariesTab> {
  /// The Status snapshot, for the counts and for what each folder holds. Read
  /// once on arriving — it queries every section's database — and again after
  /// anything changes the list.
  StatusState? _lib;
  bool _scanning = false;
  bool _over = false;

  /// A folder was added while a scan ran: that scan read the list when it
  /// began, so go round once more when it ends.
  bool _again = false;

  /// Which ten folders are showing.
  int _page = 0;
  static const int _perPage = 10;

  /// The long job Settings is running, by key -- `lib-rescan:<path>`,
  /// `rebuild-search`, `rescan-all` -- or empty.
  String get _job => widget.state.taskKey;

  SettingsController get _c => widget.controller;

  @override
  void initState() {
    super.initState();
    _load();
    // What each folder holds, counted on arriving. Not straight: sending
    // notifies, and initState runs inside a build.
    WidgetsBinding.instance
        .addPostFrameCallback((_) => _c.sendAction('lib-counts'));
  }

  @override
  void didUpdateWidget(LibrariesTab old) {
    super.didUpdateWidget(old);
    // A job just ended, so what the folders hold has changed.
    if (old.state.taskKey.isNotEmpty && widget.state.taskKey.isEmpty) _load();
  }

  Future<void> _load() async {
    try {
      final s = await statusDispatch(cmd: const StatusCmd.refresh());
      if (mounted) setState(() => _lib = s);
    } catch (_) {
      // The figures are extra; the folders themselves come from Settings.
    }
  }

  /// Every section over every watched folder. The call returns when the last
  /// scan does — minutes, on a big library — so the button says so until then.
  Future<void> _rescan({String? why}) async {
    if (_scanning) {
      _again = true;
      return;
    }
    setState(() => _scanning = true);
    _c.say(why ??
        'Rescanning every watched folder. You can keep using the app.');
    try {
      final s = await statusDispatch(cmd: const StatusCmd.rescan());
      if (!mounted) return;
      setState(() => _lib = s);
      _c.say('Rescan finished.');
    } catch (e) {
      _c.say('The rescan stopped: $e');
    } finally {
      if (mounted) setState(() => _scanning = false);
      if (mounted && _again) {
        _again = false;
        _rescan();
      }
    }
  }

  /// Watch [path] and read it at once: nobody adds a folder to then press
  /// Rescan. Shows the last page, where the new folder is.
  Future<void> _add(String path) async {
    final before = _c.state?.libraries.length ?? 0;
    await _c.send(SettingsCmd.libAdd(path: path));
    await _load();
    final after = _c.state?.libraries.length ?? 0;
    if (!mounted || after <= before) return;
    setState(() => _page = (after - 1) ~/ _perPage);
    _rescan(
        why: 'Watching ${tildePath(path)} and reading it now. You can keep '
            'using the app.');
  }

  /// `lib-rescan` or `lib-thumbs` on one folder. A rescan runs detached in
  /// Rust; the bar follows it, and the notice after says how it went.
  Future<void> _folderAction(String action, String path) =>
      _c.sendAction('$action:$path');

  Future<void> _reindex() => _c.sendAction('rebuild-search');

  Future<void> _pick() async {
    final path = await pickDirectory();
    if (path != null && path.trim().isNotEmpty) await _add(path.trim());
  }

  Future<void> _drop(DropDoneDetails d) async {
    setState(() => _over = false);
    final dirs = [
      for (final f in d.files)
        if (FileSystemEntity.isDirectorySync(f.path)) f.path,
    ];
    if (dirs.isEmpty) {
      _c.say('Drop a folder — files are found by watching the folder they are in.');
      return;
    }
    for (final p in dirs) {
      await _add(p);
    }
  }

  List<String> _sectionsOf(String path) {
    for (final r in _lib?.roots ?? const <StRoot>[]) {
      if (r.path == path) return r.sections;
    }
    return const [];
  }

  @override
  Widget build(BuildContext context) {
    final libs = widget.state.libraries;
    final missing = libs.where((l) => !l.exists).length;
    final lib = _lib;
    final pages = math.max(1, (libs.length / _perPage).ceil());
    // Clamped here rather than when a folder goes: the list shrinks under it.
    final page = _page.clamp(0, pages - 1);
    final shown = libs.skip(page * _perPage).take(_perPage).toList();
    return SettingsPageBody(
      head: SettingsHead.forTab(
        'libraries',
        note: 'The only folders Tulipix reads. Nothing inside them is ever '
            'moved or rewritten',
        actions: [
          SmallBtn(
            label: _job == 'rebuild-search' ? 'Rebuilding…' : 'Rebuild search',
            icon: Icons.manage_search,
            large: true,
            busy: _job == 'rebuild-search',
            tooltip: 'Index every photo again, so search finds what changed',
            onTap: libs.isEmpty || _job.isNotEmpty ? null : _reindex,
          ),
          SmallBtn(
            label: _scanning ? 'Rescanning…' : 'Rescan all',
            icon: Icons.refresh,
            large: true,
            busy: _scanning,
            onTap: libs.isEmpty ? null : _rescan,
          ),
          SmallBtn(
            label: 'Watch a folder',
            icon: Icons.add,
            large: true,
            primary: true,
            onTap: _pick,
          ),
        ],
      ),
      children: [
        StatStrip([
          (
            label: 'Watched',
            value: '${libs.length} ${libs.length == 1 ? 'folder' : 'folders'}',
            note: missing == 0
                ? 'All on disk'
                : '${libs.length - missing} on disk, $missing missing',
          ),
          (
            label: 'Items',
            value: lib?.libItems ?? '—',
            note: lib == null ? 'Counting…' : '+${lib.libraryWeek} this week',
          ),
          (
            label: 'On disk',
            value: lib?.libBytes ?? '—',
            note: lib == null ? '' : 'Across ${lib.libSections.length} sections',
          ),
          (
            label: 'Scans',
            value: _scanning || (lib?.scanRunning ?? false) ? 'Running' : 'Idle',
            note: lib == null ? '' : '${lib.libDatabases} databases',
          ),
        ]),
        // What is running, named, with how far it has got: the Slint
        // build's maintenance bar. A whole rescan is started from here and
        // waited on, so it is named here too.
        if (_scanning || _job.isNotEmpty)
          _JobStrip(
            label: _job.isEmpty
                ? 'Rescanning every watched folder'
                : widget.state.taskLabel,
            frac: _job.isEmpty ? -1 : widget.state.taskFrac,
          ),
        TileGrid([
          (span: 6, child: _schedule()),
          for (final l in shown) (span: 3, child: _folder(l)),
          (span: shown.length.isOdd ? 3 : 6, child: _dropTile(libs.isEmpty)),
        ]),
        if (pages > 1)
          _Pager(
            page: page,
            pages: pages,
            total: libs.length,
            per: _perPage,
            onPage: (p) => setState(() => _page = p),
          ),
      ],
    );
  }

  /// The auto-rescan cadences, as `scan.cadence` stores them.
  static const Map<String, String> _cadences = {
    'manual': 'Never',
    'hourly': 'Hourly',
    'daily': 'Daily',
    'weekly': 'Weekly',
  };

  /// When folders are read again by themselves, and what every scan skips.
  Widget _schedule() {
    final st = widget.state;
    return SettingsTile(
      icon: Icons.schedule,
      tint: Tokens.warn,
      title: 'Automatic rescans',
      note: 'Each folder is read again this often, counted from when it was '
          'last read. A low battery holds it back',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Seg(
            full: true,
            options: _cadences.keys.toList(),
            labels: _cadences.values.toList(),
            value: st.defaultCadence,
            onPick: (v) =>
                _c.send(SettingsCmd.setText(key: 'scan.cadence', value: v)),
          ),
          SettingLine(
            title: 'Skip these',
            note: 'Patterns every scan passes over, comma-separated',
            below: true,
            trailing: FieldBox(
              value: st.exclusions,
              hint: '*/Private/*, *.part',
              onSubmit: (v) =>
                  _c.send(SettingsCmd.setText(key: 'scan.exclude', value: v)),
            ),
          ),
        ],
      ),
    );
  }

  /// "1,204 items · read 3 h ago". The count waits for the first count after
  /// the tab opens; the time for the first scan since it was kept.
  static String _folderLine(LibraryRow l) {
    final n = l.items.toInt();
    final last = l.lastScan.toInt();
    return [
      if (n >= 0) '${_grouped(n)} ${n == 1 ? 'item' : 'items'}',
      last > 0 ? 'read ${_since(last)}' : 'last read not recorded yet',
    ].join(' · ');
  }

  Widget _folder(LibraryRow l) {
    final t = context.tokens;
    final secs = _sectionsOf(l.path);
    // The shelf picker shows for a folder that holds music, or one already
    // moved off My Music.
    final music = l.exists &&
        (secs.any((s) => s.toLowerCase().contains('music')) ||
            l.musicSection != 'mymusic');
    final busy = _job == 'lib-rescan:${l.path}';
    return SettingsTile(
      icon: l.exists ? Icons.folder_outlined : Icons.folder_off_outlined,
      tint: l.exists ? Tokens.warn : Tokens.error,
      title: tildePath(l.path),
      note: !l.exists
          ? 'Its items stay in the library, marked missing'
          : secs.isEmpty
              ? 'Nothing indexed from it yet. Rescan to read it'
              : 'Found by ${secs.length} '
                  '${secs.length == 1 ? 'section' : 'sections'}',
      danger: !l.exists,
      trailing: [
        StateChip(l.exists ? 'Watching' : 'Not on disk',
            tint: l.exists ? Tokens.ok : Tokens.error),
      ],
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (l.exists) ...[
            Text(_folderLine(l),
                style: TextStyle(fontSize: 11.5, color: t.textDim)),
            const SizedBox(height: 10),
          ],
          if (secs.isNotEmpty) ...[
            Wrap(
              spacing: 6,
              runSpacing: 6,
              children: [for (final s in secs) _Tag(s, tint: _sectionTint(s))],
            ),
            const SizedBox(height: 12),
          ],
          if (!l.exists) ...[
            Text('Plug the drive back in and Tulipix picks it up again.',
                style: TextStyle(fontSize: 11.5, color: t.textDim)),
            const SizedBox(height: 12),
          ],
          if (music) ...[
            Text('Its music goes to',
                style: TextStyle(fontSize: 11.5, color: t.textDim)),
            const SizedBox(height: 6),
            Seg(
              full: true,
              options: _musicShelves.keys.toList(),
              labels: _musicShelves.values.toList(),
              value: l.musicSection,
              onPick: (v) => _c.sendAction('lib-section:$v:${l.path}'),
            ),
            const SizedBox(height: 12),
          ],
          if (l.exists) ...[
            Text('Read again automatically',
                style: TextStyle(fontSize: 11.5, color: t.textDim)),
            const SizedBox(height: 6),
            Seg(
              full: true,
              options: ['', ..._cadences.keys],
              labels: ['Default', ..._cadences.values],
              value: l.cadence,
              onPick: (v) => _c.send(
                  SettingsCmd.setText(key: 'lib.cadence.${l.path}', value: v)),
            ),
            const SizedBox(height: 12),
          ],
          Row(
            children: [
              if (l.exists) ...[
                SmallBtn(
                  label: 'Open',
                  icon: Icons.open_in_new,
                  onTap: () => _c.sendAction('lib-open:${l.path}'),
                ),
                const SizedBox(width: 6),
                SmallBtn(
                  label: busy ? 'Reading…' : 'Rescan',
                  busy: busy,
                  tooltip: 'Read this folder again',
                  onTap: _job.isNotEmpty
                      ? null
                      : () => _folderAction('lib-rescan', l.path),
                ),
                const SizedBox(width: 6),
                SmallBtn(
                  label: 'Thumbnails',
                  tooltip: 'Redraw this folder\'s thumbnails',
                  onTap: busy ? null : () => _folderAction('lib-thumbs', l.path),
                ),
              ] else
                SmallBtn(
                  label: 'Check again',
                  icon: Icons.refresh,
                  onTap: () async {
                    await _c.refresh();
                    await _load();
                  },
                ),
              const Spacer(),
              SmallBtn(
                label: 'Stop watching',
                ghost: true,
                tooltip: 'Nothing indexed from it is deleted',
                onTap: () async {
                  await _c.send(SettingsCmd.libRemove(path: l.path));
                  await _load();
                },
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _dropTile(bool empty) {
    final t = context.tokens;
    return DropTarget(
      onDragEntered: (_) => setState(() => _over = true),
      onDragExited: (_) => setState(() => _over = false),
      onDragDone: _drop,
      child: Container(
        constraints: const BoxConstraints(minHeight: 150),
        alignment: Alignment.center,
        padding: const EdgeInsets.all(18),
        decoration: BoxDecoration(
          color: _over ? Tokens.warn.withValues(alpha: 0.08) : null,
          borderRadius: BorderRadius.circular(18),
          border: Border.all(
              color: _over ? Tokens.warn : t.outlineStrong, width: 1.5),
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(Icons.create_new_folder_outlined,
                size: 26, color: Tokens.warn),
            const SizedBox(height: 8),
            Text(empty ? 'No folders yet' : 'Drop a folder here',
                style: TextStyle(
                    fontSize: 13.5,
                    fontWeight: FontWeight.w700,
                    color: t.text)),
            const SizedBox(height: 2),
            Text(
                empty
                    ? 'Drop one here, or use Watch a folder above'
                    : 'or use Watch a folder above',
                textAlign: TextAlign.center,
                style: TextStyle(fontSize: 12, color: t.textDim)),
          ],
        ),
      ),
    );
  }
}

/// Which ten folders: a count, Previous and Next, and the page numbers while
/// there are few enough to show.
class _Pager extends StatelessWidget {
  const _Pager({
    required this.page,
    required this.pages,
    required this.total,
    required this.per,
    required this.onPage,
  });

  final int page;
  final int pages;
  final int total;
  final int per;
  final ValueChanged<int> onPage;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final from = page * per + 1;
    final to = math.min((page + 1) * per, total);
    return Row(
      children: [
        Text('$from–$to of $total folders',
            style: TextStyle(fontSize: 12, color: t.textDim)),
        const Spacer(),
        SmallBtn(
          label: '‹ Previous',
          onTap: page > 0 ? () => onPage(page - 1) : null,
        ),
        if (pages <= 7)
          for (var p = 0; p < pages; p++) ...[
            const SizedBox(width: 6),
            SmallBtn(
              label: '${p + 1}',
              primary: p == page,
              onTap: p == page ? null : () => onPage(p),
            ),
          ]
        else ...[
          const SizedBox(width: 10),
          Text('Page ${page + 1} of $pages',
              style: TextStyle(fontSize: 12, color: t.text)),
        ],
        const SizedBox(width: 6),
        SmallBtn(
          label: 'Next ›',
          onTap: page < pages - 1 ? () => onPage(page + 1) : null,
        ),
      ],
    );
  }
}

/// The running job: what it is, and a bar -- filled to the fraction when
/// there is one, sweeping when there is not.
class _JobStrip extends StatelessWidget {
  const _JobStrip({required this.label, required this.frac});

  final String label;
  final double frac;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final known = frac >= 0;
    return Container(
      padding: const EdgeInsets.fromLTRB(16, 12, 16, 14),
      decoration: BoxDecoration(
        color: t.panel2,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: Tokens.brand.withValues(alpha: 0.35)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(label,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 12.5,
                        fontWeight: FontWeight.w600,
                        color: t.text)),
              ),
              Text(known ? '${(frac.clamp(0.0, 1.0) * 100).floor()}%' : 'Working…',
                  style: TextStyle(
                      fontSize: 11.5,
                      color: t.textDim,
                      fontFeatures: const [FontFeature.tabularFigures()])),
            ],
          ),
          const SizedBox(height: 8),
          ClipRRect(
            borderRadius: BorderRadius.circular(3),
            child: LinearProgressIndicator(
              value: known ? frac.clamp(0.0, 1.0) : null,
              minHeight: 6,
              backgroundColor: t.outline,
              color: Tokens.brand,
            ),
          ),
        ],
      ),
    );
  }
}

// ── playback ────────────────────────────────────────────────────────────────

/// The ten bands of each preset, from `tulipix_music::eq::Equalizer::preset`.
/// Drawn, not applied: the player reads the preset name.
const Map<String, List<double>> _gains = {
  'flat': [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
  'rock': [4, 3, 1.5, 0, -1, -1, 1.5, 3, 4, 4.5],
  'pop': [-1, 0, 2, 3, 3.5, 2.5, 0, -1, -1, -1.5],
  'jazz': [3, 2, 1, 2, -1, -1, 0, 1, 2.5, 3],
  'bass': [6, 5, 4, 2, 0, 0, 0, 0, 0, 0],
  'treble': [0, 0, 0, 0, 0, 1, 2, 4, 5.5, 6],
};

const List<(String, String)> _swatches = [
  ('#ffffff', 'White'),
  ('#fde047', 'Yellow'),
  ('#67e8f9', 'Cyan'),
  ('#f9a8d4', 'Pink'),
];

Color? _hex(String s) {
  final h = s.trim().replaceFirst('#', '');
  if (h.length != 6) return null;
  final v = int.tryParse(h, radix: 16);
  return v == null ? null : Color(0xFF000000 | v);
}

class PlaybackTab extends StatelessWidget {
  const PlaybackTab({super.key, required this.controller, required this.state});

  final SettingsController controller;
  final SettingsState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = controller;
    final rows = Rows(state.playback);
    final sizeRow = rows.key('playback.sub-size');
    final colourRow = rows.key('playback.sub-color');
    final sidecar = rows.label('Subtitles next to the video');
    final interp = rows.key('playback.interpolation');
    final upscale = rows.key('playback.upscale');
    final awake = rows.label('Keep display awake');
    final shaders = rows.label('Upscale shader folder');
    final exclusive = rows.key('playback.audio-exclusive');
    final eqRow = rows.key('music.eq-preset');
    final autoplay = rows.key('music.autoplay');
    rows.use(['music-analyse', 'music-analyse-stop']);
    final rest = rows.rest;

    final size = int.tryParse(sizeRow?.value.trim() ?? '') ?? 28;
    final typed = colourRow?.value.trim().toLowerCase() ?? '';
    final colour = typed.isEmpty ? '#ffffff' : typed;
    final eqNow = eqRow?.value.trim().toLowerCase() ?? '';
    final eq = _gains.containsKey(eqNow) ? eqNow : 'flat';
    void text(String key, String v) =>
        c.send(SettingsCmd.setText(key: key, value: v));

    return SettingsPageBody(
      head: SettingsHead.forTab('playback',
          note: 'How films look, subtitles read, and music sounds'),
      children: [
        TileGrid([
          (
            span: 3,
            child: SettingsTile(
              icon: Icons.closed_caption_outlined,
              tint: const Color(0xFF0EA5E9),
              title: 'Subtitles',
              note: 'Size and colour, previewed here',
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  _SubPreview(size: size, colour: _hex(colour) ?? Colors.white),
                  const SizedBox(height: 6),
                  Lines([
                    if (sizeRow != null)
                      SettingLine(
                        title: 'Size',
                        note: 'In pixels on a 1080p picture',
                        trailing: _Stepper(
                          value: size,
                          onChanged: (v) => text('playback.sub-size', '$v'),
                        ),
                      ),
                    if (colourRow != null)
                      SettingLine(
                        title: 'Colour',
                        note: 'Applies on the next play',
                        trailing: _Swatches(
                          value: colour,
                          onPick: (v) => text('playback.sub-color', v),
                        ),
                      ),
                    if (sidecar != null)
                      SettingLine(
                        title: sidecar.label,
                        note: sidecar.value,
                        trailing: const StateChip('Always', tint: Tokens.ok),
                      ),
                  ]),
                ],
              ),
            ),
          ),
          (
            span: 3,
            child: SettingsTile(
              icon: Icons.movie_outlined,
              tint: Tokens.secVideos,
              title: 'Video',
              note: 'Motion and sharpness',
              // The picture above the switches takes whatever height the
              // Subtitles tile beside it leaves, so the two end together.
              fill: true,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Expanded(
                    child: _VideoPreview(
                      smooth: interp?.on_ ?? false,
                      sharp: upscale?.on_ ?? false,
                    ),
                  ),
                  const SizedBox(height: 6),
                  Lines([
                    if (interp != null) RowLine(row: interp, controller: c),
                    if (upscale != null) RowLine(row: upscale, controller: c),
                    SettingLine(
                      title: 'Shader folder',
                      note: shaders?.value ?? 'Where the upscale .glsl files go',
                      trailing: SmallBtn(
                        label: 'Open',
                        icon: Icons.open_in_new,
                        onTap: () => c.sendAction('open-shaders'),
                      ),
                    ),
                    if (awake != null)
                      SettingLine(
                        title: awake.label,
                        note: awake.value,
                        trailing: const StateChip('Always', tint: Tokens.ok),
                      ),
                  ]),
                ],
              ),
            ),
          ),
          (
            span: 3,
            child: SettingsTile(
              icon: Icons.graphic_eq,
              tint: Tokens.secMusic,
              title: 'Audio & music',
              note: 'Output, the equaliser, and what plays next',
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  if (eqRow != null) ...[
                    Container(
                      height: 64,
                      decoration: BoxDecoration(
                        color: t.bg,
                        borderRadius: BorderRadius.circular(12),
                        border: Border.all(color: t.outline),
                      ),
                      clipBehavior: Clip.antiAlias,
                      child: CustomPaint(painter: _EqCurve(eq, t.outlineStrong)),
                    ),
                    const SizedBox(height: 10),
                    Seg(
                      full: true,
                      options: _gains.keys.toList(),
                      labels: [for (final p in _gains.keys) _cap(p)],
                      value: eq,
                      onPick: (v) => text('music.eq-preset', v),
                    ),
                    const SizedBox(height: 4),
                  ],
                  Lines([
                    if (exclusive != null)
                      RowLine(row: exclusive, controller: c),
                    if (autoplay != null) RowLine(row: autoplay, controller: c),
                  ]),
                ],
              ),
            ),
          ),
          (span: 3, child: _AnalysisTile(controller: c, state: state)),
        ]),
        if (rest.isNotEmpty) MoreTile(rows: rest, controller: c),
      ],
    );
  }
}

/// A frame of film with a line of subtitle on it, at the size and colour set.
/// Half scale: the real thing is sized for a 1080p picture.
class _SubPreview extends StatelessWidget {
  const _SubPreview({required this.size, required this.colour});

  final int size;
  final Color colour;

  @override
  Widget build(BuildContext context) => Container(
        height: 118,
        alignment: Alignment.bottomCenter,
        padding: const EdgeInsets.fromLTRB(12, 0, 12, 12),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(12),
          border: Border.all(color: context.tokens.outline),
          gradient: const LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [Color(0xFF0F172A), Color(0xFF1E293B), Color(0xFF334155)],
          ),
        ),
        child: Text(
          'Where were you when the lights went out?',
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          textAlign: TextAlign.center,
          style: TextStyle(
            color: colour,
            fontWeight: FontWeight.w600,
            fontSize: (size * 0.5).clamp(8.0, 32.0),
            shadows: const [Shadow(blurRadius: 3), Shadow(blurRadius: 6)],
          ),
        ),
      );
}

/// The two video switches said on a frame: motion and sharpness, as they are
/// set now. The same ground as the subtitle preview beside it.
class _VideoPreview extends StatelessWidget {
  const _VideoPreview({required this.smooth, required this.sharp});

  final bool smooth;
  final bool sharp;

  static Widget _fig(IconData icon, String label, String value, String note,
          bool on) =>
      Column(
        mainAxisAlignment: MainAxisAlignment.center,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Icon(icon,
              size: 18,
              color: on ? const Color(0xFF7DD3FC) : const Color(0x99FFFFFF)),
          const SizedBox(height: 8),
          Text(label,
              style: const TextStyle(
                  fontSize: 10,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 0.8,
                  color: Color(0x99FFFFFF))),
          Text(value,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                  fontSize: 17,
                  fontWeight: FontWeight.w700,
                  color: Colors.white)),
          Text(note,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(fontSize: 11, color: Color(0xB3FFFFFF))),
        ],
      );

  @override
  Widget build(BuildContext context) => Container(
        constraints: const BoxConstraints(minHeight: 118),
        padding: const EdgeInsets.fromLTRB(14, 12, 14, 12),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(12),
          border: Border.all(color: context.tokens.outline),
          gradient: const LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [Color(0xFF0F172A), Color(0xFF1E293B), Color(0xFF334155)],
          ),
        ),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(
              child: _fig(
                Icons.animation,
                'MOTION',
                smooth ? 'Smoothed' : 'As filmed',
                smooth ? 'Frames made in between' : 'Each frame as shot',
                smooth,
              ),
            ),
            Container(
              width: 1,
              margin: const EdgeInsets.symmetric(horizontal: 14),
              color: const Color(0x26FFFFFF),
            ),
            Expanded(
              child: _fig(
                Icons.hd_outlined,
                'SHARPNESS',
                sharp ? 'Upscaled' : 'Native',
                sharp ? 'Sharpened past its size' : 'At its own resolution',
                sharp,
              ),
            ),
          ],
        ),
      );
}

class _Stepper extends StatelessWidget {
  const _Stepper({required this.value, required this.onChanged});

  final int value;
  final ValueChanged<int> onChanged;

  static const int _min = 16;
  static const int _max = 64;
  static const int _step = 2;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget button(String glyph, String tip, int? to) => Tooltip(
          message: tip,
          child: InkWell(
            onTap: to == null ? null : () => onChanged(to),
            child: SizedBox(
              width: 30,
              height: 30,
              child: Center(
                child: Text(glyph,
                    style: TextStyle(
                        fontSize: 16,
                        color: to == null
                            ? t.textDim.withValues(alpha: 0.4)
                            : t.textDim)),
              ),
            ),
          ),
        );
    return Container(
      decoration: BoxDecoration(
        color: t.bg,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: t.outlineStrong),
      ),
      clipBehavior: Clip.antiAlias,
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          button('−', 'Smaller',
              value > _min ? math.max(_min, value - _step) : null),
          SizedBox(
            width: 56,
            child: Text('$value px',
                textAlign: TextAlign.center,
                style: TextStyle(
                    fontSize: 12, fontWeight: FontWeight.w600, color: t.text)),
          ),
          button('+', 'Larger',
              value < _max ? math.min(_max, value + _step) : null),
        ],
      ),
    );
  }
}

class _Swatches extends StatelessWidget {
  const _Swatches({required this.value, required this.onPick});

  final String value;
  final ValueChanged<String> onPick;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final custom = !_swatches.any((s) => s.$1 == value);
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final (hex, name) in _swatches)
          Padding(
            padding: const EdgeInsets.only(left: 6),
            child: Tooltip(
              message: name,
              child: InkWell(
                borderRadius: BorderRadius.circular(8),
                onTap: () => onPick(hex),
                child: Container(
                  width: 26,
                  height: 26,
                  decoration: BoxDecoration(
                    color: _hex(hex),
                    borderRadius: BorderRadius.circular(8),
                    border: Border.all(
                      color: hex == value ? Tokens.brand : t.outlineStrong,
                      width: hex == value ? 2.5 : 1,
                    ),
                  ),
                ),
              ),
            ),
          ),
        // A colour typed into the old text box is kept, and said.
        if (custom)
          Padding(
            padding: const EdgeInsets.only(left: 8),
            child: Text(value,
                style: TextStyle(fontSize: 11.5, color: t.textDim)),
          ),
      ],
    );
  }
}

class _EqCurve extends CustomPainter {
  const _EqCurve(this.preset, this.grid);

  final String preset;
  final Color grid;

  @override
  void paint(Canvas canvas, Size size) {
    final g = _gains[preset] ?? _gains['flat']!;
    final mid = size.height / 2;
    // ±6 dB, the largest any preset reaches, fills the box.
    final k = (size.height / 2 - 6) / 6;
    final pts = [
      for (var i = 0; i < g.length; i++)
        Offset(i / (g.length - 1) * size.width, mid - g[i] * k),
    ];
    canvas.drawLine(Offset(0, mid), Offset(size.width, mid),
        Paint()..color = grid);
    final line = Path()..moveTo(pts[0].dx, pts[0].dy);
    for (var i = 1; i < pts.length; i++) {
      final a = pts[i - 1];
      final b = pts[i];
      final mx = (a.dx + b.dx) / 2;
      line.cubicTo(mx, a.dy, mx, b.dy, b.dx, b.dy);
    }
    final area = Path.from(line)
      ..lineTo(size.width, size.height)
      ..lineTo(0, size.height)
      ..close();
    canvas.drawPath(
      area,
      Paint()
        ..shader = LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [
            Tokens.secMusic.withValues(alpha: 0.35),
            Tokens.secMusic.withValues(alpha: 0),
          ],
        ).createShader(Offset.zero & size),
    );
    canvas.drawPath(
      line,
      Paint()
        ..color = Tokens.secMusic
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2,
    );
  }

  @override
  bool shouldRepaint(_EqCurve o) => o.preset != preset || o.grid != grid;
}

class _AnalysisTile extends StatelessWidget {
  const _AnalysisTile({required this.controller, required this.state});

  final SettingsController controller;
  final SettingsState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final total = state.analysable.toInt();
    final done = state.analysed.toInt();
    final running = state.analysing;
    final frac = total == 0 ? 0.0 : done / total;
    // Floored, so the last track or two do not read as 100%.
    final pct = (frac * 100).floor();
    return SettingsTile(
      icon: Icons.speed_outlined,
      tint: Tokens.secPhotos,
      title: 'Library analysis',
      note: 'What song matching, duplicates and keep-playing read',
      trailing: [
        if (running) const StateChip('Measuring', tint: Tokens.brand),
      ],
      // The ring takes the tile's height — whatever Audio & music beside it
      // asks for — up to its column's width.
      fill: true,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
              SizedBox(
                width: 170,
                child: CustomPaint(
                  painter: _Ring(frac, t.outline),
                  child: Center(
                    child: Column(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Text(total == 0 ? '—' : '$pct%',
                            style: TextStyle(
                                fontSize: 28,
                                fontWeight: FontWeight.w800,
                                color: t.text,
                                fontFeatures: const [
                                  FontFeature.tabularFigures()
                                ])),
                        if (total > 0)
                          Text('measured',
                              style: TextStyle(
                                  fontSize: 11, color: t.textDim)),
                      ],
                    ),
                  ),
                ),
              ),
              const SizedBox(width: 18),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    Text(
                        total == 0
                            ? 'No tracks to measure yet'
                            : '${_grouped(done)} of ${_grouped(total)} tracks '
                                'measured',
                        style: TextStyle(
                            fontSize: 14,
                            fontWeight: FontWeight.w700,
                            color: t.text)),
                    const SizedBox(height: 3),
                    Text(
                        'Reads each track once, in the background. Measured '
                        'tracks are skipped next time',
                        style: TextStyle(fontSize: 11.5, color: t.textDim)),
                    const SizedBox(height: 10),
                    Wrap(
                      spacing: 8,
                      runSpacing: 8,
                      children: [
                        SmallBtn(
                          label: running ? 'Measuring…' : 'Analyse the rest',
                          primary: true,
                          busy: running,
                          onTap: done >= total
                              ? null
                              : () => controller.sendAction('music-analyse'),
                        ),
                        SmallBtn(
                          label: 'Stop',
                          onTap: running
                              ? () => controller.sendAction('music-analyse-stop')
                              : null,
                        ),
                      ],
                    ),
                    const SizedBox(height: 12),
                    const Wrap(
                      spacing: 6,
                      runSpacing: 6,
                      children: [
                        _Tag('Tempo (BPM)'),
                        _Tag('Musical key'),
                        _Tag('Dynamic range'),
                        _Tag('What it sounds like'),
                      ],
                    ),
                  ],
                ),
              ),
        ],
      ),
    );
  }
}

/// A circle, not the box's ellipse: as wide as the box is tall, or as tall
/// as it is wide, whichever is less, and centred in it.
class _Ring extends CustomPainter {
  const _Ring(this.frac, this.track);

  final double frac;
  final Color track;

  @override
  void paint(Canvas canvas, Size size) {
    const w = 12.0;
    final side = math.min(size.width, size.height);
    if (side <= w) return;
    final r = Rect.fromCircle(
        center: size.center(Offset.zero), radius: side / 2 - w / 2);
    final p = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = w;
    canvas.drawArc(r, 0, math.pi * 2, false, p..color = track);
    if (frac > 0) {
      canvas.drawArc(r, -math.pi / 2, math.pi * 2 * frac.clamp(0.0, 1.0),
          false,
          p
            ..color = Tokens.brand
            ..strokeCap = StrokeCap.round);
    }
  }

  @override
  bool shouldRepaint(_Ring o) => o.frac != frac || o.track != track;
}

// ── services & keys ─────────────────────────────────────────────────────────

class ServicesTab extends StatefulWidget {
  const ServicesTab({super.key, required this.controller, required this.state});

  final SettingsController controller;
  final SettingsState state;

  @override
  State<ServicesTab> createState() => _ServicesTabState();
}

class _ServicesTabState extends State<ServicesTab> {
  /// The self-hosted servers fold away: almost nobody needs them, and six
  /// empty boxes pushed everything else down.
  bool _servers = false;

  /// Listens waiting for ListenBrainz. Null until the count lands.
  int? _queued;

  @override
  void initState() {
    super.initState();
    _countQueued();
  }

  /// Keys being tested, by service.
  final Set<String> _testing = {};

  /// Bumped on every save, so each key box comes back empty: a saved key is
  /// never shown again, only whether there is one.
  int _keyEpoch = 0;

  static const Map<String,
      ({IconData icon, Color tint, String note, (String, String) link})>
      _keyMeta = {
    'tmdb': (
      icon: Icons.movie_outlined,
      tint: Color(0xFF0EA5E9),
      note: 'Film and show artwork, cast and summaries',
      link: (
        'Free key from themoviedb.org',
        'https://www.themoviedb.org/settings/api'
      ),
    ),
    'tvdb': (
      icon: Icons.live_tv_outlined,
      tint: Color(0xFF22C55E),
      note: 'Series and episode details',
      link: ('Key from thetvdb.com', 'https://thetvdb.com/api-information'),
    ),
    'opensubtitles': (
      icon: Icons.closed_caption_outlined,
      tint: Color(0xFFF59E0B),
      note: 'Finds subtitles for your videos',
      link: (
        'Free key from opensubtitles.com',
        'https://www.opensubtitles.com/en/consumers'
      ),
    ),
    'lastfm': (
      icon: Icons.album_outlined,
      tint: Color(0xFFEF4444),
      note: 'Artist details and similar music',
      link: ('Key from last.fm', 'https://www.last.fm/api/account/create'),
    ),
    'libretranslate': (
      icon: Icons.translate,
      tint: Color(0xFF8B5CF6),
      note: 'Translates subtitles and lyrics',
      link: (
        'Key from your LibreTranslate server',
        'https://portal.libretranslate.com'
      ),
    ),
    // The service integrations. Each one is a secret, so each one is a
    // keychain row rather than a text box in settings.json.
    'discogs': (
      icon: Icons.library_music_outlined,
      tint: Color(0xFF111827),
      note: 'Artist biographies where MusicBrainz has no match',
      link: (
        'Personal token from discogs.com',
        'https://www.discogs.com/settings/developers'
      ),
    ),
    'spotify_id': (
      icon: Icons.graphic_eq,
      tint: Color(0xFF22C55E),
      note: 'Genres and artist pictures. Goes with the secret below',
      link: (
        'Client ID from developer.spotify.com',
        'https://developer.spotify.com/dashboard'
      ),
    ),
    'spotify_secret': (
      icon: Icons.graphic_eq,
      tint: Color(0xFF16A34A),
      note: 'The other half of the Spotify client ID',
      link: (
        'Client secret from the same app',
        'https://developer.spotify.com/dashboard'
      ),
    ),
    'youtube_data': (
      icon: Icons.smart_display_outlined,
      tint: Color(0xFFEF4444),
      note: 'View counts and real thumbnails on the YouTube tab',
      link: (
        'Key from the Google Cloud console',
        'https://console.cloud.google.com/apis/credentials'
      ),
    ),
    'trakt': (
      icon: Icons.check_circle_outline,
      tint: Color(0xFFDC2626),
      note: 'What you finish watching, sent to your Trakt history',
      link: (
        'Create an application at trakt.tv',
        'https://trakt.tv/oauth/applications'
      ),
    ),
    'trakt_secret': (
      icon: Icons.check_circle_outline,
      tint: Color(0xFFB91C1C),
      note: 'The other half of the Trakt client ID',
      link: (
        'Client secret from the same application',
        'https://trakt.tv/oauth/applications'
      ),
    ),
    'anidb': (
      icon: Icons.animation_outlined,
      tint: Color(0xFF7C3AED),
      note: 'Not a key: the client name you registered at anidb.net',
      link: (
        'Register a client at anidb.net',
        'https://anidb.net/software/add'
      ),
    ),
  };

  Future<void> _testKey(String service) async {
    setState(() => _testing.add(service));
    await _c.sendAction('key-test:$service');
    if (mounted) setState(() => _testing.remove(service));
  }

  /// One keychain key. The box is always empty: a saved key stays in the
  /// keychain, and pasting a new one replaces it.
  Widget _serviceKeyTile(ServiceKeyRow k) {
    final m = _keyMeta[k.service];
    final testing = _testing.contains(k.service);
    return SettingsTile(
      icon: m?.icon ?? Icons.key_outlined,
      tint: m?.tint ?? Tokens.brand,
      title: k.label,
      note: m?.note ?? '',
      trailing: [
        StateChip(k.saved ? 'Saved' : 'Not set',
            tint: k.saved ? Tokens.ok : null),
      ],
      fill: true,
      child: Spread(
        gap: 8,
        [
          FieldBox(
            key: ValueKey('key-${k.service}-$_keyEpoch'),
            value: '',
            secret: true,
            hint: k.saved ? 'Saved. Paste a new key to replace it' : 'Paste a key',
            onSubmit: (v) {
              setState(() => _keyEpoch++);
              _c.send(SettingsCmd.setText(key: 'key:${k.service}', value: v));
            },
          ),
          if (k.saved)
            Row(
              children: [
                SmallBtn(
                  label: testing ? 'Testing…' : 'Test',
                  busy: testing,
                  tooltip: 'Ask the service whether it takes this key',
                  onTap: testing ? null : () => _testKey(k.service),
                ),
                const SizedBox(width: 6),
                SmallBtn(
                  label: 'Remove',
                  ghost: true,
                  onTap: () => _c.sendAction('key-remove:${k.service}'),
                ),
              ],
            ),
          if (m != null) _linkLine(m.link),
        ],
      ),
    );
  }

  /// Trakt's sign-in row. One button, because there is only ever one thing to
  /// do next: Link, Cancel while the code is out, or Unlink once it is done.
  /// The bridge decides which; this draws what it says.
  Widget _traktTile(SettingItem r) {
    final t = context.tokens;
    final tint = switch (r.state) {
      'ok' => Tokens.ok,
      'busy' => Tokens.brand,
      'warn' => Tokens.warn,
      _ => null,
    };
    return SettingsTile(
      icon: Icons.check_circle_outline,
      tint: const Color(0xFFDC2626),
      title: 'Trakt account',
      note: 'What you finish watching, added to your history',
      trailing: [StateChip(r.state == 'ok' ? 'Linked' : 'Not linked', tint: tint)],
      fill: true,
      child: Spread(
        gap: 8,
        [
          Text(r.value,
              style: TextStyle(
                  fontSize: 12.5,
                  color: r.state == 'busy' ? t.text : t.textDim,
                  fontWeight:
                      r.state == 'busy' ? FontWeight.w600 : FontWeight.w400)),
          if (r.btn.isNotEmpty)
            Align(
              alignment: Alignment.centerLeft,
              child: SmallBtn(
                label: r.btn,
                icon: r.btn == 'Link' ? Icons.link : Icons.link_off,
                primary: r.btn == 'Link',
                danger: r.btn == 'Unlink',
                onTap: () => _c.sendAction(r.key),
              ),
            ),
          if (r.state == 'busy')
            _linkLine(('Open trakt.tv/activate', 'https://trakt.tv/activate')),
        ],
      ),
    );
  }

  Widget _linkLine((String, String) link) {
    final t = context.tokens;
    return Align(
      alignment: Alignment.centerLeft,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        child: GestureDetector(
          onTap: () => openExternal(context, link.$2),
          child: Text('${link.$1} ↗',
              style: TextStyle(
                  fontSize: 11.5,
                  color: t.textDim,
                  decoration: TextDecoration.underline,
                  decorationColor: t.textDim)),
        ),
      ),
    );
  }

  Future<void> _countQueued() async {
    try {
      final n = await scrobblePendingCount();
      if (mounted) setState(() => _queued = n.toInt());
    } catch (_) {
      // No music database yet: nothing is queued, and nothing needs saying.
    }
  }

  static const _sourceKeys = [
    'api.discogs',
    'api.spotify',
    'api.youtube-data',
    'api.anilist',
    'api.anidb',
    'api.addic7ed',
    'api.trakt',
    // Not 'api.listenbrainz': that switch is the Scrobbling tile's.
  ];
  static const _serverKeys = [
    'api.radio-browser',
    'api.autoeq',
    'api.tmdb-image-base',
  ];

  SettingsController get _c => widget.controller;

  void _save(SettingItem r, String v) =>
      _c.send(SettingsCmd.setText(key: r.key, value: v));

  static bool _set(SettingItem? r) => (r?.value.trim() ?? '').isNotEmpty;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final rows = Rows(widget.state.services);
    final piped = rows.key('api.piped-instance');
    // The Trakt sign-in row, which only exists while the Trakt switch is on.
    final traktLink = rows.key('trakt-link');
    final sources =
        _sourceKeys.map(rows.key).whereType<SettingItem>().toList();
    final servers =
        _serverKeys.map(rows.key).whereType<SettingItem>().toList();
    final lb = rows.key('api.listenbrainz');
    // The token row is sent with AI's rows; it is drawn here, with its switch.
    SettingItem? token;
    for (final r in widget.state.ai) {
      if (r.key == 'api.listenbrainz-token') token = r;
    }
    final rest = rows.rest;

    final keys = widget.state.keys;
    final keysSet = keys.where((k) => k.saved).length;
    final on = sources.where((r) => r.on_).length;

    return SettingsPageBody(
      head: SettingsHead.forTab(
        'services',
        note: 'All optional. Tulipix works without any of them; each adds '
            'something',
        actions: [
          StateChip('$keysSet of ${keys.length} keys saved',
              tint: keysSet > 0 ? Tokens.ok : null),
        ],
      ),
      children: [
        TileGrid([
          // The keychain keys -- TMDB, TheTVDB, OpenSubtitles, Last.fm and
          // LibreTranslate -- three to a row.
          for (final k in keys) (span: 2, child: _serviceKeyTile(k)),
          if (traktLink != null) (span: 2, child: _traktTile(traktLink)),
          if (piped != null)
            (
              span: 2,
              child: SettingsTile(
                icon: Icons.public,
                tint: Tokens.secTransfer,
                title: 'YouTube backend (Piped)',
                note: 'The server used to browse YouTube',
                trailing: [
                  StateChip(_set(piped) ? 'Custom' : 'Default',
                      tint: _set(piped) ? Tokens.brand : null),
                ],
                fill: true,
                child: Spread(
                  gap: 8,
                  [
                    FieldBox(
                      value: piped.value,
                      hint: 'Leave blank for the default',
                      onSubmit: (v) => _save(piped, v),
                      trailing: [
                        if (_set(piped))
                          SmallBtn(label: 'Reset', onTap: () => _save(piped, '')),
                      ],
                    ),
                    Text(
                        'Only change this if the default is slow or blocked '
                        'where you are.',
                        style: TextStyle(fontSize: 11.5, color: t.textDim)),
                  ],
                ),
              ),
            ),
          if (lb != null || token != null)
            (
              span: 2,
              child: SettingsTile(
                icon: Icons.podcasts,
                tint: Tokens.secTransfer,
                title: 'Scrobbling',
                note: 'Your listening record on ListenBrainz',
                trailing: [
                  if (_queued != null)
                    StateChip(
                      _queued == 0 ? 'Nothing queued' : '$_queued queued',
                      tint: _queued == 0 ? null : Tokens.warn,
                    ),
                ],
                fill: true,
                child: Spread(
                  gap: 6,
                  [
                    if (lb != null)
                      SettingLine(
                        title: 'Send listens',
                        note: 'Queued while offline, sent when you are back',
                        trailing: RowControl(row: lb, controller: _c),
                      ),
                    if (token != null) ...[
                      FieldBox(
                        value: token.value,
                        secret: true,
                        hint: 'ListenBrainz token',
                        onSubmit: (v) => _save(token!, v),
                        trailing: [
                          SmallBtn(
                            label: 'Send now',
                            onTap: (_queued ?? 0) == 0
                                ? null
                                : () async {
                                    await _c.sendAction('scrobble-flush');
                                    await _countQueued();
                                  },
                          ),
                        ],
                      ),
                      Text(token.desc,
                          style: TextStyle(fontSize: 11.5, color: t.textDim)),
                    ],
                  ],
                ),
              ),
            ),
          if (sources.isNotEmpty)
            (
              span: 6,
              child: SettingsTile(
                icon: Icons.storage_outlined,
                tint: Tokens.secPhotos,
                title: 'Extra metadata sources',
                note: 'Asked when the main source has no answer',
                trailing: [
                  Text('$on of ${sources.length} on',
                      style: TextStyle(fontSize: 11, color: t.textDim)),
                ],
                child: _SourceGrid(
                  sources: sources,
                  onToggle: (r, v) =>
                      _c.send(SettingsCmd.toggle(key: r.key, on_: v)),
                ),
              ),
            ),
          if (servers.isNotEmpty) (span: 6, child: _serversTile(servers)),
        ]),
        if (rest.isNotEmpty) MoreTile(rows: rest, controller: _c),
      ],
    );
  }

  Widget _serversTile(List<SettingItem> servers) {
    final t = context.tokens;
    Widget server(SettingItem r) => Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(r.label,
                style: TextStyle(
                    fontSize: 12, fontWeight: FontWeight.w600, color: t.text)),
            Text(r.desc,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 11.5, color: t.textDim)),
            const SizedBox(height: 6),
            FieldBox(
                value: r.value, hint: 'Default', onSubmit: (v) => _save(r, v)),
          ],
        );
    return SettingsTile(
      icon: Icons.dns_outlined,
      tint: const Color(0xFF64748B),
      title: 'Self-hosted servers',
      note: 'For mirrors and offline networks. Blank uses the public server',
      trailing: [
        SmallBtn(
          label: _servers ? 'Hide' : 'Show',
          icon: _servers ? Icons.expand_less : Icons.expand_more,
          onTap: () => setState(() => _servers = !_servers),
        ),
      ],
      child: !_servers
          ? null
          : Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (var i = 0; i < servers.length; i += 2) ...[
                  if (i > 0) const SizedBox(height: 12),
                  Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Expanded(child: server(servers[i])),
                      const SizedBox(width: 16),
                      Expanded(
                        child: i + 1 < servers.length
                            ? server(servers[i + 1])
                            : const SizedBox.shrink(),
                      ),
                    ],
                  ),
                ],
              ],
            ),
    );
  }
}

/// The metadata sources, three to a row, each a card that is its own switch.
class _SourceGrid extends StatelessWidget {
  const _SourceGrid({required this.sources, required this.onToggle});

  final List<SettingItem> sources;
  final void Function(SettingItem, bool) onToggle;

  @override
  Widget build(BuildContext context) => Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (var i = 0; i < sources.length; i += 3) ...[
            if (i > 0) const SizedBox(height: 8),
            IntrinsicHeight(
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  for (var j = i; j < i + 3; j++) ...[
                    if (j > i) const SizedBox(width: 8),
                    Expanded(
                      child: j < sources.length
                          ? _SourceCard(
                              row: sources[j],
                              onTap: () =>
                                  onToggle(sources[j], !sources[j].on_),
                            )
                          : const SizedBox.shrink(),
                    ),
                  ],
                ],
              ),
            ),
          ],
        ],
      );
}

class _SourceCard extends StatelessWidget {
  const _SourceCard({required this.row, required this.onTap});

  final SettingItem row;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final on = row.on_;
    final r = BorderRadius.circular(12);
    return Semantics(
      toggled: on,
      button: true,
      child: Material(
        color: on ? Tokens.brand.withValues(alpha: 0.08) : Colors.transparent,
        shape: RoundedRectangleBorder(
          borderRadius: r,
          side: BorderSide(
              color: on
                  ? Tokens.brand.withValues(alpha: 0.55)
                  : t.outlineStrong),
        ),
        child: InkWell(
          borderRadius: r,
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(12, 11, 12, 11),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(row.label,
                          style: TextStyle(
                              fontSize: 12.5,
                              fontWeight: FontWeight.w600,
                              color: t.text)),
                      const SizedBox(height: 2),
                      Text(row.desc,
                          style: TextStyle(fontSize: 11.5, color: t.textDim)),
                    ],
                  ),
                ),
                const SizedBox(width: 10),
                AnimatedContainer(
                  duration: const Duration(milliseconds: 150),
                  width: 30,
                  height: 18,
                  padding: const EdgeInsets.all(2),
                  decoration: BoxDecoration(
                    color: on ? Tokens.brand : t.bg,
                    borderRadius: BorderRadius.circular(9),
                    border: Border.all(color: on ? Tokens.brand : t.outline),
                  ),
                  child: AnimatedAlign(
                    duration: const Duration(milliseconds: 150),
                    alignment: on ? Alignment.centerRight : Alignment.centerLeft,
                    child: Container(
                      width: 12,
                      height: 12,
                      decoration: BoxDecoration(
                        color: on ? Colors.white : t.textDim,
                        shape: BoxShape.circle,
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

// ── ai features ─────────────────────────────────────────────────────────────

class AiTab extends StatefulWidget {
  const AiTab({super.key, required this.controller, required this.state});

  final SettingsController controller;
  final SettingsState state;

  @override
  State<AiTab> createState() => _AiTabState();
}

class _AiTabState extends State<AiTab> {
  static const _langs = {
    'en': 'English',
    'hi': 'हिन्दी',
    'pa': 'ਪੰਜਾਬੀ',
    'de': 'Deutsch',
    'fr': 'Français',
    'es': 'Español',
    'ru': 'Русский',
    'it': 'Italiano',
    'auto': 'Auto',
  };

  SettingItem? _req(String label) {
    for (final r in widget.state.aiRequirements) {
      if (r.label == label) return r;
    }
    return null;
  }

  static String _name(SettingItem m) => m.label.split(' · ').first;

  static String _size(SettingItem m) {
    final p = m.label.split(' · ');
    return p.length > 1 ? p.last : '';
  }

  /// Asks first: a model can be hundreds of megabytes to fetch again.
  Future<void> _remove(SettingItem m) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Remove ${_name(m)}?'),
        content: Text('Its ${_size(m)} is deleted from this computer. '
            'Download brings it back.'),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: Tokens.error),
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Remove'),
          ),
        ],
      ),
    );
    if (ok == true) {
      await widget.controller
          .sendAction('ai-rm-${m.key.substring('ai-dl-'.length)}');
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = widget.controller;
    final st = widget.state;
    final rows = Rows(st.ai);
    final models = [
      for (final r in st.ai)
        if (r.kind == 'status-action' && r.key.startsWith('ai-dl-')) r,
    ];
    rows.use(models.map((m) => m.key));
    final check = rows.key('ai-update-check');
    final lang = rows.key('ai.voice-lang');
    final mVoice = rows.key('ai.model.voice');
    final mTrans = rows.key('ai.model.transcribe');
    final features = ['ai.captions', 'ai.voice', 'ai.chat', 'books.tts.neural']
        .map(rows.key)
        .whereType<SettingItem>()
        .toList();
    // Scrobbling is drawn on Services & Keys.
    rows.use(['api.listenbrainz', 'api.listenbrainz-token', 'scrobble-flush']);
    final cloud = rows.key('ai.cloud-offload');
    final rest = rows.rest;

    final ram = _req('Installed memory');
    final gfx = _req('Graphics acceleration');
    final installed = models.where((m) => m.state == 'ok').toList();
    final mb = installed.fold<int>(
        0,
        (a, m) =>
            a +
            (int.tryParse(_size(m).replaceAll(RegExp(r'[^0-9]'), '')) ?? 0));
    final unknown = ram == null || ram.state == 'muted';
    final short =
        models.where((m) => _req(_name(m))?.state == 'warn').length;
    // "ONNX runtime compiled in — GPU used when available" reads as its end.
    final gfxText = gfx == null ? '—' : _cap(gfx.value.split(' — ').last);
    void set(String key, String v) =>
        c.send(SettingsCmd.setText(key: key, value: v));
    Seg modelSeg(SettingItem r) => Seg(
          options: r.options,
          labels: [for (final o in r.options) _cap(o)],
          value: r.value,
          onPick: (v) => set(r.key, v),
        );

    return SettingsPageBody(
      head: SettingsHead.forTab(
        'ai',
        note: 'Models run on this computer. Nothing is sent anywhere unless '
            'you switch on cloud help',
        actions: [
          if (check != null)
            SmallBtn(
              label: 'Check for updates',
              icon: Icons.refresh,
              large: true,
              busy: check.state == 'busy',
              onTap: () => c.sendAction('ai-update-check'),
            ),
        ],
      ),
      children: [
        _Machine(
          figures: [
            ('This computer', '${ram?.value ?? 'Unknown'} memory'),
            ('Graphics', gfxText),
            (
              'Installed',
              '${installed.length} of ${models.length} models'
                  '${mb > 0 ? ' · $mb MB' : ''}'
            ),
          ],
          chip: StateChip(
            unknown
                ? 'Memory unknown'
                : short == 0
                    ? 'Every model fits'
                    : '$short may not fit',
            tint: unknown
                ? null
                : short == 0
                    ? Tokens.ok
                    : Tokens.warn,
          ),
        ),
        SettingsTile(
          icon: Icons.memory,
          tint: Tokens.secPhotos,
          title: 'On-device models',
          note: 'Only what you use is loaded, and unloaded after',
          child: models.isEmpty
              ? Text('This build lists no models.',
                  style: TextStyle(fontSize: 12, color: t.textDim))
              : LayoutBuilder(
                  builder: (context, box) {
                    final per = box.maxWidth >= 720
                        ? 3
                        : box.maxWidth >= 460
                            ? 2
                            : 1;
                    return Column(
                      children: [
                        for (var i = 0; i < models.length; i += per) ...[
                          if (i > 0) const SizedBox(height: 10),
                          IntrinsicHeight(
                            child: Row(
                              crossAxisAlignment: CrossAxisAlignment.stretch,
                              children: [
                                for (var j = i; j < i + per; j++) ...[
                                  if (j > i) const SizedBox(width: 10),
                                  Expanded(
                                    child: j < models.length
                                        ? _ModelCard(
                                            row: models[j],
                                            name: _name(models[j]),
                                            size: _size(models[j]),
                                            need: _req(_name(models[j])),
                                            onTap: () =>
                                                c.sendAction(models[j].key),
                                            onRemove: () =>
                                                _remove(models[j]),
                                          )
                                        : const SizedBox.shrink(),
                                  ),
                                ],
                              ],
                            ),
                          ),
                        ],
                      ],
                    );
                  },
                ),
        ),
        TileGrid([
          (
            span: 3,
            child: SettingsTile(
              icon: Icons.mic_none,
              tint: const Color(0xFF0EA5E9),
              title: 'Voice recognition',
              note: 'The mic in search fields, and transcribing',
              child: Lines([
                if (lang != null)
                  SettingLine(
                    title: 'Spoken language',
                    note: 'Pinning one beats auto-detect on short clips',
                    below: true,
                    trailing: Seg(
                      options: lang.options,
                      labels: [for (final o in lang.options) _langs[o] ?? o],
                      value: lang.value,
                      onPick: (v) => set(lang.key, v),
                    ),
                  ),
                if (mVoice != null)
                  SettingLine(
                    title: 'Voice search',
                    note: 'Tiny answers fastest',
                    below: true,
                    trailing: modelSeg(mVoice),
                  ),
                if (mTrans != null)
                  SettingLine(
                    title: 'Transcribe & subtitles',
                    note: 'Bigger models catch more words',
                    below: true,
                    trailing: modelSeg(mTrans),
                  ),
              ]),
            ),
          ),
          (
            span: 3,
            child: SettingsTile(
              icon: Icons.auto_awesome_outlined,
              tint: Tokens.secMusic,
              title: 'Features',
              note: 'What the models are used for, and cloud help',
              // Cloud help is one more switch on what the models do, and the
              // one that sends anything away — so its chip is the header's.
              trailing: [
                if (cloud != null)
                  StateChip(
                    cloud.on_ ? 'Cloud help on' : 'All on this computer',
                    tint: cloud.on_ ? Tokens.warn : Tokens.ok,
                  ),
              ],
              child: Lines([
                for (final r in features) RowLine(row: r, controller: c),
                if (cloud != null)
                  SettingLine(
                    title: 'Allow cloud AI help',
                    note: 'Selected questions go to a cloud service. Off '
                        'keeps everything on this computer',
                    trailing: RowControl(row: cloud, controller: c),
                  ),
              ]),
            ),
          ),
        ]),
        if (rest.isNotEmpty) MoreTile(rows: rest, controller: c),
      ],
    );
  }
}

/// What this computer brings: memory, graphics, and what is installed.
class _Machine extends StatelessWidget {
  const _Machine({required this.figures, required this.chip});

  final List<(String, String)> figures;
  final Widget chip;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.fromLTRB(16, 14, 16, 14),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: t.outline),
        gradient: LinearGradient(
          colors: [
            Color.alphaBlend(
                Tokens.secPhotos.withValues(alpha: 0.16), t.panel2),
            t.panel2,
          ],
          stops: const [0, 0.6],
        ),
      ),
      child: Wrap(
        spacing: 28,
        runSpacing: 12,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          for (final (k, v) in figures)
            Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(k.toUpperCase(),
                    style: TextStyle(
                        fontSize: 10.5,
                        fontWeight: FontWeight.w600,
                        letterSpacing: 0.6,
                        color: t.textDim)),
                Text(v,
                    style: TextStyle(
                        fontSize: 15,
                        fontWeight: FontWeight.w700,
                        color: t.text)),
              ],
            ),
          chip,
        ],
      ),
    );
  }
}

/// One model: its size, what it is for, what it asks of this computer, and
/// the one button — Download, or Verify once it is here, with Remove beside it.
class _ModelCard extends StatelessWidget {
  const _ModelCard({
    required this.row,
    required this.name,
    required this.size,
    required this.need,
    required this.onTap,
    required this.onRemove,
  });

  final SettingItem row;
  final String name;
  final String size;

  /// Its row on the requirements list; null where there is none.
  final SettingItem? need;
  final VoidCallback onTap;

  /// Asks, then deletes the download. Offered only once it is here.
  final VoidCallback onRemove;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final downloading = row.frac >= 0;
    final download = row.btn == 'Download';
    final n = need;
    return Container(
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: t.bg,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: t.outlineStrong),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        color: t.text)),
              ),
              Text(size, style: TextStyle(fontSize: 11, color: t.textDim)),
            ],
          ),
          const SizedBox(height: 6),
          Text(row.desc,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(fontSize: 11.5, color: t.textDim)),
          if (n != null) ...[
            const SizedBox(height: 4),
            Text('Needs ${n.value.split(' · ').first}',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 11,
                    color: n.state == 'warn' ? Tokens.warn : t.textDim)),
          ],
          if (downloading) ...[
            const SizedBox(height: 8),
            ClipRRect(
              borderRadius: BorderRadius.circular(3),
              child: LinearProgressIndicator(
                value: row.frac.clamp(0.0, 1.0),
                minHeight: 5,
                backgroundColor: t.outline,
                color: Tokens.brand,
              ),
            ),
          ],
          const Spacer(),
          const SizedBox(height: 10),
          Row(
            children: [
              StateChip(
                downloading ? '${(row.frac * 100).round()}%' : row.value,
                tint: stateTint(row.state, t),
              ),
              const Spacer(),
              if (!download && !downloading) ...[
                SmallBtn(
                  label: 'Remove',
                  danger: true,
                  onTap: onRemove,
                ),
                const SizedBox(width: 6),
              ],
              SmallBtn(
                label: row.btn,
                icon: download ? Icons.download : null,
                primary: download,
                busy: row.state == 'busy',
                onTap: onTap,
              ),
            ],
          ),
        ],
      ),
    );
  }
}
