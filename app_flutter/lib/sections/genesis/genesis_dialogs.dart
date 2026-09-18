// Genesis's four popups: settings, search history, one record in full, and the
// are-you-sure box the destructive controls go through.

import 'dart:io';

import 'package:flutter/material.dart';

import '../../platform/pick.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/genesis.dart';
import 'genesis_controller.dart';
import 'genesis_page.dart';

/// Its own scrim, so it reads as a question asked on top of whatever is already
/// open.
Future<bool> confirm(
  BuildContext context, {
  required String title,
  required String body,
  required String confirmLabel,
}) async {
  final t = context.tokens;
  final yes = await showDialog<bool>(
    context: context,
    builder: (context) => AlertDialog(
      backgroundColor: t.nCard,
      title: Text(title,
          style: TextStyle(
              fontSize: 15, fontWeight: FontWeight.w800, color: t.nInk)),
      content: SizedBox(
        width: 340,
        child: Text(body, style: TextStyle(fontSize: 12.5, color: t.nInk2)),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(false),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.of(context).pop(true),
          style: FilledButton.styleFrom(backgroundColor: Tokens.error),
          child: Text(confirmLabel),
        ),
      ],
    ),
  );
  return yes ?? false;
}

// ── settings ────────────────────────────────────────────────────────────────

Future<void> openGenesisSettings(
  BuildContext context,
  GenesisController controller,
) async {
  await controller.send(const GenesisCmd.settingsOpen());
  if (!context.mounted) return;
  await showDialog<void>(
    context: context,
    builder: (context) => _Settings(controller: controller),
  );
}

class _Settings extends StatefulWidget {
  const _Settings({required this.controller});

  final GenesisController controller;

  @override
  State<_Settings> createState() => _SettingsState();
}

class _SettingsState extends State<_Settings> {
  late final TextEditingController _mirrors =
      TextEditingController(text: widget.controller.state?.mirrors ?? '');

