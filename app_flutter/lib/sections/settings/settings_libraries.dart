// Settings › Libraries: every folder Tulipix reads, as one dense table.
//
// docs/mockups/NewSections/settings-libraries-deck.html. The cards this
// replaces took a screen for four folders; this fits a dozen, grouped by the
// drive they are on (an unplugged drive folds to one line), by section, or
// flat. Ticking rows gives the bulk actions; clicking one fills the side
// panel with what the cards used to repeat on every folder.
//
// The list itself is `tulipix_core::watched`'s. When its file cannot be read
// the sections carry on from the backup and this tab says so, with the way to
// put it back — rather than showing an empty page that looks like a lost
// library.

import 'dart:io';
import 'dart:math' as math;

import 'package:desktop_drop/desktop_drop.dart';
import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../platform/pick.dart';
import '../../shell/sidebar.dart' show kSectionMeta;
import '../../src/rust/api/settings.dart';
import '../../src/rust/api/status.dart';
import 'settings_controller.dart';
import 'settings_kit.dart';
import 'settings_sections.dart' show kSectionById;

/// The five places a music folder can go, by the key the shared
/// `music_folder_sections.json` stores.
const Map<String, String> _musicShelves = {
  'mymusic': 'My Music',
  'podcasts': 'Podcasts',
  'audiobooks': 'Audiobooks',
  'radio': 'Radio',
  'youtube': 'YouTube',
};

/// The auto-rescan cadences, as `scan.cadence` stores them.
const Map<String, String> _cadences = {
  'manual': 'Never',
  'hourly': 'Hourly',
  'daily': 'Daily',
  'weekly': 'Weekly',
};

/// The sections that read watched folders, by the names Status reports.
const _readers = ['Photos', 'Videos', 'Music', 'Books'];

const _thisComputer = 'This computer';

Color _tintOf(String name) => switch (name.toLowerCase()) {
      'photos' => Tokens.secPhotos,
      'videos' => Tokens.secVideos,
      'music' => Tokens.secMusic,
      'books' => Tokens.secBooks,
      _ => Tokens.secSettings,
    };

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

String _nameOf(String path) {
  final parts = path.split(RegExp(r'[\\/]')).where((p) => p.isNotEmpty);
  return parts.isEmpty ? path : parts.last;
}

String _parentOf(String path) {
  final i = path.lastIndexOf(RegExp(r'[\\/]'));
  return i <= 0 ? '' : tildePath(path.substring(0, i));
}

class LibrariesTab extends StatefulWidget {
  const LibrariesTab(
      {super.key, required this.controller, required this.state});

  final SettingsController controller;
  final SettingsState state;

  @override
  State<LibrariesTab> createState() => _LibrariesTabState();
}

class _LibrariesTabState extends State<LibrariesTab> {
  /// The Status snapshot, for what each folder holds and the totals. Read on
  /// arriving — it queries every section's database — and again after
  /// anything changes the list.
  StatusState? _lib;
  bool _scanning = false;
  bool _over = false;

  /// A folder was added while a scan ran: that scan read the list when it
  /// began, so go round once more when it ends.
  bool _again = false;

  /// "all", a section name from [_readers], or "problems".
  String _show = 'all';
  String _query = '';

  /// "drive" | "section" | "flat".
  String _by = 'drive';

  /// Ticked rows, by path.
  final Set<String> _picked = {};

  /// The row whose details the side panel shows.
  String? _sel;

  /// Groups folded or unfolded against their default. An unplugged drive
  /// starts folded; everything else starts open.
  final Set<String> _flipped = {};

  /// The long job Settings is running, by key, or empty.
  String get _job => widget.state.taskKey;

  SettingsController get _c => widget.controller;

