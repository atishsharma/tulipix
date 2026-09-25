// The Archive section — docs/mockups/NewSections/archive-deck.html.
//
// Library (the folders and drives added, what kind of files they hold, search
// by name and inside zips, a zip opened to look inside), Duplicates (by
// checksum) and Health (files that changed on their own, broken zips, files
// gone).

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/section_tabs.dart';
import '../../src/rust/api/archive.dart';
import '../../src/rust/api/dialog.dart';
import '../kitchen/kitchen_page.dart' show Quiet, Strip, cardDeco, confirm;
import '../papers/papers_page.dart' show TabPill;
import 'archive_controller.dart';

class ArchivePage extends StatefulWidget {
  const ArchivePage({super.key, required this.visible});

  final bool visible;

  @override
  State<ArchivePage> createState() => _ArchivePageState();
}

class _ArchivePageState extends State<ArchivePage> {
  final ArchiveController _c = ArchiveController();

  @override
  void initState() {
    super.initState();
    _c.refresh();
  }

  @override
  void didUpdateWidget(ArchivePage old) {
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
        final job = st?.job;
        return ColoredBox(
          color: t.nCanvas,
          child: Column(
            children: [
              _Header(c: _c, st: st),
              if (_c.busy || job != null)
                LinearProgressIndicator(
                  minHeight: 2,
                  color: kArchive,
                  value: job != null && job.total > 0
                      ? job.done / job.total
                      : null,
                )
              else
                const SizedBox(height: 2),
              if (job != null)
                Strip(
                  icon: Icons.hourglass_top,
                  tint: kArchive,
                  text: job.total > 0
                      ? '${job.label} — ${job.done} of ${job.total}'
                      : job.label,
                  onClose: _c.refresh,
                ),
              if (_c.notice.isNotEmpty)
                Strip(
                  icon: Icons.check_circle_outline,
                  tint: kArchive,
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
                    : switch (st.tab) {
                        'dupes' => _Dupes(c: _c, st: st),
                        'health' => _Health(c: _c, st: st),
                        _ => _Library(c: _c, st: st),
                      },
              ),
            ],
          ),
        );
      },
    );
  }
}

Future<void> _addFolder(ArchiveController c) async {
  final path = await dialogPickFolder(
      title: 'A folder or drive to catalogue', initial: '');
  if (path != null && path.isNotEmpty) {
    await c.send(ArchiveCmd.addRoot(path: path));
  }
}

class _Header extends StatefulWidget {
  const _Header({required this.c, required this.st});

  final ArchiveController c;
  final ArchiveState? st;

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
    final tab = st?.tab ?? 'library';
    final r = context.skin.controlRadius ?? 10;
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
              color: kArchive.withValues(alpha: 0.17),
              borderRadius: BorderRadius.circular(10),
            ),
            child: const Icon(Icons.inventory_2_outlined,
                size: 18, color: kArchive),
          ),
          const SizedBox(width: 10),
          Text('Archive',
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
                    for (final f in keepTabs('archive', archiveTabs, (f) => f.id,
                        active: (f) => tab == f.id))
                      TabPill(
                        label: f.label,
                        on: f.id == tab,
                        count: switch (f.id) {
                          'dupes' => st?.dupes.length ?? 0,
                          'health' => st?.problems.length ?? 0,
                          _ => 0,
                        },
                        radius: r,
                        tint: kArchive,
                        onTap: () => c.send(ArchiveCmd.setTab(tab: f.id)),
                      ),
                  ],
                ),
              ),
            ),
          ),
          const SizedBox(width: 12),
          SizedBox(
            width: 280,
            height: 38,
            child: TextField(
              controller: _q,
              style: TextStyle(fontSize: 13, color: t.nInk),
              textInputAction: TextInputAction.search,
              onChanged: (_) => setState(() {}),
              onSubmitted: (v) => c.send(ArchiveCmd.search(text: v)),
              decoration: InputDecoration(
                isDense: true,
                hintText: 'Search names, and inside zips',
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
                          c.send(const ArchiveCmd.search(text: ''));
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
            style: FilledButton.styleFrom(backgroundColor: kArchive),
            icon: const Icon(Icons.create_new_folder_outlined, size: 18),
            label: const Text('Add a folder or drive'),
            onPressed: () => _addFolder(c),
          ),
        ],
      ),
    );
  }
}

// ----------------------------------------------------------------- library --

