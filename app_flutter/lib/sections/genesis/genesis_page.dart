// Genesis — book search and download, a sub-page of Books.
//
// One `phase` string drives every state this page can be in, rather than five
// booleans that can disagree with each other:
//
//   idle      — nothing searched yet
//   probing   — finding a mirror whose *search* works (3-15 s on a cold start)
//   searching — a query is out
//   ready     — results, or an honest "no results"
//   failed    — every mirror refused, with the reasons
//
// `probing` earns its own state: mirrors are health-checked by running a real
// search, because plenty of them serve a healthy front page while their search
// endpoint 500s. That check is slow, and without something on screen naming it
// the wait reads as a hang.

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/first_load.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/genesis.dart';
import 'genesis_controller.dart';
import 'genesis_dialogs.dart';

/// Genesis lives inside Books and wears its accent, the way the reader does.
const Color kGen = Tokens.secBooks;

class GenesisPage extends StatefulWidget {
  const GenesisPage({super.key, required this.onBack});

  final VoidCallback onBack;

  @override
  State<GenesisPage> createState() => _GenesisPageState();
}

class _GenesisPageState extends State<GenesisPage> {
  final GenesisController _c = GenesisController();
  final TextEditingController _terms = TextEditingController();

  /// The last value Rust reported. The box is not pushed over the bridge per
  /// keystroke, so `state.terms` sits at the last submitted search while
  /// someone types the next one — only a *change* on the Rust side (a history
  /// replay, a clear) should move the field.
  String _lastTerms = '';

  @override
  void initState() {
    super.initState();
    _c.addListener(_syncTerms);
    WidgetsBinding.instance
        .addPostFrameCallback((_) => _c.send(const GenesisCmd.enter()));
  }

  void _syncTerms() {
    final terms = _c.state?.terms ?? '';
    if (terms == _lastTerms) return;
    _lastTerms = terms;
    _terms.text = terms;
  }

  @override
  void dispose() {
    _c.removeListener(_syncTerms);
    _terms.dispose();
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
          child: st == null
              ? FirstLoad(error: _c.error, onRetry: _c.refresh)
              : Padding(
                  padding: const EdgeInsets.all(24),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      _Header(
                        controller: _c,
                        state: st,
                        onBack: widget.onBack,
                      ),
                      const SizedBox(height: 16),
                      _SearchRow(
                        controller: _c,
                        state: st,
                        terms: _terms,
                      ),
                      const SizedBox(height: 16),
                      _Filters(controller: _c, state: st),
                      const SizedBox(height: 16),
                      Expanded(child: _Body(controller: _c, state: st)),
                      if (st.busy) ...[
                        const SizedBox(height: 12),
                        _DownloadStrip(controller: _c, state: st),
                      ],
                      if (!st.busy && st.doneName.isNotEmpty) ...[
                        const SizedBox(height: 12),
                        _DoneStrip(controller: _c, state: st),
                      ],
                      if (_c.error != null) ...[
                        const SizedBox(height: 12),
                        _ErrorBar(controller: _c),
                      ],
                    ],
                  ),
                ),
        );
      },
    );
  }
}

// ── header ──────────────────────────────────────────────────────────────────

class _Header extends StatelessWidget {
  const _Header({
    required this.controller,
    required this.state,
    required this.onBack,
  });

  final GenesisController controller;
  final GenesisState state;
  final VoidCallback onBack;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      children: [
        _RoundBtn(icon: Icons.chevron_left, onTap: onBack),
        const SizedBox(width: 14),
        Container(
          height: 46,
          padding: const EdgeInsets.fromLTRB(16, 0, 18, 0),
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: kGen.withValues(alpha: 0.12),
            borderRadius: BorderRadius.circular(23),
            border: Border.all(color: kGen.withValues(alpha: 0.5), width: 1.5),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              const Icon(Icons.menu_book_outlined, size: 19, color: kGen),
              const SizedBox(width: 10),
              // The Books section's own button stays "Genesis"; once you are on
              // the page, it says what it is.
              Text('Genesis — Book Finder',
                  style: TextStyle(
                      fontSize: 16,
                      fontWeight: FontWeight.w800,
                      color: t.nInk)),
            ],
          ),
        ),
        const Spacer(),
        if (state.mirror.isNotEmpty) _MirrorChip(state: state),
        const SizedBox(width: 10),
        _RoundBtn(
          icon: Icons.history,
          onTap: () => openHistory(context, controller),
        ),
        const SizedBox(width: 8),
        _RoundBtn(
          icon: Icons.settings_outlined,
          onTap: () => openGenesisSettings(context, controller),
        ),
      ],
    );
  }
}

