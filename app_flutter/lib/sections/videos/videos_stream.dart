// Stream: the landing screen, the results grid with its Info Preview panel, and
// the detail pane with the four picker columns.
//
// The Downloads, History, Bookmarks and Stream Setting surfaces are in
// videos_stream_pages.dart — they are separate views over the same snapshot and
// this file was long enough already.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/videos.dart';
import 'videos_controller.dart';
import 'videos_library.dart' show RailTitle;
import 'videos_stream_pages.dart';
import 'videos_widgets.dart';

/// The Stream tab body. Which sub-view is showing lives in the shell, because
/// the header destinations that switch it are drawn there.
class VideosStream extends StatelessWidget {
  const VideosStream({
    super.key,
    required this.controller,
    required this.stream,
    required this.query,
    required this.view,
    required this.onView,
  });

  final VideosController controller;
  final StreamView stream;
  final TextEditingController query;

  /// search | downloads | history | bookmarks.
  final String view;
  final ValueChanged<String> onView;

  @override
  Widget build(BuildContext context) {
    switch (view) {
      case 'downloads':
        return StreamDownloadsPage(
          controller: controller,
          stream: stream,
          onBack: () => onView('search'),
        );
      case 'history':
        return StreamHistoryPage(
          controller: controller,
          stream: stream,
          onBack: () => onView('search'),
        );
      case 'bookmarks':
        return StreamBookmarksPage(
          controller: controller,
          stream: stream,
          onBack: () => onView('search'),
          onOpen: (id) {
            onView('search');
            controller.send(VideosCmd.streamOpen(id: id));
          },
        );
    }
    if (stream.detailOpen) {
      return _Detail(controller: controller, stream: stream);
    }
    if (stream.results.isEmpty) {
      return _Landing(controller: controller, stream: stream, query: query);
    }
    return _Results(controller: controller, stream: stream, query: query);
  }
}

// --------------------------------------------------------------- landing ----

class _Landing extends StatelessWidget {
  const _Landing({
    required this.controller,
    required this.stream,
    required this.query,
  });

  final VideosController controller;
  final StreamView stream;
  final TextEditingController query;

  @override
  Widget build(BuildContext context) {
    return ListView(
      padding: const EdgeInsets.symmetric(vertical: 28),
      children: [
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            if (stream.vertical.isNotEmpty) ...[
              _VerticalDramas(controller: controller, cards: stream.vertical),
              const SizedBox(width: 18),
            ],
            _SearchBlock(controller: controller, stream: stream, query: query),
            if (stream.recent.isNotEmpty) ...[
              const SizedBox(width: 18),
              _RecentBlock(
                  controller: controller, stream: stream, query: query),
            ],
          ],
        ),
        if (stream.resume.isNotEmpty || stream.trending.isNotEmpty) ...[
          const SizedBox(height: 32),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 34),
            child: SizedBox(
              height: 274,
              child: Row(
                children: [
                  // Thirty per cent of the row is what you were watching…
                  if (stream.resume.isNotEmpty)
                    Expanded(
                      flex: 3,
                      child:
                          _ResumeBlock(controller: controller, stream: stream),
                    ),
                  if (stream.resume.isNotEmpty && stream.trending.isNotEmpty)
                    const SizedBox(width: 22),
                  // …and seventy is what the catalogue is pushing.
                  if (stream.trending.isNotEmpty)
                    Expanded(
                      flex: 7,
                      child: _TrendingBlock(
                          controller: controller, stream: stream),
                    ),
                ],
              ),
            ),
          ),
        ],
      ],
    );
  }
}

/// The frosted double-outline frame the three landing blocks share.
class _Framed extends StatelessWidget {
  const _Framed({
    required this.child,
    required this.hue,
    this.width,
    this.padding = const EdgeInsets.all(13),
  });

  final Widget child;
  final Color hue;
  final double? width;
  final EdgeInsets padding;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      width: width,
      padding: const EdgeInsets.all(7),
      decoration: BoxDecoration(
        color: t.dark
            ? (t.oled ? const Color(0xC40C0C12) : const Color(0xC414162A))
            : const Color(0xC4FFFFFF),
        borderRadius: BorderRadius.circular(22),
        border: Border.all(color: hue.withValues(alpha: 0.55), width: 1.5),
      ),
      child: Container(
        padding: padding,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(16),
          border: Border.all(color: hue.withValues(alpha: 0.30)),
        ),
        child: child,
      ),
    );
  }
}

class _SearchBlock extends StatelessWidget {
  const _SearchBlock({
    required this.controller,
    required this.stream,
    required this.query,
  });

