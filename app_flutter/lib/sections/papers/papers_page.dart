// The Papers section — docs/NewSections/papers-deck.html.
//
// Four tabs over one snapshot: Inbox (what needs doing, the drop zone, what
// was just read), All papers (papers_all.dart), Expiring and Vault. Files
// dropped anywhere on the page come in the same way as Choose files.

import 'dart:async';
import 'dart:io';

import 'package:desktop_drop/desktop_drop.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/first_load.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/section_tabs.dart';
import '../../src/rust/api/dialog.dart';
import '../../src/rust/api/papers.dart';
import '../kitchen/kitchen_page.dart' show Grid, Quiet, Strip, cardDeco, confirm;
import '../transfer/qr_view.dart' show QrView;
import 'papers_all.dart';
import 'papers_controller.dart';

const List<String> kPaperExtensions = [
  'pdf', 'jpg', 'jpeg', 'png', 'webp', 'tif', 'tiff', 'bmp', 'txt', //
];

class PapersPage extends StatefulWidget {
  const PapersPage({super.key, required this.visible});

  /// On screen. Coming back asks again: a date may have come closer, and the
  /// watched folder may have new scans in it.
  final bool visible;

  @override
  State<PapersPage> createState() => _PapersPageState();
}

class _PapersPageState extends State<PapersPage> {
  final PapersController _c = PapersController();
  bool _over = false;

  @override
  void initState() {
    super.initState();
    _c.refresh();
  }

  @override
  void didUpdateWidget(PapersPage old) {
    super.didUpdateWidget(old);
    if (widget.visible && !old.visible) _c.refresh();
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
        return DropTarget(
          onDragEntered: (_) => setState(() => _over = true),
          onDragExited: (_) => setState(() => _over = false),
          onDragDone: (d) {
            setState(() => _over = false);
            final paths = [for (final f in d.files) f.path];
            if (paths.isNotEmpty) _c.send(PapersCmd.addFiles(paths: paths));
          },
          child: Stack(
            children: [
              ColoredBox(
                color: t.nCanvas,
                child: Column(
                  children: [
                    _Header(c: _c, st: st),
                    if (_c.busy || (st?.reading ?? 0) > 0)
                      const LinearProgressIndicator(minHeight: 2, color: kPapers)
                    else
                      const SizedBox(height: 2),
                    if (_c.notice.isNotEmpty)
                      Strip(
                        icon: Icons.check_circle_outline,
                        tint: kPapers,
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
                              'all' => PapersBrowser(c: _c, st: st, vault: false),
                              'expiring' => _Expiring(c: _c, st: st),
                              'vault' => _Vault(c: _c, st: st),
                              _ => _Inbox(c: _c, st: st),
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
                        color: kPapers.withValues(alpha: 0.10),
                        borderRadius: BorderRadius.circular(18),
                        border: Border.all(color: kPapers, width: 2),
                      ),
                      alignment: Alignment.center,
                      child: const Text('Drop to read them',
                          style: TextStyle(
                              fontSize: 20,
                              fontWeight: FontWeight.w700,
                              color: kPapers)),
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

// ------------------------------------------------------------------ header --

class _Header extends StatefulWidget {
  const _Header({required this.c, required this.st});

  final PapersController c;
  final PapersState? st;

  @override
  State<_Header> createState() => _HeaderState();
}

class _HeaderState extends State<_Header> {
  final TextEditingController _q = TextEditingController();

  @override
  void didUpdateWidget(_Header old) {
    super.didUpdateWidget(old);
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
    final tab = st?.tab ?? 'inbox';
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
              color: kPapers.withValues(alpha: 0.17),
              borderRadius: BorderRadius.circular(10),
            ),
            child: const Icon(Icons.description_outlined,
                size: 18, color: kPapers),
          ),
          const SizedBox(width: 10),
          Text('Papers',
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
                    for (final f in keepTabs('papers', papersTabs, (f) => f.id,
                        active: (f) => tab == f.id))
                      TabPill(
                        label: f.label,
                        on: f.id == tab,
                        count: switch (f.id) {
                          'inbox' => st?.unfiled ?? 0,
                          'expiring' => st?.soon ?? 0,
                          _ => 0,
                        },
                        radius: r,
                        onTap: () => c.send(PapersCmd.setTab(tab: f.id)),
                      ),
                  ],
                ),
              ),
            ),
          ),
          const SizedBox(width: 12),
          SizedBox(
            width: 260,
            height: 38,
            child: TextField(
              controller: _q,
              style: TextStyle(fontSize: 13, color: t.nInk),
              textInputAction: TextInputAction.search,
              onChanged: (_) => setState(() {}),
              onSubmitted: (v) => c.send(PapersCmd.search(text: v)),
              decoration: InputDecoration(
                isDense: true,
                hintText: 'Search inside every paper',
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
                          c.send(const PapersCmd.search(text: ''));
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
          const SizedBox(width: 8),
          IconButton(
            tooltip: 'Scan with phone',
            onPressed: st == null ? null : () => showPhone(context, c),
            icon: Icon(Icons.qr_code_scanner, size: 20, color: t.nInk2),
          ),
          const SizedBox(width: 6),
          FilledButton.icon(
            style: FilledButton.styleFrom(backgroundColor: kPapers),
            onPressed: st == null ? null : () => chooseFiles(c),
            icon: const Icon(Icons.add, size: 17),
            label: const Text('Add papers'),
          ),
        ],
      ),
    );
  }
}

class TabPill extends StatelessWidget {
  const TabPill({
    super.key,
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
      color: on ? kPapers : Colors.transparent,
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
                    color: on ? Colors.white : kPapers,
                    borderRadius: BorderRadius.circular(99),
                  ),
                  child: Text('$count',
                      style: TextStyle(
                          fontSize: 11,
                          fontWeight: FontWeight.w700,
                          color: on ? kPapers : Colors.white)),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

// ------------------------------------------------------------------- inbox --

class _Inbox extends StatelessWidget {
  const _Inbox({required this.c, required this.st});

  final PapersController c;
  final PapersState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final ready = st.inbox.where((p) => p.status == 'read').length;
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 40),
      children: [
        if (st.alerts.isNotEmpty) ...[
          Grid(
            min: 300,
            children: [for (final a in st.alerts) _AlertCard(c: c, a: a)],
          ),
          const SizedBox(height: 22),
        ],
        _DropZone(c: c, st: st),
        const SizedBox(height: 26),
        Row(
          crossAxisAlignment: CrossAxisAlignment.end,
          children: [
            Text('Just read',
                style: TextStyle(
                    fontSize: 18, fontWeight: FontWeight.w700, color: t.nInk)),
            const SizedBox(width: 10),
            Expanded(
              child: Text(
                st.inbox.isEmpty
                    ? 'nothing waiting'
                    : '${plural(ready, 'paper')} to file · suggestions come from what similar papers were filed as',
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12.5, color: t.nInk3),
              ),
            ),
            if (ready > 1)
              OutlinedButton(
                onPressed: () => c.send(const PapersCmd.fileAll()),
                child: Text('File all $ready'),
              ),
          ],
        ),
        const SizedBox(height: 14),
        if (st.inbox.isEmpty)
          Quiet(
            icon: Icons.inbox_outlined,
            title: 'Nothing waiting',
            body: st.total == 0
                ? 'Drop a receipt, a bill or a scan here, or choose files. Each one is read on this computer and filed for you.'
                : 'Everything read has been filed. ${plural(st.total, 'paper')} in All papers.',
          )
        else
          for (final p in st.inbox) ...[
            _InboxRow(c: c, st: st, p: p),
            const SizedBox(height: 10),
          ],
      ],
    );
  }
}

