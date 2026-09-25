// Settings → Sections. Which applications this install shows, in what order and
// groups, and which one it opens on.
//
// Tulipix ships seventeen applications and nobody wants all of them. Somebody
// installs it for Photos and Music and will never open Finances; somebody else
// wants Finances and Transfer and no media at all. Home and Settings are always
// there — Home is where the sidebar starts and Settings is the way back — so
// the panel leaves them out.
//
// Three states, not a checkbox. Hiding Finances should take it out of the
// sidebar; turning it *off* should also stop its bill rollover. One switch has
// to conflate those, and then either leaves work running for something you hid
// or silently stops work the moment you tidy your sidebar.
//
// Groups are the sidebar's dividers. A group starts at a section rather than
// holding a list (see `tulipix_core::sections::groups`), so the panel works on
// one flat run of group heads and sections, and a drag is a move in that run.

import 'package:flutter/material.dart';

import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/section_tabs.dart';
import '../../shell/shell_controller.dart';
import '../../shell/sidebar.dart' show kSectionMeta, accentFor;
import '../../src/rust/api/settings.dart';
import '../kitchen/kitchen_page.dart' show askLine, cardDeco, confirm;
import 'settings_controller.dart';

/// Section id → the enum, for the rows the bridge sends.
const Map<String, Section> kSectionById = {
  'home': Section.home,
  'photos': Section.photos,
  'videos': Section.videos,
  'music': Section.music,
  'books': Section.books,
  'cloud': Section.cloud,
  'tools': Section.tools,
  'transfer': Section.transfer,
  'finances': Section.finances,
  'feeds': Section.feeds,
  'journal': Section.journal,
  'kitchen': Section.kitchen,
  'papers': Section.papers,
  'voice': Section.voice,
  'places': Section.places,
  'studio': Section.studio,
  'archive': Section.archive,
  'arcade': Section.arcade,
  'settings': Section.settings,
};

/// What each section does in the background, and what it is for.
///
/// Copy, so it lives on the presenting side — but it is copy that has to be
/// true, because it is the whole basis on which somebody decides between Hidden
/// and Off. Each line names work that actually exists.
const Map<String, (String blurb, String work)> kSectionWork = {
  'home': ('The landing page and its cards', ''),
  'photos': (
    'Timeline, people, places and the editor',
    'folder scans, thumbnails, EXIF, faces, object tags'
  ),
  'videos': (
    'Library, Live TV and the stream browser',
    'folder scans, thumbnails, subtitle fetches'
  ),
  'music': (
    'Library, podcasts, audiobooks, radio, YouTube',
    'library scans, BPM and key, ReplayGain, scrobbling, feed refreshes'
  ),
  'books': ('Shelf, the reader, Genesis', 'folder scans and cover fetches'),
  'cloud': ('Remote drives and the file browser', 'usage polls per remote'),
  'tools': ('The conversion catalogue and its queue', 'the job queue'),
  'transfer': ('Send and receive across the network', 'the network listener'),
  'finances': (
    'Accounts, ledger, bills and planning',
    'recurring and bill rollover'
  ),
  'feeds': (
    'Sites, blogs and newsletters, and the morning brief',
    'feed refreshes while the page is built'
  ),
  'journal': (
    'A private daily log, gathered from the other sections',
    'nothing in the background'
  ),
  'kitchen': (
    'Recipes, cook mode, a week of meals and the shopping list',
    'nothing in the background'
  ),
  'papers': (
    'Receipts, bills, IDs and manuals, read and filed, with reminders',
    'reads new papers and a watched folder while open'
  ),
  'voice': (
    'Voice notes transcribed on this computer, searched, with their tasks',
    'transcribes new notes one at a time while open'
  ),
  'places': (
    'Trips found in your photos, a page per trip, a map and where you have been',
    'nothing in the background'
  ),
  'studio': (
    'Movies and photo books made from your photos and music',
    'renders one project at a time with ffmpeg while open'
  ),
  'archive': (
    'A catalogue of your folders and drives, inside zips too, with duplicates and checks',
    'scans and checksums only when asked'
  ),
  'arcade': (
    'Every game on this computer — Steam, Heroic, Lutris, ROMs — on one shelf',
    'reads the launchers\' files while open'
  ),
  'settings': ('This page', ''),
};

/// Matches `tulipix_core::sections::BONUS`: shipped, off until added. An Off
/// bonus section sits on the shelf rather than in the groups — it was never
/// taken away, it was never added.
const Set<String> kBonus = {'voice', 'places', 'studio', 'archive', 'arcade'};

/// The shelf's tag and what each one leans on.
const Map<String, (String tag, String need)> kBonusNeeds = {
  'voice': (
    'transcribes locally',
    'Uses the whisper model Journal already downloaded, if there is one'
  ),
  'places': ('reads photo locations', 'Needs Photos · works best with Journal'),
  'studio': ('uses ffmpeg', 'Needs Photos · works best with Places and Music'),
  'archive': ('catalogues drives', 'Scans and checksums only when you ask'),
  'arcade': (
    'Steam · Heroic · Lutris',
    'Reads the launchers\' files; nothing is changed in them'
  ),
};

String _mb(int bytes) {
  if (bytes <= 0) return 'no data yet';
  if (bytes < 1024 * 1024) return '${(bytes / 1024).round()} KB';
  if (bytes < 1024 * 1024 * 1024) {
    return '${(bytes / (1024 * 1024)).toStringAsFixed(bytes > 100 * 1024 * 1024 ? 0 : 1)} MB';
  }
  return '${(bytes / (1024 * 1024 * 1024)).toStringAsFixed(1)} GB';
}

String _label(String id) => kSectionMeta[kSectionById[id]]?.label ?? id;
IconData _icon(String id) =>
    kSectionMeta[kSectionById[id]]?.icon ?? Icons.widgets_outlined;
Color _accent(String id) {
  final s = kSectionById[id];
  return s == null ? Tokens.secSettings : accentFor(s);
}

Color _accent2(String id) =>
    Color.lerp(_accent(id), const Color(0xFFEC4899), 0.35)!;

bool _busy(String id) {
  final w = kSectionWork[id]?.$2 ?? '';
  return w.isNotEmpty && !w.startsWith('nothing');
}

String _workShort(String id) =>
    _busy(id) ? kSectionWork[id]!.$2.split(',').first.trim() : 'nothing';

String _nth(int n) => switch (n % 100) {
      11 || 12 || 13 => '${n}th',
      _ => switch (n % 10) {
          1 => '${n}st',
          2 => '${n}nd',
          3 => '${n}rd',
          _ => '${n}th',
        },
    };

/// One entry in the flat run: a group head (`id` is its name) or a section.
typedef _Tok = ({bool head, String id});

/// A group as drawn: its head's index in the run (null for sections above the
/// first head) and the sections in it that pass the filter.
typedef _Group = ({int? head, String name, List<String> ids});

class SettingsSections extends StatefulWidget {
  const SettingsSections({super.key, required this.controller});

  final SettingsController controller;

  @override
  State<SettingsSections> createState() => _SettingsSectionsState();
}

class _SettingsSectionsState extends State<SettingsSections> {
  String? _sel;
  String? _open; // list view: the row showing its tabs
  String _filter = 'all';
  String _query = '';
  bool _presetsOpen = false;
  final _find = TextEditingController();

