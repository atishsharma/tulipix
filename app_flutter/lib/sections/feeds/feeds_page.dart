// The Feeds section — docs/NewSections/feeds-deck.html.
//
// Four tabs over one snapshot: Today (the brief: one story told by several
// sources, then the rest, then newsletters), Unread (sources · list · reader,
// in feeds_reader.dart), Read later and Highlights. Every surface is drawn
// from the tokens and the skin, so the four design languages come for free;
// the one liberty is the serif, which is the Books reader's.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/section_tabs.dart';
import '../../src/rust/api/dialog.dart';
import '../../src/rust/api/feeds.dart';
import 'feeds_controller.dart';
import 'feeds_reader.dart';

const Color kFeeds = Tokens.secFeeds;
const Color kFeeds2 = Tokens.secFeeds2;

/// The reading face: the Books reader's.
const String kSerif = 'serif';

class FeedsPage extends StatefulWidget {
  const FeedsPage({super.key, required this.visible});

  /// On screen. Coming back to the page after a while asks the feeds again.
  final bool visible;

  @override
  State<FeedsPage> createState() => _FeedsPageState();
}

class _FeedsPageState extends State<FeedsPage> {
  final FeedsController _c = FeedsController();
  final FeedsVoice _voice = FeedsVoice();
  Timer? _tick;

  @override
  void initState() {
    super.initState();
    // The database first, so the page draws at once; then the network.
    _c.refresh().then((_) => _c.fetch());
    _tick = Timer.periodic(kFeedsTick, (_) => _c.fetch());
  }

  @override
  void didUpdateWidget(FeedsPage old) {
    super.didUpdateWidget(old);
    if (widget.visible && !old.visible) {
      _c.fetchIfStale(const Duration(minutes: 10));
    }
  }

  @override
  void dispose() {
    _tick?.cancel();
    _voice.dispose();
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: Listenable.merge([_c, _voice]),
      builder: (context, _) {
        final st = _c.state;
        return ColoredBox(
          color: t.nCanvas,
          child: Column(
            children: [
              _Header(c: _c, st: st),
              if (_c.fetching || _c.fullFor != 0)
                const LinearProgressIndicator(minHeight: 2, color: kFeeds)
              else
                const SizedBox(height: 2),
              if (_c.notice.isNotEmpty)
                _Strip(
                  icon: Icons.check_circle_outline,
                  tint: kFeeds,
                  text: _c.notice,
                  onClose: _c.dismissNotice,
                ),
              if (_c.error != null && st != null)
                _Strip(
                  icon: Icons.error_outline,
                  tint: Tokens.error,
                  text: plainError(_c.error!),
                  onClose: _c.clearError,
                ),
              Expanded(
                child: st == null
                    ? FirstLoad(error: _c.error, onRetry: _c.refresh)
                    : Stack(
                        children: [
                          Positioned.fill(
                            child: switch (st.tab) {
                              'unread' =>
                                UnreadView(c: _c, st: st, voice: _voice),
                              'saved' =>
                                _SavedView(c: _c, st: st, voice: _voice),
                              'highlights' => _HighlightsView(c: _c, st: st),
                              _ => _TodayView(c: _c, st: st, voice: _voice),
                            },
                          ),
                          if (_voice.on)
                            Positioned(
                              left: 0,
                              right: 0,
                              bottom: 16,
                              child: ListenBar(voice: _voice),
                            ),
                        ],
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

  final FeedsController c;
  final FeedsState? st;

  @override
  State<_Header> createState() => _HeaderState();
}

class _HeaderState extends State<_Header> {
  final TextEditingController _q = TextEditingController();

  @override
  void didUpdateWidget(_Header old) {
    super.didUpdateWidget(old);
    // Cleared by picking a source: the field follows.
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
    final tab = st?.tab ?? 'today';
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
              color: kFeeds.withValues(alpha: 0.17),
              borderRadius: BorderRadius.circular(10),
            ),
            child: const Icon(Icons.rss_feed, size: 18, color: kFeeds),
          ),
          const SizedBox(width: 10),
          Text('Feeds',
              style: TextStyle(
                  fontSize: 19, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(width: 14),
          Expanded(
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: _Tabs(
                tabs: keepTabs('feeds', feedTabs, (f) => f.id,
                    active: (f) => tab == f.id),
                active: tab,
                unread: st?.unread ?? 0,
                onTap: (id) => c.send(FeedsCmd.setTab(tab: id)),
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
              onSubmitted: (v) => c.send(FeedsCmd.search(text: v)),
              decoration: InputDecoration(
                isDense: true,
                hintText: 'Search every article',
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
                          c.send(const FeedsCmd.search(text: ''));
                        },
                      ),
                filled: true,
                fillColor: t.nChip,
                contentPadding: const EdgeInsets.symmetric(vertical: 10),
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(
                      context.skin.controlRadius ?? 10),
                  borderSide: BorderSide.none,
                ),
              ),
            ),
          ),
          const SizedBox(width: 8),
          IconButton(
            tooltip: 'Refresh every feed',
            iconSize: 19,
            onPressed: c.fetching ? null : c.fetch,
            icon: const Icon(Icons.refresh),
          ),
          const SizedBox(width: 4),
          FilledButton.icon(
            style: FilledButton.styleFrom(backgroundColor: kFeeds),
            onPressed: st == null ? null : () => showFollow(context, c, st),
            icon: const Icon(Icons.add, size: 17),
            label: const Text('Follow'),
          ),
        ],
      ),
    );
  }
}

class _Tabs extends StatelessWidget {
  const _Tabs({
    required this.tabs,
    required this.active,
    required this.unread,
    required this.onTap,
  });