class _AlertCard extends StatelessWidget {
  const _AlertCard({required this.c, required this.a});

  final PapersController c;
  final PaperAlert a;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final expiry = a.kind == 'expiry';
    final urgent = expiry && a.n <= 45;
    final days = a.n.toInt();
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: urgent
          ? BoxDecoration(
              color: const Color(0xFFEF4444).withValues(alpha: 0.08),
              borderRadius: BorderRadius.circular(context.skin.panelRadius ?? 14),
              border: Border.all(
                  color: const Color(0xFFEF4444).withValues(alpha: 0.35)),
            )
          : cardDeco(context),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Ring(
            text: a.ring,
            colour: expiry ? ringColour(days) : kPapers,
            fraction: expiry ? 1 - days / 365 : 1.0,
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(a.title,
                    style: TextStyle(
                        fontSize: 14,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
                if (a.body.isNotEmpty) ...[
                  const SizedBox(height: 3),
                  Text(a.body,
                      style: TextStyle(
                          fontSize: 12.5, height: 1.4, color: t.nInk2)),
                ],
                const SizedBox(height: 10),
                Wrap(
                  spacing: 8,
                  runSpacing: 6,
                  children: expiry
                      ? [
                          OutlinedButton(
                            onPressed: () =>
                                c.send(PapersCmd.snooze(id: a.paperId)),
                            child: const Text('Snooze a week'),
                          ),
                          FilledButton.icon(
                            style: FilledButton.styleFrom(
                                backgroundColor: kPapers),
                            onPressed: () => openPaper(c, a.paperId),
                            icon: const Icon(Icons.open_in_new, size: 15),
                            label: const Text('Open'),
                          ),
                        ]
                      : [
                          FilledButton.icon(
                            style: FilledButton.styleFrom(
                                backgroundColor: kPapers),
                            onPressed: () =>
                                c.send(const PapersCmd.matchAll()),
                            icon: const Icon(Icons.link, size: 15),
                            label: Text(a.n == 1 ? 'Match it' : 'Match all'),
                          ),
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

class _DropZone extends StatelessWidget {
  const _DropZone({required this.c, required this.st});

  final PapersController c;
  final PapersState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final watching = st.watch.isNotEmpty;
    final folder = st.watch.split(Platform.pathSeparator).lastWhere(
        (s) => s.isNotEmpty,
        orElse: () => st.watch);
    return Container(
      padding: const EdgeInsets.fromLTRB(18, 16, 16, 16),
      decoration: BoxDecoration(
        color: kPapers.withValues(alpha: 0.06),
        borderRadius: BorderRadius.circular(context.skin.panelRadius ?? 14),
        border: Border.all(color: kPapers.withValues(alpha: 0.45)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Wrap(
            crossAxisAlignment: WrapCrossAlignment.center,
            spacing: 14,
            runSpacing: 12,
            children: [
              Container(
                width: 44,
                height: 44,
                decoration: BoxDecoration(
                  color: kPapers.withValues(alpha: 0.15),
                  borderRadius: BorderRadius.circular(12),
                ),
                child: const Icon(Icons.upload_outlined, color: kPapers),
              ),
              ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 480),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text('Drop PDFs, photos or scans',
                        style: TextStyle(
                            fontSize: 14.5,
                            fontWeight: FontWeight.w700,
                            color: t.nInk)),
                    const SizedBox(height: 2),
                    Text(
                        'Read on this computer: the text, the date, the amount and what it is. Nothing is uploaded.',
                        style: TextStyle(fontSize: 12.5, color: t.nInk2)),
                  ],
                ),
              ),
              OutlinedButton.icon(
                onPressed: () => chooseFiles(c),
                icon: const Icon(Icons.folder_open_outlined, size: 16),
                label: const Text('Choose files'),
              ),
              OutlinedButton.icon(
                onPressed: () => showPhone(context, c),
                icon: const Icon(Icons.qr_code_2, size: 16),
                label: const Text('Scan with phone'),
              ),
              if (watching)
                PopupMenuButton<String>(
                  tooltip: st.watch,
                  onSelected: (v) {
                    if (v == 'change') watchFolder(c, st.watch);
                    if (v == 'stop') {
                      c.send(const PapersCmd.watchFolder(path: ''));
                    }
                  },
                  itemBuilder: (_) => const [
                    PopupMenuItem(value: 'change', child: Text('Watch another folder')),
                    PopupMenuItem(value: 'stop', child: Text('Stop watching')),
                  ],
                  child: Container(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 12, vertical: 9),
                    decoration: BoxDecoration(
                      color: kPapers.withValues(alpha: 0.14),
                      borderRadius: BorderRadius.circular(20),
                    ),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        const Icon(Icons.visibility_outlined,
                            size: 16, color: kPapers),
                        const SizedBox(width: 6),
                        Text('Watching $folder',
                            style: const TextStyle(
                                fontSize: 13,
                                fontWeight: FontWeight.w600,
                                color: kPapers)),
                      ],
                    ),
                  ),
                )
              else
                OutlinedButton.icon(
                  onPressed: () => watchFolder(c, ''),
                  icon: const Icon(Icons.visibility_outlined, size: 16),
                  label: const Text('Watch a folder'),
                ),
            ],
          ),
          if (!st.canOcr) ...[
            const SizedBox(height: 10),
            Text(
              'PDFs with text read now. Scans and photos need tesseract — install it (e.g. pacman -S tesseract tesseract-data-eng), then Read again.',
              style: TextStyle(fontSize: 12, color: t.nInk3),
            ),
          ],
        ],
      ),
    );
  }
}