  @override
  void dispose() {
    _find.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final st = widget.controller.state;
    if (st == null) return const SizedBox.shrink();
    final rows = {for (final r in st.sections) r.id: r};
    // Home and Settings are locked on: not listed, not moved.
    final order = [
      for (final r in st.sections)
        if (!r.locked) r.id
    ];
    final toks = _tokens(order, st.sectionGroups);
    bool added(String id) => !kBonus.contains(id) || rows[id]!.mode != 'off';
    final listed = order.where(added).toList();
    int count(bool Function(SectionRow) f) =>
        listed.where((id) => f(rows[id]!)).length;
    final shown = count((r) => r.mode == 'shown');
    final hidden = count((r) => r.mode == 'hidden');
    final off = count((r) => r.mode == 'off');

    final q = _query.trim().toLowerCase();
    bool pass(String id) {
      final r = rows[id]!;
      final ok = switch (_filter) {
        'shown' || 'hidden' || 'off' => r.mode == _filter,
        'bonus' => kBonus.contains(id),
        'busy' => _busy(id) && r.mode != 'off',
        _ => true,
      };
      return ok &&
          (q.isEmpty ||
              _label(id).toLowerCase().contains(q) ||
              (kSectionWork[id]?.$1 ?? '').toLowerCase().contains(q));
    }

    // Drag only when every section is on screen: a move among filtered rows
    // would land next to ones you cannot see.
    final canDrag = _filter == 'all' && q.isEmpty;
    final groups = _groups(toks, (id) => added(id) && pass(id), canDrag);
    final sel = listed.contains(_sel) ? _sel! : (listed.isEmpty ? null : listed.first);
    final sidebarPos = {
      for (final (i, id)
          in order.where((id) => rows[id]!.mode == 'shown').indexed)
        id: i + 1
    };
    final view = st.sectionsView;
    // Where each section sits in the run: dropping on it puts the dragged one
    // there, just before it.
    final at = {
      for (final (i, t) in toks.indexed)
        if (!t.head) t.id: i
    };

    final main = <Widget>[
      _TitleRow(
        summary: '${listed.length} applications and ${kBonus.length} bonus · '
            '$shown shown · $hidden hidden · $off off. Home and Settings are '
            'always there. Order and groups here are the sidebar\'s.',
        view: view,
        onView: (v) =>
            _apply(SettingsCmd.setText(key: 'sections.view', value: v)),
      ),
      const SizedBox(height: 14),
      _PresetStrip(
        st: st,
        open: _presetsOpen,
        onToggle: () => setState(() => _presetsOpen = !_presetsOpen),
        onPick: (id) {
          setState(() => _presetsOpen = false);
          _apply(SettingsCmd.sectionPreset(name: id));
        },
        onUndo: () => _apply(const SettingsCmd.sectionUndo()),
        onSave: () async {
          final name = await askLine(context, 'Save as a preset', 'Name');
          if (name == null || name.trim().isEmpty) return;
          _apply(SettingsCmd.sectionPresetSave(name: name.trim()));
        },
        onDelete: (name) async {
          if (!await confirm(context, 'Delete “$name”?',
              'The preset goes; the sections stay as they are.')) {
            return;
          }
          _apply(SettingsCmd.sectionPresetDelete(name: name));
        },
      ),
      const SizedBox(height: 14),
      Wrap(
        spacing: 8,
        runSpacing: 8,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          for (final (id, name, n) in [
            ('all', 'All', listed.length),
            ('shown', 'Shown', shown),
            ('hidden', 'Hidden', hidden),
            ('off', 'Off', off),
            ('bonus', 'Bonus', kBonus.length),
            ('busy', 'Busy in background',
                listed
                    .where((id) => _busy(id) && rows[id]!.mode != 'off')
                    .length),
          ])
            _Chip(
              label: name,
              count: n,
              on: _filter == id,
              onTap: () => setState(() => _filter = id),
            ),
          SizedBox(
            width: 200,
            height: 34,
            child: TextField(
              controller: _find,
              onChanged: (v) => setState(() => _query = v),
              style: const TextStyle(fontSize: 13),
              decoration: InputDecoration(
                isDense: true,
                hintText: 'Find a section',
                prefixIcon: const Icon(Icons.search, size: 17),
                border: OutlineInputBorder(
                    borderRadius: BorderRadius.circular(10)),
              ),
            ),
          ),
          TextButton.icon(
            icon: const Icon(Icons.add, size: 16),
            label: const Text('New group'),
            onPressed: () => _newGroup(toks),
          ),
          TextButton.icon(
            icon: const Icon(Icons.restart_alt, size: 16),
            label: const Text('Reset layout'),
            onPressed: () => _apply(const SettingsCmd.sectionLayoutReset()),
          ),
        ],
      ),
      const SizedBox(height: 6),
      if (view == 'list')
        _ListView(
          groups: groups,
          rows: rows,
          sel: sel,
          open: _open,
          canDrag: canDrag,
          at: at,
          onSelect: (id) => setState(() {
            _sel = id;
            _open = _open == id ? null : id;
          }),
          onMode: _setMode,
          onTab: _setTab,
          onDrop: (id, at) => _move(toks, id, at),
          onRename: (i) => _renameGroup(toks, i),
          onDeleteGroup: (i) => _deleteGroup(toks, i),
        )
      else
        for (final g in groups)
          _CardGroup(
            group: g,
            rows: rows,
            sel: sel,
            pos: sidebarPos,
            canDrag: canDrag,
            at: at,
            endAt: _endOf(toks, g.head),
            onSelect: (id) => setState(() => _sel = id),
            onMode: _setMode,
            onDrop: (id, at) => _move(toks, id, at),
            onRename: g.head == null ? null : () => _renameGroup(toks, g.head!),
            onDelete: g.head == null ? null : () => _deleteGroup(toks, g.head!),
          ),
      if (_filter == 'all' || _filter == 'bonus') ...[
        const SizedBox(height: 18),
        const _GroupHead(
          name: 'Bonus sections',
          note: 'off by default · nothing is created until you add one',
        ),
        _BonusShelf(rows: rows, onMode: _setMode),
      ],
      const SizedBox(height: 18),
      _Panel(
        icon: Icons.rocket_launch_outlined,
        title: 'Opens at launch',
        child: Wrap(
          spacing: 7,
          runSpacing: 7,
          children: [
            for (final r in st.sections)
              if (r.mode == 'shown' && r.id != 'settings')
                _Chip(
                  label: _label(r.id),
                  on: st.landing == r.id,
                  onTap: () => _apply(SettingsCmd.sectionLanding(id: r.id)),
                ),
          ],
        ),
      ),
    ];

    final aside = <Widget>[
      if (sel != null)
        _Details(
          row: rows[sel]!,
          pos: sidebarPos[sel],
          group: _groupOf(toks, sel),
          onMode: _setMode,
          onTab: _setTab,
          onUp: () => _step(toks, sel, -1),
          onDown: () => _step(toks, sel, 1),
        ),
      const SizedBox(height: 14),
      _SidebarPreview(
        order: order,
        rows: rows,
        toks: toks,
        dividers: st.sidebarDividers,
      ),
      const SizedBox(height: 14),
      _LongSidebar(
        overflow: st.sidebarOverflow,
        dividers: st.sidebarDividers,
        onOverflow: (v) =>
            _apply(SettingsCmd.setText(key: 'sidebar.overflow', value: v)),
        onDividers: (v) =>
            _apply(SettingsCmd.toggle(key: 'sidebar.dividers', on_: v)),
      ),
    ];