  final VideosController controller;
  final StreamView stream;
  final TextEditingController query;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final fourk = stream.source == 'fourk';
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        // Which catalogue is being searched. Two unrelated services, so the
        // switcher sits with the search: it decides what every result on the
        // page came from.
        Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            VideoTab(
              hue: cPlay,
              label: 'MovieBox',
              active: stream.source == 'moviebox',
              onTap: () => controller
                  .send(const VideosCmd.streamSetSource(name: 'moviebox')),
            ),
            const SizedBox(width: 8),
            VideoTab(
              hue: cSave,
              label: '4KHDHub',
              active: fourk,
              onTap: () => controller
                  .send(const VideosCmd.streamSetSource(name: 'fourk')),
            ),
          ],
        ),
        const SizedBox(height: 14),
        _Framed(
          hue: cInfo,
          width: 640,
          padding: const EdgeInsets.all(26),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              Container(
                width: 104,
                height: 104,
                decoration: BoxDecoration(
                  color: Tokens.secVideos.withValues(alpha: 0.13),
                  borderRadius: BorderRadius.circular(30),
                  border: Border.all(
                      color: Tokens.secVideos.withValues(alpha: 0.33),
                      width: 1.5),
                ),
                child: const Icon(Icons.smart_display_outlined,
                    size: 50, color: Tokens.secVideos),
              ),
              const SizedBox(height: 14),
              Text(
                stream.busy ? 'Searching…' : 'Stream',
                style: TextStyle(
                    fontSize: 26, fontWeight: FontWeight.w800, color: t.nInk),
              ),
              const SizedBox(height: 22),
              VideoSearchField(
                controller: query,
                width: 560,
                large: true,
                hint:
                    fourk ? 'Search Movies in 4K' : 'Search movies and series',
                onChanged: (q) =>
                    controller.sendQuiet(VideosCmd.streamSuggest(query: q)),
                onSubmitted: (q) {
                  controller.send(const VideosCmd.streamSuggestClear());
                  controller.send(VideosCmd.streamSearch(query: q));
                },
              ),
              if (stream.suggestions.isNotEmpty) ...[
                const SizedBox(height: 8),
                Container(
                  width: 560,
                  constraints: const BoxConstraints(maxHeight: 214),
                  decoration: BoxDecoration(
                    color: t.modal,
                    borderRadius: BorderRadius.circular(12),
                    border: Border.all(color: t.nHair),
                  ),
                  child: ListView(
                    shrinkWrap: true,
                    padding: const EdgeInsets.all(5),
                    children: [
                      for (final name in stream.suggestions)
                        ListTile(
                          dense: true,
                          title: Text(name,
                              style: TextStyle(fontSize: 12.5, color: t.nInk)),
                          onTap: () {
                            query.text = name;
                            controller
                                .send(const VideosCmd.streamSuggestClear());
                            controller
                                .send(VideosCmd.streamSearch(query: name));
                          },
                        ),
                    ],
                  ),
                ),
              ],
              // Status only matters here when something went wrong.
              if (stream.status.isNotEmpty) ...[
                const SizedBox(height: 14),
                Text(
                  stream.status,
                  textAlign: TextAlign.center,
                  style: TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                    color: stream.busy ? Tokens.secVideos : t.nInk3,
                  ),
                ),
              ],
            ],
          ),
        ),
      ],
    );
  }
}

/// One card at a time, cycled by the arrows. The Slint page rotates these on a
/// ten-second beat; here they only move when asked, which is the difference
/// between a widget tree that rebuilds on a timer and one that does not.
class _VerticalDramas extends StatefulWidget {
  const _VerticalDramas({required this.controller, required this.cards});

  final VideosController controller;
  final List<StreamCard> cards;

  @override
  State<_VerticalDramas> createState() => _VerticalDramasState();
}

class _VerticalDramasState extends State<_VerticalDramas> {
  int _i = 0;