class _InboxRow extends StatelessWidget {
  const _InboxRow({required this.c, required this.st, required this.p});

  final PapersController c;
  final PapersState st;
  final InboxPaper p;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final reading = p.status == 'reading';
    final failed = p.status == 'failed';
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 12, 14, 12),
      decoration: cardDeco(context),
      child: Row(
        children: [
          SizedBox(
            width: 50,
            height: 64,
            child: PaperThumb(path: p.thumb, kind: p.kind, sealed: p.vault),
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Flexible(
                      child: Text(p.title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 14,
                              fontWeight: FontWeight.w700,
                              color: t.nInk)),
                    ),
                    if (p.status == 'read' && p.confidence > 0) ...[
                      const SizedBox(width: 8),
                      Container(
                        padding: const EdgeInsets.symmetric(
                            horizontal: 7, vertical: 2),
                        decoration: BoxDecoration(
                          color: kPapers.withValues(alpha: 0.13),
                          borderRadius: BorderRadius.circular(6),
                        ),
                        child: Row(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            const Icon(Icons.auto_awesome,
                                size: 11, color: kPapers),
                            const SizedBox(width: 3),
                            Text('Read ${p.confidence}%',
                                style: const TextStyle(
                                    fontSize: 11,
                                    fontWeight: FontWeight.w700,
                                    color: kPapers)),
                          ],
                        ),
                      ),
                    ],
                  ],
                ),
                const SizedBox(height: 2),
                Text(p.line,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 12.5,
                        color: failed ? Tokens.error : t.nInk2)),
                if (p.status == 'read') ...[
                  const SizedBox(height: 8),
                  Wrap(
                    spacing: 6,
                    runSpacing: 6,
                    children: [
                      if (p.collection.isNotEmpty)
                        InfoChip(
                            icon: Icons.folder_outlined,
                            text: p.collection.replaceAll('>', '›')),
                      if (p.vault)
                        const InfoChip(icon: Icons.lock_outline, text: 'Vault'),
                      if (p.expiry.isNotEmpty)
                        InfoChip(
                            icon: Icons.notifications_none, text: p.expiry),
                      if (p.finance.isNotEmpty)
                        InfoChip(
                            icon: Icons.credit_card, text: p.finance),
                    ],
                  ),
                ],
              ],
            ),
          ),
          const SizedBox(width: 12),
          if (reading)
            const SizedBox(
                width: 20,
                height: 20,
                child: CircularProgressIndicator(strokeWidth: 2, color: kPapers))
          else if (failed) ...[
            TextButton(
              onPressed: () => c.send(PapersCmd.delete(id: p.id)),
              child: const Text('Remove'),
            ),
            const SizedBox(width: 6),
            OutlinedButton(
              onPressed: () => c.send(PapersCmd.readAgain(id: p.id)),
              child: const Text('Read again'),
            ),
          ] else ...[
            OutlinedButton(
              onPressed: () => editPaper(context, c, p.id),
              child: const Text('Edit'),
            ),
            const SizedBox(width: 8),
            FilledButton.icon(
              style: FilledButton.styleFrom(backgroundColor: kPapers),
              onPressed: () => c.send(PapersCmd.file(id: p.id)),
              icon: const Icon(Icons.check, size: 16),
              label: const Text('File it'),
            ),
          ],
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------- expiring --

