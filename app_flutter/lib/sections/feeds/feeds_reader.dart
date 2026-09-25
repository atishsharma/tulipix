// Feeds → Unread: sources, the list, and the reader.
//
// Three panes, as the deck draws them. Below 1180px the sources pane folds
// into a menu on the list's header, because the reader is what the width is
// for. The reader is keyed by article, so its scroll and its selection belong
// to one article and a new snapshot does not reset them.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/feeds.dart';
import 'feeds_controller.dart';
import 'feeds_page.dart';

class UnreadView extends StatelessWidget {
  const UnreadView({
    super.key,
    required this.c,
    required this.st,
    required this.voice,
  });

  final FeedsController c;
  final FeedsState st;
  final FeedsVoice voice;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (!st.hasFeeds) return FollowFirst(c: c, st: st);
    final open = st.open;
    return LayoutBuilder(
      builder: (context, box) {
        final wide = box.maxWidth >= 1180;
        return Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (wide) ...[
              SizedBox(width: 230, child: _Sources(c: c, st: st)),
              VerticalDivider(width: 1, color: t.nHair),
            ],
            SizedBox(
              width: wide ? 360 : 300,
              child: _List(c: c, st: st, pickSource: !wide),
            ),
            VerticalDivider(width: 1, color: t.nHair),
            Expanded(
              child: open == null
                  ? _NoArticle(c: c, st: st)
                  : Reader(
                      key: ValueKey(open.id),
                      c: c,
                      a: open,
                      voice: voice,
                    ),
            ),
          ],
        );
      },
    );
  }
}

// ----------------------------------------------------------------- sources --

class _Sources extends StatelessWidget {
  const _Sources({required this.c, required this.st});

  final FeedsController c;
  final FeedsState st;

  @override
  Widget build(BuildContext context) {
    return ListView(
      padding: const EdgeInsets.fromLTRB(10, 14, 10, 24),
      children: [
        for (final s in st.sources)
          _SourceTile(c: c, s: s, on: s.key == st.source),
        const SizedBox(height: 8),
        Align(
          alignment: Alignment.centerLeft,
          child: TextButton.icon(
            style: TextButton.styleFrom(foregroundColor: kFeeds),
            onPressed: () => showFollow(context, c, st),
            icon: const Icon(Icons.add, size: 15),
            label: const Text('Follow a site or newsletter',
                style: TextStyle(fontSize: 13)),
          ),
        ),
      ],
    );
  }
}

class _SourceTile extends StatelessWidget {
  const _SourceTile({required this.c, required this.s, required this.on});