  void _step(int by) => setState(() {
        final n = widget.cards.length;
        _i = (_i + by + n) % n;
      });

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final card = widget.cards[_i.clamp(0, widget.cards.length - 1)];
    return Padding(
      padding: const EdgeInsets.only(top: 50),
      child: _Framed(
        hue: cDl,
        width: 260,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                const Text('VERTICAL DRAMAS',
                    style: TextStyle(
                        fontSize: 11,
                        fontWeight: FontWeight.w800,
                        letterSpacing: 1.3,
                        color: cDl)),
                const Spacer(),
                Text('${_i + 1}/${widget.cards.length}',
                    style: TextStyle(
                        fontSize: 11,
                        fontWeight: FontWeight.w700,
                        color: t.nInk3)),
              ],
            ),
            const SizedBox(height: 9),
            Row(
              children: [
                IconBtn(
                    icon: Icons.chevron_left,
                    size: 28,
                    hue: cDl,
                    onTap: () => _step(-1)),
                Expanded(
                  child: _MiniPoster(
                    card: card,
                    height: 275,
                    hue: cDl,
                    onTap: () => widget.controller
                        .send(VideosCmd.streamOpenPick(id: card.id)),
                  ),
                ),
                IconBtn(
                    icon: Icons.chevron_right,
                    size: 28,
                    hue: cDl,
                    onTap: () => _step(1)),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

class _RecentBlock extends StatelessWidget {
  const _RecentBlock({
    required this.controller,
    required this.stream,
    required this.query,
  });

  final VideosController controller;
  final StreamView stream;
  final TextEditingController query;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(top: 50),
      child: _Framed(
        hue: cPlay,
        width: 260,
        padding: const EdgeInsets.all(12),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                const Text('RECENT',
                    style: TextStyle(
                        fontSize: 11,
                        fontWeight: FontWeight.w800,
                        letterSpacing: 1.3,
                        color: cPlay)),
                const Spacer(),
                GestureDetector(
                  onTap: () =>
                      controller.send(const VideosCmd.streamRecentClear()),
                  child: MouseRegion(
                    cursor: SystemMouseCursors.click,
                    child: Text('Clear',
                        style: TextStyle(
                            fontSize: 9,
                            fontWeight: FontWeight.w700,
                            color: t.nInk3)),
                  ),
                ),
              ],
            ),
            const SizedBox(height: 8),
            for (final term in stream.recent)
              Padding(
                padding: const EdgeInsets.only(bottom: 6),
                child: InkWell(
                  onTap: () {
                    query.text = term;
                    controller.send(VideosCmd.streamSearch(query: term));
                  },
                  borderRadius: BorderRadius.circular(8),
                  child: Container(
                    height: 30,
                    alignment: Alignment.centerLeft,
                    padding: const EdgeInsets.symmetric(horizontal: 10),
                    decoration: BoxDecoration(
                      color: t.panel2,
                      borderRadius: BorderRadius.circular(8),
                    ),
                    child: Text(term,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 12, color: t.nInk)),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

class _ResumeBlock extends StatefulWidget {
  const _ResumeBlock({required this.controller, required this.stream});

  final VideosController controller;
  final StreamView stream;

  @override
  State<_ResumeBlock> createState() => _ResumeBlockState();
}

class _ResumeBlockState extends State<_ResumeBlock> {
  int _i = 0;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final cards = widget.stream.resume;
    final card = cards[_i.clamp(0, cards.length - 1)];
    void step(int by) => setState(() {
          _i = (_i + by + cards.length) % cards.length;
        });

    return Container(
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: cSave.withValues(alpha: 0.55), width: 1.5),
        color: t.panel.withValues(alpha: 0.5),
      ),
      padding: const EdgeInsets.all(14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              const Text('CONTINUE WATCHING',
                  style: TextStyle(
                      fontSize: 10,
                      fontWeight: FontWeight.w800,
                      letterSpacing: 1.3,
                      color: cSave)),
              const Spacer(),
              Text('${_i + 1}/${cards.length}',
                  style: TextStyle(
                      fontSize: 10,
                      fontWeight: FontWeight.w700,
                      color: t.nInk3)),
            ],
          ),
          const SizedBox(height: 8),
          Expanded(
            child: Row(
              children: [
                IconBtn(
                    icon: Icons.chevron_left,
                    size: 26,
                    hue: cSave,
                    onTap: () => step(-1)),
                Expanded(
                  child: _ResumeCard(
                    card: card,
                    onTap: () => widget.controller
                        .send(VideosCmd.streamOpenResume(id: card.id)),
                  ),
                ),
                IconBtn(
                    icon: Icons.chevron_right,
                    size: 26,
                    hue: cSave,
                    onTap: () => step(1)),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _ResumeCard extends StatelessWidget {
  const _ResumeCard({required this.card, required this.onTap});

  final StreamCard card;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(12),
      child: Container(
        padding: const EdgeInsets.all(10),
        decoration: BoxDecoration(
          color: t.panel2,
          borderRadius: BorderRadius.circular(12),
          border: Border.all(color: cSave.withValues(alpha: 0.30)),
        ),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Artwork(path: card.poster, width: 100, height: 150, radius: 6),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(card.title,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 15,
                          fontWeight: FontWeight.w800,
                          color: t.nInk)),
                  if (card.year.isNotEmpty)
                    Text(card.year,
                        style: TextStyle(fontSize: 11, color: t.nInk2)),
                  const SizedBox(height: 6),
                  if (card.meta.isNotEmpty)
                    Text(card.meta,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11, color: t.nInk3)),
                  const Spacer(),
                  if (card.progress > 0)
                    ClipRRect(
                      borderRadius: BorderRadius.circular(2),
                      child: ProgressStrip(value: card.progress, hue: cSave),
                    ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _TrendingBlock extends StatelessWidget {
  const _TrendingBlock({required this.controller, required this.stream});

  final VideosController controller;
  final StreamView stream;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: cCopy.withValues(alpha: 0.55), width: 1.5),
        color: t.panel.withValues(alpha: 0.5),
      ),
      padding: const EdgeInsets.all(12),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const Text('TRENDING',
              style: TextStyle(
                  fontSize: 10,
                  fontWeight: FontWeight.w800,
                  letterSpacing: 1.3,
                  color: cCopy)),
          const SizedBox(height: 8),
          Expanded(
            child: Row(
              children: [
                for (final card in stream.trending)
                  Expanded(
                    child: Padding(
                      padding: const EdgeInsets.only(right: 14),
                      child: _MiniPoster(
                        card: card,
                        hue: cCopy,
                        onTap: () => controller
                            .send(VideosCmd.streamOpenPick(id: card.id)),
                      ),
                    ),
                  ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _MiniPoster extends StatelessWidget {
  const _MiniPoster({
    required this.card,
    required this.onTap,
    required this.hue,
    this.height,
  });

  final StreamCard card;
  final Color hue;
  final double? height;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(10),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          SizedBox(
            height: height ?? 190,
            child: Stack(
              fit: StackFit.expand,
              children: [
                Artwork(
                  path: card.poster,
                  width: double.infinity,
                  height: height ?? 190,
                  radius: 10,
                  fallback: card.isSeries ? Icons.tv : Icons.movie_outlined,
                ),
                DecoratedBox(
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(10),
                    border: Border.all(color: hue.withValues(alpha: 0.55)),
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(height: 6),
          Text(card.title,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 11, fontWeight: FontWeight.w700, color: t.nInk)),
          if (card.year.isNotEmpty)
            Text(card.year, style: TextStyle(fontSize: 10, color: t.nInk3)),
        ],
      ),
    );
  }
}

// --------------------------------------------------------------- results ----

class _Results extends StatelessWidget {
  const _Results({
    required this.controller,
    required this.stream,
    required this.query,
  });

  final VideosController controller;
  final StreamView stream;
  final TextEditingController query;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      children: [
        _Toolbar(controller: controller, stream: stream, query: query),
        if (stream.busy)
          const LinearProgressIndicator(minHeight: 2, value: null),
        if (stream.dlLabel.isNotEmpty)
          _DownloadStrip(controller: controller, stream: stream),
        Expanded(
          child: Row(
            children: [
              Expanded(
                child: ListView(
                  padding: const EdgeInsets.fromLTRB(44, 24, 24, 36),
                  children: [
                    Row(
                      children: [
                        const RailTitle('Results'),
                        const SizedBox(width: 10),
                        Text('${stream.results.length} titles',
                            style: TextStyle(fontSize: 12, color: t.nInk3)),
                      ],
                    ),
                    const SizedBox(height: 14),
                    Wrap(
                      spacing: 16,
                      runSpacing: 22,
                      children: [
                        for (final card in stream.results)
                          _ResultPoster(card: card, controller: controller),
                      ],
                    ),
                    if (stream.more) ...[
                      const SizedBox(height: 20),
                      Center(
                        child: PlexButton(
                          icon: Icons.expand_more,
                          label: stream.busy ? 'Loading…' : 'Load more',
                          onTap: () => controller
                              .send(const VideosCmd.streamSearchMore()),
                        ),
                      ),
                    ],
                  ],
                ),
              ),
              // The panel reserves its width so the grid never reflows.
              _InfoPreview(stream: stream),
            ],
          ),
        ),
      ],
    );
  }
}

class _Toolbar extends StatelessWidget {
  const _Toolbar({
    required this.controller,
    required this.stream,
    required this.query,
  });

  final VideosController controller;
  final StreamView stream;
  final TextEditingController query;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 66,
      color: t.panel,
      padding: const EdgeInsets.symmetric(horizontal: 28),
      child: Row(
        children: [
          PlexButton(
            icon: Icons.arrow_back,
            label: stream.detailOpen ? 'Results' : 'Back',
            hue: cBack,
            filled: true,
            compact: true,
            onTap: () {
              if (stream.detailOpen) {
                controller.send(const VideosCmd.streamBack());
              } else {
                query.clear();
                controller.send(const VideosCmd.streamSearch(query: ''));
              }
            },
          ),
          const SizedBox(width: 12),
          IconBtn(
            icon: Icons.home_outlined,
            tip: 'Back to the Stream home',
            hue: cInfo,
            onTap: () {
              query.clear();
              controller.send(const VideosCmd.streamHome());
            },
          ),
          const SizedBox(width: 12),
          VideoSearchField(
            controller: query,
            width: 420,
            hint: stream.source == 'fourk'
                ? 'Search Movies in 4K'
                : 'Search movies and series',
            onChanged: (q) =>
                controller.sendQuiet(VideosCmd.streamSuggest(query: q)),
            onSubmitted: (q) =>
                controller.send(VideosCmd.streamSearch(query: q)),
          ),
          const SizedBox(width: 12),
          PlexButton(
            icon: Icons.search,
            label: 'Search',
            filled: true,
            compact: true,
            onTap: () =>
                controller.send(VideosCmd.streamSearch(query: query.text)),
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Text(
              stream.status,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontSize: 12,
                fontWeight: FontWeight.w600,
                color: stream.busy ? Tokens.secVideos : t.nInk3,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// The header progress strip. It reads live ticks off the controller rather
/// than the snapshot, which only moves when a command is dispatched.
class _DownloadStrip extends StatelessWidget {
  const _DownloadStrip({required this.controller, required this.stream});

  final VideosController controller;
  final StreamView stream;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final active = controller.dlActive;
    return Container(
      height: 46,
      color: t.panel2,
      padding: const EdgeInsets.symmetric(horizontal: 28),
      child: Row(
        children: [
          Icon(Icons.download,
              size: 15, color: active ? Tokens.secVideos : t.nInk3),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(controller.dlLabel,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 11,
                        fontWeight: FontWeight.w600,
                        color: t.nInk2)),
                if (active) ...[
                  const SizedBox(height: 5),
                  ClipRRect(
                    borderRadius: BorderRadius.circular(2),
                    child: LinearProgressIndicator(
                      minHeight: 4,
                      value: controller.dlFrac.clamp(0.0, 1.0),
                      backgroundColor: t.nHair,
                      valueColor:
                          const AlwaysStoppedAnimation(Tokens.secVideos),
                    ),
                  ),
                ],
              ],
            ),
          ),
          const SizedBox(width: 12),
          if (active)
            VideoTab(
              label: 'Cancel',
              hue: cErr,
              onTap: () =>
                  controller.send(const VideosCmd.streamDownloadCancel()),
            )
          else
            IconBtn(
              icon: Icons.close,
              size: 28,
              onTap: () =>
                  controller.send(const VideosCmd.streamDownloadDismiss()),
            ),
        ],
      ),
    );
  }
}

class _ResultPoster extends StatelessWidget {
  const _ResultPoster({required this.card, required this.controller});

  final StreamCard card;
  final VideosController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      width: 150,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) =>
            controller.sendQuiet(VideosCmd.streamPreview(id: card.id)),
        onExit: (_) =>
            controller.sendQuiet(const VideosCmd.streamPreview(id: '')),
        child: GestureDetector(
          onTap: () => controller.send(VideosCmd.streamOpen(id: card.id)),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Stack(
                children: [
                  Artwork(
                    path: card.poster,
                    fallback: card.isSeries ? Icons.tv : Icons.movie_outlined,
                  ),
                  Positioned(
                    top: 6,
                    left: 6,
                    child: OverlayPill(
                      text: card.isSeries ? 'SERIES' : 'MOVIE',
                      background: card.isSeries ? cBadgeSeries : cBadgeMovie,
                    ),
                  ),
                  if (card.seasons > 0)
                    Positioned(
                      bottom: 8,
                      right: 6,
                      child: OverlayPill(
                          text: '${card.seasons} '
                              'season${card.seasons == 1 ? '' : 's'}'),
                    ),
                ],
              ),
              const SizedBox(height: 8),
              Text(card.title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 12,
                      fontWeight: FontWeight.w600,
                      color: t.nInk)),
              if (card.year.isNotEmpty)
                Text(card.year, style: TextStyle(fontSize: 10, color: t.nInk3)),
            ],
          ),
        ),
      ),
    );
  }
}

class _InfoPreview extends StatelessWidget {
  const _InfoPreview({required this.stream});

