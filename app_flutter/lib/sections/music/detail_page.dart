// The album / artist / genre / playlist / folder page.
//
// One widget for five kinds, because they are the same page in
// ui/page_music.slint too: a 244px hero — cover, an outlined info card, and a
// round Back the size of the cover — over a tracklist. The artist is the one
// branch: its page is 70/30, songs on the left and its albums on the right.

import 'package:flutter/material.dart';

import '../../design/pick.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';
import 'music_widgets.dart';

/// Everything in the hero is sized off this. Slint fixes it at 244 with a
/// 196px cover inside, and the proportion is the design: the cover is big
/// enough to be the subject and the card beside it is exactly as tall.
const double _heroHeight = 244;
const double _coverSide = 196;

class DetailPage extends StatelessWidget {
  const DetailPage({super.key, required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    if (st == null) return const SizedBox.shrink();
    final tracks = st.detailTracks;
    final artist = st.detailKind == 'artist';

    return ListView(
      padding: EdgeInsets.zero,
      children: [
        _Hero(controller: controller, st: st),
        const SizedBox(height: 20),
        if (tracks.isEmpty && st.detailAlbums.isEmpty)
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 40),
            child: MusicEmpty(
              icon: Icons.music_off_outlined,
              title: 'Nothing in here',
              body: 'Every track this belonged to has been removed or is '
                  'missing from disk.',
            ),
          )
        else if (artist)
          Padding(
            padding: const EdgeInsets.fromLTRB(36, 0, 36, 24),
            child: LayoutBuilder(
              builder: (context, box) {
                final songs = _Songs(controller: controller, st: st);
                final albums = _ArtistAlbums(controller: controller, st: st);
                // Under about a thousand pixels the 30% column is narrower
                // than one album tile, so the split stops paying for itself.
                if (box.maxWidth < 1000) {
                  return Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [songs, const SizedBox(height: 22), albums],
                  );
                }
                return Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Expanded(flex: 7, child: songs),
                    const SizedBox(width: 22),
                    Expanded(flex: 3, child: albums),
                  ],
                );
              },
            ),
          )
        else
          Padding(
            padding: const EdgeInsets.fromLTRB(36, 0, 36, 24),
            child: _Tracks(controller: controller, st: st),
          ),
      ],
    );
  }
}