    return LayoutBuilder(builder: (context, box) {
      if (box.maxWidth < 1000) {
        return ListView(
          padding: const EdgeInsets.fromLTRB(4, 4, 4, 32),
          children: [...main, const SizedBox(height: 18), ...aside],
        );
      }
      return Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(
            child: ListView(
              padding: const EdgeInsets.fromLTRB(4, 4, 18, 32),
              children: main,
            ),
          ),
          SizedBox(
            width: 300,
            child: ListView(
              padding: const EdgeInsets.fromLTRB(0, 4, 4, 32),
              children: aside,
            ),
          ),
        ],
      );
    });
  }

  // ── the run of heads and sections ─────────────────────────────────────────

  /// Heads go before the section they start at, in the order they were saved;
  /// a head whose section is gone, or that starts nowhere, goes at the end.
  static List<_Tok> _tokens(List<String> order, List<SectionsGroup> groups) {
    final out = <_Tok>[];
    for (final id in order) {
      for (final g in groups) {
        if (g.anchor == id) out.add((head: true, id: g.name));
      }
      out.add((head: false, id: id));
    }
    for (final g in groups) {
      if (!order.contains(g.anchor)) out.add((head: true, id: g.name));
    }
    return out;
  }

  /// The groups as drawn. Sections above the first head are a nameless group,
  /// drawn only if it has something in it.
  static List<_Group> _groups(
      List<_Tok> toks, bool Function(String) keep, bool keepEmpty) {
    final out = <_Group>[(head: null, name: '', ids: <String>[])];
    for (final (i, t) in toks.indexed) {
      if (t.head) {
        out.add((head: i, name: t.id, ids: <String>[]));
      } else if (keep(t.id)) {
        out.last.ids.add(t.id);
      }
    }
    return [
      for (final g in out)
        if (g.ids.isNotEmpty || (keepEmpty && g.head != null)) g
    ];
  }

  /// Where "the end of this group" is in the run: the next head, or the end.
  static int _endOf(List<_Tok> toks, int? head) {
    for (var i = (head ?? -1) + 1; i < toks.length; i++) {
      if (toks[i].head) return i;
    }
    return toks.length;
  }

  static String? _groupOf(List<_Tok> toks, String id) {
    String? g;
    for (final t in toks) {
      if (t.head) g = t.id;
      if (!t.head && t.id == id) return g;
    }
    return null;
  }

  /// Save the run: the order, and each head anchored at the section after it.
  void _commit(List<_Tok> toks) {
    final groups = <String>[];
    for (final (i, t) in toks.indexed) {
      if (!t.head) continue;
      final next = toks.skip(i + 1).where((x) => !x.head).firstOrNull;
      groups.add('${t.id}=${next?.id ?? ''}');
    }
    _apply(SettingsCmd.sectionLayout(
      ids: ['home', for (final t in toks) if (!t.head) t.id],
      groups: groups,
    ));
  }

  /// Move section [id] to sit at index [at] of the run as it is now.
  void _move(List<_Tok> toks, String id, int at) {
    final next = [...toks];
    final from = next.indexWhere((t) => !t.head && t.id == id);
    if (from < 0) return;
    final t = next.removeAt(from);
    next.insert((from < at ? at - 1 : at).clamp(0, next.length), t);
    _commit(next);
  }

  /// One place up or down; past a head is into the next group.
  void _step(List<_Tok> toks, String id, int by) {
    final from = toks.indexWhere((t) => !t.head && t.id == id);
    if (from < 0) return;
    final at = by < 0 ? from - 1 : from + 2;
    if (at < 0 || at > toks.length) return;
    _move(toks, id, at);
  }

  Future<void> _newGroup(List<_Tok> toks) async {
    final name = await askLine(context, 'New group', 'Name');
    if (name == null || name.trim().isEmpty) return;
    _commit([...toks, (head: true, id: name.trim())]);
  }

  Future<void> _renameGroup(List<_Tok> toks, int i) async {
    final name = await askLine(context, 'Rename group', 'Name',
        initial: toks[i].id);
    if (name == null || name.trim().isEmpty) return;
    _commit([...toks]..[i] = (head: true, id: name.trim()));
  }

  void _deleteGroup(List<_Tok> toks, int i) =>
      _commit([...toks]..removeAt(i));

  // ── sending ───────────────────────────────────────────────────────────────

  /// Send, then tell the shell to re-read itself.
  ///
  /// The sidebar and the pages the shell builds come from the shell's own
  /// snapshot, which is taken when something asks for it — so without this the
  /// setting was saved and nothing on screen moved until the next launch. The
  /// refresh waits for the save: it re-reads the same settings file this
  /// command has just written.
  Future<void> _apply(SettingsCmd cmd) async {
    await widget.controller.send(cmd);
    await ShellController.instance.refresh();
  }

  void _setMode(String id, String mode) =>
      _apply(SettingsCmd.sectionSet(id: id, mode: mode));

  /// One tab inside one section. The last one cannot go: a section with no tab
  /// row is a blank page, and "off" is the switch for not wanting the section.
  void _setTab(SectionRow row, String tab, bool on) {
    final all = sectionTabs[row.id] ?? const <TabEntry>[];
    final off = row.tabsOff.toSet();
    final kept = all.where((e) => !off.contains(e.id)).length;
    if (!on && kept <= 1) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text(
              'Switch ${_label(row.id)} off instead — a section needs one tab.'),
        ),
      );
      return;
    }
    // The live answer every page reads, moved now rather than on the next
    // shell snapshot — which only arrives when something asks the shell.
    setTabOff(row.id, tab, on);
    widget.controller
        .send(SettingsCmd.sectionTab(id: row.id, tab: tab, enabled: on));
  }
}

// ------------------------------------------------------------------- pieces --

class _TitleRow extends StatelessWidget {
  const _TitleRow(
      {required this.summary, required this.view, required this.onView});

  final String summary;
  final String view;
  final void Function(String) onView;

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
              Text(
                'Applications',
                style: TextStyle(
                  fontFamily: context.skin.fontFamily ?? Tokens.fontFamily,
                  fontSize: 22,
                  fontWeight: FontWeight.w700,
                  color: t.text,
                ),
              ),
              const SizedBox(height: 2),
              Text(summary, style: TextStyle(fontSize: 12, color: t.textDim)),
            ],
          ),
        ),
        const SizedBox(width: 12),
        SegmentedButton<String>(
          showSelectedIcon: false,
          segments: const [
            ButtonSegment(
                value: 'cards',
                icon: Icon(Icons.grid_view_rounded, size: 16),
                label: Text('Cards')),
            ButtonSegment(
                value: 'list',
                icon: Icon(Icons.view_list_rounded, size: 16),
                label: Text('List')),
          ],
          selected: {view},
          onSelectionChanged: (v) => onView(v.first),
        ),
      ],
    );
  }
}

/// The active preset on one line; opened, every preset as a tile.
class _PresetStrip extends StatelessWidget {
  const _PresetStrip({
    required this.st,
    required this.open,
    required this.onToggle,
    required this.onPick,
    required this.onUndo,
    required this.onSave,
    required this.onDelete,
  });

