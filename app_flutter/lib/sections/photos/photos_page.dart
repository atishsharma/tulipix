// Photos — Google Photos language: light & airy, tight justified grid.
// Tabs: Timeline · Library · Starred · Archive · Trash · People · Things ·
// Albums · Memories · Places · Dedupe, in the order ui/page_photos.slint
// lists them. Three of them are not grids -- Library is a folder list, Places
// is a map over the basemap, Dedupe is a side-by-side compare -- so `_Body`
// routes on the category rather than every tab sharing one view.
// Ports ui/page_photos.slint.

import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/photos.dart';
import 'photo_tile.dart';
import 'photo_viewer.dart';
import 'photos_controller.dart';
import 'places_map.dart';

const double _kTileExtent = 168;
const double _kTileGap = 3;

class PhotosPage extends StatefulWidget {
  const PhotosPage({super.key});

  @override
  State<PhotosPage> createState() => _PhotosPageState();
}

class _PhotosPageState extends State<PhotosPage> {
  final PhotosController _c = PhotosController();
  final TextEditingController _search = TextEditingController();
  StreamSubscription<PhotosEvent>? _events;
  String? _scanStatus;

  @override
  void initState() {
    super.initState();
    _c.refresh();
    // Each photosEvents() call registers its own sink on the Rust side, so the
    // subscription has to be cancelled with the page, not left to the GC.
    _events = _c.events.listen(_onEvent);
  }

  @override
  void dispose() {
    _events?.cancel();
    _search.dispose();
    _c.dispose();
    super.dispose();
  }

  void _onEvent(PhotosEvent e) {
    if (!mounted) return;
    setState(() {
      _scanStatus = switch (e) {
        PhotosEvent_ScanStarted(:final root) => 'Scanning $root…',
        PhotosEvent_ScanFinished(:final scanned, :final inserted) =>
          'Scanned $scanned, added $inserted',
        PhotosEvent_ScanFailed(:final message) => 'Scan failed: $message',
      };
    });
    // A finished scan changed the library under the grid.
    if (e is PhotosEvent_ScanFinished) _c.refresh();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // Material, not ColoredBox: the folder list is built from ListTiles, and a
    // ListTile paints its background and its ink splash onto the nearest
    // Material ancestor. A plain coloured box between the two hides both --
    // Flutter asserts on exactly this. Painting the page background *with* a
    // Material gives the tiles the surface they were already looking for
    // instead of adding another layer to find it through.
    return Material(
      color: t.nCanvas,
      child: ListenableBuilder(
        listenable: _c,
        builder: (context, _) {
          final state = _c.state;
          return Column(
            children: [
              _Header(controller: _c, search: _search, status: _scanStatus),
              _CategoryChips(controller: _c),
              Expanded(
                child: state == null
                    ? FirstLoad(error: _c.error, onRetry: _c.refresh)
                    : _Body(controller: _c, state: state),
              ),
              if (_c.selecting) _SelectionBar(controller: _c),
            ],
          );
        },
      ),
    );
  }
}

// ---------------------------------------------------------------- header ----

class _Header extends StatelessWidget {
  const _Header({required this.controller, required this.search, this.status});

  final PhotosController controller;
  final TextEditingController search;
  final String? status;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final state = controller.state;
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 8),
      child: Row(
        children: [
          Text(
            'Photos',
            style: TextStyle(
              fontFamily: Tokens.fontFamily,
              fontSize: 26,
              fontWeight: FontWeight.w600,
              color: t.nInk,
            ),
          ),
          if (state != null)
            for (final crumb in [_crumbFor(state)])
              if (crumb != null) ...[
                Icon(Icons.chevron_right, size: 20, color: t.nInk2),
                InkWell(
                  onTap: () =>
                      controller.send(PhotosCmd.setCategory(name: crumb.$1)),
                  borderRadius: BorderRadius.circular(6),
                  child: Padding(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
                    child: Text(
                      crumb.$2,
                      style: const TextStyle(
                        fontFamily: Tokens.fontFamily,
                        fontSize: 20,
                        fontWeight: FontWeight.w500,
                        color: Tokens.secPhotos,
                      ),
                    ),
                  ),
                ),
              ],
          const SizedBox(width: 12),
          Text(
            status ?? '${state?.itemCount ?? 0} items',
            style: TextStyle(fontSize: 13, color: t.nInk2),
          ),
          const Spacer(),
          SizedBox(
            width: 280,
            height: 40,
            child: TextField(
              controller: search,
              onSubmitted: (q) => controller.send(PhotosCmd.search(query: q)),
              style: TextStyle(fontSize: 14, color: t.nInk),
              decoration: InputDecoration(
                hintText: 'Search photos, people, tags',
                hintStyle: TextStyle(fontSize: 14, color: t.nInk2),
                prefixIcon: Icon(Icons.search, size: 18, color: t.nInk2),
                suffixIcon: (controller.state?.query ?? '').isEmpty
                    ? null
                    : IconButton(
                        icon: Icon(Icons.close, size: 16, color: t.nInk2),
                        onPressed: () {
                          search.clear();
                          controller.send(const PhotosCmd.search(query: ''));
                        },
                      ),
                filled: true,
                fillColor: t.nChip,
                contentPadding: EdgeInsets.zero,
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(20),
                  borderSide: BorderSide.none,
                ),
              ),
            ),
          ),
          const SizedBox(width: 8),
          _SortMenu(controller: controller),
          IconButton(
            tooltip: 'Rescan watched folders',
            icon: Icon(Icons.refresh, color: t.nInk2),
            onPressed: () => controller.send(const PhotosCmd.scan()),
          ),
          IconButton(
            tooltip: 'Add folder',
            icon: Icon(Icons.create_new_folder_outlined, color: t.nInk2),
            onPressed: () => _promptAddFolder(context, controller),
          ),
        ],
      ),
    );
  }
}