  @override
  void initState() {
    super.initState();
    _load();
    // Not straight: sending notifies, and initState runs inside a build.
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
    _c.say(
        why ?? 'Rescanning every watched folder. You can keep using the app.');
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
  /// Rescan.
  Future<void> _add(String path) async {
    final before = _c.state?.libraries.length ?? 0;
    await _c.send(SettingsCmd.libAdd(path: path));
    await _load();
    final after = _c.state?.libraries.length ?? 0;
    if (!mounted || after <= before) return;
    setState(() => _sel = path);
    _rescan(
        why: 'Watching ${tildePath(path)} and reading it now. You can keep '
            'using the app.');
  }

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
      _c.say(
          'Drop a folder — files are found by watching the folder they are in.');
      return;
    }
    for (final p in dirs) {
      await _add(p);
    }
  }

  Future<void> _remove(List<String> paths) async {
    final many = paths.length > 1;
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(many
            ? 'Stop watching ${paths.length} folders?'
            : 'Stop watching ${_nameOf(paths.first)}?'),
        content: const Text(
            'Nothing is deleted. What was found in them is marked missing at '
            'the next scan, and comes back if you watch them again.'),
        actions: [
          TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('Cancel')),
          FilledButton(
              onPressed: () => Navigator.pop(context, true),
              child: const Text('Stop watching')),
        ],
      ),
    );
    if (ok != true) return;
    for (final p in paths) {
      await _c.send(SettingsCmd.libRemove(path: p));
    }
    setState(() {
      _picked.removeAll(paths);
      if (paths.contains(_sel)) _sel = null;
    });
    await _load();
  }

  List<String> _sectionsOf(String path) {
    for (final r in _lib?.roots ?? const <StRoot>[]) {
      if (r.path == path) return r.sections;
    }
    return const [];
  }

  /// Gone, or watched and never read.
  static bool _problem(LibraryRow l) =>
      !l.exists || (l.lastScan.toInt() == 0 && l.items.toInt() <= 0);

  bool _passes(LibraryRow l) {
    final q = _query.trim().toLowerCase();
    if (q.isNotEmpty && !l.path.toLowerCase().contains(q)) return false;
    return switch (_show) {
      'all' => true,
      'problems' => _problem(l),
      _ => _sectionsOf(l.path).contains(_show),
    };
  }

  /// The rows in their groups. A folder more than one section reads is in
  /// each of their groups when grouped by section.
  List<({String name, List<LibraryRow> rows})> _groups(List<LibraryRow> libs) {
    if (_by == 'flat') return [(name: '', rows: libs)];
    final out = <String, List<LibraryRow>>{};
    for (final l in libs) {
      final keys = _by == 'drive'
          ? [l.drive]
          : (_sectionsOf(l.path).isEmpty
              ? ['Nothing found yet']
              : _sectionsOf(l.path));
      for (final k in keys) {
        (out[k] ??= []).add(l);
      }
    }
    int rank(String k) => _by == 'drive'
        ? (k == _thisComputer ? 0 : 1)
        : (_readers.contains(k) ? _readers.indexOf(k) : 99);
    final names = out.keys.toList()
      ..sort((a, b) {
        final r = rank(a).compareTo(rank(b));
        return r != 0 ? r : a.toLowerCase().compareTo(b.toLowerCase());
      });
    return [for (final n in names) (name: n, rows: out[n]!)];
  }

  @override
  Widget build(BuildContext context) {
    final st = widget.state;
    final libs = st.libraries;
    final shown = libs.where(_passes).toList();
    final byPath = {for (final l in libs) l.path: l};
    final sel = byPath[_sel];
    // Ticks on rows that went away, or were filtered out, do not count.
    final picked = [
      for (final l in shown)
        if (_picked.contains(l.path)) l.path
    ];

    final main = <Widget>[
      SettingsHead.forTab(
        'libraries',
        note: '',
        actions: [
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
      if (st.libHealth == 'unreadable') ...[
        const SizedBox(height: 14),
        _Recovery(
          backup: st.libBackup,
          onRestore: () async {
            await _c.send(const SettingsCmd.libRestore());
            await _load();
          },
        ),
      ],
      const SizedBox(height: 12),
      _HealthLine(st: st, lib: _lib),
      if (_scanning || _job.isNotEmpty) ...[
        const SizedBox(height: 12),
        _JobStrip(
          label:
              _job.isEmpty ? 'Rescanning every watched folder' : st.taskLabel,
          frac: _job.isEmpty ? -1 : st.taskFrac,
        ),
      ],
      const SizedBox(height: 14),
      _toolbar(libs),
      const SizedBox(height: 10),
      if (picked.isNotEmpty) ...[
        _bulkBar(picked),
        const SizedBox(height: 10),
      ],
      _table(libs, shown, picked),
      const SizedBox(height: 10),
      _dropStrip(libs.isEmpty),
      if (st.libOwned.isNotEmpty) ...[
        const SizedBox(height: 14),
        _Owned(rows: st.libOwned),
      ],
    ];
    final aside = <Widget>[
      _details(sel),
      const SizedBox(height: 14),
      _schedule(),
      const SizedBox(height: 14),
      SettingsTile(
        icon: Icons.manage_search,
        tint: Tokens.brand,
        title: 'Search index',
        note: 'Index every photo again, so search finds what changed',
        child: Align(
          alignment: Alignment.centerLeft,
          child: SmallBtn(
            label: _job == 'rebuild-search' ? 'Rebuilding…' : 'Rebuild search',
            busy: _job == 'rebuild-search',
            onTap: libs.isEmpty || _job.isNotEmpty
                ? null
                : () => _c.sendAction('rebuild-search'),
          ),
        ),
      ),
    ];

    return LayoutBuilder(builder: (context, box) {
      // The same gutter as SettingsPageBody, so the blocks line up with the
      // other tabs.
      final g = math.max(16.0, box.maxWidth * 0.02);
      if (box.maxWidth < 1100) {
        return ListView(
          padding: EdgeInsets.fromLTRB(g, 18, g, 28),
          children: [...main, const SizedBox(height: 14), ...aside],
        );
      }
      return Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(
            child: ListView(
              padding: EdgeInsets.fromLTRB(g, 18, 16, 28),
              children: main,
            ),
          ),
          SizedBox(
            width: 330,
            child: ListView(
              padding: EdgeInsets.fromLTRB(0, 18, g, 28),
              children: aside,
            ),
          ),
        ],
      );
    });
  }

  // ── toolbar ───────────────────────────────────────────────────────────────

  Widget _toolbar(List<LibraryRow> libs) {
    int count(String k) => libs
        .where((l) => k == 'all'
            ? true
            : k == 'problems'
                ? _problem(l)
                : _sectionsOf(l.path).contains(k))
        .length;
    final problems = count('problems');
    return Wrap(
      spacing: 8,
      runSpacing: 8,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        for (final k in ['all', ..._readers, if (problems > 0) 'problems'])
          _Pill(
            label: k == 'all'
                ? 'All'
                : k == 'problems'
                    ? 'Problems'
                    : k,
            count: count(k),
            tint: k == 'problems'
                ? Tokens.error
                : k == 'all'
                    ? null
                    : _tintOf(k),
            on: _show == k,
            onTap: () => setState(() => _show = k),
          ),
        SizedBox(
          width: 220,
          height: 34,
          child: TextField(
            onChanged: (v) => setState(() => _query = v),
            style: const TextStyle(fontSize: 13),
            decoration: const InputDecoration(
              isDense: true,
              hintText: 'Filter by path',
              prefixIcon: Icon(Icons.search, size: 17),
              prefixIconConstraints: BoxConstraints(minWidth: 34),
              contentPadding: EdgeInsets.symmetric(vertical: 8),
              border: OutlineInputBorder(),
            ),
          ),
        ),
        SegmentedButton<String>(
          style: const ButtonStyle(
            visualDensity: VisualDensity.compact,
            tapTargetSize: MaterialTapTargetSize.shrinkWrap,
          ),
          showSelectedIcon: false,
          segments: const [
            ButtonSegment(value: 'drive', label: Text('By drive')),
            ButtonSegment(value: 'section', label: Text('By section')),
            ButtonSegment(value: 'flat', label: Text('Flat')),
          ],
          selected: {_by},
          onSelectionChanged: (v) => setState(() => _by = v.first),
        ),
      ],
    );
  }

  Widget _bulkBar(List<String> picked) {
    final t = context.tokens;
    final n = picked.length;
    return Container(
      padding: const EdgeInsets.fromLTRB(14, 8, 8, 8),
      decoration: BoxDecoration(
        color: Tokens.brand.withValues(alpha: 0.10),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: Tokens.brand.withValues(alpha: 0.35)),
      ),
      child: Wrap(
        spacing: 6,
        runSpacing: 6,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          Padding(
            padding: const EdgeInsets.only(right: 8),
            child: Text('$n selected',
                style: TextStyle(
                    fontSize: 13, fontWeight: FontWeight.w700, color: t.text)),
          ),
          SmallBtn(
            label: 'Rescan',
            icon: Icons.refresh,
            onTap: _job.isNotEmpty
                ? null
                : () => _c.sendAction('lib-rescan-many:${picked.join('\n')}'),
          ),
          PopupMenuButton<String>(
            tooltip: 'How often these are read again by themselves',
            onSelected: (v) async {
              for (final p in picked) {
                await _c
                    .send(SettingsCmd.setText(key: 'lib.cadence.$p', value: v));
              }
            },
            itemBuilder: (_) => [
              for (final e in {'': 'Default', ..._cadences}.entries)
                PopupMenuItem(value: e.key, child: Text(e.value)),
            ],
            child: const IgnorePointer(
              child: SmallBtn(
                  label: 'Read again…', icon: Icons.schedule, onTap: _noop),
            ),
          ),
          SmallBtn(
            label: 'Thumbnails',
            tooltip: 'Redraw their thumbnails',
            onTap: () async {
              for (final p in picked) {
                await _c.sendAction('lib-thumbs:$p');
              }
            },
          ),
          SmallBtn(
            label: 'Stop watching',
            danger: true,
            onTap: () => _remove(picked),
          ),
          SmallBtn(
            label: 'Clear',
            ghost: true,
            onTap: () => setState(_picked.clear),
          ),
        ],
      ),
    );
  }

  static void _noop() {}

  // ── the table ─────────────────────────────────────────────────────────────

  Widget _table(
      List<LibraryRow> libs, List<LibraryRow> shown, List<String> picked) {
    final t = context.tokens;
    final Widget body;
    if (libs.isEmpty) {
      body = Padding(
        padding: const EdgeInsets.all(28),
        child: Text(
          'No folders yet. Use Watch a folder, or drop one below.',
          textAlign: TextAlign.center,
          style: TextStyle(fontSize: 13, color: t.textDim),
        ),
      );
    } else if (shown.isEmpty) {
      body = Padding(
        padding: const EdgeInsets.all(28),
        child: Text('No folder matches.',
            textAlign: TextAlign.center,
            style: TextStyle(fontSize: 13, color: t.textDim)),
      );
    } else {
      body = LayoutBuilder(builder: (context, box) {
        final wide = box.maxWidth >= 720;
        final rows = <Widget>[];
        for (final grp in _groups(shown)) {
          final gone = grp.rows.every((l) => !l.exists);
          // An unplugged drive folds to one line by default.
          final folded =
              (_by == 'drive' && gone) != _flipped.contains(grp.name);
          if (grp.name.isNotEmpty) {
            rows.add(_GroupHead(
              name: grp.name,
              by: _by,
              count: grp.rows.length,
              missing: grp.rows.where((l) => !l.exists).length,
              unplugged: _by == 'drive' && gone,
              folded: folded,
              onTap: () => setState(() => _flipped.contains(grp.name)
                  ? _flipped.remove(grp.name)
                  : _flipped.add(grp.name)),
            ));
          }
          if (folded) continue;
          for (final l in grp.rows) {
            rows.add(_Row(
              row: l,
              sections: _sectionsOf(l.path),
              wide: wide,
              ticked: _picked.contains(l.path),
              selected: _sel == l.path,
              busy: _job == 'lib-rescan:${l.path}',
              onTick: (v) => setState(
                  () => v ? _picked.add(l.path) : _picked.remove(l.path)),
              onTap: () => setState(() => _sel = l.path),
            ));
          }
        }
        final all = shown.every((l) => _picked.contains(l.path));
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _HeadRow(
              wide: wide,
              ticked: picked.isEmpty ? false : (all ? true : null),
              onTick: () => setState(() => all
                  ? _picked.removeAll(shown.map((l) => l.path))
                  : _picked.addAll(shown.map((l) => l.path))),
            ),
            ...rows,
          ],
        );
      });
    }
    return Container(
      clipBehavior: Clip.antiAlias,
      decoration: BoxDecoration(
        color: t.panel2,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: t.outline),
      ),
      child: body,
    );
  }

  Widget _dropStrip(bool empty) {
    final t = context.tokens;
    return DropTarget(
      onDragEntered: (_) => setState(() => _over = true),
      onDragExited: (_) => setState(() => _over = false),
      onDragDone: _drop,
      child: Container(
        height: 46,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: _over ? Tokens.warn.withValues(alpha: 0.08) : null,
          borderRadius: BorderRadius.circular(12),
          border: Border.all(
              color: _over ? Tokens.warn : t.outlineStrong, width: 1.2),
        ),
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            const Icon(Icons.create_new_folder_outlined,
                size: 18, color: Tokens.warn),
            const SizedBox(width: 8),
            Text(
                empty
                    ? 'Drop a folder here to start'
                    : 'Drop folders here to watch them',
                style: TextStyle(fontSize: 12.5, color: t.textDim)),
          ],
        ),
      ),
    );
  }

  // ── side panel ────────────────────────────────────────────────────────────

  Widget _details(LibraryRow? l) {
    final t = context.tokens;
    if (l == null) {
      return const SettingsTile(
        icon: Icons.folder_outlined,
        tint: Tokens.warn,
        title: 'Pick a folder',
        note: 'Click a row to see what it holds and change how it is read',
      );
    }
    final secs = _sectionsOf(l.path);
    // The shelf picker shows for a folder that holds music, or one already
    // moved off My Music.
    final music =
        l.exists && (secs.contains('Music') || l.musicSection != 'mymusic');
    final busy = _job == 'lib-rescan:${l.path}';
    final n = l.items.toInt();
    final last = l.lastScan.toInt();
    return SettingsTile(
      icon: l.exists ? Icons.folder_outlined : Icons.folder_off_outlined,
      tint: l.exists ? Tokens.warn : Tokens.error,
      title: _nameOf(l.path),
      note: tildePath(l.path),
      danger: !l.exists,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            !l.exists
                ? 'Not on disk. Its items stay in the library, marked '
                    'missing; plug the drive back in and it is picked up again.'
                : [
                    if (n >= 0) '${_grouped(n)} ${n == 1 ? 'item' : 'items'}',
                    last > 0 ? 'read ${_since(last)}' : 'not read yet',
                    l.drive,
                  ].join(' · '),
            style: TextStyle(fontSize: 12, color: t.textDim),
          ),
          if (secs.isNotEmpty) ...[
            const SizedBox(height: 10),
            Wrap(spacing: 6, runSpacing: 6, children: [
              for (final s in secs) _SecChip(s),
            ]),
          ],
          if (music) ...[
            const SizedBox(height: 14),
            Text('Its music goes to',
                style: TextStyle(fontSize: 11.5, color: t.textDim)),
            const SizedBox(height: 6),
            Pick(
              options: _musicShelves.keys.toList(),
              labels: _musicShelves.values.toList(),
              value: l.musicSection,
              onPick: (v) => _c.sendAction('lib-section:$v:${l.path}'),
            ),
          ],
          if (l.exists) ...[
            const SizedBox(height: 14),
            Text('Read again automatically',
                style: TextStyle(fontSize: 11.5, color: t.textDim)),
            const SizedBox(height: 6),
            Pick(
              options: ['', ..._cadences.keys],
              labels: ['Default', ..._cadences.values],
              value: l.cadence,
              onPick: (v) => _c.send(
                  SettingsCmd.setText(key: 'lib.cadence.${l.path}', value: v)),
            ),
          ],
          const SizedBox(height: 14),
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [
              if (l.exists) ...[
                SmallBtn(
                  label: 'Open',
                  icon: Icons.open_in_new,
                  onTap: () => _c.sendAction('lib-open:${l.path}'),
                ),
                SmallBtn(
                  label: busy ? 'Reading…' : 'Rescan',
                  busy: busy,
                  tooltip: 'Read this folder again',
                  onTap: _job.isNotEmpty
                      ? null
                      : () => _c.sendAction('lib-rescan:${l.path}'),
                ),
                SmallBtn(
                  label: 'Thumbnails',
                  tooltip: 'Redraw this folder\'s thumbnails',
                  onTap:
                      busy ? null : () => _c.sendAction('lib-thumbs:${l.path}'),
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
            ],
          ),
          const SizedBox(height: 10),
          Center(
            child: SmallBtn(
              label: 'Stop watching',
              danger: true,
              tooltip: 'Nothing indexed from it is deleted',
              onTap: () => _remove([l.path]),
            ),
          ),
        ],
      ),
    );
  }

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
          Pick(
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
}