  final SettingsState st;
  final bool open;
  final VoidCallback onToggle;
  final void Function(String id) onPick;
  final VoidCallback onUndo;
  final VoidCallback onSave;
  final void Function(String name) onDelete;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final all = st.sectionPresets;
    final exact = all.where((p) => p.id == st.sectionsPreset).firstOrNull;
    final last = all.where((p) => p.id == st.sectionsLastPreset).firstOrNull;
    final active = exact ?? last;
    final changed = exact == null && last != null;
    final shownNow = [
      for (final r in st.sections)
        if (r.mode == 'shown' && !r.locked) r.id
    ];
    return Container(
      decoration: cardDeco(context),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          InkWell(
            onTap: onToggle,
            borderRadius: BorderRadius.circular(14),
            child: Padding(
              padding: const EdgeInsets.fromLTRB(14, 10, 10, 10),
              child: Wrap(
                spacing: 12,
                runSpacing: 8,
                crossAxisAlignment: WrapCrossAlignment.center,
                children: [
                  Row(mainAxisSize: MainAxisSize.min, children: [
                    Icon(Icons.layers_outlined, size: 17, color: t.textDim),
                    const SizedBox(width: 7),
                    Text('Preset',
                        style: TextStyle(
                            fontSize: 13,
                            fontWeight: FontWeight.w700,
                            color: t.textDim)),
                  ]),
                  Container(
                    padding: const EdgeInsets.fromLTRB(12, 5, 12, 5),
                    decoration: BoxDecoration(
                      color: Tokens.secSettings.withValues(alpha: 0.16),
                      borderRadius: BorderRadius.circular(10),
                    ),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Text(active?.name ?? 'Custom',
                            style: TextStyle(
                                fontSize: 13.5,
                                fontWeight: FontWeight.w700,
                                color: t.text)),
                        const SizedBox(width: 10),
                        _Icons(ids: active?.ids ?? shownNow, max: 6),
                        const SizedBox(width: 8),
                        Text(
                            _apps(active?.ids.length ?? shownNow.length),
                            style:
                                TextStyle(fontSize: 12, color: t.textDim)),
                        if (changed) ...[
                          const SizedBox(width: 8),
                          const _Tag('changed', Tokens.warn),
                        ],
                      ],
                    ),
                  ),
                  if (st.sectionsCanUndo)
                    TextButton.icon(
                      icon: const Icon(Icons.undo, size: 16),
                      label: const Text('Undo'),
                      onPressed: onUndo,
                    ),
                  TextButton.icon(
                    icon: AnimatedRotation(
                      turns: open ? 0.5 : 0,
                      duration: const Duration(milliseconds: 180),
                      child: const Icon(Icons.expand_more, size: 18),
                    ),
                    label: const Text('Change preset'),
                    onPressed: onToggle,
                  ),
                ],
              ),
            ),
          ),
          if (open) ...[
            Container(height: 1, color: t.outline),
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 14, 16, 16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                      'One click sets which applications are shown. Nothing '
                      'is deleted, and it can be undone.',
                      style: TextStyle(fontSize: 12, color: t.textDim)),
                  const SizedBox(height: 12),
                  Wrap(
                    spacing: 10,
                    runSpacing: 10,
                    children: [
                      for (final p in all)
                        _PresetTile(
                          name: p.name,
                          hint: p.hint,
                          ids: p.ids,
                          on: p.id == st.sectionsPreset,
                          mine: p.mine,
                          onTap: () => onPick(p.id),
                          onDelete: p.mine ? () => onDelete(p.name) : null,
                        ),
                      _PresetTile(
                        name: 'Save current as a preset',
                        hint: '${shownNow.length} shown now',
                        ids: const [],
                        on: false,
                        add: true,
                        onTap: onSave,
                      ),
                    ],
                  ),
                  const SizedBox(height: 10),
                  Row(children: [
                    const _BonusDot(),
                    const SizedBox(width: 7),
                    Text('A preset with bonus applications adds them too.',
                        style: TextStyle(fontSize: 11.5, color: t.textDim)),
                  ]),
                ],
              ),
            ),
          ],
        ],
      ),
    );
  }
}

String _apps(int n) => n == 1 ? '1 application' : '$n applications';

class _PresetTile extends StatelessWidget {
  const _PresetTile({
    required this.name,
    required this.hint,
    required this.ids,
    required this.on,
    required this.onTap,
    this.mine = false,
    this.add = false,
    this.onDelete,
  });

  final String name;
  final String hint;
  final List<String> ids;
  final bool on;
  final bool mine;
  final bool add;
  final VoidCallback onTap;
  final VoidCallback? onDelete;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final bonus = ids.where(kBonus.contains).length;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(12),
      child: Container(
        width: 176,
        constraints: const BoxConstraints(minHeight: 112),
        padding: const EdgeInsets.fromLTRB(12, 11, 8, 10),
        decoration: BoxDecoration(
          color: on
              ? Tokens.secSettings.withValues(alpha: 0.16)
              : add
                  ? null
                  : t.panel2,
          borderRadius: BorderRadius.circular(12),
          border: Border.all(
            color: on ? Tokens.secSettings : t.outline,
            width: on ? 1.5 : 1,
          ),
        ),
        child: add
            ? Column(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  Icon(Icons.add, size: 18, color: t.textDim),
                  const SizedBox(height: 4),
                  Text(name,
                      textAlign: TextAlign.center,
                      style: TextStyle(
                          fontSize: 12.5,
                          fontWeight: FontWeight.w700,
                          color: t.text)),
                  Text(hint,
                      style: TextStyle(fontSize: 11, color: t.textDim)),
                ],
              )
            : Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(children: [
                    Expanded(
                      child: Text(on ? '$name ✓' : name,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 13,
                              fontWeight: FontWeight.w700,
                              color: t.text)),
                    ),
                    if (onDelete != null)
                      InkWell(
                        onTap: onDelete,
                        child: Icon(Icons.close, size: 15, color: t.textDim),
                      ),
                  ]),
                  Text(hint,
                      style: TextStyle(fontSize: 11, color: t.textDim)),
                  const SizedBox(height: 7),
                  _Icons(ids: ids, max: 7),
                  const SizedBox(height: 7),
                  Text(
                    [
                      _apps(ids.length),
                      if (bonus > 0) '$bonus bonus',
                      if (mine) 'yours',
                    ].join(' · '),
                    style: TextStyle(
                        fontSize: 11,
                        fontWeight: FontWeight.w600,
                        color: t.textDim),
                  ),
                ],
              ),
      ),
    );
  }
}

/// A row of small section icons, bonus ones marked.
class _Icons extends StatelessWidget {
  const _Icons({required this.ids, required this.max});

  final List<String> ids;
  final int max;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final more = ids.length - max;
    return Wrap(
      spacing: 3,
      runSpacing: 3,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        for (final id in ids.take(max))
          Stack(
            clipBehavior: Clip.none,
            children: [
              Container(
                width: 22,
                height: 22,
                decoration: BoxDecoration(
                  color: _accent(id).withValues(alpha: 0.2),
                  borderRadius: BorderRadius.circular(7),
                ),
                child: Icon(_icon(id), size: 13, color: _accent(id)),
              ),
              if (kBonus.contains(id))
                const Positioned(top: -2, right: -2, child: _BonusDot()),
            ],
          ),
        if (more > 0)
          Text(' +$more',
              style: TextStyle(
                  fontSize: 10.5,
                  fontWeight: FontWeight.w700,
                  color: t.textDim)),
      ],
    );
  }
}

class _BonusDot extends StatelessWidget {
  const _BonusDot();

  @override
  Widget build(BuildContext context) => Container(
        width: 7,
        height: 7,
        decoration: const BoxDecoration(
          color: Tokens.warn,
          shape: BoxShape.circle,
        ),
      );
}

class _Tag extends StatelessWidget {
  const _Tag(this.label, this.color);

  final String label;
  final Color color;

  @override
  Widget build(BuildContext context) => Container(
        padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 2),
        decoration: BoxDecoration(
          color: color.withValues(alpha: 0.16),
          borderRadius: BorderRadius.circular(99),
        ),
        child: Text(label,
            style: TextStyle(
                fontSize: 10.5, fontWeight: FontWeight.w800, color: color)),
      );
}

class _Chip extends StatelessWidget {
  const _Chip(
      {required this.label, this.count, required this.on, required this.onTap});

  final String label;
  final int? count;
  final bool on;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(99),
      child: Container(
        height: 30,
        padding: const EdgeInsets.symmetric(horizontal: 12),
        decoration: BoxDecoration(
          color: on ? Tokens.secSettings.withValues(alpha: 0.17) : t.panel2,
          borderRadius: BorderRadius.circular(99),
          border: Border.all(color: on ? Tokens.secSettings : t.outline),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(label,
                style: TextStyle(
                    fontSize: 12.5,
                    fontWeight: FontWeight.w600,
                    color: on ? t.text : t.textDim)),
            if (count != null) ...[
              const SizedBox(width: 6),
              Text('$count',
                  style: TextStyle(fontSize: 11, color: t.textDim)),
            ],
          ],
        ),
      ),
    );
  }
}

class _GroupHead extends StatelessWidget {
  const _GroupHead({
    required this.name,
    this.note,
    this.onRename,
    this.onDelete,
  });

