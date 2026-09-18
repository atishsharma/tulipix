// Settings → Sections. Which of the ten this install shows, in what order, and
// which one it opens on.
//
// Tulipix ships ten sections and nobody wants all ten. Somebody installs it for
// Photos and Music and will never open Finances; somebody else wants Finances
// and Transfer and no media at all. The sidebar was a const list and the shell
// an IndexedStack keyed on the Section ordinal, so every page was built at
// launch whether or not it was ever looked at.
//
// Three states, not a checkbox. Hiding Finances should take it out of the
// sidebar; turning it *off* should also stop its bill rollover. One switch has
// to conflate those, and then either leaves work running for something you hid
// or silently stops work the moment you tidy your sidebar.

import 'package:flutter/material.dart';

import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/section_tabs.dart';
import '../../shell/shell_controller.dart';
import '../../shell/sidebar.dart' show kSectionMeta, accentFor;
import '../../src/rust/api/settings.dart';
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
  'settings': ('This page', ''),
};

/// The ready-made shapes, matching `tulipix_core::sections::preset`.
const List<(String, String, String)> kPresets = [
  ('all', 'Everything', 'ten sections'),
  ('media', 'Media only', 'photos · video · music · books'),
  ('work', 'Work only', 'files · tools · money'),
  ('photos', 'Just Photos', 'one section'),
  ('lean', 'Lean', 'least running in the background'),
];

String _mb(int bytes) {
  if (bytes <= 0) return 'no data of its own';
  if (bytes < 1024 * 1024) return '${(bytes / 1024).round()} KB on disk';
  return '${(bytes / (1024 * 1024)).toStringAsFixed(bytes > 100 * 1024 * 1024 ? 0 : 1)} MB on disk';
}

class SettingsSections extends StatefulWidget {
  const SettingsSections({super.key, required this.controller});

  final SettingsController controller;

  @override
  State<SettingsSections> createState() => _SettingsSectionsState();
}

class _SettingsSectionsState extends State<SettingsSections> {
  /// Which row is expanded. One at a time — the panel is a list you scan, and
  /// ten open rows is a page you scroll.
  String? _open;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.controller.state;
    if (st == null) return const SizedBox.shrink();
    final rows = st.sections;
    final shownCount = rows.where((r) => r.mode == 'shown').length;
    final hidden = rows.where((r) => r.mode == 'hidden').length;
    final off = rows.where((r) => r.mode == 'off').length;

