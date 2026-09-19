// The on-disk library: Movies, Shows and Local. docs/videos-deck.html.
//
// Three views of the same files, each shaped by what it holds:
//
//   Movies  a stage -- the backdrop of whatever poster is under the pointer --
//           then Continue and Recently added, then every movie behind filter
//           chips that are filters. A poster opens the movie's page.
//   Shows   the same stage for shows, Next up as episode stills, every show
//           with how many episodes are new; a show opens its page, seasons as
//           tabs and episodes as rows with their titles.
//   Local   personal video: no posters, no metadata, so frames, grouped by
//           month or by folder.
//
// Everything drawn here is in the snapshot or one lazy call away: posters
// (`videosEnsureThumb`), backdrops (`videosEnsureBackdrop`) and the movie
// page's file details (`videosMovieDetail`). Surfaces and controls go through
// the skin, so all four design languages draw it in their own material.

import 'dart:async';
import 'dart:io';
import 'dart:ui' show ImageFilter;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../platform/pick.dart';
import '../../shell/section_tabs.dart';
import '../../src/rust/api/videos.dart';
import 'videos_controller.dart';
import 'videos_widgets.dart';

const double _pad = 32;

class VideosLibrary extends StatelessWidget {
  const VideosLibrary({
    super.key,
    required this.controller,
    required this.state,
  });

  final VideosController controller;
  final VideosState state;

  @override
  Widget build(BuildContext context) {
    final movie = controller.openMovie;
    if (state.kind == 'movies' && movie != null) {
      return _MoviePage(controller: controller, state: state, tile: movie);
    }
    return switch (state.kind) {
      'tv' when state.showOpen =>
        _ShowPage(controller: controller, state: state),
      'tv' when state.category == 'library' =>
        _ShowsHome(controller: controller, state: state),
      'local' => _LocalView(controller: controller, state: state),
      _ => _MoviesHome(controller: controller, state: state),
    };
  }
}

/// The chooser is `pickDirectory`: the portal's dialog, opened through the
/// bridge the way the Slint build opens it. A cdylib has no window handle to
/// give it, so it is not parented to the app window — as in the Slint build.
Future<void> addVideoFolder(
    BuildContext context, VideosController controller) async {
  final path = await pickDirectory();
  if (path == null) return;
  await controller.send(VideosCmd.addFolder(path: path));
}

// ------------------------------------------------------------ the stage ----

/// What the stage is showing: a movie, or a show with its next episode.
class _Subject {
  _Subject.movie(VideoTile t, {required this.onPlay, required this.onOpen})
      : key = 'item:${t.itemId}',
        itemId = t.itemId,
        showId = 0,
        title = t.title,
        overview = t.overview,
        poster = t.thumb,
        backdrop = t.backdrop,
        progress = t.watched ? 0 : t.progress,
        kicker = t.progress > 0 && !t.watched
            ? 'Continue · ${_left(t)} left'
            : (t.watched ? 'Watched' : 'In your library'),
        meta = [
          if (t.year > 0) '${t.year}',
          if (t.runtimeMin > 0) _mins(t.runtimeMin),
        ],
        badges = [t.res, t.hdr, t.channels].where((b) => b.isNotEmpty).toList(),
        playLabel = t.progress > 0 && !t.watched ? 'Resume' : 'Play',
        openLabel = 'Details',
        starred = t.starred,
        tile = t;

  _Subject.show(ShowCard s, VideoTile? next,
      {required this.onPlay, required this.onOpen})
      : key = 'show:${s.id}',
        itemId = next?.itemId ?? 0,
        showId = s.id,
        title = s.title,
        overview = s.overview,
        poster = s.cover,
        backdrop = s.backdrop,
        progress = next?.progress ?? 0,
        kicker = next == null
            ? (s.unwatched == 0 ? 'Watched' : 'In your library')
            : '${(next.progress > 0) ? 'Continue' : 'Next up'} · '
                'S${next.season} E${next.episode}',
        meta = [
          if (s.year > 0) '${s.year}',
          '${s.count} episode${s.count == 1 ? '' : 's'}',
        ],
        badges = [
          if (s.unwatched > 0 && s.unwatched < s.count) '${s.unwatched} new'
        ],
        playLabel = next == null
            ? 'Play'
            : '${next.progress > 0 ? 'Resume' : 'Play'} S${next.season} E${next.episode}',
        openLabel = 'Episodes',
        starred = false,
        tile = null;

  final String key;
  final int itemId;
  final int showId;
  final String title;
  final String overview;
  final String poster;
  final String backdrop;
  final double progress;
  final String kicker;
  final List<String> meta;
  final List<String> badges;
  final String playLabel;
  final String openLabel;
  final bool starred;
  final VideoTile? tile;
  final VoidCallback onPlay;
  final VoidCallback onOpen;
}

String _mins(int m) => m >= 60 ? '${m ~/ 60}h ${m % 60}m' : '${m}m';

String _left(VideoTile t) {
  final total = t.runtimeMin > 0 ? t.runtimeMin : 0;
  if (total == 0) return '${(100 - t.progress * 100).round()}%';
  return _mins((total * (1 - t.progress)).round());
}

/// The backdrop that follows the pointer, with the title set large over it.
class _Stage extends StatelessWidget {
  const _Stage({
    required this.controller,
    required this.subject,
    this.onStar,
  });

