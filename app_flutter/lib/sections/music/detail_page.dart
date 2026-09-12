// The album / artist / genre / playlist / folder page.
//
// One widget for five kinds. The hero is one skeleton -- back and the trail
// that led here, the thumb, the info box, a side card, and the tools along its
// foot -- which each kind fills with what only it has. There is no second bar
// under it: once the hero has scrolled away a 60px strip stands in for it. The
// artist is the one branch in the list: its page is 70/30, songs on the left
// and its albums on the right.

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart' show RenderAbstractViewport;
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart' show Int64List;

import '../../design/pick.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_accent.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';
import 'music_widgets.dart';
import 'smart_editor.dart';

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
  bool _lovedOnly = false;

  /// One controller behind both filter fields, the hero's and the strip's, so
  /// what was typed into one is what the other shows.
  final _filter = TextEditingController();

  /// A decade picked on a genre page's chart -- 1990 for the nineties -- or
  /// null for all of them.
  int? _decade;

  /// A genre grouped by artist, a folder by the folders under it.
  bool _group = false;

  /// Which of the artist page's jump buttons was pressed last.
  int _jumped = 0;

  /// Which page of a playlist is showing, from 0.
  int _page = 0;

  final _scroll = ScrollController();
  final _heroKey = GlobalKey();
  final _listKey = GlobalKey();

  /// Whether the hero has scrolled away and the strip stands in for it. A
  /// notifier rather than setState: it flips mid-scroll, and the track list
  /// has no reason to rebuild when it does.
  final _stripOn = ValueNotifier(false);

  // Where the side card's links and the artist's jump buttons scroll to.
  final _topKey = GlobalKey();
  final _songsKey = GlobalKey();
  final _albumsKey = GlobalKey();
  final _similarKey = GlobalKey();
  final _shelfKeys = <String, GlobalKey>{};

  /// Asked once per page: the side card previews it and the shelf at the foot
  /// lists it, and two widgets asking separately would be two lookups.
  Future<List<BrowseCard>>? _similar;

  /// Item ids, not indices: the visible list is re-sorted and re-filtered
  /// underneath a selection, and a set of positions would silently come to mean
  /// different rows.
  final Set<int> _selected = {};

  /// Where a shift-click measures from -- the last row picked without one.
  int? _anchor;

  @override
  void initState() {
    super.initState();
    _filter.addListener(_refilter);
    _scroll.addListener(_onScroll);
    // The page is keyed on what it shows (the CrossFade in my_music_tab.dart),
    // so one state is one subject and this never has to be asked again.
    final st = widget.controller.state;
    if (st != null && st.detailKind == 'artist') {
      _similar = musicSimilarArtists(artistId: st.detailId, limit: 8)
          .catchError((Object _) => const <BrowseCard>[]);
    }
  }

  @override
  void dispose() {
    _filter.dispose();
    _scroll.dispose();
    _stripOn.dispose();
    super.dispose();
  }

  /// A new filter starts from the first page of what it matches.
  void _refilter() => setState(() => _page = 0);

  void _onScroll() {
    // Null once the hero is far enough up to have been dropped, and by then
    // the strip is already showing.
    final hero = _heroKey.currentContext?.size?.height;
    if (hero == null) return;
    // As the tools at the hero's foot go under the top edge, not before: the
    // strip carries the filter, and there should never be two of them.
    _stripOn.value = _scroll.offset > hero - 56;
  }

  /// Scroll so [key]'s widget sits just under the strip.
  void _jump(GlobalKey key) {
    final box = key.currentContext?.findRenderObject();
    if (box == null || !_scroll.hasClients) return;
    final viewport = RenderAbstractViewport.maybeOf(box);
    if (viewport == null) return;
    final to = (viewport.getOffsetToReveal(box, 0).offset - 72)
        .clamp(0.0, _scroll.position.maxScrollExtent);
    if (context.tokens.reduceMotion) {
      _scroll.jumpTo(to);
    } else {
      _scroll.animateTo(to,
          duration: const Duration(milliseconds: 380),
          curve: Curves.easeOutCubic);
    }
  }

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

  /// What a grouped list groups on: the artist on a genre page, the folder
  /// under this one on a folder page. Null when the list is not grouped.
  String Function(Track)? _groupOf(MusicState st) {
    if (!_group) return null;
    if (st.detailKind == 'genre') {
      return (t) => t.artist.isEmpty ? 'Unknown artist' : t.artist;
    }
    if (st.detailKind == 'folder') {
      final root = _trimSep(st.detailKey);
      return (t) {
        final dir = File(t.path).parent.path;
        if (dir.length <= root.length || !dir.startsWith(root)) {
          return 'In this folder';
        }
        return dir.substring(root.length + 1);
      };
    }
    return null;
  }

  /// The rows to draw: the bridge's list, filtered and reordered.
  ///
  /// Sorting happens in Dart rather than as another bridge command because the
  /// whole list is already here -- a round trip to reorder a hundred rows that
  /// are in memory would be slower than the sort and would lose the scroll
  /// position on the way back.
  List<Track> _visible(MusicState st) {
    final q = _filter.text.trim().toLowerCase();
    var out = st.detailTracks.where((t) {
      if (_lovedOnly && !t.loved) return false;
      if (_decade != null && t.year ~/ 10 * 10 != _decade) return false;
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
    final group = _groupOf(st);
    if (group != null) {
      // Bucketed rather than sorted: List.sort is not stable, and each group
      // should keep the order the sort above just gave it.
      final buckets = <String, List<Track>>{};
      for (final t in out) {
        (buckets[group(t)] ??= []).add(t);
      }
      final keys = buckets.keys.toList()
        ..sort((a, b) => a.toLowerCase().compareTo(b.toLowerCase()));
      out = [for (final k in keys) ...buckets[k]!];
    }
    return out;
  }

  GlobalKey? _editionsKey(MusicState st) {
    for (final shelf in st.detailShelves) {
      if (shelf.title.contains('edition')) {
        return _shelfKeys.putIfAbsent(shelf.title, GlobalKey.new);
      }
    }
    return null;
  }

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    final st = controller.state;
    if (st == null) return const SizedBox.shrink();
    final tracks = _visible(st);
    final editions = _editionsKey(st);

    // The bulk bar floats over the page rather than pushing it: a selection is
    // something you are holding while you look at the list, and a list that
    // jumps when you pick a row makes picking the next one harder.
    return Stack(
      children: [
        CustomScrollView(
          controller: _scroll,
          slivers: [
            SliverToBoxAdapter(
              child: _Hero(
                key: _heroKey,
                controller: controller,
                st: st,
                similar: _similar,
                decade: _decade,
                onDecade: (d) =>
                    setState(() => _decade = _decade == d ? null : d),
                onEditions: editions == null ? null : () => _jump(editions),
                onSimilar: () => _jump(_similarKey),
                tools: _tools(context, st, tracks),
              ),
            ),
            // One box for the list and the shelves under it, so every place
            // the side card links to is laid out and can be scrolled to. The
            // list was already one eager Column; a sliver per shelf only meant
            // a shelf past the cache extent had nowhere to jump.
            SliverToBoxAdapter(child: _body(context, st, tracks)),
          ],
        ),
        Positioned(
          top: 0,
          left: 0,
          right: 0,
          child: ValueListenableBuilder<bool>(
            valueListenable: _stripOn,
            builder: (context, on, _) => AnimatedSwitcher(
              duration: context.tokens.reduceMotion
                  ? Duration.zero
                  : const Duration(milliseconds: 160),
              child: on
                  ? _strip(context, st)
                  : const SizedBox.shrink(key: ValueKey('no-strip')),
            ),
          ),
        ),
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

  Widget _body(BuildContext context, MusicState st, List<Track> tracks) {
    final controller = widget.controller;
    final all = st.detailTracks;
    final artist = st.detailKind == 'artist';
    final Widget list;
    if (all.isEmpty && st.detailAlbums.isEmpty) {
      list = const Padding(
        padding: EdgeInsets.symmetric(vertical: 40),
        child: MusicEmpty(
          icon: Icons.music_off_outlined,
          title: 'Nothing in here',
          body: 'Every track this belonged to has been removed or is '
              'missing from disk.',
        ),
      );
    } else if (tracks.isEmpty) {
      list = Padding(
        padding: const EdgeInsets.symmetric(vertical: 40),
        child: MusicEmpty(
          icon: Icons.search_off,
          title: 'Nothing matches',
          body: 'No track here matches that filter.',
          action: (
            'Clear the filter',
            () {
              _filter.clear();
              setState(() {
                _lovedOnly = false;
                _decade = null;
              });
            }
          ),
        ),
      );
    } else if (artist) {
      list = LayoutBuilder(
        builder: (context, box) {
          final songs = Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              KeyedSubtree(
                key: _topKey,
                child: _TopTracks(controller: controller, st: st, all: all),
              ),
              KeyedSubtree(
                key: _songsKey,
                child: _Songs(controller: controller, st: st, tracks: tracks),
              ),
            ],
          );
          final albums = KeyedSubtree(
            key: _albumsKey,
            child: _ArtistAlbums(controller: controller, st: st),
          );
          // Under about a thousand pixels the 30% column is narrower than one
          // album tile, so the split stops paying for itself.
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
      );
    } else {
      // A playlist is paged: it is the one list here that can run to
      // thousands. Playing a row still hands the whole list to the queue.
      final paged = st.detailKind == 'playlist';
      final pages = paged ? (tracks.length / _kPlaylistPage).ceil() : 1;
      final page = _page.clamp(0, pages - 1);
      final from = paged ? page * _kPlaylistPage : 0;
      list = Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _Tracks(
            controller: controller,
            st: st,
            tracks: tracks,
            from: from,
            to: paged
                ? (from + _kPlaylistPage).clamp(0, tracks.length)
                : tracks.length,
            // Disc dividers belong to the bridge's own order: under any other
            // sort, or a grouping of the page's own, the discs interleave.
            grouped: _sort == _Sort.natural && !_group,
            groupOf: _groupOf(st),
            selected: _selected,
            onSelect: (i, {required bool range}) =>
                _select(tracks, i, range: range),
          ),
          Pager(
            page: page,
            pages: pages,
            onGo: (p) {
              setState(() => _page = p);
              // From down the page, back up to the new page's first row. From
              // the hero there is nothing to fix.
              if (_stripOn.value) {
                WidgetsBinding.instance
                    .addPostFrameCallback((_) => _jump(_listKey));
              }
            },
          ),
        ],
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        KeyedSubtree(
          key: _listKey,
          child: Padding(
              padding: const EdgeInsets.fromLTRB(36, 14, 36, 24), child: list),
        ),
        // Other editions and the rest of the artist's work on an album page;
        // the records they only guest on for an artist. Same row of tiles
        // either way, so the heading comes from Rust with the cards.
        for (final shelf in st.detailShelves)
          KeyedSubtree(
            key: _shelfKeys.putIfAbsent(shelf.title, GlobalKey.new),
            child: _Shelf(controller: controller, shelf: shelf),
          ),
        if (_similar != null)
          KeyedSubtree(
            key: _similarKey,
            child: _SimilarArtists(controller: controller, future: _similar!),
          ),
        // Room under the last shelf for the bulk bar, which floats over the
        // foot of the page and would otherwise cover the final row.
        if (_selected.isNotEmpty) const SizedBox(height: 70),
      ],
    );
  }

  Widget _sortChip(MusicState st) => PopupMenuButton<_Sort>(
        tooltip: 'Sort',
        initialValue: _sort,
        onSelected: (v) => setState(() => _sort = v),
        itemBuilder: (_) => [
          for (final v in _Sort.values)
            CheckedPopupMenuItem(
              value: v,
              checked: v == _sort,
              child: Text(_sortLabel(v, st)),
            ),
        ],
        child: _ToolChip(icon: Icons.swap_vert, label: '${_sortLabel(_sort, st)} ▾'),
      );

  /// The hero's foot: everything the pinned bar under it used to hold that
  /// the hero did not already have.
  Widget _tools(BuildContext context, MusicState st, List<Track> tracks) {
    final t = context.tokens;
    final all = st.detailTracks;
    final kind = st.detailKind;
    final noun = switch (kind) {
      'artist' => 'songs',
      'folder' => 'files',
      _ => 'tracks',
    };
    final discs = all.map((x) => x.discNo).where((d) => d > 0).toSet().length;
    final count = StringBuffer(tracks.length == all.length
        ? '${_thousands(all.length)} $noun'
        : '${_thousands(tracks.length)} of ${_thousands(all.length)} $noun');
    if (kind == 'album' && discs > 1) count.write(' · $discs discs');

    // Only the artist page is long enough to need them.
    final jumps = <(String, GlobalKey)>[
      if (kind == 'artist' && _mostPlayed(all).length >= 3)
        ('Top tracks', _topKey),
      if (kind == 'artist') ('All songs', _songsKey),
      if (kind == 'artist' && st.detailAlbums.isNotEmpty)
        ('Albums · ${st.detailAlbums.length}', _albumsKey),
    ];

    return Container(
      height: 56,
      decoration: BoxDecoration(
        border: Border(
          top: BorderSide(color: t.nInk.withValues(alpha: 0.10)),
        ),
      ),
      child: Row(
        children: [
          Expanded(
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: Row(
                children: [
                  if (jumps.length > 1) ...[
                    _Segmented(
                      labels: [for (final j in jumps) j.$1],
                      selected: _jumped.clamp(0, jumps.length - 1),
                      onTap: (i) {
                        setState(() => _jumped = i);
                        _jump(jumps[i].$2);
                      },
                    ),
                    const SizedBox(width: 10),
                  ],
                  _FilterField(
                    controller: _filter,
                    hint: kind == 'album'
                        ? 'Filter these ${all.length} tracks'
                        : 'Filter ${_thousands(all.length)} $noun',
                  ),
                  const SizedBox(width: 10),
                  _sortChip(st),
                  const SizedBox(width: 8),
                  _ToolChip(
                    icon: _lovedOnly ? Icons.favorite : Icons.favorite_border,
                    label: 'Loved only',
                    on: _lovedOnly,
                    onTap: () => setState(() => _lovedOnly = !_lovedOnly),
                  ),
                  if (kind == 'genre' || kind == 'folder') ...[
                    const SizedBox(width: 8),
                    _ToolChip(
                      icon: Icons.segment,
                      label: kind == 'genre'
                          ? 'Group by artist'
                          : 'Group by subfolder',
                      on: _group,
                      onTap: () => setState(() => _group = !_group),
                    ),
                  ],
                  if (_decade != null) ...[
                    const SizedBox(width: 8),
                    _ToolChip(
                      icon: Icons.close,
                      label: 'The ${_decade}s',
                      on: true,
                      onTap: () => setState(() => _decade = null),
                    ),
                  ],
                ],
              ),
            ),
          ),
          const SizedBox(width: 12),
          // Says what the filter did, so a short list never looks like a page
          // that failed to load.
          Text(
            count.toString(),
            style: TextStyle(
              fontSize: 12,
              color: t.nInk2,
              fontFeatures: const [FontFeature.tabularFigures()],
            ),
          ),
        ],
      ),
    );
  }

  /// What stands in for the hero once it has scrolled away: enough to know
  /// where you are, play it, and keep narrowing the list.
  Widget _strip(BuildContext context, MusicState st) {
    final t = context.tokens;
    final c = widget.controller;
    final all = st.detailTracks;
    final n = _thousands(all.length);
    final sub = switch (st.detailKind) {
      'album' => st.detailSubtitle.isEmpty
          ? '$n tracks'
          : '${st.detailSubtitle} · $n tracks',
      'artist' => '${st.detailAlbums.length} albums · $n songs',
      'playlist' => st.detailIsSmart ? 'Smart · $n tracks' : '$n tracks',
      'folder' => '${File(st.detailKey).parent.path} · $n files',
      _ => '$n tracks',
    };
    return Container(
      key: const ValueKey('strip'),
      height: 60,
      padding: const EdgeInsets.symmetric(horizontal: 32),
      decoration: BoxDecoration(
        color: Color.alphaBlend(c.accent.withValues(alpha: 0.10), t.nCanvas),
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          _BackBtn(size: 34, onTap: c.goBack),
          const SizedBox(width: 12),
          _mini(context, st),
          const SizedBox(width: 12),
          // The title takes the slack, so the filter and sort sit hard on the
          // right edge. As a Flexible beside a Spacer the two split the free
          // space, and what the capped filter did not use sat after it.
          Expanded(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  st.detailTitle,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontFamily: Tokens.fontFamily,
                    fontSize: 14,
                    fontWeight: FontWeight.w700,
                    color: t.nInk,
                  ),
                ),
                Text(
                  sub,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 12, color: t.nInk2),
                ),
              ],
            ),
          ),
          const SizedBox(width: 14),
          _PlayPill(
            compact: true,
            label: st.detailKind == 'artist' && _mostPlayed(all).length >= 3
                ? 'Play top'
                : 'Play',
            onTap: all.isEmpty ? null : () => _playAll(c, st),
          ),
          IconButton(
            tooltip: 'Shuffle',
            onPressed: all.isEmpty ? null : () => _shuffle(c, st),
            icon: Icon(Icons.shuffle, size: 18, color: t.nInk3),
          ),
          const SizedBox(width: 14),
          _FilterField(controller: _filter, hint: 'Filter', width: 220),
          const SizedBox(width: 8),
          _sortChip(st),
        ],
      ),
    );
  }

  Widget _mini(BuildContext context, MusicState st) {
    final t = context.tokens;
    if (st.detailKind == 'folder') {
      return Container(
        width: 40,
        height: 40,
        decoration: BoxDecoration(
          color: t.nTile,
          borderRadius: BorderRadius.circular(8),
        ),
        child: const Icon(Icons.folder, size: 22, color: Color(0xFFF59E0B)),
      );
    }
    return ClipRRect(
      borderRadius: BorderRadius.circular(st.detailKind == 'artist' ? 20 : 8),
      child: SizedBox.square(
        dimension: 40,
        child: _art(widget.controller, st, 40),
      ),
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
  const _Bio({required this.controller, required this.st, this.expanded = false});

  final MusicController controller;
  final MusicState st;

  /// Opened by Read more, where starting at three lines would be a step back
  /// from the four the side card already showed.
  final bool expanded;

  @override
  State<_Bio> createState() => _BioState();
}

/// Lines shown before "more". Three is what the box beside the portrait holds
/// without pushing the marks below the fold.
const int _kBioLines = 3;

class _BioState extends State<_Bio> {
  late bool _open = widget.expanded;

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

  Future<void> _pick() => _pickArt(widget.controller, widget.st);

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
      mainAxisSize: MainAxisSize.min,
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

// ---------------------------------------------------------------- the hero --
//
// One skeleton for all five kinds -- back and trail, the thumb, the info box, a
// side card, and the tools along its foot -- each filled with what only that
// kind of page has.

class _Hero extends StatelessWidget {
  const _Hero({
    super.key,
    required this.controller,
    required this.st,
    required this.tools,
    required this.decade,
    required this.onDecade,
    required this.onSimilar,
    this.onEditions,
    this.similar,
  });

  final MusicController controller;
  final MusicState st;

  /// Filter, sort and the toggles. Built by the page, which owns their state.
  final Widget tools;
  final int? decade;
  final ValueChanged<int> onDecade;
  final VoidCallback onSimilar;
  final VoidCallback? onEditions;
  final Future<List<BrowseCard>>? similar;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return DecoratedBox(
      // The artwork's own two colours, so the page is lit by what is on it
      // rather than by the section.
      decoration: BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          stops: const [0, 0.5, 1],
          colors: [
            Color.alphaBlend(controller.accent.withValues(alpha: 0.32), t.nCanvas),
            Color.alphaBlend(
                controller.accentAlt.withValues(alpha: 0.14), t.nCanvas),
            t.nCanvas,
          ],
        ),
      ),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(32, 18, 32, 0),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _TopBar(controller: controller, st: st),
            LayoutBuilder(builder: (context, box) {
              // Thumb, info and side card come to about a thousand pixels.
              // Under that the card drops below and the thumb gets smaller.
              final wide = box.maxWidth >= 1040;
              final thumb =
                  _Thumb(controller: controller, st: st, side: wide ? 212 : 160);
              final info = _Info(controller: controller, st: st, big: wide);
              final side = _Side(
                controller: controller,
                st: st,
                decade: decade,
                onDecade: onDecade,
                onEditions: onEditions,
                onSimilar: onSimilar,
                similar: similar,
              );
              return Padding(
                // Headroom over an album's sleeve for the record, which
                // rises out of the top of it.
                padding: EdgeInsets.only(
                    top: st.detailKind == 'album' ? 56 : 18, bottom: 22),
                child: wide
                    ? Row(
                        crossAxisAlignment: CrossAxisAlignment.end,
                        children: [
                          thumb,
                          const SizedBox(width: 30),
                          Expanded(child: info),
                          const SizedBox(width: 30),
                          // The artist's card holds prose, and prose wants a
                          // wider measure than a column of figures does.
                          SizedBox(
                            width: st.detailKind == 'artist' ? 380 : 300,
                            child: side,
                          ),
                        ],
                      )
                    : Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          Row(
                            crossAxisAlignment: CrossAxisAlignment.end,
                            children: [
                              thumb,
                              const SizedBox(width: 24),
                              Expanded(child: info),
                            ],
                          ),
                          const SizedBox(height: 18),
                          side,
                        ],
                      ),
              );
            }),
            tools,
          ],
        ),
      ),
    );
  }
}