/// Typed path rather than a native directory chooser: a picker means another
/// package, and the shell (phase 4) owns file dialogs for every section.
Future<void> _promptAddFolder(BuildContext context, PhotosController c) async {
  final field = TextEditingController();
  final path = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Add a folder to the library'),
      content: TextField(
        controller: field,
        autofocus: true,
        decoration: const InputDecoration(hintText: '/home/you/Pictures'),
        onSubmitted: (v) => Navigator.pop(ctx, v),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx), child: const Text('Cancel')),
        FilledButton(
          onPressed: () => Navigator.pop(ctx, field.text),
          child: const Text('Add'),
        ),
      ],
    ),
  );
  field.dispose();
  final trimmed = path?.trim() ?? '';
  if (trimmed.isNotEmpty) await c.send(PhotosCmd.addFolder(path: trimmed));
}

class _SortMenu extends StatelessWidget {
  const _SortMenu({required this.controller});

  final PhotosController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final state = controller.state;
    final mode = state?.sortMode ?? 'date';
    final dir = state?.sortDir ?? 'desc';
    return PopupMenuButton<String>(
      tooltip: 'Sort',
      icon: Icon(Icons.sort, color: t.nInk2),
      onSelected: (v) => controller.send(
        v == 'flip'
            ? PhotosCmd.setSort(mode: mode, dir: dir == 'desc' ? 'asc' : 'desc')
            : PhotosCmd.setSort(mode: v, dir: dir),
      ),
      itemBuilder: (_) => [
        for (final (value, label) in const [
          ('date', 'Date taken'),
          ('name', 'Name'),
          ('size', 'File size'),
        ])
          CheckedPopupMenuItem(
            value: value,
            checked: mode == value,
            child: Text(label),
          ),
        const PopupMenuDivider(),
        PopupMenuItem(
          value: 'flip',
          child: Text(dir == 'desc' ? 'Newest first ✓' : 'Oldest first ✓'),
        ),
      ],
    );
  }
}

// ----------------------------------------------------------------- chips ----

class _CategoryChips extends StatelessWidget {
  const _CategoryChips({required this.controller});