  final VideosController controller;
  final _Subject subject;
  final VoidCallback? onStar;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = subject;
    final scrim = t.nCanvas;
    return SizedBox(
      height: 420,
      child: Stack(
        fit: StackFit.expand,
        children: [
          // A cut, not a flash: the old picture fades under the new one.
          AnimatedSwitcher(
            duration: const Duration(milliseconds: 420),
            layoutBuilder: (current, previous) => Stack(
              fit: StackFit.expand,
              children: [...previous, if (current != null) current],
            ),
            child: _Backdrop(
              key: ValueKey(s.key),
              controller: controller,
              itemId: s.itemId,
              showId: s.showId,
              known: s.backdrop,
              poster: s.poster,
            ),
          ),
          DecoratedBox(
            decoration: BoxDecoration(
              gradient: LinearGradient(
                colors: [
                  scrim,
                  scrim.withValues(alpha: 0.7),
                  scrim.withValues(alpha: 0)
                ],
                stops: const [0, 0.34, 0.72],
              ),
            ),
          ),
          DecoratedBox(
            decoration: BoxDecoration(
              gradient: LinearGradient(
                begin: Alignment.bottomCenter,
                end: Alignment.topCenter,
                colors: [scrim, scrim.withValues(alpha: 0)],
                stops: const [0, 0.55],
              ),
            ),
          ),
          Positioned(
            left: _pad,
            right: _pad,
            bottom: 30,
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 640),
              child: AnimatedSwitcher(
                duration: const Duration(milliseconds: 220),
                layoutBuilder: (current, previous) => Stack(
                  alignment: Alignment.bottomLeft,
                  children: [...previous, if (current != null) current],
                ),
                child: _StageCopy(
                    key: ValueKey(s.key), subject: s, onStar: onStar),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _StageCopy extends StatelessWidget {
  const _StageCopy({super.key, required this.subject, this.onStar});

  final _Subject subject;
  final VoidCallback? onStar;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = subject;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(s.kicker,
            style: const TextStyle(
                fontSize: 12.5,
                fontWeight: FontWeight.w700,
                color: Tokens.secVideos)),
        const SizedBox(height: 8),
        Text(
          s.title,
          maxLines: 2,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(
            fontSize: 52,
            height: 1.0,
            fontWeight: FontWeight.w800,
            letterSpacing: -1.6,
            color: t.nInk,
          ),
        ),
        const SizedBox(height: 14),
        _MetaRow(meta: s.meta, badges: s.badges),
        if (s.overview.isNotEmpty) ...[
          const SizedBox(height: 10),
          Text(
            s.overview,
            maxLines: 3,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(fontSize: 14, height: 1.5, color: t.nInk2),
          ),
        ],
        const SizedBox(height: 18),
        Wrap(
          spacing: 10,
          runSpacing: 10,
          children: [
            _PlayButton(label: s.playLabel, onTap: s.onPlay),
            _Ghost(label: s.openLabel, onTap: s.onOpen),
            if (onStar != null)
              _Ghost(
                icon: s.starred ? Icons.star : Icons.star_border,
                tip: s.starred ? 'Unstar' : 'Star',
                active: s.starred,
                onTap: onStar!,
              ),
          ],
        ),
        if (s.progress > 0) ...[
          const SizedBox(height: 16),
          SizedBox(
            width: 360,
            child: Row(children: [
              Expanded(child: _Bar(value: s.progress)),
              const SizedBox(width: 10),
              Text('${(s.progress * 100).round()}%',
                  style: TextStyle(fontSize: 12, color: t.nInk2)),
            ]),
          ),
        ],
      ],
    );
  }
}

/// A backdrop, fetched once per title; the poster, blurred, until then or
/// where TMDB has none.
class _Backdrop extends StatefulWidget {
  const _Backdrop({
    super.key,
    required this.controller,
    required this.itemId,
    required this.showId,
    required this.known,
    required this.poster,
  });

  final VideosController controller;
  final int itemId;
  final int showId;
  final String known;
  final String poster;

  @override
  State<_Backdrop> createState() => _BackdropState();
}

class _BackdropState extends State<_Backdrop> {
  String? _path;

  @override
  void initState() {
    super.initState();
    _path = widget.known.isNotEmpty
        ? widget.known
        : widget.controller.backdropCached(widget.itemId, widget.showId);
    if (_path == null && (widget.itemId > 0 || widget.showId > 0)) {
      widget.controller
          .backdropFor(widget.itemId, widget.showId)
          .then((p) => mounted && p != null ? setState(() => _path = p) : null);
    }
  }

  @override
  Widget build(BuildContext context) {
    final path = _path;
    if (path != null && path.isNotEmpty) return _Pic(path: path);
    if (widget.poster.isNotEmpty) {
      return ImageFiltered(
        imageFilter: ImageFilter.blur(sigmaX: 40, sigmaY: 40),
        child: _Pic(path: widget.poster),
      );
    }
    return DecoratedBox(
      decoration: BoxDecoration(
        gradient: RadialGradient(
          center: const Alignment(0.6, -0.2),
          radius: 1.1,
          colors: [
            Tokens.secVideos.withValues(alpha: 0.55),
            Tokens.secVideos.withValues(alpha: 0),
          ],
        ),
      ),
    );
  }
}

// -------------------------------------------------------------- Movies ----

class _MoviesHome extends StatefulWidget {
  const _MoviesHome({required this.controller, required this.state});

  final VideosController controller;
  final VideosState state;

  @override
  State<_MoviesHome> createState() => _MoviesHomeState();
}

class _MoviesHomeState extends State<_MoviesHome> {
  VideoTile? _focus;
  Timer? _settle;

  @override
  void dispose() {
    _settle?.cancel();
    super.dispose();
  }

  /// A short settle, so sweeping across the grid does not strobe the stage.
  void _hover(VideoTile t) {
    _settle?.cancel();
    _settle = Timer(const Duration(milliseconds: 160), () {
      if (mounted && _focus?.itemId != t.itemId) setState(() => _focus = t);
    });
  }

  @override
  Widget build(BuildContext context) {
    final st = widget.state;
    final c = widget.controller;
    final front = st.category == 'library' && st.query.isEmpty;
    final all = [...st.railContinue, ...st.railRecent, ...st.tiles];
    // The focus is re-read out of this snapshot, so starring it updates the
    // stage; a title that has left the library drops back to the default.
    VideoTile? focus;
    if (_focus != null) {
      for (final t in all) {
        if (t.itemId == _focus!.itemId) focus = t;
      }
    }
    focus ??= st.railContinue.isNotEmpty
        ? st.railContinue.first
        : (st.railRecent.isNotEmpty ? st.railRecent.first : null);
    void open(VideoTile t) => c.openMoviePage(t);

    if (st.itemCount == 0 && st.category == 'library' && st.query.isEmpty) {
      return VideosEmpty(
        icon: Icons.movie_creation_outlined,
        title: 'No movies yet',
        message: 'Add a folder of films. A file is a movie when its name has a '
            'year, like "Arrival (2016).mkv" — see Naming.',
        action: PlexButton(
          label: '+ Add folder',
          filled: true,
          hue: cPlay,
          onTap: () => addVideoFolder(context, c),
        ),
      );
    }

    return CustomScrollView(
      slivers: [
        if (front && focus != null)
          SliverToBoxAdapter(
            child: _Stage(
              controller: c,
              subject: _Subject.movie(
                focus,
                onPlay: () => c.send(VideosCmd.play(index: focus!.index)),
                onOpen: () => open(focus!),
              ),
              onStar: () => c.send(
                  VideosCmd.tileAction(index: focus!.index, action: 'star')),
            ),
          ),
        if (front && st.railContinue.isNotEmpty)
          _rail(
            'Continue watching',
            '${st.railContinue.length}',
            height: 212,
            children: [
              for (final t in st.railContinue)
                _Wide(
                  controller: c,
                  tile: t,
                  title: t.title,
                  sub: [if (t.year > 0) '${t.year}', t.res, t.hdr]
                      .where((x) => x.isNotEmpty)
                      .join(' · '),
                  corner: '${_left(t)} left',
                  onTap: () => c.send(VideosCmd.play(index: t.index)),
                  onHover: () => _hover(t),
                ),
            ],
          ),
        if (front && st.railRecent.isNotEmpty)
          _rail(
            'Recently added',
            '',
            height: 300,
            children: [
              for (final t in st.railRecent)
                _Poster(
                  controller: c,
                  tile: t,
                  category: st.category,
                  onOpen: () => open(t),
                  onHover: () => _hover(t),
                ),
            ],
          ),
        SliverToBoxAdapter(
          child: Padding(
            padding: const EdgeInsets.fromLTRB(_pad, 24, _pad, 14),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                _Heading(
                  st.query.isNotEmpty
                      ? 'Results for “${st.query}”'
                      : _categoryHeading(st.category, 'movies'),
                  '${st.itemCount}',
                ),
                const SizedBox(height: 12),
                _FilterRow(controller: c, state: st, withSort: true),
              ],
            ),
          ),
        ),
        _posterGrid(
          st.tiles,
          (t) => _Poster(
            controller: c,
            tile: t,
            category: st.category,
            onOpen: () => open(t),
            onHover: front ? () => _hover(t) : null,
          ),
          empty: _emptyFor(st.category),
        ),
        _more(c, st),
      ],
    );
  }
}

String _categoryHeading(String category, String what) => switch (category) {
      'unwatched' => 'Unwatched',
      'continue' => 'In progress',
      'starred' => 'Starred',
      '4k' => 'In 4K',
      'hdr' => 'In HDR',
      'archive' => 'Archived',
      'trash' => 'Trash',
      _ => 'All $what',
    };

String _emptyFor(String category) => switch (category) {
      'unwatched' => 'You have watched everything here.',
      'continue' =>
        'Nothing in progress. Press play on a title and it lands here.',
      'starred' => 'Star a title and it waits here.',
      '4k' => 'Nothing here is 4K.',
      'hdr' => 'Nothing here is HDR.',
      'archive' =>
        'Archived titles are hidden from the library, and kept here.',
      'trash' => 'Trash is empty.',
      _ => 'Nothing matches. Clear the filter or search for another title.',
    };

Widget _rail(String title, String note,
        {required double height, required List<Widget> children}) =>
    SliverToBoxAdapter(
      child: Padding(
        padding: const EdgeInsets.only(top: 22),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: _pad),
              child: _Heading(title, note),
            ),
            const SizedBox(height: 12),
            SizedBox(
              height: height,
              child: ListView.separated(
                scrollDirection: Axis.horizontal,
                padding: const EdgeInsets.symmetric(horizontal: _pad),
                itemCount: children.length,
                separatorBuilder: (_, __) => const SizedBox(width: 16),
                itemBuilder: (_, i) => children[i],
              ),
            ),
          ],
        ),
      ),
    );

Widget _posterGrid<T>(List<T> items, Widget Function(T) cell,
    {required String empty}) {
  if (items.isEmpty) {
    return SliverToBoxAdapter(child: _EmptyLine(empty));
  }
  return SliverPadding(
    padding: const EdgeInsets.fromLTRB(_pad, 0, _pad, 24),
    sliver: SliverGrid(
      gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(
        maxCrossAxisExtent: 172,
        mainAxisSpacing: 22,
        crossAxisSpacing: 18,
        // A 2:3 poster and two lines of label under it.
        childAspectRatio: 150 / 272,
      ),
      delegate: SliverChildBuilderDelegate(
        (context, i) => cell(items[i]),
        childCount: items.length,
      ),
    ),
  );
}

Widget _more(VideosController c, VideosState st) => SliverToBoxAdapter(
      child: st.moreCount > 0
          ? Padding(
              padding: const EdgeInsets.only(bottom: 32),
              child: Center(
                child: PlexButton(
                  label: 'Show ${st.moreCount} more',
                  hue: Tokens.secVideos,
                  onTap: () => c.send(const VideosCmd.showMore()),
                ),
              ),
            )
          : const SizedBox(height: 24),
    );

// ---------------------------------------------------------- movie page ----

class _MoviePage extends StatefulWidget {
  const _MoviePage(
      {required this.controller, required this.state, required this.tile});

  final VideosController controller;
  final VideosState state;
  final VideoTile tile;

  @override
  State<_MoviePage> createState() => _MoviePageState();
}