// ── pieces ──────────────────────────────────────────────────────────────────

/// The list could not be read: the sections are on the backup, and this puts
/// it back — or, with no backup, starts a new list so folders can be added.
class _Recovery extends StatelessWidget {
  const _Recovery({required this.backup, required this.onRestore});

  final int backup;
  final VoidCallback onRestore;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final has = backup >= 0;
    return Container(
      padding: const EdgeInsets.fromLTRB(16, 14, 14, 14),
      decoration: BoxDecoration(
        color: Tokens.error.withValues(alpha: 0.08),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: Tokens.error.withValues(alpha: 0.4)),
      ),
      child: Row(
        children: [
          const Icon(Icons.restore_page_outlined, color: Tokens.error),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text('The folder list could not be read',
                    style: TextStyle(
                        fontSize: 13.5,
                        fontWeight: FontWeight.w700,
                        color: t.text)),
                const SizedBox(height: 2),
                Text(
                  has
                      ? 'Your libraries are fine. Tulipix is reading the backup '
                          'from before the last change ($backup '
                          '${backup == 1 ? 'folder' : 'folders'}) and has not '
                          'written over anything. Put it back to make it the '
                          'list again.'
                      : 'There is no backup to go back to. Nothing indexed is '
                          'lost; start a new list and watch your folders again.',
                  style: TextStyle(fontSize: 12, color: t.textDim),
                ),
              ],
            ),
          ),
          const SizedBox(width: 12),
          SmallBtn(
            label: has ? 'Put the backup back' : 'Start a new list',
            primary: true,
            onTap: onRestore,
          ),
        ],
      ),
    );
  }
}