  final List<FeedTab> tabs;
  final String active;
  final int unread;
  final ValueChanged<String> onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final r = context.skin.controlRadius ?? 10;
    return Container(
      padding: const EdgeInsets.all(3),
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(r + 3),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (final f in tabs)
            _TabButton(
              label: f.label,
              on: f.id == active,
              count: f.id == 'unread' ? unread : 0,
              radius: r,
              onTap: () => onTap(f.id),
            ),
        ],
      ),
    );
  }
}

class _TabButton extends StatelessWidget {
  const _TabButton({
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
      color: on ? kFeeds : Colors.transparent,
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
                    color: on ? Colors.white : kFeeds,
                    borderRadius: BorderRadius.circular(99),
                  ),
                  child: Text('$count',
                      style: TextStyle(
                          fontSize: 11,
                          fontWeight: FontWeight.w700,
                          color: on ? kFeeds : Colors.white)),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

class _Strip extends StatelessWidget {
  const _Strip({
    required this.icon,
    required this.tint,
    required this.text,
    required this.onClose,
  });

  final IconData icon;
  final Color tint;
  final String text;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(22, 6, 8, 6),
      color: tint.withValues(alpha: 0.10),
      child: Row(
        children: [
          Icon(icon, size: 16, color: tint),
          const SizedBox(width: 10),
          Expanded(
            child: Text(text,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12.5, color: t.nInk)),
          ),
          IconButton(
            iconSize: 16,
            tooltip: 'Dismiss',
            onPressed: onClose,
            icon: const Icon(Icons.close),
          ),
        ],
      ),
    );
  }
}

// ------------------------------------------------------------------- today --

class _TodayView extends StatelessWidget {
  const _TodayView({required this.c, required this.st, required this.voice});