class _Hero extends StatelessWidget {
  const _Hero({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final artist = st.detailKind == 'artist';
    final tracks = st.detailTracks;
    return SizedBox(
      height: _heroHeight,
      child: DecoratedBox(
        // The wash is the artwork's own colour, so the page is tinted by what
        // is on it rather than by the section.
        decoration: BoxDecoration(
          gradient: LinearGradient(
            begin: Alignment.topCenter,
            end: Alignment.bottomCenter,
            colors: [
              controller.accent.withValues(alpha: 0.35),
              t.nCanvas,
            ],
          ),
        ),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(36, 18, 36, 16),
          // The hero is three fixed blocks — a 196px cover, a card with a 420
          // minimum, and a Back the size of the cover — which comes to 928
          // before padding. A Row cannot shrink any of them, so anything under
          // about a thousand pixels overflowed. Below that the card gives up
          // its minimum and Back gives up most of its diameter; the shape is
          // the same, there is just less of it.
          child: LayoutBuilder(
            builder: (context, box) {
              final roomy = box.maxWidth >= 940;
              return Row(
                children: [
                  _Cover(
                    controller: controller,
                    st: st,
                    child: Container(
                      width: _coverSide,
                      height: _coverSide,
                      decoration: BoxDecoration(
                        borderRadius:
                            BorderRadius.circular(artist ? _coverSide / 2 : 14),
                        border: Border.all(color: t.nHair),
                        boxShadow: const [
                          BoxShadow(color: Color(0xAA000000), blurRadius: 24),
                        ],
                      ),
                      clipBehavior: Clip.antiAlias,
                      child: MusicArt(
                        controller: controller,
                        kind: st.detailKind,
                        artKey: st.detailKind == 'album' || artist
                            ? '${st.detailId}'
                            : st.detailKey,
                        direct: st.detailArt,
                        size: _coverSide,
                        radius: 0,
                        fallback: switch (st.detailKind) {
                          'artist' => Icons.mic_none,
                          'playlist' => Icons.queue_music,
                          'folder' => Icons.folder,
                          'genre' => Icons.library_music_outlined,
                          _ => Icons.album_outlined,
                        },
                      ),
                    ),
                  ),
                  const SizedBox(width: 22),
                  // Sized to its content, not to the row: Slint gives it a 420
                  // minimum and lets the Back button take the far edge, so a long
                  // album title does not push the actions off screen.
                  Flexible(
                    child: Container(
                      height: _coverSide,
                      constraints: BoxConstraints(minWidth: roomy ? 420 : 0),
                      padding: const EdgeInsets.all(18),
                      decoration: BoxDecoration(
                        color: t.nCard,
                        borderRadius: BorderRadius.circular(14),
                        border: Border.all(color: t.nHair),
                      ),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            switch (st.detailKind) {
                              'artist' => 'ARTIST',
                              'genre' => 'GENRE',
                              'folder' => 'FOLDER',
                              'playlist' => 'PLAYLIST',
                              _ => 'ALBUM',
                            },
                            style: TextStyle(
                              fontFamily: Tokens.fontFamily,
                              fontSize: 11,
                              fontWeight: FontWeight.w700,
                              letterSpacing: 2,
                              color: t.nInk3,
                            ),
                          ),
                          const SizedBox(height: 7),
                          Text(
                            st.detailTitle,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                              fontFamily: Tokens.fontFamily,
                              fontSize: 32,
                              fontWeight: FontWeight.w800,
                              color: t.nInk,
                            ),
                          ),
                          const SizedBox(height: 7),
                          // On an album page the subtitle is who made it, and
                          // the artist has a page of their own — so it is a
                          // link, the same way the player bar's second line is.
                          if (st.detailKind == 'album' &&
                              st.detailArtistId != 0 &&
                              st.detailSubtitle.isNotEmpty)
                            _ArtistLink(
                              name: st.detailSubtitle,
                              onTap: () => controller.send(MusicCmd.openArtist(
                                  artistId: st.detailArtistId)),
                            )
                          else
                            Text(
                              st.detailSubtitle,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(fontSize: 14, color: t.nInk2),
                            ),
                          // What the artist actually is — MusicBrainz's type,
                          // country and years, fetched once and kept. The box
                          // beside a face was three lines of nothing.
                          if (st.detailInfo.isNotEmpty) ...[
                            const SizedBox(height: 8),
                            Text(
                              st.detailInfo,
                              maxLines: 3,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                fontSize: 12,
                                height: 1.4,
                                color: t.nInk2,
                              ),
                            ),
                          ],
                          // The heart and the stars on the album or the artist
                          // itself. They live on that row, not on its tracks,
                          // and the grid tile you came from already draws them
                          // — the page that opens from it should not be the one
                          // place you cannot set them.
                          if (st.detailKind == 'album' ||
                              st.detailKind == 'artist') ...[
                            const SizedBox(height: 10),
                            _Marks(controller: controller, st: st),
                          ],
                          const Spacer(),
                          SingleChildScrollView(
                            scrollDirection: Axis.horizontal,
                            child: Row(
                              children: [
                                _PlayAll(
                                  onTap: tracks.isEmpty
                                      ? null
                                      : () => controller.playFrom(
                                          tracks, 0, st.detailKind),
                                ),
                                const SizedBox(width: 8),
                                DetailActionBtn(
                                  icon: Icons.shuffle,
                                  label: 'Shuffle',
                                  onTap: () async {
                                    if (tracks.isEmpty) return;
                                    if (!st.shuffle) {
                                      await controller
                                          .send(const MusicCmd.toggleShuffle());
                                    }
                                    await controller.playFrom(
                                        tracks, 0, st.detailKind);
                                  },
                                ),
                                const SizedBox(width: 8),
                                DetailActionBtn(
                                  icon: Icons.queue_music,
                                  label: 'Queue',
                                  onTap: () => controller.queueAll(tracks),
                                ),
                                if (st.detailKind == 'playlist') ...[
                                  const SizedBox(width: 8),
                                  DetailActionBtn(
                                    icon: Icons.delete_outline,
                                    label: 'Delete',
                                    tint: Tokens.error,
                                    onTap: () => confirmThen(
                                      context,
                                      controller,
                                      title: 'Delete this playlist?',
                                      body: '“${st.detailTitle}” and its order '
                                          'go. The tracks in it stay in the '
                                          'library.',
                                      action: 'Delete playlist',
                                      cmd: MusicCmd.playlistDelete(
                                          playlistId: st.detailId),
                                    ),
                                  ),
                                ],
                              ],
                            ),
                          ),
                        ],
                      ),
                    ),
                  ),
                  const SizedBox(width: 22),
                  _BackDisc(
                    size: roomy ? _coverSide : 72,
                    onTap: () => controller.send(const MusicCmd.closeDetail()),
                  ),
                ],
              );
            },
          ),
        ),
      ),
    );
  }
}