/// Back, the trail that led here, and what the page can do beyond its list.
///
/// Back is one step -- arriving here from an artist page goes back to that
/// artist -- and the trail is every step, each one a link.
class _TopBar extends StatelessWidget {
  const _TopBar({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  static const _tabs = {
    'albums': 'Albums',
    'artists': 'Artists',
    'genres': 'Genres',
    'folders': 'Folders',
    'playlists': 'Playlists',
    'songs': 'Songs',
    'favorites': 'Loved',
    'history': 'History',
  };

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final trail = controller.trail;
    final tab = _tabs[controller.libTab];
    final crumbs = <(String, VoidCallback)>[
      ('My Music', controller.closeDetail),
      if (tab != null) (tab, controller.closeDetail),
      for (var i = 0; i < trail.length - 1; i++)
        (trail[i], () => controller.goBackTo(i)),
    ];
    return SizedBox(
      height: 40,
      child: Row(
        children: [
          _BackBtn(size: 40, onTap: controller.goBack),
          const SizedBox(width: 12),
          Expanded(
            // Reversed, so a long trail loses its oldest steps off the left
            // edge and never the page you are on.
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              reverse: true,
              child: Row(
                children: [
                  for (final (label, onTap) in crumbs) ...[
                    _Crumb(label: label, onTap: onTap),
                    Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 4),
                      child: Text(
                        '›',
                        style: TextStyle(
                            fontSize: 13,
                            color: t.nInk2.withValues(alpha: 0.6)),
                      ),
                    ),
                  ],
                  Text(
                    st.detailTitle,
                    maxLines: 1,
                    style: TextStyle(
                      fontFamily: Tokens.fontFamily,
                      fontSize: 13,
                      fontWeight: FontWeight.w600,
                      color: t.nInk,
                    ),
                  ),
                ],
              ),
            ),
          ),
          const SizedBox(width: 8),
          // A folder has nothing to change and one thing to open, so it gets
          // that as a button rather than a menu of one.
          if (st.detailKind == 'folder')
            IconButton(
              tooltip: 'Open in Files',
              onPressed: st.detailRoot.isEmpty
                  ? null
                  : () => controller
                      .send(MusicCmd.revealFolder(path: st.detailKey)),
              icon: Icon(Icons.open_in_new, size: 19, color: t.nInk3),
            )
          else
            _More(controller: controller, st: st),
        ],
      ),
    );
  }
}