class _Expiring extends StatelessWidget {
  const _Expiring({required this.c, required this.st});

  final PapersController c;
  final PapersState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 40),
      children: [
        Container(
          padding: const EdgeInsets.fromLTRB(18, 14, 18, 18),
          decoration: cardDeco(context),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Text('The next twelve months',
                      style: TextStyle(
                          fontSize: 14,
                          fontWeight: FontWeight.w700,
                          color: t.nInk)),
                  const Spacer(),
                  Text(
                      '${plural(st.dated, 'date')} · reminders 30 and 7 days before',
                      style: TextStyle(fontSize: 12, color: t.nInk2)),
                ],
              ),
              const SizedBox(height: 12),
              Row(
                children: [
                  for (var i = 0; i < st.months.length; i++) ...[
                    if (i > 0) const SizedBox(width: 6),
                    Expanded(child: _MonthCell(m: st.months[i])),
                  ],
                ],
              ),
            ],
          ),
        ),
        if (st.groups.isEmpty) ...[
          const SizedBox(height: 22),
          const Quiet(
            icon: Icons.event_available_outlined,
            title: 'Nothing runs out',
            body:
                'Passports, warranties, contracts and renewals show here with the date read off them, and a reminder 30 and 7 days before.',
          ),
        ],
        for (final g in st.groups) ...[
          const SizedBox(height: 24),
          Row(
            children: [
              Text(g.title,
                  style: TextStyle(
                      fontSize: 17,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
              const SizedBox(width: 8),
              Text('${g.rows.length}',
                  style: TextStyle(fontSize: 13, color: t.nInk3)),
            ],
          ),
          const SizedBox(height: 12),
          Container(
            decoration: cardDeco(context),
            child: Column(
              children: [
                for (var i = 0; i < g.rows.length; i++) ...[
                  if (i > 0) Divider(height: 1, color: t.nHair),
                  _ExpiryLine(c: c, r: g.rows[i]),
                ],
              ],
            ),
          ),
        ],
      ],
    );
  }
}

class _MonthCell extends StatelessWidget {
  const _MonthCell({required this.m});

  final MonthDots m;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 62,
      padding: const EdgeInsets.fromLTRB(8, 8, 6, 6),
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(m.label, style: TextStyle(fontSize: 11.5, color: t.nInk2)),
          const SizedBox(height: 6),
          Wrap(
            spacing: 4,
            runSpacing: 4,
            children: [
              for (final k in m.kinds)
                Container(
                  width: 9,
                  height: 9,
                  decoration: BoxDecoration(
                      color: kindColour(k), shape: BoxShape.circle),
                ),
            ],
          ),
        ],
      ),
    );
  }
}

class _ExpiryLine extends StatelessWidget {
  const _ExpiryLine({required this.c, required this.r});

  final PapersController c;
  final ExpiryRow r;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final days = r.days.toInt();
    return InkWell(
      onTap: () => openPaper(c, r.id),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 12, 16, 12),
        child: Row(
          children: [
            Ring(
                text: r.ring,
                colour: ringColour(days),
                fraction: 1 - days / 365),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(r.title,
                      style: TextStyle(
                          fontSize: 14,
                          fontWeight: FontWeight.w700,
                          color: t.nInk)),
                  const SizedBox(height: 2),
                  Text(r.snoozed ? '${r.line} · snoozed' : r.line,
                      style: TextStyle(fontSize: 12.5, color: t.nInk2)),
                ],
              ),
            ),
            KindPill(kind: r.kind, label: c.kindLabel(r.kind)),
            const SizedBox(width: 10),
            OutlinedButton(
              onPressed:
                  r.snoozed ? null : () => c.send(PapersCmd.snooze(id: r.id)),
              child: const Text('Snooze'),
            ),
            const SizedBox(width: 8),
            FilledButton(
              style: FilledButton.styleFrom(backgroundColor: kPapers),
              onPressed: () => renewed(context, c, r.id),
              child: const Text('Renewed'),
            ),
          ],
        ),
      ),
    );
  }
}

// ------------------------------------------------------------------- vault --

