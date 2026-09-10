// The album / artist / genre / playlist / folder page.
//
// One widget for five kinds, because they are the same page in
// ui/page_music.slint too: a 244px hero — cover, an outlined info card, and a
// round Back the size of the cover — over a tracklist. The artist is the one
// branch: its page is 70/30, songs on the left and its albums on the right.

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart' show Int64List;

import '../../design/pick.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';
import 'music_widgets.dart';
import 'smart_editor.dart';

/// Everything in the hero is sized off this. Slint fixes it at 244 with a
/// 196px cover inside, and the proportion is the design: the cover is big
/// enough to be the subject and the card beside it is exactly as tall.
const double _heroHeight = 244;
const double _coverSide = 196;

/// The artwork inside the info box, which is the box minus its own padding.
/// The hero keeps the height it always had; the cover moved in rather than
/// growing it.
const double _artSide = _coverSide - 36;

/// How a detail page's track list is ordered. `natural` is the order the
/// bridge sent -- disc/track for an album, the stored order for a playlist --
/// and is the only one under which disc grouping means anything.
enum _Sort { natural, title, artist, plays, longest }

extension on _Sort {
  String get label => switch (this) {
        _Sort.natural => 'Track order',
        _Sort.title => 'Title',
        _Sort.artist => 'Artist',
        _Sort.plays => 'Plays',
        _Sort.longest => 'Length',
      };
}

class DetailPage extends StatefulWidget {
  const DetailPage({super.key, required this.controller});

  final MusicController controller;

  @override
  State<DetailPage> createState() => _DetailPageState();
}

class _DetailPageState extends State<DetailPage> {
  _Sort _sort = _Sort.natural;
  String _filter = '';
  bool _lovedOnly = false;

  /// Item ids, not indices: the visible list is re-sorted and re-filtered
  /// underneath a selection, and a set of positions would silently come to mean
  /// different rows.
  final Set<int> _selected = {};

  /// Where a shift-click measures from -- the last row picked without one.
  int? _anchor;

  /// Ctrl-click adds one, shift-click takes everything between.
  void _select(List<Track> shown, int index, {required bool range}) {
    setState(() {
      final id = shown[index].itemId;
      if (range && _anchor != null) {
        final from = shown.indexWhere((t) => t.itemId == _anchor);
        if (from >= 0) {
          final lo = from < index ? from : index;
          final hi = from < index ? index : from;
          for (var i = lo; i <= hi; i++) {
            _selected.add(shown[i].itemId);
          }
          return;
        }
      }
      if (!_selected.remove(id)) {
        _selected.add(id);
      }
      // The anchor follows the last plain pick, so a shift-click after an
      // unpick measures from where the pointer actually was.
      _anchor = id;
    });
  }

  void _clearSelection() {
    if (_selected.isEmpty) return;
    setState(() {
      _selected.clear();
      _anchor = null;
    });
  }

  /// The rows to draw: the bridge's list, filtered and reordered.
  ///
  /// Sorting happens in Dart rather than as another bridge command because the
  /// whole list is already here -- a round trip to reorder a hundred rows that
  /// are in memory would be slower than the sort and would lose the scroll
  /// position on the way back.
  List<Track> _visible(List<Track> all) {
    final q = _filter.trim().toLowerCase();
    var out = all.where((t) {
      if (_lovedOnly && !t.loved) return false;
      if (q.isEmpty) return true;
      return t.title.toLowerCase().contains(q) ||
          t.artist.toLowerCase().contains(q) ||
          t.album.toLowerCase().contains(q);
    }).toList();
    switch (_sort) {
      case _Sort.natural:
        break;
      case _Sort.title:
        out.sort(
            (a, b) => a.title.toLowerCase().compareTo(b.title.toLowerCase()));
      case _Sort.artist:
        out.sort(
            (a, b) => a.artist.toLowerCase().compareTo(b.artist.toLowerCase()));
      case _Sort.plays:
        out.sort((a, b) => b.playCount.compareTo(a.playCount));
      case _Sort.longest:
        out.sort((a, b) => b.durationS.compareTo(a.durationS));
    }
    return out;
  }

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    final st = controller.state;
    if (st == null) return const SizedBox.shrink();
    final all = st.detailTracks;
    final tracks = _visible(all);
    final artist = st.detailKind == 'artist';
    // Grouping is an album-order idea: under any other sort the discs
    // interleave and a divider would be drawing a boundary that is not there.
    final grouped = _sort == _Sort.natural;