  final FeedsController c;
  final SourceRow s;
  final bool on;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final ink = on ? kFeeds : t.nInk2;
    final Widget lead = s.key.isEmpty
        ? Icon(Icons.rss_feed, size: 13, color: ink)
        : s.feedId == 0
            ? Icon(Icons.folder_outlined, size: 14, color: ink)
            : SourceMark(name: s.label, size: 18);
    return GestureDetector(
      onSecondaryTapDown: s.feedId == 0
          ? null
          : (d) => _menuAt(context, d.globalPosition, c, s),
      child: Material(
        color: on ? kFeeds.withValues(alpha: 0.17) : Colors.transparent,
        borderRadius: BorderRadius.circular(9),
        child: InkWell(
          borderRadius: BorderRadius.circular(9),
          onTap: () => c.send(FeedsCmd.setSource(key: s.key)),
          onLongPress:
              s.feedId == 0 ? null : () => _menuAt(context, null, c, s),
          child: Container(
            height: 32,
            padding: EdgeInsets.only(left: s.depth > 0 ? 24 : 9, right: 9),
            child: Row(
              children: [
                lead,
                const SizedBox(width: 9),
                Expanded(
                  child: Text(s.label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 13,
                          fontWeight:
                              s.depth > 0 ? FontWeight.w500 : FontWeight.w600,
                          color: ink)),
                ),
                if (s.error.isNotEmpty)
                  Tooltip(
                    message: s.error,
                    child: const Padding(
                      padding: EdgeInsets.only(right: 6),
                      child: Icon(Icons.warning_amber_rounded,
                          size: 14, color: Tokens.warn),
                    ),
                  ),
                if (s.unread > 0)
                  Text('${s.unread}',
                      style: TextStyle(fontSize: 11.5, color: t.nInk3)),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

const List<PopupMenuEntry<String>> _feedItems = [
  PopupMenuItem(value: 'move', child: Text('Move to folder…')),
  PopupMenuItem(value: 'unfollow', child: Text('Unfollow')),
];

Future<void> _menuAt(
  BuildContext context,
  Offset? at,
  FeedsController c,
  SourceRow s,
) async {
  final overlay = Overlay.of(context).context.findRenderObject() as RenderBox;
  final box = context.findRenderObject() as RenderBox;
  final point = at ?? box.localToGlobal(box.size.centerRight(Offset.zero));
  final v = await showMenu<String>(
    context: context,
    position: RelativeRect.fromRect(
        point & const Size(1, 1), Offset.zero & overlay.size),
    items: _feedItems,
  );
  if (v != null && context.mounted) await feedAction(context, c, s, v);
}

/// Move or unfollow one feed.
Future<void> feedAction(
  BuildContext context,
  FeedsController c,
  SourceRow s,
  String action,
) async {
  switch (action) {
    case 'move':
      final folder = await _askFolder(context, s.folder, c.state?.folders);
      if (folder != null) {
        c.send(FeedsCmd.moveFeed(feedId: s.feedId, folder: folder));
      }
    case 'unfollow':
      final ok = await showDialog<bool>(
        context: context,
        builder: (ctx) => AlertDialog(
          title: Text('Unfollow ${s.label}?'),
          content: const Text(
              'Its articles go with it, and so do any highlights kept from '
              'them. Read later copies sent to Books stay in Books.'),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(ctx).pop(false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              style: FilledButton.styleFrom(backgroundColor: Tokens.error),
              onPressed: () => Navigator.of(ctx).pop(true),
              child: const Text('Unfollow'),
            ),
          ],
        ),
      );
      if (ok ?? false) c.send(FeedsCmd.unfollow(feedId: s.feedId));
  }
}

Future<String?> _askFolder(
  BuildContext context,
  String current,
  List<String>? folders,
) {
  final ctl = TextEditingController(text: current);
  return showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Move to folder'),
      content: SizedBox(
        width: 400,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            TextField(
              controller: ctl,
              autofocus: true,
              onSubmitted: (v) => Navigator.of(ctx).pop(v),
              decoration: const InputDecoration(
                labelText: 'Folder',
                helperText: 'Empty takes it out of every folder',
              ),
            ),
            const SizedBox(height: 10),
            Wrap(
              spacing: 6,
              runSpacing: 6,
              children: [
                for (final f in {...?folders, 'Newsletters'})
                  ActionChip(
                    label: Text(f, style: const TextStyle(fontSize: 12)),
                    onPressed: () => Navigator.of(ctx).pop(f),
                  ),
              ],
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(ctx).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          style: FilledButton.styleFrom(backgroundColor: kFeeds),
          onPressed: () => Navigator.of(ctx).pop(ctl.text),
          child: const Text('Move'),
        ),
      ],
    ),
  );
}

// -------------------------------------------------------------------- list --

class _List extends StatelessWidget {
  const _List({required this.c, required this.st, required this.pickSource});

  final FeedsController c;
  final FeedsState st;

  /// The sources pane is folded away: pick one from the header instead.
  final bool pickSource;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final current = st.sources.where((s) => s.key == st.source).firstOrNull;
    final unread = current?.unread ?? st.unread;
    final searching = st.query.isNotEmpty;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Container(
          padding: const EdgeInsets.fromLTRB(16, 10, 6, 10),
          decoration: BoxDecoration(
            border: Border(bottom: BorderSide(color: t.nHair)),
          ),
          child: Row(
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(st.listTitle,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 14,
                            fontWeight: FontWeight.w700,
                            color: t.nInk)),
                    Text(
                        searching
                            ? plural(st.list.length, 'match', 'matches')
                            : '$unread unread',
                        style: TextStyle(fontSize: 12, color: t.nInk3)),
                  ],
                ),
              ),
              if (!searching)
                SegmentedButton<bool>(
                  showSelectedIcon: false,
                  style: const ButtonStyle(
                    visualDensity: VisualDensity.compact,
                    tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                  ),
                  segments: const [
                    ButtonSegment(value: true, label: Text('Unread')),
                    ButtonSegment(value: false, label: Text('All')),
                  ],
                  selected: {st.unreadOnly},
                  onSelectionChanged: (v) =>
                      c.send(FeedsCmd.setUnreadOnly(on_: v.first)),
                ),
              PopupMenuButton<String>(
                tooltip: 'More',
                iconSize: 18,
                onSelected: (v) {
                  if (v == 'read') {
                    c.send(const FeedsCmd.markAllRead());
                  } else if (v.startsWith('src:')) {
                    c.send(FeedsCmd.setSource(key: v.substring(4)));
                  } else if (current != null) {
                    feedAction(context, c, current, v);
                  }
                },
                itemBuilder: (_) => [
                  PopupMenuItem(
                    value: 'read',
                    enabled: unread > 0,
                    child: const Text('Mark all as read'),
                  ),
                  if (current != null && current.feedId != 0) ..._feedItems,
                  if (pickSource) ...[
                    const PopupMenuDivider(),
                    for (final s in st.sources)
                      PopupMenuItem(
                        value: 'src:${s.key}',
                        child: Padding(
                          padding: EdgeInsets.only(left: s.depth * 14.0),
                          child: Text(
                              s.unread > 0
                                  ? '${s.label}  ·  ${s.unread}'
                                  : s.label,
                              style: TextStyle(
                                  fontWeight: s.key == st.source
                                      ? FontWeight.w700
                                      : FontWeight.w400)),
                        ),
                      ),
                  ],
                ],
              ),
            ],
          ),
        ),
        Expanded(
          child: st.list.isEmpty
              ? Center(
                  child: Padding(
                    padding: const EdgeInsets.all(24),
                    child: Text(
                      searching
                          ? 'No article mentions that.'
                          : st.unreadOnly
                              ? 'Nothing unread here.'
                              : 'No articles here yet.',
                      textAlign: TextAlign.center,
                      style: TextStyle(fontSize: 13, color: t.nInk3),
                    ),
                  ),
                )
              : ListView.separated(
                  itemCount: st.list.length,
                  separatorBuilder: (_, __) =>
                      Divider(height: 1, color: t.nHair),
                  itemBuilder: (_, i) => _ArticleTile(
                    c: c,
                    a: st.list[i],
                    on: st.list[i].id == st.open?.id,
                  ),
                ),
        ),
      ],
    );
  }
}