class _MirrorChip extends StatelessWidget {
  const _MirrorChip({required this.state});

  final GenesisState state;

  @override
  Widget build(BuildContext context) {
    final bad = state.mirrorTone == 'bad';
    final tint = bad ? Tokens.error : Tokens.ok;
    return Container(
      height: 30,
      padding: const EdgeInsets.symmetric(horizontal: 11),
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: tint.withValues(alpha: 0.14),
        borderRadius: BorderRadius.circular(15),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 7,
            height: 7,
            decoration: BoxDecoration(color: tint, shape: BoxShape.circle),
          ),
          const SizedBox(width: 7),
          Text(state.mirror,
              style: TextStyle(
                  fontSize: 11.5, fontWeight: FontWeight.w700, color: tint)),
        ],
      ),
    );
  }
}

class _RoundBtn extends StatelessWidget {
  const _RoundBtn({required this.icon, required this.onTap, this.side = 40});

  final IconData icon;
  final VoidCallback onTap;
  final double side;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      width: side,
      height: side,
      child: Material(
        color: t.nChip,
        shape: const CircleBorder(),
        child: InkWell(
          customBorder: const CircleBorder(),
          onTap: onTap,
          child: Icon(icon, size: 18, color: t.nInk),
        ),
      ),
    );
  }
}

// ── search row ──────────────────────────────────────────────────────────────

/// No debounce: each search is a network round trip, so it fires on Enter and
/// on the button, never per keystroke.
class _SearchRow extends StatelessWidget {
  const _SearchRow({
    required this.controller,
    required this.state,
    required this.terms,
  });

  final GenesisController controller;
  final GenesisState state;
  final TextEditingController terms;

  Future<void> _go() async {
    await controller.send(GenesisCmd.setTerms(value: terms.text.trim()));
    await controller.send(const GenesisCmd.search());
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // The words live in the field until a search is asked for. Sending every
    // keystroke over the bridge would cost a snapshot per character, and the
    // Slint page does not do it either — it binds the box and reads it on
    // Enter.
    return ValueListenableBuilder<TextEditingValue>(
      valueListenable: terms,
      builder: (context, typed, _) => _row(context, t, typed.text),
    );
  }

  Widget _row(BuildContext context, Tokens t, String typed) {
    return Row(
      children: [
        Expanded(
          child: Container(
            height: 52,
            padding: const EdgeInsets.symmetric(horizontal: 18),
            decoration: BoxDecoration(
              color: kGen.withValues(alpha: 0.10),
              borderRadius: BorderRadius.circular(26),
              border:
                  Border.all(color: kGen.withValues(alpha: 0.4), width: 1.5),
            ),
            child: Row(
              children: [
                Icon(Icons.search, size: 19, color: t.nInk3),
                const SizedBox(width: 12),
                Expanded(
                  child: TextField(
                    controller: terms,
                    onSubmitted: (_) => _go(),
                    style: TextStyle(fontSize: 15, color: t.nInk),
                    decoration: InputDecoration(
                      border: InputBorder.none,
                      isCollapsed: true,
                      hintText: 'Title, author, series or ISBN',
                      hintStyle: TextStyle(fontSize: 15, color: t.nInk3),
                    ),
                  ),
                ),
                if (typed.isNotEmpty)
                  IconButton(
                    icon: Icon(Icons.close, size: 14, color: t.nInk3),
                    onPressed: () {
                      terms.clear();
                      controller.send(const GenesisCmd.setTerms(value: ''));
                    },
                  ),
              ],
            ),
          ),
        ),
        const SizedBox(width: 12),
        SizedBox(
          width: 156,
          height: 52,
          child: state.replay
              // Replaying: the button becomes its own progress bar, driven by
              // the phase rather than by a timer — so what it shows is the
              // search actually moving.
              ? ClipRRect(
                  borderRadius: BorderRadius.circular(26),
                  child: Stack(
                    fit: StackFit.expand,
                    children: [
                      ColoredBox(color: t.nTile),
                      FractionallySizedBox(
                        alignment: Alignment.centerLeft,
                        widthFactor: switch (state.phase) {
                          'probing' => 0.32,
                          'searching' => 0.72,
                          _ => 1.0,
                        },
                        child: const ColoredBox(color: kGen),
                      ),
                      const Center(
                        child: Text('Recent search…',
                            style: TextStyle(
                                fontSize: 12.5,
                                fontWeight: FontWeight.w700,
                                color: Colors.white)),
                      ),
                    ],
                  ),
                )
              : FilledButton(
                  onPressed: _go,
                  style: FilledButton.styleFrom(
                    backgroundColor: kGen,
                    shape: RoundedRectangleBorder(
                        borderRadius: BorderRadius.circular(26)),
                  ),
                  child: const Text('Search',
                      style:
                          TextStyle(fontSize: 14, fontWeight: FontWeight.w700)),
                ),
        ),
        // Start over. Only when there is something to throw away, and never
        // without asking — this is the one control here that destroys work.
        if (state.phase != 'idle' || typed.isNotEmpty) ...[
          const SizedBox(width: 12),
          _RoundBtn(
            icon: Icons.delete_outline,
            side: 52,
            onTap: () async {
              final yes = await confirm(
                context,
                title: 'Start over?',
                body: 'Throws away the results, the words you typed and the '
                    'mirror status. Nothing already downloaded is touched.',
                confirmLabel: 'Clear',
              );
              if (!yes) return;
              terms.clear();
              await controller.send(const GenesisCmd.clear());
            },
          ),
        ],
      ],
    );
  }
}