    // The bar floats over the page rather than pushing it: a selection is
    // something you are holding while you look at the list, and a list that
    // jumps when you pick a row makes picking the next one harder.
    return Stack(
      children: [
        _scroller(context, st, all, tracks, artist, grouped),
        if (_selected.isNotEmpty)
          Positioned(
            left: 36,
            right: 36,
            bottom: 16,
            child: _BulkBar(
              controller: controller,
              tracks: tracks.where((t) => _selected.contains(t.itemId)).toList(),
              onClear: _clearSelection,
            ),
          ),
      ],
    );
  }

  Widget _scroller(
    BuildContext context,
    MusicState st,
    List<Track> all,
    List<Track> tracks,
    bool artist,
    bool grouped,
  ) {
    final controller = widget.controller;
    return CustomScrollView(
      slivers: [
        SliverToBoxAdapter(child: _Hero(controller: controller, st: st)),
        // Pinned, so the title, Play and the controls stay reachable however
        // far down a two-hundred-track artist page you are. The hero itself
        // scrolls away above it rather than shrinking -- same result, without
        // rebuilding the hero as a FlexibleSpaceBar.
        SliverPersistentHeader(
          pinned: true,
          delegate: _BarDelegate(
            height: 52,
            child: _DetailBar(
              controller: controller,
              st: st,
              sort: _sort,
              filter: _filter,
              lovedOnly: _lovedOnly,
              shown: tracks.length,
              total: all.length,
              onSort: (v) => setState(() => _sort = v),
              onFilter: (v) => setState(() => _filter = v),
              onLovedOnly: (v) => setState(() => _lovedOnly = v),
            ),
          ),
        ),
        const SliverToBoxAdapter(child: SizedBox(height: 14)),
        if (all.isEmpty && st.detailAlbums.isEmpty)
          const SliverToBoxAdapter(
            child: Padding(
              padding: EdgeInsets.symmetric(vertical: 40),
              child: MusicEmpty(
                icon: Icons.music_off_outlined,
                title: 'Nothing in here',
                body: 'Every track this belonged to has been removed or is '
                    'missing from disk.',
              ),
            ),
          )
        else if (tracks.isEmpty)
          SliverToBoxAdapter(
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 40),
              child: MusicEmpty(
                icon: Icons.search_off,
                title: 'Nothing matches',
                body: 'No track here matches that filter.',
                action: (
                  'Clear the filter',
                  () {
                    setState(() {
                      _filter = '';
                      _lovedOnly = false;
                    });
                  }
                ),
              ),
            ),
          )
        else if (artist)
          SliverToBoxAdapter(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(36, 0, 36, 24),
              child: LayoutBuilder(
                builder: (context, box) {
                  final songs = Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      _TopTracks(controller: controller, st: st, all: all),
                      _Songs(controller: controller, st: st, tracks: tracks),
                    ],
                  );
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
            ),
          )
        else
          SliverToBoxAdapter(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(36, 0, 36, 24),
              child: _Tracks(
                controller: controller,
                st: st,
                tracks: tracks,
                grouped: grouped,
                selected: _selected,
                onSelect: (i, {required bool range}) =>
                    _select(tracks, i, range: range),
              ),
            ),
          ),
        // Other editions and the rest of the artist's work on an album page;
        // the records they only guest on for an artist. Same row of tiles
        // either way, so the heading comes from Rust with the cards.
        for (final shelf in st.detailShelves)
          SliverToBoxAdapter(
            child: _Shelf(controller: controller, shelf: shelf),
          ),
        if (artist)
          SliverToBoxAdapter(
            child: _SimilarArtists(controller: controller, artistId: st.detailId),
          ),
        // Room under the last shelf for the bulk bar, which floats over the
        // foot of the page and would otherwise cover the final row.
        if (_selected.isNotEmpty) const SliverToBoxAdapter(child: SizedBox(height: 70)),
      ],
    );
  }
}




/// Label, catalogue number, exact release date, source format, album gain and
/// when it entered the library.
///
/// Every one of these was either tagged in the file or known to the library
/// already, and none of it was shown. Only the rows that have a value are
/// drawn: a panel of six "unknown"s is worse than a panel of two facts.
class _Details extends StatefulWidget {
  const _Details({required this.rows});

  final List<MetaRow> rows;

  @override
  State<_Details> createState() => _DetailsState();
}

class _DetailsState extends State<_Details> {
  bool _open = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        InkWell(
          borderRadius: BorderRadius.circular(6),
          onTap: () => setState(() => _open = !_open),
          child: Padding(
            padding: const EdgeInsets.symmetric(vertical: 4),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  'Details',
                  style: TextStyle(
                    fontFamily: Tokens.fontFamily,
                    fontSize: 11.5,
                    fontWeight: FontWeight.w700,
                    color: t.nInk3,
                  ),
                ),
                Icon(_open ? Icons.expand_less : Icons.expand_more,
                    size: 16, color: t.nInk3),
              ],
            ),
          ),
        ),
        if (_open)
          Padding(
            padding: const EdgeInsets.only(bottom: 2),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                for (final row in widget.rows)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 2),
                    child: Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        SizedBox(
                          width: 82,
                          child: Text(
                            row.label,
                            style: TextStyle(fontSize: 11.5, color: t.nInk3),
                          ),
                        ),
                        Expanded(
                          child: Text(
                            row.value,
                            style: TextStyle(
                              fontFamily: Tokens.fontFamily,
                              fontSize: 11.5,
                              color: t.nInk2,
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
              ],
            ),
          ),
      ],
    );
  }
}




/// What you can do to a handful of rows at once.
///
/// Retagging fifty mislabelled tracks used to be fifty separate trips through
/// the manager. Delete asks first and names the count, because it is the one
/// action here that cannot be undone and the one where a mis-shift-click is
/// expensive.
class _BulkBar extends StatelessWidget {
  const _BulkBar({
    required this.controller,
    required this.tracks,
    required this.onClear,
  });