    return ListView(
      padding: const EdgeInsets.fromLTRB(4, 4, 4, 32),
      children: [
        Text(
          'Take out the ones you will never open. The sidebar changes as you '
          'switch, and a section that is not in it is not a page kept offstage '
          '— it is one the shell never builds; anything switched right off '
          'stops working in the background too. Nothing on disk is touched, so '
          'switching one back on picks up where it was.',
          style: TextStyle(fontSize: 12.5, height: 1.55, color: t.textDim),
        ),
        const _Head('Start from', 'one click, then adjust'),
        Wrap(
          spacing: 7,
          runSpacing: 7,
          children: [
            for (final (id, name, hint) in kPresets)
              _Preset(
                name: name,
                hint: hint,
                on: st.sectionsPreset == id,
                onTap: () => _apply(SettingsCmd.sectionPreset(name: id)),
              ),
            if (st.sectionsPreset == 'custom')
              _Preset(
                name: 'Custom',
                hint: '$shownCount of ${rows.length} shown',
                on: true,
                onTap: null,
              ),
          ],
        ),
        _Head(
          'Sections',
          'drag to reorder the sidebar',
          trailing: '$shownCount shown · $hidden hidden · $off off',
        ),
        // ReorderableListView rather than a drag target per row: it owns the
        // gap-opening and the autoscroll, and a hand-rolled one gets neither.
        ReorderableListView.builder(
          shrinkWrap: true,
          physics: const NeverScrollableScrollPhysics(),
          buildDefaultDragHandles: false,
          itemCount: rows.length,
          onReorderItem: (from, to) => _reorder(rows, from, to),
          itemBuilder: (context, i) => _SectionRow(
            key: ValueKey(rows[i].id),
            index: i,
            row: rows[i],
            open: _open == rows[i].id,
            onToggleOpen: () => setState(
              () => _open = _open == rows[i].id ? null : rows[i].id,
            ),
            onMode: (m) =>
                _apply(SettingsCmd.sectionSet(id: rows[i].id, mode: m)),
            onTab: (tab, on) => _setTab(rows[i], tab, on),
          ),
        ),
        const _Head('Opens at launch', 'the section Tulipix lands on'),
        Wrap(
          spacing: 7,
          runSpacing: 7,
          children: [
            for (final r in rows.where((r) => r.mode == 'shown'))
              _Preset(
                name: kSectionMeta[kSectionById[r.id]]?.label ?? r.id,
                hint: st.landing == r.id ? 'opens here' : 'set',
                on: st.landing == r.id,
                onTap: () => _apply(SettingsCmd.sectionLanding(id: r.id)),
              ),
          ],
        ),
        ..._warnings(context, rows),
      ],
    );
  }

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

  /// One tab inside one section. The last one cannot go: a section with no tab
  /// row is a blank page, and "off" is the switch for not wanting the section.
  void _setTab(SectionRow row, String tab, bool on) {
    final all = sectionTabs[row.id] ?? const <TabEntry>[];
    final off = row.tabsOff.toSet();
    final kept = all.where((e) => !off.contains(e.id)).length;
    if (!on && kept <= 1) {
      final name = kSectionMeta[kSectionById[row.id]]?.label ?? row.id;
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text('Switch $name off instead — a section needs one tab.'),
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

  /// `onReorderItem` hands back a destination that already accounts for the
  /// row leaving its old slot, so there is no index to adjust here.
  void _reorder(List<SectionRow> rows, int from, int to) {
    final ids = [for (final r in rows) r.id];
    ids.insert(to, ids.removeAt(from));
    _apply(SettingsCmd.sectionOrder(ids: ids));
  }

  /// What is about to go with the thing being switched off. Every one of these
  /// is a real coupling somebody would otherwise discover afterwards.
  List<Widget> _warnings(BuildContext context, List<SectionRow> rows) {
    String modeOf(String id) =>
        rows.firstWhere((r) => r.id == id, orElse: () => rows.first).mode;
    final msgs = <String>[];
    if (rows.where((r) => r.mode == 'shown' && r.id != 'settings').isEmpty) {
      msgs.add('Every section but Settings is switched off. Tulipix will open '
          'on Settings and there will be nothing else to open.');
    }
    if (modeOf('books') == 'off') {
      msgs.add('Genesis lives inside Books. Turning Books off takes it too.');
    }
    if (modeOf('music') == 'off') {
      msgs.add('The player bar is Music’s. Anything playing stops, and the '
          'mini player goes with it.');
    }
    final offNames = [
      for (final r in rows)
        if (r.mode == 'off' && r.id != 'home' && r.id != 'settings')
          kSectionMeta[kSectionById[r.id]]?.label ?? r.id
    ];
    if (offNames.isNotEmpty && modeOf('home') == 'shown') {
      msgs.add('Home cards for ${offNames.join(', ')} go too — a card that '
          'opens nothing is worse than no card.');
    }
    if (msgs.isEmpty) return const [];
    final t = context.tokens;
    return [
      const SizedBox(height: 18),
      Container(
        padding: const EdgeInsets.fromLTRB(12, 11, 12, 12),
        decoration: BoxDecoration(
          color: Color.alphaBlend(Tokens.warn.withValues(alpha: 0.12), t.panel),
          borderRadius: BorderRadius.circular(11),
          border: Border.all(color: Tokens.warn.withValues(alpha: 0.34)),
        ),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const Padding(
              padding: EdgeInsets.only(top: 1),
              child: Icon(Icons.warning_amber_rounded,
                  size: 17, color: Tokens.warn),
            ),
            const SizedBox(width: 9),
            Expanded(
              child: Text(
                msgs.join('\n'),
                style: TextStyle(fontSize: 12, height: 1.6, color: t.textDim),
              ),
            ),
          ],
        ),
      ),
    ];
  }
}

// ------------------------------------------------------------------- pieces --

class _Head extends StatelessWidget {
  const _Head(this.title, this.hint, {this.trailing});

  final String title;
  final String hint;
  final String? trailing;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.fromLTRB(2, 22, 2, 9),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          Text(
            title.toUpperCase(),
            style: TextStyle(
              fontFamily: context.skin.fontFamily ?? Tokens.fontFamily,
              fontSize: 11,
              fontWeight: FontWeight.w700,
              letterSpacing: 0.9,
              color: t.textDim,
            ),
          ),
          const SizedBox(width: 9),
          Expanded(
            child: Text(
              hint,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(fontSize: 11.5, color: t.textDim),
            ),
          ),
          if (trailing != null)
            Text(
              trailing!,
              style: TextStyle(fontSize: 11.5, color: t.textDim),
            ),
        ],
      ),
    );
  }
}

