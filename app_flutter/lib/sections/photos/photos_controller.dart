// Photos section state. Everything ephemeral lives here rather than in Rust:
// selection, the clipboard, which tab is blinking, whether the album picker is
// open. The Slint build had to declare all of that as UI properties because
// Slint has no store; crossing the FFI boundary for it would be reproducing a
// shape that only existed because of the framework.

// frb generates its own Int64List (a BigInt-strict wrapper), not the one in
// dart:typed_data. Importing the latter here makes every batched-command call
// a type mismatch against the generated signatures.
import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart' show Int64List;

import '../../src/rust/api/photos.dart';

/// The tabs, in the order the Slint page shows them.
/// The tabs, in the order and with the icons and tints ui/page_photos.slint
/// gives its `HdrChip` row. Slint paints each chip with a two-stop gradient;
/// the first stop is `tint` here and the second is `tint2`.
///
/// `drill` is the category a tab lands on once you open one of its cards, so
/// the tab still reads as active while you are inside it.
class PhotoCategory {
  const PhotoCategory(
    this.id,
    this.label,
    this.icon,
    this.tint,
    this.tint2, {
    this.drill,
  });

  final String id;
  final String label;
  final IconData icon;
  final Color tint;
  final Color tint2;
  final String? drill;

  bool isActive(String current) => current == id || current == drill;
}

const List<PhotoCategory> photoCategories = [
  PhotoCategory('recent', 'Timeline', Icons.schedule, Color(0xFF8B5CF6),
      Color(0xFFEC4899)),
  PhotoCategory('library', 'Library', Icons.folder_outlined, Color(0xFF3B82F6),
      Color(0xFF06B6D4)),
  PhotoCategory(
      'starred', 'Starred', Icons.star, Color(0xFFF59E0B), Color(0xFFF97316)),
  PhotoCategory('archive', 'Archive', Icons.archive_outlined,
      Color(0xFF14B8A6), Color(0xFF0EA5E9)),
  PhotoCategory('trash', 'Trash', Icons.delete_outline, Color(0xFFF43F5E),
      Color(0xFFE11D48)),
  PhotoCategory('people', 'People', Icons.person_outline, Color(0xFFEC4899),
      Color(0xFFF43F5E),
      drill: 'facephotos'),
  PhotoCategory('things', 'Things', Icons.sell_outlined, Color(0xFF22C55E),
      Color(0xFF14B8A6),
      drill: 'tagphotos'),
  PhotoCategory('albums', 'Albums', Icons.photo_album_outlined,
      Color(0xFFA855F7), Color(0xFF8B5CF6),
      drill: 'album'),
  PhotoCategory('memories', 'Memories', Icons.movie_outlined,
      Color(0xFFF97316), Color(0xFFEC4899)),
  PhotoCategory('places', 'Places', Icons.place_outlined, Color(0xFF06B6D4),
      Color(0xFF3B82F6)),
  PhotoCategory('dedupe', 'Dedupe', Icons.copy_all_outlined, Color(0xFF84CC16),
      Color(0xFF22C55E)),
];

class PhotosController extends ChangeNotifier {
  PhotosState? state;
  Object? error;
  bool busy = false;

  /// Item ids, not tile indices: a refresh reorders the grid, and a selection
  /// that survives a star or a trash has to survive that reorder with it.
  final Set<int> selected = <int>{};

  /// item_id → thumbnail path, filled in as tiles scroll into view. Survives
  /// a refresh, which is the point — re-querying the cache for a tile that
  /// already painted is a wasted FFI round trip per tile per scroll.
  final Map<int, String> _thumbs = <int, String>{};
  final Set<int> _thumbsInFlight = <int>{};

  Stream<PhotosEvent> get events => photosEvents();

  String get category => state?.category ?? 'recent';
  bool get selecting => selected.isNotEmpty;

  Future<void> send(PhotosCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      state = await photosDispatch(cmd: cmd);
      // Drop selections the new view no longer contains, so the action bar
      // can never act on a photo that is not on screen.
      final visible = state!.tiles.map((t) => t.itemId).toSet();
      selected.removeWhere((id) => !visible.contains(id));
    } catch (e) {
      error = e;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  Future<void> refresh() => send(const PhotosCmd.refresh());

  void toggleSelect(int itemId) {
    if (!selected.remove(itemId)) selected.add(itemId);
    notifyListeners();
  }

  void selectAll() {
    selected.addAll(state?.tiles.map((t) => t.itemId) ?? const <int>[]);
    notifyListeners();
  }

  void clearSelection() {
    selected.clear();
    notifyListeners();
  }

  /// Single-item writes for the viewer, which acts on the photo on screen and
  /// never on the grid's selection. Each one re-queries, so the viewer sees the
  /// new flag through the same snapshot the grid does.
  Future<void> setStar(int itemId, bool starred) =>
      send(PhotosCmd.star(itemId: itemId, starred: starred));

  Future<void> setArchived(int itemId, bool archived) => send(
        PhotosCmd.archive(
          itemIds: Int64List.fromList([itemId]),
          archived: archived,
        ),
      );

  Future<void> trashOne(int itemId) =>
      send(PhotosCmd.trash(itemIds: Int64List.fromList([itemId])));

  Future<void> starSelection(bool starred) async {
    // star::set is per item; the rest of the section's writes are batched.
    for (final id in selected.toList()) {
      await photosDispatch(cmd: PhotosCmd.star(itemId: id, starred: starred));
    }
    selected.clear();
    await refresh();
  }

  Future<void> archiveSelection(bool archived) =>
      _batch((ids) => PhotosCmd.archive(itemIds: ids, archived: archived));

  Future<void> trashSelection() =>
      _batch((ids) => PhotosCmd.trash(itemIds: ids));

  Future<void> restoreSelection() =>
      _batch((ids) => PhotosCmd.restore(itemIds: ids));

  Future<void> addSelectionToAlbum(int albumId) =>
      _batch((ids) => PhotosCmd.albumAdd(albumId: albumId, itemIds: ids));

  Future<void> _batch(PhotosCmd Function(Int64List ids) build) async {
    if (selected.isEmpty) return;
    final ids = Int64List.fromList(selected.toList());
    selected.clear();
    await send(build(ids));
  }

  /// Path to the tile's 256px thumbnail, rendering it first if the cache does
  /// not hold it. Returns null when the file cannot be decoded at all.
  Future<String?> thumbFor(PhotoTile tile) async {
    if (tile.thumb.isNotEmpty) return tile.thumb;
    final cached = _thumbs[tile.itemId];
    if (cached != null) return cached;
    // Two tiles for the same item (a stack, a search hit that is also on
    // screen) must not both kick off a decode.
    if (!_thumbsInFlight.add(tile.itemId)) return null;
    try {
      final path = await photosEnsureThumb(itemId: tile.itemId);
      if (path != null) _thumbs[tile.itemId] = path;
      return path;
    } finally {
      _thumbsInFlight.remove(tile.itemId);
    }
  }
}