/// The rarer things: a new picture, where a biography came from, and a
/// playlist's export, copy and delete.
class _More extends StatelessWidget {
  const _More({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final kind = st.detailKind;
    return PopupMenuButton<String>(
      tooltip: 'More',
      icon: Icon(Icons.more_horiz, color: context.tokens.nInk3),
      onSelected: (v) => _run(context, v),
      itemBuilder: (_) => <PopupMenuEntry<String>>[
        PopupMenuItem(
          value: 'art',
          child: Text(kind == 'artist' ? 'Change picture…' : 'Change cover…'),
        ),
        if (kind == 'artist' && st.detailMbid.isNotEmpty)
          const PopupMenuItem(value: 'mb', child: Text('Open on MusicBrainz')),
        if (kind == 'artist')
          const PopupMenuItem(value: 'wiki', child: Text('Look up on Wikipedia')),
        if (kind == 'playlist') ...[
          const PopupMenuItem(value: 'export', child: Text('Export as .m3u…')),
          const PopupMenuItem(value: 'copy', child: Text('Duplicate')),
          const PopupMenuDivider(),
          const PopupMenuItem(value: 'delete', child: Text('Delete playlist')),
        ],
      ],
    );
  }

  Future<void> _run(BuildContext context, String action) async {
    switch (action) {
      case 'art':
        await _pickArt(controller, st);
      case 'mb':
      case 'wiki':
        await controller.send(MusicCmd.openArtistSource(
          artistId: st.detailId,
          source: action == 'mb' ? 'musicbrainz' : 'wikipedia',
        ));
      case 'export':
        final path = await pickSaveLocation(
          suggestedName: '${st.detailTitle}.m3u',
          label: 'Playlist',
          extensions: const ['m3u', 'm3u8'],
        );
        if (path == null) return;
        await controller.send(
            MusicCmd.playlistExport(playlistId: st.detailId, path: path));
      case 'copy':
        await controller.send(MusicCmd.playlistDuplicate(playlistId: st.detailId));
      case 'delete':
        if (context.mounted) await _deletePlaylist(context, controller, st);
    }
  }
}

class _BackBtn extends StatefulWidget {
  const _BackBtn({required this.size, required this.onTap});

  final double size;
  final VoidCallback onTap;

  @override
  State<_BackBtn> createState() => _BackBtnState();
}

class _BackBtnState extends State<_BackBtn> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Tooltip(
      message: 'Back',
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hovered = true),
        onExit: (_) => setState(() => _hovered = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: AnimatedContainer(
            duration: t.reduceMotion
                ? Duration.zero
                : const Duration(milliseconds: 150),
            width: widget.size,
            height: widget.size,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: _hovered ? t.nInk : t.nCanvas.withValues(alpha: 0.55),
              border: Border.all(color: t.nInk.withValues(alpha: 0.18)),
            ),
            child: Icon(
              Icons.arrow_back,
              size: widget.size * 0.45,
              color: _hovered ? t.nCanvas : t.nInk,
            ),
          ),
        ),
      ),
    );
  }
}

class _Crumb extends StatefulWidget {
  const _Crumb({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  State<_Crumb> createState() => _CrumbState();
}

class _CrumbState extends State<_Crumb> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 2),
          decoration: BoxDecoration(
            color: _hovered ? t.nInk.withValues(alpha: 0.08) : null,
            borderRadius: BorderRadius.circular(6),
          ),
          child: Text(
            widget.label,
            maxLines: 1,
            style: TextStyle(fontSize: 13, color: _hovered ? t.nInk : t.nInk2),
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------- thumbs --

/// The picture, drawn the way its kind of thing looks: a record with its disc,
/// a portrait with a ring, a genre's name when it has no cover, and a folder
/// as the formats it holds.
class _Thumb extends StatelessWidget {
  const _Thumb({required this.controller, required this.st, required this.side});

  final MusicController controller;
  final MusicState st;
  final double side;

  @override
  Widget build(BuildContext context) {
    final Widget art = switch (st.detailKind) {
      'album' => _AlbumThumb(controller: controller, st: st, side: side),
      'artist' => _ArtistThumb(controller: controller, st: st, side: side),
      'folder' => _Framed(side: side, child: _FolderTile(st: st, side: side)),
      'genre' when !_hasArt(controller, st) => _Framed(
          side: side,
          child: _GenreTile(controller: controller, st: st, side: side),
        ),
      _ => _Framed(side: side, child: _art(controller, st, side)),
    };
    return _Cover(controller: controller, st: st, child: art);
  }
}

/// Rounded, lifted off the wash, with a hairline so a dark cover does not
/// melt into a dark page.
class _Framed extends StatelessWidget {
  const _Framed({required this.side, required this.child, this.radius = 14});

  final double side;
  final double radius;
  final Widget child;

  @override
  Widget build(BuildContext context) => Container(
        width: side,
        height: side,
        clipBehavior: Clip.antiAlias,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(radius),
          boxShadow: const [
            BoxShadow(
              color: Color(0x99000000),
              blurRadius: 40,
              spreadRadius: -12,
              offset: Offset(0, 18),
            ),
          ],
        ),
        foregroundDecoration: BoxDecoration(
          borderRadius: BorderRadius.circular(radius),
          border: Border.all(color: const Color(0x1AFFFFFF)),
        ),
        child: child,
      );
}

/// The sleeve, with the record behind it sliding half out on hover.
class _AlbumThumb extends StatefulWidget {
  const _AlbumThumb({required this.controller, required this.st, required this.side});

  final MusicController controller;
  final MusicState st;
  final double side;

  @override
  State<_AlbumThumb> createState() => _AlbumThumbState();
}

class _AlbumThumbState extends State<_AlbumThumb> {
  bool _out = false;

  /// The cover's own colour, for the pressing. Not `controller.accent`: that
  /// is whatever is playing, and this record is the one on the page.
  late String _path = _coverPath();
  late Future<Color?> _colour = dominantColour(_path);

  String _coverPath() => widget.st.detailArt.isNotEmpty
      ? widget.st.detailArt
      : widget.controller.artFor('album', '${widget.st.detailId}') ?? '';

  @override
  void didUpdateWidget(covariant _AlbumThumb old) {
    super.didUpdateWidget(old);
    // The cover can resolve after the first build, from embedded art.
    final path = _coverPath();
    if (path != _path) {
      _path = path;
      _colour = dominantColour(path);
    }
  }

  @override
  Widget build(BuildContext context) {
    final side = widget.side;
    final disc = side * 0.92;
    // Out of the top of the sleeve, not its side: the side is where the title
    // starts, and a record sliding out there covered the words. Resting and
    // risen both stay inside the headroom `_Hero` leaves an album, clear of
    // the back button and the trail.
    final rest = side * 0.12;
    final lift = side * 0.14;
    return MouseRegion(
      onEnter: (_) => setState(() => _out = true),
      onExit: (_) => setState(() => _out = false),
      child: SizedBox(
        width: side,
        height: side,
        child: Stack(
          clipBehavior: Clip.none,
          children: [
            Positioned(
              top: -rest,
              left: (side - disc) / 2,
              child: TweenAnimationBuilder<double>(
                tween: Tween<double>(end: _out ? 1.0 : 0.0),
                duration: context.tokens.reduceMotion
                    ? Duration.zero
                    : const Duration(milliseconds: 350),
                curve: Curves.easeOutCubic,
                builder: (_, v, child) =>
                    Transform.translate(offset: Offset(0, -lift * v), child: child),
                child: FutureBuilder<Color?>(
                  future: _colour,
                  builder: (context, snap) => CustomPaint(
                    size: Size.square(disc),
                    painter: _VinylPainter(
                      colour: snap.data ?? const Color(0xFF3A3A44),
                    ),
                  ),
                ),
              ),
            ),
            _Framed(side: side, child: _art(widget.controller, widget.st, side)),
          ],
        ),
      ),
    );
  }
}

/// A coloured pressing, drawn: vinyl in the sleeve's own colour, fine grooves
/// with the smooth gaps between tracks, a pressed rim, the two wedges of sheen
/// a light puts across it, and a label.
class _VinylPainter extends CustomPainter {
  _VinylPainter({required this.colour});

  /// The cover's dominant colour.
  final Color colour;