class _Preset extends StatelessWidget {
  const _Preset({
    required this.name,
    required this.hint,
    required this.on,
    required this.onTap,
  });

  final String name;
  final String hint;
  final bool on;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(12),
      child: Container(
        constraints: const BoxConstraints(minWidth: 126),
        padding: const EdgeInsets.fromLTRB(13, 9, 13, 9),
        decoration: BoxDecoration(
          color: on
              ? Color.alphaBlend(
                  Tokens.secPhotos.withValues(alpha: 0.10), t.panel)
              : t.panel,
          borderRadius: BorderRadius.circular(12),
          border: Border.all(
            color: on ? Tokens.secPhotos : t.outline,
          ),
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              name,
              style: TextStyle(
                fontSize: 12.5,
                fontWeight: FontWeight.w600,
                color: t.text,
              ),
            ),
            const SizedBox(height: 1),
            Text(hint, style: TextStyle(fontSize: 10.5, color: t.textDim)),
          ],
        ),
      ),
    );
  }
}

class _SectionRow extends StatelessWidget {
  const _SectionRow({
    super.key,
    required this.index,
    required this.row,
    required this.open,
    required this.onToggleOpen,
    required this.onMode,
    required this.onTab,
  });

  final int index;
  final SectionRow row;
  final bool open;
  final VoidCallback onToggleOpen;
  final void Function(String mode) onMode;
  final void Function(String tab, bool on) onTab;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final section = kSectionById[row.id];
    final meta = kSectionMeta[section];
    final accent = section == null ? t.textDim : accentFor(section);
    final work = kSectionWork[row.id]?.$2 ?? '';
    final blurb = kSectionWork[row.id]?.$1 ?? '';
    final dim = row.mode != 'shown';