  final StreamView stream;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      width: 360,
      decoration: BoxDecoration(
        color: t.panel,
        border: Border(left: BorderSide(color: t.nHair)),
      ),
      child: ListView(
        padding: const EdgeInsets.all(22),
        children: [
          const Text('INFO PREVIEW',
              style: TextStyle(
                  fontSize: 10,
                  fontWeight: FontWeight.w800,
                  letterSpacing: 1.4,
                  color: Tokens.secVideos)),
          const SizedBox(height: 12),
          if (!stream.previewOpen)
            Text('Hover a title to see details.',
                style: TextStyle(fontSize: 12, color: t.nInk3))
          else ...[
            Artwork(
              path: stream.previewCover,
              width: double.infinity,
              height: 400,
              radius: 10,
              fallback:
                  stream.previewIsSeries ? Icons.tv : Icons.movie_outlined,
            ),
            const SizedBox(height: 12),
            OverlayPill(
              text: stream.previewIsSeries ? 'SERIES' : 'MOVIE',
              background: stream.previewIsSeries ? cBadgeSeries : cBadgeMovie,
            ),
            const SizedBox(height: 12),
            Text(stream.previewTitle,
                style: TextStyle(
                    fontSize: 20, fontWeight: FontWeight.w800, color: t.nInk)),
            if (stream.previewMeta.isNotEmpty) ...[
              const SizedBox(height: 8),
              Text(stream.previewMeta,
                  style: TextStyle(
                      fontSize: 12,
                      fontWeight: FontWeight.w600,
                      color: t.nInk2)),
            ],
            if (stream.previewSeasons > 0) ...[
              const SizedBox(height: 8),
              Text(
                '${stream.previewSeasons} '
                'season${stream.previewSeasons == 1 ? '' : 's'}',
                style: const TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w700,
                    color: Tokens.secVideos),
              ),
            ],
            if (stream.previewLangs.isNotEmpty) ...[
              const SizedBox(height: 12),
              const _Caption('AUDIO'),
              Text(stream.previewLangs,
                  style: TextStyle(fontSize: 12, color: t.nInk2)),
            ],
            if (stream.previewOverview.isNotEmpty) ...[
              const SizedBox(height: 12),
              const _Caption('SYNOPSIS'),
              Text(stream.previewOverview,
                  style: TextStyle(fontSize: 12, color: t.nInk2)),
            ],
          ],
        ],
      ),
    );
  }
}

