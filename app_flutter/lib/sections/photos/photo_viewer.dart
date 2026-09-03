// The full-size viewer. Opened by tapping a tile when nothing is selected;
// once a selection exists, a tap toggles it instead and the viewer stays shut
// -- the same rule ui/page_photos.slint states as "Google-Photos style".
//
// The cursor lives here, not in Rust. Dart already holds the ordered tile list
// from the last snapshot, so next/prev is an index move; Rust is only asked to
// describe the photo that lands under it.

import 'dart:async';
import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/photos.dart';
import 'photo_editor.dart';
import 'photos_controller.dart';

Future<void> openPhotoViewer(
  BuildContext context,
  PhotosController controller,
  int itemId,
) {
  return Navigator.of(context).push(
    PageRouteBuilder<void>(
      opaque: true,
      barrierColor: Colors.black,
      transitionDuration: const Duration(milliseconds: 140),
      pageBuilder: (_, __, ___) =>
          _Viewer(controller: controller, startId: itemId),
      transitionsBuilder: (_, anim, __, child) =>
          FadeTransition(opacity: anim, child: child),
    ),
  );
}

class _Viewer extends StatefulWidget {
  const _Viewer({required this.controller, required this.startId});

  final PhotosController controller;
  final int startId;

  @override
  State<_Viewer> createState() => _ViewerState();
}

class _ViewerState extends State<_Viewer> {
  late int _id = widget.startId;
  PhotoDetail? _detail;
  bool _info = false;
  Timer? _slideshow;
  int _dwellSeconds = 4;
  bool _shuffle = false;
  final _random = math.Random();
  final _focus = FocusNode();
  final _zoom = TransformationController();

  @override
  void initState() {
    super.initState();
    _load();
    widget.controller.addListener(_onGridChanged);
  }

  @override
  void dispose() {
    _slideshow?.cancel();
    widget.controller.removeListener(_onGridChanged);
    _focus.dispose();
    _zoom.dispose();
    super.dispose();
  }

  List<PhotoTile> get _tiles => widget.controller.state?.tiles ?? const [];
  int get _pos => _tiles.indexWhere((t) => t.itemId == _id);

  /// A star or a trash re-queries the grid underneath us. If the photo being
  /// viewed survived, stay on it; if it did not, fall to whatever now occupies
  /// its place, and close when the view empties.
  void _onGridChanged() {
    if (!mounted) return;
    if (_tiles.isEmpty) {
      Navigator.of(context).maybePop();
      return;
    }
    if (_pos >= 0) {
      setState(() {});
      return;
    }
    final at = _lastPos.clamp(0, _tiles.length - 1);
    _go(_tiles[at].itemId);
  }

  int _lastPos = 0;

  Future<void> _load() async {
    final want = _id;
    final d = await photosItemDetail(itemId: want);
    if (!mounted || want != _id) return;
    setState(() => _detail = d);
  }

  void _go(int itemId) {
    if (itemId == _id) return;
    _lastPos = _pos >= 0 ? _pos : _lastPos;
    setState(() {
      _id = itemId;
      _detail = null;
      // Slint resets zoom and pan on every photo change; matching that keeps a
      // 4x crop from carrying over onto the next, differently sized, photo.
      _zoom.value = Matrix4.identity();
    });
    _load();
  }

  void _step(int delta) {
    final i = _pos;
    if (i < 0) return;
    final next = i + delta;
    if (next < 0 || next >= _tiles.length) return;
    _go(_tiles[next].itemId);
  }

  /// Advance, wrapping at the end rather than stopping: a slideshow that halts
  /// on the last photo has to be restarted by hand every time.
  void _advance() {
    if (_tiles.isEmpty) return;
    if (_shuffle) {
      _go(_tiles[_random.nextInt(_tiles.length)].itemId);
      return;
    }
    final i = _pos;
    _go(_tiles[(i < 0 ? 0 : (i + 1) % _tiles.length)].itemId);
  }

  void _toggleSlideshow() {
    setState(() {
      if (_slideshow != null) {
        _slideshow?.cancel();
        _slideshow = null;
      } else {
        _slideshow = Timer.periodic(
          Duration(seconds: _dwellSeconds),
          (_) => _advance(),
        );
      }
    });
  }

  void _setDwell(int seconds) {
    _dwellSeconds = seconds;
    if (_slideshow != null) {
      _slideshow?.cancel();
      _slideshow =
          Timer.periodic(Duration(seconds: _dwellSeconds), (_) => _advance());
    }
    setState(() {});
  }