  final FeedsController c;
  final FeedsState st;
  final FeedsVoice voice;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final b = st.brief;
    final stories = b.stories;
    if (!st.hasFeeds) return FollowFirst(c: c, st: st);
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 22, 24, 90),
      children: [
        Wrap(
          spacing: 10,
          runSpacing: 10,
          crossAxisAlignment: WrapCrossAlignment.end,
          alignment: WrapAlignment.spaceBetween,
          children: [
            Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text('Your morning brief',
                    style: TextStyle(
                        fontFamily: kSerif,
                        fontSize: 34,
                        height: 1.1,
                        fontWeight: FontWeight.w600,
                        color: t.nInk)),
                const SizedBox(height: 6),
                Text(
                  stories.isEmpty
                      ? '${today()} · nothing unread'
                      : '${today()} · ${plural(stories.length, 'story', 'stories')} '
                          'worth your time from ${st.unread} unread · '
                          'about ${plural(b.minutes, 'minute')}',
                  style: TextStyle(fontSize: 13, color: t.nInk2),
                ),
              ],
            ),
            Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                OutlinedButton.icon(
                  onPressed: stories.isEmpty
                      ? null
                      : () => voice.read('The morning brief', _briefText(b)),
                  icon: const Icon(Icons.headphones_outlined, size: 16),
                  label: Text('Listen to the brief · ${clock(b.listenS)}'),
                ),
                const SizedBox(width: 10),
                FilledButton.icon(
                  style: FilledButton.styleFrom(backgroundColor: kFeeds),
                  onPressed: st.unread == 0
                      ? null
                      : () => c.send(const FeedsCmd.markRestRead()),
                  icon: const Icon(Icons.done, size: 16),
                  label: const Text('Mark the rest read'),
                ),
              ],
            ),
          ],
        ),
        const SizedBox(height: 24),
        if (stories.isEmpty)
          _Quiet(
            icon: Icons.local_cafe_outlined,
            title: 'You are all caught up',
            body: st.lastRefresh == 0
                ? 'Checking your feeds for anything new…'
                : 'Nothing unread from the last week. New stories land here '
                    'as your feeds publish them.',
          )
        else ...[
          _Lead(c: c, story: stories.first),
          if (stories.length > 1) ...[
            const SizedBox(height: 24),
            _Grid(
              min: 220,
              children: [
                for (final s in stories.skip(1)) _StoryCard(c: c, story: s),
              ],
            ),
          ],
        ],
        if (b.newsletters.isNotEmpty) ...[
          const SizedBox(height: 26),
          _H2(
            title: 'From your newsletters',
            hint: '${b.newsletters.length} new',
          ),
          const SizedBox(height: 12),
          _Grid(
            min: 260,
            gap: 12,
            children: [
              for (final a in b.newsletters) _LetterRow(c: c, a: a),
            ],
          ),
        ],
      ],
    );
  }
}

String _briefText(Brief b) => [
      for (final s in b.stories) [s.title, ...s.bullets].join('\n'),
    ].join('\n\n');

class _Lead extends StatelessWidget {
  const _Lead({required this.c, required this.story});

  final FeedsController c;
  final Story story;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final many = story.sources.length;
    final art = Stack(
      fit: StackFit.expand,
      children: [
        Art(url: story.image, seed: story.articleId, width: 900),
        if (many > 1)
          Positioned(
            left: 14,
            top: 14,
            child: Container(
              height: 22,
              padding: const EdgeInsets.symmetric(horizontal: 8),
              decoration: BoxDecoration(
                color: Colors.black.withValues(alpha: 0.45),
                borderRadius: BorderRadius.circular(6),
              ),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  const Icon(Icons.auto_awesome, size: 12, color: Colors.white),
                  const SizedBox(width: 5),
                  Text('$many sources, one story',
                      style: const TextStyle(
                          fontSize: 11,
                          fontWeight: FontWeight.w700,
                          color: Colors.white)),
                ],
              ),
            ),
          ),
      ],
    );
    final body = Padding(
      padding: const EdgeInsets.fromLTRB(22, 20, 22, 20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(story.title,
              style: TextStyle(
                  fontFamily: kSerif,
                  fontSize: 27,
                  height: 1.2,
                  fontWeight: FontWeight.w600,
                  color: t.nInk)),
          const SizedBox(height: 12),
          for (final line in story.bullets)
            Padding(
              padding: const EdgeInsets.only(bottom: 6),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Padding(
                    padding: const EdgeInsets.only(top: 8, right: 10),
                    child: Container(
                      width: 5,
                      height: 5,
                      decoration: BoxDecoration(
                          color: t.nInk3, shape: BoxShape.circle),
                    ),
                  ),
                  Expanded(
                    child: Text(line,
                        style: TextStyle(
                            fontSize: 14, height: 1.45, color: t.nInk2)),
                  ),
                ],
              ),
            ),
          const SizedBox(height: 8),
          Row(
            children: [
              _Avatars(names: story.sources),
              const SizedBox(width: 10),
              Expanded(
                child: Text(_tellers(story.sources),
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 12, color: t.nInk3)),
              ),
              OutlinedButton(
                onPressed: () => c.open(story.articleId),
                child: Text(many > 1 ? 'Read the coverage' : 'Read'),
              ),
            ],
          ),
        ],
      ),
    );
    return LayoutBuilder(
      builder: (context, box) {
        final wide = box.maxWidth >= 820;
        return Container(
          clipBehavior: Clip.antiAlias,
          decoration: cardDeco(context),
          child: wide
              ? IntrinsicHeight(
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      Expanded(
                        flex: 11,
                        child: ConstrainedBox(
                          constraints: const BoxConstraints(minHeight: 250),
                          child: art,
                        ),
                      ),
                      Expanded(flex: 10, child: body),
                    ],
                  ),
                )
              : Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [SizedBox(height: 200, child: art), body],
                ),
        );
      },
    );
  }
}