class _Caption extends StatelessWidget {
  const _Caption(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.only(bottom: 4),
        child: Text(text,
            style: TextStyle(
                fontSize: 9,
                fontWeight: FontWeight.w800,
                letterSpacing: 1.2,
                color: context.tokens.nInk3)),
      );
}

// ---------------------------------------------------------------- detail ----

class _Detail extends StatelessWidget {
  const _Detail({required this.controller, required this.stream});

  final VideosController controller;
  final StreamView stream;

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        if (stream.busy)
          const LinearProgressIndicator(minHeight: 2, value: null),
        Expanded(
          child: Padding(
            padding: const EdgeInsets.fromLTRB(28, 18, 28, 22),
            child: Column(
              children: [
                _SubjectInfo(controller: controller, stream: stream),
                const SizedBox(height: 16),
                Expanded(
                    child: _Pickers(controller: controller, stream: stream)),
              ],
            ),
          ),
        ),
      ],
    );
  }
}

class _SubjectInfo extends StatelessWidget {
  const _SubjectInfo({required this.controller, required this.stream});

  final VideosController controller;
  final StreamView stream;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 278,
      padding: const EdgeInsets.all(18),
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: t.nHair),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Stack(
            children: [
              Artwork(
                path: stream.cover,
                width: 158,
                height: 230,
                fallback: stream.isSeries ? Icons.tv : Icons.movie_outlined,
              ),
              // Ringed twice, like the landing blocks: a violet edge with a
              // green one inside it, drawn over the art.
              Container(
                width: 158,
                height: 230,
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(
                      color: cInfo.withValues(alpha: 0.75), width: 2),
                ),
              ),
              Positioned(
                left: 5,
                top: 5,
                child: Container(
                  width: 148,
                  height: 220,
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(5),
                    border: Border.all(
                        color: cPlay.withValues(alpha: 0.60), width: 1.5),
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(width: 22),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Expanded(
                      child: Text(stream.title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 30,
                              fontWeight: FontWeight.w800,
                              color: t.nInk)),
                    ),
                    const SizedBox(width: 12),
                    IconBtn(
                      icon: stream.bookmarked
                          ? Icons.bookmark
                          : Icons.bookmark_outline,
                      tip: stream.bookmarked ? 'Bookmarked' : 'Bookmark',
                      hue: cSave,
                      filled: stream.bookmarked,
                      size: 40,
                      onTap: () => controller
                          .send(const VideosCmd.streamToggleBookmark()),
                    ),
                  ],
                ),
                if (stream.meta.isNotEmpty) ...[
                  const SizedBox(height: 8),
                  Text(stream.meta,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 14,
                          fontWeight: FontWeight.w600,
                          color: t.nInk2)),
                ],
                const SizedBox(height: 8),
                Expanded(
                  child: SingleChildScrollView(
                    child: Text(stream.overview,
                        style: TextStyle(fontSize: 14, color: t.nInk2)),
                  ),
                ),
                const SizedBox(height: 8),
                _Actions(controller: controller, stream: stream),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _Actions extends StatelessWidget {
  const _Actions({required this.controller, required this.stream});

  final VideosController controller;
  final StreamView stream;

  Future<void> _copy(BuildContext context, int index) async {
    // Resolving happens in Rust because a 4KHDHub row points at a landing page;
    // the copy itself is the platform's own clipboard.
    try {
      final url = await videosStreamLink(index: index);
      if (url.isEmpty) return;
      await Clipboard.setData(ClipboardData(text: url));
      if (!context.mounted) return;
      ScaffoldMessenger.of(context)
          .showSnackBar(const SnackBar(content: Text('Stream link copied.')));
    } catch (e) {
      if (!context.mounted) return;
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('Could not copy: $e')));
    }
  }

  @override
  Widget build(BuildContext context) {
    final armed = stream.current >= 0;
    return Row(
      children: [
        SizedBox(
          width: 240,
          child: PlexButton(
            icon: Icons.play_arrow,
            hue: cPlay,
            filled: true,
            label: stream.currentLabel.isNotEmpty
                ? 'Play ${stream.currentLabel}'
                : 'Play',
            enabled: armed,
            onTap: () => controller.send(const VideosCmd.streamPlayCurrent()),
          ),
        ),
        const SizedBox(width: 8),
        IconBtn(
          icon: Icons.link,
          tip: 'Copy link',
          hue: cCopy,
          onTap: () => armed ? _copy(context, stream.current) : null,
        ),
        const SizedBox(width: 8),
        IconBtn(
          icon: Icons.download,
          tip: 'Download',
          hue: cDl,
          onTap: () => controller.send(const VideosCmd.streamDownloadCurrent()),
        ),
        if (stream.isSeries) ...[
          const SizedBox(width: 8),
          IconBtn(
            icon: Icons.playlist_add,
            tip: 'Download season',
            hue: cDl,
            onTap: () =>
                controller.send(const VideosCmd.streamDownloadSeason()),
          ),
        ],
        const SizedBox(width: 8),
        IconBtn(
          icon: Icons.cast,
          tip: stream.castActive
              ? 'Casting to ${stream.castTarget}'
              : 'Cast to a device',
          hue: cCopy,
          filled: stream.castActive,
          onTap: () {
            if (stream.castActive) {
              controller.send(const VideosCmd.streamCastStop());
            } else {
              openCastPicker(context, controller);
            }
          },
        ),
        const SizedBox(width: 8),
        IconBtn(
          icon: Icons.theaters,
          tip: 'Watch the trailer',
          hue: cPlay,
          onTap: () => controller.send(const VideosCmd.streamTrailer()),
        ),
      ],
    );
  }
}