  final PhotosController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final active = controller.category;
    return DecoratedBox(
      // The 2px gradient underline the Slint header draws beneath its chip row.
      decoration: const BoxDecoration(
        border: Border(
          bottom: BorderSide(color: Colors.transparent, width: 2),
        ),
        gradient: LinearGradient(
          begin: Alignment.bottomLeft,
          end: Alignment.bottomRight,
          stops: [0.0, 0.22, 0.78, 1.0],
          colors: [
            Color(0x008B5CF6),
            Color(0xFF8B5CF6),
            Color(0xFFEC4899),
            Color(0x00EC4899),
          ],
        ),
      ),
      child: Padding(
        padding: const EdgeInsets.only(bottom: 2),
        child: ColoredBox(
          color: t.nCanvas,
          child: SizedBox(
            height: 54,
            child: ListView(
              scrollDirection: Axis.horizontal,
              padding: const EdgeInsets.symmetric(horizontal: 24),
              children: [
                for (final c in photoCategories)
                  Padding(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 4, vertical: 9),
                    child: _Chip(
                      category: c,
                      active: c.isActive(active),
                      onTap: () =>
                          controller.send(PhotosCmd.setCategory(name: c.id)),
                      tokens: t,
                    ),
                  ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _Chip extends StatelessWidget {
  const _Chip({
    required this.category,
    required this.active,
    required this.onTap,
    required this.tokens,
  });

  final PhotoCategory category;
  final bool active;
  final VoidCallback onTap;
  final Tokens tokens;

  @override
  Widget build(BuildContext context) {
    const r = 18.0;
    return Material(
      color: active ? Colors.transparent : tokens.nChip,
      borderRadius: BorderRadius.circular(r),
      child: Ink(
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(r),
          gradient: active
              ? LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: [category.tint, category.tint2],
                )
              : null,
        ),
        child: InkWell(
          borderRadius: BorderRadius.circular(r),
          onTap: onTap,
          child: Container(
            height: 36,
            alignment: Alignment.center,
            padding: const EdgeInsets.symmetric(horizontal: 14),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(
                  category.icon,
                  size: 16,
                  color: active ? Colors.white : category.tint,
                ),
                const SizedBox(width: 7),
                Text(
                  category.label,
                  style: TextStyle(
                    fontFamily: Tokens.fontFamily,
                    fontSize: 13,
                    fontWeight: FontWeight.w500,
                    color: active ? Colors.white : tokens.nInk3,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

// ------------------------------------------------------------------ body ----

class _Body extends StatelessWidget {
  const _Body({required this.controller, required this.state});

  final PhotosController controller;
  final PhotosState state;

  @override
  Widget build(BuildContext context) {
    if (controller.error != null) {
      return _Empty(
        icon: Icons.error_outline,
        title: 'Photos could not be read',
        detail: '${controller.error}',
        action: ('Try again', controller.refresh),
      );
    }
    return switch (state.category) {
      'albums' => _AlbumsGrid(controller: controller, state: state),
      'library' => _FolderList(controller: controller, state: state),
      'people' => _PeopleGrid(controller: controller, state: state),
      'things' => _ThingsGrid(controller: controller, state: state),
      'dedupe' => _DedupeList(controller: controller, state: state),
      // Without a basemap the map has pins and nothing to draw them over, so
      // the flat grid of geotagged photos stays the honest answer.
      'places' when state.hasBasemap && state.pins.isNotEmpty =>
        PlacesMap(controller: controller, state: state),
      _ => _TileGrid(controller: controller, state: state),
    };
  }
}

class _TileGrid extends StatelessWidget {
  const _TileGrid({required this.controller, required this.state});

  final PhotosController controller;
  final PhotosState state;

  @override
  Widget build(BuildContext context) {
    if (state.tiles.isEmpty) return _emptyFor(context, controller, state);
    final t = context.tokens;
    // Groups are contiguous runs over `tiles`, so an ungrouped view is just
    // one implicit group over the whole list.
    final List<(String?, List<int>)> groups = state.groups.isEmpty
        ? [(null, List<int>.generate(state.tiles.length, (i) => i))]
        : [for (final g in state.groups) (g.label, g.tiles.toList())];

    return CustomScrollView(
      slivers: [
        for (final (label, indices) in groups) ...[
          if (label != null)
            SliverToBoxAdapter(
              child: Padding(
                padding: const EdgeInsets.fromLTRB(24, 20, 24, 8),
                child: Text(
                  label,
                  style: TextStyle(
                    fontFamily: Tokens.fontFamily,
                    fontSize: 15,
                    fontWeight: FontWeight.w600,
                    color: t.nInk,
                  ),
                ),
              ),
            ),
          SliverPadding(
            padding: const EdgeInsets.symmetric(horizontal: 20),
            sliver: SliverGrid(
              gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(
                maxCrossAxisExtent: _kTileExtent,
                mainAxisSpacing: _kTileGap,
                crossAxisSpacing: _kTileGap,
              ),
              delegate: SliverChildBuilderDelegate(
                childCount: indices.length,
                (context, i) {
                  final tile = state.tiles[indices[i]];
                  return PhotoTileView(
                    key: ValueKey(tile.itemId),
                    tile: tile,
                    controller: controller,
                    selected: controller.selected.contains(tile.itemId),
                    // Once anything is selected a plain tap extends the
                    // selection rather than opening the photo — the rule
                    // ui/page_photos.slint calls "Google-Photos style". The
                    // dot in the corner always toggles, selecting or not.
                    onTap: () {
                      if (controller.selecting) {
                        controller.toggleSelect(tile.itemId);
                      } else {
                        openPhotoViewer(context, controller, tile.itemId);
                      }
                    },
                    onStackMenu: tile.stackSize > 1
                        ? (e) => _stackMenu(context, controller, tile, e)
                        : null,
                    onToggleSelect: () => controller.toggleSelect(tile.itemId),
                  );
                },
              ),
            ),
          ),
        ],
        if (state.moreCount > 0)
          SliverToBoxAdapter(
            child: Center(
              child: Padding(
                padding: const EdgeInsets.all(24),
                child: OutlinedButton(
                  onPressed: () => controller.send(const PhotosCmd.showMore()),
                  child: Text('Show ${state.moreCount} more'),
                ),
              ),
            ),
          ),
        const SliverToBoxAdapter(child: SizedBox(height: 24)),
      ],
    );
  }
}

/// Expand / collapse / break up, on the stack badge. Three explicit choices
/// rather than a toggle, because the grid does not carry a per-stack expanded
/// flag and a toggle that guesses wrong is worse than a menu.
Future<void> _stackMenu(
  BuildContext context,
  PhotosController c,
  PhotoTile tile,
  TapDownDetails at,
) async {
  final picked = await showMenu<String>(
    context: context,
    position: RelativeRect.fromLTRB(
      at.globalPosition.dx,
      at.globalPosition.dy,
      at.globalPosition.dx,
      at.globalPosition.dy,
    ),
    items: [
      PopupMenuItem(
        value: 'expand',
        child: Text('Show all ${tile.stackSize} photos'),
      ),
      const PopupMenuItem(value: 'collapse', child: Text('Collapse')),
      const PopupMenuItem(value: 'unstack', child: Text('Break up stack')),
    ],
  );
  switch (picked) {
    case 'expand':
      await c
          .send(PhotosCmd.toggleStack(stackId: tile.stackId, expanded: true));
    case 'collapse':
      await c
          .send(PhotosCmd.toggleStack(stackId: tile.stackId, expanded: false));
    case 'unstack':
      await c.send(PhotosCmd.unstack(stackId: tile.stackId));
  }
}

class _AlbumsGrid extends StatelessWidget {
  const _AlbumsGrid({required this.controller, required this.state});

  final PhotosController controller;
  final PhotosState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (state.albums.isEmpty) {
      return _Empty(
        icon: Icons.photo_album_outlined,
        title: 'No albums yet',
        detail:
            'Albums group photos by hand. Smart albums fill themselves from a rule.',
        action: ('New album', () => _promptNewAlbum(context, controller)),
      );
    }
    return GridView.builder(
      padding: const EdgeInsets.all(20),
      gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(
        maxCrossAxisExtent: 220,
        mainAxisSpacing: 16,
        crossAxisSpacing: 16,
        childAspectRatio: 0.82,
      ),
      itemCount: state.albums.length,
      itemBuilder: (context, i) {
        final a = state.albums[i];
        return InkWell(
          onTap: () => controller.send(PhotosCmd.openAlbum(albumId: a.id)),
          // Rename and delete live on the right-click, not on a hover button:
          // an album card is mostly cover art, and a floating menu button over
          // it is the first thing that makes a grid look like a file manager.
          onSecondaryTapDown: (e) => _albumMenu(context, controller, a, e),
          borderRadius: BorderRadius.circular(Tokens.radiusMd),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: Container(
                  width: double.infinity,
                  decoration: BoxDecoration(
                    color: t.nTile,
                    borderRadius: BorderRadius.circular(Tokens.radiusMd),
                  ),
                  child: _CoverThumb(
                    path: a.cover,
                    fallback: Icon(
                      a.smart ? Icons.auto_awesome : Icons.photo_album_outlined,
                      color: t.nInk2,
                    ),
                  ),
                ),
              ),
              const SizedBox(height: 8),
              Text(
                a.name,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 14, fontWeight: FontWeight.w500, color: t.nInk),
              ),
              Text('${a.count} items',
                  style: TextStyle(fontSize: 12, color: t.nInk2)),
            ],
          ),
        );
      },
    );
  }
}

Future<void> _albumMenu(
  BuildContext context,
  PhotosController c,
  AlbumCard album,
  TapDownDetails at,
) async {
  final picked = await showMenu<String>(
    context: context,
    position: RelativeRect.fromLTRB(
      at.globalPosition.dx,
      at.globalPosition.dy,
      at.globalPosition.dx,
      at.globalPosition.dy,
    ),
    items: const [
      PopupMenuItem(value: 'rename', child: Text('Rename')),
      PopupMenuItem(value: 'delete', child: Text('Delete album')),
    ],
  );
  if (!context.mounted || picked == null) return;
  if (picked == 'rename') {
    final name = await _promptText(
      context,
      title: 'Rename album',
      initial: album.name,
      confirm: 'Rename',
    );
    if (name != null && name.isNotEmpty) {
      await c.send(PhotosCmd.albumRename(albumId: album.id, name: name));
    }
    return;
  }
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text('Delete "${album.name}"?'),
      // Worth saying plainly: nothing about this touches the photos, and the
      // dialog is the only place a user finds that out before clicking.
      content: const Text(
        'The album is removed. The photos in it stay in your library.',
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Cancel')),
        FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('Delete')),
      ],
    ),
  );
  if (ok ?? false) await c.send(PhotosCmd.albumDelete(albumId: album.id));
}

/// One text field in a dialog. Returns null when cancelled, so an empty
/// string can still mean "the user cleared it".
Future<String?> _promptText(
  BuildContext context, {
  required String title,
  required String initial,
  required String confirm,
}) async {
  final field = TextEditingController(text: initial);
  final out = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(title),
      content: TextField(
        controller: field,
        autofocus: true,
        onSubmitted: (v) => Navigator.pop(ctx, v),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx), child: const Text('Cancel')),
        FilledButton(
          onPressed: () => Navigator.pop(ctx, field.text),
          child: Text(confirm),
        ),
      ],
    ),
  );
  field.dispose();
  return out?.trim();
}

Future<void> _promptNewAlbum(BuildContext context, PhotosController c) async {
  final field = TextEditingController();
  final name = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('New album'),
      content: TextField(
        controller: field,
        autofocus: true,
        onSubmitted: (v) => Navigator.pop(ctx, v),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx), child: const Text('Cancel')),
        FilledButton(
          onPressed: () => Navigator.pop(ctx, field.text),
          child: const Text('Create'),
        ),
      ],
    ),
  );
  field.dispose();
  final trimmed = name?.trim() ?? '';
  if (trimmed.isNotEmpty) await c.send(PhotosCmd.albumNew(name: trimmed));
}