class _ArticleTile extends StatelessWidget {
  const _ArticleTile({required this.c, required this.a, required this.on});

  final FeedsController c;
  final ArticleRow a;
  final bool on;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final meta = [
      '${a.minutes} min read',
      if (a.summary) 'summary ready',
      if (a.saved) 'saved',
    ].join(' · ');
    return Material(
      color: on ? kFeeds.withValues(alpha: 0.17) : Colors.transparent,
      child: InkWell(
        onTap: () => c.open(a.id),
        child: Stack(
          children: [
            if (on)
              Positioned(
                left: 0,
                top: 10,
                bottom: 10,
                child: Container(
                  width: 3,
                  decoration: BoxDecoration(
                    color: kFeeds,
                    borderRadius: BorderRadius.circular(3),
                  ),
                ),
              ),
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 14, 16, 14),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          children: [
                            SourceMark(name: a.source, size: 18),
                            const SizedBox(width: 6),
                            Expanded(
                              child: Text(
                                  '${a.source} · ${ago(a.published)}',
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: TextStyle(
                                      fontSize: 12, color: t.nInk3)),
                            ),
                          ],
                        ),
                        const SizedBox(height: 4),
                        Text(a.title,
                            maxLines: 3,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontFamily: kSerif,
                                fontSize: 16,
                                height: 1.3,
                                fontWeight:
                                    a.read ? FontWeight.w400 : FontWeight.w600,
                                color: a.read ? t.nInk3 : t.nInk)),
                        const SizedBox(height: 4),
                        Text(meta,
                            style: TextStyle(fontSize: 11, color: t.nInk3)),
                      ],
                    ),
                  ),
                  if (a.image.isNotEmpty) ...[
                    const SizedBox(width: 12),
                    ClipRRect(
                      borderRadius: BorderRadius.circular(10),
                      child: SizedBox(
                        width: 64,
                        height: 64,
                        child: Art(url: a.image, seed: a.id, width: 128),
                      ),
                    ),
                  ],
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _NoArticle extends StatelessWidget {
  const _NoArticle({required this.c, required this.st});

  final FeedsController c;
  final FeedsState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final empty = st.list.isEmpty;
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(empty ? Icons.done_all : Icons.article_outlined,
              size: 34, color: t.nInk3),
          const SizedBox(height: 10),
          Text(empty ? 'All read' : 'Pick an article',
              style: TextStyle(
                  fontSize: 15, fontWeight: FontWeight.w700, color: t.nInk)),
          if (empty && st.unreadOnly && st.query.isEmpty) ...[
            const SizedBox(height: 10),
            OutlinedButton(
              onPressed: () => c.send(const FeedsCmd.setUnreadOnly(on_: false)),
              child: const Text('Show read articles'),
            ),
          ],
        ],
      ),
    );
  }
}