/// "Circuit, Deep Stack and 3 more".
String _tellers(List<String> names) => switch (names.length) {
      0 => '',
      1 => names.first,
      2 => '${names[0]} and ${names[1]}',
      _ => '${names[0]}, ${names[1]} and ${names.length - 2} more',
    };

class _Avatars extends StatelessWidget {
  const _Avatars({required this.names});

  final List<String> names;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final shown = names.take(5).toList();
    return SizedBox(
      width: shown.isEmpty ? 0 : 24 + (shown.length - 1) * 18.0,
      height: 24,
      child: Stack(
        children: [
          for (var i = 0; i < shown.length; i++)
            Positioned(
              left: i * 18.0,
              child: Container(
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  border: Border.all(color: t.nCard, width: 2),
                ),
                child: SourceMark(
                    name: shown[i],
                    size: 22,
                    round: true),
              ),
            ),
        ],
      ),
    );
  }
}

class _StoryCard extends StatelessWidget {
  const _StoryCard({required this.c, required this.story});

  final FeedsController c;
  final Story story;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final first = story.sources.isEmpty ? '' : story.sources.first;
    final more = story.sources.length - 1;
    return _Tap(
      onTap: () => c.open(story.articleId),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SizedBox(
            height: 110,
            child: Art(url: story.image, seed: story.articleId, width: 480),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 12, 14, 14),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    SourceMark(name: first, size: 18),
                    const SizedBox(width: 7),
                    Expanded(
                      child: Text(more > 0 ? '$first + $more more' : first,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 12, color: t.nInk3)),
                    ),
                  ],
                ),
                const SizedBox(height: 6),
                Text(story.title,
                    maxLines: 3,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontFamily: kSerif,
                        fontSize: 17,
                        height: 1.3,
                        fontWeight: FontWeight.w600,
                        color: t.nInk)),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _LetterRow extends StatelessWidget {
  const _LetterRow({required this.c, required this.a});

  final FeedsController c;
  final ArticleRow a;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return _Tap(
      onTap: () => c.open(a.id),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(14, 12, 14, 12),
        child: Row(
          children: [
            SourceMark(name: a.source, size: 34),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(a.source,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 13.5,
                          fontWeight: FontWeight.w700,
                          color: t.nInk)),
                  Text('${a.title} · ${a.minutes} min',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 12, color: t.nInk2)),
                ],
              ),
            ),
            Icon(Icons.chevron_right, size: 17, color: t.nInk3),
          ],
        ),
      ),
    );
  }
}

// -------------------------------------------------------------- read later --

class _SavedView extends StatelessWidget {
  const _SavedView({required this.c, required this.st, required this.voice});

  final FeedsController c;
  final FeedsState st;
  final FeedsVoice voice;

  static const _filters = [
    ('all', 'All'),
    ('started', 'Started'),
    ('new', 'Not started'),
    ('done', 'Finished'),
  ];

  @override
  Widget build(BuildContext context) {
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 22, 24, 90),
      children: [
        Wrap(
          spacing: 12,
          runSpacing: 10,
          crossAxisAlignment: WrapCrossAlignment.center,
          alignment: WrapAlignment.spaceBetween,
          children: [
            _H2(
              title: 'Read later',
              hint:
                  '${plural(st.savedTotal.toInt(), 'article')} · kept offline in full',
              size: 20,
            ),
            Wrap(
              spacing: 8,
              children: [
                for (final (id, label) in _filters)
                  ChoiceChip(
                    label: Text(label),
                    selected: st.savedFilter == id,
                    onSelected: (_) =>
                        c.send(FeedsCmd.setSavedFilter(filter: id)),
                  ),
              ],
            ),
          ],
        ),
        const SizedBox(height: 18),
        if (st.saved.isEmpty)
          _Quiet(
            icon: Icons.bookmark_border,
            title: st.savedTotal == 0 ? 'Nothing saved yet' : 'None here',
            body: st.savedTotal == 0
                ? 'Press Save in the reader to keep an article here, whole and '
                    'readable offline.'
                : 'No saved article matches this filter.',
          )
        else
          _Grid(
            min: 280,
            children: [
              for (final a in st.saved) _SavedCard(c: c, a: a, voice: voice),
            ],
          ),
      ],
    );
  }
}