class _FolderList extends StatelessWidget {
  const _FolderList({required this.controller, required this.state});

  final PhotosController controller;
  final PhotosState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (state.folders.isEmpty) {
      return _Empty(
        icon: Icons.folder_outlined,
        title: 'No watched folders',
        detail:
            'Add a folder and Tulipix indexes the photos in it. Files are never copied.',
        action: ('Add folder', () => _promptAddFolder(context, controller)),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _LibSortBar(controller: controller, state: state),
        Expanded(
          child: ListView.separated(
            padding: const EdgeInsets.fromLTRB(20, 0, 20, 20),
            itemCount: state.folders.length,
            separatorBuilder: (_, __) => Divider(height: 1, color: t.nHair),
            itemBuilder: (context, i) {
              final f = state.folders[i];
              return ListTile(
                leading: const Icon(Icons.folder, color: Tokens.secPhotos),
                title: Text(f.name, style: TextStyle(color: t.nInk)),
                subtitle: Text(f.path,
                    style: TextStyle(fontSize: 12, color: t.nInk2)),
                trailing: Text('${f.count}', style: TextStyle(color: t.nInk2)),
              );
            },
          ),
        ),
      ],
    );
  }
}

/// The Library tab sorts folders, and the grid's own sort menu sorts photos.
/// Two controls because they are two lists; sharing one would mean "sort by
/// date taken" silently meaning something else on this tab.
class _LibSortBar extends StatelessWidget {
  const _LibSortBar({required this.controller, required this.state});