class _Vault extends StatefulWidget {
  const _Vault({required this.c, required this.st});

  final PapersController c;
  final PapersState st;

  @override
  State<_Vault> createState() => _VaultState();
}

class _VaultState extends State<_Vault> {
  String _pin = '';
  bool _wrong = false;
  final FocusNode _keys = FocusNode();

  @override
  void dispose() {
    _keys.dispose();
    super.dispose();
  }

  Future<void> _press(String k) async {
    final st = widget.st;
    setState(() {
      _wrong = false;
      if (k == '<') {
        if (_pin.isNotEmpty) _pin = _pin.substring(0, _pin.length - 1);
      } else if (_pin.length < 12) {
        _pin += k;
      }
    });
    if (st.pinLen > 0 && _pin.length == st.pinLen) await _unlock();
  }

  Future<void> _unlock() async {
    final err = await widget.c.attempt(PapersCmd.unlock(pin: _pin));
    if (!mounted) return;
    setState(() {
      _pin = '';
      _wrong = err != null;
    });
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.st;
    if (st.vaultOpen) {
      return Column(
        children: [
          Container(
            padding: const EdgeInsets.fromLTRB(24, 14, 20, 0),
            child: Row(
              children: [
                const Icon(Icons.lock_open_outlined, size: 18, color: kPapers),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    st.hasPin
                        ? '${plural(st.vaultN, 'paper')} in the vault · open for five minutes, or until Tulipix locks'
                        : '${plural(st.vaultN, 'paper')} in the vault · sealed on disk. Set an app PIN in Settings › Security to lock it too.',
                    style: TextStyle(fontSize: 12.5, color: t.nInk2),
                  ),
                ),
                if (st.hasPin)
                  OutlinedButton.icon(
                    onPressed: () =>
                        widget.c.send(const PapersCmd.lockVault()),
                    icon: const Icon(Icons.lock_outline, size: 16),
                    label: const Text('Lock now'),
                  ),
              ],
            ),
          ),
          Expanded(child: PapersBrowser(c: widget.c, st: st, vault: true)),
        ],
      );
    }
    final dots = st.pinLen > 0 ? st.pinLen : _pin.length.clamp(4, 12).toInt();
    return Stack(
      children: [
        // The deck's blurred shelf behind the pad: shapes only, no titles.
        Positioned.fill(
          child: Opacity(
            opacity: 0.35,
            child: Padding(
              padding: const EdgeInsets.all(24),
              child: Grid(
                min: 150,
                children: [
                  for (var i = 0; i < st.vaultN.clamp(0, 12); i++)
                    const AspectRatio(
                        aspectRatio: 0.75,
                        child: PaperThumb(path: '', kind: 'other', sealed: true)),
                ],
              ),
            ),
          ),
        ),
        Center(
          child: Container(
            width: 420,
            padding: const EdgeInsets.fromLTRB(28, 28, 28, 22),
            decoration: cardDeco(context),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                Container(
                  width: 56,
                  height: 56,
                  decoration: BoxDecoration(
                    color: kPapers.withValues(alpha: 0.16),
                    borderRadius: BorderRadius.circular(14),
                  ),
                  child: const Icon(Icons.lock_outline, color: kPapers),
                ),
                const SizedBox(height: 16),
                Text(
                    st.vaultN == 0
                        ? 'The vault is empty'
                        : '${plural(st.vaultN, 'paper')} ${st.vaultN == 1 ? 'is' : 'are'} in the vault',
                    style: TextStyle(
                        fontSize: 21,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
                const SizedBox(height: 8),
                Text(
                  'Passports, medical results and tax returns are sealed and hidden from search until you unlock with your app PIN.',
                  textAlign: TextAlign.center,
                  style: TextStyle(fontSize: 13, height: 1.45, color: t.nInk2),
                ),
                const SizedBox(height: 18),
                Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    for (var i = 0; i < dots; i++)
                      Container(
                        width: 13,
                        height: 13,
                        margin: const EdgeInsets.symmetric(horizontal: 5),
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: i < _pin.length
                              ? (_wrong ? Tokens.error : kPapers)
                              : Colors.transparent,
                          border: Border.all(
                              color: _wrong ? Tokens.error : t.nInk3,
                              width: 1.5),
                        ),
                      ),
                  ],
                ),
                if (_wrong) ...[
                  const SizedBox(height: 8),
                  const Text('That PIN is not right',
                      style: TextStyle(fontSize: 12, color: Tokens.error)),
                ],
                const SizedBox(height: 16),
                KeyboardListener(
                  focusNode: _keys,
                  autofocus: true,
                  onKeyEvent: (e) {
                    if (e is! KeyDownEvent) return;
                    final ch = e.character ?? '';
                    if (RegExp(r'^[0-9]$').hasMatch(ch)) _press(ch);
                    if (e.logicalKey == LogicalKeyboardKey.backspace) _press('<');
                    if (e.logicalKey == LogicalKeyboardKey.enter &&
                        _pin.isNotEmpty) {
                      _unlock();
                    }
                  },
                  child: _Keypad(onKey: _press),
                ),
                if (st.pinLen == 0) ...[
                  const SizedBox(height: 12),
                  FilledButton(
                    style: FilledButton.styleFrom(backgroundColor: kPapers),
                    onPressed: _pin.isEmpty ? null : _unlock,
                    child: const Text('Unlock'),
                  ),
                ],
                const SizedBox(height: 14),
                Text('Locks again after 5 minutes, or when Tulipix locks.',
                    style: TextStyle(fontSize: 11.5, color: t.nInk3)),
              ],
            ),
          ),
        ),
      ],
    );
  }
}