/// The four picker columns. A film gets two (Audio · Streams); a series gets
/// four (Audio+Seasons · Episodes · Quality+Subtitles · Streams).
class _Pickers extends StatelessWidget {
  const _Pickers({required this.controller, required this.stream});

  final VideosController controller;
  final StreamView stream;

  @override
  Widget build(BuildContext context) {
    final series = stream.isSeries;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SizedBox(
          width: 168,
          child: Column(
            children: [
              // On a series Audio is a fixed card with Seasons under it; on a
              // film it fills the column, because there is nothing to stack.
              if (series) SizedBox(height: 210, child: _audio()),
              if (!series) Expanded(child: _audio()),
              if (series) ...[
                const SizedBox(height: 14),
                Expanded(child: _seasons()),
              ],
            ],
          ),
        ),
        if (series) ...[
          const SizedBox(width: 14),
          SizedBox(width: 168, child: _episodes()),
        ],
        const SizedBox(width: 14),
        SizedBox(
          width: 168,
          child: Column(
            children: [
              SizedBox(height: 244, child: _resolution()),
              const SizedBox(height: 14),
              Expanded(child: _subtitles()),
            ],
          ),
        ),
        const SizedBox(width: 14),
        Expanded(child: _streams(context)),
      ],
    );
  }

  Widget _audio() => _Column(
        title: 'Audio',
        child: ListView(
          padding: const EdgeInsets.all(6),
          children: [
            if (stream.dubs.isEmpty)
              const PickRow(label: 'Original', active: true, onTap: null),
            for (final dub in stream.dubs)
              PickRow(
                label: dub.label,
                active: dub.active,
                onTap: () =>
                    controller.send(VideosCmd.streamSetDub(id: dub.id)),
              ),
          ],
        ),
      );

  Widget _seasons() => _Column(
        title: 'Seasons',
        child: ListView(
          padding: const EdgeInsets.all(6),
          children: [
            for (final n in stream.seasons.map((e) => e.toInt()))
              PickRow(
                label: 'Season $n',
                active: n == stream.season,
                onTap: () =>
                    controller.send(VideosCmd.streamSetSeason(season: n)),
              ),
          ],
        ),
      );

  Widget _episodes() => _Column(
        title: 'Episodes',
        hint: '${stream.episodes.length}',
        child: ListView(
          padding: const EdgeInsets.all(6),
          children: [
            // Named where the catalogue named it, numbered where it did not;
            // the bar is how far in you are.
            for (final ep in stream.episodes)
              _EpisodeRow(
                episode: ep,
                active: ep.number == stream.episode,
                onTap: () => controller
                    .send(VideosCmd.streamSetEpisode(episode: ep.number)),
              ),
          ],
        ),
      );

  Widget _resolution() => _Column(
        title: 'Resolution',
        child: ListView(
          padding: const EdgeInsets.all(6),
          children: [
            for (final r in const ['1080', '720', '480', '360'])
              PickRow(
                label: '${r}p',
                active: stream.resolution == r,
                onTap: () =>
                    controller.send(VideosCmd.streamSetResolution(res: r)),
              ),
            PickRow(
              label: 'All qualities',
              active: stream.resolution.isEmpty,
              onTap: () =>
                  controller.send(const VideosCmd.streamSetResolution(res: '')),
            ),
          ],
        ),
      );

  Widget _subtitles() => _Column(
        title: 'Subtitles',
        hint: '${stream.subs.length}',
        child: ListView(
          padding: const EdgeInsets.all(6),
          children: [
            PickRow(
              label: 'Off',
              active: stream.subChoice < 0,
              onTap: () =>
                  controller.send(const VideosCmd.streamSetSub(index: -1)),
            ),
            for (var i = 0; i < stream.subs.length; i++)
              PickRow(
                label: stream.subs[i].label,
                active: stream.subChoice == i,
                onTap: () => controller.send(VideosCmd.streamSetSub(index: i)),
              ),
            if (stream.subs.isEmpty)
              const Padding(
                padding: EdgeInsets.all(12),
                child: Text('None for this episode',
                    textAlign: TextAlign.center,
                    style: TextStyle(fontSize: 10)),
              ),
          ],
        ),
      );

  Widget _streams(BuildContext context) => _Column(
        title: 'Streams',
        hint: '${stream.files.length} available',
        child: stream.files.isEmpty
            ? Center(
                child: Padding(
                  padding: const EdgeInsets.all(20),
                  child: Text(
                    stream.busy
                        ? 'Finding streams…'
                        : 'No playable streams here — try another episode or '
                            'language.',
                    textAlign: TextAlign.center,
                    style: TextStyle(fontSize: 13, color: context.tokens.nInk3),
                  ),
                ),
              )
            : ListView.separated(
                padding: const EdgeInsets.all(8),
                itemCount: stream.files.length,
                separatorBuilder: (_, __) => const SizedBox(height: 8),
                itemBuilder: (_, i) => _FileRow(
                  file: stream.files[i],
                  current: stream.files[i].index == stream.current,
                  onSelect: () => controller.send(
                      VideosCmd.streamSetCurrent(index: stream.files[i].index)),
                  onPlay: () => controller
                      .send(VideosCmd.streamPlay(index: stream.files[i].index)),
                  onCopy: () => _copyLink(context, stream.files[i].index),
                  onDownload: () => controller.send(
                      VideosCmd.streamDownload(index: stream.files[i].index)),
                ),
              ),
      );

  Future<void> _copyLink(BuildContext context, int index) async {
    try {
      final url = await videosStreamLink(index: index);
      if (url.isEmpty) return;
      await Clipboard.setData(ClipboardData(text: url));
      if (!context.mounted) return;
      ScaffoldMessenger.of(context)
          .showSnackBar(const SnackBar(content: Text('Stream link copied.')));
    } catch (e) {
      if (!context.mounted) return;
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('Could not copy: $e')));
    }
  }
}