IconData _kindIcon(String k) => switch (k) {
      'archive' => Icons.folder_zip_outlined,
      'disk' => Icons.album_outlined,
      'installer' => Icons.install_desktop,
      'document' => Icons.description_outlined,
      'picture' => Icons.image_outlined,
      'video' => Icons.movie_outlined,
      'audio' => Icons.audiotrack_outlined,
      _ => Icons.insert_drive_file_outlined,
    };

class _Library extends StatelessWidget {
  const _Library({required this.c, required this.st});

  final ArchiveController c;
  final ArchiveState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (st.roots.isEmpty) {
      return Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 500),
          child: Column(mainAxisSize: MainAxisSize.min, children: [
            const Quiet(
              icon: Icons.inventory_2_outlined,
              title: 'Nothing catalogued yet',
              body: 'Add the folders and drives where you keep archives, disk '
                  'images, installers, backups and old documents. Archive reads '
                  'their names — and what is inside their zips — and keeps the '
                  'list, so a drive that is not plugged in is still searchable.',
            ),
            const SizedBox(height: 14),
            FilledButton.icon(
              style: FilledButton.styleFrom(backgroundColor: kArchive),
              icon: const Icon(Icons.create_new_folder_outlined, size: 18),
              label: const Text('Add a folder or drive'),
              onPressed: () => _addFolder(c),
            ),
          ]),
        ),
      );
    }
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Container(
          width: 270,
          decoration:
              BoxDecoration(border: Border(right: BorderSide(color: t.nHair))),
          child: ListView(
            padding: const EdgeInsets.fromLTRB(12, 14, 12, 20),
            children: [
              _KindTile(
                  label: 'Everything',
                  icon: Icons.grid_view,
                  n: st.total.toInt(),
                  on: st.kind.isEmpty,
                  onTap: () => c.send(const ArchiveCmd.setKind(kind: ''))),
              for (final k in st.kinds)
                _KindTile(
                    label: k.label,
                    icon: _kindIcon(k.id),
                    n: k.n.toInt(),
                    on: st.kind == k.id,
                    onTap: () => c.send(ArchiveCmd.setKind(kind: k.id))),
              Padding(
                padding: const EdgeInsets.fromLTRB(8, 18, 8, 8),
                child: Row(children: [
                  Expanded(
                    child: Text('FOLDERS AND DRIVES',
                        style: TextStyle(
                            fontSize: 11,
                            fontWeight: FontWeight.w700,
                            letterSpacing: 0.7,
                            color: t.nInk3)),
                  ),
                  IconButton(
                    tooltip: 'Scan them all',
                    iconSize: 17,
                    icon: const Icon(Icons.refresh),
                    onPressed: () => c.send(const ArchiveCmd.scan(id: 0)),
                  ),
                ]),
              ),
              for (final r in st.roots) _RootCard(c: c, r: r),
              Padding(
                padding: const EdgeInsets.all(8),
                child: Text('${st.total} files · ${st.bytes}',
                    style: TextStyle(fontSize: 12, color: t.nInk3)),
              ),
            ],
          ),
        ),
        Expanded(
          child: st.inside != null
              ? _InsideView(c: c, x: st.inside!)
              : _FileList(c: c, st: st),
        ),
      ],
    );
  }
}

class _KindTile extends StatelessWidget {
  const _KindTile({
    required this.label,
    required this.icon,
    required this.n,
    required this.on,
    required this.onTap,
  });

  final String label;
  final IconData icon;
  final int n;
  final bool on;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: on ? kArchive.withValues(alpha: 0.14) : Colors.transparent,
      borderRadius: BorderRadius.circular(10),
      child: InkWell(
        borderRadius: BorderRadius.circular(10),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 9),
          child: Row(children: [
            Icon(icon, size: 18, color: on ? kArchive : t.nInk2),
            const SizedBox(width: 10),
            Expanded(
              child: Text(label,
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: on ? FontWeight.w700 : FontWeight.w500,
                      color: t.nInk)),
            ),
            Text('$n', style: TextStyle(fontSize: 11.5, color: t.nInk3)),
          ]),
        ),
      ),
    );
  }
}

class _RootCard extends StatelessWidget {
  const _RootCard({required this.c, required this.r});