  @override
  void paint(Canvas canvas, Size size) {
    final c = size.center(Offset.zero);
    final r = size.width / 2;
    final disc = Rect.fromCircle(center: c, radius: r);

    // The vinyl itself: deep, the colour showing through the black the way a
    // coloured pressing does, a little lighter towards the middle.
    canvas.drawCircle(
      c,
      r,
      Paint()
        ..shader = RadialGradient(
          colors: [
            Color.lerp(Colors.black, colour, 0.42)!,
            Color.lerp(Colors.black, colour, 0.24)!,
          ],
        ).createShader(disc),
    );

    // Grooves, alternately cut and catching light, and every so often the
    // smooth band where one track ends and the next begins.
    final groove = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 0.7;
    var i = 0;
    for (var g = r * 0.38; g < r * 0.94; g += 1.5, i++) {
      groove.color = i % 11 == 0
          ? const Color(0x1FFFFFFF)
          : i.isEven
              ? const Color(0x40000000)
              : const Color(0x0DFFFFFF);
      canvas.drawCircle(c, g, groove);
    }

    // The pressed rim and the smooth run-out edge.
    canvas.drawCircle(
      c,
      r * 0.965,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = r * 0.05
        ..color = const Color(0x33000000),
    );
    canvas.drawCircle(
      c,
      r - 0.75,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1.5
        ..color = const Color(0x2EFFFFFF),
    );

    // The sheen: two opposed wedges of light, warmed by the vinyl's colour.
    // Faded to the glint's own transparent, not to black, so the edges of
    // the wedges do not go grey.
    final glint = Color.lerp(Colors.white, colour, 0.3)!;
    final clear = glint.withValues(alpha: 0);
    canvas.drawCircle(
      c,
      r * 0.95,
      Paint()
        ..shader = SweepGradient(
          transform: const GradientRotation(-0.9),
          colors: [
            clear,
            glint.withValues(alpha: 0.26),
            clear,
            clear,
            glint.withValues(alpha: 0.18),
            clear,
            clear,
          ],
          stops: const [0.02, 0.09, 0.17, 0.52, 0.59, 0.67, 1],
        ).createShader(disc),
    );

    // The label, in the cover's colour, with a ring pressed into it.
    final lr = r * 0.34;
    canvas.drawCircle(
      c,
      lr,
      Paint()
        ..shader = RadialGradient(
          colors: [
            Color.lerp(colour, Colors.white, 0.2)!,
            colour,
            Color.lerp(colour, Colors.black, 0.35)!,
          ],
          stops: const [0, 0.55, 1],
        ).createShader(Rect.fromCircle(center: c, radius: lr)),
    );
    canvas.drawCircle(
      c,
      lr * 0.78,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1
        ..color = const Color(0x2E000000),
    );
    // The spindle hole.
    canvas.drawCircle(c, r * 0.03, Paint()..color = const Color(0xFF050506));
  }

  @override
  bool shouldRepaint(_VinylPainter old) => old.colour != colour;
}

/// A portrait reads as a person, so it is round, and ringed in its colour.
class _ArtistThumb extends StatelessWidget {
  const _ArtistThumb({required this.controller, required this.st, required this.side});

  final MusicController controller;
  final MusicState st;
  final double side;

  @override
  Widget build(BuildContext context) => SizedBox(
        width: side,
        height: side,
        child: Stack(
          clipBehavior: Clip.none,
          children: [
            Positioned(
              left: -8,
              top: -8,
              right: -8,
              bottom: -8,
              child: DecoratedBox(
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  border: Border.all(
                    color: controller.accent.withValues(alpha: 0.7),
                    width: 2,
                  ),
                ),
              ),
            ),
            _Framed(
              side: side,
              radius: side / 2,
              child: _art(controller, st, side),
            ),
          ],
        ),
      );
}

/// A genre with no cover picked: its name, set large, over its colours --
/// rather than a stock icon that says nothing about which genre this is.
class _GenreTile extends StatelessWidget {
  const _GenreTile({required this.controller, required this.st, required this.side});

  final MusicController controller;
  final MusicState st;
  final double side;

  @override
  Widget build(BuildContext context) {
    final years = st.detailTracks
        .map((x) => x.year)
        .where((y) => y > 1000)
        .toList()
      ..sort();
    final span = years.isEmpty
        ? ''
        : years.first == years.last
            ? ' · ${years.first}'
            : ' · ${years.first}–${years.last}';
    return DecoratedBox(
      decoration: BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.bottomLeft,
          end: Alignment.topRight,
          stops: const [0, 0.55, 1],
          colors: [
            Color.lerp(controller.accent, Colors.black, 0.25)!,
            const Color(0xFF111827),
            Color.lerp(controller.accentAlt, Colors.black, 0.25)!,
          ],
        ),
      ),
      child: Padding(
        padding: EdgeInsets.all(side * 0.075),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          children: [
            Text(
              'GENRE$span',
              style: const TextStyle(
                fontSize: 10.5,
                fontWeight: FontWeight.w600,
                letterSpacing: 1.5,
                color: Color(0xB3FFFFFF),
              ),
            ),
            Text(
              st.detailTitle,
              maxLines: 3,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: side * 0.18,
                height: 0.95,
                fontWeight: FontWeight.w800,
                letterSpacing: -1,
                color: Colors.white,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A folder has no cover anywhere, so its tile is what it is made of.
class _FolderTile extends StatelessWidget {
  const _FolderTile({required this.st, required this.side});

  final MusicState st;
  final double side;

  @override
  Widget build(BuildContext context) {
    final formats = _formatCounts(st.detailTracks);
    return DecoratedBox(
      decoration: const BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [Color(0xFF2A2D33), Color(0xFF1B1D22)],
        ),
      ),
      child: Center(
        child: SizedBox.square(
          dimension: side * 0.71,
          child: CustomPaint(
            painter: _RingPainter(parts: [
              for (final (i, (_, n)) in formats.indexed)
                (n.toDouble(), _formatColor(i)),
            ]),
            child: Center(
              child: Icon(Icons.folder,
                  size: side * 0.25, color: const Color(0xFFF59E0B)),
            ),
          ),
        ),
      ),
    );
  }
}

class _RingPainter extends CustomPainter {
  _RingPainter({required this.parts});

  final List<(double, Color)> parts;

  @override
  void paint(Canvas canvas, Size size) {
    const turn = 6.283185307179586;
    final total = parts.fold<double>(0, (a, p) => a + p.$1);
    final stroke = size.width * 0.13;
    final rect = (Offset.zero & size).deflate(stroke / 2);
    final paint = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = stroke;
    if (total <= 0) {
      canvas.drawArc(rect, 0, turn, false, paint..color = const Color(0xFF33363D));
      return;
    }
    var start = -turn / 4;
    for (final (value, color) in parts) {
      final sweep = value / total * turn;
      canvas.drawArc(rect, start, sweep, false, paint..color = color);
      start += sweep;
    }
  }

  @override
  bool shouldRepaint(_RingPainter old) => old.parts != parts;
}

// ---------------------------------------------------------------- info --

class _Info extends StatelessWidget {
  const _Info({required this.controller, required this.st, required this.big});

  final MusicController controller;
  final MusicState st;

  /// The wide layout's 46px title; 34 when the side card has dropped below.
  final bool big;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final credit = _credit();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        _KindLine(st: st),
        const SizedBox(height: 12),
        Text(
          st.detailTitle,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(
            fontFamily: Tokens.fontFamily,
            fontSize: big ? 46 : 34,
            height: 1.05,
            fontWeight: FontWeight.w800,
            letterSpacing: big ? -1.1 : -0.8,
            color: t.nInk,
          ),
        ),
        if (credit != null) ...[const SizedBox(height: 12), credit],
        const SizedBox(height: 14),
        _Specs(tiles: _tiles(st)),
        const SizedBox(height: 16),
        _HeroActions(controller: controller, st: st),
      ],
    );
  }

  /// The line under the title: who made it, what it is about, where it is.
  Widget? _credit() {
    switch (st.detailKind) {
      case 'album':
        return Wrap(
          spacing: 12,
          runSpacing: 8,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            // Who made it has a page of their own, so it is a link -- the same
            // way the player bar's second line is.
            if (st.detailSubtitle.isNotEmpty && st.detailArtistId != 0)
              _ArtistLink(
                name: st.detailSubtitle,
                onTap: () => controller
                    .send(MusicCmd.openArtist(artistId: st.detailArtistId)),
              )
            else if (st.detailSubtitle.isNotEmpty)
              Text(st.detailSubtitle),
            ..._genrePills(),
          ],
        );
      case 'artist':
        final pills = _genrePills();
        return pills.isEmpty ? null : Wrap(spacing: 6, runSpacing: 6, children: pills);
      case 'playlist':
        return _InlineNote(controller: controller, st: st);
      case 'genre':
        final shelf =
            st.detailShelves.where((s) => s.kind == 'artist').firstOrNull;
        if (shelf == null || shelf.cards.isEmpty) return null;
        return Wrap(
          spacing: 6,
          runSpacing: 6,
          children: [
            for (final card in shelf.cards.take(3))
              _Pill(
                label: '${card.title} · ${card.count}',
                onTap: () =>
                    controller.send(MusicCmd.openArtist(artistId: card.id)),
              ),
            if (shelf.cards.length > 3) _Pill(label: '+${shelf.cards.length - 3}'),
          ],
        );
      case 'folder':
        return _PathLinks(controller: controller, st: st);
    }
    return null;
  }

  List<Widget> _genrePills() => [
        for (final g in _topGenres(st.detailTracks))
          _Pill(
            label: g,
            onTap: () => controller.send(MusicCmd.openGenre(name: g)),
          ),
      ];
}

/// "ALBUM  2007 · XL Recordings · XLCD 324": the kind, and the facts that used
/// to be behind a Details dialog.
class _KindLine extends StatelessWidget {
  const _KindLine({required this.st});

  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final kind = st.detailKind;
    final note = switch (kind) {
      'album' => [_year(st), _meta(st, 'Label'), _meta(st, 'Catalogue')]
          .whereType<String>()
          .where((s) => s.isNotEmpty)
          .join(' · '),
      // Nothing: formed, origin and the rest are the About card's.
      'artist' => '',
      'genre' => _genreNote(st),
      'folder' => st.detailRoot.isEmpty
          ? ''
          : st.detailScanned.isEmpty
              ? 'Watched'
              : 'Watched · last scanned ${st.detailScanned}',
      _ => !st.detailIsSmart && st.detailLastPlayed.isNotEmpty
          ? 'Last played ${st.detailLastPlayed}'
          : '',
    };
    final smart = kind == 'playlist' && st.detailIsSmart;
    return Wrap(
      spacing: 10,
      runSpacing: 6,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        Text(
          kind.toUpperCase(),
          style: TextStyle(
            fontFamily: Tokens.fontFamily,
            fontSize: 11,
            fontWeight: FontWeight.w700,
            letterSpacing: 1.8,
            color: t.nInk2,
          ),
        ),
        if (smart) ...[
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
            decoration: BoxDecoration(
              color: Tokens.secMusic.withValues(alpha: 0.22),
              borderRadius: BorderRadius.circular(999),
              border: Border.all(color: Tokens.secMusic.withValues(alpha: 0.45)),
            ),
            child: Text(
              'SMART',
              style: TextStyle(
                fontSize: 10.5,
                fontWeight: FontWeight.w700,
                letterSpacing: 1,
                color: t.dark ? const Color(0xFFF9A8D4) : Tokens.secMusic,
              ),
            ),
          ),
          // A smart playlist rebuilds itself as the library changes; the dot
          // is the difference between this and a list somebody made.
          const Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              DecoratedBox(
                decoration: BoxDecoration(
                  color: Color(0xFF34D399),
                  shape: BoxShape.circle,
                  boxShadow: [BoxShadow(color: Color(0x4034D399), spreadRadius: 3)],
                ),
                child: SizedBox.square(dimension: 7),
              ),
              SizedBox(width: 6),
              Text(
                'Live · updates with the library',
                style: TextStyle(fontSize: 11.5, color: Color(0xFF34D399)),
              ),
            ],
          ),
        ],
        if (note.isNotEmpty)
          Text(
            note,
            style: TextStyle(
                fontSize: 12.5, fontWeight: FontWeight.w500, color: t.nInk3),
          ),
      ],
    );
  }
}

/// The page's numbers, as a row of tiles rather than one sentence of them.
class _Specs extends StatelessWidget {
  const _Specs({required this.tiles});

