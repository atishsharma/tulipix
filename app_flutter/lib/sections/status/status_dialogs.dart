// The three Status popups: one section card in full, one metric explained, and
// the library — what is indexed, what is watched, what is reindexing, and the
// two buttons that act on all of it.

import 'package:flutter/material.dart';

import '../../design/pick.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/status.dart';
import 'status_controller.dart';
import 'status_page.dart';

/// The shell every one of them shares: a stripe, a glyph, a title, a close
/// cross, a scrolling body and a foot of buttons.
Future<void> _sheet(
  BuildContext context, {
  required Color tint,
  required IconData icon,
  required String title,
  required String subtitle,
  required Widget body,
  required List<Widget> Function(BuildContext) foot,
}) {
  final t = context.tokens;
  return showDialog<void>(
    context: context,
    builder: (context) => Dialog(
      backgroundColor: t.panel,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 640, maxHeight: 720),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(height: 3, color: tint),
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 16, 8, 16),
              child: Row(
                children: [
                  Glyph(icon: icon, tint: tint),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(title,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontSize: 16,
                                fontWeight: FontWeight.w800,
                                color: t.text)),
                        const SizedBox(height: 3),
                        Text(subtitle,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 11.5, color: t.textDim)),
                      ],
                    ),
                  ),
                  IconButton(
                    icon: const Icon(Icons.close, size: 16),
                    onPressed: () => Navigator.of(context).pop(),
                  ),
                ],
              ),
            ),
            Divider(height: 1, color: t.outline),
            Flexible(
              child: SingleChildScrollView(
                padding: const EdgeInsets.all(16),
                child: body,
              ),
            ),
            Divider(height: 1, color: t.outline),
            Padding(
              padding: const EdgeInsets.all(13),
              child: Row(children: foot(context)),
            ),
          ],
        ),
      ),
    ),
  );
}

// ── one section card ────────────────────────────────────────────────────────

/// Open the card at `index`. Guarded: `systemIdx`/`toolsIdx` are -1 until the
/// snapshot has resolved them, and the three header figures open by index.
Future<void> openCard(
  BuildContext context,
  StatusController controller,
  StatusState state,
  int index,
) {
  if (index < 0 || index >= state.cards.length) return Future.value();
  final card = state.cards[index];
  final tint = accentOf(card.accent);
  return _sheet(
    context,
    tint: tint,
    icon: iconForKey(card.key),
    title: card.name,
    subtitle: card.status,
    body: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (card.why.isNotEmpty) ...[
          Container(
            padding: const EdgeInsets.all(10),
            decoration: BoxDecoration(
              color: Tokens.error.withValues(alpha: 0.09),
              borderRadius: BorderRadius.circular(9),
            ),
            child: Text(card.why,
                style: const TextStyle(fontSize: 12, color: Tokens.error)),
          ),
          const SizedBox(height: 14),
        ],
        if (card.kpis.isNotEmpty) ...[
          Row(
            children: [
              for (var i = 0; i < card.kpis.length; i++) ...[
                if (i > 0) const SizedBox(width: 10),
                Expanded(child: _Kpi(kpi: card.kpis[i])),
              ],
            ],
          ),
          const SizedBox(height: 14),
        ],
        for (final g in card.groups) ...[
          _Group(group: g),
          const SizedBox(height: 14),
        ],
      ],
    ),
    foot: (context) => [
      const Spacer(),
      Btn(
        label: 'Close',
        tint: Tokens.secSettings,
        onTap: () => Navigator.of(context).pop(),
      ),
    ],
  );
}

class _Kpi extends StatelessWidget {
  const _Kpi({required this.kpi});

  final StKpi kpi;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(11),
      decoration: BoxDecoration(
        color: t.panel2,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: t.outline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(kpi.v,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 17, fontWeight: FontWeight.w800, color: t.text)),
          Text(kpi.k,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 10.5,
                  fontWeight: FontWeight.w600,
                  color: t.textDim)),
        ],
      ),
    );
  }
}

/// A row list *or* a table, never both — `cols` being non-empty is what says
/// which, the same test the HTML page makes.
class _Group extends StatelessWidget {
  const _Group({required this.group});