/// A titled picker column that fills its slot.
class _Column extends StatelessWidget {
  const _Column({required this.title, required this.child, this.hint});

  final String title;
  final String? hint;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        border: Border.all(color: t.nHair),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 12, 14, 6),
            child: Row(
              children: [
                Text(title,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w800,
                        color: t.nInk)),
                const Spacer(),
                if (hint != null)
                  Text(hint!, style: TextStyle(fontSize: 11, color: t.nInk3)),
              ],
            ),
          ),
          Expanded(child: child),
        ],
      ),
    );
  }
}

/// A row in one of the picker columns. `onTap` null makes it a static label —
/// what "Original" is when the catalogue lists no language cuts.
class PickRow extends StatelessWidget {
  const PickRow({
    super.key,
    required this.label,
    required this.active,
    required this.onTap,
  });

  final String label;
  final bool active;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(7),
      child: Container(
        height: 30,
        alignment: Alignment.centerLeft,
        padding: const EdgeInsets.symmetric(horizontal: 10),
        margin: const EdgeInsets.only(bottom: 2),
        decoration: BoxDecoration(
          color: active ? Tokens.secVideos : Colors.transparent,
          borderRadius: BorderRadius.circular(7),
        ),
        child: Text(
          label,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(
            fontSize: 12.5,
            fontWeight: active ? FontWeight.w700 : FontWeight.w500,
            color: active ? Colors.white : t.nInk,
          ),
        ),
      ),
    );
  }
}