class _Keypad extends StatelessWidget {
  const _Keypad({required this.onKey});

  final ValueChanged<String> onKey;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget key(String k) => Padding(
          padding: const EdgeInsets.all(4),
          child: Material(
            color: t.nChip,
            borderRadius: BorderRadius.circular(10),
            child: InkWell(
              borderRadius: BorderRadius.circular(10),
              onTap: () => onKey(k),
              child: SizedBox(
                width: 62,
                height: 46,
                child: Center(
                  child: k == '<'
                      ? Icon(Icons.chevron_left, color: t.nInk2)
                      : Text(k,
                          style: TextStyle(
                              fontSize: 18,
                              fontWeight: FontWeight.w600,
                              color: t.nInk)),
                ),
              ),
            ),
          ),
        );
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final row in const [
          ['1', '2', '3'],
          ['4', '5', '6'],
          ['7', '8', '9'],
          ['', '0', '<'],
        ])
          Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              for (final k in row)
                k.isEmpty ? const SizedBox(width: 70) : key(k),
            ],
          ),
      ],
    );
  }
}

// ----------------------------------------------------------------- actions --

/// Open a paper beside the grid on All papers — or the Vault, for a sealed one.
Future<void> openPaper(PapersController c, int id) async {
  await c.send(PapersCmd.select(id: id));
  await c.send(const PapersCmd.setTab(tab: 'all'));
}

Future<void> chooseFiles(PapersController c) async {
  final paths = await dialogPickFiles(
    title: 'Add papers',
    initial: '',
    label: 'PDFs and pictures',
    extensions: kPaperExtensions,
  );
  if (paths.isNotEmpty) await c.send(PapersCmd.addFiles(paths: paths));
}

Future<void> watchFolder(PapersController c, String initial) async {
  final path = await dialogPickFolder(
      title: 'A folder your scanner saves to', initial: initial);
  if (path != null && path.isNotEmpty) {
    await c.send(PapersCmd.watchFolder(path: path));
  }
}

/// Scan with phone: a QR code for a page on the same wifi. While it is open
/// the Inbox asks every few seconds, so papers appear as they arrive.
Future<void> showPhone(BuildContext context, PapersController c) async {
  try {
    final link = await papersPhone();
    if (!context.mounted) return;
    final qr = link.qr;
    final tick =
        Timer.periodic(const Duration(seconds: 3), (_) => c.refresh(quiet: true));
    await c.send(const PapersCmd.setTab(tab: 'inbox'));
    if (!context.mounted) {
      tick.cancel();
      return;
    }
    await showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Scan with phone'),
        content: SizedBox(
          width: 320,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (qr != null)
                Container(
                  width: 240,
                  height: 240,
                  color: Colors.white,
                  child: QrView(code: qr),
                ),
              const SizedBox(height: 12),
              SelectableText(link.url,
                  textAlign: TextAlign.center,
                  style: const TextStyle(fontSize: 12)),
              const SizedBox(height: 8),
              const Text(
                'Open it on a phone on the same wifi, then take photos or pick PDFs. They are read here, not on the phone. A new code ends the old one.',
                textAlign: TextAlign.center,
                style: TextStyle(fontSize: 12),
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Clipboard.setData(ClipboardData(text: link.url)),
            child: const Text('Copy link'),
          ),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: kPapers),
            onPressed: () => Navigator.pop(ctx),
            child: const Text('Done'),
          ),
        ],
      ),
    );
    tick.cancel();
  } catch (e) {
    c.say(plainError(e));
  }
}

/// Renewed: the next date it comes round, or none.
Future<void> renewed(BuildContext context, PapersController c, int id) async {
  final how = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Renewed'),
      content: const Text(
          'Pick the next date it runs out and the reminders start again — or say it does not come round again.'),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: const Text('Cancel')),
        TextButton(
            onPressed: () => Navigator.pop(ctx, 'none'),
            child: const Text('No next date')),
        FilledButton(
          style: FilledButton.styleFrom(backgroundColor: kPapers),
          onPressed: () => Navigator.pop(ctx, 'pick'),
          child: const Text('Pick the date…'),
        ),
      ],
    ),
  );
  if (how == null || !context.mounted) return;
  if (how == 'none') {
    await c.send(PapersCmd.renewed(id: id, next: ''));
    return;
  }
  final now = DateTime.now();
  final d = await showDatePicker(
    context: context,
    initialDate: DateTime(now.year + 1, now.month, now.day),
    firstDate: now,
    lastDate: DateTime(now.year + 30),
  );
  if (d == null) return;
  await c.send(PapersCmd.renewed(id: id, next: isoDay(d)));
}