/// Folders, how many are on disk, the totals, and when the list was saved.
class _HealthLine extends StatelessWidget {
  const _HealthLine({required this.st, required this.lib});

  final SettingsState st;
  final StatusState? lib;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final n = st.libraries.length;
    final missing = st.libraries.where((l) => !l.exists).length;
    final saved = st.libSavedAt;
    final parts = [
      '$n ${n == 1 ? 'folder' : 'folders'}',
      if (n > 0)
        missing == 0
            ? 'all on disk'
            : '${n - missing} on disk, $missing missing',
      if (lib != null) '${lib!.libItems} items',
      if (lib != null) lib!.libBytes,
      if (saved > 0) 'list saved ${_since(saved)}',
      if (st.libBackup >= 0) 'backup kept',
    ];
    final ok = st.libHealth != 'unreadable' && missing == 0;
    return Row(
      children: [
        Container(
          width: 8,
          height: 8,
          decoration: BoxDecoration(
            color: ok ? Tokens.ok : (missing > 0 ? Tokens.warn : Tokens.error),
            shape: BoxShape.circle,
          ),
        ),
        const SizedBox(width: 10),
        Expanded(
          child: Text(parts.join(' · '),
              style: TextStyle(fontSize: 12.5, color: t.textDim)),
        ),
      ],
    );
  }
}