  /// (value, label) pairs.
  final List<(String, String)> tiles;

  @override
  Widget build(BuildContext context) {
    if (tiles.isEmpty) return const SizedBox.shrink();
    final t = context.tokens;
    return SingleChildScrollView(
      scrollDirection: Axis.horizontal,
      child: Container(
        decoration: BoxDecoration(
          color: t.nCanvas.withValues(alpha: 0.55),
          borderRadius: BorderRadius.circular(12),
          border: Border.all(color: t.nInk.withValues(alpha: 0.12)),
        ),
        child: IntrinsicHeight(
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              for (var i = 0; i < tiles.length; i++) ...[
                if (i > 0)
                  VerticalDivider(
                      width: 1,
                      thickness: 1,
                      color: t.nInk.withValues(alpha: 0.10)),
                Padding(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 16, vertical: 9),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(
                        tiles[i].$1,
                        style: TextStyle(
                          fontFamily: Tokens.fontFamily,
                          fontSize: 17,
                          fontWeight: FontWeight.w700,
                          color: t.nInk,
                          fontFeatures: const [FontFeature.tabularFigures()],
                        ),
                      ),
                      const SizedBox(height: 2),
                      Text(
                        tiles[i].$2.toUpperCase(),
                        style: TextStyle(
                            fontSize: 10.5, letterSpacing: 1, color: t.nInk2),
                      ),
                    ],
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// Play, and what else this kind of page is for. One row, once: the pinned
/// bar that used to repeat Play, Shuffle and Queue under it is gone.
class _HeroActions extends StatelessWidget {
  const _HeroActions({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final all = st.detailTracks;
    final kind = st.detailKind;
    final artist = kind == 'artist';
    // The four- and five-star tracks, best first.
    final best = kind == 'genre'
        ? (all.where((x) => x.stars >= 4).toList()
          ..sort((a, b) => a.stars != b.stars
              ? b.stars.compareTo(a.stars)
              : b.playCount.compareTo(a.playCount)))
        : const <Track>[];
    return Wrap(
      spacing: 8,
      runSpacing: 8,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        _PlayPill(
          label: artist && _mostPlayed(all).length >= 3 ? 'Play top tracks' : 'Play',
          onTap: all.isEmpty ? null : () => _playAll(controller, st),
        ),
        DetailActionBtn(
          icon: Icons.shuffle,
          label: artist ? 'Shuffle all' : 'Shuffle',
          onTap: () => _shuffle(controller, st),
        ),
        DetailActionBtn(
          icon: Icons.queue_music,
          label: artist ? 'Queue all' : 'Queue',
          onTap: () => controller.queueAll(all),
        ),
        if (kind == 'album')
          DetailActionBtn(
            icon: Icons.playlist_add,
            label: 'Add to playlist',
            onTap: () => addToPlaylist(context, controller, all),
          ),
        if (best.isNotEmpty)
          DetailActionBtn(
            icon: Icons.star_outline,
            label: 'Play best rated',
            onTap: () => controller.playFrom(best, 0, 'genre'),
          ),
        // Only on a smart playlist. A manual one has no rules, and offering the
        // editor on it would turn it into a smart one the moment anything was
        // saved.
        if (kind == 'playlist' && st.detailIsSmart)
          DetailActionBtn(
            icon: Icons.tune,
            label: 'Edit rules',
            onTap: () => editSmartPlaylist(context, controller,
                playlistId: st.detailId, name: st.detailTitle),
          ),
        if (kind == 'playlist') ...[
          const _Sep(),
          DetailActionBtn(
            icon: Icons.delete_outline,
            label: 'Delete',
            tint: Tokens.error,
            onTap: () => _deletePlaylist(context, controller, st),
          ),
        ],
        // Only for a folder the library watches: the bridge refuses both for
        // anywhere else.
        if (kind == 'folder' && st.detailRoot.isNotEmpty) ...[
          const _Sep(),
          DetailActionBtn(
            icon: Icons.refresh,
            label: 'Rescan folder',
            onTap: () =>
                controller.send(MusicCmd.rescanFolder(path: st.detailKey)),
          ),
          DetailActionBtn(
            icon: Icons.open_in_new,
            label: 'Open in Files',
            onTap: () =>
                controller.send(MusicCmd.revealFolder(path: st.detailKey)),
          ),
        ],
        if (kind == 'album' || artist) ...[
          const _Sep(),
          _Marks(controller: controller, st: st),
        ],
      ],
    );
  }
}

class _Sep extends StatelessWidget {
  const _Sep();

  @override
  Widget build(BuildContext context) => Container(
        width: 1,
        height: 26,
        margin: const EdgeInsets.symmetric(horizontal: 4),
        color: context.tokens.nInk.withValues(alpha: 0.14),
      );
}

/// The one filled control on the page, because there is exactly one thing you
/// usually came here to do.
class _PlayPill extends StatelessWidget {
  const _PlayPill({required this.label, required this.onTap, this.compact = false});

  final String label;
  final VoidCallback? onTap;

  /// The strip's size.
  final bool compact;

  @override
  Widget build(BuildContext context) {
    final h = compact ? 36.0 : 44.0;
    final shape = BorderRadius.circular(h / 2);
    return Opacity(
      opacity: onTap == null ? 0.5 : 1,
      child: DecoratedBox(
        decoration: BoxDecoration(
          borderRadius: shape,
          boxShadow: compact
              ? null
              : [
                  BoxShadow(
                    color: Tokens.secMusic.withValues(alpha: 0.5),
                    blurRadius: 24,
                    spreadRadius: -8,
                    offset: const Offset(0, 8),
                  ),
                ],
        ),
        child: Material(
          color: Tokens.secMusic,
          borderRadius: shape,
          clipBehavior: Clip.antiAlias,
          child: InkWell(
            onTap: onTap,
            child: SizedBox(
              height: h,
              child: Padding(
                padding: EdgeInsets.fromLTRB(compact ? 12 : 18, 0, compact ? 16 : 22, 0),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const Icon(Icons.play_arrow_rounded, size: 20, color: Colors.white),
                    const SizedBox(width: 6),
                    Text(
                      label,
                      style: const TextStyle(
                        fontFamily: Tokens.fontFamily,
                        fontSize: 14,
                        fontWeight: FontWeight.w700,
                        color: Colors.white,
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _Pill extends StatefulWidget {
  const _Pill({required this.label, this.onTap});

  final String label;
  final VoidCallback? onTap;

  @override
  State<_Pill> createState() => _PillState();
}

class _PillState extends State<_Pill> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final live = widget.onTap != null && _hovered;
    return MouseRegion(
      cursor: widget.onTap == null ? MouseCursor.defer : SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
          decoration: BoxDecoration(
            color: t.nInk.withValues(alpha: 0.08),
            borderRadius: BorderRadius.circular(999),
            border: Border.all(color: t.nInk.withValues(alpha: live ? 0.30 : 0.10)),
          ),
          child: Text(
            widget.label,
            style: TextStyle(
              fontSize: 12,
              fontWeight: FontWeight.w500,
              color: live ? t.nInk : t.nInk3,
            ),
          ),
        ),
      ),
    );
  }
}

/// The folder's path, each step up it a way to that folder's own page -- but
/// only as far up as the watched root. A page for the folder above the root
/// would be everything under the home directory.
class _PathLinks extends StatelessWidget {
  const _PathLinks({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final sep = Platform.pathSeparator;
    final absolute = st.detailKey.startsWith(sep);
    final parts = st.detailKey.split(sep).where((s) => s.isNotEmpty).toList();
    final root = _trimSep(st.detailRoot);
    final slash = Text(sep, style: TextStyle(fontSize: 13, color: t.nInk2));
    return Wrap(
      spacing: 4,
      runSpacing: 4,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        for (var i = 0; i < parts.length; i++) ...[
          if (i > 0 || absolute) slash,
          _segment(t, parts, i, absolute, root),
        ],
      ],
    );
  }

  Widget _segment(
      Tokens t, List<String> parts, int i, bool absolute, String root) {
    final sep = Platform.pathSeparator;
    final path = (absolute ? sep : '') + parts.take(i + 1).join(sep);
    final last = i == parts.length - 1;
    if (!last && root.isNotEmpty && path.startsWith(root)) {
      return _Pill(
        label: parts[i],
        onTap: () => controller.send(MusicCmd.openFolder(path: path)),
      );
    }
    return Text(
      parts[i],
      style: TextStyle(
        fontSize: 13,
        fontWeight: last ? FontWeight.w600 : FontWeight.w400,
        color: last ? t.nInk : t.nInk3,
      ),
    );
  }
}

/// A playlist's description, edited where it is shown.
class _InlineNote extends StatefulWidget {
  const _InlineNote({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  State<_InlineNote> createState() => _InlineNoteState();
}

class _InlineNoteState extends State<_InlineNote> {
  bool _editing = false;
  final _text = TextEditingController();
  final _focus = FocusNode();

  @override
  void initState() {
    super.initState();
    // Clicking away keeps what was typed; there is no Save button to miss.
    _focus.addListener(() {
      if (!_focus.hasFocus && _editing) _save();
    });
  }

  @override
  void dispose() {
    _text.dispose();
    _focus.dispose();
    super.dispose();
  }

  void _edit() {
    _text.text = widget.st.detailNote;
    setState(() => _editing = true);
  }

  Future<void> _save() async {
    setState(() => _editing = false);
    final text = _text.text.trim();
    if (text == widget.st.detailNote) return;
    await widget.controller.send(
      MusicCmd.playlistDescribe(playlistId: widget.st.detailId, text: text),
    );
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (_editing) {
      return ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 560),
        child: TextField(
          controller: _text,
          focusNode: _focus,
          autofocus: true,
          onSubmitted: (_) => _save(),
          style: TextStyle(fontSize: 15, color: t.nInk),
          decoration: const InputDecoration(
            isDense: true,
            hintText: 'Sunday morning, gym, the drive north…',
          ),
        ),
      );
    }
    final note = widget.st.detailNote;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Flexible(
          child: GestureDetector(
            onTap: _edit,
            child: Text(
              note.isEmpty ? 'Say what this playlist is for' : note,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontSize: 15,
                height: 1.4,
                color: note.isEmpty ? t.nInk2 : t.nInk3,
                fontStyle: note.isEmpty ? FontStyle.italic : FontStyle.normal,
              ),
            ),
          ),
        ),
        IconButton(
          tooltip: 'Edit description',
          iconSize: 15,
          visualDensity: VisualDensity.compact,
          onPressed: _edit,
          icon: Icon(Icons.edit_outlined, color: t.nInk2),
        ),
      ],
    );
  }
}

// ---------------------------------------------------------------- side card --

/// What each page is about, shown rather than kept behind a dialog: your
/// listening on an album, the biography on an artist, the rules on a smart
/// playlist, the decades on a genre, what is on disk in a folder.
class _Side extends StatelessWidget {
  const _Side({
    required this.controller,
    required this.st,
    required this.decade,
    required this.onDecade,
    required this.onSimilar,
    this.onEditions,
    this.similar,
  });

  final MusicController controller;
  final MusicState st;
  final int? decade;
  final ValueChanged<int> onDecade;
  final VoidCallback onSimilar;
  final VoidCallback? onEditions;
  final Future<List<BrowseCard>>? similar;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final parts = switch (st.detailKind) {
      'artist' => _artist(context),
      'playlist' when st.detailIsSmart => _rules(context),
      'genre' when st.detailBars.isNotEmpty => _decades(context),
      'folder' => _folder(context),
      _ => _listening(context),
    };
    return Container(
      padding: const EdgeInsets.fromLTRB(18, 16, 18, 16),
      decoration: BoxDecoration(
        color: t.nCard.withValues(alpha: 0.82),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: t.nInk.withValues(alpha: 0.10)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          for (var i = 0; i < parts.length; i++) ...[
            if (i > 0) const SizedBox(height: 12),
            parts[i],
          ],
        ],
      ),
    );
  }

  Future<void> _history() async {
    await controller.closeDetail();
    await controller.send(MusicCmd.setLibTab(name: 'history'));
  }

  /// How much of it you have heard, and where to pick it up.
  List<Widget> _listening(BuildContext context) {
    final t = context.tokens;
    final all = st.detailTracks;
    final heard = all.where((x) => x.playCount > 0).length;
    final plays = all.fold<int>(0, (a, x) => a + x.playCount);
    final at = st.detailResumeItem == 0
        ? -1
        : all.indexWhere((x) => x.itemId == st.detailResumeItem);
    final added = _meta(st, 'Added');
    final editions = st.detailShelves
        .where((s) => s.title.contains('edition'))
        .fold<int>(0, (a, s) => a + s.cards.length);
    final rows = <(String, String, VoidCallback?)>[
      if (at < 0 && st.detailLastPlayed.isNotEmpty)
        ('Last played', st.detailLastPlayed, null),
      if (added != null) ('Added', added, null),
      if (onEditions != null && editions > 0)
        ('Other editions', '$editions ›', onEditions),
    ];
    return [
      _SideHead('Your listening', action: ('History', _history)),
      _Meter(
        value: all.isEmpty ? 0 : heard / all.length,
        a: controller.accent,
        b: controller.accentAlt,
      ),
      Text.rich(
        TextSpan(children: [
          TextSpan(
            text: '$heard of ${_thousands(all.length)}',
            style: TextStyle(fontWeight: FontWeight.w700, color: t.nInk),
          ),
          TextSpan(
              text: ' tracks heard · ${_thousands(plays)} '
                  '${plays == 1 ? 'play' : 'plays'}'),
        ]),
        style: TextStyle(fontSize: 13, color: t.nInk3),
      ),
      if (at >= 0)
        _PlayRow(
          title: 'Pick up at ${at + 1} · ${all[at].title}',
          sub: st.detailLastPlayed.isEmpty
              ? 'where you left off'
              : 'left off ${st.detailLastPlayed}',
          onTap: () =>
              controller.send(MusicCmd.resumeAlbum(albumId: st.detailId)),
        ),
      if (rows.isNotEmpty) _Kv(rows: rows),
    ];
  }

  List<Widget> _artist(BuildContext context) {
    final t = context.tokens;
    final bio = st.detailInfo.trim();
    final top = _mostPlayed(st.detailTracks);
    return [
      _SideHead(
        'About',
        action: bio.isEmpty ? null : ('Read more', () => _readMore(context)),
      ),
      // A taste, not the article: 120 characters over two lines at most, and
      // Read more has the rest.
      Text(
        bio.isEmpty ? 'No biography for this artist yet.' : _clip(bio, 120),
        maxLines: 2,
        overflow: TextOverflow.ellipsis,
        style: TextStyle(
            fontSize: 13, height: 1.55, color: bio.isEmpty ? t.nInk2 : t.nInk3),
      ),
      if (top.isNotEmpty)
        _PlayRow(
          title: 'Most played · ${top.first.title}',
          sub: [
            '${top.first.playCount} ${top.first.playCount == 1 ? 'play' : 'plays'}',
            if (top.first.album.isNotEmpty) top.first.album,
          ].join(' · '),
          onTap: () => controller.playFrom(top, 0, 'artist'),
        ),
      if (similar != null)
        FutureBuilder<List<BrowseCard>>(
          future: similar,
          builder: (context, snap) {
            final cards = snap.data ?? const <BrowseCard>[];
            if (cards.isEmpty) return const SizedBox.shrink();
            final names = cards.take(3).map((c) => c.title).join(', ');
            return Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                _SideHead('Similar', action: ('See all', onSimilar)),
                const SizedBox(height: 10),
                Row(
                  children: [
                    _Faces(controller: controller, cards: cards.take(4).toList()),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Text(
                        cards.length > 3 ? '$names +${cards.length - 3}' : names,
                        maxLines: 2,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 12.5, color: t.nInk3),
                      ),
                    ),
                  ],
                ),
              ],
            );
          },
        ),
    ];
  }

  Future<void> _readMore(BuildContext context) => showDialog<void>(
        context: context,
        builder: (ctx) => AlertDialog(
          title: Text(st.detailTitle, maxLines: 1, overflow: TextOverflow.ellipsis),
          content: SizedBox(
            width: 460,
            child: SingleChildScrollView(
              child: _Bio(controller: controller, st: st, expanded: true),
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(ctx).pop(),
              child: const Text('Close'),
            ),
          ],
        ),
      );

  List<Widget> _rules(BuildContext context) {
    final t = context.tokens;
    final n = st.detailTracks.length;
    final foot = [
      if (st.detailLibraryTotal > 0)
        'Matched ${_thousands(n)} of ${_thousands(st.detailLibraryTotal)} tracks.',
      if (st.detailLastPlayed.isNotEmpty) 'Last played ${st.detailLastPlayed}.',
    ].join(' ');
    return [
      _SideHead(
        'Rules',
        action: (
          'Edit',
          () => editSmartPlaylist(context, controller,
              playlistId: st.detailId, name: st.detailTitle),
        ),
      ),
      if (st.detailRules.isEmpty)
        Text('No rules yet, so it matches every track.',
            style: TextStyle(fontSize: 12.5, color: t.nInk2))
      else
        Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            for (var i = 0; i < st.detailRules.length; i++) ...[
              if (i > 0) const SizedBox(height: 6),
              Container(
                padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 7),
                decoration: BoxDecoration(
                  color: t.nInk.withValues(alpha: 0.06),
                  borderRadius: BorderRadius.circular(9),
                ),
                child: Row(
                  children: [
                    Icon(Icons.rule, size: 14, color: t.nInk2),
                    const SizedBox(width: 8),
                    Expanded(
                      child: Text(st.detailRules[i],
                          style: TextStyle(fontSize: 12.5, color: t.nInk)),
                    ),
                  ],
                ),
              ),
            ],
          ],
        ),
      if (foot.isNotEmpty)
        Text(foot, style: TextStyle(fontSize: 12, height: 1.5, color: t.nInk3)),
    ];
  }

  List<Widget> _decades(BuildContext context) {
    final t = context.tokens;
    final bars = st.detailBars;
    final n = _thousands(st.detailTracks.length);
    final top = bars.reduce((a, b) => b.plays > a.plays ? b : a);
    final picked = bars.where((b) => b.key == decade).firstOrNull;
    return [
      const _SideHead('Across the decades'),
      _DecadeBars(
        bars: bars,
        picked: decade,
        a: controller.accent,
        b: controller.accentAlt,
        onTap: onDecade,
      ),
      Text(
        picked != null
            ? 'Showing the ${picked.key}s: ${picked.value} of $n tracks. '
                'Click it again for every decade.'
            : '${top.value} of $n tracks are from the ${top.key}s. '
                'Click a decade to filter to it.',
        style: TextStyle(fontSize: 12.5, height: 1.5, color: t.nInk3),
      ),
    ];
  }

  List<Widget> _folder(BuildContext context) {
    final t = context.tokens;
    final all = st.detailTracks;
    final formats = _formatCounts(all);
    final root = _trimSep(st.detailKey);
    final subs = all
        .map((x) => File(x.path).parent.path)
        .where((d) => d.length > root.length && d.startsWith(root))
        .map((d) => d.substring(root.length + 1).split(Platform.pathSeparator).first)
        .toSet()
        .length;
    final untagged =
        all.where((x) => x.artist.trim().isEmpty || x.album.trim().isEmpty).length;
    final rows = <(String, String, VoidCallback?)>[
      if (subs > 0) ('Subfolders', '$subs', null),
      if (untagged > 0)
        (
          'Missing tags',
          '$untagged ${untagged == 1 ? 'file' : 'files'} ›',
          () async {
            await controller.send(MusicCmd.mgrOpen(tab: 'tags'));
            await controller.send(MusicCmd.mgrSetFilter(key: 'missing'));
          },
        ),
      if (st.detailRoot.isNotEmpty) ('Belongs to', _basename(st.detailRoot), null),
      if (st.detailLastPlayed.isNotEmpty) ('Last played', st.detailLastPlayed, null),
    ];
    return [
      const _SideHead('What’s in it'),
      Column(
        children: [
          for (final (i, (ext, count)) in formats.indexed)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 3),
              child: Row(
                children: [
                  Container(
                    width: 10,
                    height: 10,
                    decoration: BoxDecoration(
                      color: _formatColor(i),
                      borderRadius: BorderRadius.circular(3),
                    ),
                  ),
                  const SizedBox(width: 8),
                  Text(ext, style: TextStyle(fontSize: 12.5, color: t.nInk)),
                  const Spacer(),
                  Text(
                    _thousands(count),
                    style: TextStyle(
                      fontSize: 12.5,
                      color: t.nInk2,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    ),
                  ),
                ],
              ),
            ),
        ],
      ),
      if (rows.isNotEmpty) _Kv(rows: rows),
    ];
  }
}