String isoDay(DateTime d) =>
    '${d.year.toString().padLeft(4, '0')}-${d.month.toString().padLeft(2, '0')}-${d.day.toString().padLeft(2, '0')}';

const List<String> kEndLabels = [
  'Expires',
  'Renews',
  'Ends',
  'Due',
  'Warranty ends',
  'Service due',
  'Renew by',
];

/// The editor: every field read off the paper, to put right.
Future<void> editPaper(BuildContext context, PapersController c, int id) async {
  final PaperDraft d;
  try {
    d = await papersDraft(id: id);
  } catch (e) {
    c.say(plainError(e));
    return;
  }
  if (!context.mounted) return;
  final st = c.state;
  final title = TextEditingController(text: d.title);
  final collection = TextEditingController(text: d.collection);
  final date = TextEditingController(text: d.docDate);
  final amount = TextEditingController(text: d.amount);
  final merchant = TextEditingController(text: d.merchant);
  final expires = TextEditingController(text: d.expires);
  final serial = TextEditingController(text: d.serial);
  var kind = d.kind;
  var label = kEndLabels.contains(d.expiryLabel) ? d.expiryLabel : 'Expires';
  String? error;
  final kinds = [
    for (final k in st?.kinds ?? const <KindCount>[])
      if (k.id != 'all') k,
  ];
  if (!kinds.any((k) => k.id == kind)) {
    kinds.add(KindCount(id: kind, label: c.kindLabel(kind), n: 0));
  }

  Widget dateField(StateSetter set, TextEditingController ctl, String name) =>
      TextField(
        controller: ctl,
        decoration: InputDecoration(
          labelText: name,
          hintText: '2026-09-14',
          suffixIcon: IconButton(
            tooltip: 'Pick a date',
            icon: const Icon(Icons.calendar_today_outlined, size: 17),
            onPressed: () async {
              final now = DateTime.now();
              final picked = await showDatePicker(
                context: context,
                initialDate: DateTime.tryParse(ctl.text) ?? now,
                firstDate: DateTime(1950),
                lastDate: DateTime(now.year + 40),
              );
              if (picked != null) set(() => ctl.text = isoDay(picked));
            },
          ),
        ),
      );

  await showDialog<void>(
    context: context,
    builder: (ctx) => StatefulBuilder(
      builder: (ctx, set) => AlertDialog(
        title: const Text('Edit paper'),
        content: SizedBox(
          width: 560,
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                TextField(
                    controller: title,
                    decoration: const InputDecoration(labelText: 'Name')),
                Row(
                  children: [
                    Expanded(
                      child: DropdownButtonFormField<String>(
                        initialValue: kind,
                        decoration: const InputDecoration(labelText: 'Kind'),
                        items: [
                          for (final k in kinds)
                            DropdownMenuItem(value: k.id, child: Text(k.label)),
                        ],
                        onChanged: (v) => set(() => kind = v ?? kind),
                      ),
                    ),
                    const SizedBox(width: 12),
                    Expanded(
                      flex: 2,
                      child: TextField(
                          controller: collection,
                          decoration: const InputDecoration(
                              labelText: 'Collection',
                              hintText: 'Home › Appliances')),
                    ),
                  ],
                ),
                if ((st?.names ?? const <String>[]).isNotEmpty) ...[
                  const SizedBox(height: 8),
                  Align(
                    alignment: Alignment.centerLeft,
                    child: Wrap(
                      spacing: 6,
                      runSpacing: 6,
                      children: [
                        for (final n in st!.names)
                          ActionChip(
                            label: Text(n, style: const TextStyle(fontSize: 12)),
                            onPressed: () => set(() => collection.text = n),
                          ),
                      ],
                    ),
                  ),
                ],
                Row(
                  children: [
                    Expanded(child: dateField(set, date, 'Date on it')),
                    const SizedBox(width: 12),
                    Expanded(
                      child: TextField(
                          controller: amount,
                          decoration:
                              const InputDecoration(labelText: 'Amount')),
                    ),
                  ],
                ),
                TextField(
                    controller: merchant,
                    decoration:
                        const InputDecoration(labelText: 'Shop or sender')),
                Row(
                  children: [
                    Expanded(
                      child: DropdownButtonFormField<String>(
                        initialValue: label,
                        decoration:
                            const InputDecoration(labelText: 'What the date is'),
                        items: [
                          for (final l in kEndLabels)
                            DropdownMenuItem(value: l, child: Text(l)),
                        ],
                        onChanged: (v) => set(() => label = v ?? label),
                      ),
                    ),
                    const SizedBox(width: 12),
                    Expanded(child: dateField(set, expires, 'Date')),
                  ],
                ),
                TextField(
                    controller: serial,
                    decoration:
                        const InputDecoration(labelText: 'Serial number')),
                if (error != null) ...[
                  const SizedBox(height: 10),
                  Text(error!,
                      style:
                          const TextStyle(color: Tokens.error, fontSize: 12.5)),
                ],
              ],
            ),
          ),
        ),
        actions: [
          TextButton(
              onPressed: () => Navigator.pop(ctx),
              child: const Text('Cancel')),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: kPapers),
            onPressed: () async {
              final err = await c.attempt(PapersCmd.save(
                id: d.id,
                title: title.text,
                kind: kind,
                collection: collection.text,
                docDate: date.text,
                amount: amount.text,
                merchant: merchant.text,
                expires: expires.text,
                expiryLabel: label,
                serial: serial.text,
              ));
              if (!ctx.mounted) return;
              if (err == null) {
                Navigator.pop(ctx);
              } else {
                set(() => error = err);
              }
            },
            child: const Text('Save'),
          ),
        ],
      ),
    ),
  );
}