class _MoviePageState extends State<_MoviePage> {
  late final Future<MovieDetail> _detail =
      widget.controller.movieDetail(widget.tile.itemId);
  final FocusNode _keys = FocusNode();

  @override
  void dispose() {
    _keys.dispose();
    super.dispose();
  }

  /// The tile as the latest snapshot has it, so starring it shows at once.
  VideoTile get _tile {
    final st = widget.state;
    for (final t in [...st.tiles, ...st.railContinue, ...st.railRecent]) {
      if (t.itemId == widget.tile.itemId) return t;
    }
    return widget.tile;
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final m = _tile;
    void back() => c.openMoviePage(null);
    return Focus(
      focusNode: _keys,
      autofocus: true,
      onKeyEvent: (_, e) {
        if (e is KeyDownEvent && e.logicalKey == LogicalKeyboardKey.escape) {
          back();
          return KeyEventResult.handled;
        }
        return KeyEventResult.ignored;
      },
      child: CustomScrollView(
        slivers: [
          SliverToBoxAdapter(
            child: _DetailHead(
              controller: c,
              itemId: m.itemId,
              showId: 0,
              backdrop: m.backdrop,
              poster: m.thumb,
              posterItem: m.itemId,
              backLabel: 'Movies',
              onBack: back,
              title: m.title,
              meta: [
                if (m.year > 0) '${m.year}',
                if (m.runtimeMin > 0) _mins(m.runtimeMin),
                if (m.watched) 'Watched',
              ],
              badges: [m.res, m.hdr, m.channels]
                  .where((b) => b.isNotEmpty)
                  .toList(),
              overview: m.overview,
              actions: [
                _PlayButton(
                  label: m.progress > 0 && !m.watched ? 'Resume' : 'Play',
                  onTap: () => c.send(VideosCmd.play(index: m.index)),
                ),
                _Ghost(
                  icon: m.starred ? Icons.star : Icons.star_border,
                  tip: m.starred ? 'Unstar' : 'Star',
                  active: m.starred,
                  onTap: () => c.send(
                      VideosCmd.tileAction(index: m.index, action: 'star')),
                ),
                Builder(
                  builder: (ctx) => _Ghost(
                    icon: Icons.more_horiz,
                    tip: 'More',
                    onTapAt: (pos) =>
                        _tileMenu(ctx, c, m, widget.state.category, pos),
                  ),
                ),
              ],
            ),
          ),
          SliverToBoxAdapter(
            child: FutureBuilder<MovieDetail>(
              future: _detail,
              builder: (context, snap) {
                final d = snap.data;
                if (d == null) {
                  return const Padding(
                    padding: EdgeInsets.all(40),
                    child: Center(child: CircularProgressIndicator()),
                  );
                }
                final chapters = _Box(
                  title: 'Chapters',
                  note: '${d.chapters.length}',
                  child: Column(
                    children: [
                      for (final (i, ch) in d.chapters.indexed)
                        _ChapterRow(
                          first: i == 0,
                          title: ch.title,
                          at: _clock(ch.start),
                        ),
                    ],
                  ),
                );
                final file = _Box(
                  title: 'The file',
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      if (d.video.isNotEmpty) _Spec('Video', d.video),
                      if (d.audio.isNotEmpty) _Spec('Audio', d.audio),
                      _Spec(
                          'Subtitles',
                          d.subtitles.isEmpty
                              ? 'None beside the file'
                              : d.subtitles.join(', ')),
                      _Spec('Size', d.size),
                      if (d.container.isNotEmpty)
                        _Spec('Container', d.container),
                      _Spec('Location', d.path, mono: true),
                    ],
                  ),
                );
                return Padding(
                  padding: const EdgeInsets.fromLTRB(_pad, 22, _pad, 32),
                  child: LayoutBuilder(
                    builder: (context, box) =>
                        box.maxWidth < 760 || d.chapters.isEmpty
                            ? Column(children: [
                                if (d.chapters.isNotEmpty) ...[
                                  chapters,
                                  const SizedBox(height: 16)
                                ],
                                file,
                              ])
                            : Row(
                                crossAxisAlignment: CrossAxisAlignment.start,
                                children: [
                                  Expanded(flex: 13, child: chapters),
                                  const SizedBox(width: 18),
                                  Expanded(flex: 10, child: file),
                                ],
                              ),
                  ),
                );
              },
            ),
          ),
        ],
      ),
    );
  }
}

String _clock(double s) {
  final n = s.round();
  final h = n ~/ 3600, m = (n % 3600) ~/ 60, sec = n % 60;
  return h > 0
      ? '$h:${m.toString().padLeft(2, '0')}:${sec.toString().padLeft(2, '0')}'
      : '$m:${sec.toString().padLeft(2, '0')}';
}

/// The top of a movie's or a show's page: its backdrop full-bleed, the poster
/// over it, and what it is.
class _DetailHead extends StatelessWidget {
  const _DetailHead({
    required this.controller,
    required this.itemId,
    required this.showId,
    required this.backdrop,
    required this.poster,
    required this.posterItem,
    required this.backLabel,
    required this.onBack,
    required this.title,
    required this.meta,
    required this.badges,
    required this.overview,
    required this.actions,
  });

  final VideosController controller;
  final int itemId;
  final int showId;
  final String backdrop;
  final String poster;

  /// The item whose poster to render when [poster] is empty; 0 for none.
  final int posterItem;
  final String backLabel;
  final VoidCallback onBack;
  final String title;
  final List<String> meta;
  final List<String> badges;
  final String overview;
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final scrim = t.nCanvas;
    return SizedBox(
      height: 440,
      child: Stack(
        fit: StackFit.expand,
        children: [
          _Backdrop(
            controller: controller,
            itemId: itemId,
            showId: showId,
            known: backdrop,
            poster: poster,
          ),
          DecoratedBox(
            decoration: BoxDecoration(
              gradient: LinearGradient(
                begin: Alignment.bottomCenter,
                end: Alignment.topCenter,
                colors: [
                  scrim,
                  scrim.withValues(alpha: 0.55),
                  scrim.withValues(alpha: 0)
                ],
                stops: const [0.06, 0.5, 0.88],
              ),
            ),
          ),
          Positioned(
            left: 22,
            top: 18,
            child: _BackPill(label: backLabel, onTap: onBack),
          ),
          Positioned(
            left: _pad,
            right: _pad,
            bottom: 26,
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: [
                // The poster only where there is room for it beside the copy.
                if (MediaQuery.sizeOf(context).width >= 900)
                  Padding(
                    padding: const EdgeInsets.only(right: 28),
                    child: _PosterArt(
                      controller: controller,
                      path: poster,
                      itemId: posterItem,
                      width: 170,
                    ),
                  ),
                Expanded(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        title,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          fontSize: 44,
                          height: 1.0,
                          fontWeight: FontWeight.w800,
                          letterSpacing: -1.2,
                          color: t.nInk,
                        ),
                      ),
                      const SizedBox(height: 12),
                      _MetaRow(meta: meta, badges: badges),
                      if (overview.isNotEmpty) ...[
                        const SizedBox(height: 10),
                        ConstrainedBox(
                          constraints: const BoxConstraints(maxWidth: 680),
                          child: Text(
                            overview,
                            maxLines: 4,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontSize: 14, height: 1.5, color: t.nInk2),
                          ),
                        ),
                      ],
                      const SizedBox(height: 18),
                      Wrap(spacing: 10, runSpacing: 10, children: actions),
                    ],
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

class _ChapterRow extends StatelessWidget {
  const _ChapterRow(
      {required this.first, required this.title, required this.at});

  final bool first;
  final String title;
  final String at;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.symmetric(vertical: 9),
      decoration: BoxDecoration(
        border: first ? null : Border(top: BorderSide(color: t.nHair)),
      ),
      child: Row(children: [
        Expanded(
          child: Text(title,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 13, fontWeight: FontWeight.w600, color: t.nInk)),
        ),
        Text(at,
            style: TextStyle(
                fontSize: 12,
                color: t.nInk3,
                fontFeatures: const [FontFeature.tabularFigures()])),
      ]),
    );
  }
}

class _Spec extends StatelessWidget {
  const _Spec(this.label, this.value, {this.mono = false});

