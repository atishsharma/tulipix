// All papers, and the vault once it is open: the rail of kinds and
// collections, the shelf of pages, and the paper beside it.
//
// Wide windows show all three; narrower ones drop the rail, and the paper
// takes the shelf's place until it is closed.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/dialog.dart';
import '../../src/rust/api/papers.dart';
import '../kitchen/kitchen_page.dart' show Grid, Quiet, cardDeco;
import 'papers_controller.dart';
import 'papers_page.dart';

class PapersBrowser extends StatelessWidget {
  const PapersBrowser({
    super.key,
    required this.c,
    required this.st,
    required this.vault,
  });

  final PapersController c;
  final PapersState st;

  /// The Vault tab: its own papers, no rail.
  final bool vault;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final open = st.open;
    return LayoutBuilder(builder: (context, box) {
      final wide = box.maxWidth >= 1100;
      final rail = !vault && box.maxWidth >= 900;
      return Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (rail)
            Container(
              width: 210,
              decoration: BoxDecoration(
                border: Border(right: BorderSide(color: t.nHair)),
              ),
              child: _Rail(c: c, st: st),
            ),
          Expanded(
            child: open != null && !wide
                ? _Detail(c: c, st: st, v: open)
                : _Shelf(c: c, st: st, vault: vault),
          ),
          if (open != null && wide)
            Container(
              width: 370,
              decoration: BoxDecoration(
                border: Border(left: BorderSide(color: t.nHair)),
              ),
              child: _Detail(c: c, st: st, v: open),
            ),
        ],
      );
    });
  }
}

// -------------------------------------------------------------------- rail --

class _Rail extends StatelessWidget {
  const _Rail({required this.c, required this.st});

  final PapersController c;
  final PapersState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget head(String s) => Padding(
          padding: const EdgeInsets.fromLTRB(8, 16, 8, 6),
          child: Text(s,
              style: TextStyle(
                  fontSize: 12, fontWeight: FontWeight.w700, color: t.nInk3)),
        );
    return ListView(
      padding: const EdgeInsets.fromLTRB(10, 4, 10, 24),
      children: [
        head('Type'),
        for (final k in st.kinds)
          _RailRow(
            lead: Container(
              width: 8,
              height: 8,
              decoration: BoxDecoration(
                color: k.id == 'all' ? t.nInk3 : kindColour(k.id),
                shape: BoxShape.circle,
              ),
            ),
            label: k.label,
            n: k.n,
            on: st.kind == k.id,
            onTap: () => c.send(PapersCmd.setKind(kind: k.id)),
          ),
        if (st.collections.isNotEmpty) head('Collections'),
        for (final k in st.collections)
          _RailRow(
            lead: Icon(
                k.id == '@vault' ? Icons.lock_outline : Icons.folder_outlined,
                size: 15,
                color: t.nInk2),
            label: k.label,
            n: k.n,
            on: st.collection == k.id,
            onTap: () => c.send(PapersCmd.setCollection(
                collection: st.collection == k.id ? '' : k.id)),
          ),
        head('Only'),
        _Toggle(
          label: 'With a reminder',
          on: st.onlyReminder,
          onChanged: (v) => c.send(
              PapersCmd.setOnly(reminder: v, unlinked: st.onlyUnlinked)),
        ),
        _Toggle(
          label: 'Not in Finances',
          on: st.onlyUnlinked,
          onChanged: (v) => c.send(
              PapersCmd.setOnly(reminder: st.onlyReminder, unlinked: v)),
        ),
      ],
    );
  }
}

class _RailRow extends StatelessWidget {
  const _RailRow({
    required this.lead,
    required this.label,
    required this.n,
    required this.on,
    required this.onTap,
  });

  final Widget lead;
  final String label;
  final int n;
  final bool on;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: on ? kPapers.withValues(alpha: 0.15) : Colors.transparent,
      borderRadius: BorderRadius.circular(8),
      child: InkWell(
        borderRadius: BorderRadius.circular(8),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 8),
          child: Row(
            children: [
              SizedBox(width: 18, child: Center(child: lead)),
              const SizedBox(width: 8),
              Expanded(
                child: Text(label,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: on ? FontWeight.w700 : FontWeight.w500,
                        color: on ? kPapers : t.nInk)),
              ),
              Text('$n', style: TextStyle(fontSize: 12, color: t.nInk3)),
            ],
          ),
        ),
      ),
    );
  }
}

class _Toggle extends StatelessWidget {
  const _Toggle({required this.label, required this.on, required this.onChanged});

  final String label;
  final bool on;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 8),
      child: Row(
        children: [
          Expanded(
              child: Text(label,
                  style: TextStyle(fontSize: 13, color: t.nInk))),
          Switch(value: on, activeThumbColor: kPapers, onChanged: onChanged),
        ],
      ),
    );
  }
}