  final String name;
  final String? note;
  final VoidCallback? onRename;
  final VoidCallback? onDelete;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.fromLTRB(2, 14, 2, 9),
      child: Row(
        children: [
          Text(
            name.toUpperCase(),
            style: TextStyle(
              fontSize: 11,
              fontWeight: FontWeight.w700,
              letterSpacing: 0.9,
              color: t.textDim,
            ),
          ),
          const SizedBox(width: 10),
          Expanded(child: Container(height: 1, color: t.outline)),
          if (note != null) ...[
            const SizedBox(width: 10),
            Text(note!, style: TextStyle(fontSize: 11.5, color: t.textDim)),
          ],
          if (onRename != null)
            IconButton(
              tooltip: 'Rename group',
              iconSize: 15,
              visualDensity: VisualDensity.compact,
              icon: Icon(Icons.edit_outlined, color: t.textDim),
              onPressed: onRename,
            ),
          if (onDelete != null)
            IconButton(
              tooltip: 'Remove group — its sections join the one above',
              iconSize: 15,
              visualDensity: VisualDensity.compact,
              icon: Icon(Icons.close, color: t.textDim),
              onPressed: onDelete,
            ),
        ],
      ),
    );
  }
}

/// Drop a dragged section here to put it at [at] in the run.
class _Drop extends StatelessWidget {
  const _Drop({required this.at, required this.onDrop, required this.child});

  final int at;
  final void Function(String id, int at) onDrop;
  final Widget Function(bool over) child;

  @override
  Widget build(BuildContext context) => DragTarget<String>(
        onAcceptWithDetails: (d) => onDrop(d.data, at),
        builder: (context, cand, _) => child(cand.isNotEmpty),
      );
}

/// [child] as something that can be dragged, when dragging is allowed.
Widget _draggable(
    {required String id,
    required bool enabled,
    required double width,
    required Widget child}) {
  if (!enabled) return child;
  return Draggable<String>(
    data: id,
    feedback: Material(
      color: Colors.transparent,
      child: SizedBox(width: width, child: Opacity(opacity: 0.85, child: child)),
    ),
    childWhenDragging: Opacity(opacity: 0.3, child: child),
    child: child,
  );
}

// ── cards ───────────────────────────────────────────────────────────────────

class _CardGroup extends StatelessWidget {
  const _CardGroup({
    required this.group,
    required this.rows,
    required this.sel,
    required this.pos,
    required this.canDrag,
    required this.at,
    required this.endAt,
    required this.onSelect,
    required this.onMode,
    required this.onDrop,
    this.onRename,
    this.onDelete,
  });

  final _Group group;
  final Map<String, SectionRow> rows;
  final String? sel;
  final Map<String, int> pos;
  final bool canDrag;
  final Map<String, int> at;
  final int endAt;
  final void Function(String) onSelect;
  final void Function(String id, String mode) onMode;
  final void Function(String id, int at) onDrop;
  final VoidCallback? onRename;
  final VoidCallback? onDelete;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final head = group.head;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (head != null)
          canDrag
              ? _Drop(
                  at: head + 1,
                  onDrop: onDrop,
                  child: (over) => Container(
                    color: over
                        ? Tokens.secSettings.withValues(alpha: 0.08)
                        : null,
                    child: _GroupHead(
                        name: group.name,
                        onRename: onRename,
                        onDelete: onDelete),
                  ),
                )
              : _GroupHead(name: group.name),
        LayoutBuilder(builder: (context, box) {
          const gap = 12.0;
          final cols = ((box.maxWidth + gap) / (205 + gap)).floor().clamp(1, 8);
          final w = (box.maxWidth - gap * (cols - 1)) / cols;
          return Wrap(
            spacing: gap,
            runSpacing: gap,
            children: [
              for (final id in group.ids)
                SizedBox(
                  width: w,
                  child: _Drop(
                    at: at[id]!,
                    onDrop: onDrop,
                    child: (over) => _draggable(
                      id: id,
                      enabled: canDrag,
                      width: w,
                      child: _Card(
                        row: rows[id]!,
                        pos: pos[id],
                        selected: sel == id,
                        over: over,
                        grip: canDrag,
                        onTap: () => onSelect(id),
                        onMode: (m) => onMode(id, m),
                      ),
                    ),
                  ),
                ),
              if (canDrag)
                SizedBox(
                  width: w,
                  height: 64,
                  child: _Drop(
                    at: endAt,
                    onDrop: onDrop,
                    child: (over) => _DropBox(
                      over: over,
                      child: Text('Drop a section here',
                          style: TextStyle(
                              fontSize: 12,
                              fontWeight: FontWeight.w700,
                              color: over ? Tokens.secSettings : t.textDim)),
                    ),
                  ),
                ),
            ],
          );
        }),
      ],
    );
  }
}

class _DropBox extends StatelessWidget {
  const _DropBox({required this.over, required this.child});

  final bool over;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: over ? Tokens.secSettings.withValues(alpha: 0.08) : null,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
          color: over ? Tokens.secSettings : t.outlineStrong,
          width: 1.5,
        ),
      ),
      child: child,
    );
  }
}

class _Card extends StatelessWidget {
  const _Card({
    required this.row,
    required this.pos,
    required this.selected,
    required this.over,
    required this.grip,
    required this.onTap,
    required this.onMode,
  });

  final SectionRow row;
  final int? pos;
  final bool selected;
  final bool over;
  final bool grip;
  final VoidCallback onTap;
  final void Function(String) onMode;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final id = row.id;
    final c = _accent(id);
    final tabs = sectionTabs[id];
    final kept = tabs?.where((e) => !row.tabsOff.contains(e.id)).length;
    final off = row.mode == 'off';
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(14),
      child: Container(
        padding: const EdgeInsets.fromLTRB(14, 14, 14, 12),
        foregroundDecoration: selected || over
            ? BoxDecoration(
                borderRadius: BorderRadius.circular(14),
                border: Border.all(
                    color: over ? Tokens.secSettings : c, width: 2),
              )
            : null,
        decoration: cardDeco(context),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                if (grip)
                  Padding(
                    padding: const EdgeInsets.only(right: 6),
                    child: Icon(Icons.drag_indicator,
                        size: 16, color: t.textDim),
                  ),
                Opacity(
                  opacity: off ? 0.5 : 1,
                  child: Container(
                    width: 42,
                    height: 42,
                    decoration: BoxDecoration(
                      gradient: off
                          ? null
                          : LinearGradient(colors: [c, _accent2(id)]),
                      color: off ? t.textDim : null,
                      borderRadius: BorderRadius.circular(13),
                    ),
                    child: Icon(_icon(id), size: 21, color: Colors.white),
                  ),
                ),
                const Spacer(),
                switch (row.mode) {
                  'hidden' => const _Tag('Hidden', Tokens.warn),
                  'off' => const _Tag('Off', Tokens.error),
                  _ => Text(
                      pos == null ? '—' : pos!.toString().padLeft(2, '0'),
                      style: TextStyle(
                          fontSize: 11,
                          fontWeight: FontWeight.w700,
                          color: t.textDim),
                    ),
                },
              ],
            ),
            const SizedBox(height: 9),
            Opacity(
              opacity: off ? 0.6 : 1,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(_label(id),
                      style: TextStyle(
                          fontSize: 14.5,
                          fontWeight: FontWeight.w700,
                          color: t.text)),
                  const SizedBox(height: 3),
                  SizedBox(
                    height: 34,
                    child: Text(
                      switch (row.mode) {
                        'hidden' =>
                          'Hidden from the sidebar; ${_workShort(id)} still runs',
                        'off' => 'Off: hidden and stopped, its data kept',
                        _ => kSectionWork[id]?.$1 ?? '',
                      },
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 12, height: 1.4, color: t.textDim),
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(height: 9),
            Wrap(
              spacing: 5,
              runSpacing: 5,
              children: [
                _Fact(row.bytes > 0 && off
                    ? '${_mb(row.bytes)} kept'
                    : _mb(row.bytes)),
                if (!off) _Fact(_workShort(id)),
                if (kept != null) _Fact('$kept/${tabs!.length} tabs'),
              ],
            ),
            const SizedBox(height: 10),
            _Seg(value: row.mode, enabled: true, expand: true, onPick: onMode),
          ],
        ),
      ),
    );
  }
}