class _SavedCard extends StatelessWidget {
  const _SavedCard({required this.c, required this.a, required this.voice});

  final FeedsController c;
  final ArticleRow a;
  final FeedsVoice voice;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final p = a.progress.clamp(0.0, 1.0);
    final state = p >= 0.98
        ? 'Finished'
        : p <= 0
            ? 'Not started'
            : '${(p * 100).round()}% read';
    return _Tap(
      onTap: () => c.open(a.id),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 14, 12, 12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                SourceMark(name: a.source, size: 18),
                const SizedBox(width: 7),
                Expanded(
                  child: Text(a.source,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 12, color: t.nInk3)),
                ),
                Text('${a.minutes} min',
                    style: TextStyle(fontSize: 12, color: t.nInk3)),
              ],
            ),
            const SizedBox(height: 10),
            Text(a.title,
                maxLines: 3,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontFamily: kSerif,
                    fontSize: 18,
                    height: 1.3,
                    fontWeight: FontWeight.w600,
                    color: t.nInk)),
            const SizedBox(height: 12),
            ClipRRect(
              borderRadius: BorderRadius.circular(9),
              child: LinearProgressIndicator(
                value: p.toDouble(),
                minHeight: 6,
                color: kFeeds,
                backgroundColor: t.nChip,
              ),
            ),
            const SizedBox(height: 8),
            Row(
              children: [
                Text(state, style: TextStyle(fontSize: 11, color: t.nInk3)),
                const Spacer(),
                IconButton(
                  tooltip: 'Remove from Read later',
                  iconSize: 17,
                  onPressed: () => c.send(FeedsCmd.toggleSaved(id: a.id)),
                  icon: const Icon(Icons.bookmark_remove_outlined),
                ),
                IconButton(
                  tooltip: 'Listen',
                  iconSize: 17,
                  onPressed: () async {
                    final text = await feedsArticleText(id: a.id);
                    voice.read(a.title, text);
                  },
                  icon: const Icon(Icons.headphones_outlined),
                ),
                const SizedBox(width: 4),
                OutlinedButton.icon(
                  onPressed: () => c.send(FeedsCmd.sendToBooks(id: a.id)),
                  icon: const Icon(Icons.menu_book_outlined, size: 15),
                  label: const Text('Send to Books'),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

// -------------------------------------------------------------- highlights --

class _HighlightsView extends StatelessWidget {
  const _HighlightsView({required this.c, required this.st});

  final FeedsController c;
  final FeedsState st;

  Future<void> _export(BuildContext context) async {
    final path = await dialogSaveFile(
      title: 'Export highlights',
      fileName: 'Highlights.md',
      label: 'Markdown',
      extensions: const ['md'],
    );
    if (path == null || path.isEmpty) return;
    try {
      final n = await feedsExportHighlights(path: path);
      c.say('${plural(n.toInt(), 'highlight')} written to $path');
    } catch (e) {
      c.say('The highlights could not be written: ${plainError(e)}');
    }
  }

  @override
  Widget build(BuildContext context) {
    final n = st.highlights.length;
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 22, 24, 90),
      children: [
        Row(
          children: [
            Expanded(
              child: _H2(
                title: 'Highlights',
                hint:
                    '${plural(n, 'passage')} from ${plural(st.highlightedArticles.toInt(), 'article')}',
                size: 20,
              ),
            ),
            OutlinedButton.icon(
              onPressed: n == 0 ? null : () => _export(context),
              icon: const Icon(Icons.download_outlined, size: 16),
              label: const Text('Export as Markdown'),
            ),
          ],
        ),
        const SizedBox(height: 18),
        if (n == 0)
          const _Quiet(
            icon: Icons.format_quote_outlined,
            title: 'No highlights yet',
            body: 'Select a passage in the reader and press Highlight. It '
                'lands here with the article it came from, and a note if you '
                'add one.',
          )
        else
          _Grid(
            min: 320,
            children: [
              for (final h in st.highlights) _HighlightCard(c: c, h: h),
            ],
          ),
      ],
    );
  }
}

class _HighlightCard extends StatelessWidget {
  const _HighlightCard({required this.c, required this.h});

  final FeedsController c;
  final HighlightRow h;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      decoration: cardDeco(context),
      padding: const EdgeInsets.fromLTRB(18, 16, 10, 12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Container(
            padding: const EdgeInsets.only(left: 14),
            decoration: const BoxDecoration(
              border: Border(left: BorderSide(color: kMark, width: 3)),
            ),
            child: Text(h.quote,
                style: TextStyle(
                    fontFamily: kSerif,
                    fontStyle: FontStyle.italic,
                    fontSize: 19,
                    height: 1.5,
                    color: t.nInk)),
          ),
          const SizedBox(height: 10),
          InkWell(
            onTap: () async {
              final note = await askNote(context, initial: h.note);
              if (note != null) {
                c.send(FeedsCmd.setHighlightNote(id: h.id, note: note));
              }
            },
            child: Row(
              children: [
                Icon(Icons.edit_outlined, size: 13, color: t.nInk3),
                const SizedBox(width: 6),
                Expanded(
                  child: Text(h.note.isEmpty ? 'Add a note' : h.note,
                      style: TextStyle(
                          fontSize: 13,
                          color: h.note.isEmpty ? t.nInk3 : t.nInk2)),
                ),
              ],
            ),
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              SourceMark(name: h.source, size: 18),
              const SizedBox(width: 7),
              Expanded(
                child: InkWell(
                  onTap: () => c.open(h.articleId),
                  child: Text('${h.source} · ${h.title}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 12, color: t.nInk3)),
                ),
              ),
              IconButton(
                tooltip: 'Delete highlight',
                iconSize: 16,
                onPressed: () =>
                    c.send(FeedsCmd.deleteHighlight(id: h.id)),
                icon: const Icon(Icons.delete_outline),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

// ------------------------------------------------------------------ follow --

/// Paste a site, a feed or a newsletter's feed address, pick a folder.
Future<void> showFollow(
  BuildContext context,
  FeedsController c,
  FeedsState st,
) {
  return showDialog<void>(
    context: context,
    builder: (ctx) => _FollowDialog(c: c, folders: st.folders),
  );
}

class _FollowDialog extends StatefulWidget {
  const _FollowDialog({required this.c, required this.folders});

  final FeedsController c;
  final List<String> folders;

  @override
  State<_FollowDialog> createState() => _FollowDialogState();
}

class _FollowDialogState extends State<_FollowDialog> {
  final _url = TextEditingController();
  final _folder = TextEditingController();
  bool _busy = false;
  String _error = '';

  @override
  void dispose() {
    _url.dispose();
    _folder.dispose();
    super.dispose();
  }

  Future<void> _go() async {
    if (_url.text.trim().isEmpty || _busy) return;
    setState(() {
      _busy = true;
      _error = '';
    });
    final err = await widget.c.follow(_url.text.trim(), _folder.text.trim());
    if (!mounted) return;
    if (err == null) {
      Navigator.of(context).pop();
    } else {
      setState(() {
        _busy = false;
        _error = err;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final suggest = {...widget.folders, 'Newsletters'}.toList();
    return AlertDialog(
      title: const Text('Follow a site or newsletter'),
      content: SizedBox(
        width: 460,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              'Paste the site’s address or its feed. RSS, Atom and JSON Feed '
              'all work, and so do newsletters that publish a feed — most '
              'Substack, Buttondown and Ghost ones do.',
              style: TextStyle(fontSize: 12.5, height: 1.5, color: t.nInk2),
            ),
            const SizedBox(height: 14),
            TextField(
              controller: _url,
              autofocus: true,
              enabled: !_busy,
              onSubmitted: (_) => _go(),
              decoration: const InputDecoration(
                labelText: 'Address',
                hintText: 'example.com or example.com/feed.xml',
                prefixIcon: Icon(Icons.link, size: 18),
              ),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: _folder,
              enabled: !_busy,
              onSubmitted: (_) => _go(),
              decoration: const InputDecoration(
                labelText: 'Folder (optional)',
                prefixIcon: Icon(Icons.folder_outlined, size: 18),
              ),
            ),
            const SizedBox(height: 8),
            Wrap(
              spacing: 6,
              runSpacing: 6,
              children: [
                for (final f in suggest)
                  ActionChip(
                    label: Text(f, style: const TextStyle(fontSize: 12)),
                    onPressed: _busy
                        ? null
                        : () => setState(() => _folder.text = f),
                  ),
              ],
            ),
            if (_error.isNotEmpty) ...[
              const SizedBox(height: 12),
              Text(_error,
                  style: const TextStyle(fontSize: 12.5, color: Tokens.error)),
            ],
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: _busy ? null : () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          style: FilledButton.styleFrom(backgroundColor: kFeeds),
          onPressed: _busy ? null : _go,
          child: _busy
              ? const SizedBox(
                  width: 16,
                  height: 16,
                  child: CircularProgressIndicator(
                      strokeWidth: 2, color: Colors.white),
                )
              : const Text('Follow'),
        ),
      ],
    );
  }
}

/// A note for a highlight. Null when cancelled; empty clears it.
Future<String?> askNote(BuildContext context, {String initial = ''}) {
  final ctl = TextEditingController(text: initial);
  return showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Note'),
      content: SizedBox(
        width: 420,
        child: TextField(
          controller: ctl,
          autofocus: true,
          minLines: 2,
          maxLines: 5,
          decoration: const InputDecoration(
              hintText: 'Why this passage matters (optional)'),
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
          child: const Text('Save'),
        ),
      ],
    ),
  );
}

/// No feeds yet: what the section is, and the one button that starts it.
class FollowFirst extends StatelessWidget {
  const FollowFirst({super.key, required this.c, required this.st});

  final FeedsController c;
  final FeedsState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 480),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              width: 56,
              height: 56,
              decoration: BoxDecoration(
                gradient: const LinearGradient(colors: [kFeeds, kFeeds2]),
                borderRadius: BorderRadius.circular(16),
              ),
              child: const Icon(Icons.rss_feed, color: Colors.white, size: 28),
            ),
            const SizedBox(height: 16),
            Text('Sites, blogs and newsletters in one calm reader',
                textAlign: TextAlign.center,
                style: TextStyle(
                    fontFamily: kSerif,
                    fontSize: 24,
                    fontWeight: FontWeight.w600,
                    color: t.nInk)),
            const SizedBox(height: 10),
            Text(
              'Follow a few and a morning brief gathers the same story from '
              'several of them. Summaries are written on this computer, and '
              'any article can be read aloud, highlighted, or sent to Books.',
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 13, height: 1.5, color: t.nInk2),
            ),
            const SizedBox(height: 18),
            FilledButton.icon(
              style: FilledButton.styleFrom(backgroundColor: kFeeds),
              onPressed: () => showFollow(context, c, st),
              icon: const Icon(Icons.add, size: 17),
              label: const Text('Follow your first site'),
            ),
          ],
        ),
      ),
    );
  }
}