// ------------------------------------------------------------------- shelf --

class _Shelf extends StatelessWidget {
  const _Shelf({required this.c, required this.st, required this.vault});

  final PapersController c;
  final PapersState st;
  final bool vault;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final n = st.papers.length;
    final head = st.query.isNotEmpty
        ? '${plural(n, 'paper')} with “${st.query}”'
        : vault
            ? plural(n, 'paper')
            : '${plural(st.total, 'paper')} · ${st.size} · everything searchable by its text';
    Widget sortChip(String id, String label) {
      final on = st.sort == id;
      return Padding(
        padding: const EdgeInsets.only(left: 6),
        child: ChoiceChip(
          label: Text(label, style: const TextStyle(fontSize: 12.5)),
          selected: on,
          selectedColor: kPapers.withValues(alpha: 0.18),
          onSelected: (_) => c.send(PapersCmd.setSort(sort: id)),
        ),
      );
    }

    return ListView(
      padding: const EdgeInsets.fromLTRB(22, 16, 22, 40),
      children: [
        Row(
          children: [
            Expanded(
              child: Text(head,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 13, color: t.nInk2)),
            ),
            sortChip('newest', 'Newest'),
            sortChip('expiring', 'Expiring'),
            sortChip('amount', 'Amount'),
            const SizedBox(width: 8),
            IconButton(
              tooltip: c.list ? 'As pages' : 'As a list',
              onPressed: c.toggleList,
              icon: Icon(c.list ? Icons.grid_view : Icons.view_list,
                  size: 19, color: t.nInk2),
            ),
          ],
        ),
        const SizedBox(height: 14),
        if (n == 0)
          Quiet(
            icon: Icons.description_outlined,
            title: st.total == 0 && !vault ? 'No papers yet' : 'Nothing here',
            body: st.total == 0 && !vault
                ? 'Add papers from the Inbox: drop them on the window, choose files, or send them from your phone.'
                : 'Nothing fits this search or filter.',
          )
        else if (c.list)
          Container(
            decoration: cardDeco(context),
            child: Column(
              children: [
                for (var i = 0; i < n; i++) ...[
                  if (i > 0) Divider(height: 1, color: t.nHair),
                  _ListRow(c: c, p: st.papers[i], on: st.open?.id == st.papers[i].id),
                ],
              ],
            ),
          )
        else
          Grid(
            min: 150,
            children: [
              for (final p in st.papers)
                _Card(c: c, p: p, on: st.open?.id == p.id),
            ],
          ),
      ],
    );
  }
}

class _Card extends StatelessWidget {
  const _Card({required this.c, required this.p, required this.on});

  final PapersController c;
  final PaperCard p;
  final bool on;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      borderRadius: BorderRadius.circular(12),
      onTap: () => c.send(PapersCmd.select(id: on ? 0 : p.id)),
      child: Container(
        padding: const EdgeInsets.all(6),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(12),
          border: Border.all(
              color: on ? kPapers : Colors.transparent, width: 2),
          color: on ? kPapers.withValues(alpha: 0.06) : null,
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            AspectRatio(
              aspectRatio: 0.76,
              child: PaperThumb(path: p.thumb, kind: p.kind, sealed: p.vault),
            ),
            const SizedBox(height: 8),
            Text(p.title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 13.5, fontWeight: FontWeight.w700, color: t.nInk)),
            const SizedBox(height: 3),
            Row(
              children: [
                Expanded(
                  child: Text(p.line,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 11.5, color: t.nInk3)),
                ),
                KindPill(kind: p.kind, label: c.kindLabel(p.kind)),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

class _ListRow extends StatelessWidget {
  const _ListRow({required this.c, required this.p, required this.on});

  final PapersController c;
  final PaperCard p;
  final bool on;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: on ? kPapers.withValues(alpha: 0.08) : Colors.transparent,
      child: InkWell(
        onTap: () => c.send(PapersCmd.select(id: on ? 0 : p.id)),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
          child: Row(
            children: [
              SizedBox(
                width: 34,
                height: 44,
                child: PaperThumb(path: p.thumb, kind: p.kind, sealed: p.vault),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(p.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 13.5,
                            fontWeight: FontWeight.w700,
                            color: t.nInk)),
                    Text(p.line,
                        style: TextStyle(fontSize: 12, color: t.nInk3)),
                  ],
                ),
              ),
              if (p.reminder)
                Padding(
                  padding: const EdgeInsets.only(right: 8),
                  child: Icon(Icons.notifications_none, size: 16, color: t.nInk3),
                ),
              if (p.linked)
                Padding(
                  padding: const EdgeInsets.only(right: 8),
                  child: Icon(Icons.credit_card, size: 16, color: t.nInk3),
                ),
              if (p.vault)
                Padding(
                  padding: const EdgeInsets.only(right: 8),
                  child: Icon(Icons.lock_outline, size: 16, color: t.nInk3),
                ),
              KindPill(kind: p.kind, label: c.kindLabel(p.kind)),
            ],
          ),
        ),
      ),
    );
  }
}