  final String label;
  final String value;
  final bool mono;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 5),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 92,
            child: Text(label, style: TextStyle(fontSize: 13, color: t.nInk3)),
          ),
          Expanded(
            child: SelectableText(
              value,
              style: TextStyle(
                fontSize: mono ? 12 : 13,
                fontFamily: mono ? 'monospace' : null,
                color: mono ? t.nInk2 : t.nInk,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

// --------------------------------------------------------------- Shows ----

enum _ShowFilter { all, watching, fresh, done }

class _ShowsHome extends StatefulWidget {
  const _ShowsHome({required this.controller, required this.state});

  final VideosController controller;
  final VideosState state;

  @override
  State<_ShowsHome> createState() => _ShowsHomeState();
}

class _ShowsHomeState extends State<_ShowsHome> {
  int? _focus;
  Timer? _settle;
  _ShowFilter _filter = _ShowFilter.all;

  @override
  void dispose() {
    _settle?.cancel();
    super.dispose();
  }

  void _hover(int showId) {
    _settle?.cancel();
    _settle = Timer(const Duration(milliseconds: 160), () {
      if (mounted && _focus != showId) setState(() => _focus = showId);
    });
  }

  /// A Next up tile names its show by title; the card is found the same way.
  ShowCard? _cardFor(VideoTile t) {
    for (final s in widget.state.shows) {
      if (s.title == t.title) return s;
    }
    return null;
  }

  VideoTile? _nextFor(ShowCard s) {
    for (final t in widget.state.nextUp) {
      if (t.title == s.title) return t;
    }
    return null;
  }

  @override
  Widget build(BuildContext context) {
    final st = widget.state;
    final c = widget.controller;
    if (st.shows.isEmpty && st.query.isEmpty) {
      return const VideosEmpty(
        icon: Icons.tv_outlined,
        title: 'No shows yet',
        message: 'Episodes group here when their names carry the season and '
            'episode, like "Show/Season 01/Show - S01E01.mkv" — see Naming.',
      );
    }
    ShowCard? focus;
    for (final s in st.shows) {
      if (s.id == _focus) focus = s;
    }
    focus ??= st.nextUp.isNotEmpty ? _cardFor(st.nextUp.first) : null;
    focus ??= st.shows.isNotEmpty ? st.shows.first : null;
    final shows = st.shows
        .where((s) => switch (_filter) {
              _ShowFilter.all => true,
              _ShowFilter.watching => s.unwatched > 0 && s.unwatched < s.count,
              _ShowFilter.fresh => s.unwatched == s.count,
              _ShowFilter.done => s.unwatched == 0,
            })
        .toList();
    void open(ShowCard s) => c.send(VideosCmd.openShow(showId: s.id));

    return CustomScrollView(
      slivers: [
        if (focus != null && st.query.isEmpty)
          SliverToBoxAdapter(
            child: Builder(builder: (context) {
              final f = focus!;
              final next = _nextFor(f);
              return _Stage(
                controller: c,
                subject: _Subject.show(
                  f,
                  next,
                  onPlay: () => next != null
                      ? c.send(VideosCmd.play(index: next.index))
                      : open(f),
                  onOpen: () => open(f),
                ),
              );
            }),
          ),
        if (st.nextUp.isNotEmpty && st.query.isEmpty)
          _rail(
            'Next up',
            '${st.nextUp.length}',
            height: 212,
            children: [
              for (final t in st.nextUp)
                _Wide(
                  controller: c,
                  tile: t,
                  title: t.title,
                  sub: 'S${t.season} · E${t.episode}'
                      '${t.epTitle.isNotEmpty ? '  ${t.epTitle}' : ''}',
                  corner: t.progress > 0
                      ? '${_left(t)} left'
                      : (t.runtimeMin > 0 ? '${t.runtimeMin}m' : ''),
                  onTap: () => c.send(VideosCmd.play(index: t.index)),
                  onHover: () {
                    final card = _cardFor(t);
                    if (card != null) _hover(card.id);
                  },
                ),
            ],
          ),
        SliverToBoxAdapter(
          child: Padding(
            padding: const EdgeInsets.fromLTRB(_pad, 24, _pad, 14),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                _Heading(
                  st.query.isNotEmpty
                      ? 'Results for “${st.query}”'
                      : 'Your shows',
                  '${shows.length}',
                ),
                const SizedBox(height: 12),
                _FilterRow(
                  controller: c,
                  state: st,
                  withSort: false,
                  leading: [
                    for (final (f, label) in const [
                      (_ShowFilter.all, 'All'),
                      (_ShowFilter.watching, 'Watching'),
                      (_ShowFilter.fresh, 'Not started'),
                      (_ShowFilter.done, 'Finished'),
                    ])
                      _Chip(
                        label: label,
                        active: _filter == f,
                        onTap: () => setState(() => _filter = f),
                      ),
                  ],
                ),
              ],
            ),
          ),
        ),
        _posterGrid(
          shows,
          (s) => _ShowPoster(
            controller: c,
            show: s,
            onOpen: () => open(s),
            onHover: () => _hover(s.id),
          ),
          empty: 'No shows here.',
        ),
        const SliverToBoxAdapter(child: SizedBox(height: 24)),
      ],
    );
  }
}

class _ShowPage extends StatefulWidget {
  const _ShowPage({required this.controller, required this.state});

  final VideosController controller;
  final VideosState state;

  @override
  State<_ShowPage> createState() => _ShowPageState();
}

class _ShowPageState extends State<_ShowPage> {
  int? _season;
  final FocusNode _keys = FocusNode();

  @override
  void dispose() {
    _keys.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final st = widget.state;
    final info = st.showInfo;
    final seasons = st.seasons;
    // The first unwatched episode, in order: what Play means on a show.
    VideoTile? next;
    for (final se in seasons) {
      for (final e in se.tiles) {
        if (next == null && !e.watched) next = e;
      }
    }
    final pick = _season ??
        (next != null
            ? next.season
            : (seasons.isNotEmpty ? seasons.first.number : 0));
    VideoSeason? season;
    for (final se in seasons) {
      if (se.number == pick) season = se;
    }
    season ??= seasons.isNotEmpty ? seasons.first : null;
    void back() => c.send(const VideosCmd.showBack());

    return Focus(
      focusNode: _keys,
      autofocus: true,
      onKeyEvent: (_, e) {
        if (e is KeyDownEvent && e.logicalKey == LogicalKeyboardKey.escape) {
          back();
          return KeyEventResult.handled;
        }
        return KeyEventResult.ignored;
      },
      child: CustomScrollView(
        slivers: [
          SliverToBoxAdapter(
            child: _DetailHead(
              controller: c,
              itemId: 0,
              showId: info.id,
              backdrop: info.backdrop,
              poster: info.cover,
              posterItem: 0,
              backLabel: 'Shows',
              onBack: back,
              title: info.title.isNotEmpty ? info.title : st.showTitle,
              meta: [
                if (info.year > 0) '${info.year}',
                '${seasons.length} season${seasons.length == 1 ? '' : 's'}',
                '${info.count} episodes',
                if (info.unwatched == 0 && info.count > 0) 'Watched',
              ],
              badges: [if (info.unwatched > 0) '${info.unwatched} unwatched'],
              overview: info.overview,
              actions: [
                _PlayButton(
                  label: next == null
                      ? 'Play from the start'
                      : '${next.progress > 0 ? 'Resume' : 'Play'} '
                          'S${next.season} E${next.episode}',
                  onTap: () {
                    final target = next ??
                        (seasons.isNotEmpty && seasons.first.tiles.isNotEmpty
                            ? seasons.first.tiles.first
                            : null);
                    if (target != null) {
                      c.send(VideosCmd.play(index: target.index));
                    }
                  },
                ),
              ],
            ),
          ),
          if (seasons.length > 1)
            SliverToBoxAdapter(
              child: Padding(
                padding: const EdgeInsets.fromLTRB(_pad, 20, _pad, 4),
                child: Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    for (final se in seasons)
                      _Chip(
                        label: se.label,
                        count: se.tiles.length,
                        active: se.number == season?.number,
                        onTap: () => setState(() => _season = se.number),
                      ),
                  ],
                ),
              ),
            ),
          if (season == null)
            const SliverToBoxAdapter(
                child: _EmptyLine('This show has no episodes in the library.'))
          else
            SliverPadding(
              padding: const EdgeInsets.fromLTRB(_pad - 10, 10, _pad - 10, 32),
              sliver: SliverList.builder(
                itemCount: season.tiles.length,
                itemBuilder: (context, i) => _EpisodeRow(
                  controller: c,
                  tile: season!.tiles[i],
                  category: st.category,
                ),
              ),
            ),
        ],
      ),
    );
  }
}

class _EpisodeRow extends StatefulWidget {
  const _EpisodeRow(
      {required this.controller, required this.tile, required this.category});

  final VideosController controller;
  final VideoTile tile;
  final String category;

  @override
  State<_EpisodeRow> createState() => _EpisodeRowState();
}