  final PhotosController controller;
  final PhotosState state;

  static const _modes = <(String, String)>[
    ('name', 'Name'),
    ('count', 'Photos'),
    ('path', 'Path'),
  ];

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 16, 20, 10),
      child: Row(
        children: [
          Text(
            '${state.folderCount} folder${state.folderCount == 1 ? '' : 's'}',
            style: TextStyle(fontSize: 12, color: t.nInk2),
          ),
          const Spacer(),
          for (final (id, label) in _modes)
            Padding(
              padding: const EdgeInsets.only(left: 6),
              child: TextButton.icon(
                onPressed: () => controller.send(
                  PhotosCmd.setLibSort(
                    mode: id,
                    // Tapping the active column flips it, which is the one
                    // interaction every file list in the world already has.
                    dir: state.libSort == id && state.libSortDir == 'asc'
                        ? 'desc'
                        : 'asc',
                  ),
                ),
                icon: Icon(
                  state.libSort != id
                      ? Icons.unfold_more
                      : state.libSortDir == 'asc'
                          ? Icons.arrow_upward
                          : Icons.arrow_downward,
                  size: 14,
                ),
                label: Text(label),
                style: TextButton.styleFrom(
                  foregroundColor: state.libSort == id ? t.nInk : t.nInk2,
                  visualDensity: VisualDensity.compact,
                ),
              ),
            ),
        ],
      ),
    );
  }
}

// --------------------------------------------------------- empty + action ----

/// (tab to return to, what is being shown) while drilled into one album,
/// person or tag. Null on a top-level tab.
///
/// The chips alone would get you out -- picking a tab clears what it was
/// drilled into -- but nothing on screen would say whose photos these are.
(String, String)? _crumbFor(PhotosState s) => switch (s.category) {
      'album' => (
          'albums',
          s.albums
                  .where((a) => a.id == s.albumId)
                  .map((a) => a.name)
                  .firstOrNull ??
              'Album',
        ),
      'facephotos' => (
          'people',
          s.people
                  .where((p) => p.id == s.personId)
                  .map((p) => p.name.isEmpty ? 'Unnamed' : p.name)
                  .firstOrNull ??
              'Person',
        ),
      'tagphotos' => ('things', s.tagName.isEmpty ? 'Tag' : s.tagName),
      _ => null,
    };