class _SideHead extends StatelessWidget {
  const _SideHead(this.title, {this.action});

  final String title;
  final (String, VoidCallback)? action;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final action = this.action;
    return Row(
      children: [
        Expanded(
          child: Text(
            title.toUpperCase(),
            style: TextStyle(
              fontFamily: Tokens.fontFamily,
              fontSize: 11,
              fontWeight: FontWeight.w700,
              letterSpacing: 1.5,
              color: t.nInk2,
            ),
          ),
        ),
        if (action != null) _TextLink(label: action.$1, onTap: action.$2),
      ],
    );
  }
}

class _TextLink extends StatefulWidget {
  const _TextLink({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  State<_TextLink> createState() => _TextLinkState();
}

class _TextLinkState extends State<_TextLink> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) => MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hovered = true),
        onExit: (_) => setState(() => _hovered = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: Text(
            widget.label,
            style: TextStyle(
              fontSize: 12,
              fontWeight: FontWeight.w600,
              color: Tokens.secMusic,
              decoration: _hovered ? TextDecoration.underline : null,
              decorationColor: Tokens.secMusic,
            ),
          ),
        ),
      );
}

/// Label on the left, value on the right; a value with somewhere to go is a
/// link.
class _Kv extends StatelessWidget {
  const _Kv({required this.rows});

