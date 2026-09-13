// The on-disk library: the featured hero, the poster grid, the TV show cards,
// the Next Up rail and the drilled-in season browser.
//
// One body for TV / Movies / Local — they differ only in what the bridge put in
// the snapshot, which is the same split the Slint page makes with `cards-mode`
// and `drilled`.

import 'package:flutter/material.dart';

import '../../design/pick.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/videos.dart';
import 'videos_controller.dart';
import 'videos_widgets.dart';

/// Poster 150 wide + 16 of gap; the label sits in the 44 below it.
const double _cellW = 166;
const double _cellH = 285;

class VideosLibrary extends StatelessWidget {
  const VideosLibrary({
    super.key,
    required this.controller,
    required this.state,
  });

  final VideosController controller;
  final VideosState state;

  bool get _cardsMode => state.kind == 'tv' && !state.showOpen;
  bool get _drilled => state.kind == 'tv' && state.showOpen;

  @override
  Widget build(BuildContext context) {
    if (_cardsMode) return _ShowCards(controller: controller, state: state);
    if (_drilled) return _Seasons(controller: controller, state: state);
    if (state.tiles.isEmpty) {
      return VideosEmpty(
        icon: Icons.movie_creation_outlined,
        title: categoryTitle(state.category),
        message: categoryEmpty(state.category),
        action: state.category == 'library'
            ? PlexButton(
                label: '+ Add folder',
                filled: true,
                hue: cPlay,
                onTap: () => addVideoFolder(context, controller),
              )
            : null,
      );
    }
    return _Grid(controller: controller, state: state);
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

// ------------------------------------------------------------------ grid ----

class _Grid extends StatelessWidget {
  const _Grid({required this.controller, required this.state});

  final VideosController controller;
  final VideosState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final hero = state.category == 'library' &&
        state.kind == 'local' &&
        state.tiles.isNotEmpty;
    return ListView(
      padding: EdgeInsets.zero,
      children: [
        if (hero) _Hero(controller: controller, state: state),
        Padding(
          padding: const EdgeInsets.fromLTRB(44, 28, 44, 36),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _RailTitle(categoryTitle(state.category)),
              const SizedBox(height: 14),
              _PosterWrap(
                tiles: state.tiles,
                controller: controller,
                category: state.category,
              ),
              if (state.moreCount > 0) ...[
                const SizedBox(height: 20),
                Center(
                  child: PlexButton(
                    label: 'Show ${state.moreCount} more',
                    hue: Tokens.secVideos,
                    onTap: () => controller.send(const VideosCmd.showMore()),
                  ),
                ),
              ],
              const SizedBox(height: 8),
              Text(
                '${state.itemCount} titles',
                style: TextStyle(fontSize: 11, color: t.nInk3),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

class _Hero extends StatefulWidget {
  const _Hero({required this.controller, required this.state});

  final VideosController controller;
  final VideosState state;

  @override
  State<_Hero> createState() => _HeroState();
}

class _HeroState extends State<_Hero> {
  String? _art;

  @override
  void initState() {
    super.initState();
    _load();
  }

  @override
  void didUpdateWidget(_Hero old) {
    super.didUpdateWidget(old);
    if (old.state.tiles.first.itemId != widget.state.tiles.first.itemId) {
      _art = null;
      _load();
    }
  }

  Future<void> _load() async {
    final tile = widget.state.tiles.first;
    final path = tile.thumb.isNotEmpty
        ? tile.thumb
        : await widget.controller.thumbFor(tile.itemId);
    if (mounted && path != null) setState(() => _art = path);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tile = widget.state.tiles.first;
    return SizedBox(
      height: 360,
      child: Stack(
        fit: StackFit.expand,
        children: [
          ColoredBox(color: t.bg),
          if (_art != null)
            Opacity(
              opacity: 0.55,
              child: Artwork(
                  path: _art!, width: double.infinity, height: 360, radius: 0),
            ),
          // Two scrims, bottom and left, so the copy sits on a readable edge
          // whatever the still behind it is doing.
          DecoratedBox(
            decoration: BoxDecoration(
              gradient: LinearGradient(
                begin: Alignment.bottomCenter,
                end: Alignment.topCenter,
                colors: [t.panel, t.panel.withValues(alpha: 0)],
                stops: const [0.04, 0.60],
              ),
            ),
          ),
          DecoratedBox(
            decoration: BoxDecoration(
              gradient: LinearGradient(
                begin: Alignment.centerLeft,
                end: Alignment.centerRight,
                colors: [t.panel, t.panel.withValues(alpha: 0)],
                stops: const [0.0, 0.55],
              ),
            ),
          ),
          Padding(
            padding: const EdgeInsets.all(44),
            child: Column(
              mainAxisAlignment: MainAxisAlignment.end,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text('FEATURED',
                    style: TextStyle(
                        fontSize: 11,
                        fontWeight: FontWeight.w800,
                        letterSpacing: 2,
                        color: Tokens.secVideos)),
                const SizedBox(height: 14),
                Text(
                  tile.label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 34, fontWeight: FontWeight.w800, color: t.nInk),
                ),
                const SizedBox(height: 6),
                Text('${widget.state.itemCount} titles in your library',
                    style: TextStyle(fontSize: 13, color: t.nInk2)),
                const SizedBox(height: 14),
                Row(
                  children: [
                    PlexButton(
                      icon: Icons.play_arrow,
                      label: 'Play',
                      filled: true,
                      hue: cPlay,
                      onTap: () => widget.controller
                          .send(VideosCmd.play(index: tile.index)),
                    ),
                    const SizedBox(width: 12),
                    PlexButton(
                      label: '+ Add media',
                      onTap: () => addVideoFolder(context, widget.controller),
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

/// A wrapping run of posters. Wrap rather than a fixed column count so the grid
/// reflows with the window, which is what the Slint page's `cols` computation
/// does by hand.
class _PosterWrap extends StatelessWidget {
  const _PosterWrap({
    required this.tiles,
    required this.controller,
    required this.category,
  });

  final List<VideoTile> tiles;
  final VideosController controller;
  final String category;

  @override
  Widget build(BuildContext context) {
    return Wrap(
      spacing: _cellW - 150,
      runSpacing: _cellH - 225 - 22,
      children: [
        for (final tile in tiles)
          PosterTile(
            tile: tile,
            controller: controller,
            category: category,
          ),
      ],
    );
  }
}

/// One library poster: artwork, the badges over it, and the right-click menu.
class PosterTile extends StatefulWidget {
  const PosterTile({
    super.key,
    required this.tile,
    required this.controller,
    required this.category,
  });

  final VideoTile tile;
  final VideosController controller;
  final String category;

  @override
  State<PosterTile> createState() => _PosterTileState();
}

class _PosterTileState extends State<PosterTile> {
  String? _thumb;
  bool _hover = false;

  @override
  void initState() {
    super.initState();
    _resolve();
  }

  @override
  void didUpdateWidget(PosterTile old) {
    super.didUpdateWidget(old);
    if (old.tile.itemId != widget.tile.itemId) {
      _thumb = null;
      _resolve();
    }
  }

  Future<void> _resolve() async {
    if (widget.tile.thumb.isNotEmpty) {
      setState(() => _thumb = widget.tile.thumb);
      return;
    }
    final path = await widget.controller.thumbFor(widget.tile.itemId);
    // The grid recycles cells while renders are in flight, so a late result
    // belongs to whichever item the cell now holds.
    if (mounted && path != null) setState(() => _thumb = path);
  }

  Future<void> _menu(BuildContext context, Offset global) async {
    final overlay =
        Overlay.of(context).context.findRenderObject() as RenderBox?;
    if (overlay == null) return;
    final trash = widget.category == 'trash';
    final picked = await showMenu<String>(
      context: context,
      position: RelativeRect.fromRect(
        global & const Size(1, 1),
        Offset.zero & overlay.size,
      ),
      items: [
        const PopupMenuItem(value: 'play', child: Text('Play')),
        PopupMenuItem(
          value: 'star',
          child: Text(widget.category == 'starred' ? 'Unstar' : 'Star'),
        ),
        PopupMenuItem(
          value: 'mark-watched',
          child: Text(widget.tile.watched ? 'Mark unwatched' : 'Mark watched'),
        ),
        if (!trash)
          PopupMenuItem(
            value: 'archive',
            child: Text(widget.category == 'archive' ? 'Unarchive' : 'Archive'),
          ),
        if (!trash) const PopupMenuItem(value: 'trash', child: Text('Trash')),
        if (trash)
          const PopupMenuItem(value: 'restore', child: Text('Restore')),
        if (trash)
          const PopupMenuItem(
              value: 'delete-forever', child: Text('Remove from library')),
        const PopupMenuDivider(),
        const PopupMenuItem(value: 'reveal', child: Text('Show in files')),
      ],
    );
    if (picked == null) return;
    await widget.controller
        .send(VideosCmd.tileAction(index: widget.tile.index, action: picked));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tile = widget.tile;
    final ep = tile.episode > 0
        ? '${tile.season > 0 ? 'S${tile.season}' : ''}E${tile.episode}'
        : '';
    return SizedBox(
      width: 150,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          MouseRegion(
            cursor: SystemMouseCursors.click,
            onEnter: (_) => setState(() => _hover = true),
            onExit: (_) => setState(() => _hover = false),
            child: GestureDetector(
              onTap: () =>
                  widget.controller.send(VideosCmd.play(index: tile.index)),
              onSecondaryTapDown: (d) => _menu(context, d.globalPosition),
              child: Stack(
                children: [
                  Artwork(path: _thumb ?? ''),
                  if (_hover)
                    Container(
                      width: 150,
                      height: 225,
                      decoration: BoxDecoration(
                        borderRadius: BorderRadius.circular(8),
                        border: Border.all(color: Tokens.secVideos, width: 2),
                      ),
                    ),
                  if (tile.starred)
                    const Positioned(
                      top: 6,
                      right: 6,
                      child: _RoundBadge(
                          icon: Icons.star,
                          background: Color(0xCC000000),
                          ink: Tokens.secVideos),
                    ),
                  if (tile.watched)
                    const Positioned(
                      top: 6,
                      left: 6,
                      child: _RoundBadge(
                          icon: Icons.check,
                          background: Tokens.secVideos,
                          ink: Colors.white),
                    ),
                  if (tile.duration.isNotEmpty)
                    Positioned(
                      right: 6,
                      bottom: 10,
                      child: OverlayPill(text: tile.duration, bold: false),
                    ),
                  if (ep.isNotEmpty)
                    Positioned(
                      left: 6,
                      bottom: 10,
                      child: OverlayPill(
                        text: ep,
                        background: Tokens.secVideos.withValues(alpha: 0.80),
                      ),
                    ),
                  if (tile.progress > 0 && !tile.watched)
                    Positioned(
                      left: 0,
                      right: 0,
                      bottom: 0,
                      child: ProgressStrip(value: tile.progress),
                    ),
                ],
              ),
            ),
          ),
          const SizedBox(height: 8),
          Text(
            tile.label,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
                fontSize: 12, fontWeight: FontWeight.w500, color: t.nInk),
          ),
        ],
      ),
    );
  }
}

class _RoundBadge extends StatelessWidget {
  const _RoundBadge(
      {required this.icon, required this.background, required this.ink});

  final IconData icon;
  final Color background;
  final Color ink;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 24,
      height: 24,
      decoration: BoxDecoration(color: background, shape: BoxShape.circle),
      child: Icon(icon, size: 14, color: ink),
    );
  }
}

// ------------------------------------------------------------- TV shows ----

class _ShowCards extends StatelessWidget {
  const _ShowCards({required this.controller, required this.state});

  final VideosController controller;
  final VideosState state;

  @override
  Widget build(BuildContext context) {
    if (state.shows.isEmpty) {
      return const VideosEmpty(
        icon: Icons.tv_outlined,
        title: 'No TV shows',
        message: "Name episode files like 'Show S01E01.mkv' in a per-show "
            'folder; they group here.',
      );
    }
    return ListView(
      padding: const EdgeInsets.fromLTRB(44, 28, 44, 36),
      children: [
        if (state.nextUp.isNotEmpty) ...[
          const _RailTitle('Next Up'),
          const SizedBox(height: 14),
          SizedBox(
            height: _cellH,
            child: ListView.separated(
              scrollDirection: Axis.horizontal,
              itemCount: state.nextUp.length,
              separatorBuilder: (_, __) => const SizedBox(width: 16),
              itemBuilder: (_, i) => PosterTile(
                tile: state.nextUp[i],
                controller: controller,
                category: state.category,
              ),
            ),
          ),
          const SizedBox(height: 20),
        ],
        const _RailTitle('TV Shows'),
        const SizedBox(height: 14),
        Wrap(
          spacing: _cellW - 150,
          runSpacing: _cellH - 225 - 22,
          children: [
            for (final show in state.shows)
              _ShowCardView(show: show, controller: controller),
          ],
        ),
      ],
    );
  }
}

class _ShowCardView extends StatefulWidget {
  const _ShowCardView({required this.show, required this.controller});

  final ShowCard show;
  final VideosController controller;

  @override
  State<_ShowCardView> createState() => _ShowCardViewState();
}

class _ShowCardViewState extends State<_ShowCardView> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      width: 150,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          MouseRegion(
            cursor: SystemMouseCursors.click,
            onEnter: (_) => setState(() => _hover = true),
            onExit: (_) => setState(() => _hover = false),
            child: GestureDetector(
              onTap: () => widget.controller
                  .send(VideosCmd.openShow(showId: widget.show.id)),
              child: Stack(
                children: [
                  Artwork(path: widget.show.cover, fallback: Icons.tv),
                  if (_hover)
                    Container(
                      width: 150,
                      height: 225,
                      decoration: BoxDecoration(
                        borderRadius: BorderRadius.circular(8),
                        border: Border.all(color: Tokens.secVideos, width: 2),
                      ),
                    ),
                  Positioned(
                    top: 6,
                    right: 6,
                    child: OverlayPill(text: '${widget.show.count} ep'),
                  ),
                ],
              ),
            ),
          ),
          const SizedBox(height: 8),
          Text(
            widget.show.title,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
                fontSize: 12, fontWeight: FontWeight.w600, color: t.nInk),
          ),
        ],
      ),
    );
  }
}

class _Seasons extends StatelessWidget {
  const _Seasons({required this.controller, required this.state});

  final VideosController controller;
  final VideosState state;

  @override
  Widget build(BuildContext context) {
    if (state.seasons.isEmpty) {
      return const VideosEmpty(
        icon: Icons.tv_outlined,
        title: 'No episodes',
        message: 'This show has no episodes in the current tab.',
      );
    }
    return ListView(
      padding: const EdgeInsets.fromLTRB(44, 28, 44, 36),
      children: [
        for (final season in state.seasons) ...[
          Row(
            crossAxisAlignment: CrossAxisAlignment.center,
            children: [
              _RailTitle(season.label),
              const SizedBox(width: 10),
              Text(
                '${season.tiles.length} '
                '${season.tiles.length == 1 ? 'episode' : 'episodes'}',
                style: TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                    color: context.tokens.nInk3),
              ),
            ],
          ),
          const SizedBox(height: 14),
          _PosterWrap(
            tiles: season.tiles,
            controller: controller,
            category: state.category,
          ),
          const SizedBox(height: 22),
        ],
      ],
    );
  }
}

/// The accent tick plus a heading — the section's rail title, used a dozen
/// times across the seven tabs.
class _RailTitle extends StatelessWidget {
  const _RailTitle(this.text);

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

/// Shared with the other tabs — the same tick-and-heading, exported.
class RailTitle extends StatelessWidget {
  const RailTitle(this.text, {super.key});

  final String text;

  @override
  Widget build(BuildContext context) => _RailTitle(text);
}