// ── list ────────────────────────────────────────────────────────────────────

class _ListView extends StatelessWidget {
  const _ListView({
    required this.groups,
    required this.rows,
    required this.sel,
    required this.open,
    required this.canDrag,
    required this.at,
    required this.onSelect,
    required this.onMode,
    required this.onTab,
    required this.onDrop,
    required this.onRename,
    required this.onDeleteGroup,
  });

  final List<_Group> groups;
  final Map<String, SectionRow> rows;
  final String? sel;
  final String? open;
  final bool canDrag;
  final Map<String, int> at;
  final void Function(String) onSelect;
  final void Function(String id, String mode) onMode;
  final void Function(SectionRow, String, bool) onTab;
  final void Function(String id, int at) onDrop;
  final void Function(int head) onRename;
  final void Function(int head) onDeleteGroup;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final dim = TextStyle(
        fontSize: 10.5,
        fontWeight: FontWeight.w700,
        letterSpacing: 0.6,
        color: t.textDim);
    return LayoutBuilder(builder: (context, box) {
      final wide = box.maxWidth > 760;
      return Container(
        clipBehavior: Clip.antiAlias,
        decoration: cardDeco(context),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(70, 9, 40, 9),
              child: Row(children: [
                Expanded(flex: 3, child: Text('APPLICATION', style: dim)),
                if (wide) ...[
                  Expanded(flex: 2, child: Text('BACKGROUND', style: dim)),
                  SizedBox(width: 70, child: Text('DISK', style: dim)),
                  SizedBox(width: 50, child: Text('TABS', style: dim)),
                ],
                SizedBox(width: 190, child: Text('STATE', style: dim)),
              ]),
            ),
            for (final g in groups) ...[
              if (g.head != null)
                _Drop(
                  at: g.head! + 1,
                  onDrop: onDrop,
                  child: (over) => Container(
                    padding: const EdgeInsets.fromLTRB(14, 0, 4, 0),
                    color: over
                        ? Tokens.secSettings.withValues(alpha: 0.12)
                        : t.panel2,
                    child: Row(children: [
                      Expanded(
                          child: Text(g.name.toUpperCase(),
                              style: dim.copyWith(fontSize: 11))),
                      if (canDrag) ...[
                        IconButton(
                          tooltip: 'Rename group',
                          iconSize: 15,
                          visualDensity: VisualDensity.compact,
                          icon: Icon(Icons.edit_outlined, color: t.textDim),
                          onPressed: () => onRename(g.head!),
                        ),
                        IconButton(
                          tooltip: 'Remove group',
                          iconSize: 15,
                          visualDensity: VisualDensity.compact,
                          icon: Icon(Icons.close, color: t.textDim),
                          onPressed: () => onDeleteGroup(g.head!),
                        ),
                      ],
                    ]),
                  ),
                ),
              for (final id in g.ids)
                _Drop(
                  at: at[id]!,
                  onDrop: onDrop,
                  child: (over) => _draggable(
                    id: id,
                    enabled: canDrag,
                    width: box.maxWidth,
                    child: _ListRow(
                      row: rows[id]!,
                      wide: wide,
                      grip: canDrag,
                      selected: sel == id,
                      over: over,
                      open: open == id,
                      onTap: () => onSelect(id),
                      onMode: (m) => onMode(id, m),
                      onTab: onTab,
                    ),
                  ),
                ),
            ],
          ],
        ),
      );
    });
  }
}

class _ListRow extends StatelessWidget {
  const _ListRow({
    required this.row,
    required this.wide,
    required this.grip,
    required this.selected,
    required this.over,
    required this.open,
    required this.onTap,
    required this.onMode,
    required this.onTab,
  });

  final SectionRow row;
  final bool wide;
  final bool grip;
  final bool selected;
  final bool over;
  final bool open;
  final VoidCallback onTap;
  final void Function(String) onMode;
  final void Function(SectionRow, String, bool) onTab;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final id = row.id;
    final c = _accent(id);
    final off = row.mode == 'off';
    final tabs = sectionTabs[id];
    final kept = tabs?.where((e) => !row.tabsOff.contains(e.id)).length;
    final num = TextStyle(
        fontSize: 12,
        color: t.textDim,
        fontFeatures: const [FontFeature.tabularFigures()]);
    return Material(
      color: over
          ? Tokens.secSettings.withValues(alpha: 0.12)
          : selected
              ? Tokens.secSettings.withValues(alpha: 0.07)
              : Colors.transparent,
      child: InkWell(
        onTap: onTap,
        child: Container(
          decoration: BoxDecoration(
              border: Border(top: BorderSide(color: t.outline))),
          padding: const EdgeInsets.fromLTRB(14, 9, 14, 9),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  SizedBox(
                    width: 16,
                    child: grip
                        ? Icon(Icons.drag_indicator,
                            size: 16, color: t.textDim)
                        : null,
                  ),
                  const SizedBox(width: 10),
                  Opacity(
                    opacity: off ? 0.5 : 1,
                    child: Container(
                      width: 30,
                      height: 30,
                      decoration: BoxDecoration(
                        color: c.withValues(alpha: 0.17),
                        borderRadius: BorderRadius.circular(9),
                      ),
                      child: Icon(_icon(id), size: 17, color: c),
                    ),
                  ),
                  const SizedBox(width: 10),
                  Expanded(
                    flex: 3,
                    child: Opacity(
                      opacity: off ? 0.6 : 1,
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(_label(id),
                              style: TextStyle(
                                  fontSize: 13,
                                  fontWeight: FontWeight.w700,
                                  color: t.text)),
                          Text(
                            switch (row.mode) {
                              'hidden' =>
                                'Hidden: ${_workShort(id)} still runs',
                              'off' => 'Off: hidden and stopped, data kept',
                              _ => kSectionWork[id]?.$1 ?? '',
                            },
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontSize: 11.5,
                                color: row.mode == 'hidden'
                                    ? Tokens.warn
                                    : t.textDim),
                          ),
                        ],
                      ),
                    ),
                  ),
                  if (wide) ...[
                    Expanded(
                      flex: 2,
                      child: Text(
                        off ? 'stopped' : (kSectionWork[id]?.$2 ?? ''),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 12, color: t.textDim),
                      ),
                    ),
                    SizedBox(
                        width: 70,
                        child: Text(row.bytes > 0 ? _mb(row.bytes) : '—',
                            style: num)),
                    SizedBox(
                        width: 50,
                        child: Text(
                            kept == null ? '—' : '$kept / ${tabs!.length}',
                            style: num)),
                  ],
                  SizedBox(
                    width: 170,
                    child: _Seg(
                        value: row.mode,
                        enabled: true,
                        expand: true,
                        onPick: onMode),
                  ),
                  const SizedBox(width: 4),
                  AnimatedRotation(
                    turns: open ? 0.5 : 0,
                    duration: const Duration(milliseconds: 160),
                    child: Icon(Icons.expand_more, size: 18, color: t.textDim),
                  ),
                ],
              ),
              if (open)
                Padding(
                  padding: const EdgeInsets.fromLTRB(66, 10, 0, 4),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      _TabChips(row: row, onTab: onTab),
                      const SizedBox(height: 6),
                      Text(
                        [
                          if (row.db.isNotEmpty) row.db,
                          if (kept != null) '$kept of ${tabs!.length} tabs kept',
                          if (tabs != null) 'click a tab to take it away',
                        ].join(' · '),
                        style: TextStyle(fontSize: 11, color: t.textDim),
                      ),
                    ],
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