// ------------------------------------------------------------------ detail --

class _Detail extends StatelessWidget {
  const _Detail({required this.c, required this.st, required this.v});

  final PapersController c;
  final PapersState st;
  final PaperView v;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget section(String s) => Padding(
          padding: const EdgeInsets.fromLTRB(0, 18, 0, 8),
          child: Text(s,
              style: TextStyle(
                  fontSize: 12.5,
                  fontWeight: FontWeight.w700,
                  color: t.nInk2)),
        );
    return ColoredBox(
      color: t.panel,
      child: ListView(
        padding: const EdgeInsets.fromLTRB(18, 14, 18, 30),
        children: [
          Row(
            children: [
              KindPill(kind: v.kind, label: c.kindLabel(v.kind)),
              const Spacer(),
              PopupMenuButton<String>(
                tooltip: 'More',
                icon: Icon(Icons.more_horiz, color: t.nInk2),
                onSelected: (w) async {
                  switch (w) {
                    case 'edit':
                      await editPaper(context, c, v.id);
                    case 'vault':
                      await c.send(PapersCmd.setVault(id: v.id, sealed: !v.vault));
                    case 'copy':
                      await saveCopy(c, v);
                    case 'delete':
                      if (context.mounted) {
                        await deletePaper(context, c, v.id, v.title);
                      }
                  }
                },
                itemBuilder: (_) => [
                  const PopupMenuItem(value: 'edit', child: Text('Edit')),
                  PopupMenuItem(
                      value: 'vault',
                      enabled: !v.locked,
                      child: Text(v.vault ? 'Take out of the vault' : 'Move to vault')),
                  PopupMenuItem(
                      value: 'copy',
                      enabled: !v.locked,
                      child: const Text('Save a copy…')),
                  const PopupMenuItem(value: 'delete', child: Text('Delete')),
                ],
              ),
              IconButton(
                tooltip: 'Close',
                onPressed: () => c.send(const PapersCmd.select(id: 0)),
                icon: Icon(Icons.close, size: 19, color: t.nInk2),
              ),
            ],
          ),
          const SizedBox(height: 6),
          Text(v.title,
              style: TextStyle(
                  fontSize: 19, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(height: 14),
          Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 300, maxHeight: 380),
              child: AspectRatio(
                aspectRatio: 0.76,
                child: v.locked
                    ? _Locked(c: c)
                    : InkWell(
                        onTap: () => c.send(PapersCmd.openFile(id: v.id)),
                        child: PaperThumb(
                            path: v.thumb, kind: v.kind, sealed: v.vault),
                      ),
              ),
            ),
          ),
          if (v.status != 'read') ...[
            const SizedBox(height: 12),
            Text(v.status == 'reading' ? 'Still being read…' : 'It could not be read.',
                style: TextStyle(fontSize: 12.5, color: t.nInk2)),
          ],
          const SizedBox(height: 14),
          for (final f in v.fields)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 5),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  SizedBox(
                    width: 96,
                    child: Text(f.label,
                        style: TextStyle(fontSize: 12.5, color: t.nInk3)),
                  ),
                  Expanded(
                    child: Wrap(
                      spacing: 8,
                      runSpacing: 4,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        SelectableText(f.value,
                            style: TextStyle(
                                fontSize: 13,
                                fontWeight: FontWeight.w600,
                                color: t.nInk)),
                        if (f.note.isNotEmpty)
                          Container(
                            padding: const EdgeInsets.symmetric(
                                horizontal: 6, vertical: 2),
                            decoration: BoxDecoration(
                              color: (f.note == 'passed'
                                      ? Tokens.error
                                      : const Color(0xFF16A34A))
                                  .withValues(alpha: 0.13),
                              borderRadius: BorderRadius.circular(5),
                            ),
                            child: Text(f.note,
                                style: TextStyle(
                                    fontSize: 10.5,
                                    fontWeight: FontWeight.w700,
                                    color: f.note == 'passed'
                                        ? Tokens.error
                                        : const Color(0xFF16A34A))),
                          ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          if (v.finance != null || v.canAdd) section('Linked'),
          if (v.finance case final f?)
            Container(
              decoration: cardDeco(context, radius: 10),
              child: ListTile(
                dense: true,
                leading: const Icon(Icons.credit_card, size: 19),
                title: Text(f.title,
                    style: const TextStyle(
                        fontSize: 13, fontWeight: FontWeight.w700)),
                subtitle: Text(f.sub, style: const TextStyle(fontSize: 12)),
                trailing: IconButton(
                  tooltip: 'Unlink',
                  icon: const Icon(Icons.link_off, size: 18),
                  onPressed: () => c.send(PapersCmd.unlink(id: v.id)),
                ),
              ),
            )
          else if (v.canAdd)
            Align(
              alignment: Alignment.centerLeft,
              child: OutlinedButton.icon(
                onPressed: () => addToFinances(context, c, v),
                icon: const Icon(Icons.add_card, size: 16),
                label: const Text('Add to Finances'),
              ),
            ),
          if (v.snippet.isNotEmpty) ...[
            section('Found in the text'),
            Container(
              padding: const EdgeInsets.all(12),
              decoration: BoxDecoration(
                color: t.nChip,
                borderRadius: BorderRadius.circular(10),
              ),
              child: SelectableText(v.snippet,
                  style: TextStyle(fontSize: 12.5, height: 1.5, color: t.nInk2)),
            ),
          ],
          const SizedBox(height: 18),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              if (!v.filed && v.status == 'read')
                FilledButton.icon(
                  style: FilledButton.styleFrom(backgroundColor: kPapers),
                  onPressed: () => c.send(PapersCmd.file(id: v.id)),
                  icon: const Icon(Icons.check, size: 16),
                  label: const Text('File it'),
                ),
              FilledButton.icon(
                style: FilledButton.styleFrom(backgroundColor: kPapers),
                onPressed:
                    v.locked ? null : () => c.send(PapersCmd.openFile(id: v.id)),
                icon: const Icon(Icons.open_in_new, size: 16),
                label: const Text('Open'),
              ),
              OutlinedButton.icon(
                onPressed: v.locked ? null : () => saveCopy(c, v),
                icon: const Icon(Icons.save_alt, size: 16),
                label: const Text('Save a copy'),
              ),
              OutlinedButton.icon(
                onPressed: v.locked
                    ? null
                    : () => c.send(PapersCmd.setVault(id: v.id, sealed: !v.vault)),
                icon: Icon(v.vault ? Icons.lock_open_outlined : Icons.lock_outline,
                    size: 16),
                label: Text(v.vault ? 'Out of vault' : 'Move to vault'),
              ),
              if (v.dated) ...[
                OutlinedButton(
                  onPressed: () => c.send(PapersCmd.snooze(id: v.id)),
                  child: const Text('Snooze a week'),
                ),
                OutlinedButton(
                  onPressed: () => renewed(context, c, v.id),
                  child: const Text('Renewed'),
                ),
              ],
            ],
          ),
        ],
      ),
    );
  }
}