Widget _emptyFor(BuildContext context, PhotosController c, PhotosState s) {
  if (s.query.isNotEmpty) {
    return _Empty(
      icon: Icons.search_off,
      title: 'Nothing matches "${s.query}"',
      detail: 'Search covers filenames, camera, tags and named people.',
      action: ('Clear search', () => c.send(const PhotosCmd.search(query: ''))),
    );
  }
  return switch (s.category) {
    'starred' => const _Empty(
        icon: Icons.star_border,
        title: 'No starred photos',
        detail: 'Star a photo to pin it here.',
      ),
    'archive' => const _Empty(
        icon: Icons.archive_outlined,
        title: 'Archive is empty',
        detail: 'Archived photos stay in the library but leave the timeline.',
      ),
    'trash' => const _Empty(
        icon: Icons.delete_outline,
        title: 'Trash is empty',
        detail:
            'Trashed photos are removed from disk only after their purge date.',
      ),
    'dedupe' => const _Empty(
        icon: Icons.done_all,
        title: 'No duplicates found',
        detail:
            'Duplicates are grouped during a scan, by checksum and by perceptual hash.',
      ),
    'places' => const _Empty(
        icon: Icons.place_outlined,
        title: 'No geotagged photos',
        detail: 'Photos with EXIF GPS appear here, grouped by place.',
      ),
    'memories' => const _Empty(
        icon: Icons.movie_outlined,
        title: 'Nothing from this day',
        detail:
            'Memories shows photos taken on today\'s month and day in earlier years.',
      ),
    'facephotos' => const _Empty(
        icon: Icons.person_search_outlined,
        title: 'No photos of this person',
        detail:
            'Every photo in this cluster has since been trashed or gone missing.',
      ),
    'tagphotos' => _Empty(
        icon: Icons.sell_outlined,
        title: 'Nothing tagged "${s.tagName}"',
        detail: 'Every photo with this tag has been trashed or archived.',
      ),
    _ => _Empty(
        icon: Icons.photo_library_outlined,
        title: 'No photos yet',
        detail:
            'Add a folder to build the library. Tulipix indexes in place — nothing is copied.',
        action: ('Add folder', () => _promptAddFolder(context, c)),
      ),
  };
}

class _Empty extends StatelessWidget {
  const _Empty({
    required this.icon,
    required this.title,
    required this.detail,
    this.action,
  });

  final IconData icon;
  final String title;
  final String detail;
  final (String, VoidCallback)? action;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 380),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 44, color: t.nInk2),
            const SizedBox(height: 14),
            Text(
              title,
              textAlign: TextAlign.center,
              style: TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: 17,
                fontWeight: FontWeight.w600,
                color: t.nInk,
              ),
            ),
            const SizedBox(height: 6),
            Text(
              detail,
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 13, height: 1.5, color: t.nInk2),
            ),
            if (action case (final label, final onTap)) ...[
              const SizedBox(height: 18),
              FilledButton(onPressed: onTap, child: Text(label)),
            ],
          ],
        ),
      ),
    );
  }
}

class _SelectionBar extends StatelessWidget {
  const _SelectionBar({required this.controller});