// ── filters ─────────────────────────────────────────────────────────────────

class _Filters extends StatelessWidget {
  const _Filters({required this.controller, required this.state});

  final GenesisController controller;
  final GenesisState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final hasRows = state.rows.isNotEmpty;
    return Wrap(
      spacing: 9,
      runSpacing: 9,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        for (final topic in state.topics)
          GenChip(
            label: topic.label,
            on: topic.on_,
            onTap: () =>
                controller.send(GenesisCmd.toggleTopic(code: topic.code)),
          ),
        Container(width: 1, height: 22, color: t.nHair),
        _Picker(
          label: 'Field',
          value: state.field,
          options: kGenesisFields,
          onPick: (v) => controller.send(GenesisCmd.setField(value: v)),
        ),
        _Picker(
          label: 'Format',
          value: state.format,
          options: kGenesisFormats,
          onPick: (v) => controller.send(GenesisCmd.setFormat(value: v)),
        ),
        _Picker(
          label: 'Language',
          value: state.language,
          options: kGenesisLanguages,
          onPick: (v) => controller.send(GenesisCmd.setLanguage(value: v)),
        ),
        if (hasRows) ...[
          Text('${state.total} results  ·  SORT',
              style: TextStyle(
                  fontSize: 10.5,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 0.6,
                  color: t.nInk3)),
          // Clicking the active key again flips the direction, which Rust owns.
          for (final key in const ['title', 'author', 'year', 'size'])
            GenChip(
              label: _sortLabel(key, state),
              on: state.sort == key,
              onTap: () => controller.send(GenesisCmd.sortBy(key: key)),
            ),
        ],
        // A page of results that failed to load. Only ever set once results are
        // up — a search clears it on the way in.
        if (state.phase == 'ready' && state.status.isNotEmpty)
          Text(state.status,
              style: const TextStyle(fontSize: 10.5, color: Tokens.error)),
        if (hasRows && !state.moreDone)
          _LoadMore(controller: controller, state: state),
      ],
    );
  }

  static String _sortLabel(String key, GenesisState st) {
    final name = '${key[0].toUpperCase()}${key.substring(1)}';
    if (st.sort != key) return name;
    return st.sortDesc ? '$name ↓' : '$name ↑';
  }
}

class GenChip extends StatelessWidget {
  const GenChip({super.key, required this.label, this.on = false, this.onTap});