class _EpisodeRow extends StatelessWidget {
  const _EpisodeRow({
    required this.episode,
    required this.active,
    required this.onTap,
  });

  final StreamEpisode episode;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(7),
      child: Container(
        height: 38,
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
        margin: const EdgeInsets.only(bottom: 2),
        decoration: BoxDecoration(
          color: active ? Tokens.secVideos : Colors.transparent,
          borderRadius: BorderRadius.circular(7),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Text(
              episode.title.isEmpty
                  ? 'Episode ${episode.number}'
                  : '${episode.number}. ${episode.title}',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontSize: 12,
                fontWeight: active ? FontWeight.w700 : FontWeight.w500,
                color: active ? Colors.white : t.nInk,
              ),
            ),
            if (episode.progress > 0) ...[
              const SizedBox(height: 4),
              ClipRRect(
                borderRadius: BorderRadius.circular(2),
                child: ProgressStrip(
                  value: episode.progress,
                  hue: active ? Colors.white : Tokens.secVideos,
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

class _FileRow extends StatelessWidget {
  const _FileRow({
    required this.file,
    required this.current,
    required this.onSelect,
    required this.onPlay,
    required this.onCopy,
    required this.onDownload,
  });

  final StreamFileRow file;
  final bool current;
  final VoidCallback onSelect;
  final VoidCallback onPlay;
  final VoidCallback onCopy;
  final VoidCallback onDownload;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onSelect,
      borderRadius: BorderRadius.circular(8),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        decoration: BoxDecoration(
          color: current ? Tokens.secVideos.withValues(alpha: 0.14) : t.panel2,
          borderRadius: BorderRadius.circular(8),
          border: Border.all(color: current ? Tokens.secVideos : t.nHair),
        ),
        child: Row(
          children: [
            SizedBox(
              width: 58,
              child: Text(file.label,
                  style: TextStyle(
                      fontSize: 12,
                      fontWeight: FontWeight.w800,
                      color: t.nInk)),
            ),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(file.sub,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 11, color: t.nInk2)),
                  if (file.uploader.isNotEmpty)
                    Text(file.uploader,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 10, color: t.nInk3)),
                ],
              ),
            ),
            SizedBox(
              width: 62,
              child: Text(
                file.subs,
                style: TextStyle(
                  fontSize: 10,
                  fontWeight: FontWeight.w700,
                  color: file.hasSubs ? cPlay : t.nInk3,
                ),
              ),
            ),
            IconBtn(
                icon: Icons.play_arrow,
                tip: 'Play',
                hue: cPlay,
                size: 30,
                filled: true,
                onTap: onPlay),
            const SizedBox(width: 6),
            IconBtn(
                icon: Icons.link,
                tip: 'Copy link',
                hue: cCopy,
                size: 30,
                onTap: onCopy),
            const SizedBox(width: 6),
            IconBtn(
                icon: Icons.download,
                tip: 'Download',
                hue: cDl,
                size: 30,
                onTap: onDownload),
          ],
        ),
      ),
    );
  }
}