class _EpisodeRowState extends State<_EpisodeRow> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final e = widget.tile;
    final wide = MediaQuery.sizeOf(context).width > 900;
    final title = e.epTitle.isNotEmpty ? e.epTitle : 'Episode ${e.episode}';
    final air = e.airDate > 0
        ? _date(
            DateTime.fromMillisecondsSinceEpoch(e.airDate * 1000, isUtc: true))
        : '';
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: () => widget.controller.send(VideosCmd.play(index: e.index)),
        onSecondaryTapDown: (d) => _tileMenu(
            context, widget.controller, e, widget.category, d.globalPosition),
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 140),
          padding: const EdgeInsets.all(10),
          decoration: BoxDecoration(
            color: _hover ? t.nHover : Colors.transparent,
            borderRadius: BorderRadius.circular(14),
          ),
          child: Row(
            children: [
              SizedBox(
                width: wide ? 200 : 132,
                child: AspectRatio(
                  aspectRatio: 16 / 9,
                  child: ClipRRect(
                    borderRadius: BorderRadius.circular(10),
                    child: Stack(
                      fit: StackFit.expand,
                      children: [
                        _TileArt(
                            controller: widget.controller, tile: e, wide: true),
                        if (e.progress > 0 && !e.watched)
                          Positioned(
                            left: 0,
                            right: 0,
                            bottom: 0,
                            child: ProgressStrip(value: e.progress),
                          ),
                        if (_hover) const _PlayOverlay(),
                      ],
                    ),
                  ),
                ),
              ),
              const SizedBox(width: 18),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text.rich(
                      TextSpan(children: [
                        TextSpan(
                          text: '${e.episode}   ',
                          style: TextStyle(color: t.nInk3, fontFeatures: const [
                            FontFeature.tabularFigures()
                          ]),
                        ),
                        TextSpan(text: title),
                      ]),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 14.5,
                          fontWeight: FontWeight.w700,
                          color: t.nInk),
                    ),
                    const SizedBox(height: 5),
                    Text(
                      e.overview.isNotEmpty
                          ? e.overview
                          : (e.epTitle.isEmpty
                              ? 'Details arrive once TMDB matches this episode.'
                              : ''),
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style:
                          TextStyle(fontSize: 13, height: 1.4, color: t.nInk2),
                    ),
                  ],
                ),
              ),
              if (wide) ...[
                const SizedBox(width: 16),
                Column(
                  crossAxisAlignment: CrossAxisAlignment.end,
                  children: [
                    if (e.runtimeMin > 0)
                      Text('${e.runtimeMin}m',
                          style: TextStyle(fontSize: 12, color: t.nInk3)),
                    if (air.isNotEmpty)
                      Text(air, style: TextStyle(fontSize: 12, color: t.nInk3)),
                    if (e.watched)
                      Row(mainAxisSize: MainAxisSize.min, children: [
                        const Icon(Icons.check, size: 13, color: Tokens.ok),
                        const SizedBox(width: 4),
                        Text('Watched',
                            style: TextStyle(fontSize: 12, color: t.nInk3)),
                      ])
                    else if (e.progress > 0)
                      Text('${_left(e)} left',
                          style: TextStyle(fontSize: 12, color: t.nInk3)),
                  ],
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

const _months = [
  'January',
  'February',
  'March',
  'April',
  'May',
  'June',
  'July',
  'August',
  'September',
  'October',
  'November',
  'December',
];

String _date(DateTime d) =>
    '${_months[d.month - 1].substring(0, 3)} ${d.day}, ${d.year}';

// --------------------------------------------------------------- Local ----

class _LocalView extends StatefulWidget {
  const _LocalView({required this.controller, required this.state});

  final VideosController controller;
  final VideosState state;

  @override
  State<_LocalView> createState() => _LocalViewState();
}

class _LocalViewState extends State<_LocalView> {
  bool _byFolder = false;

  String _groupOf(VideoTile v) {
    if (_byFolder) {
      final parts = v.path.split(Platform.pathSeparator);
      return parts.length > 1 ? parts[parts.length - 2] : '';
    }
    final d = DateTime.fromMillisecondsSinceEpoch(v.mtime * 1000);
    return '${_months[d.month - 1]} ${d.year}';
  }

  @override
  Widget build(BuildContext context) {
    final st = widget.state;
    final c = widget.controller;
    if (st.itemCount == 0 && st.category == 'library' && st.query.isEmpty) {
      return VideosEmpty(
        icon: Icons.video_library_outlined,
        title: 'No personal videos',
        message: 'Clips from your phone, camera or screen recorder land here: '
            'anything that is not named like a movie or an episode.',
        action: PlexButton(
          label: '+ Add folder',
          filled: true,
          hue: cPlay,
          onTap: () => addVideoFolder(context, c),
        ),
      );
    }
    // Grouped in the order the snapshot gives them, which is newest first.
    final groups = <(String, List<VideoTile>)>[];
    for (final v in st.tiles) {
      final k = _groupOf(v);
      if (groups.isEmpty || groups.last.$1 != k) {
        final existing = groups.indexWhere((g) => g.$1 == k);
        if (existing >= 0) {
          groups[existing].$2.add(v);
          continue;
        }
        groups.add((k, [v]));
      } else {
        groups.last.$2.add(v);
      }
    }
    return CustomScrollView(
      slivers: [
        SliverToBoxAdapter(
          child: Padding(
            padding: const EdgeInsets.fromLTRB(_pad, 22, _pad, 8),
            child: _FilterRow(
              controller: c,
              state: st,
              withSort: false,
              leading: [
                _Chip(
                  icon: Icons.calendar_month_outlined,
                  label: 'By month',
                  active: !_byFolder,
                  onTap: () => setState(() => _byFolder = false),
                ),
                _Chip(
                  icon: Icons.folder_outlined,
                  label: 'By folder',
                  active: _byFolder,
                  onTap: () => setState(() => _byFolder = true),
                ),
                const SizedBox(width: 10),
              ],
            ),
          ),
        ),
        if (groups.isEmpty)
          SliverToBoxAdapter(child: _EmptyLine(_emptyFor(st.category))),
        for (final (label, clips) in groups) ...[
          SliverToBoxAdapter(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(_pad, 14, _pad, 12),
              child: _Heading(label.isEmpty ? 'Unsorted' : label,
                  '${clips.length} video${clips.length == 1 ? '' : 's'}',
                  size: 15),
            ),
          ),
          SliverPadding(
            padding: const EdgeInsets.symmetric(horizontal: _pad),
            sliver: SliverGrid(
              gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(
                maxCrossAxisExtent: 280,
                mainAxisSpacing: 18,
                crossAxisSpacing: 16,
                childAspectRatio: 16 / 12.6,
              ),
              delegate: SliverChildBuilderDelegate(
                (context, i) => _Clip(
                  controller: c,
                  tile: clips[i],
                  category: st.category,
                  sub: _byFolder
                      ? _date(DateTime.fromMillisecondsSinceEpoch(
                          clips[i].mtime * 1000))
                      : _folderOf(clips[i]),
                ),
                childCount: clips.length,
              ),
            ),
          ),
        ],
        _more(c, st),
      ],
    );
  }

  String _folderOf(VideoTile v) {
    final parts = v.path.split(Platform.pathSeparator);
    return parts.length > 1 ? parts[parts.length - 2] : '';
  }
}

class _Clip extends StatefulWidget {
  const _Clip({
    required this.controller,
    required this.tile,
    required this.category,
    required this.sub,
  });

  final VideosController controller;
  final VideoTile tile;
  final String category;
  final String sub;

  @override
  State<_Clip> createState() => _ClipState();
}

class _ClipState extends State<_Clip> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final v = widget.tile;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: () => widget.controller.send(VideosCmd.play(index: v.index)),
        onSecondaryTapDown: (d) => _tileMenu(
            context, widget.controller, v, widget.category, d.globalPosition),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            AspectRatio(
              aspectRatio: 16 / 9,
              child: _Framed(
                hover: _hover,
                child: Stack(
                  fit: StackFit.expand,
                  children: [
                    _TileArt(
                        controller: widget.controller, tile: v, wide: true),
                    if (v.res == '4K')
                      const Positioned(top: 8, left: 8, child: _Tag('4K')),
                    if (v.duration.isNotEmpty)
                      Positioned(right: 8, bottom: 8, child: _Tag(v.duration)),
                    if (v.progress > 0 && !v.watched)
                      Positioned(
                        left: 0,
                        right: 0,
                        bottom: 0,
                        child: ProgressStrip(value: v.progress),
                      ),
                    if (_hover) const _PlayOverlay(),
                  ],
                ),
              ),
            ),
            const SizedBox(height: 8),
            Text(
              v.title,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 13, fontWeight: FontWeight.w600, color: t.nInk),
            ),
            Text(
              [widget.sub, v.res].where((x) => x.isNotEmpty).join(' · '),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(fontSize: 12, color: t.nInk3),
            ),
          ],
        ),
      ),
    );
  }
}

// ------------------------------------------------------------ the cards ----

/// A movie poster: art, badges, the hover that lifts it, and its menu.
class _Poster extends StatefulWidget {
  const _Poster({
    required this.controller,
    required this.tile,
    required this.category,
    required this.onOpen,
    this.onHover,
  });

  final VideosController controller;
  final VideoTile tile;
  final String category;
  final VoidCallback onOpen;
  final VoidCallback? onHover;

  @override
  State<_Poster> createState() => _PosterState();
}