  final String label;
  final bool on;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: on ? kGen.withValues(alpha: 0.14) : t.nChip,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(16),
        side: BorderSide(
          color: on ? kGen.withValues(alpha: 0.45) : Colors.transparent,
          width: 1.5,
        ),
      ),
      child: InkWell(
        borderRadius: BorderRadius.circular(16),
        onTap: onTap,
        child: Container(
          height: 32,
          padding: const EdgeInsets.symmetric(horizontal: 14),
          alignment: Alignment.center,
          child: Text(label,
              style: TextStyle(
                  fontSize: 12.5,
                  fontWeight: FontWeight.w600,
                  color: on ? kGen : t.nInk)),
        ),
      ),
    );
  }
}

class _Picker extends StatelessWidget {
  const _Picker({
    required this.label,
    required this.value,
    required this.options,
    required this.onPick,
  });

  final String label;
  final String value;
  final List<String> options;
  final ValueChanged<String> onPick;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return PopupMenuButton<String>(
      tooltip: label,
      onSelected: onPick,
      itemBuilder: (context) => [
        for (final o in options)
          PopupMenuItem(
            value: o,
            height: 36,
            child: Text(o, style: const TextStyle(fontSize: 13)),
          ),
      ],
      child: Container(
        height: 32,
        padding: const EdgeInsets.symmetric(horizontal: 12),
        decoration: BoxDecoration(
          color: t.nChip,
          borderRadius: BorderRadius.circular(16),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text('$label: ', style: TextStyle(fontSize: 11.5, color: t.nInk3)),
            Text(value,
                style: TextStyle(
                    fontSize: 12.5,
                    fontWeight: FontWeight.w700,
                    color: t.nInk)),
            const SizedBox(width: 4),
            Icon(Icons.expand_more, size: 15, color: t.nInk3),
          ],
        ),
      ),
    );
  }
}

/// Another `limit` results, from what the mirror already sent where possible
/// and from its next page when that runs out. Hides itself when there is
/// nothing left rather than going dead — a permanently disabled control invites
/// clicking.
class _LoadMore extends StatelessWidget {
  const _LoadMore({required this.controller, required this.state});

  final GenesisController controller;
  final GenesisState state;

  @override
  Widget build(BuildContext context) => Material(
        color: state.moreBusy
            ? Tokens.error.withValues(alpha: 0.55)
            : Tokens.error,
        borderRadius: BorderRadius.circular(16),
        child: InkWell(
          borderRadius: BorderRadius.circular(16),
          onTap: state.moreBusy
              ? null
              : () => controller.send(const GenesisCmd.loadMore()),
          child: Container(
            height: 32,
            padding: const EdgeInsets.symmetric(horizontal: 13),
            alignment: Alignment.center,
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(state.moreBusy ? Icons.refresh : Icons.keyboard_arrow_down,
                    size: 13, color: Colors.white),
                const SizedBox(width: 6),
                Text(state.moreBusy ? 'Loading…' : 'Load More',
                    style: const TextStyle(
                        fontSize: 12,
                        fontWeight: FontWeight.w700,
                        color: Colors.white)),
              ],
            ),
          ),
        ),
      );
}

// ── body ────────────────────────────────────────────────────────────────────

class _Body extends StatelessWidget {
  const _Body({required this.controller, required this.state});

  final GenesisController controller;
  final GenesisState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: t.nHair),
      ),
      clipBehavior: Clip.antiAlias,
      child: switch (state.phase) {
        'idle' => _Empty(
            icon: Icons.search,
            tint: state.canDownload ? kGen : Tokens.warn,
            title: 'Search for a book',
            body: state.canDownload
                ? 'Results come from Library Genesis mirrors. Downloads land '
                    'in ${state.dest}, which is watched by your library.'
                : 'The download folder could not be created, so there is '
                    'nowhere for a book to land.',
          ),
        // Said plainly because the wait is real and otherwise looks like a hang.
        'probing' => _Empty(
            icon: Icons.wifi_tethering,
            tint: Tokens.warn,
            title: 'Finding a working mirror…',
            body: state.status.isNotEmpty
                ? state.status
                : 'Mirrors often serve a healthy front page while search is '
                    'down, so each one is checked with a real query.',
          ),
        'searching' => const _Skeletons(),
        'failed' => _Empty(
            icon: Icons.warning_amber_rounded,
            tint: Tokens.error,
            title: 'Every mirror failed',
            body: state.error,
            action: FilledButton(
              onPressed: () => controller.send(const GenesisCmd.retry()),
              style: FilledButton.styleFrom(backgroundColor: kGen),
              child: const Text('Try again'),
            ),
          ),
        _ when state.rows.isEmpty => _Empty(
            icon: Icons.search_off,
            tint: t.nInk3,
            title: 'No results',
            // Filters run after the mirror truncates, so a narrow one is the
            // usual reason a real title comes back empty.
            body: 'Nothing matched on ${state.mirror}. Format and language are '
                'applied after the mirror answers, so a narrow filter is the '
                'usual cause.',
          ),
        _ => _Grid(controller: controller, state: state),
      },
    );
  }
}