  final ArchiveController c;
  final ArchiveRoot r;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      margin: const EdgeInsets.only(bottom: 8),
      padding: const EdgeInsets.fromLTRB(12, 10, 4, 10),
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(10),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(children: [
                  Flexible(
                    child: Text(r.label,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontWeight: FontWeight.w700, color: t.nInk)),
                  ),
                  const SizedBox(width: 6),
                  Container(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
                    decoration: BoxDecoration(
                      color: (r.online ? Tokens.ok : t.nInk3)
                          .withValues(alpha: 0.15),
                      borderRadius: BorderRadius.circular(99),
                    ),
                    child: Text(r.online ? 'here' : 'not plugged in',
                        style: TextStyle(
                            fontSize: 10.5,
                            fontWeight: FontWeight.w700,
                            color: r.online ? Tokens.ok : t.nInk3)),
                  ),
                ]),
                const SizedBox(height: 3),
                Text('${r.files} files · ${r.bytes}',
                    style: TextStyle(fontSize: 12, color: t.nInk2)),
                Text('${r.scanned} · ${r.checked}',
                    style: TextStyle(fontSize: 11, color: t.nInk3)),
              ],
            ),
          ),
          PopupMenuButton<String>(
            tooltip: 'More',
            onSelected: (v) async {
              switch (v) {
                case 'scan':
                  await c.send(ArchiveCmd.scan(id: r.id));
                case 'hash':
                  await c.send(ArchiveCmd.hash(id: r.id));
                case 'check':
                  await c.send(ArchiveCmd.check(id: r.id));
                case 'remove':
                  final ok = await confirm(context, 'Stop cataloguing it?',
                      '${r.path} and its files leave the catalogue. Nothing on disk is touched.');
                  if (ok) await c.send(ArchiveCmd.removeRoot(id: r.id));
              }
            },
            itemBuilder: (_) => [
              PopupMenuItem(
                  value: 'scan',
                  enabled: r.online,
                  child: const Text('Scan again')),
              PopupMenuItem(
                  value: 'hash',
                  enabled: r.online,
                  child: Text('Take checksums (${r.hashed} of ${r.files})')),
              PopupMenuItem(
                  value: 'check',
                  enabled: r.online,
                  child: const Text('Check for changes')),
              const PopupMenuItem(value: 'remove', child: Text('Remove')),
            ],
          ),
        ],
      ),
    );
  }
}

class _FileList extends StatelessWidget {
  const _FileList({required this.c, required this.st});

  final ArchiveController c;
  final ArchiveState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.fromLTRB(20, 16, 20, 26),
      children: [
        Text(
            st.query.isEmpty
                ? 'Newest first'
                : st.files.isEmpty
                    ? 'Nothing called “${st.query}”, here or inside a zip.'
                    : '${plural(st.files.length, 'match', 'matches')} for “${st.query}”',
            style: TextStyle(fontSize: 12.5, color: t.nInk2)),
        const SizedBox(height: 10),
        Container(
          decoration: cardDeco(context),
          child: Column(
            children: [
              for (final f in st.files)
                ListTile(
                  leading: Icon(_kindIcon(f.kind), color: kArchive),
                  title: Text(f.inner.isEmpty ? f.name : f.inner.split('/').last,
                      style: TextStyle(
                          fontWeight: FontWeight.w600, color: t.nInk)),
                  subtitle: Text(
                      [
                        if (f.inner.isNotEmpty) 'inside ${f.name}',
                        f.dir.isEmpty ? f.root : '${f.root} › ${f.dir}',
                        f.size,
                        f.date,
                        if (!f.online) 'drive not plugged in',
                        if (f.state.isNotEmpty) f.state,
                      ].join(' · '),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 12, color: t.nInk2)),
                  trailing: Row(mainAxisSize: MainAxisSize.min, children: [
                    if (f.isZip)
                      IconButton(
                        tooltip: 'Look inside',
                        icon: const Icon(Icons.folder_zip_outlined),
                        onPressed: () =>
                            c.send(ArchiveCmd.lookInside(id: f.id)),
                      ),
                    IconButton(
                      tooltip: 'Open',
                      icon: const Icon(Icons.open_in_new),
                      onPressed: f.online
                          ? () => c.send(ArchiveCmd.openFile(id: f.id))
                          : null,
                    ),
                    IconButton(
                      tooltip: 'Show in its folder',
                      icon: const Icon(Icons.folder_open_outlined),
                      onPressed: f.online
                          ? () => c.send(ArchiveCmd.reveal(id: f.id))
                          : null,
                    ),
                  ]),
                ),
            ],
          ),
        ),
      ],
    );
  }
}