// ── bonus shelf ─────────────────────────────────────────────────────────────

class _BonusShelf extends StatelessWidget {
  const _BonusShelf({required this.rows, required this.onMode});

  final Map<String, SectionRow> rows;
  final void Function(String id, String mode) onMode;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return LayoutBuilder(builder: (context, box) {
      const gap = 12.0;
      final cols = ((box.maxWidth + gap) / (240 + gap)).floor().clamp(1, 6);
      final w = (box.maxWidth - gap * (cols - 1)) / cols;
      return Wrap(
        spacing: gap,
        runSpacing: gap,
        children: [
          for (final id in kBonus)
            if (rows[id] case final r?)
              SizedBox(
                width: w,
                child: Container(
                  clipBehavior: Clip.antiAlias,
                  foregroundDecoration: r.mode != 'off'
                      ? BoxDecoration(
                          borderRadius: BorderRadius.circular(14),
                          border: Border.all(color: _accent(id), width: 2),
                        )
                      : null,
                  decoration: cardDeco(context),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      Container(
                        height: 78,
                        padding: const EdgeInsets.fromLTRB(12, 10, 10, 10),
                        decoration: BoxDecoration(
                          gradient: LinearGradient(
                            begin: Alignment.topLeft,
                            end: Alignment.bottomRight,
                            colors: [_accent(id), _accent2(id)],
                          ),
                        ),
                        child: Stack(
                          children: [
                            Align(
                              alignment: Alignment.topRight,
                              child: Container(
                                padding: const EdgeInsets.symmetric(
                                    horizontal: 8, vertical: 3),
                                decoration: BoxDecoration(
                                  color: Colors.black.withValues(alpha: 0.28),
                                  borderRadius: BorderRadius.circular(6),
                                ),
                                child: Text(
                                  r.mode != 'off'
                                      ? '✓ added'
                                      : kBonusNeeds[id]!.$1,
                                  style: const TextStyle(
                                      fontSize: 11,
                                      fontWeight: FontWeight.w700,
                                      color: Colors.white),
                                ),
                              ),
                            ),
                            Align(
                              alignment: Alignment.bottomLeft,
                              child: Row(children: [
                                Icon(_icon(id), size: 26, color: Colors.white),
                                const SizedBox(width: 10),
                                Text(_label(id),
                                    style: const TextStyle(
                                        fontSize: 17,
                                        fontWeight: FontWeight.w700,
                                        color: Colors.white)),
                              ]),
                            ),
                          ],
                        ),
                      ),
                      Padding(
                        padding: const EdgeInsets.fromLTRB(14, 12, 14, 14),
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(kSectionWork[id]?.$1 ?? '',
                                style: TextStyle(
                                    fontSize: 12,
                                    height: 1.45,
                                    color: t.textDim)),
                            const SizedBox(height: 9),
                            Row(
                              crossAxisAlignment: CrossAxisAlignment.start,
                              children: [
                                Icon(Icons.info_outline,
                                    size: 13, color: t.textDim),
                                const SizedBox(width: 6),
                                Expanded(
                                  child: Text(kBonusNeeds[id]!.$2,
                                      style: TextStyle(
                                          fontSize: 11, color: t.textDim)),
                                ),
                              ],
                            ),
                            const SizedBox(height: 11),
                            if (r.mode == 'off')
                              FilledButton.icon(
                                style: FilledButton.styleFrom(
                                    backgroundColor: _accent(id)),
                                icon: const Icon(Icons.add, size: 16),
                                label: const Text('Add'),
                                onPressed: () => onMode(id, 'shown'),
                              )
                            else ...[
                              _Seg(
                                  value: r.mode,
                                  enabled: true,
                                  expand: true,
                                  onPick: (m) => onMode(id, m)),
                              const SizedBox(height: 6),
                              Text(
                                  'In the list above — drag it to another '
                                  'group. Off puts it back on the shelf.',
                                  style: TextStyle(
                                      fontSize: 11, color: t.textDim)),
                            ],
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
              ),
        ],
      );
    });
  }
}

// ── the side panel ──────────────────────────────────────────────────────────

class _Panel extends StatelessWidget {
  const _Panel({required this.icon, required this.title, required this.child});

  final IconData icon;
  final String title;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(14),
      decoration: cardDeco(context),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(children: [
            Icon(icon, size: 15, color: t.textDim),
            const SizedBox(width: 8),
            Text(title.toUpperCase(),
                style: TextStyle(
                    fontSize: 11.5,
                    fontWeight: FontWeight.w700,
                    letterSpacing: 0.7,
                    color: t.textDim)),
          ]),
          const SizedBox(height: 12),
          child,
        ],
      ),
    );
  }
}

class _Details extends StatelessWidget {
  const _Details({
    required this.row,
    required this.pos,
    required this.group,
    required this.onMode,
    required this.onTab,
    required this.onUp,
    required this.onDown,
  });

  final SectionRow row;
  final int? pos;
  final String? group;
  final void Function(String id, String mode) onMode;
  final void Function(SectionRow, String, bool) onTab;
  final VoidCallback onUp;
  final VoidCallback onDown;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final id = row.id;
    final section = kSectionById[id];
    Widget kv(String k, String v) => Container(
          padding: const EdgeInsets.symmetric(vertical: 6),
          decoration:
              BoxDecoration(border: Border(top: BorderSide(color: t.outline))),
          child: Row(children: [
            Text(k, style: TextStyle(fontSize: 12, color: t.textDim)),
            const SizedBox(width: 10),
            Expanded(
              child: Text(v,
                  textAlign: TextAlign.right,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 12,
                      fontWeight: FontWeight.w700,
                      color: t.text)),
            ),
          ]),
        );
    return _Panel(
      icon: Icons.touch_app_outlined,
      title: 'Selected',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(children: [
            Container(
              width: 44,
              height: 44,
              decoration: BoxDecoration(
                gradient:
                    LinearGradient(colors: [_accent(id), _accent2(id)]),
                borderRadius: BorderRadius.circular(13),
              ),
              child: Icon(_icon(id), size: 22, color: Colors.white),
            ),
            const SizedBox(width: 11),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(_label(id),
                      style: TextStyle(
                          fontSize: 16,
                          fontWeight: FontWeight.w700,
                          color: t.text)),
                  Text(kSectionWork[id]?.$1 ?? '',
                      style: TextStyle(fontSize: 11, color: t.textDim)),
                ],
              ),
            ),
          ]),
          const SizedBox(height: 12),
          _Seg(
              value: row.mode,
              enabled: true,
              expand: true,
              onPick: (m) => onMode(id, m)),
          if (sectionTabs[id] != null) ...[
            const SizedBox(height: 12),
            Text('Tabs',
                style: TextStyle(
                    fontSize: 12, fontWeight: FontWeight.w700, color: t.text)),
            const SizedBox(height: 6),
            _TabChips(row: row, onTab: onTab),
          ],
          const SizedBox(height: 12),
          kv(
              'Sidebar',
              pos == null
                  ? 'not in the sidebar'
                  : [_nth(pos!), if (group != null) 'in $group'].join(' ')),
          kv('Database', row.db.isEmpty ? 'none' : row.db),
          kv('On disk', row.bytes > 0 ? _mb(row.bytes) : '—'),
          kv('In the background',
              row.mode == 'off' ? 'stopped' : _workShort(id)),
          const SizedBox(height: 10),
          Row(children: [
            IconButton.outlined(
              tooltip: 'Move up',
              iconSize: 16,
              icon: const Icon(Icons.arrow_upward),
              onPressed: onUp,
            ),
            const SizedBox(width: 6),
            IconButton.outlined(
              tooltip: 'Move down',
              iconSize: 16,
              icon: const Icon(Icons.arrow_downward),
              onPressed: onDown,
            ),
            const Spacer(),
            if (section != null && row.mode == 'shown')
              TextButton(
                onPressed: () => ShellController.instance.go(section),
                child: const Text('Open →'),
              ),
          ]),
          const SizedBox(height: 6),
          Text(
            row.bytes > 0
                ? 'Turning a section off deletes nothing: its '
                    '${_mb(row.bytes)} stays where it is.'
                : 'Turning a section off deletes nothing.',
            style: TextStyle(fontSize: 11, color: t.textDim),
          ),
        ],
      ),
    );
  }
}