class _Empty extends StatelessWidget {
  const _Empty({
    required this.icon,
    required this.tint,
    required this.title,
    required this.body,
    this.action,
  });

  final IconData icon;
  final Color tint;
  final String title;
  final String body;
  final Widget? action;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              width: 64,
              height: 64,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: tint.withValues(alpha: 0.14),
                shape: BoxShape.circle,
              ),
              child: Icon(icon, size: 28, color: tint),
            ),
            const SizedBox(height: 14),
            Text(title,
                style: TextStyle(
                    fontSize: 16, fontWeight: FontWeight.w800, color: t.nInk)),
            const SizedBox(height: 6),
            ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 460),
              child: Text(body,
                  textAlign: TextAlign.center,
                  style: TextStyle(fontSize: 12.5, color: t.nInk2)),
            ),
            if (action != null) ...[const SizedBox(height: 14), action!],
          ],
        ),
      ),
    );
  }
}

class _Skeletons extends StatelessWidget {
  const _Skeletons();

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget bar(double h, [double w = 1]) => FractionallySizedBox(
          alignment: Alignment.centerLeft,
          widthFactor: w,
          child: Container(
            height: h,
            decoration: BoxDecoration(
              color: t.nTile,
              borderRadius: BorderRadius.circular(6),
            ),
          ),
        );
    return Padding(
      padding: const EdgeInsets.all(16),
      child: Column(
        children: [
          for (var i = 0; i < 2; i++) ...[
            if (i > 0) const SizedBox(height: 14),
            Row(
              children: [
                for (var j = 0; j < 4; j++) ...[
                  if (j > 0) const SizedBox(width: 14),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        bar(150),
                        const SizedBox(height: 8),
                        bar(12),
                        const SizedBox(height: 8),
                        bar(10, 0.7),
                      ],
                    ),
                  ),
                ],
              ],
            ),
          ],
        ],
      ),
    );
  }
}

class _Grid extends StatelessWidget {
  const _Grid({required this.controller, required this.state});

  final GenesisController controller;
  final GenesisState state;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
        builder: (context, box) {
          // Cards want ~200px; never fewer than two columns, or a narrow window
          // gives one enormous card per row.
          final cols = (box.maxWidth / 232).floor().clamp(2, 8);
          return GridView.builder(
            padding: const EdgeInsets.all(14),
            gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
              crossAxisCount: cols,
              crossAxisSpacing: 14,
              mainAxisSpacing: 14,
              // The cover is 1.18 × the cell, and the text block under it is a
              // fixed 143.
              mainAxisExtent:
                  ((box.maxWidth - 28 - (cols - 1) * 14) / cols) * 1.18 + 143,
            ),
            itemCount: state.rows.length,
            itemBuilder: (context, i) => BookCard(
              row: state.rows[i],
              canDownload: state.canDownload,
              busy: state.busy,
              onDownload: () =>
                  controller.send(GenesisCmd.download(md5: state.rows[i].md5)),
              onOpen: () => openRecord(context, controller, state.rows[i].md5),
            ),
          );
        },
      );
}

/// One result card. `have` marks a book already in the download history, keyed
/// on MD5 — the catalogue's own identity for a file. The cover plate is drawn
/// until the cover pass finds real art, and stays drawn for a book with none.
class BookCard extends StatelessWidget {
  const BookCard({
    super.key,
    required this.row,
    required this.canDownload,
    required this.busy,
    required this.onDownload,
    required this.onOpen,
  });