  KeyEventResult _onKey(FocusNode _, KeyEvent e) {
    if (e is! KeyDownEvent && e is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    // if/else rather than a switch: LogicalKeyboardKey overrides `==`, which
    // makes its constants awkward as constant patterns.
    final k = e.logicalKey;
    if (k == LogicalKeyboardKey.arrowRight || k == LogicalKeyboardKey.space) {
      _step(1);
    } else if (k == LogicalKeyboardKey.arrowLeft) {
      _step(-1);
    } else if (k == LogicalKeyboardKey.escape) {
      Navigator.of(context).maybePop();
    } else if (k == LogicalKeyboardKey.keyI) {
      setState(() => _info = !_info);
    } else if (k == LogicalKeyboardKey.keyS) {
      final t = _current;
      if (t != null) widget.controller.setStar(t.itemId, !t.starred);
    } else if (k == LogicalKeyboardKey.keyP) {
      _toggleSlideshow();
    } else if (k == LogicalKeyboardKey.keyE) {
      final t = _current;
      if (t != null) openPhotoEditor(context, widget.controller, t.itemId);
    } else {
      return KeyEventResult.ignored;
    }
    return KeyEventResult.handled;
  }

  PhotoTile? get _current {
    final i = _pos;
    return i >= 0 ? _tiles[i] : _detail?.tile;
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tile = _current;
    final i = _pos;

    return Focus(
      focusNode: _focus,
      autofocus: true,
      onKeyEvent: _onKey,
      child: Scaffold(
        backgroundColor: Colors.black,
        body: Row(
          children: [
            Expanded(
              child: Stack(
                fit: StackFit.expand,
                children: [
                  if (tile != null) _Frame(tile: tile, zoom: _zoom),
                  _TopBar(
                    label: tile?.label ?? '',
                    position: i >= 0 ? '${i + 1} / ${_tiles.length}' : '',
                    starred: tile?.starred ?? false,
                    archived: tile?.archived ?? false,
                    info: _info,
                    onClose: () => Navigator.of(context).maybePop(),
                    onInfo: () => setState(() => _info = !_info),
                    playing: _slideshow != null,
                    dwellSeconds: _dwellSeconds,
                    onPlay: _toggleSlideshow,
                    onDwell: _setDwell,
                    shuffle: _shuffle,
                    onShuffle: () => setState(() => _shuffle = !_shuffle),
                    onEdit: tile == null
                        ? null
                        : () => openPhotoEditor(
                            context, widget.controller, tile.itemId),
                    onStar: tile == null
                        ? null
                        : () => widget.controller
                            .setStar(tile.itemId, !tile.starred),
                    onArchive: tile == null
                        ? null
                        : () => widget.controller
                            .setArchived(tile.itemId, !tile.archived),
                    onTrash: tile == null
                        ? null
                        : () => widget.controller.trashOne(tile.itemId),
                  ),
                  if (i > 0)
                    _Arrow(
                      icon: Icons.chevron_left,
                      alignment: Alignment.centerLeft,
                      onTap: () => _step(-1),
                    ),
                  if (i >= 0 && i < _tiles.length - 1)
                    _Arrow(
                      icon: Icons.chevron_right,
                      alignment: Alignment.centerRight,
                      onTap: () => _step(1),
                    ),
                ],
              ),
            ),
            if (_info) _InfoPanel(detail: _detail, tokens: t),
          ],
        ),
      ),
    );
  }
}

/// The original file, not the 256px grid thumb -- a viewer showing the thumb
/// scaled up is the one thing a viewer must not do. The thumb is the
/// placeholder underneath while the original decodes.
class _Frame extends StatelessWidget {
  const _Frame({required this.tile, required this.zoom});

  final PhotoTile tile;
  final TransformationController zoom;

  @override
  Widget build(BuildContext context) {
    return InteractiveViewer(
      transformationController: zoom,
      minScale: 1,
      maxScale: 8,
      child: Center(
        child: Image.file(
          File(tile.path),
          fit: BoxFit.contain,
          filterQuality: FilterQuality.medium,
          gaplessPlayback: true,
          frameBuilder: (context, child, frame, wasSync) {
            if (frame != null || wasSync) return child;
            return tile.thumb.isEmpty
                ? const SizedBox.shrink()
                : Image.file(File(tile.thumb), fit: BoxFit.contain);
          },
          errorBuilder: (_, __, ___) => const _Unreadable(),
        ),
      ),
    );
  }
}

class _Unreadable extends StatelessWidget {
  const _Unreadable();

  @override
  Widget build(BuildContext context) => const Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.broken_image_outlined, size: 44, color: Colors.white38),
            SizedBox(height: 10),
            Text(
              'This file could not be opened.',
              style: TextStyle(color: Colors.white54, fontSize: 13),
            ),
          ],
        ),
      );
}

class _TopBar extends StatelessWidget {
  const _TopBar({
    required this.label,
    required this.position,
    required this.starred,
    required this.archived,
    required this.info,
    required this.onClose,
    required this.onInfo,
    required this.onEdit,
    required this.playing,
    required this.dwellSeconds,
    required this.onPlay,
    required this.onDwell,
    required this.shuffle,
    required this.onShuffle,
    required this.onStar,
    required this.onArchive,
    required this.onTrash,
  });

