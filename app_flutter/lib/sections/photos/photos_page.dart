// Photos — Google Photos language: light & airy, tight justified grid.
// Tabs: Timeline · Starred · Albums · Places · Library · Archive · Trash.
// Ports ui/page_photos.slint.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/photos.dart';
import 'photo_tile.dart';
import 'photos_controller.dart';

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
    return ColoredBox(
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
                    ? const Center(child: CircularProgressIndicator())
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
    return SizedBox(
      height: 52,
      child: ListView(
        scrollDirection: Axis.horizontal,
        padding: const EdgeInsets.symmetric(horizontal: 20),
        children: [
          for (final (id, label) in photoCategories)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 4, vertical: 8),
              child: _Chip(
                label: label,
                active: id == active || (id == 'albums' && active == 'album'),
                onTap: () => controller.send(PhotosCmd.setCategory(name: id)),
                tokens: t,
              ),
            ),
        ],
      ),
    );
  }
}

class _Chip extends StatelessWidget {
  const _Chip({
    required this.label,
    required this.active,
    required this.onTap,
    required this.tokens,
  });

  final String label;
  final bool active;
  final VoidCallback onTap;
  final Tokens tokens;

  @override
  Widget build(BuildContext context) => Material(
        color: active ? const Color(0xFF1A73E8) : tokens.nChip,
        borderRadius: BorderRadius.circular(17),
        child: InkWell(
          borderRadius: BorderRadius.circular(17),
          onTap: onTap,
          child: Container(
            height: 36,
            constraints: const BoxConstraints(minWidth: 84),
            alignment: Alignment.center,
            padding: const EdgeInsets.symmetric(horizontal: 16),
            child: Text(
              label,
              style: TextStyle(
                fontSize: 13,
                fontWeight: FontWeight.w500,
                color: active ? Colors.white : tokens.nInk3,
              ),
            ),
          ),
        ),
      );
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
                    // Tap selects. Opening the full viewer is the next slice —
                    // ui/photo_editor.slint and the viewer's exif/histogram
                    // panel are 699 + ~500 lines that have not been ported yet.
                    onTap: () => controller.toggleSelect(tile.itemId),
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
                  child:
                      a.smart ? Icon(Icons.auto_awesome, color: t.nInk2) : null,
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
    return ListView.separated(
      padding: const EdgeInsets.all(20),
      itemCount: state.folders.length,
      separatorBuilder: (_, __) => Divider(height: 1, color: t.nHair),
      itemBuilder: (context, i) {
        final f = state.folders[i];
        return ListTile(
          leading: const Icon(Icons.folder, color: Tokens.secPhotos),
          title: Text(f.name, style: TextStyle(color: t.nInk)),
          subtitle:
              Text(f.path, style: TextStyle(fontSize: 12, color: t.nInk2)),
          trailing: Text('${f.count}', style: TextStyle(color: t.nInk2)),
        );
      },
    );
  }
}

// --------------------------------------------------------- empty + action ----

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
    'places' => const _Empty(
        icon: Icons.place_outlined,
        title: 'No geotagged photos',
        detail: 'Photos with EXIF GPS appear here, grouped by place.',
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
