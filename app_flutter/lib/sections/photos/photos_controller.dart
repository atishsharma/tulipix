// Photos section state. Everything ephemeral lives here rather than in Rust:
// selection, the clipboard, which tab is blinking, whether the album picker is
// open. The Slint build had to declare all of that as UI properties because
// Slint has no store; crossing the FFI boundary for it would be reproducing a
// shape that only existed because of the framework.

// frb generates its own Int64List (a BigInt-strict wrapper), not the one in
// dart:typed_data. Importing the latter here makes every batched-command call
// a type mismatch against the generated signatures.
import 'package:flutter/foundation.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart' show Int64List;

import '../../src/rust/api/photos.dart';

/// The tabs, in the order the Slint page shows them.
const List<(String, String)> photoCategories = [
  ('recent', 'Timeline'),
  ('starred', 'Starred'),
  ('albums', 'Albums'),
  ('places', 'Places'),
  ('library', 'Library'),
  ('archive', 'Archive'),
  ('trash', 'Trash'),
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