class _Pill extends StatelessWidget {
  const _Pill({
    required this.label,
    required this.count,
    required this.on,
    required this.onTap,
    this.tint,
  });

  final String label;
  final int count;
  final bool on;
  final VoidCallback onTap;
  final Color? tint;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final k = tint;
    return Material(
      color: on ? t.text.withValues(alpha: 0.10) : Colors.transparent,
      shape: StadiumBorder(side: BorderSide(color: on ? t.text : t.outline)),
      child: InkWell(
        customBorder: const StadiumBorder(),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 7),
          child: Row(mainAxisSize: MainAxisSize.min, children: [
            if (k != null) ...[
              Container(
                width: 8,
                height: 8,
                decoration: BoxDecoration(color: k, shape: BoxShape.circle),
              ),
              const SizedBox(width: 7),
            ],
            Text(label,
                style: TextStyle(
                    fontSize: 12.5,
                    fontWeight: on ? FontWeight.w700 : FontWeight.w500,
                    color: t.text)),
            const SizedBox(width: 6),
            Text('$count', style: TextStyle(fontSize: 12, color: t.textDim)),
          ]),
        ),
      ),
    );
  }
}

class _SecChip extends StatelessWidget {
  const _SecChip(this.name);

  final String name;

  @override
  Widget build(BuildContext context) {
    final k = _tintOf(name);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
      decoration: BoxDecoration(
        color: k.withValues(alpha: 0.14),
        borderRadius: BorderRadius.circular(6),
      ),
      child: Text(name,
          style:
              TextStyle(fontSize: 11, fontWeight: FontWeight.w600, color: k)),
    );
  }
}