// ------------------------------------------------------------------ listen --

class ListenBar extends StatelessWidget {
  const ListenBar({super.key, required this.voice});

  final FeedsVoice voice;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final has = voice.sentences.isNotEmpty;
    return Center(
      child: Container(
        constraints: const BoxConstraints(maxWidth: 620),
        margin: const EdgeInsets.symmetric(horizontal: 16),
        padding: const EdgeInsets.fromLTRB(10, 8, 8, 8),
        decoration: BoxDecoration(
          color: t.modal,
          borderRadius: BorderRadius.circular(Tokens.radiusLg),
          border: Border.all(color: t.outline),
          boxShadow: const [
            BoxShadow(color: Color(0x59000000), blurRadius: 28),
          ],
        ),
        child: Row(
          children: [
            IconButton(
              tooltip: 'Previous sentence',
              onPressed: has ? () => voice.step(-1) : null,
              icon: const Icon(Icons.skip_previous),
            ),
            IconButton.filled(
              style: IconButton.styleFrom(backgroundColor: kFeeds),
              tooltip: voice.playing ? 'Pause' : 'Play',
              onPressed: has ? voice.playPause : null,
              icon: Icon(voice.playing ? Icons.pause : Icons.play_arrow),
            ),
            IconButton(
              tooltip: 'Next sentence',
              onPressed: has ? () => voice.step(1) : null,
              icon: const Icon(Icons.skip_next),
            ),
            const SizedBox(width: 8),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(voice.label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 13,
                          fontWeight: FontWeight.w700,
                          color: t.nInk)),
                  Text(
                    voice.note.isNotEmpty
                        ? voice.note
                        : has
                            ? '${voice.active + 1} of ${voice.sentences.length}'
                            : 'Getting ready…',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11.5, color: t.nInk3),
                  ),
                ],
              ),
            ),
            IconButton(
              tooltip: 'Stop',
              onPressed: voice.stop,
              icon: const Icon(Icons.close),
            ),
          ],
        ),
      ),
    );
  }
}