  final String label;
  final String position;
  final bool starred;
  final bool archived;
  final bool info;
  final VoidCallback onClose;
  final VoidCallback onInfo;
  final VoidCallback? onEdit;
  final bool playing;
  final int dwellSeconds;
  final VoidCallback onPlay;
  final void Function(int) onDwell;
  final bool shuffle;
  final VoidCallback onShuffle;
  final VoidCallback? onStar;
  final VoidCallback? onArchive;
  final VoidCallback? onTrash;

  @override
  Widget build(BuildContext context) {
    return Align(
      alignment: Alignment.topCenter,
      child: DecoratedBox(
        decoration: const BoxDecoration(
          gradient: LinearGradient(
            begin: Alignment.topCenter,
            end: Alignment.bottomCenter,
            colors: [Color(0xCC000000), Color(0x00000000)],
          ),
        ),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(8, 8, 8, 26),
          child: Row(
            children: [
              _Action(
                  icon: Icons.arrow_back, tip: 'Close  (Esc)', onTap: onClose),
              const SizedBox(width: 6),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      label,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                        fontFamily: Tokens.fontFamily,
                        color: Colors.white,
                        fontSize: 14,
                        fontWeight: FontWeight.w600,
                      ),
                    ),
                    if (position.isNotEmpty)
                      Text(
                        position,
                        style: const TextStyle(
                          fontFamily: Tokens.fontFamily,
                          color: Colors.white60,
                          fontSize: 11,
                        ),
                      ),
                  ],
                ),
              ),
              _Action(
                icon: playing ? Icons.pause : Icons.play_arrow,
                tip: 'Slideshow  (P)',
                active: playing,
                onTap: onPlay,
              ),
              PopupMenuButton<int>(
                tooltip: 'Seconds per photo',
                initialValue: dwellSeconds,
                onSelected: onDwell,
                itemBuilder: (context) => [
                  for (final n in const [2, 4, 6, 10, 20])
                    PopupMenuItem(value: n, child: Text('${n}s')),
                ],
                icon: const Icon(Icons.timer_outlined, size: 20),
                color: null,
              ),
              _Action(
                icon: Icons.shuffle,
                tip: 'Shuffle',
                active: shuffle,
                onTap: onShuffle,
              ),
              _Action(icon: Icons.tune, tip: 'Edit  (E)', onTap: onEdit),
              _Action(
                icon: starred ? Icons.star : Icons.star_border,
                tip: 'Star  (S)',
                active: starred,
                onTap: onStar,
              ),
              _Action(
                icon: archived
                    ? Icons.unarchive_outlined
                    : Icons.archive_outlined,
                tip: archived ? 'Unarchive' : 'Archive',
                onTap: onArchive,
              ),
              _Action(icon: Icons.delete_outline, tip: 'Trash', onTap: onTrash),
              _Action(
                icon: Icons.info_outline,
                tip: 'Info  (I)',
                active: info,
                onTap: onInfo,
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _Action extends StatelessWidget {
  const _Action({
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
  Widget build(BuildContext context) => IconButton(
        onPressed: onTap,
        icon: Icon(icon, size: 20),
        tooltip: tip,
        color: active ? Tokens.secPhotos : Colors.white,
        disabledColor: Colors.white24,
      );
}

class _Arrow extends StatelessWidget {
  const _Arrow({
    required this.icon,
    required this.alignment,
    required this.onTap,
  });

  final IconData icon;
  final Alignment alignment;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => Align(
        alignment: alignment,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 8),
          child: Material(
            color: const Color(0x66000000),
            shape: const CircleBorder(),
            child: InkWell(
              customBorder: const CircleBorder(),
              onTap: onTap,
              child: Padding(
                padding: const EdgeInsets.all(8),
                child: Icon(icon, color: Colors.white, size: 26),
              ),
            ),
          ),
        ),
      );
}

class _InfoPanel extends StatelessWidget {
  const _InfoPanel({required this.detail, required this.tokens});

  final PhotoDetail? detail;
  final Tokens tokens;

  @override
  Widget build(BuildContext context) {
    final rows = detail?.exif ?? const <ExifRow>[];
    return Container(
      width: 320,
      color: tokens.bg,
      child: rows.isEmpty
          ? const Center(
              child: SizedBox(
                width: 22,
                height: 22,
                child: CircularProgressIndicator(strokeWidth: 2),
              ),
            )
          : ListView.builder(
              padding: const EdgeInsets.fromLTRB(18, 18, 18, 28),
              itemCount: rows.length,
              itemBuilder: (context, i) => Padding(
                padding: const EdgeInsets.symmetric(vertical: 5),
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    SizedBox(
                      width: 104,
                      child: Text(
                        rows[i].label,
                        style: TextStyle(
                          fontFamily: Tokens.fontFamily,
                          fontSize: 11.5,
                          color: tokens.nInk2,
                        ),
                      ),
                    ),
                    Expanded(
                      child: SelectableText(
                        rows[i].value,
                        style: TextStyle(
                          fontFamily: Tokens.fontFamily,
                          fontSize: 11.5,
                          color: tokens.nInk,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
    );
  }
}