/// The cover, with a way to change it.
///
/// Every kind of page keeps its picture somewhere different — an album in
/// `albums.cover_path`, an artist in `artists.image_path`, a genre and a
/// playlist in a preference — so the button is one control over four stores.
/// A folder has no cover anywhere and gets no button.
class _Cover extends StatefulWidget {
  const _Cover({
    required this.controller,
    required this.st,
    required this.child,
  });

  final MusicController controller;
  final MusicState st;
  final Widget child;

  @override
  State<_Cover> createState() => _CoverState();
}

class _CoverState extends State<_Cover> {
  bool _hovered = false;

  String get _key => switch (widget.st.detailKind) {
        'album' || 'artist' => '${widget.st.detailId}',
        _ => widget.st.detailKey,
      };

  Future<void> _pick() async {
    final path = await pickFile(
      label: 'Images',
      extensions: const ['jpg', 'jpeg', 'png', 'webp', 'bmp'],
    );
    if (path == null) return;
    await widget.controller.setCardArt(widget.st.detailKind, _key, path);
  }

  @override
  Widget build(BuildContext context) {
    if (widget.st.detailKind == 'folder') return widget.child;
    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: Stack(
        children: [
          widget.child,
          Positioned(
            right: 8,
            bottom: 8,
            child: AnimatedOpacity(
              duration: context.tokens.reduceMotion
                  ? Duration.zero
                  : const Duration(milliseconds: 140),
              opacity: _hovered ? 1 : 0,
              child: Material(
                color: Tokens.secMusic,
                shape: const CircleBorder(),
                clipBehavior: Clip.antiAlias,
                child: InkWell(
                  onTap: _pick,
                  child: const SizedBox(
                    width: 40,
                    height: 40,
                    child: Tooltip(
                      message: 'Change cover…',
                      child: Icon(Icons.image_outlined,
                          size: 19, color: Colors.white),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// The album's artist, as a link. Underlined on hover only — a permanent rule
/// under one line of a three-line card reads as an error state.
class _ArtistLink extends StatefulWidget {
  const _ArtistLink({required this.name, required this.onTap});

  final String name;
  final VoidCallback onTap;

  @override
  State<_ArtistLink> createState() => _ArtistLinkState();
}

class _ArtistLinkState extends State<_ArtistLink> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) => MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hovered = true),
        onExit: (_) => setState(() => _hovered = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: Text(
            widget.name,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              fontSize: 14,
              color: _hovered ? Tokens.secMusic : context.tokens.nInk2,
              decoration: _hovered ? TextDecoration.underline : null,
              decorationColor: Tokens.secMusic,
            ),
          ),
        ),
      );
}

/// The heart and the five stars, for the album or artist the page is about.
class _Marks extends StatelessWidget {
  const _Marks({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final album = st.detailKind == 'album';
    return Row(
      children: [
        IconButton(
          iconSize: 20,
          visualDensity: VisualDensity.compact,
          padding: EdgeInsets.zero,
          constraints: const BoxConstraints(minWidth: 30, minHeight: 30),
          tooltip: st.detailLoved ? 'Remove from favourites' : 'Favourite',
          icon: Icon(
            st.detailLoved ? Icons.favorite : Icons.favorite_border,
            color: st.detailLoved ? Tokens.secMusic : t.nInk2,
          ),
          onPressed: () => controller.send(album
              ? MusicCmd.albumFav(albumId: st.detailId)
              : MusicCmd.artistFav(artistId: st.detailId)),
        ),
        const SizedBox(width: 4),
        for (var i = 1; i <= 5; i++)
          IconButton(
            iconSize: 18,
            visualDensity: VisualDensity.compact,
            padding: EdgeInsets.zero,
            constraints: const BoxConstraints(minWidth: 24, minHeight: 30),
            // Tapping the star that is already the rating clears it, which is
            // the only way back to unrated.
            onPressed: () => controller.send(
              album
                  ? MusicCmd.albumRate(
                      albumId: st.detailId,
                      stars: st.detailStars == i ? 0 : i,
                    )
                  : MusicCmd.artistRate(
                      artistId: st.detailId,
                      stars: st.detailStars == i ? 0 : i,
                    ),
            ),
            icon: Icon(
              i <= st.detailStars ? Icons.star : Icons.star_border,
              color: i <= st.detailStars ? const Color(0xFFF59E0B) : t.nInk3,
            ),
          ),
      ],
    );
  }
}

/// The 116x38 pink pill. The one filled control on the page, because there is
/// exactly one thing you usually came here to do.
class _PlayAll extends StatelessWidget {
  const _PlayAll({required this.onTap});

  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) => Opacity(
        opacity: onTap == null ? 0.5 : 1,
        child: SizedBox(
          width: 116,
          height: 38,
          child: Material(
            color: Tokens.secMusic,
            borderRadius: BorderRadius.circular(19),
            clipBehavior: Clip.antiAlias,
            child: InkWell(
              onTap: onTap,
              child: const Row(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  Icon(Icons.play_arrow, size: 15, color: Colors.white),
                  SizedBox(width: 6),
                  Text(
                    'Play all',
                    style: TextStyle(
                      fontFamily: Tokens.fontFamily,
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      color: Colors.white,
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      );
}

/// Back, as a circle the size of the cover on the far edge of the hero. It is
/// deliberately enormous: this page covers the library, and the way out of it
/// should not be a 24px glyph in a corner.
class _BackDisc extends StatefulWidget {
  const _BackDisc({required this.onTap, this.size = _coverSide});

  final VoidCallback onTap;

  /// As wide as the cover when there is room for it. A narrow window cannot
  /// afford a 196px circle of empty space beside a 420px card.
  final double size;

  @override
  State<_BackDisc> createState() => _BackDiscState();
}

class _BackDiscState extends State<_BackDisc> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final ink = _hovered ? Colors.white : t.nInk2;
    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: t.reduceMotion
              ? Duration.zero
              : const Duration(milliseconds: 180),
          curve: Curves.easeOut,
          width: widget.size,
          height: widget.size,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: _hovered ? Tokens.secMusic : t.nChip,
            border: Border.all(
              color: _hovered ? Tokens.secMusic : t.nHair,
              width: 2,
            ),
            boxShadow: _hovered
                ? const [BoxShadow(color: Color(0xAAEC4899), blurRadius: 34)]
                : null,
          ),
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              Icon(Icons.chevron_left, size: _hovered ? 56 : 48, color: ink),
              const SizedBox(height: 4),
              Text(
                'Back',
                style: TextStyle(
                  fontFamily: Tokens.fontFamily,
                  fontSize: 16,
                  fontWeight: FontWeight.w700,
                  color: ink,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _Songs extends StatelessWidget {
  const _Songs({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) => Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          _Head(st.detailKind == 'artist' ? 'Songs' : 'Tracks'),
          const SizedBox(height: 8),
          _Tracks(controller: controller, st: st),
        ],
      );
}

class _ArtistAlbums extends StatelessWidget {
  const _ArtistAlbums({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    if (st.detailAlbums.isEmpty) return const SizedBox.shrink();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const _Head('Albums'),
        const SizedBox(height: 10),
        MusicGrid(
          count: st.detailAlbums.length,
          // Two, always: this is the 30% column, and letting it reflow would
          // make an artist with four albums look different from one with five.
          minCols: 2,
          maxCols: 2,
          inset: 6,
          builder: (context, i) {
            final a = st.detailAlbums[i];
            return MusicCard(
              controller: controller,
              title: a.title,
              subtitle: a.subtitle,
              artKind: 'album',
              artKey: a.key,
              direct: a.art,
              count: a.count,
              loved: a.loved,
              stars: a.stars,
              onFav: () => controller.send(MusicCmd.albumFav(albumId: a.id)),
              onRate: (n) =>
                  controller.send(MusicCmd.albumRate(albumId: a.id, stars: n)),
              onTap: () => controller.send(MusicCmd.openAlbum(albumId: a.id)),
            );
          },
        ),
      ],
    );
  }
}

class _Tracks extends StatelessWidget {
  const _Tracks({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tracks = st.detailTracks;
    if (tracks.isEmpty) {
      return Text('No tracks.', style: TextStyle(fontSize: 13, color: t.nInk2));
    }
    // A playlist has an order the user chose, so its rows are draggable; an
    // album's order is the album's and dragging it would mean nothing.
    if (st.detailKind == 'playlist') {
      return ReorderableListView.builder(
        shrinkWrap: true,
        physics: const NeverScrollableScrollPhysics(),
        itemCount: tracks.length,
        // onReorderItem, unlike the deprecated onReorder, already accounts for
        // the lifted row, so `to` is the destination the store should store.
        onReorderItem: (from, to) => controller.send(MusicCmd.playlistMove(
          playlistId: st.detailId,
          from: from,
          to: to,
        )),
        itemBuilder: (_, i) => TrackRow(
          key: ValueKey(tracks[i].itemId),
          controller: controller,
          track: tracks[i],
          index: i,
          draggable: true,
          onPlay: () => controller.playFrom(tracks, i, st.detailKind),
          onQueue: () => controller.queueAll([tracks[i]]),
          onRemove: () => controller.send(MusicCmd.playlistRemove(
            playlistId: st.detailId,
            itemId: tracks[i].itemId,
          )),
        ),
      );
    }
    return Column(
      children: [
        for (var i = 0; i < tracks.length; i++) ...[
          if (i > 0) const SizedBox(height: 2),
          TrackRow(
            controller: controller,
            track: tracks[i],
            index: i,
            // An album's own art is the hero's; repeating it on every row is
            // twelve copies of one picture.
            showArt: st.detailKind != 'album',
            onPlay: () => controller.playFrom(tracks, i, st.detailKind),
            onQueue: () => controller.queueAll([tracks[i]]),
          ),
        ],
      ],
    );
  }
}

class _Head extends StatelessWidget {
  const _Head(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Text(
        text,
        style: TextStyle(
          fontFamily: Tokens.fontFamily,
          fontSize: 16,
          fontWeight: FontWeight.w700,
          color: context.tokens.nInk,
        ),
      );
}