// ------------------------------------------------------------------ shared --

/// The highlighter's yellow.
const Color kMark = Color(0xFFFACC15);

Decoration cardDeco(BuildContext context, {double? radius}) {
  final t = context.tokens;
  final r = radius ?? context.skin.panelRadius ?? 14;
  return context.skin.surface(SurfaceRole.card, radius: r) ??
      BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(r),
        border: Border.all(color: t.nHair),
      );
}

/// A source's mark: its initial on its colour.
class SourceMark extends StatelessWidget {
  const SourceMark({
    super.key,
    required this.name,
    this.size = 34,
    this.round = false,
  });

  final String name;
  final double size;
  final bool round;

  @override
  Widget build(BuildContext context) {
    final letter = name.trim().isEmpty ? '?' : name.trim()[0].toUpperCase();
    return Container(
      width: size,
      height: size,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: nameColor(name),
        borderRadius: BorderRadius.circular(round ? size : size * 0.29),
      ),
      child: Text(letter,
          style: TextStyle(
              fontSize: size * 0.4,
              fontWeight: FontWeight.w800,
              color: Colors.white)),
    );
  }
}

/// An article's picture, or a gradient in its source's colour while there is
/// none — or when the picture will not load.
class Art extends StatelessWidget {
  const Art({super.key, required this.url, required this.seed, this.width = 200});