  final List<(String, String, VoidCallback?)> rows;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      children: [
        for (final (label, value, onTap) in rows)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 2.5),
            child: Row(
              children: [
                Text(label, style: TextStyle(fontSize: 12.5, color: t.nInk2)),
                const SizedBox(width: 14),
                Expanded(
                  child: Align(
                    alignment: Alignment.centerRight,
                    child: onTap != null
                        ? _TextLink(label: value, onTap: onTap)
                        : Text(
                            value,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                              fontSize: 12.5,
                              color: t.nInk,
                              fontFeatures: const [FontFeature.tabularFigures()],
                            ),
                          ),
                  ),
                ),
              ],
            ),
          ),
      ],
    );
  }
}

class _Meter extends StatelessWidget {
  const _Meter({required this.value, required this.a, required this.b});

  final double value;
  final Color a;
  final Color b;

  @override
  Widget build(BuildContext context) => ClipRRect(
        borderRadius: BorderRadius.circular(3),
        child: SizedBox(
          height: 6,
          child: Stack(
            children: [
              Positioned.fill(child: ColoredBox(color: context.tokens.nChip)),
              FractionallySizedBox(
                alignment: Alignment.centerLeft,
                widthFactor: value.clamp(0.0, 1.0),
                heightFactor: 1,
                child: DecoratedBox(
                  decoration: BoxDecoration(gradient: LinearGradient(colors: [a, b])),
                ),
              ),
            ],
          ),
        ),
      );
}

/// A round play button and two lines: where to resume, what gets played most.
class _PlayRow extends StatelessWidget {
  const _PlayRow({required this.title, required this.sub, required this.onTap});