const double _wSec = 170, _wItems = 80, _wRead = 100, _wState = 96;

class _HeadRow extends StatelessWidget {
  const _HeadRow(
      {required this.wide, required this.ticked, required this.onTick});

  final bool wide;

  /// All, none, or some (null).
  final bool? ticked;
  final VoidCallback onTick;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget h(String s, double? w, {bool end = false}) {
      final x = Text(s,
          textAlign: end ? TextAlign.end : TextAlign.start,
          style: TextStyle(
              fontSize: 10.5,
              fontWeight: FontWeight.w700,
              letterSpacing: 0.6,
              color: t.textDim));
      return w == null ? Expanded(child: x) : SizedBox(width: w, child: x);
    }

    return Container(
      padding: const EdgeInsets.fromLTRB(6, 4, 14, 4),
      decoration:
          BoxDecoration(border: Border(bottom: BorderSide(color: t.outline))),
      child: Row(children: [
        Checkbox(
          value: ticked,
          tristate: true,
          visualDensity: VisualDensity.compact,
          onChanged: (_) => onTick(),
        ),
        const SizedBox(width: 34),
        h('FOLDER', null),
        if (wide) h('SECTIONS', _wSec),
        h('ITEMS', _wItems, end: true),
        if (wide) ...[const SizedBox(width: 16), h('LAST READ', _wRead)],
        const SizedBox(width: 8),
        h('STATE', _wState),
      ]),
    );
  }
}