// ------------------------------------------------------------------ reader --

class Reader extends StatefulWidget {
  const Reader({
    super.key,
    required this.c,
    required this.a,
    required this.voice,
  });

  final FeedsController c;
  final ArticleView a;
  final FeedsVoice voice;

  @override
  State<Reader> createState() => _ReaderState();
}

class _ReaderState extends State<Reader> {
  final ScrollController _scroll = ScrollController();

  /// What is selected right now, for Highlight.
  String _selected = '';

  /// The furthest point already stored, so a scroll back up sends nothing.
  late double _sent = widget.a.progress;

  @override
  void dispose() {
    _scroll.dispose();
    super.dispose();
  }

  FeedsController get c => widget.c;
  ArticleView get a => widget.a;

  bool _onScrollEnd(ScrollEndNotification n) {
    // Progress is Read later's; nothing else shows it.
    if (!a.saved) return false;
    final m = n.metrics;
    final p = m.maxScrollExtent <= 0
        ? 1.0
        : (m.pixels / m.maxScrollExtent).clamp(0.0, 1.0);
    if (p > _sent + 0.05 || (p >= 0.98 && _sent < 0.98)) {
      _sent = p;
      c.send(FeedsCmd.setProgress(id: a.id, progress: p));
    }
    return false;
  }