  final MusicController controller;
  final List<Track> tracks;
  final VoidCallback onClear;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final n = tracks.length;
    return Material(
      elevation: 8,
      borderRadius: BorderRadius.circular(14),
      color: t.panel,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 8),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(14),
          border: Border.all(color: Tokens.secMusic.withValues(alpha: 0.4)),
        ),
        child: Row(
          children: [
            Text(
              n == 1 ? '1 selected' : '$n selected',
              style: const TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: 12.5,
                fontWeight: FontWeight.w700,
                color: Tokens.secMusic,
              ),
            ),
            const SizedBox(width: 16),
            TextButton.icon(
              onPressed: () => controller.queueAll(tracks),
              icon: const Icon(Icons.queue_music, size: 17),
              label: const Text('Queue'),
            ),
            TextButton.icon(
              onPressed: () => addToPlaylist(context, controller, tracks),
              icon: const Icon(Icons.playlist_add, size: 17),
              label: const Text('Playlist'),
            ),
            TextButton.icon(
              onPressed: () => _retag(context),
              icon: const Icon(Icons.sell_outlined, size: 17),
              label: const Text('Tags'),
            ),
            TextButton.icon(
              onPressed: () => _delete(context),
              icon: const Icon(Icons.delete_outline, size: 17),
              label: const Text('Delete'),
            ),
            const Spacer(),
            IconButton(
              tooltip: 'Clear selection',
              iconSize: 18,
              onPressed: onClear,
              icon: const Icon(Icons.close),
            ),
          ],
        ),
      ),
    );
  }

  /// One field across the whole selection. Not title or track number: those
  /// are per-track by definition, and setting fifty files to one title is a way
  /// to lose a library rather than fix one.
  Future<void> _retag(BuildContext context) async {
    const fields = [
      ('artist', 'Artist'),
      ('album_artist', 'Album artist'),
      ('album', 'Album'),
      ('genre', 'Genre'),
      ('year', 'Year'),
    ];
    var field = fields.first.$1;
    final value = TextEditingController();
    final ok = await showDialog<bool>(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (ctx, setLocal) => AlertDialog(
          title: Text('Set a tag on ${tracks.length} tracks'),
          content: SizedBox(
            width: 400,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                DropdownButton<String>(
                  value: field,
                  isExpanded: true,
                  items: [
                    for (final (id, label) in fields)
                      DropdownMenuItem(value: id, child: Text(label)),
                  ],
                  onChanged: (v) =>
                      v == null ? null : setLocal(() => field = v),
                ),
                const SizedBox(height: 10),
                TextField(
                  controller: value,
                  autofocus: true,
                  decoration: const InputDecoration(
                    border: OutlineInputBorder(),
                    labelText: 'New value',
                  ),
                ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(ctx, false),
              child: const Text('Cancel'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(ctx, true),
              child: const Text('Apply'),
            ),
          ],
        ),
      ),
    );
    final text = value.text;
    value.dispose();
    if (ok != true) return;
    await controller.send(MusicCmd.bulkTag(
      itemIds: Int64List.fromList(tracks.map((t) => t.itemId).toList()),
      field: field,
      value: text,
    ));
  }

  Future<void> _delete(BuildContext context) async {
    final ok = await confirm(
      context,
      title: 'Delete ${tracks.length} tracks?',
      body: 'The files go from disk. This cannot be undone.',
    );
    if (!ok) return;
    for (final track in tracks) {
      await controller.send(MusicCmd.deleteTrack(itemId: track.itemId));
    }
    onClear();
  }
}

/// Four covers in a square, for a playlist with no picture of its own.
class _Collage extends StatelessWidget {
  const _Collage({required this.paths, required this.side});

  final List<String> paths;
  final double side;

  @override
  Widget build(BuildContext context) {
    final half = side / 2;
    return SizedBox(
      width: side,
      height: side,
      child: Column(
        children: [
          for (var row = 0; row < 2; row++)
            Row(
              children: [
                for (var col = 0; col < 2; col++)
                  Image.file(
                    File(paths[row * 2 + col]),
                    width: half,
                    height: half,
                    fit: BoxFit.cover,
                    // A cover that vanished between the query and the paint is
                    // one blank quarter, not a broken page.
                    errorBuilder: (_, __, ___) =>
                        SizedBox(width: half, height: half),
                  ),
              ],
            ),
        ],
      ),
    );
  }
}

/// How a genre spreads across the decades you own.
class _Decades extends StatelessWidget {
  const _Decades({required this.bars});