class _GroupHead extends StatelessWidget {
  const _GroupHead({
    required this.name,
    required this.by,
    required this.count,
    required this.missing,
    required this.unplugged,
    required this.folded,
    required this.onTap,
  });

  final String name;
  final String by;
  final int count;
  final int missing;
  final bool unplugged;
  final bool folded;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final icon = by == 'section'
        ? (kSectionMeta[kSectionById[name.toLowerCase()]]?.icon ??
            Icons.help_outline)
        : name == _thisComputer
            ? Icons.computer_outlined
            : unplugged
                ? Icons.usb_off_outlined
                : Icons.storage_outlined;
    return InkWell(
      onTap: onTap,
      child: Container(
        color: t.text.withValues(alpha: 0.03),
        padding: const EdgeInsets.fromLTRB(12, 8, 14, 8),
        child: Row(children: [
          Icon(folded ? Icons.chevron_right : Icons.expand_more,
              size: 18, color: t.textDim),
          const SizedBox(width: 6),
          Icon(icon,
              size: 16, color: by == 'section' ? _tintOf(name) : t.textDim),
          const SizedBox(width: 8),
          Text(name,
              style: TextStyle(
                  fontSize: 12.5, fontWeight: FontWeight.w700, color: t.text)),
          const SizedBox(width: 8),
          Text(
            [
              '$count ${count == 1 ? 'folder' : 'folders'}',
              if (unplugged)
                'unplugged'
              else if (missing > 0)
                '$missing missing',
            ].join(' · '),
            style: TextStyle(
                fontSize: 12, color: unplugged ? Tokens.warn : t.textDim),
          ),
        ]),
      ),
    );
  }
}

class _Row extends StatelessWidget {
  const _Row({
    required this.row,
    required this.sections,
    required this.wide,
    required this.ticked,
    required this.selected,
    required this.busy,
    required this.onTick,
    required this.onTap,
  });