  final PhotosController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final inTrash = controller.category == 'trash';
    return Material(
      color: t.nCard,
      child: Container(
        height: 56,
        padding: const EdgeInsets.symmetric(horizontal: 16),
        decoration:
            BoxDecoration(border: Border(top: BorderSide(color: t.nHair))),
        child: Row(
          children: [
            IconButton(
              icon: Icon(Icons.close, color: t.nInk2),
              onPressed: controller.clearSelection,
            ),
            Text(
              '${controller.selected.length} selected',
              style: TextStyle(fontSize: 14, color: t.nInk),
            ),
            const Spacer(),
            TextButton(
                onPressed: controller.selectAll,
                child: const Text('Select all')),
            if (inTrash)
              TextButton.icon(
                onPressed: controller.restoreSelection,
                icon: const Icon(Icons.restore_from_trash_outlined, size: 18),
                label: const Text('Restore'),
              )
            else ...[
              TextButton.icon(
                onPressed: () => controller.starSelection(true),
                icon: const Icon(Icons.star_border, size: 18),
                label: const Text('Star'),
              ),
              TextButton.icon(
                onPressed: () => controller.archiveSelection(true),
                icon: const Icon(Icons.archive_outlined, size: 18),
                label: const Text('Archive'),
              ),
              TextButton.icon(
                onPressed: controller.trashSelection,
                icon: const Icon(Icons.delete_outline, size: 18),
                label: const Text('Trash'),
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// A card cover. Every card grid on this page -- albums, people, things --
/// stores an absolute image path, and until now none of them drew it: the
/// album grid painted an empty rounded rectangle. The grid thumb is resolved
/// through the same cache the tiles use, so a cover costs no extra decode.
class _CoverThumb extends StatelessWidget {
  const _CoverThumb({required this.path, this.fallback, this.circle = false});

  /// Already the cached grid thumb when one exists -- `covers()` in the bridge
  /// resolves that, so this never decodes an original to fill a card.
  final String path;
  final Widget? fallback;
  final bool circle;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final shape = circle
        ? BorderRadius.circular(999)
        : BorderRadius.circular(Tokens.radiusMd);
    final empty = DecoratedBox(
      decoration: BoxDecoration(color: t.nTile, borderRadius: shape),
      child: Center(child: fallback ?? const SizedBox.shrink()),
    );
    if (path.isEmpty) return empty;
    return ClipRRect(
      borderRadius: shape,
      child: Image.file(
        File(path),
        fit: BoxFit.cover,
        width: double.infinity,
        height: double.infinity,
        filterQuality: FilterQuality.low,
        errorBuilder: (_, __, ___) => empty,
      ),
    );
  }
}

/// Face clusters. Unnamed clusters are shown rather than hidden -- naming them
/// is what the tab is for, so hiding the unnamed ones would hide the work.
class _PeopleGrid extends StatelessWidget {
  const _PeopleGrid({required this.controller, required this.state});

  final PhotosController controller;
  final PhotosState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (state.people.isEmpty) {
      return const _Empty(
        icon: Icons.person_search_outlined,
        title: 'No people yet',
        detail:
            'Faces are grouped by the background indexer. It runs when the machine is idle; '
            'clusters appear here as it finds them.',
      );
    }
    return GridView.builder(
      padding: const EdgeInsets.all(20),
      gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(
        maxCrossAxisExtent: 150,
        mainAxisSpacing: 18,
        crossAxisSpacing: 18,
        childAspectRatio: 0.78,
      ),
      itemCount: state.people.length,
      itemBuilder: (context, i) {
        final p = state.people[i];
        final named = p.name.isNotEmpty;
        return InkWell(
          onTap: () => controller.send(PhotosCmd.openPerson(personId: p.id)),
          borderRadius: BorderRadius.circular(Tokens.radiusMd),
          child: Column(
            children: [
              Expanded(
                child: AspectRatio(
                  aspectRatio: 1,
                  child: _CoverThumb(
                    path: p.cover,
                    circle: true,
                    fallback: Icon(Icons.person, color: t.nInk2, size: 30),
                  ),
                ),
              ),
              const SizedBox(height: 8),
              GestureDetector(
                onTap: () => _promptRenamePerson(context, controller, p),
                child: Text(
                  named ? p.name : 'Add a name',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontFamily: Tokens.fontFamily,
                    fontSize: 13,
                    fontWeight: FontWeight.w500,
                    color: named ? t.nInk : Tokens.secPhotos,
                  ),
                ),
              ),
              Text(
                '${p.count} ${p.count == 1 ? 'face' : 'faces'}',
                style: TextStyle(
                    fontFamily: Tokens.fontFamily,
                    fontSize: 11,
                    color: t.nInk2),
              ),
            ],
          ),
        );
      },
    );
  }
}

Future<void> _promptRenamePerson(
  BuildContext context,
  PhotosController controller,
  PersonCard person,
) async {
  final field = TextEditingController(text: person.name);
  final name = await showDialog<String>(
    context: context,
    builder: (context) => AlertDialog(
      title: const Text('Name this person'),
      content: TextField(
        controller: field,
        autofocus: true,
        decoration: const InputDecoration(hintText: 'Name'),
        onSubmitted: (v) => Navigator.of(context).pop(v),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.of(context).pop(field.text),
          child: const Text('Save'),
        ),
      ],
    ),
  );
  field.dispose();
  if (name == null) return;
  await controller
      .send(PhotosCmd.renamePerson(personId: person.id, name: name.trim()));
}

/// Object tags, most-photographed first -- the order `ai::tags::things` returns.
class _ThingsGrid extends StatelessWidget {
  const _ThingsGrid({required this.controller, required this.state});

  final PhotosController controller;
  final PhotosState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (state.things.isEmpty) {
      return const _Empty(
        icon: Icons.sell_outlined,
        title: 'Nothing tagged yet',
        detail:
            'Objects are detected by the background indexer. Tags appear here as it works through the library.',
      );
    }
    return GridView.builder(
      padding: const EdgeInsets.all(20),
      gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(
        maxCrossAxisExtent: 200,
        mainAxisSpacing: 16,
        crossAxisSpacing: 16,
        childAspectRatio: 1.15,
      ),
      itemCount: state.things.length,
      itemBuilder: (context, i) {
        final tag = state.things[i];
        return InkWell(
          onTap: () =>
              controller.send(PhotosCmd.openTag(tagId: tag.id, name: tag.name)),
          borderRadius: BorderRadius.circular(Tokens.radiusMd),
          child: Stack(
            fit: StackFit.expand,
            children: [
              _CoverThumb(
                path: tag.cover,
                fallback: Icon(Icons.sell_outlined, color: t.nInk2),
              ),
              // A scrim, because the label sits on an arbitrary photo and has
              // to stay legible against a white sky as well as a night shot.
              DecoratedBox(
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(Tokens.radiusMd),
                  gradient: const LinearGradient(
                    begin: Alignment.center,
                    end: Alignment.bottomCenter,
                    colors: [Color(0x00000000), Color(0xC0000000)],
                  ),
                ),
              ),
              Align(
                alignment: Alignment.bottomLeft,
                child: Padding(
                  padding: const EdgeInsets.all(12),
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        tag.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          fontFamily: Tokens.fontFamily,
                          fontSize: 14,
                          fontWeight: FontWeight.w600,
                          color: Colors.white,
                        ),
                      ),
                      Text(
                        '${tag.count}',
                        style: const TextStyle(
                          fontFamily: Tokens.fontFamily,
                          fontSize: 11,
                          color: Colors.white70,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

/// Duplicate clusters, two at a time. The decision is always the same one --
/// which of these two to keep -- so the row is two photos and two buttons, and
/// resolving one removes it from the list.
class _DedupeList extends StatelessWidget {
  const _DedupeList({required this.controller, required this.state});

  final PhotosController controller;
  final PhotosState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (state.dedupe.isEmpty) return _emptyFor(context, controller, state);
    return ListView.builder(
      padding: const EdgeInsets.all(20),
      itemCount: state.dedupe.length,
      itemBuilder: (context, i) {
        final pair = state.dedupe[i];
        final identical = pair.kind == 'sha256';
        return Container(
          margin: const EdgeInsets.only(bottom: 16),
          decoration: BoxDecoration(
            color: t.nCard,
            borderRadius: BorderRadius.circular(Tokens.radiusMd),
            border: Border.all(color: t.nHair),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(14, 12, 14, 0),
                child: Row(
                  children: [
                    Container(
                      padding: const EdgeInsets.symmetric(
                          horizontal: 8, vertical: 3),
                      decoration: BoxDecoration(
                        color: identical
                            ? const Color(0x2234A853)
                            : const Color(0x22EA8600),
                        borderRadius: BorderRadius.circular(5),
                      ),
                      child: Text(
                        identical ? 'SHA-256' : 'pHash',
                        style: TextStyle(
                          fontFamily: Tokens.fontFamily,
                          fontSize: 10.5,
                          fontWeight: FontWeight.w700,
                          color: identical
                              ? const Color(0xFF34A853)
                              : const Color(0xFFEA8600),
                        ),
                      ),
                    ),
                    const SizedBox(width: 10),
                    Text(
                      identical
                          ? 'Byte-for-byte identical'
                          : 'Looks the same, different bytes',
                      style: TextStyle(fontSize: 12, color: t.nInk2),
                    ),
                  ],
                ),
              ),
              Padding(
                padding: const EdgeInsets.all(14),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Expanded(
                      child: _DedupeSide(
                        controller: controller,
                        tile: pair.left,
                        other: pair.right,
                      ),
                    ),
                    const SizedBox(width: 14),
                    Expanded(
                      child: _DedupeSide(
                        controller: controller,
                        tile: pair.right,
                        other: pair.left,
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

class _DedupeSide extends StatelessWidget {
  const _DedupeSide({
    required this.controller,
    required this.tile,
    required this.other,
  });

  final PhotosController controller;
  final PhotoTile tile;
  final PhotoTile other;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        AspectRatio(
          aspectRatio: 1,
          child: GestureDetector(
            onTap: () => openPhotoViewer(context, controller, tile.itemId),
            child: _CoverThumb(
              path: tile.thumb.isNotEmpty ? tile.thumb : tile.path,
              fallback: Icon(Icons.image_outlined, color: t.nInk2),
            ),
          ),
        ),
        const SizedBox(height: 8),
        Text(
          tile.label,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(fontSize: 12, color: t.nInk),
        ),
        Text(
          '${tile.width} × ${tile.height}',
          style: TextStyle(fontSize: 11, color: t.nInk2),
        ),
        const SizedBox(height: 8),
        // Trash, not delete. Getting this call wrong on a near-identical pair
        // has to be recoverable from the Trash tab.
        OutlinedButton(
          onPressed: () => controller.send(
            PhotosCmd.dedupeResolve(
              keepItemId: tile.itemId,
              trashItemId: other.itemId,
            ),
          ),
          child: const Text('Keep this one'),
        ),
      ],
    );
  }
}