  final StGroup group;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Caption(group.h),
        const SizedBox(height: 6),
        for (final r in group.rows) KvRow(row: r),
        if (group.cols.isNotEmpty) ...[
          Row(
            children: [
              for (final h in group.cols)
                Expanded(
                  child: Text(h,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 10,
                          fontWeight: FontWeight.w700,
                          letterSpacing: 0.5,
                          color: t.textDim)),
                ),
            ],
          ),
          const SizedBox(height: 6),
          Divider(height: 1, color: t.outline),
          if (group.table.isEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 7),
              child: Text('Nothing right now.',
                  style: TextStyle(fontSize: 12, color: t.textDim)),
            ),
          for (final tr in group.table)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 7),
              child: Row(
                children: [
                  for (final c in tr.cells)
                    Expanded(
                      child: Align(
                        alignment: Alignment.centerLeft,
                        child: c.tag
                            ? Pill(label: c.text, cls: c.cls)
                            : Text(c.text,
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(
                                    fontSize: 12,
                                    fontWeight: c.cls == 'mut'
                                        ? FontWeight.w500
                                        : FontWeight.w600,
                                    color: clsColor(c.cls, t))),
                      ),
                    ),
                ],
              ),
            ),
        ],
      ],
    );
  }
}

// ── one metric, explained ───────────────────────────────────────────────────

Future<void> openMetric(
  BuildContext context,
  StatusController controller,
  StatusState state,
  int index,
) {
  if (index < 0 || index >= state.metrics.length) return Future.value();
  final m = state.metrics[index];
  return _sheet(
    context,
    tint: accentOf(m.accent),
    icon: Icons.info_outline,
    title: m.label,
    subtitle: '${m.value} · ${m.note}',
    body: Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const Caption('WHAT IT IS'),
        const SizedBox(height: 5),
        _Para(m.helpIs),
        const SizedBox(height: 14),
        const Caption('WHY IT IS HERE'),
        const SizedBox(height: 5),
        _Para(m.helpFor),
        if (m.rows.isNotEmpty) ...[
          const SizedBox(height: 14),
          const Caption('RIGHT NOW'),
          const SizedBox(height: 5),
          for (final r in m.rows) KvRow(row: r),
        ],
      ],
    ),
    foot: (context) => [
      const Spacer(),
      Btn(
        label: 'Close',
        tint: Tokens.secSettings,
        onTap: () => Navigator.of(context).pop(),
      ),
    ],
  );
}

class _Para extends StatelessWidget {
  const _Para(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Text(
        text,
        style: TextStyle(fontSize: 12.5, color: context.tokens.text),
      );
}

// ── the library ─────────────────────────────────────────────────────────────

Future<void> openLibrary(
  BuildContext context,
  StatusController controller,
  StatusState state,
) =>
    showDialog<void>(
      context: context,
      builder: (context) => _LibrarySheet(controller: controller),
    );

/// Stateful because the reset gate is: arming it and typing the word are two
/// deliberate acts, and a refresh underneath must not disarm a half-typed
/// confirmation.
class _LibrarySheet extends StatefulWidget {
  const _LibrarySheet({required this.controller});

  final StatusController controller;

  @override
  State<_LibrarySheet> createState() => _LibrarySheetState();
}

class _LibrarySheetState extends State<_LibrarySheet> {
  bool _armed = false;
  final TextEditingController _typed = TextEditingController();

  /// The word that arms the reset, from the crate that checks it.
  final String _phrase = statusResetPhrase();