  final GenRow row;
  final bool canDownload;
  final bool busy;
  final VoidCallback onDownload;
  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final live = canDownload && !busy;
    return Material(
      color: t.nCard,
      borderRadius: BorderRadius.circular(14),
      child: InkWell(
        // The card itself opens the record. The Get button sits on top of this
        // and takes its own tap, so downloading never opens the popup.
        onTap: onOpen,
        borderRadius: BorderRadius.circular(14),
        child: Container(
          padding: const EdgeInsets.all(10),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(14),
            border: Border.all(color: t.nHair),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(child: _Plate(row: row)),
              const SizedBox(height: 8),
              SizedBox(
                height: 32,
                child: Text(row.title,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 12.5,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
              ),
              const SizedBox(height: 8),
              SizedBox(
                height: 15,
                child: Text(row.author.isEmpty ? 'Unknown author' : row.author,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11.5, color: t.nInk2)),
              ),
              const SizedBox(height: 8),
              SizedBox(
                height: 14,
                child: Text(
                    '${row.year.isEmpty ? "—" : row.year}'
                    '${row.pages.isEmpty ? "" : "  ·  ${row.pages} pp"}'
                    '${row.language.isEmpty ? "" : "  ·  ${row.language}"}',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 10.5, color: t.nInk3)),
              ),
              const SizedBox(height: 8),
              // The size used to sit outside the button as grey text, which put
              // two separate things on a card that only ever does one. Filled,
              // they read as a single control: what you are about to fetch, and
              // how big it is.
              SizedBox(
                height: 30,
                child: row.have
                    ? _Have(row: row)
                    : Opacity(
                        opacity: live ? 1 : 0.45,
                        child: Material(
                          color: kGen,
                          borderRadius: BorderRadius.circular(15),
                          child: InkWell(
                            borderRadius: BorderRadius.circular(15),
                            onTap: live ? onDownload : null,
                            child: Padding(
                              padding:
                                  const EdgeInsets.symmetric(horizontal: 12),
                              child: Row(
                                children: [
                                  Expanded(
                                    child: Text(row.size,
                                        overflow: TextOverflow.ellipsis,
                                        style: const TextStyle(
                                            fontSize: 10.5,
                                            fontWeight: FontWeight.w700,
                                            color: Colors.white70)),
                                  ),
                                  const Icon(Icons.download,
                                      size: 13, color: Colors.white),
                                  const SizedBox(width: 7),
                                  const Text('Download',
                                      style: TextStyle(
                                          fontSize: 11.5,
                                          fontWeight: FontWeight.w700,
                                          color: Colors.white)),
                                ],
                              ),
                            ),
                          ),
                        ),
                      ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _Have extends StatelessWidget {
  const _Have({required this.row});

  final GenRow row;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12),
      decoration: BoxDecoration(
        color: Tokens.ok.withValues(alpha: 0.18),
        borderRadius: BorderRadius.circular(15),
      ),
      child: Row(
        children: [
          Expanded(
            child: Text(row.size,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 10.5,
                    fontWeight: FontWeight.w700,
                    color: t.nInk2)),
          ),
          const Icon(Icons.check, size: 13, color: Tokens.ok),
          const SizedBox(width: 7),
          const Text('In library',
              style: TextStyle(
                  fontSize: 11, fontWeight: FontWeight.w700, color: Tokens.ok)),
        ],
      ),
    );
  }
}

/// Real art when the cover pass found some; otherwise a drawn plate tinted by
/// format, which is also what a book with no cover keeps for good.
class _Plate extends StatelessWidget {
  const _Plate({required this.row});

  final GenRow row;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final plate = switch (row.format) {
      'PDF' => const Color(0xFFEC4899),
      'EPUB' => const Color(0xFF10B981),
      'MOBI' => const Color(0xFFF97316),
      'CBZ' => const Color(0xFF06B6D4),
      _ => kGen,
    };
    return ClipRRect(
      borderRadius: BorderRadius.circular(10),
      child: Stack(
        fit: StackFit.expand,
        children: [
          ColoredBox(color: plate.withValues(alpha: 0.14)),
          if (row.cover.isNotEmpty)
            Image.file(File(row.cover),
                fit: BoxFit.cover,
                errorBuilder: (_, __, ___) => const SizedBox.shrink())
          else ...[
            // A spine, so the plate reads as a book rather than a swatch.
            Align(
              alignment: Alignment.centerLeft,
              child: Container(width: 7, color: plate.withValues(alpha: 0.28)),
            ),
            Center(
              child: Opacity(
                opacity: 0.22,
                child: Padding(
                  padding: const EdgeInsets.fromLTRB(20, 0, 6, 0),
                  child: Text(row.title.isEmpty ? '?' : row.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 30,
                          fontWeight: FontWeight.w800,
                          color: t.nInk)),
                ),
              ),
            ),
          ],
          Positioned(
            right: 8,
            bottom: 8,
            child: Container(
              height: 22,
              padding: const EdgeInsets.symmetric(horizontal: 8),
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: t.nCard,
                borderRadius: BorderRadius.circular(6),
              ),
              child: Text(row.format,
                  style: TextStyle(
                      fontSize: 10.5,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
            ),
          ),
          // Already in the library: a check, not a button.
          if (row.have)
            Positioned(
              left: 8,
              top: 8,
              child: Container(
                width: 24,
                height: 24,
                alignment: Alignment.center,
                decoration: const BoxDecoration(
                    color: Tokens.ok, shape: BoxShape.circle),
                child: const Icon(Icons.check, size: 13, color: Colors.white),
              ),
            ),
        ],
      ),
    );
  }
}