/// The sidebar as these settings will draw it.
class _SidebarPreview extends StatelessWidget {
  const _SidebarPreview({
    required this.order,
    required this.rows,
    required this.toks,
    required this.dividers,
  });

  final List<String> order;
  final Map<String, SectionRow> rows;
  final List<_Tok> toks;
  final bool dividers;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // A head shows on the first shown section after it — the same rule as
    // `tulipix_core::sections::sidebar_groups`.
    final lines = <Widget>[];
    String? pending;
    Widget line(String id, {bool home = false}) => SizedBox(
          height: 26,
          child: Row(children: [
            Container(
              width: 24,
              height: 22,
              decoration: BoxDecoration(
                color: home ? Tokens.secSettings.withValues(alpha: 0.18) : null,
                borderRadius: BorderRadius.circular(7),
              ),
              child: Icon(_icon(id), size: 14, color: _accent(id)),
            ),
            const SizedBox(width: 9),
            Text(_label(id), style: TextStyle(fontSize: 11.5, color: t.text)),
          ]),
        );
    lines.add(line('home', home: true));
    for (final tk in toks) {
      if (tk.head) {
        pending = tk.id;
        continue;
      }
      if (rows[tk.id]?.mode != 'shown') continue;
      if (dividers && pending != null) {
        lines.add(Padding(
          padding: const EdgeInsets.fromLTRB(2, 6, 0, 3),
          child: Text(pending.toUpperCase(),
              style: TextStyle(
                  fontSize: 9,
                  fontWeight: FontWeight.w800,
                  letterSpacing: 1,
                  color: t.textDim)),
        ));
        pending = null;
      }
      lines.add(line(tk.id));
    }
    final hiddenNames = [
      for (final id in order)
        if (rows[id]?.mode == 'hidden') _label(id)
    ];
    return _Panel(
      icon: Icons.view_sidebar_outlined,
      title: 'Sidebar preview',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            padding: const EdgeInsets.all(10),
            decoration: BoxDecoration(
              color: t.panel2,
              borderRadius: BorderRadius.circular(12),
              border: Border.all(color: t.outline),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: lines,
            ),
          ),
          if (hiddenNames.isNotEmpty) ...[
            const SizedBox(height: 8),
            Text('Hidden, still working: ${hiddenNames.join(', ')}',
                style: TextStyle(fontSize: 11, color: t.textDim)),
          ],
        ],
      ),
    );
  }
}

class _LongSidebar extends StatelessWidget {
  const _LongSidebar({
    required this.overflow,
    required this.dividers,
    required this.onOverflow,
    required this.onDividers,
  });

  final String overflow;
  final bool dividers;
  final void Function(String) onOverflow;
  final void Function(bool) onDividers;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return _Panel(
      icon: Icons.height,
      title: 'Long sidebars',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text("When the sidebar doesn't fit the window:",
              style: TextStyle(fontSize: 12, color: t.textDim)),
          const SizedBox(height: 8),
          Wrap(
            spacing: 7,
            runSpacing: 7,
            children: [
              for (final (id, name) in const [
                ('shrink', 'Smaller rows'),
                ('scroll', 'Scroll'),
                ('more', 'Fold the rest into “More”'),
              ])
                _Chip(
                  label: name,
                  on: overflow == id,
                  onTap: () => onOverflow(id),
                ),
            ],
          ),
          const SizedBox(height: 10),
          Row(children: [
            Expanded(
              child: Text('Show group dividers',
                  style: TextStyle(fontSize: 12.5, color: t.text)),
            ),
            Switch(value: dividers, onChanged: onDividers),
          ]),
        ],
      ),
    );
  }
}

class _TabChips extends StatelessWidget {
  const _TabChips({required this.row, required this.onTab});

  final SectionRow row;
  final void Function(SectionRow, String, bool) onTab;

  @override
  Widget build(BuildContext context) {
    final all = sectionTabs[row.id] ?? const <TabEntry>[];
    final off = row.tabsOff.toSet();
    return Wrap(
      spacing: 6,
      runSpacing: 6,
      children: [
        for (final e in all)
          _TabChip(
            label: e.label,
            on: !off.contains(e.id),
            onTap: () => onTab(row, e.id, off.contains(e.id)),
          ),
      ],
    );
  }
}

class _Seg extends StatelessWidget {
  const _Seg({
    required this.value,
    required this.enabled,
    required this.onPick,
    this.expand = false,
  });

  final String value;
  final bool enabled;
  final void Function(String) onPick;

  /// Share the width three ways, as on a card, rather than hug the labels.
  final bool expand;

  static const List<(String, String)> _modes = [
    ('shown', 'Shown'),
    ('hidden', 'Hidden'),
    ('off', 'Off'),
  ];

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget seg(String id, String label) => InkWell(
          onTap: enabled && id != value ? () => onPick(id) : null,
          borderRadius: BorderRadius.circular(8),
          child: Container(
            height: 26,
            padding: const EdgeInsets.symmetric(horizontal: 11),
            alignment: Alignment.center,
            decoration: id == value
                ? BoxDecoration(
                    color: t.panel,
                    borderRadius: BorderRadius.circular(8),
                    boxShadow: const [
                      BoxShadow(color: Color(0x33000000), blurRadius: 3),
                    ],
                  )
                : null,
            child: Text(
              label,
              style: TextStyle(
                fontSize: 11.5,
                fontWeight: FontWeight.w700,
                color: id != value
                    ? t.textDim
                    : switch (id) {
                        'hidden' => Tokens.warn,
                        'off' => Tokens.error,
                        _ => t.text,
                      },
              ),
            ),
          ),
        );
    return Opacity(
      opacity: enabled ? 1 : 0.5,
      child: Container(
        padding: const EdgeInsets.all(3),
        decoration: BoxDecoration(
          color: t.panel2,
          borderRadius: BorderRadius.circular(10),
        ),
        child: Row(
          mainAxisSize: expand ? MainAxisSize.max : MainAxisSize.min,
          children: [
            for (final (id, label) in _modes)
              expand ? Expanded(child: seg(id, label)) : seg(id, label),
          ],
        ),
      ),
    );
  }
}

class _TabChip extends StatelessWidget {
  const _TabChip({required this.label, required this.on, required this.onTap});

  final String label;
  final bool on;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      borderRadius: BorderRadius.circular(999),
      onTap: onTap,
      child: Container(
        height: 27,
        padding: const EdgeInsets.fromLTRB(8, 0, 11, 0),
        decoration: BoxDecoration(
          color: on ? t.panel2 : Colors.transparent,
          borderRadius: BorderRadius.circular(999),
          border: Border.all(color: t.outline),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(
              on ? Icons.check : Icons.remove,
              size: 13,
              color: on ? Tokens.ok : t.textDim,
            ),
            const SizedBox(width: 5),
            Text(
              label,
              style: TextStyle(
                fontSize: 11.5,
                color: on ? t.text : t.textDim,
                decoration: on ? null : TextDecoration.lineThrough,
                decorationColor: t.textDim,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _Fact extends StatelessWidget {
  const _Fact(this.label);

  final String label;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
      decoration: BoxDecoration(
        color: t.panel2,
        borderRadius: BorderRadius.circular(6),
      ),
      child: Text(
        label,
        style: TextStyle(
            fontSize: 10.5, fontWeight: FontWeight.w600, color: t.textDim),
      ),
    );
  }
}