class _PosterState extends State<_Poster> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final m = widget.tile;
    final sub = [
      if (m.year > 0) '${m.year}',
      if (m.runtimeMin > 0) _mins(m.runtimeMin),
    ].join(' · ');
    return SizedBox(
      width: 150,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) {
          setState(() => _hover = true);
          widget.onHover?.call();
        },
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTap: widget.onOpen,
          onSecondaryTapDown: (d) => _tileMenu(
              context, widget.controller, m, widget.category, d.globalPosition),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              AspectRatio(
                aspectRatio: 2 / 3,
                child: _Framed(
                  hover: _hover,
                  child: Stack(
                    fit: StackFit.expand,
                    children: [
                      _TileArt(
                          controller: widget.controller, tile: m, wide: false),
                      if (m.res == '4K')
                        Positioned(
                            top: 8,
                            left: 8,
                            child: _Tag(m.hdr.isNotEmpty ? '4K HDR' : '4K')),
                      if (m.watched)
                        const Positioned(
                          top: 8,
                          right: 8,
                          child: _RoundMark(icon: Icons.check),
                        ),
                      if (m.progress > 0 && !m.watched)
                        Positioned(
                          left: 0,
                          right: 0,
                          bottom: 0,
                          child: ProgressStrip(value: m.progress),
                        ),
                      if (_hover)
                        Positioned(
                          left: 10,
                          bottom: 12,
                          child: GestureDetector(
                            onTap: () => widget.controller
                                .send(VideosCmd.play(index: m.index)),
                            child: const _PlayDisc(),
                          ),
                        ),
                      if (_hover || m.starred)
                        Positioned(
                          right: 10,
                          bottom: 14,
                          child: GestureDetector(
                            onTap: () => widget.controller.send(
                                VideosCmd.tileAction(
                                    index: m.index, action: 'star')),
                            child: Icon(
                              m.starred ? Icons.star : Icons.star_border,
                              size: 20,
                              color: m.starred
                                  ? const Color(0xFFFBBF24)
                                  : Colors.white,
                            ),
                          ),
                        ),
                    ],
                  ),
                ),
              ),
              const SizedBox(height: 9),
              Text(m.title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w600,
                      color: t.nInk)),
              if (sub.isNotEmpty)
                Text(sub,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 12, color: t.nInk3)),
            ],
          ),
        ),
      ),
    );
  }
}

class _ShowPoster extends StatefulWidget {
  const _ShowPoster({
    required this.controller,
    required this.show,
    required this.onOpen,
    required this.onHover,
  });

  final VideosController controller;
  final ShowCard show;
  final VoidCallback onOpen;
  final VoidCallback onHover;

  @override
  State<_ShowPoster> createState() => _ShowPosterState();
}

class _ShowPosterState extends State<_ShowPoster> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = widget.show;
    final started = s.unwatched < s.count;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) {
        setState(() => _hover = true);
        widget.onHover();
      },
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onOpen,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            AspectRatio(
              aspectRatio: 2 / 3,
              child: _Framed(
                hover: _hover,
                child: Stack(
                  fit: StackFit.expand,
                  children: [
                    s.cover.isNotEmpty
                        ? _Pic(path: s.cover)
                        : ColoredBox(
                            color: t.nTile,
                            child: Icon(Icons.tv, color: t.nInk3, size: 36)),
                    if (s.unwatched == 0)
                      const Positioned(
                          top: 8,
                          right: 8,
                          child: _RoundMark(icon: Icons.check))
                    else if (started)
                      Positioned(
                        top: 8,
                        right: 8,
                        child: _Tag('${s.unwatched} new', accent: true),
                      ),
                    if (_hover) const _PlayOverlay(icon: Icons.list),
                  ],
                ),
              ),
            ),
            const SizedBox(height: 9),
            Text(s.title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 13, fontWeight: FontWeight.w600, color: t.nInk)),
            Text(
              [if (s.year > 0) '${s.year}', '${s.count} episodes'].join(' · '),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(fontSize: 12, color: t.nInk3),
            ),
          ],
        ),
      ),
    );
  }
}

/// A 16:9 card for the rails: Continue watching, Next up.
class _Wide extends StatefulWidget {
  const _Wide({
    required this.controller,
    required this.tile,
    required this.title,
    required this.sub,
    required this.corner,
    required this.onTap,
    required this.onHover,
  });

  final VideosController controller;
  final VideoTile tile;
  final String title;
  final String sub;
  final String corner;
  final VoidCallback onTap;
  final VoidCallback onHover;

  @override
  State<_Wide> createState() => _WideState();
}

class _WideState extends State<_Wide> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final v = widget.tile;
    return SizedBox(
      width: 300,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) {
          setState(() => _hover = true);
          widget.onHover();
        },
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              AspectRatio(
                aspectRatio: 16 / 9,
                child: _Framed(
                  hover: _hover,
                  child: Stack(
                    fit: StackFit.expand,
                    children: [
                      _TileArt(
                          controller: widget.controller, tile: v, wide: true),
                      if (widget.corner.isNotEmpty)
                        Positioned(
                            right: 8, bottom: 12, child: _Tag(widget.corner)),
                      if (v.progress > 0)
                        Positioned(
                          left: 10,
                          right: 10,
                          bottom: 6,
                          child: _Bar(value: v.progress, light: true),
                        ),
                      if (_hover) const _PlayOverlay(),
                    ],
                  ),
                ),
              ),
              const SizedBox(height: 9),
              Text(widget.title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w600,
                      color: t.nInk)),
              Text(widget.sub,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 12, color: t.nInk3)),
            ],
          ),
        ),
      ),
    );
  }
}

/// Rounded, lifted and ringed on hover: every card's frame.
class _Framed extends StatelessWidget {
  const _Framed({required this.hover, required this.child});

  final bool hover;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final radius = context.skin.isStandard ? 10.0 : 14.0;
    return AnimatedSlide(
      duration: const Duration(milliseconds: 160),
      offset: Offset(0, hover ? -0.015 : 0),
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 160),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(radius),
          boxShadow: [
            if (hover)
              const BoxShadow(color: Tokens.secVideos, spreadRadius: 2),
            BoxShadow(
              color: const Color(0x66000000),
              blurRadius: hover ? 26 : 16,
              offset: const Offset(0, 10),
              spreadRadius: -10,
            ),
          ],
        ),
        child: ClipRRect(
          borderRadius: BorderRadius.circular(radius),
          child: child,
        ),
      ),
    );
  }
}

/// The best picture a tile has: for a wide card an episode still or a
/// backdrop, else its poster, else a frame rendered from the file.
class _TileArt extends StatefulWidget {
  const _TileArt(
      {required this.controller, required this.tile, required this.wide});

  final VideosController controller;
  final VideoTile tile;
  final bool wide;

  @override
  State<_TileArt> createState() => _TileArtState();
}

class _TileArtState extends State<_TileArt> {
  String? _path;

  @override
  void initState() {
    super.initState();
    _resolve();
  }

  @override
  void didUpdateWidget(_TileArt old) {
    super.didUpdateWidget(old);
    if (old.tile.itemId != widget.tile.itemId) {
      _path = null;
      _resolve();
    }
  }

  Future<void> _resolve() async {
    final v = widget.tile;
    final c = widget.controller;
    if (widget.wide && (v.still.isNotEmpty || v.backdrop.isNotEmpty)) {
      setState(() => _path = v.still.isNotEmpty ? v.still : v.backdrop);
      return;
    }
    if (v.thumb.isNotEmpty) {
      setState(() => _path = v.thumb);
    } else {
      final p = await c.thumbFor(v.itemId);
      // Cells are recycled while renders are in flight; a late result belongs
      // to whatever the cell holds now.
      if (mounted && p != null && widget.tile.itemId == v.itemId) {
        setState(() => _path = p);
      }
    }
    // A movie's wide card upgrades to its backdrop when TMDB has one.
    if (widget.wide && v.season == 0 && v.episode == 0 && v.still.isEmpty) {
      final b = await c.backdropFor(v.itemId, 0);
      if (mounted && b != null && widget.tile.itemId == v.itemId) {
        setState(() => _path = b);
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final p = _path;
    if (p == null || p.isEmpty) {
      return ColoredBox(
        color: context.tokens.nTile,
        child:
            Icon(Icons.movie_outlined, color: context.tokens.nInk3, size: 30),
      );
    }
    return _Pic(path: p);
  }
}

class _PosterArt extends StatelessWidget {
  const _PosterArt({
    required this.controller,
    required this.path,
    required this.itemId,
    required this.width,
  });

  final VideosController controller;
  final String path;
  final int itemId;
  final double width;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: width,
      child: AspectRatio(
        aspectRatio: 2 / 3,
        child: _Framed(
          hover: false,
          child: path.isNotEmpty || itemId == 0
              ? (path.isNotEmpty
                  ? _Pic(path: path)
                  : ColoredBox(color: context.tokens.nTile))
              : FutureBuilder<String?>(
                  future: controller.thumbFor(itemId),
                  builder: (context, snap) => snap.data == null
                      ? ColoredBox(color: context.tokens.nTile)
                      : _Pic(path: snap.data!),
                ),
        ),
      ),
    );
  }
}