  final LibraryRow row;
  final List<String> sections;
  final bool wide;
  final bool ticked;
  final bool selected;
  final bool busy;
  final ValueChanged<bool> onTick;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final l = row;
    final n = l.items.toInt();
    final last = l.lastScan.toInt();
    final dim = !l.exists;
    final (state, tint) = busy
        ? ('Reading', Tokens.brand)
        : !l.exists
            ? ('Missing', Tokens.error)
            : last == 0 && n <= 0
                ? ('Not read', Tokens.warn)
                : ('Watching', Tokens.ok);
    final chips = sections.take(2).toList();
    final more = sections.length - chips.length;
    return Material(
      color:
          selected ? Tokens.brand.withValues(alpha: 0.08) : Colors.transparent,
      child: InkWell(
        onTap: onTap,
        child: Container(
          padding: const EdgeInsets.fromLTRB(6, 3, 14, 3),
          decoration: BoxDecoration(
              border: Border(bottom: BorderSide(color: t.outline))),
          child: Opacity(
            opacity: dim ? 0.6 : 1,
            child: Row(children: [
              Checkbox(
                value: ticked,
                visualDensity: VisualDensity.compact,
                onChanged: (v) => onTick(v ?? false),
              ),
              Icon(l.exists ? Icons.folder_outlined : Icons.folder_off_outlined,
                  size: 18, color: l.exists ? Tokens.warn : Tokens.error),
              const SizedBox(width: 16),
              Expanded(
                child: Tooltip(
                  message: l.path,
                  waitDuration: const Duration(milliseconds: 600),
                  child: Text.rich(
                    TextSpan(children: [
                      TextSpan(
                          text: _nameOf(l.path),
                          style: TextStyle(
                              fontSize: 13,
                              fontWeight: FontWeight.w600,
                              color: t.text)),
                      TextSpan(
                          text: '   ${_parentOf(l.path)}',
                          style: TextStyle(fontSize: 11.5, color: t.textDim)),
                    ]),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                  ),
                ),
              ),
              if (wide)
                SizedBox(
                  width: _wSec,
                  child: Row(children: [
                    for (final s in chips) ...[
                      _SecChip(s),
                      const SizedBox(width: 4),
                    ],
                    if (more > 0)
                      Tooltip(
                        message: sections.skip(2).join(', '),
                        child: Text('+$more',
                            style: TextStyle(fontSize: 11, color: t.textDim)),
                      ),
                  ]),
                ),
              SizedBox(
                width: _wItems,
                child: Text(n >= 0 ? _grouped(n) : '—',
                    textAlign: TextAlign.end,
                    style: TextStyle(
                        fontSize: 12.5,
                        color: t.text,
                        fontFeatures: const [FontFeature.tabularFigures()])),
              ),
              if (wide) ...[
                const SizedBox(width: 16),
                SizedBox(
                  width: _wRead,
                  child: Text(last > 0 ? _since(last) : 'never',
                      style: TextStyle(fontSize: 12, color: t.textDim)),
                ),
              ],
              const SizedBox(width: 8),
              SizedBox(
                width: _wState,
                child: Align(
                  alignment: Alignment.centerLeft,
                  child: StateChip(state, tint: tint),
                ),
              ),
            ]),
          ),
        ),
      ),
    );
  }
}

/// Folders sections keep for themselves: not on the watched list, each
/// section manages its own. Here so every folder Tulipix touches is on one
/// page.
class _Owned extends StatelessWidget {
  const _Owned({required this.rows});

  final List<LibOwnedRow> rows;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SettingsTile(
      icon: Icons.folder_special_outlined,
      tint: Tokens.brand,
      title: 'Folders sections keep for themselves',
      note: 'Not watched here: each is set in its own section',
      child: Column(
        children: [
          for (final r in rows)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 5),
              child: Row(children: [
                Icon(
                  kSectionMeta[kSectionById[r.section]]?.icon ??
                      Icons.folder_outlined,
                  size: 17,
                  color: kSectionById[r.section] == null
                      ? t.textDim
                      : Tokens.accentOf(kSectionById[r.section]!),
                ),
                const SizedBox(width: 10),
                SizedBox(
                  width: 200,
                  child: Text(
                    '${kSectionMeta[kSectionById[r.section]]?.label ?? r.section}'
                    ' · ${r.label}',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 12.5, color: t.text),
                  ),
                ),
                Expanded(
                  child: Text(tildePath(r.path),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 12, color: t.textDim)),
                ),
                if (!r.exists) const StateChip('Not made yet'),
              ]),
            ),
        ],
      ),
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
              Text(
                  known
                      ? '${(frac.clamp(0.0, 1.0) * 100).floor()}%'
                      : 'Working…',
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