  @override
  void dispose() {
    _mirrors.dispose();
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
          backgroundColor: t.nCard,
          shape:
              RoundedRectangleBorder(borderRadius: BorderRadius.circular(16)),
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 560, maxHeight: 700),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                _Head(
                  title: 'Genesis settings',
                  onClose: () => Navigator.of(context).pop(),
                ),
                Flexible(
                  child: ListView(
                    // Without this the list takes the full 700 of the
                    // constraint whatever it holds, and the button row is
                    // pushed to the bottom of a mostly empty card.
                    shrinkWrap: true,
                    padding: const EdgeInsets.fromLTRB(20, 4, 20, 20),
                    children: [
                      const _Label('Download folder'),
                      const SizedBox(height: 8),
                      Row(
                        children: [
                          Expanded(
                            child: Container(
                              height: 40,
                              alignment: Alignment.centerLeft,
                              padding:
                                  const EdgeInsets.symmetric(horizontal: 12),
                              decoration: BoxDecoration(
                                color: t.nChip,
                                borderRadius: BorderRadius.circular(10),
                                border: Border.all(color: t.nHair),
                              ),
                              child: Text(
                                st.dest.isEmpty ? 'not set' : st.dest,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(
                                    fontSize: 12.5,
                                    color: st.dest.isEmpty ? t.nInk2 : t.nInk),
                              ),
                            ),
                          ),
                          const SizedBox(width: 10),
                          SizedBox(
                            height: 40,
                            child: FilledButton(
                              // The Slint build opens the desktop's own folder
                              // dialog here; typing an absolute path from
                              // memory was never the same control.
                              onPressed: () async {
                                final path = await pickDirectory(
                                    initial: st.dest.isEmpty ? null : st.dest);
                                if (path == null) return;
                                await widget.controller
                                    .send(GenesisCmd.setDest(path: path));
                              },
                              style:
                                  FilledButton.styleFrom(backgroundColor: kGen),
                              child: const Text('Change…'),
                            ),
                          ),
                        ],
                      ),
                      const SizedBox(height: 8),
                      const _Hint(
                          'Watched by your library — anything downloaded '
                          'here shows up in Books without importing it by '
                          'hand.'),
                      const SizedBox(height: 20),
                      _Label('Results per search — ${st.limit}'),
                      Slider(
                        value: st.limit.clamp(1, 49).toDouble(),
                        min: 1,
                        max: 49,
                        divisions: 48,
                        activeColor: kGen,
                        label: '${st.limit}',
                        onChanged: (v) => widget.controller
                            .send(GenesisCmd.setLimit(value: v.round())),
                      ),
                      if (st.activeUrl.isNotEmpty) ...[
                        const SizedBox(height: 12),
                        // The one thing that says *where* the books are
                        // actually coming from.
                        Container(
                          padding: const EdgeInsets.all(12),
                          decoration: BoxDecoration(
                            color: t.nChip,
                            borderRadius: BorderRadius.circular(10),
                          ),
                          child: Column(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              const _Label('Working mirror'),
                              const SizedBox(height: 4),
                              SelectableText(st.activeUrl,
                                  style:
                                      TextStyle(fontSize: 12, color: t.nInk)),
                            ],
                          ),
                        ),
                      ],
                      const SizedBox(height: 20),
                      const _Label(
                          'Mirrors — one per line, tried top to bottom'),
                      const SizedBox(height: 8),
                      TextField(
                        controller: _mirrors,
                        maxLines: 6,
                        onChanged: (v) => widget.controller
                            .send(GenesisCmd.setMirrors(value: v)),
                        style: TextStyle(fontSize: 12, color: t.nInk),
                        decoration: InputDecoration(
                          border: const OutlineInputBorder(),
                          hintText: st.mirrorsDefault,
                          hintMaxLines: 6,
                        ),
                      ),
                      const SizedBox(height: 8),
                      const _Hint(
                          'Leave empty for the built-in list. Each entry is '
                          'health-checked with a real search before it is used '
                          '— a mirror can serve a fine front page while its '
                          'search endpoint is down.'),
                    ],
                  ),
                ),
                Divider(height: 1, color: t.nHair),
                Padding(
                  padding: const EdgeInsets.all(14),
                  child: Row(
                    children: [
                      const Spacer(),
                      OutlinedButton(
                        onPressed: () async {
                          await widget.controller
                              .send(const GenesisCmd.settingsReset());
                          // Reset clears the stored mirrors and the download
                          // folder; the box has to show that, not the text
                          // that was in it a moment ago.
                          _mirrors.text =
                              widget.controller.state?.mirrors ?? '';
                        },
                        child: const Text('Reset'),
                      ),
                      const SizedBox(width: 9),
                      FilledButton(
                        onPressed: () => widget.controller
                            .send(const GenesisCmd.settingsSave()),
                        style: FilledButton.styleFrom(backgroundColor: kGen),
                        child: Text(st.settingsSaved ? 'Saved ✓' : 'Save'),
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
}

// ── search history ──────────────────────────────────────────────────────────

/// Ten a page, ten pages — the popup shows the last hundred searches.
const int _perPage = 10;

Future<void> openHistory(
  BuildContext context,
  GenesisController controller,
) async {
  await controller.send(const GenesisCmd.historyOpen());
  if (!context.mounted) return;
  await showDialog<void>(
    context: context,
    builder: (context) => _History(controller: controller),
  );
}

class _History extends StatefulWidget {
  const _History({required this.controller});

  final GenesisController controller;

  @override
  State<_History> createState() => _HistoryState();
}

class _HistoryState extends State<_History> {
  int _page = 0;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: widget.controller,
      builder: (context, _) {
        final st = widget.controller.state;
        if (st == null) return const SizedBox.shrink();
        final pages = (st.history.length / _perPage).ceil().clamp(1, 10);
        final page = _page.clamp(0, pages - 1);
        final from = page * _perPage;
        final rows = st.history.skip(from).take(_perPage).toList();
        return Dialog(
          backgroundColor: t.nCard,
          shape:
              RoundedRectangleBorder(borderRadius: BorderRadius.circular(16)),
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 560, maxHeight: 620),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                // Paging and Clear live up here beside the close button, so the
                // popup is one block of searches with its controls on one line.
                _Head(
                  title: 'Recent searches',
                  onClose: () => Navigator.of(context).pop(),
                  trailing: [
                    if (pages > 1) ...[
                      IconButton(
                        icon: const Icon(Icons.chevron_left, size: 18),
                        onPressed: page == 0
                            ? null
                            : () => setState(() => _page = page - 1),
                      ),
                      Text('${page + 1} / $pages',
                          style: TextStyle(fontSize: 12, color: t.nInk2)),
                      IconButton(
                        icon: const Icon(Icons.chevron_right, size: 18),
                        onPressed: page >= pages - 1
                            ? null
                            : () => setState(() => _page = page + 1),
                      ),
                    ],
                    if (st.history.isNotEmpty)
                      TextButton(
                        onPressed: () async {
                          final yes = await confirm(
                            context,
                            title: 'Clear search history?',
                            body: 'Forgets every search remembered here. '
                                'Downloads and their records are untouched.',
                            confirmLabel: 'Clear',
                          );
                          if (!yes) return;
                          await widget.controller
                              .send(const GenesisCmd.historyClear());
                          if (context.mounted) setState(() => _page = 0);
                        },
                        child: const Text('Clear'),
                      ),
                  ],
                ),
                Flexible(
                  child: st.history.isEmpty
                      ? Padding(
                          padding: const EdgeInsets.all(24),
                          child: Text('Nothing searched yet.',
                              style: TextStyle(fontSize: 12.5, color: t.nInk2)),
                        )
                      : ListView.builder(
                          padding: const EdgeInsets.fromLTRB(12, 0, 12, 12),
                          itemCount: rows.length,
                          itemBuilder: (context, i) => _HistoryRow(
                            search: rows[i],
                            onRun: () {
                              Navigator.of(context).pop();
                              widget.controller
                                  .send(GenesisCmd.historyRun(index: from + i));
                            },
                          ),
                        ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

class _HistoryRow extends StatelessWidget {
  const _HistoryRow({required this.search, required this.onRun});

  final GenSearch search;
  final VoidCallback onRun;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListTile(
      dense: true,
      onTap: onRun,
      title: Text(search.terms,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(
              fontSize: 13, fontWeight: FontWeight.w600, color: t.nInk)),
      subtitle: Text(
          '${search.field} · ${search.format} · ${search.language} · '
          '${search.results} results',
          style: TextStyle(fontSize: 11, color: t.nInk3)),
      trailing:
          Text(search.when, style: TextStyle(fontSize: 11, color: t.nInk3)),
    );
  }
}

// ── one record, in full ─────────────────────────────────────────────────────

/// Fetched from the mirror's record page the first time and cached in
/// genesis.db, so opening the same book again costs nothing.
Future<void> openRecord(
  BuildContext context,
  GenesisController controller,
  String md5,
) async {
  await controller.send(GenesisCmd.openDetails(md5: md5));
  if (!context.mounted) return;
  await showDialog<void>(
    context: context,
    builder: (context) => _Record(controller: controller),
  );
  await controller.send(const GenesisCmd.closeDetails());
}

class _Record extends StatelessWidget {
  const _Record({required this.controller});

  final GenesisController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: controller,
      builder: (context, _) {
        final st = controller.state;
        if (st == null) return const SizedBox.shrink();
        final d = st.details;
        return Dialog(
          backgroundColor: t.nCard,
          shape:
              RoundedRectangleBorder(borderRadius: BorderRadius.circular(16)),
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 680, maxHeight: 640),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                _Head(
                  title: 'Book details',
                  onClose: () => Navigator.of(context).pop(),
                  trailing: [
                    if (st.detailsLoading)
                      Text('fetching…',
                          style: TextStyle(fontSize: 12, color: t.nInk3)),
                  ],
                ),
                Flexible(
                  child: Padding(
                    padding: const EdgeInsets.fromLTRB(20, 0, 20, 20),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        SizedBox(
                          width: 170,
                          height: 240,
                          child: _Cover(details: d),
                        ),
                        const SizedBox(width: 18),
                        Expanded(
                          child: SingleChildScrollView(
                            child: _Fields(controller: controller, state: st),
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

class _Cover extends StatelessWidget {
  const _Cover({required this.details});

  final GenDetails details;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ClipRRect(
      borderRadius: BorderRadius.circular(12),
      child: Stack(
        fit: StackFit.expand,
        children: [
          ColoredBox(color: kGen.withValues(alpha: 0.14)),
          if (details.cover.isNotEmpty)
            Image.file(File(details.cover),
                fit: BoxFit.cover,
                errorBuilder: (_, __, ___) => const SizedBox.shrink())
          else
            Center(
              child: Text(details.extension_,
                  style: TextStyle(
                      fontSize: 22,
                      fontWeight: FontWeight.w800,
                      color: t.nInk3)),
            ),
        ],
      ),
    );
  }
}

class _Fields extends StatelessWidget {
  const _Fields({required this.controller, required this.state});

  final GenesisController controller;
  final GenesisState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final d = state.details;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(d.title,
            style: TextStyle(
                fontSize: 17, fontWeight: FontWeight.w800, color: t.nInk)),
        if (d.series.isNotEmpty)
          Text('Series: ${d.series}',
              style: TextStyle(fontSize: 12, color: t.nInk2)),
        const SizedBox(height: 4),
        Text(d.authors.isEmpty ? 'Unknown author' : d.authors,
            style: TextStyle(fontSize: 13, color: t.nInk2)),
        const SizedBox(height: 10),
        Wrap(
          spacing: 7,
          runSpacing: 7,
          children: [
            for (final chip in [
              d.extension_,
              d.year,
              d.pages.isEmpty ? '' : '${d.pages} pages',
              d.language,
              d.size,
            ])
              if (chip.isNotEmpty) GenChip(label: chip),
          ],
        ),
        const SizedBox(height: 10),
        if (d.publisher.isNotEmpty)
          Text('Publisher: ${d.publisher}',
              style: TextStyle(fontSize: 12, color: t.nInk2)),
        if (d.isbn.isNotEmpty)
          Text('ISBN: ${d.isbn}',
              style: TextStyle(fontSize: 12, color: t.nInk2)),
        Text('MD5: ${d.md5}', style: TextStyle(fontSize: 11, color: t.nInk3)),
        if (d.description.isNotEmpty) ...[
          const SizedBox(height: 10),
          Text(d.description, style: TextStyle(fontSize: 12.5, color: t.nInk2)),
        ],
        if (state.detailsError.isNotEmpty) ...[
          const SizedBox(height: 10),
          Text(state.detailsError,
              style: const TextStyle(fontSize: 12, color: Tokens.error)),
        ],
        if (d.source.isNotEmpty) ...[
          const SizedBox(height: 12),
          InkWell(
            onTap: () => copyLink(context, d.source),
            child: Text(d.source,
                style: const TextStyle(
                    fontSize: 11.5,
                    color: kGen,
                    decoration: TextDecoration.underline)),
          ),
        ],
        const SizedBox(height: 14),
        if (d.have)
          const Text('Already in your library',
              style: TextStyle(
                  fontSize: 12.5,
                  fontWeight: FontWeight.w700,
                  color: Tokens.ok))
        else
          FilledButton.icon(
            onPressed: state.canDownload && !state.busy
                ? () {
                    Navigator.of(context).pop();
                    controller.send(GenesisCmd.download(md5: d.md5));
                  }
                : null,
            icon: const Icon(Icons.download, size: 15),
            label: Text('Download${d.size.isEmpty ? "" : " · ${d.size}"}'),
            style: FilledButton.styleFrom(backgroundColor: kGen),
          ),
      ],
    );
  }
}

// ── shared bits ─────────────────────────────────────────────────────────────

class _Head extends StatelessWidget {
  const _Head({
    required this.title,
    required this.onClose,
    this.trailing = const [],
  });

  final String title;
  final VoidCallback onClose;
  final List<Widget> trailing;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 16, 8, 8),
      child: Row(
        children: [
          Expanded(
            child: Text(title,
                style: TextStyle(
                    fontSize: 16, fontWeight: FontWeight.w800, color: t.nInk)),
          ),
          ...trailing,
          IconButton(
            icon: const Icon(Icons.close, size: 16),
            onPressed: onClose,
          ),
        ],
      ),
    );
  }
}

class _Label extends StatelessWidget {
  const _Label(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Text(
        text,
        style: TextStyle(
          fontSize: 11,
          fontWeight: FontWeight.w700,
          letterSpacing: 0.5,
          color: context.tokens.nInk3,
        ),
      );
}

class _Hint extends StatelessWidget {
  const _Hint(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Text(
        text,
        style: TextStyle(fontSize: 11.5, color: context.tokens.nInk3),
      );
}