    return Padding(
      padding: const EdgeInsets.only(bottom: 7),
      child: Opacity(
        opacity: dim ? 0.64 : 1,
        child: Container(
          decoration: BoxDecoration(
            color: t.panel,
            borderRadius: BorderRadius.circular(13),
            border: Border.all(color: t.outline),
          ),
          child: Column(
            children: [
              Row(
                children: [
                  if (row.locked)
                    const SizedBox(width: 34)
                  else
                    ReorderableDragStartListener(
                      index: index,
                      child: Padding(
                        padding: const EdgeInsets.fromLTRB(10, 12, 2, 12),
                        child: Icon(Icons.drag_indicator,
                            size: 18, color: t.textDim),
                      ),
                    ),
                  Container(
                    width: 34,
                    height: 34,
                    margin: const EdgeInsets.symmetric(horizontal: 8),
                    decoration: BoxDecoration(
                      color: accent.withValues(alpha: 0.16),
                      borderRadius: BorderRadius.circular(10),
                    ),
                    child: Icon(meta?.icon ?? Icons.widgets_outlined,
                        size: 18, color: accent),
                  ),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        Row(
                          children: [
                            Text(
                              meta?.label ?? row.id,
                              style: TextStyle(
                                fontSize: 13.5,
                                fontWeight: FontWeight.w600,
                                color: t.text,
                              ),
                            ),
                            if (row.locked) ...[
                              const SizedBox(width: 7),
                              Container(
                                padding: const EdgeInsets.symmetric(
                                    horizontal: 6, vertical: 2),
                                decoration: BoxDecoration(
                                  borderRadius: BorderRadius.circular(999),
                                  border: Border.all(color: t.outline),
                                ),
                                child: Text(
                                  'always on',
                                  style:
                                      TextStyle(fontSize: 10, color: t.textDim),
                                ),
                              ),
                            ],
                          ],
                        ),
                        const SizedBox(height: 1),
                        Text(
                          switch (row.mode) {
                            'hidden' =>
                              'Hidden — still working in the background',
                            'off' => 'Off — hidden, and its background work is '
                                'stopped',
                            _ => blurb,
                          },
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 11.5, color: t.textDim),
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(width: 10),
                  _Seg(
                    value: row.mode,
                    enabled: !row.locked,
                    onPick: onMode,
                  ),
                  IconButton(
                    iconSize: 18,
                    icon: AnimatedRotation(
                      turns: open ? 0.5 : 0,
                      duration: const Duration(milliseconds: 160),
                      child: Icon(Icons.expand_more, color: t.textDim),
                    ),
                    onPressed: onToggleOpen,
                  ),
                ],
              ),
              if (open)
                Container(
                  width: double.infinity,
                  padding: const EdgeInsets.fromLTRB(12, 11, 12, 12),
                  decoration: BoxDecoration(
                    border: Border(top: BorderSide(color: t.outline)),
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        _effect(row, work),
                        style: TextStyle(
                            fontSize: 12, height: 1.55, color: t.textDim),
                      ),
                      const SizedBox(height: 10),
                      Wrap(
                        spacing: 6,
                        runSpacing: 6,
                        children: [
                          _Fact(row.db.isEmpty ? 'no database' : row.db),
                          _Fact(_mb(row.bytes)),
                          _Fact(work.isEmpty
                              ? 'no background work'
                              : work.split(',').first.trim()),
                        ],
                      ),
                      ..._tabs(context),
                    ],
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }

  /// The tab row inside this section, as chips you can take away.
  ///
  /// Only the five sections whose tab row is a list appear — see
  /// `shell/section_tabs.dart`. For the rest this is empty rather than a row of
  /// chips that would toggle a setting nothing reads.
  List<Widget> _tabs(BuildContext context) {
    final t = context.tokens;
    final all = sectionTabs[row.id] ?? const <TabEntry>[];
    if (all.isEmpty) return const [];
    final off = row.tabsOff.toSet();
    final kept = all.where((e) => !off.contains(e.id)).length;
    return [
      const SizedBox(height: 12),
      Text(
        'Tabs',
        style: TextStyle(
          fontSize: 11,
          fontWeight: FontWeight.w600,
          letterSpacing: 0.4,
          color: t.textDim,
        ),
      ),
      const SizedBox(height: 7),
      Wrap(
        spacing: 6,
        runSpacing: 6,
        children: [
          for (final e in all)
            _TabChip(
              label: e.label,
              on: !off.contains(e.id),
              onTap: () => onTab(e.id, off.contains(e.id)),
            ),
        ],
      ),
      const SizedBox(height: 7),
      Text(
        '$kept of ${all.length} kept · the tab you are standing on stays until '
        'you leave it',
        style: TextStyle(fontSize: 11, color: t.textDim),
      ),
    ];
  }

  /// The three sentences that make three states worth having. If they read the
  /// same, the states are the same and one of them should not exist.
  String _effect(SectionRow row, String work) {
    if (row.locked) {
      return 'Settings is how you get back, so it cannot be hidden and it '
          'stays last in the sidebar.';
    }
    final w = work.isEmpty ? 'nothing in the background' : work;
    return switch (row.mode) {
      'shown' => 'In the sidebar and built at launch. Runs $w.',
      'hidden' =>
        'Out of the sidebar and not built at launch — but $w keep running, so '
            'it is already current the moment you bring it back.',
      _ => 'Out of the sidebar, and $w stop. Nothing on disk is deleted'
          '${row.db.isEmpty ? '' : ' — ${row.db} is left exactly as it is'}.',
    };
  }
}

class _Seg extends StatelessWidget {
  const _Seg({
    required this.value,
    required this.enabled,
    required this.onPick,
  });

  final String value;
  final bool enabled;
  final void Function(String) onPick;

  static const List<(String, String)> _modes = [
    ('shown', 'Shown'),
    ('hidden', 'Hidden'),
    ('off', 'Off'),
  ];

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Opacity(
      opacity: enabled ? 1 : 0.5,
      child: Container(
        padding: const EdgeInsets.all(2),
        decoration: BoxDecoration(
          color: t.panel2,
          borderRadius: BorderRadius.circular(999),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            for (final (id, label) in _modes)
              InkWell(
                onTap: enabled && id != value ? () => onPick(id) : null,
                borderRadius: BorderRadius.circular(999),
                child: Container(
                  height: 26,
                  padding: const EdgeInsets.symmetric(horizontal: 11),
                  alignment: Alignment.center,
                  decoration: id == value
                      ? BoxDecoration(
                          color: t.panel,
                          borderRadius: BorderRadius.circular(999),
                        )
                      : null,
                  child: Text(
                    label,
                    style: TextStyle(
                      fontSize: 11.5,
                      fontWeight: FontWeight.w600,
                      color: id != value
                          ? t.textDim
                          : switch (id) {
                              'shown' => Tokens.ok,
                              'off' => Tokens.error,
                              _ => t.text,
                            },
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
      height: 25,
      padding: const EdgeInsets.symmetric(horizontal: 10),
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: t.panel2,
        borderRadius: BorderRadius.circular(999),
        border: Border.all(color: t.outline),
      ),
      child: Text(
        label,
        style: TextStyle(fontSize: 11, color: t.textDim),
      ),
    );
  }
}