class _Pic extends StatelessWidget {
  const _Pic({required this.path});

  final String path;

  @override
  Widget build(BuildContext context) => Image.file(
        File(path),
        fit: BoxFit.cover,
        gaplessPlayback: true,
        // A picture deleted under us is a missing picture, not a crash.
        errorBuilder: (_, __, ___) => ColoredBox(color: context.tokens.nTile),
      );
}

class _PlayOverlay extends StatelessWidget {
  const _PlayOverlay({this.icon = Icons.play_arrow});

  final IconData icon;

  @override
  Widget build(BuildContext context) => ColoredBox(
        color: const Color(0x40000000),
        child: Center(
          child: Container(
            width: 46,
            height: 46,
            decoration: const BoxDecoration(
                color: Color(0xF2FFFFFF), shape: BoxShape.circle),
            child: Icon(icon, color: const Color(0xFF111111)),
          ),
        ),
      );
}

class _PlayDisc extends StatelessWidget {
  const _PlayDisc();

  @override
  Widget build(BuildContext context) => Container(
        width: 38,
        height: 38,
        decoration:
            const BoxDecoration(color: Colors.white, shape: BoxShape.circle),
        child: const Icon(Icons.play_arrow, color: Color(0xFF111111), size: 22),
      );
}

class _Tag extends StatelessWidget {
  const _Tag(this.text, {this.accent = false});

  final String text;
  final bool accent;

  @override
  Widget build(BuildContext context) => Container(
        padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
        decoration: BoxDecoration(
          color: accent ? Tokens.secVideos : const Color(0x99000000),
          borderRadius: BorderRadius.circular(accent ? 99 : 6),
        ),
        child: Text(text,
            style: const TextStyle(
                fontSize: 10.5,
                fontWeight: FontWeight.w800,
                color: Colors.white,
                fontFeatures: [FontFeature.tabularFigures()])),
      );
}

class _RoundMark extends StatelessWidget {
  const _RoundMark({required this.icon});

  final IconData icon;

  @override
  Widget build(BuildContext context) => Container(
        width: 24,
        height: 24,
        decoration: const BoxDecoration(
            color: Color(0x99000000), shape: BoxShape.circle),
        child: Icon(icon, size: 14, color: Colors.white),
      );
}

// ---------------------------------------------------------- the chrome ----

class _Heading extends StatelessWidget {
  const _Heading(this.text, this.note, {this.size = 17});

  final String text;
  final String note;
  final double size;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Text.rich(
      TextSpan(children: [
        TextSpan(text: text),
        if (note.isNotEmpty)
          TextSpan(
            text: '   $note',
            style: TextStyle(
                fontSize: 12.5, fontWeight: FontWeight.w500, color: t.nInk3),
          ),
      ]),
      style: TextStyle(
          fontSize: size,
          fontWeight: FontWeight.w700,
          letterSpacing: -0.2,
          color: t.nInk),
    );
  }
}

class _MetaRow extends StatelessWidget {
  const _MetaRow({required this.meta, required this.badges});

  final List<String> meta;
  final List<String> badges;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Wrap(
      spacing: 8,
      runSpacing: 6,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        for (final m in meta)
          Text(m, style: TextStyle(fontSize: 13, color: t.nInk2)),
        for (final b in badges) _Badge(b),
      ],
    );
  }
}

class _Badge extends StatelessWidget {
  const _Badge(this.text);

  final String text;

  bool get _hdr =>
      text.contains('HDR') || text == 'Dolby Vision' || text == 'HLG';

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 22,
      padding: const EdgeInsets.symmetric(horizontal: 7),
      decoration: BoxDecoration(
        gradient: _hdr
            ? const LinearGradient(
                colors: [Color(0xFFF59E0B), Color(0xFFEC4899)])
            : null,
        border: _hdr ? null : Border.all(color: t.nHair),
        borderRadius: BorderRadius.circular(6),
      ),
      child: Center(
        widthFactor: 1,
        child: Text(text,
            style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w700,
                color: _hdr ? Colors.white : t.nInk)),
      ),
    );
  }
}

/// The one filled button a stage or page is built around.
class _PlayButton extends StatelessWidget {
  const _PlayButton({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final skin = context.skin;
    final row = Row(mainAxisSize: MainAxisSize.min, children: [
      Icon(skin.icon(Icons.play_arrow),
          size: 20,
          color: skin.isStandard
              ? (t.dark ? const Color(0xFF111111) : Colors.white)
              : (skin.onProminent ?? skin.accent)),
      const SizedBox(width: 8),
      Text(label,
          style: TextStyle(
              fontSize: 14,
              fontWeight: FontWeight.w700,
              color: skin.isStandard
                  ? (t.dark ? const Color(0xFF111111) : Colors.white)
                  : (skin.onProminent ?? skin.accent))),
    ]);
    if (!skin.isStandard) {
      return SkinButton(
        prominent: true,
        radius: 22,
        height: 44,
        padding: const EdgeInsets.only(left: 16, right: 20),
        onTap: onTap,
        child: row,
      );
    }
    return Material(
      color: t.dark ? const Color(0xFFF0F0F3) : t.nInk,
      shape: const StadiumBorder(),
      child: InkWell(
        customBorder: const StadiumBorder(),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 12, 20, 12),
          child: row,
        ),
      ),
    );
  }
}

/// A quieter button: an outline, or an icon alone.
class _Ghost extends StatelessWidget {
  const _Ghost({
    this.label,
    this.icon,
    this.tip,
    this.active = false,
    this.onTap,
    this.onTapAt,
  });

  final String? label;
  final IconData? icon;
  final String? tip;
  final bool active;
  final VoidCallback? onTap;

  /// For a button that opens a menu where it was pressed.
  final ValueChanged<Offset>? onTapAt;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final skin = context.skin;
    final ink = active ? const Color(0xFFFBBF24) : t.nInk;
    final child = Row(mainAxisSize: MainAxisSize.min, children: [
      if (icon != null)
        Icon(skin.icon(icon!), size: 18, color: ink, fill: active ? 1 : null),
      if (icon != null && label != null) const SizedBox(width: 8),
      if (label != null)
        Text(label!,
            style: TextStyle(
                fontSize: 13.5, fontWeight: FontWeight.w600, color: t.nInk)),
    ]);
    final pad = label == null
        ? EdgeInsets.zero
        : const EdgeInsets.symmetric(horizontal: 16);
    Widget b;
    if (!skin.isStandard) {
      b = SkinButton(
        radius: 22,
        width: label == null ? 44 : null,
        height: 44,
        padding: pad,
        onTap: onTap ?? () {},
        child: Center(widthFactor: 1, child: child),
      );
    } else {
      b = Container(
        height: 44,
        width: label == null ? 44 : null,
        padding: pad,
        decoration: BoxDecoration(
          color: t.nChip,
          borderRadius: BorderRadius.circular(label == null ? 22 : 12),
          border: Border.all(color: t.nHair),
        ),
        child: Center(widthFactor: 1, child: child),
      );
      b = MouseRegion(
        cursor: SystemMouseCursors.click,
        child: GestureDetector(onTap: onTap, child: b),
      );
    }
    if (onTapAt != null) {
      b = GestureDetector(
        onTapDown: (d) => onTapAt!(d.globalPosition),
        child: AbsorbPointer(child: b),
      );
    }
    return tip == null ? b : Tooltip(message: tip!, child: b);
  }
}

class _BackPill extends StatelessWidget {
  const _BackPill({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ClipRRect(
      borderRadius: BorderRadius.circular(99),
      child: BackdropFilter(
        filter: ImageFilter.blur(sigmaX: 10, sigmaY: 10),
        child: Material(
          color: t.dark ? const Color(0x59000000) : const Color(0xB3FFFFFF),
          child: InkWell(
            onTap: onTap,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(10, 8, 14, 8),
              child: Row(mainAxisSize: MainAxisSize.min, children: [
                Icon(Icons.chevron_left, size: 18, color: t.nInk),
                const SizedBox(width: 2),
                Text(label,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w600,
                        color: t.nInk)),
              ]),
            ),
          ),
        ),
      ),
    );
  }
}

class _Box extends StatelessWidget {
  const _Box({required this.title, this.note = '', required this.child});

  final String title;
  final String note;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(20, 18, 20, 16),
      decoration: context.skin.surface(SurfaceRole.card, radius: 16) ??
          BoxDecoration(
            color: t.nCard,
            borderRadius: BorderRadius.circular(16),
            border: Border.all(color: t.nHair),
          ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          _Heading(title, note, size: 14),
          const SizedBox(height: 10),
          child,
        ],
      ),
    );
  }
}