Future<void> deletePaper(
    BuildContext context, PapersController c, int id, String title) async {
  if (await confirm(context, 'Delete $title?',
      'Its copy here and what was read off it go. The file you added it from is not touched.')) {
    await c.send(PapersCmd.delete(id: id));
  }
}

// ------------------------------------------------------------------ shared --

/// A paper's page one: the picture of it, or the deck's page — a coloured top
/// and grey lines — when there is none or it is sealed.
class PaperThumb extends StatelessWidget {
  const PaperThumb({
    super.key,
    required this.path,
    required this.kind,
    this.sealed = false,
  });

  final String path;
  final String kind;
  final bool sealed;

  @override
  Widget build(BuildContext context) {
    final page = _Page(kind: kind, sealed: sealed);
    return ClipRRect(
      borderRadius: BorderRadius.circular(6),
      child: path.isEmpty || sealed
          ? page
          : Image.file(
              File(path),
              fit: BoxFit.cover,
              alignment: Alignment.topCenter,
              cacheWidth: 400,
              gaplessPlayback: true,
              errorBuilder: (_, __, ___) => page,
            ),
    );
  }
}

class _Page extends StatelessWidget {
  const _Page({required this.kind, required this.sealed});

  final String kind;
  final bool sealed;

  @override
  Widget build(BuildContext context) {
    const widths = [0.92, 0.7, 0.84, 0.55, 0.78, 0.64, 0.88];
    return Container(
      decoration: BoxDecoration(
        color: const Color(0xFFFBFAF6),
        border: Border.all(color: const Color(0x14000000)),
      ),
      child: LayoutBuilder(builder: (context, box) {
        final w = box.maxWidth;
        final pad = w * 0.1;
        return Stack(
          children: [
            Padding(
              padding: EdgeInsets.fromLTRB(pad, pad, pad, pad),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Container(
                    width: (w - pad * 2) * 0.55,
                    height: (w * 0.05).clamp(3, 8).toDouble(),
                    decoration: BoxDecoration(
                      color: kindColour(kind),
                      borderRadius: BorderRadius.circular(3),
                    ),
                  ),
                  SizedBox(height: w * 0.08),
                  for (final f in widths)
                    Padding(
                      padding: EdgeInsets.only(bottom: w * 0.045),
                      child: Container(
                        width: (w - pad * 2) * f,
                        height: (w * 0.018).clamp(1.5, 3).toDouble(),
                        color: const Color(0xFFD9D6CC),
                      ),
                    ),
                ],
              ),
            ),
            if (sealed)
              Positioned(
                right: pad * 0.6,
                bottom: pad * 0.6,
                child: Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
                  decoration: BoxDecoration(
                    border: Border.all(color: const Color(0xFFE11D48)),
                    borderRadius: BorderRadius.circular(4),
                  ),
                  child: const Text('Vault',
                      style: TextStyle(
                          fontSize: 9.5,
                          fontWeight: FontWeight.w700,
                          color: Color(0xFFE11D48))),
                ),
              ),
          ],
        );
      }),
    );
  }
}

/// Days left as a ring: "42d", "14mo".
class Ring extends StatelessWidget {
  const Ring({
    super.key,
    required this.text,
    required this.colour,
    required this.fraction,
  });

  final String text;
  final Color colour;
  final double fraction;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      width: 46,
      height: 46,
      child: Stack(
        alignment: Alignment.center,
        children: [
          SizedBox.expand(
            child: CircularProgressIndicator(
              value: fraction.clamp(0.04, 1).toDouble(),
              strokeWidth: 4,
              color: colour,
              backgroundColor: t.nHair,
            ),
          ),
          Text(text,
              style: TextStyle(
                  fontSize: 11.5, fontWeight: FontWeight.w800, color: t.nInk)),
        ],
      ),
    );
  }
}

class InfoChip extends StatelessWidget {
  const InfoChip({super.key, required this.icon, required this.text});

  final IconData icon;
  final String text;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 5),
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(20),
        border: Border.all(color: t.nHair),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 13, color: t.nInk2),
          const SizedBox(width: 5),
          Text(text,
              style: TextStyle(
                  fontSize: 12, fontWeight: FontWeight.w600, color: t.nInk2)),
        ],
      ),
    );
  }
}

class KindPill extends StatelessWidget {
  const KindPill({super.key, required this.kind, required this.label});

  final String kind;
  final String label;

  @override
  Widget build(BuildContext context) {
    final c = kindColour(kind);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
      decoration: BoxDecoration(
        color: c.withValues(alpha: 0.13),
        borderRadius: BorderRadius.circular(6),
      ),
      child: Text(label,
          style: TextStyle(
              fontSize: 11, fontWeight: FontWeight.w700, color: c)),
    );
  }
}