class _Locked extends StatelessWidget {
  const _Locked({required this.c});

  final PapersController c;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(8),
      ),
      padding: const EdgeInsets.all(16),
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Icon(Icons.lock_outline, size: 30, color: t.nInk3),
          const SizedBox(height: 10),
          Text('In the vault',
              style: TextStyle(
                  fontSize: 14, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(height: 4),
          Text('Unlock it to see the paper and its text.',
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 12, color: t.nInk2)),
          const SizedBox(height: 12),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: kPapers),
            onPressed: () => c.send(const PapersCmd.setTab(tab: 'vault')),
            child: const Text('Unlock'),
          ),
        ],
      ),
    );
  }
}

// ----------------------------------------------------------------- dialogs --

Future<void> saveCopy(PapersController c, PaperView v) async {
  final to = await dialogSaveFile(
    title: 'Save a copy',
    fileName: '${v.title}.${v.ext}',
    label: v.ext.toUpperCase(),
    extensions: [v.ext],
  );
  if (to != null && to.isNotEmpty) {
    await c.send(PapersCmd.saveCopy(id: v.id, to: to));
  }
}

/// The paper as an expense in Finances, from the account it was paid from.
Future<void> addToFinances(
    BuildContext context, PapersController c, PaperView v) async {
  final accounts = c.state?.accounts ?? const [];
  if (accounts.isEmpty) {
    c.say('Add an account in Finances first');
    return;
  }
  var pick = accounts.first.id;
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => StatefulBuilder(
      builder: (ctx, set) => AlertDialog(
        title: const Text('Add to Finances'),
        content: SizedBox(
          width: 380,
          child: DropdownButtonFormField<int>(
            initialValue: pick,
            decoration: const InputDecoration(labelText: 'Paid from'),
            items: [
              for (final a in accounts)
                DropdownMenuItem(
                    value: a.id, child: Text('${a.name} · ${a.currency}')),
            ],
            onChanged: (x) => set(() => pick = x ?? pick),
          ),
        ),
        actions: [
          TextButton(
              onPressed: () => Navigator.pop(ctx, false),
              child: const Text('Cancel')),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: kPapers),
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('Add expense'),
          ),
        ],
      ),
    ),
  );
  if (ok == true) {
    await c.send(PapersCmd.addToFinances(id: v.id, accountId: pick));
  }
}