  @override
  void dispose() {
    _typed.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: widget.controller,
      builder: (context, _) {
        final st = widget.controller.state;
        if (st == null) return const SizedBox.shrink();
        return Dialog(
          backgroundColor: t.panel,
          shape:
              RoundedRectangleBorder(borderRadius: BorderRadius.circular(14)),
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 700, maxHeight: 760),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Container(height: 3, color: Tokens.secBooks),
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 16, 8, 16),
                  child: Row(
                    children: [
                      const Glyph(
                          icon: Icons.folder_outlined, tint: Tokens.secBooks),
                      const SizedBox(width: 12),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text('Library',
                                style: TextStyle(
                                    fontSize: 16,
                                    fontWeight: FontWeight.w800,
                                    color: t.text)),
                            const SizedBox(height: 3),
                            Text(
                                '${st.libItems} items · ${st.libBytes} on disk '
                                '· ${st.libDatabases} databases',
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(
                                    fontSize: 11.5, color: t.textDim)),
                          ],
                        ),
                      ),
                      IconButton(
                        icon: const Icon(Icons.close, size: 16),
                        onPressed: () => Navigator.of(context).pop(),
                      ),
                    ],
                  ),
                ),
                Divider(height: 1, color: t.outline),
                Flexible(
                  child: ListView(
                    padding: const EdgeInsets.all(16),
                    children: [
                      const Caption('WHAT IS INDEXED'),
                      const SizedBox(height: 6),
                      _IndexTable(state: st),
                      const SizedBox(height: 16),
                      const Caption('WATCHED FOLDERS'),
                      const SizedBox(height: 6),
                      _Roots(state: st),
                      const SizedBox(height: 16),
                      // Progress only while something is actually indexing. A
                      // bar left at 100% after the fact is indistinguishable
                      // from a job that stopped.
                      if (st.scanRunning) ...[
                        const Caption('REINDEXING NOW'),
                        const SizedBox(height: 8),
                        for (final s in st.scan) _ScanRow(scan: s),
                        const SizedBox(height: 16),
                      ],
                      _Danger(
                        armed: _armed,
                        typed: _typed,
                        phrase: _phrase,
                        onChanged: () => setState(() {}),
                      ),
                    ],
                  ),
                ),
                Divider(height: 1, color: t.outline),
                Padding(
                  padding: const EdgeInsets.all(13),
                  child: Wrap(
                    spacing: 9,
                    runSpacing: 9,
                    crossAxisAlignment: WrapCrossAlignment.center,
                    children: [
                      Btn(
                        label: 'Close',
                        tint: Tokens.secSettings,
                        onTap: () => Navigator.of(context).pop(),
                      ),
                      Btn(
                        label: 'Add Folder…',
                        icon: Icons.add,
                        tint: Tokens.secBooks,
                        onTap: _addFolder,
                      ),
                      Btn(
                        label:
                            st.scanRunning ? 'Reindexing…' : 'Reindex Library',
                        tint: Tokens.secStatus,
                        enabled: !st.scanRunning,
                        onTap: () {
                          widget.controller.send(const StatusCmd.rescan());
                          Navigator.of(context).pop();
                        },
                      ),
                      Btn(
                        label: _armed ? 'Confirm Reset' : 'Reset App',
                        tint: Tokens.error,
                        enabled: !_armed || _typed.text == _phrase,
                        onTap: () {
                          if (!_armed) {
                            setState(() => _armed = true);
                            return;
                          }
                          if (_typed.text != _phrase) return;
                          widget.controller.send(const StatusCmd.resetApp());
                          Navigator.of(context).pop();
                        },
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }

  Future<void> _addFolder() async {
    final path = await pickDirectory();
    if (path == null || path.isEmpty) return;
    await widget.controller.send(StatusCmd.addFolder(path: path));
  }
}

class _IndexTable extends StatelessWidget {
  const _IndexTable({required this.state});

  final StatusState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final head =
        TextStyle(fontSize: 10, fontWeight: FontWeight.w700, color: t.textDim);
    return Column(
      children: [
        Row(
          children: [
            Expanded(child: Text('Section', style: head)),
            SizedBox(
                width: 80,
                child: Text('Items', textAlign: TextAlign.right, style: head)),
            SizedBox(
                width: 90,
                child: Text('Size', textAlign: TextAlign.right, style: head)),
            SizedBox(
                width: 110,
                child: Text('State', textAlign: TextAlign.right, style: head)),
          ],
        ),
        const SizedBox(height: 5),
        Divider(height: 1, color: t.outline),
        for (final s in state.libSections)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 7),
            child: Row(
              children: [
                Expanded(
                  child: Text(s.name,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 12,
                          fontWeight: FontWeight.w600,
                          color: t.text)),
                ),
                SizedBox(
                  width: 80,
                  child: Text(s.count,
                      textAlign: TextAlign.right,
                      style: TextStyle(fontSize: 12, color: t.text)),
                ),
                SizedBox(
                  width: 90,
                  child: Text(s.size,
                      textAlign: TextAlign.right,
                      style: TextStyle(fontSize: 12, color: t.text)),
                ),
                SizedBox(
                  width: 110,
                  child: Align(
                    alignment: Alignment.centerRight,
                    child: Pill(
                      label: s.status,
                      cls: s.level == 'problem'
                          ? 'b'
                          : s.level == 'busy'
                              ? 'w'
                              : '',
                    ),
                  ),
                ),
              ],
            ),
          ),
      ],
    );
  }
}