  final String title;
  final String sub;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: t.nInk.withValues(alpha: 0.06),
      borderRadius: BorderRadius.circular(10),
      clipBehavior: Clip.antiAlias,
      child: InkWell(
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.all(8),
          child: Row(
            children: [
              const DecoratedBox(
                decoration: BoxDecoration(
                  color: Tokens.secMusic,
                  shape: BoxShape.circle,
                ),
                child: SizedBox.square(
                  dimension: 34,
                  child: Icon(Icons.play_arrow_rounded,
                      size: 20, color: Colors.white),
                ),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 13, fontWeight: FontWeight.w600, color: t.nInk),
                    ),
                    Text(
                      sub,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 11.5, color: t.nInk2),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Up to four artist portraits, overlapping.
class _Faces extends StatelessWidget {
  const _Faces({required this.controller, required this.cards});

  final MusicController controller;
  final List<BrowseCard> cards;

  @override
  Widget build(BuildContext context) {
    final ring = context.tokens.nCard;
    return SizedBox(
      width: 30 + 22.0 * (cards.length - 1),
      height: 30,
      child: Stack(
        children: [
          for (var i = 0; i < cards.length; i++)
            Positioned(
              left: 22.0 * i,
              child: Container(
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  border: Border.all(color: ring, width: 2),
                ),
                child: MusicArt(
                  controller: controller,
                  kind: 'artist',
                  artKey: cards[i].key,
                  direct: cards[i].art.isEmpty ? null : cards[i].art,
                  size: 26,
                  radius: 13,
                  fallback: Icons.mic_none,
                ),
              ),
            ),
        ],
      ),
    );
  }
}

/// How a genre spreads across the decades you own. A bar is also a filter:
/// clicking one narrows the list to that decade, clicking it again undoes it.
class _DecadeBars extends StatelessWidget {
  const _DecadeBars({
    required this.bars,
    required this.picked,
    required this.a,
    required this.b,
    required this.onTap,
  });

  final List<Tally> bars;
  final int? picked;
  final Color a;
  final Color b;
  final ValueChanged<int> onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final top = bars.reduce((x, y) => y.plays > x.plays ? y : x);
    return SizedBox(
      height: 84,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          for (final bar in bars)
            Expanded(
              child: Tooltip(
                message: '${bar.value} tracks',
                child: MouseRegion(
                  cursor: SystemMouseCursors.click,
                  child: GestureDetector(
                    behavior: HitTestBehavior.opaque,
                    onTap: () => onTap(bar.key),
                    child: Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 3),
                      child: Column(
                        mainAxisAlignment: MainAxisAlignment.end,
                        children: [
                          Opacity(
                            opacity: picked != null && picked != bar.key ? 0.4 : 1,
                            child: Container(
                              // A floor, so a decade with one record is a short
                              // bar rather than a gap in the run of years.
                              height: (4 + bar.frac * 58).clamp(4.0, 62.0),
                              decoration: BoxDecoration(
                                borderRadius: const BorderRadius.vertical(
                                  top: Radius.circular(4),
                                  bottom: Radius.circular(2),
                                ),
                                gradient: LinearGradient(
                                  begin: Alignment.topCenter,
                                  end: Alignment.bottomCenter,
                                  colors: picked == bar.key ||
                                          (picked == null && identical(bar, top))
                                      ? [Colors.white, a]
                                      : [a, Color.lerp(a, b, 0.6)!],
                                ),
                              ),
                            ),
                          ),
                          const SizedBox(height: 4),
                          Text(
                            bar.label,
                            style: TextStyle(fontSize: 10, color: t.nInk2),
                          ),
                        ],
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

// ---------------------------------------------------------------- tools --

class _ToolChip extends StatelessWidget {
  const _ToolChip({required this.icon, required this.label, this.on = false, this.onTap});

  final IconData icon;
  final String label;
  final bool on;

  /// Null inside a PopupMenuButton, which takes the tap itself.
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final ink = on ? (t.dark ? const Color(0xFFF9A8D4) : Tokens.secMusic) : t.nInk3;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onTap,
        child: Container(
          height: 34,
          padding: const EdgeInsets.symmetric(horizontal: 12),
          decoration: BoxDecoration(
            color: on ? Tokens.secMusic.withValues(alpha: 0.18) : null,
            borderRadius: BorderRadius.circular(17),
            border: Border.all(
              color: on
                  ? Tokens.secMusic.withValues(alpha: 0.6)
                  : t.nInk.withValues(alpha: 0.12),
            ),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(icon, size: 15, color: ink),
              const SizedBox(width: 6),
              Text(
                label,
                style: TextStyle(fontSize: 12.5, fontWeight: FontWeight.w600, color: ink),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _Segmented extends StatelessWidget {
  const _Segmented({required this.labels, required this.selected, required this.onTap});

  final List<String> labels;
  final int selected;
  final ValueChanged<int> onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(3),
      decoration: BoxDecoration(
        color: t.nInk.withValues(alpha: 0.06),
        borderRadius: BorderRadius.circular(17),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (var i = 0; i < labels.length; i++)
            MouseRegion(
              cursor: SystemMouseCursors.click,
              child: GestureDetector(
                onTap: () => onTap(i),
                child: Container(
                  height: 28,
                  alignment: Alignment.center,
                  padding: const EdgeInsets.symmetric(horizontal: 12),
                  decoration: BoxDecoration(
                    color: i == selected ? t.nInk : null,
                    borderRadius: BorderRadius.circular(14),
                  ),
                  child: Text(
                    labels[i],
                    style: TextStyle(
                      fontSize: 12.5,
                      fontWeight: FontWeight.w600,
                      color: i == selected ? t.nCanvas : t.nInk2,
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

class _FilterField extends StatelessWidget {
  const _FilterField({required this.controller, required this.hint, this.width = 250});

  final TextEditingController controller;
  final String hint;

  /// Null to take what the parent allows.
  final double? width;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      width: width,
      height: 34,
      padding: const EdgeInsets.symmetric(horizontal: 12),
      decoration: BoxDecoration(
        color: t.nInk.withValues(alpha: 0.06),
        borderRadius: BorderRadius.circular(17),
        border: Border.all(color: t.nInk.withValues(alpha: 0.10)),
      ),
      child: Row(
        children: [
          Icon(Icons.search, size: 16, color: t.nInk2),
          const SizedBox(width: 8),
          Expanded(
            child: TextField(
              controller: controller,
              style: TextStyle(fontSize: 13, color: t.nInk),
              // Every border named: the app theme's outline would otherwise
              // draw a box inside the pill.
              decoration: InputDecoration(
                isCollapsed: true,
                filled: false,
                hintText: hint,
                hintStyle: TextStyle(fontSize: 13, color: t.nInk2),
                border: InputBorder.none,
                enabledBorder: InputBorder.none,
                focusedBorder: InputBorder.none,
              ),
            ),
          ),
          if (controller.text.isNotEmpty)
            GestureDetector(
              onTap: controller.clear,
              child: Icon(Icons.close, size: 15, color: t.nInk2),
            ),
        ],
      ),
    );
  }
}

// ---------------------------------------------------------------- lists --

/// "DISC 2 ————— 39:52", or an artist or a subfolder when the page groups.
class _GroupHead extends StatelessWidget {
  const _GroupHead({required this.label, required this.seconds});

  final String label;
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
        Flexible(
          child: Text(label.toUpperCase(),
              maxLines: 1, overflow: TextOverflow.ellipsis, style: style),
        ),
        const SizedBox(width: 10),
        Expanded(child: Divider(height: 1, color: t.outline)),
        const SizedBox(width: 10),
        Text(fmtClock(seconds), style: style),
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
    this.groupOf,
    this.selected = const {},
    this.onSelect,
    this.from = 0,
    this.to,
  });

  /// Item ids currently picked, and how a row reports being picked.
  final Set<int> selected;
  final void Function(int index, {required bool range})? onSelect;

  final MusicController controller;
  final MusicState st;

  /// Already filtered and sorted by the page.
  final List<Track> tracks;

  /// The page of [tracks] to draw, `from` inclusive, `to` exclusive (null for
  /// the end). Indices everywhere else stay indices into the whole list, so a
  /// row plays, selects and reorders as its place in the playlist.
  final int from;
  final int? to;

  /// Whether disc dividers may be drawn -- only true in the bridge's own
  /// order, which is the only order in which discs are contiguous.
  final bool grouped;

  /// The page's own grouping, an artist or a subfolder, which the page has
  /// already made contiguous. Wins over discs.
  final String Function(Track)? groupOf;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (tracks.isEmpty) {
      return Text('No tracks.', style: TextStyle(fontSize: 13, color: t.nInk2));
    }
    // A playlist has an order the user chose, so its rows are draggable; an
    // album's order is the album's and dragging it would mean nothing.
    final end = to ?? tracks.length;
    if (st.detailKind == 'playlist') {
      return ReorderableListView.builder(
        shrinkWrap: true,
        physics: const NeverScrollableScrollPhysics(),
        itemCount: end - from,
        // Reordering only means something in the stored order; a filtered or
        // re-sorted view would send the wrong indices to `playlistMove`.
        buildDefaultDragHandles: grouped,
        // onReorderItem, unlike the deprecated onReorder, already accounts for
        // the lifted row, so `b` is the destination the store should store --
        // on this page, which starts `from` rows into the playlist.
        onReorderItem: (a, b) => controller.send(MusicCmd.playlistMove(
          playlistId: st.detailId,
          from: from + a,
          to: from + b,
        )),
        itemBuilder: (_, j) {
          final i = from + j;
          return TrackRow(
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
          );
        },
      );
    }
    // A disc divider is only worth drawing when there is more than one disc
    // to separate; a single-disc album, and every untagged one, draws none.
    final group = groupOf ??
        (grouped &&
                tracks.map((x) => x.discNo).where((d) => d > 0).toSet().length > 1
            ? (Track x) => 'Disc ${x.discNo}'
            : null);
    final seconds = <String, double>{};
    if (group != null) {
      for (final x in tracks) {
        final k = group(x);
        seconds[k] = (seconds[k] ?? 0) + x.durationS;
      }
    }

    return Column(
      children: [
        for (var i = from; i < end; i++) ...[
          if (group != null &&
              (i == from || group(tracks[i]) != group(tracks[i - 1]))) ...[
            if (i > from) const SizedBox(height: 14),
            _GroupHead(
              label: group(tracks[i]),
              seconds: seconds[group(tracks[i])] ?? 0,
            ),
            const SizedBox(height: 4),
          ] else if (i > from)
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

/// Artists on these shelves who sound like this one.
///
/// Local, and that is the point: the ranking is shared genres, overlapping era
/// and the distance between the two artists' fingerprints, so every name here
/// is something already owned and one tap from playing. A recommendation that
/// cannot be played is an advertisement.
///
/// Draws nothing at all when there is no answer — an unanalysed library with
/// no genre tags has nothing to go on, and an empty "Similar artists" heading
/// is worse than no heading.
class _SimilarArtists extends StatelessWidget {
  const _SimilarArtists({required this.controller, required this.future});

  final MusicController controller;

  /// The page's, shared with the side card's preview.
  final Future<List<BrowseCard>> future;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return FutureBuilder<List<BrowseCard>>(
      future: future,
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
                        controller: controller,
                        title: card.title,
                        subtitle: card.subtitle,
                        artKind: 'artist',
                        artKey: card.key,
                        direct: card.art.isEmpty ? null : card.art,
                        round: true,
                        centred: true,
                        fallback: Icons.mic_none,
                        onTap: () => controller
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

// ---------------------------------------------------------------- helpers --

/// What Play means on this page: the list in its order -- or, on an artist,
/// what you play most, since that is what the page opens on.
Future<void> _playAll(MusicController c, MusicState st) {
  if (st.detailKind == 'artist') {
    final top = _mostPlayed(st.detailTracks);
    if (top.length >= 3) return c.playFrom(top, 0, 'artist');
  }
  return c.playFrom(st.detailTracks, 0, st.detailKind);
}

Future<void> _shuffle(MusicController c, MusicState st) async {
  if (st.detailTracks.isEmpty) return;
  if (!st.shuffle) await c.send(const MusicCmd.toggleShuffle());
  await c.playFrom(st.detailTracks, 0, st.detailKind);
}

Future<void> _deletePlaylist(
    BuildContext context, MusicController c, MusicState st) async {
  await confirmThen(
    context,
    c,
    title: 'Delete this playlist?',
    body: '“${st.detailTitle}” and its order go. The tracks in it stay in the '
        'library.',
    action: 'Delete playlist',
    cmd: MusicCmd.playlistDelete(playlistId: st.detailId),
  );
}

/// A picture for the page's subject. Every kind keeps it somewhere different --
/// an album in `albums.cover_path`, an artist in `artists.image_path`, a genre
/// and a playlist in a preference -- so this is one control over four stores.
Future<void> _pickArt(MusicController c, MusicState st) async {
  final path = await pickFile(
    label: 'Images',
    extensions: const ['jpg', 'jpeg', 'png', 'webp', 'bmp'],
  );
  if (path == null) return;
  await c.setCardArt(st.detailKind, _artKey(st), path);
}

String _artKey(MusicState st) => switch (st.detailKind) {
      'album' || 'artist' => '${st.detailId}',
      _ => st.detailKey,
    };

bool _hasArt(MusicController c, MusicState st) =>
    st.detailArt.isNotEmpty || c.artFor(st.detailKind, _artKey(st)) != null;

Widget _art(MusicController c, MusicState st, double side) =>
    // Four covers when a playlist has no picture of its own. Rust sends four
    // or none: two covers in a four-up grid is a broken tile, not a collage.
    st.detailCollage.length == 4
        ? _Collage(paths: st.detailCollage, side: side)
        : MusicArt(
            controller: c,
            kind: st.detailKind,
            artKey: _artKey(st),
            direct: st.detailArt,
            size: side,
            radius: 0,
            fallback: switch (st.detailKind) {
              'artist' => Icons.mic_none,
              'playlist' => Icons.queue_music,
              'folder' => Icons.folder,
              'genre' => Icons.library_music_outlined,
              _ => Icons.album_outlined,
            },
          );

List<Track> _mostPlayed(List<Track> all) =>
    all.where((x) => x.playCount > 0).toList()
      ..sort((a, b) => b.playCount.compareTo(a.playCount));

String _sortLabel(_Sort s, MusicState st) => s != _Sort.natural
    ? s.label
    : switch (st.detailKind) {
        'album' => 'Track order',
        'playlist' => st.detailIsSmart ? 'Rule order' : 'Playlist order',
        _ => 'Album order',
      };

String? _meta(MusicState st, String label) {
  for (final row in st.detailMeta) {
    if (row.label == label) return row.value;
  }
  return null;
}

/// The year it came out: the exact release date's, else the earliest tagged.
String? _year(MusicState st) {
  final released = _meta(st, 'Released');
  if (released != null && released.length >= 4) return released.substring(0, 4);
  final years = st.detailTracks.map((x) => x.year).where((y) => y > 0);
  return years.isEmpty ? null : '${years.reduce((a, b) => a < b ? a : b)}';
}

String _genreNote(MusicState st) {
  final artists =
      st.detailTracks.map((x) => x.artist).where((a) => a.isNotEmpty).toSet().length;
  return [
    if (st.detailBars.isNotEmpty)
      'Mostly the ${st.detailBars.reduce((a, b) => b.plays > a.plays ? b : a).key}s',
    if (artists > 0) '$artists ${artists == 1 ? 'artist' : 'artists'}',
  ].join(' · ');
}

List<String> _topGenres(List<Track> tracks, [int take = 3]) {
  final n = <String, int>{};
  for (final t in tracks) {
    final g = t.genre.trim();
    if (g.isNotEmpty) n[g] = (n[g] ?? 0) + 1;
  }
  final keys = n.keys.toList()..sort((a, b) => n[b]!.compareTo(n[a]!));
  return keys.take(take).toList();
}

/// The page's numbers, per kind: (value, label).
List<(String, String)> _tiles(MusicState st) {
  final all = st.detailTracks;
  final n = _thousands(all.length);
  final runtime = _runtime(all.fold<double>(0, (a, x) => a + x.durationS));
  final loved = all.where((x) => x.loved).length;
  int distinct(String Function(Track) f) =>
      all.map(f).where((s) => s.isNotEmpty).toSet().length;
  switch (st.detailKind) {
    case 'album':
      // "FLAC · 44.1 kHz · 1411 kbps · stereo": the codec is the tile, the
      // two numbers after it its caption.
      final format = _meta(st, 'Format')?.split(' · ');
      final best = _bestFormat(all);
      final gain = _meta(st, 'Album gain');
      return [
        (n, all.length == 1 ? 'Track' : 'Tracks'),
        (runtime, 'Runtime'),
        if (format != null && format.isNotEmpty)
          (format.first, format.length > 1 ? format.skip(1).take(2).join(' · ') : 'Format')
        else if (best != null)
          (best, 'Format'),
        if (gain != null) (gain, 'Album gain'),
      ];
    case 'artist':
      return [
        ('${st.detailAlbums.length}', 'Albums'),
        (n, 'Tracks'),
        (_thousands(all.fold<int>(0, (a, x) => a + x.playCount)), 'Plays'),
        ('$loved', 'Loved'),
      ];
    case 'playlist':
      return [
        (n, 'Tracks'),
        (runtime, 'Runtime'),
        ('${distinct((x) => x.artist)}', 'Artists'),
        ('$loved', 'Loved'),
      ];
    case 'genre':
      final rated = all.where((x) => x.stars > 0).toList();
      return [
        (n, 'Tracks'),
        ('${distinct((x) => x.album)}', 'Albums'),
        (runtime, 'Runtime'),
        if (rated.isNotEmpty)
          (
            '${(rated.fold<int>(0, (a, x) => a + x.stars) / rated.length).toStringAsFixed(1)} ★',
            'Avg rating',
          ),
      ];
    case 'folder':
      return [
        (n, 'Files'),
        ('${distinct((x) => x.album)}', 'Albums'),
        (runtime, 'Runtime'),
        if (st.detailDiskBytes > 0) (_bytes(st.detailDiskBytes), 'On disk'),
      ];
  }
  return const [];
}

/// Ranked worst to best, so the highest index present is the best copy.
const _formatRank = ['mp3', 'm4a', 'aac', 'ogg', 'opus', 'alac', 'flac', 'wav'];

String? _bestFormat(List<Track> tracks) {
  final best = tracks
      .map((x) => _formatRank.indexOf(x.path.split('.').last.toLowerCase()))
      .fold(-1, (a, b) => b > a ? b : a);
  return best < 0 ? null : _formatRank[best].toUpperCase();
}

/// Extension counts, commonest first.
List<(String, int)> _formatCounts(List<Track> tracks) {
  final n = <String, int>{};
  for (final t in tracks) {
    final dot = t.path.lastIndexOf('.');
    final ext = dot < 0 ? '?' : t.path.substring(dot + 1).toUpperCase();
    n[ext] = (n[ext] ?? 0) + 1;
  }
  return [for (final e in n.entries) (e.key, e.value)]
    ..sort((a, b) => b.$2.compareTo(a.$2));
}

const _formatPalette = [
  Color(0xFF22D3EE),
  Color(0xFFF472B6),
  Color(0xFFFBBF24),
  Color(0xFFA78BFA),
  Color(0xFF34D399),
  Color(0xFFF87171),
];

Color _formatColor(int i) => _formatPalette[i % _formatPalette.length];

/// "42:39" under an hour, "3 h 12 m" over.
String _runtime(double secs) {
  final m = (secs / 60).round();
  return m < 60
      ? fmtClock(secs)
      : '${m ~/ 60} h ${(m % 60).toString().padLeft(2, '0')} m';
}

/// Rows on one page of a playlist.
const int _kPlaylistPage = 25;

/// At most [max] characters, cut at a word where there is one nearby.
String _clip(String s, int max) {
  if (s.length <= max) return s;
  final cut = s.substring(0, max);
  final space = cut.lastIndexOf(' ');
  return '${(space > max * 0.6 ? cut.substring(0, space) : cut).trimRight()}…';
}

String _thousands(int n) =>
    n.toString().replaceAllMapped(RegExp(r'\B(?=(\d{3})+$)'), (_) => ',');

String _bytes(int b) {
  const gb = 1 << 30, mb = 1 << 20;
  if (b >= gb) return '${(b / gb).toStringAsFixed(1)} GB';
  if (b >= mb) return '${(b / mb).round()} MB';
  return '${(b / 1024).ceil()} KB';
}

String _trimSep(String path) =>
    path.length > 1 && path.endsWith(Platform.pathSeparator)
        ? path.substring(0, path.length - 1)
        : path;

String _basename(String path) =>
    _trimSep(path).split(Platform.pathSeparator).where((s) => s.isNotEmpty).lastOrNull ??
    path;