class _Bar extends StatelessWidget {
  const _Bar({required this.value, this.light = false});

  final double value;
  final bool light;

  @override
  Widget build(BuildContext context) => ClipRRect(
        borderRadius: BorderRadius.circular(9),
        child: SizedBox(
          height: light ? 3 : 4,
          child: LinearProgressIndicator(
            value: value.clamp(0.0, 1.0),
            backgroundColor:
                light ? const Color(0x47FFFFFF) : context.tokens.nHair,
            color: light ? Colors.white : Tokens.secVideos,
          ),
        ),
      );
}

class _EmptyLine extends StatelessWidget {
  const _EmptyLine(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.fromLTRB(_pad, 24, _pad, 48),
        child: Text(text,
            style: TextStyle(fontSize: 14, color: context.tokens.nInk3)),
      );
}

/// A filter chip, with a count when there is one.
class _Chip extends StatelessWidget {
  const _Chip({
    required this.label,
    required this.active,
    required this.onTap,
    this.count,
    this.icon,
  });

  final String label;
  final bool active;
  final VoidCallback onTap;
  final int? count;
  final IconData? icon;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final skin = context.skin;
    final ink = active ? Tokens.secVideos : t.nInk2;
    final child = Row(mainAxisSize: MainAxisSize.min, children: [
      if (icon != null) ...[
        Icon(skin.icon(icon!), size: 15, color: ink),
        const SizedBox(width: 6),
      ],
      Text(label,
          style: TextStyle(
              fontSize: 12.5, fontWeight: FontWeight.w600, color: ink)),
      if (count != null) ...[
        const SizedBox(width: 6),
        Text('$count', style: TextStyle(fontSize: 11, color: t.nInk3)),
      ],
    ]);
    if (!skin.isStandard) {
      return SkinButton(
        active: active,
        radius: 16,
        height: 32,
        padding: const EdgeInsets.symmetric(horizontal: 13),
        tint: Tokens.secVideos,
        onTap: onTap,
        child: Center(widthFactor: 1, child: child),
      );
    }
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 140),
          height: 32,
          padding: const EdgeInsets.symmetric(horizontal: 13),
          decoration: BoxDecoration(
            color: active ? Tokens.secVideos.withValues(alpha: 0.16) : t.nChip,
            borderRadius: BorderRadius.circular(16),
            border: Border.all(color: active ? Colors.transparent : t.nHair),
          ),
          child: Center(widthFactor: 1, child: child),
        ),
      ),
    );
  }
}

/// A small text button for the things you visit rarely: Archived, Trash,
/// Rescan.
class _Quiet extends StatelessWidget {
  const _Quiet(
      {required this.icon,
      required this.label,
      required this.onTap,
      this.active = false});

  final IconData icon;
  final String label;
  final VoidCallback onTap;
  final bool active;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final ink = active ? Tokens.secVideos : t.nInk3;
    return InkWell(
      borderRadius: BorderRadius.circular(8),
      onTap: onTap,
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 7),
        child: Row(mainAxisSize: MainAxisSize.min, children: [
          Icon(context.skin.icon(icon), size: 15, color: ink),
          const SizedBox(width: 6),
          Text(label, style: TextStyle(fontSize: 12.5, color: ink)),
        ]),
      ),
    );
  }
}

/// The row over a grid: the filter chips, then Archived, Trash, the sort and
/// the library actions. The chips Settings → Sections has hidden stay hidden.
class _FilterRow extends StatelessWidget {
  const _FilterRow({
    required this.controller,
    required this.state,
    required this.withSort,
    this.leading,
  });

  final VideosController controller;
  final VideosState state;
  final bool withSort;

  /// Chips of the view's own instead of the category chips (Shows' watching
  /// filter, Local's grouping) -- drawn before whatever category chips the view
  /// keeps.
  final List<Widget>? leading;

  int _count(String name) {
    for (final c in state.counts) {
      if (c.id == name) return c.count;
    }
    return 0;
  }

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final st = state;
    // Which of the Settings-managed categories are on.
    final kept = {
      for (final k in keepTabs('videos', kVideoCategories, (k) => k.id,
          active: (k) => st.category == k.id))
        k.id,
    };
    void pick(String name) => c.send(VideosCmd.setCategory(name: name));
    final chips = st.kind == 'tv'
        ? const <(String, String)>[]
        : st.kind == 'local'
            ? const [
                ('library', 'All'),
                ('continue', 'In progress'),
                ('starred', 'Starred')
              ]
            : const [
                ('library', 'All'),
                ('unwatched', 'Unwatched'),
                ('continue', 'In progress'),
                ('starred', 'Starred'),
                ('4k', '4K'),
                ('hdr', 'HDR'),
              ];
    const sorts = [
      ('added', 'Recently added'),
      ('title', 'Title'),
      ('year', 'Year'),
      ('runtime', 'Runtime'),
    ];
    return Wrap(
      spacing: 8,
      runSpacing: 8,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        ...?leading,
        for (final (id, label) in chips)
          if (!kVideoCategories.any((k) => k.id == id) || kept.contains(id))
            _Chip(
              label: label,
              count: id == 'library' ? null : _count(id),
              active: st.category == id,
              onTap: () => pick(id),
            ),
        const SizedBox(width: 12),
        if (kept.contains('archive'))
          _Quiet(
            icon: Icons.archive_outlined,
            label: 'Archived ${_count('archive')}',
            active: st.category == 'archive',
            onTap: () => pick(st.category == 'archive' ? 'library' : 'archive'),
          ),
        if (kept.contains('trash'))
          _Quiet(
            icon: Icons.delete_outline,
            label: 'Trash ${_count('trash')}',
            active: st.category == 'trash',
            onTap: () => pick(st.category == 'trash' ? 'library' : 'trash'),
          ),
        _Quiet(
          icon: Icons.sync,
          label: 'Rescan',
          onTap: () => c.send(const VideosCmd.scan()),
        ),
        _Quiet(
          icon: Icons.refresh,
          label: 'Clear thumbs',
          onTap: () => c.send(const VideosCmd.clearThumbs()),
        ),
        if (withSort)
          PopupMenuButton<String>(
            tooltip: 'Sort',
            initialValue: st.sort,
            onSelected: (k) => c.send(VideosCmd.setSort(key: k)),
            itemBuilder: (_) => [
              for (final (k, l) in sorts)
                PopupMenuItem(value: k, child: Text(l)),
            ],
            child: IgnorePointer(
              child: _Quiet(
                icon: Icons.sort,
                label: sorts
                    .firstWhere((s) => s.$1 == st.sort,
                        orElse: () => sorts.first)
                    .$2,
                onTap: () {},
              ),
            ),
          ),
      ],
    );
  }
}

/// A title's right-click menu. Which actions it offers depends on the
/// category it is in — Trash offers Restore, everything else offers Trash.
Future<void> _tileMenu(BuildContext context, VideosController controller,
    VideoTile tile, String category, Offset global) async {
  final overlay = Overlay.of(context).context.findRenderObject() as RenderBox?;
  if (overlay == null) return;
  final trash = category == 'trash';
  final picked = await showMenu<String>(
    context: context,
    position: RelativeRect.fromRect(
      global & const Size(1, 1),
      Offset.zero & overlay.size,
    ),
    items: [
      const PopupMenuItem(value: 'play', child: Text('Play')),
      PopupMenuItem(
          value: 'star', child: Text(tile.starred ? 'Unstar' : 'Star')),
      PopupMenuItem(
        value: 'mark-watched',
        child: Text(tile.watched ? 'Mark unwatched' : 'Mark watched'),
      ),
      if (!trash)
        PopupMenuItem(
          value: 'archive',
          child: Text(category == 'archive' ? 'Unarchive' : 'Archive'),
        ),
      if (!trash) const PopupMenuItem(value: 'trash', child: Text('Trash')),
      if (trash) const PopupMenuItem(value: 'restore', child: Text('Restore')),
      if (trash)
        const PopupMenuItem(
            value: 'delete-forever', child: Text('Remove from library')),
      const PopupMenuDivider(),
      const PopupMenuItem(value: 'reveal', child: Text('Show in files')),
    ],
  );
  if (picked == null) return;
  await controller
      .send(VideosCmd.tileAction(index: tile.index, action: picked));
}

/// The accent tick plus a heading — the section's rail title, used by the
/// other tabs.
class RailTitle extends StatelessWidget {
  const RailTitle(this.text, {super.key});

  final String text;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Container(
          width: 4,
          height: 20,
          decoration: BoxDecoration(
            color: Tokens.secVideos,
            borderRadius: BorderRadius.circular(2),
          ),
        ),
        const SizedBox(width: 8),
        Text(text,
            style: TextStyle(
                fontSize: 18, fontWeight: FontWeight.w700, color: t.nInk)),
      ],
    );
  }
}