  final String url;
  final int seed;

  /// Decoded no wider than this, so a list of thumbnails does not hold every
  /// photo at full size.
  final int width;

  @override
  Widget build(BuildContext context) {
    final base = sourceColor(seed);
    final fill = DecoratedBox(
      decoration: BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [base, Color.lerp(base, const Color(0xFF1E1B4B), 0.55)!],
        ),
      ),
    );
    if (url.isEmpty) return fill;
    return Image.network(
      url,
      fit: BoxFit.cover,
      cacheWidth: width,
      errorBuilder: (_, __, ___) => fill,
      frameBuilder: (_, child, frame, sync) =>
          frame == null && !sync ? fill : child,
    );
  }
}

class _H2 extends StatelessWidget {
  const _H2({required this.title, this.hint = '', this.size = 16.5});

  final String title;
  final String hint;
  final double size;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Text.rich(
      TextSpan(children: [
        TextSpan(
            text: title,
            style: TextStyle(
                fontSize: size, fontWeight: FontWeight.w700, color: t.nInk)),
        if (hint.isNotEmpty)
          TextSpan(
              text: '   $hint',
              style: TextStyle(
                  fontSize: 12.5,
                  fontWeight: FontWeight.w500,
                  color: t.nInk3)),
      ]),
    );
  }
}

/// A card you can press.
class _Tap extends StatelessWidget {
  const _Tap({required this.child, required this.onTap});

  final Widget child;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final r = context.skin.panelRadius ?? 14;
    return Container(
      clipBehavior: Clip.antiAlias,
      decoration: cardDeco(context, radius: r),
      child: Material(
        type: MaterialType.transparency,
        child: InkWell(onTap: onTap, child: child),
      ),
    );
  }
}

/// Cards in columns of at least [min] wide, filling the row.
class _Grid extends StatelessWidget {
  const _Grid({required this.children, required this.min, this.gap = 14});

  final List<Widget> children;
  final double min;
  final double gap;

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, box) {
        final cols = ((box.maxWidth + gap) / (min + gap)).floor().clamp(1, 99);
        final w = (box.maxWidth - gap * (cols - 1)) / cols;
        return Wrap(
          spacing: gap,
          runSpacing: gap,
          children: [
            for (final c in children) SizedBox(width: w, child: c),
          ],
        );
      },
    );
  }
}

class _Quiet extends StatelessWidget {
  const _Quiet({required this.icon, required this.title, required this.body});

  final IconData icon;
  final String title;
  final String body;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(28),
      decoration: cardDeco(context),
      child: Column(
        children: [
          Icon(icon, size: 30, color: t.nInk3),
          const SizedBox(height: 10),
          Text(title,
              style: TextStyle(
                  fontSize: 15, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(height: 6),
          Text(body,
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 12.5, height: 1.5, color: t.nInk2)),
        ],
      ),
    );
  }
}