class _Roots extends StatelessWidget {
  const _Roots({required this.state});

  final StatusState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (state.roots.isEmpty)
          Text('No folders are being watched yet. Add one below.',
              style: TextStyle(fontSize: 12, color: t.textDim)),
        for (final r in state.roots)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 6),
            child: Row(
              children: [
                Expanded(
                  child: Text(r.path,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 11.5, color: t.textDim)),
                ),
                const SizedBox(width: 8),
                // Which sections found something under this root, left of its
                // on-disk state.
                for (final sec in r.sections) ...[
                  Pill(label: sec, cls: 'i'),
                  const SizedBox(width: 6),
                ],
                if (r.sections.isEmpty) ...[
                  const Pill(label: 'Not indexed', cls: 'i'),
                  const SizedBox(width: 6),
                ],
                Pill(label: r.state, cls: r.bad ? 'b' : ''),
              ],
            ),
          ),
        const SizedBox(height: 6),
        Text(
            'These are the only paths Tulipix reads. Nothing outside them is '
            'scanned, and nothing inside them is ever moved or rewritten.',
            style: TextStyle(fontSize: 11.5, color: t.textDim)),
      ],
    );
  }
}

class _ScanRow extends StatelessWidget {
  const _ScanRow({required this.scan});

  final StScan scan;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(scan.section,
                    style: TextStyle(fontSize: 11.5, color: t.text)),
              ),
              Text(
                  scan.waiting
                      ? 'counting files…'
                      : '${scan.done} / ${scan.total}'
                          '${scan.failed != "0" ? " · ${scan.failed} failed" : ""}',
                  style: TextStyle(fontSize: 11.5, color: t.textDim)),
            ],
          ),
          const SizedBox(height: 4),
          ClipRRect(
            borderRadius: BorderRadius.circular(3.5),
            child: LinearProgressIndicator(
              value: scan.waiting ? null : (scan.pct.clamp(0, 100)) / 100,
              minHeight: 7,
              backgroundColor: t.text.withValues(alpha: 0.08),
              color: scan.active ? Tokens.brand : Tokens.secSettings,
            ),
          ),
        ],
      ),
    );
  }
}

/// The destructive block, kept visually apart — the same separation the HTML
/// page draws.
class _Danger extends StatelessWidget {
  const _Danger({
    required this.armed,
    required this.typed,
    required this.phrase,
    required this.onChanged,
  });

  final bool armed;
  final TextEditingController typed;
  final String phrase;
  final VoidCallback onChanged;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(13),
      decoration: BoxDecoration(
        color: Tokens.error.withValues(alpha: 0.06),
        borderRadius: BorderRadius.circular(11),
        border: Border.all(color: Tokens.error.withValues(alpha: 0.28)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const Text('Reset Tulipix',
              style: TextStyle(
                  fontSize: 12.5,
                  fontWeight: FontWeight.w700,
                  color: Tokens.error)),
          const SizedBox(height: 7),
          Text(
              'Erases every index, setting, thumbnail and watched-folder entry. '
              'Your media files are never touched. Unlike the Slint build this '
              'does not restart the app for you — close and reopen it when the '
              'wipe finishes.',
              style: TextStyle(fontSize: 11.5, color: t.textDim)),
          if (armed) ...[
            const SizedBox(height: 8),
            TextField(
              controller: typed,
              autofocus: true,
              onChanged: (_) => onChanged(),
              style: TextStyle(fontSize: 12.5, color: t.text),
              decoration: InputDecoration(
                isDense: true,
                hintText: phrase,
                border: const OutlineInputBorder(),
              ),
            ),
            if (typed.text != phrase) ...[
              const SizedBox(height: 6),
              Text('Type $phrase to confirm.',
                  style: TextStyle(fontSize: 11, color: t.textDim)),
            ],
          ],
        ],
      ),
    );
  }
}