class _InsideView extends StatelessWidget {
  const _InsideView({required this.c, required this.x});

  final ArchiveController c;
  final ArchiveInside x;

  Future<void> _extractAll() async {
    final dir = await dialogPickFolder(title: 'Extract everything into', initial: '');
    if (dir == null || dir.isEmpty) return;
    try {
      final n = await archiveExtractAll(id: x.fileId, dir: dir);
      c.say('${plural(n.toInt(), 'file')} extracted into $dir');
    } catch (e) {
      c.say('Could not extract — ${plainError(e)}');
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.fromLTRB(20, 14, 20, 26),
      children: [
        Row(children: [
          TextButton.icon(
            icon: const Icon(Icons.arrow_back, size: 17),
            label: const Text('Back'),
            onPressed: () => c.send(const ArchiveCmd.closeInside()),
          ),
          const Spacer(),
          FilledButton.icon(
            style: FilledButton.styleFrom(backgroundColor: kArchive),
            icon: const Icon(Icons.unarchive_outlined, size: 18),
            label: const Text('Extract everything…'),
            onPressed: x.online ? _extractAll : null,
          ),
        ]),
        const SizedBox(height: 8),
        Text(x.name,
            style: TextStyle(
                fontSize: 22, fontWeight: FontWeight.w800, color: t.nInk)),
        Text(
            '${x.size} · ${plural(x.count.toInt(), 'file')} · '
            '${x.online ? 'read without extracting' : 'from the catalogue — the drive is not plugged in'}',
            style: TextStyle(fontSize: 12.5, color: t.nInk2)),
        const SizedBox(height: 12),
        Container(
          decoration: cardDeco(context),
          child: Column(children: [
            for (final e in x.entries)
              ListTile(
                dense: true,
                leading: Icon(
                    e.isDir
                        ? Icons.folder_outlined
                        : _kindIcon(_kindOf(e.name)),
                    size: 18,
                    color: t.nInk2),
                title: Text(e.name, style: TextStyle(color: t.nInk)),
                trailing: Text(e.isDir ? '' : e.size,
                    style: TextStyle(fontSize: 12, color: t.nInk3)),
                onTap: e.isDir || !x.online
                    ? null
                    : () => c.send(ArchiveCmd.openEntry(name: e.name)),
              ),
          ]),
        ),
        const SizedBox(height: 8),
        Text('Click a file to open a copy of it; the zip is not changed.',
            style: TextStyle(fontSize: 11.5, color: t.nInk3)),
      ],
    );
  }
}

String _kindOf(String name) {
  final e = name.toLowerCase().split('.').last;
  if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'heic'].contains(e)) return 'picture';
  if (['pdf', 'doc', 'docx', 'txt', 'md', 'xls', 'xlsx', 'csv'].contains(e)) {
    return 'document';
  }
  if (['zip', '7z', 'rar', 'gz', 'tar'].contains(e)) return 'archive';
  if (['mp4', 'mkv', 'mov'].contains(e)) return 'video';
  if (['mp3', 'flac', 'm4a', 'wav'].contains(e)) return 'audio';
  return 'other';
}

// -------------------------------------------------------------- duplicates --

class _Dupes extends StatelessWidget {
  const _Dupes({required this.c, required this.st});