// ── the two strips ──────────────────────────────────────────────────────────

class _DownloadStrip extends StatelessWidget {
  const _DownloadStrip({required this.controller, required this.state});

  final GenesisController controller;
  final GenesisState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // The live tick where there is one: it arrives ten times a second and the
    // snapshot does not.
    final pct = controller.tick?.pct ?? state.busyPct;
    final detail = controller.tick?.detail ?? state.busyDetail;
    return Container(
      height: 66,
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: t.nHair),
      ),
      child: Row(
        children: [
          Expanded(
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: Text(state.busyName,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 13,
                              fontWeight: FontWeight.w600,
                              color: t.nInk)),
                    ),
                    Text(detail,
                        style: TextStyle(fontSize: 12, color: t.nInk2)),
                  ],
                ),
                const SizedBox(height: 7),
                ClipRRect(
                  borderRadius: BorderRadius.circular(3),
                  child: LinearProgressIndicator(
                    value: pct <= 0 ? null : pct,
                    minHeight: 6,
                    backgroundColor: t.nTile,
                    color: kGen,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 16),
          OutlinedButton(
            onPressed: () => controller.send(const GenesisCmd.cancel()),
            child: const Text('Cancel', style: TextStyle(fontSize: 12.5)),
          ),
        ],
      ),
    );
  }
}

class _DoneStrip extends StatelessWidget {
  const _DoneStrip({required this.controller, required this.state});

  final GenesisController controller;
  final GenesisState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 62,
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: Tokens.ok.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(14),
      ),
      child: Row(
        children: [
          Container(
            width: 32,
            height: 32,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: Tokens.ok.withValues(alpha: 0.2),
              shape: BoxShape.circle,
            ),
            child: const Icon(Icons.check, size: 16, color: Tokens.ok),
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text('Added to library — ${state.doneName}',
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w600,
                        color: t.nInk)),
                Text(state.doneDetail,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11.5, color: t.nInk2)),
              ],
            ),
          ),
          const SizedBox(width: 14),
          OutlinedButton(
            onPressed: () => controller.send(const GenesisCmd.revealDest()),
            child: const Text('Show file', style: TextStyle(fontSize: 12)),
          ),
        ],
      ),
    );
  }
}

class _ErrorBar extends StatelessWidget {
  const _ErrorBar({required this.controller});

  final GenesisController controller;

  @override
  Widget build(BuildContext context) => Material(
        color: Tokens.error.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(12),
        child: ListTile(
          dense: true,
          leading: const Icon(Icons.error_outline, color: Tokens.error),
          title: Text('${controller.error}',
              style: const TextStyle(fontSize: 12.5, color: Tokens.error)),
          trailing: IconButton(
            icon: const Icon(Icons.close, size: 18),
            onPressed: controller.clearError,
          ),
        ),
      );
}

/// Copy a mirror-supplied address rather than hand it to the OS. The Slint page
/// routes it through the window's own opener, which checks the scheme first;
/// there is no such gate here, and a URL off a mirror is not something to open
/// unchecked.
Future<void> copyLink(BuildContext context, String url) async {
  await Clipboard.setData(ClipboardData(text: url));
  if (!context.mounted) return;
  ScaffoldMessenger.maybeOf(context)?.showSnackBar(
    const SnackBar(content: Text('Link copied')),
  );
}