  final List<Tally> bars;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(top: 10),
      child: SizedBox(
        height: 56,
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.end,
          children: [
            for (final bar in bars)
              Padding(
                padding: const EdgeInsets.only(right: 6),
                child: Tooltip(
                  message: '${bar.value} tracks',
                  child: Column(
                    mainAxisAlignment: MainAxisAlignment.end,
                    children: [
                      Container(
                        width: 22,
                        // A floor, so a decade with one record is a short bar
                        // rather than a gap in the run of years.
                        height: (4 + bar.frac * 34).clamp(4.0, 38.0),
                        decoration: BoxDecoration(
                          color: Tokens.secMusic
                              .withValues(alpha: 0.35 + 0.65 * bar.frac),
                          borderRadius: const BorderRadius.vertical(
                            top: Radius.circular(3),
                          ),
                        ),
                      ),
                      const SizedBox(height: 4),
                      Text(
                        bar.label,
                        style: TextStyle(
                          fontFamily: Tokens.fontFamily,
                          fontSize: 10,
                          color: t.nInk3,
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
  }
}

/// A playlist's description, editable in place.
class _Note extends StatelessWidget {
  const _Note({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final has = st.detailNote.isNotEmpty;
    return Padding(
      padding: const EdgeInsets.only(top: 8),
      child: InkWell(
        borderRadius: BorderRadius.circular(6),
        onTap: () => _edit(context),
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: 2),
          child: Text(
            has ? st.detailNote : 'Say what this playlist is for…',
            maxLines: 3,
            overflow: TextOverflow.ellipsis,
            style: TextStyle(
              fontSize: 12.5,
              height: 1.4,
              color: has ? t.nInk2 : t.nInk3,
              fontStyle: has ? FontStyle.normal : FontStyle.italic,
            ),
          ),
        ),
      ),
    );
  }

  Future<void> _edit(BuildContext context) async {
    final field = TextEditingController(text: st.detailNote);
    final saved = await showDialog<String>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Description'),
        content: SizedBox(
          width: 420,
          child: TextField(
            controller: field,
            autofocus: true,
            maxLines: 4,
            decoration: const InputDecoration(
              border: OutlineInputBorder(),
              hintText: 'Sunday morning, gym, the drive north…',
            ),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(ctx, field.text),
            child: const Text('Save'),
          ),
        ],
      ),
    );
    field.dispose();
    if (saved == null) return;
    await controller.send(
      MusicCmd.playlistDescribe(playlistId: st.detailId, text: saved),
    );
  }
}

/// A row of album tiles under a detail page, with its own heading.
class _Shelf extends StatelessWidget {
  const _Shelf({required this.controller, required this.shelf});

  final MusicController controller;
  final Shelf shelf;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (shelf.cards.isEmpty) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.fromLTRB(36, 4, 36, 26),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            shelf.title.toUpperCase(),
            style: TextStyle(
              fontFamily: Tokens.fontFamily,
              fontSize: 11,
              fontWeight: FontWeight.w700,
              letterSpacing: 0.7,
              color: t.nInk3,
            ),
          ),
          const SizedBox(height: 12),
          SizedBox(
            height: 186,
            child: ListView.separated(
              scrollDirection: Axis.horizontal,
              itemCount: shelf.cards.length,
              separatorBuilder: (_, __) => const SizedBox(width: 14),
              itemBuilder: (_, i) {
                final card = shelf.cards[i];
                // Artists read as people, so their tile is a circle and its
                // label is centred under it -- the same rule the browse grid
                // uses, since a shelf is a grid of one row.
                final artist = shelf.kind == 'artist';
                return SizedBox(
                  width: 132,
                  child: MusicCard(
                    controller: controller,
                    title: card.title,
                    subtitle: card.subtitle,
                    artKind: shelf.kind,
                    artKey: card.key,
                    direct: card.art.isEmpty ? null : card.art,
                    count: card.count,
                    round: artist,
                    centred: artist,
                    fallback: artist
                        ? Icons.mic_none
                        : Icons.album_outlined,
                    onTap: () => controller.send(artist
                        ? MusicCmd.openArtist(artistId: card.id)
                        : MusicCmd.openAlbum(albumId: card.id)),
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

/// The artist blurb, its facts, and where it came from.
///
/// The text was already fetched and cached; it was drawn as three clipped lines
/// with no way to read the rest and no sign of its source. The facts beside it
/// come from the same MusicBrainz lookup, which used to fold them into the
/// prose — "Group · GB · since 1991" as the tail of a sentence rather than as
/// three things you can read at a glance.
class _Bio extends StatefulWidget {
  const _Bio({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  State<_Bio> createState() => _BioState();
}

/// Lines shown before "more". Three is what the box beside the portrait holds
/// without pushing the marks below the fold.
const int _kBioLines = 3;

class _BioState extends State<_Bio> {
  bool _open = false;

  @override
  void didUpdateWidget(covariant _Bio old) {
    super.didUpdateWidget(old);
    // A different artist starts collapsed, or the next page opens expanded
    // because the last one was.
    if (old.st.detailId != widget.st.detailId && _open) {
      _open = false;
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.st;
    final body = st.detailInfo.trim();

    // Only offer "more" when there is more. A one-line blurb with a dead
    // "more" under it is worse than no control at all.
    final span = TextSpan(
      text: body,
      style: const TextStyle(fontSize: 12, height: 1.4),
    );
    return LayoutBuilder(
      builder: (context, box) {
        final tp = TextPainter(
          text: span,
          maxLines: _kBioLines,
          textDirection: Directionality.of(context),
        )..layout(maxWidth: box.maxWidth);
        final clipped = tp.didExceedMaxLines;

        return Padding(
          padding: const EdgeInsets.only(top: 8),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              if (st.detailFacts.isNotEmpty)
                Padding(
                  padding: const EdgeInsets.only(bottom: 6),
                  child: Wrap(
                    spacing: 6,
                    runSpacing: 6,
                    children: [
                      for (final fact in st.detailFacts)
                        Container(
                          padding: const EdgeInsets.symmetric(
                              horizontal: 8, vertical: 3),
                          decoration: BoxDecoration(
                            color: t.nHair,
                            borderRadius: BorderRadius.circular(20),
                          ),
                          child: Text(
                            fact,
                            style: TextStyle(
                              fontFamily: Tokens.fontFamily,
                              fontSize: 11,
                              fontWeight: FontWeight.w600,
                              color: t.nInk2,
                            ),
                          ),
                        ),
                    ],
                  ),
                ),
              if (body.isNotEmpty)
                Text(
                  body,
                  maxLines: _open ? null : _kBioLines,
                  overflow:
                      _open ? TextOverflow.clip : TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: 12,
                    height: 1.4,
                    color: t.nInk2,
                  ),
                ),
              Padding(
                padding: const EdgeInsets.only(top: 4),
                child: Wrap(
                  spacing: 12,
                  children: [
                    if (clipped)
                      _BioLink(
                        label: _open ? 'less' : 'more',
                        onTap: () => setState(() => _open = !_open),
                      ),
                    // Where it came from. A blurb with no source is a claim.
                    if (st.detailMbid.isNotEmpty)
                      _BioLink(
                        label: 'MusicBrainz',
                        onTap: () => widget.controller.send(
                          MusicCmd.openArtistSource(
                            artistId: st.detailId,
                            source: 'musicbrainz',
                          ),
                        ),
                      ),
                    _BioLink(
                      label: 'Wikipedia',
                      onTap: () => widget.controller.send(
                        MusicCmd.openArtistSource(
                          artistId: st.detailId,
                          source: 'wikipedia',
                        ),
                      ),
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

class _BioLink extends StatelessWidget {
  const _BioLink({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(4),
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: 2),
          child: Text(
            label,
            style: TextStyle(
              fontFamily: Tokens.fontFamily,
              fontSize: 11.5,
              fontWeight: FontWeight.w700,
              color: context.tokens.nInk3,
            ),
          ),
        ),
      );
}

/// Artists on these shelves who sound like this one.
///
/// Local, and that is the point: the ranking is shared genres, overlapping era
/// and the distance between the two artists' fingerprints, so every name here
/// is something already owned and one tap from playing. A recommendation that
/// cannot be played is a advertisement.
///
/// Draws nothing at all when there is no answer — an unanalysed library with
/// no genre tags has nothing to go on, and an empty "Similar artists" heading
/// is worse than no heading.
class _SimilarArtists extends StatefulWidget {
  const _SimilarArtists({required this.controller, required this.artistId});

  final MusicController controller;
  final int artistId;

  @override
  State<_SimilarArtists> createState() => _SimilarArtistsState();
}

class _SimilarArtistsState extends State<_SimilarArtists> {
  late Future<List<BrowseCard>> _future = _load();

  Future<List<BrowseCard>> _load() async {
    try {
      return await musicSimilarArtists(artistId: widget.artistId, limit: 8);
    } catch (_) {
      return const [];
    }
  }

  @override
  void didUpdateWidget(covariant _SimilarArtists old) {
    super.didUpdateWidget(old);
    // The page is one widget reused across artists, so a new id has to re-ask.
    if (old.artistId != widget.artistId) _future = _load();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return FutureBuilder<List<BrowseCard>>(
      future: _future,
      builder: (context, snap) {
        final cards = snap.data ?? const <BrowseCard>[];
        if (cards.isEmpty) return const SizedBox.shrink();
        return Padding(
          padding: const EdgeInsets.fromLTRB(36, 4, 36, 30),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                'SIMILAR ARTISTS',
                style: TextStyle(
                  fontFamily: Tokens.fontFamily,
                  fontSize: 11,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 0.7,
                  color: t.nInk3,
                ),
              ),
              const SizedBox(height: 4),
              Text(
                'From your own library',
                style: TextStyle(fontSize: 12, color: t.nInk3),
              ),
              const SizedBox(height: 12),
              SizedBox(
                height: 176,
                child: ListView.separated(
                  scrollDirection: Axis.horizontal,
                  itemCount: cards.length,
                  separatorBuilder: (_, __) => const SizedBox(width: 14),
                  itemBuilder: (_, i) {
                    final card = cards[i];
                    return SizedBox(
                      width: 128,
                      child: MusicCard(
                        controller: widget.controller,
                        title: card.title,
                        subtitle: card.subtitle,
                        artKind: 'artist',
                        artKey: card.key,
                        direct: card.art.isEmpty ? null : card.art,
                        round: true,
                        centred: true,
                        fallback: Icons.mic_none,
                        onTap: () => widget.controller
                            .send(MusicCmd.openArtist(artistId: card.id)),
                      ),
                    );
                  },
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

/// A fixed-height pinned header. `SliverAppBar` carries a whole toolbar
/// contract this does not want -- a leading widget, a title, actions -- for a
/// bar that is one row of its own controls.
class _BarDelegate extends SliverPersistentHeaderDelegate {
  _BarDelegate({required this.child, required this.height});

  final Widget child;
  final double height;

  @override
  double get minExtent => height;
  @override
  double get maxExtent => height;

  @override
  Widget build(BuildContext context, double shrinkOffset, bool overlaps) =>
      SizedBox.expand(child: child);

  @override
  bool shouldRebuild(_BarDelegate old) =>
      old.child != child || old.height != height;
}

/// "Disc 2 ————— 39:52". Only drawn on a set that actually has more than one.
class _DiscHead extends StatelessWidget {
  const _DiscHead({required this.disc, required this.seconds});

  final int disc;
  final double seconds;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final style = TextStyle(
      fontSize: 10,
      fontWeight: FontWeight.w800,
      letterSpacing: 1,
      color: t.nInk2,
    );
    return Row(
      children: [
        Text('DISC $disc', style: style),
        const SizedBox(width: 10),
        Expanded(child: Divider(height: 1, color: t.outline)),
        const SizedBox(width: 10),
        Text(fmtClock(seconds), style: style),
      ],
    );
  }
}

/// Track count, total runtime, the years it spans and the best copy on disk.
///
/// All four are derived from the track list that is already here -- no new
/// bridge field, no second query.
class _Facts extends StatelessWidget {
  const _Facts({required this.tracks});

  final List<Track> tracks;

  /// Ranked worst to best, so `max` over the list picks the best copy present.
  static const _formats = [
    'mp3',
    'm4a',
    'aac',
    'ogg',
    'opus',
    'alac',
    'flac',
    'wav'
  ];

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (tracks.isEmpty) return const SizedBox.shrink();
    final runtime = tracks.fold<double>(0, (a, x) => a + x.durationS);
    final years = tracks.map((x) => x.year).where((y) => y > 0).toList()
      ..sort();
    final best = tracks
        .map((x) => x.path.split('.').last.toLowerCase())
        .map(_formats.indexOf)
        .fold(-1, (a, b) => b > a ? b : a);

    final parts = <String>[
      '${tracks.length} track${tracks.length == 1 ? '' : 's'}',
      fmtClock(runtime),
      if (years.isNotEmpty)
        years.first == years.last
            ? '${years.first}'
            : '${years.first}–${years.last}',
      if (best >= 0) _formats[best].toUpperCase(),
    ];

    return Padding(
      padding: const EdgeInsets.only(top: 6),
      child: Text(
        parts.join('  ·  '),
        style: TextStyle(
            fontSize: 11.5,
            color: t.nInk2,
            fontFeatures: const [FontFeature.tabularFigures()]),
      ),
    );
  }
}

/// The pinned control row: what the page is, what to do with it, and how to
/// narrow it. One bar for all five kinds, so the actions stop changing
/// depending on which sort of page you happen to have open.
class _DetailBar extends StatelessWidget {
  const _DetailBar({
    required this.controller,
    required this.st,
    required this.sort,
    required this.filter,
    required this.lovedOnly,
    required this.shown,
    required this.total,
    required this.onSort,
    required this.onFilter,
    required this.onLovedOnly,
  });

  final MusicController controller;
  final MusicState st;
  final _Sort sort;
  final String filter;
  final bool lovedOnly;
  final int shown;
  final int total;
  final ValueChanged<_Sort> onSort;
  final ValueChanged<String> onFilter;
  final ValueChanged<bool> onLovedOnly;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tracks = st.detailTracks;
    return Material(
      color: t.nCanvas,
      child: DecoratedBox(
        decoration: BoxDecoration(
          border: Border(bottom: BorderSide(color: t.outline)),
        ),
        child: LayoutBuilder(builder: (context, box) {
          final wide = box.maxWidth > 720;
          return Padding(
            padding: const EdgeInsets.symmetric(horizontal: 36),
            child: Row(
              children: [
                // The title only appears once the hero has scrolled off, which is
                // the whole reason the bar is pinned.
                Flexible(
                  child: Text(
                    st.detailTitle,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 13.5,
                        fontWeight: FontWeight.w600,
                        color: t.nInk),
                  ),
                ),
                const SizedBox(width: 14),
                _BarBtn(
                  icon: Icons.play_arrow_rounded,
                  tip: 'Play all',
                  onTap: tracks.isEmpty
                      ? null
                      : () => controller.playFrom(tracks, 0, st.detailKind),
                ),
                _BarBtn(
                  icon: Icons.shuffle,
                  tip: 'Shuffle',
                  onTap: tracks.isEmpty
                      ? null
                      : () async {
                          if (!st.shuffle) {
                            await controller
                                .send(const MusicCmd.toggleShuffle());
                          }
                          await controller.playFrom(tracks, 0, st.detailKind);
                        },
                ),
                _BarBtn(
                  icon: Icons.queue_music,
                  tip: 'Add all to queue',
                  onTap:
                      tracks.isEmpty ? null : () => controller.queueAll(tracks),
                ),
                _BarBtn(
                  icon: lovedOnly ? Icons.favorite : Icons.favorite_border,
                  tip: lovedOnly ? 'Showing loved only' : 'Loved only',
                  active: lovedOnly,
                  onTap: () => onLovedOnly(!lovedOnly),
                ),
                // Only on a smart playlist. A manual one has no rules, and
                // offering the editor on it would turn it into a smart one the
                // moment anything was saved.
                if (st.detailKind == 'playlist' && st.detailIsSmart)
                  _BarBtn(
                    icon: Icons.tune,
                    tip: 'Rules',
                    onTap: () => editSmartPlaylist(
                      context,
                      controller,
                      playlistId: st.detailId,
                      name: st.detailTitle,
                    ),
                  ),
                const Spacer(),
                // Says what the filter did, so a short list never looks like a
                // page that failed to load. Drops out first when the bar is
                // tight: it is the least load-bearing thing on the row.
                if (shown != total && wide)
                  Padding(
                    padding: const EdgeInsets.only(right: 10),
                    child: Text('$shown of $total',
                        style: TextStyle(fontSize: 11, color: t.nInk2)),
                  ),
                // Flexible, not a fixed 190: on a narrow window the fixed width
                // plus four buttons and the sort label was wider than the bar,
                // and a Row overflows rather than shrinking anything for you.
                Flexible(
                  child: ConstrainedBox(
                    constraints:
                        const BoxConstraints(maxWidth: 190, minWidth: 90),
                    child: SizedBox(
                      height: 32,
                      child: TextField(
                        onChanged: onFilter,
                        style: TextStyle(fontSize: 12.5, color: t.nInk),
                        decoration: InputDecoration(
                          isDense: true,
                          hintText: 'Filter…',
                          hintStyle: TextStyle(fontSize: 12.5, color: t.nInk2),
                          prefixIcon:
                              Icon(Icons.search, size: 15, color: t.nInk2),
                          prefixIconConstraints:
                              const BoxConstraints(minWidth: 30, minHeight: 30),
                          contentPadding: const EdgeInsets.symmetric(
                              horizontal: 8, vertical: 6),
                          border: OutlineInputBorder(
                            borderRadius: BorderRadius.circular(7),
                            borderSide: BorderSide(color: t.outline),
                          ),
                          enabledBorder: OutlineInputBorder(
                            borderRadius: BorderRadius.circular(7),
                            borderSide: BorderSide(color: t.outline),
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
                const SizedBox(width: 8),
                PopupMenuButton<_Sort>(
                  tooltip: 'Sort',
                  initialValue: sort,
                  onSelected: onSort,
                  itemBuilder: (_) => [
                    for (final v in _Sort.values)
                      CheckedPopupMenuItem(
                        value: v,
                        checked: v == sort,
                        child: Text(v.label),
                      ),
                  ],
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Icon(Icons.swap_vert, size: 16, color: t.nInk2),
                      // The icon alone still opens the menu; the words are the
                      // part a narrow bar can do without.
                      if (wide) ...[
                        const SizedBox(width: 4),
                        Text(sort.label,
                            style: TextStyle(fontSize: 11.5, color: t.nInk2)),
                      ],
                    ],
                  ),
                ),
              ],
            ),
          );
        }),
      ),
    );
  }
}

class _BarBtn extends StatelessWidget {
  const _BarBtn({
    required this.icon,
    required this.tip,
    required this.onTap,
    this.active = false,
  });

  final IconData icon;
  final String tip;
  final VoidCallback? onTap;
  final bool active;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return IconButton(
      onPressed: onTap,
      tooltip: tip,
      iconSize: 17,
      visualDensity: VisualDensity.compact,
      constraints: const BoxConstraints(minWidth: 32, minHeight: 32),
      padding: EdgeInsets.zero,
      icon: Icon(icon,
          color: active ? Tokens.secMusic : (onTap == null ? t.nInk2 : t.nInk)),
    );
  }
}

/// What you actually play, at the top of an artist page.
///
/// Ranked by `play_count`, which has always been on `Track` and shipped to
/// Dart -- nothing read it here, so the page opened on an alphabetical dump of
/// everything instead.
class _TopTracks extends StatelessWidget {
  const _TopTracks({
    required this.controller,
    required this.st,
    required this.all,
  });

  final MusicController controller;
  final MusicState st;
  final List<Track> all;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final played = all.where((x) => x.playCount > 0).toList()
      ..sort((a, b) => b.playCount.compareTo(a.playCount));
    // Fewer than three plays across a whole artist is not a ranking, it is
    // noise -- better to show nothing than to crown an accident.
    if (played.length < 3) return const SizedBox.shrink();
    final top = played.take(5).toList();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Padding(
          padding: const EdgeInsets.only(bottom: 6),
          child: Text('MOST PLAYED',
              style: TextStyle(
                  fontSize: 10.5,
                  fontWeight: FontWeight.w800,
                  letterSpacing: 1,
                  color: t.nInk2)),
        ),
        for (var i = 0; i < top.length; i++) ...[
          if (i > 0) const SizedBox(height: 2),
          TrackRow(
            controller: controller,
            track: top[i],
            index: i,
            onPlay: () => controller.playFrom(top, i, st.detailKind),
            onQueue: () => controller.queueAll([top[i]]),
          ),
        ],
        const SizedBox(height: 20),
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
          // One box, the width of the page, with only Back outside it. The
          // cover lives inside rather than beside: three fixed blocks in a Row
          // could not shrink, and everything under about a thousand pixels
          // overflowed.
          child: Row(
            children: [
              Expanded(
                child: Container(
                  height: _coverSide,
                  padding: const EdgeInsets.all(18),
                  decoration: BoxDecoration(
                    color: t.nCard,
                    borderRadius: BorderRadius.circular(14),
                    border: Border.all(color: t.nHair),
                  ),
                  child: Row(
                    children: [
                      _Cover(
                        controller: controller,
                        st: st,
                        child: Container(
                          width: _artSide,
                          height: _artSide,
                          decoration: BoxDecoration(
                            borderRadius: BorderRadius.circular(
                                artist ? _artSide / 2 : 12),
                            border: Border.all(color: t.nHair),
                            boxShadow: const [
                              BoxShadow(
                                  color: Color(0xAA000000), blurRadius: 18),
                            ],
                          ),
                          clipBehavior: Clip.antiAlias,
                          // Four covers when a playlist has no picture of its
                          // own, which is the only kind with nothing to show.
                          // Rust sends four or none: two covers in a four-up
                          // grid is a broken tile, not a collage.
                          child: st.detailCollage.length == 4
                              ? _Collage(paths: st.detailCollage, side: _artSide)
                              : MusicArt(
                                  controller: controller,
                                  kind: st.detailKind,
                                  artKey: st.detailKind == 'album' || artist
                                      ? '${st.detailId}'
                                      : st.detailKey,
                                  direct: st.detailArt,
                                  size: _artSide,
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
                      const SizedBox(width: 18),
                      Expanded(
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
                            const SizedBox(height: 6),
                            Text(
                              st.detailTitle,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                fontFamily: Tokens.fontFamily,
                                fontSize: 30,
                                fontWeight: FontWeight.w800,
                                color: t.nInk,
                              ),
                            ),
                            const SizedBox(height: 4),
                            // On an album page the subtitle is who made it, and
                            // the artist has a page of their own — so it is a
                            // link, the same way the player bar's second line
                            // is. `_Facts` rides the same line: it is one
                            // sentence of numbers and it was costing a row.
                            Row(
                              children: [
                                Flexible(
                                  child: st.detailKind == 'album' &&
                                          st.detailArtistId != 0 &&
                                          st.detailSubtitle.isNotEmpty
                                      ? _ArtistLink(
                                          name: st.detailSubtitle,
                                          onTap: () => controller.send(
                                              MusicCmd.openArtist(
                                                  artistId: st.detailArtistId)),
                                        )
                                      : Text(
                                          st.detailSubtitle,
                                          maxLines: 1,
                                          overflow: TextOverflow.ellipsis,
                                          style: TextStyle(
                                              fontSize: 14, color: t.nInk2),
                                        ),
                                ),
                                Flexible(
                                  child: _Facts(tracks: st.detailTracks),
                                ),
                              ],
                            ),
                            const Spacer(),
                            // Everything else, on one line.
                            //
                            // It used to be five stacked blocks — facts,
                            // details, decades, a description, a biography —
                            // inside a box of fixed height, which is exactly
                            // how the box came to overflow. Each is now a chip
                            // that opens what it is about, so the box holds the
                            // same information without pretending to show all
                            // of it at once.
                            _Actions(controller: controller, st: st),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
              ),
              const SizedBox(width: 18),
              _BackDisc(
                size: 72,
                // One step, not all the way out: arriving here from an artist
                // page should go back to that artist, and only a page opened
                // straight from the grid closes the detail.
                onTap: controller.goBack,
              ),
            ],
          ),
        ),
      ),
    );
  }
}


/// Everything below the title, on one line.
///
/// The info box is a fixed height and the blocks that used to stack here —
/// the record's details, a genre's decades, a playlist's description, an
/// artist's biography — have no fixed height between them, which is how the
/// box came to overflow. Each is a chip that opens what it is about instead.
/// The row scrolls sideways rather than wrapping: a second line here is the
/// bug this is fixing.
class _Actions extends StatelessWidget {
  const _Actions({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final tracks = st.detailTracks;
    final marked = st.detailKind == 'album' || st.detailKind == 'artist';
    return SingleChildScrollView(
      scrollDirection: Axis.horizontal,
      child: Row(
        children: [
          _PlayAll(
            onTap: tracks.isEmpty
                ? null
                : () => controller.playFrom(tracks, 0, st.detailKind),
          ),
          const SizedBox(width: 8),
          DetailActionBtn(
            icon: Icons.shuffle,
            label: 'Shuffle',
            onTap: () async {
              if (tracks.isEmpty) return;
              if (!st.shuffle) {
                await controller.send(const MusicCmd.toggleShuffle());
              }
              await controller.playFrom(tracks, 0, st.detailKind);
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
              icon: Icons.notes,
              label: 'Description',
              onTap: () => _sheet(
                context,
                st.detailTitle,
                _Note(controller: controller, st: st),
              ),
            ),
            const SizedBox(width: 8),
            DetailActionBtn(
              icon: Icons.delete_outline,
              label: 'Delete',
              tint: Tokens.error,
              onTap: () => confirmThen(
                context,
                controller,
                title: 'Delete this playlist?',
                body: '“${st.detailTitle}” and its order go. The tracks in it '
                    'stay in the library.',
                action: 'Delete playlist',
                cmd: MusicCmd.playlistDelete(playlistId: st.detailId),
              ),
            ),
          ],
          if (st.detailMeta.isNotEmpty) ...[
            const SizedBox(width: 8),
            DetailActionBtn(
              icon: Icons.info_outline,
              label: 'Details',
              onTap: () => _sheet(
                context,
                st.detailTitle,
                _Details(rows: st.detailMeta),
              ),
            ),
          ],
          if (st.detailBars.isNotEmpty) ...[
            const SizedBox(width: 8),
            DetailActionBtn(
              icon: Icons.bar_chart,
              label: 'By decade',
              onTap: () => _sheet(
                context,
                st.detailTitle,
                _Decades(bars: st.detailBars),
              ),
            ),
          ],
          if (st.detailInfo.isNotEmpty || st.detailFacts.isNotEmpty) ...[
            const SizedBox(width: 8),
            DetailActionBtn(
              icon: Icons.person_outline,
              label: 'About',
              onTap: () => _sheet(
                context,
                st.detailTitle,
                _Bio(controller: controller, st: st),
              ),
            ),
          ],
          if (marked) ...[
            const SizedBox(width: 10),
            _Marks(controller: controller, st: st),
          ],
        ],
      ),
    );
  }

  /// One dialog shape for every chip on the row. The widgets inside it are the
  /// same ones that used to sit in the box, unchanged — they were never wrong,
  /// they just had nowhere to be.
  static Future<void> _sheet(
      BuildContext context, String title, Widget child) async {
    await showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: Text(title, maxLines: 1, overflow: TextOverflow.ellipsis),
        content: SizedBox(
          width: 460,
          child: SingleChildScrollView(child: child),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(),
            child: const Text('Close'),
          ),
        ],
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
            // Five identical icons in a row are five identical announcements
            // without this, and the whole control is which one you press.
            tooltip: i == 1 ? '1 star' : '$i stars',
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
  const _Songs({
    required this.controller,
    required this.st,
    required this.tracks,
  });

  final MusicController controller;
  final MusicState st;
  final List<Track> tracks;

  @override
  Widget build(BuildContext context) => Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          _Head(st.detailKind == 'artist' ? 'Songs' : 'Tracks'),
          const SizedBox(height: 8),
          // An artist's songs span albums, so disc numbers there are several
          // records' numbering side by side and mean nothing as a grouping.
          _Tracks(
              controller: controller, st: st, tracks: tracks, grouped: false),
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
  const _Tracks({
    required this.controller,
    required this.st,
    required this.tracks,
    required this.grouped,
    this.selected = const {},
    this.onSelect,
  });

  /// Item ids currently picked, and how a row reports being picked.
  final Set<int> selected;
  final void Function(int index, {required bool range})? onSelect;

  final MusicController controller;
  final MusicState st;

  /// Already filtered and sorted by the page.
  final List<Track> tracks;

  /// Whether disc dividers may be drawn -- only true in the bridge's own
  /// order, which is the only order in which discs are contiguous.
  final bool grouped;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
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
        // Reordering only means something in the stored order; a filtered or
        // re-sorted view would send the wrong indices to `playlistMove`.
        buildDefaultDragHandles: grouped,
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
          selected: selected.contains(tracks[i].itemId),
          onSelect: onSelect == null
              ? null
              : ({required bool range}) => onSelect!(i, range: range),
          onRemove: () => controller.send(MusicCmd.playlistRemove(
            playlistId: st.detailId,
            itemId: tracks[i].itemId,
          )),
        ),
      );
    }
    // A divider is only worth drawing when there is more than one disc to
    // separate; a single-disc album, and every untagged one, looks exactly as
    // it did before.
    final discs = grouped
        ? tracks.map((x) => x.discNo).where((d) => d > 0).toSet()
        : const <int>{};
    final showDiscs = discs.length > 1;

    return Column(
      children: [
        for (var i = 0; i < tracks.length; i++) ...[
          if (showDiscs &&
              (i == 0 || tracks[i].discNo != tracks[i - 1].discNo)) ...[
            if (i > 0) const SizedBox(height: 14),
            _DiscHead(
              disc: tracks[i].discNo,
              seconds: tracks
                  .where((x) => x.discNo == tracks[i].discNo)
                  .fold<double>(0, (a, x) => a + x.durationS),
            ),
            const SizedBox(height: 4),
          ] else if (i > 0)
            const SizedBox(height: 2),
          TrackRow(
            controller: controller,
            track: tracks[i],
            selected: selected.contains(tracks[i].itemId),
            onSelect: onSelect == null
                ? null
                : ({required bool range}) => onSelect!(i, range: range),
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