  final ArchiveController c;
  final ArchiveState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 30),
      children: [
        Row(children: [
          Expanded(
            child: Text(
                st.dupes.isEmpty
                    ? 'No duplicates found yet.'
                    : '${plural(st.dupes.length, 'file')} with copies · ${st.dupeBytes} could be freed',
                style: TextStyle(
                    fontSize: 16, fontWeight: FontWeight.w700, color: t.nInk)),
          ),
          if (st.dupeUnhashed > 0)
            FilledButton.icon(
              style: FilledButton.styleFrom(backgroundColor: kArchive),
              icon: const Icon(Icons.compare_arrows, size: 18),
              label: Text('Compare ${st.dupeUnhashed} files of the same size'),
              onPressed: () => c.send(const ArchiveCmd.findDupes()),
            ),
        ]),
        const SizedBox(height: 4),
        Text(
            'Duplicates are files with the same checksum, not just the same name. '
            'The newest copy is marked; a copy you remove goes to the system trash.',
            style: TextStyle(fontSize: 12.5, color: t.nInk2)),
        const SizedBox(height: 14),
        for (final g in st.dupes)
          Container(
            margin: const EdgeInsets.only(bottom: 12),
            decoration: cardDeco(context),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 12, 16, 6),
                  child: Row(children: [
                    Expanded(
                      child: Text(g.name,
                          style: TextStyle(
                              fontWeight: FontWeight.w700, color: t.nInk)),
                    ),
                    Text('${plural(g.files.length, 'copy', 'copies')} of ${g.size} · ${g.wasted} spare',
                        style: TextStyle(fontSize: 12, color: t.nInk2)),
                  ]),
                ),
                for (final f in g.files)
                  ListTile(
                    dense: true,
                    leading: f.newest
                        ? const Icon(Icons.star_rounded, color: Tokens.ok)
                        : Icon(Icons.content_copy, color: t.nInk3, size: 18),
                    title: Text(f.path,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 12.5, color: t.nInk)),
                    subtitle: Text(
                        '${f.root} · ${f.date}${f.newest ? ' · newest' : ''}${f.online ? '' : ' · not plugged in'}',
                        style: TextStyle(fontSize: 11.5, color: t.nInk3)),
                    trailing: TextButton(
                      onPressed: !f.online
                          ? null
                          : () async {
                              final ok = await confirm(context,
                                  'Move this copy to the trash?', f.path);
                              if (ok) {
                                await c.send(ArchiveCmd.trash(id: f.id));
                              }
                            },
                      child: const Text('Move to trash'),
                    ),
                  ),
              ],
            ),
          ),
      ],
    );
  }
}

// ------------------------------------------------------------------ health --

class _Health extends StatelessWidget {
  const _Health({required this.c, required this.st});

  final ArchiveController c;
  final ArchiveState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 30),
      children: [
        Text(
            'Take checksums of a folder once. A check later reads every file '
            'again: a file whose bytes changed while its date and size did not '
            'changed on its own — usually a failing disk. Zips are read through '
            'to find broken members.',
            style: TextStyle(fontSize: 12.5, color: t.nInk2)),
        const SizedBox(height: 14),
        Container(
          decoration: cardDeco(context),
          child: Column(children: [
            for (final r in st.roots)
              ListTile(
                title: Text(r.label,
                    style:
                        TextStyle(fontWeight: FontWeight.w700, color: t.nInk)),
                subtitle: Text(
                    '${r.hashed} of ${r.files} files have a checksum · ${r.checked}',
                    style: TextStyle(fontSize: 12, color: t.nInk2)),
                trailing: Row(mainAxisSize: MainAxisSize.min, children: [
                  TextButton(
                    onPressed: r.online
                        ? () => c.send(ArchiveCmd.hash(id: r.id))
                        : null,
                    child: const Text('Take checksums'),
                  ),
                  FilledButton(
                    style: FilledButton.styleFrom(backgroundColor: kArchive),
                    onPressed: r.online
                        ? () => c.send(ArchiveCmd.check(id: r.id))
                        : null,
                    child: const Text('Check'),
                  ),
                ]),
              ),
          ]),
        ),
        const SizedBox(height: 18),
        Text(
            st.problems.isEmpty
                ? 'No problems found.'
                : plural(st.problems.length, 'problem'),
            style: TextStyle(
                fontSize: 16, fontWeight: FontWeight.w700, color: t.nInk)),
        const SizedBox(height: 10),
        for (final p in st.problems)
          Container(
            margin: const EdgeInsets.only(bottom: 10),
            padding: const EdgeInsets.fromLTRB(14, 12, 8, 12),
            decoration: cardDeco(context),
            child: Row(children: [
              Icon(
                  p.state == 'missing'
                      ? Icons.help_outline
                      : Icons.warning_amber_rounded,
                  color: p.state == 'changed' ? Tokens.error : Tokens.warn),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(p.name,
                        style: TextStyle(
                            fontWeight: FontWeight.w700, color: t.nInk)),
                    Text(p.path,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11.5, color: t.nInk3)),
                    const SizedBox(height: 3),
                    Text(p.why,
                        style: TextStyle(fontSize: 12.5, color: t.nInk2)),
                  ],
                ),
              ),
              TextButton(
                onPressed: () => c.send(ArchiveCmd.dismiss(id: p.id)),
                child: const Text('Dismiss'),
              ),
            ]),
          ),
      ],
    );
  }
}