  Future<void> _highlight() async {
    final quote = _selected.trim();
    if (quote.isEmpty) {
      c.say('Select a passage in the article first, then press Highlight.');
      return;
    }
    final note = await askNote(context);
    if (note == null) return;
    c.send(FeedsCmd.addHighlight(id: a.id, quote: quote, note: note));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final size = FeedsController.textSizes[c.textSize];
    final by = a.author.isEmpty ? '' : ' · ${a.author}';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Container(
          padding: const EdgeInsets.fromLTRB(20, 10, 20, 10),
          decoration: BoxDecoration(
            border: Border(bottom: BorderSide(color: t.nHair)),
          ),
          child: Wrap(
            spacing: 8,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              IconButton.outlined(
                tooltip: 'Text size',
                iconSize: 16,
                onPressed: c.cycleTextSize,
                icon: const Icon(Icons.format_size),
              ),
              OutlinedButton.icon(
                onPressed: () => widget.voice
                    .read(a.title, [a.title, ...a.paragraphs].join('\n\n')),
                icon: const Icon(Icons.headphones_outlined, size: 15),
                label: const Text('Listen'),
              ),
              OutlinedButton.icon(
                onPressed: _highlight,
                icon: const Icon(Icons.border_color_outlined, size: 15),
                label: const Text('Highlight'),
              ),
              OutlinedButton.icon(
                onPressed: () => c.send(FeedsCmd.toggleSaved(id: a.id)),
                icon: Icon(a.saved ? Icons.bookmark : Icons.bookmark_border,
                    size: 15, color: a.saved ? kFeeds : null),
                label: Text(a.saved ? 'Saved' : 'Save'),
              ),
              IconButton(
                tooltip: 'Mark as unread',
                iconSize: 17,
                onPressed: () =>
                    c.send(FeedsCmd.setRead(id: a.id, read: false)),
                icon: const Icon(Icons.mark_email_unread_outlined),
              ),
              if (a.url.isNotEmpty)
                OutlinedButton.icon(
                  onPressed: () => c.send(FeedsCmd.openOriginal(id: a.id)),
                  icon: const Icon(Icons.public, size: 15),
                  label: const Text('Original'),
                ),
            ],
          ),
        ),
        Expanded(
          child: NotificationListener<ScrollEndNotification>(
            onNotification: _onScrollEnd,
            child: SingleChildScrollView(
              controller: _scroll,
              padding: const EdgeInsets.fromLTRB(30, 26, 30, 96),
              child: Center(
                child: ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 680),
                  child: SelectionArea(
                    onSelectionChanged: (s) => _selected = s?.plainText ?? '',
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          children: [
                            SourceMark(name: a.source, size: 18),
                            const SizedBox(width: 7),
                            Expanded(
                              child: Text(
                                  '${a.source} · ${ago(a.published)} · '
                                  '${a.minutes} min read$by',
                                  style: TextStyle(
                                      fontSize: 12, color: t.nInk3)),
                            ),
                          ],
                        ),
                        const SizedBox(height: 12),
                        Text(a.title,
                            style: TextStyle(
                                fontFamily: kSerif,
                                fontSize: 34,
                                height: 1.18,
                                fontWeight: FontWeight.w600,
                                color: t.nInk)),
                        const SizedBox(height: 18),
                        if (a.summary.isNotEmpty || c.summarising == a.id)
                          _Summary(c: c, a: a),
                        for (var i = 0; i < a.paragraphs.length; i++)
                          Padding(
                            padding: const EdgeInsets.only(bottom: 16),
                            child: Text.rich(
                              TextSpan(
                                  children: _marked(
                                      a.paragraphs[i], a.highlights)),
                              style: TextStyle(
                                fontFamily: kSerif,
                                fontSize: i == 0 ? size + 2 : size,
                                height: 1.7,
                                color: i == 0 ? t.nInk2 : t.nInk,
                              ),
                            ),
                          ),
                        if (c.fullFor == a.id)
                          Text('Fetching the whole article…',
                              style: TextStyle(fontSize: 12, color: t.nInk3))
                        else if (a.paragraphs.isEmpty)
                          Text(
                            a.url.isEmpty
                                ? 'This feed sends headlines only.'
                                : 'This feed sends headlines only, and the '
                                    'page did not give up its text. Original '
                                    'opens it in the browser.',
                            style: TextStyle(fontSize: 13, color: t.nInk3),
                          ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

class _Summary extends StatelessWidget {
  const _Summary({required this.c, required this.a});

  final FeedsController c;
  final ArticleView a;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final model = c.state?.summarizer ?? '';
    return Container(
      margin: const EdgeInsets.only(bottom: 22),
      padding: const EdgeInsets.fromLTRB(16, 8, 12, 12),
      decoration: BoxDecoration(
        color: kFeeds.withValues(alpha: 0.09),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: kFeeds.withValues(alpha: 0.25)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const Icon(Icons.auto_awesome, size: 13, color: kFeeds),
              const SizedBox(width: 6),
              const Text('Summary',
                  style: TextStyle(
                      fontSize: 12,
                      fontWeight: FontWeight.w700,
                      color: kFeeds)),
              const SizedBox(width: 8),
              Flexible(
                child: Text(
                    c.summarising == a.id
                        ? 'writing it with $model…'
                        : a.summaryModel
                            ? 'written by $model on your server'
                            : 'picked from the article on this computer',
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11, color: t.nInk3)),
              ),
              const Spacer(),
              IconButton(
                tooltip: 'Where summaries come from',
                iconSize: 15,
                visualDensity: VisualDensity.compact,
                onPressed: () => showSummarizer(context, c),
                icon: Icon(Icons.tune, color: t.nInk3),
              ),
              Switch(
                value: c.summaryOpen,
                activeTrackColor: kFeeds,
                onChanged: (_) => c.toggleSummary(),
              ),
            ],
          ),
          if (c.summaryOpen && c.summarising == a.id)
            const Padding(
              padding: EdgeInsets.only(top: 8),
              child: LinearProgressIndicator(minHeight: 2, color: kFeeds),
            ),
          if (c.summaryOpen &&
              c.summaryError.isNotEmpty &&
              c.summarising == 0 &&
              !a.summaryModel)
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: Text('The model server: ${c.summaryError}',
                  style: TextStyle(fontSize: 11, color: t.nInk3)),
            ),
          if (c.summaryOpen)
            for (final line in a.summary)
              Padding(
                padding: const EdgeInsets.only(top: 5),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Padding(
                      padding: const EdgeInsets.only(top: 8, right: 9),
                      child: Container(
                        width: 4,
                        height: 4,
                        decoration: BoxDecoration(
                            color: t.nInk3, shape: BoxShape.circle),
                      ),
                    ),
                    Expanded(
                      child: Text(line,
                          style: TextStyle(
                              fontSize: 14, height: 1.5, color: t.nInk2)),
                    ),
                  ],
                ),
              ),
        ],
      ),
    );
  }
}

/// A paragraph with the kept quotes marked in it.
///
/// A selection that ran across paragraphs arrives joined by newlines, so each
/// line of a quote is looked for on its own; each is marked where it first
/// appears in this paragraph.
List<TextSpan> _marked(String p, List<String> quotes) {
  final ranges = <(int, int)>[];
  for (final q in quotes) {
    for (final piece in q.split('\n')) {
      final s = piece.trim();
      if (s.length < 3) continue;
      final i = p.indexOf(s);
      if (i >= 0) ranges.add((i, i + s.length));
    }
  }
  if (ranges.isEmpty) return [TextSpan(text: p)];
  ranges.sort((x, y) => x.$1.compareTo(y.$1));
  final mark = TextStyle(backgroundColor: kMark.withValues(alpha: 0.38));
  final out = <TextSpan>[];
  var at = 0;
  for (final (s, e) in ranges) {
    if (e <= at) continue;
    final from = s < at ? at : s;
    if (from > at) out.add(TextSpan(text: p.substring(at, from)));
    out.add(TextSpan(text: p.substring(from, e), style: mark));
    at = e;
  }
  if (at < p.length) out.add(TextSpan(text: p.substring(at)));
  return out;
}
