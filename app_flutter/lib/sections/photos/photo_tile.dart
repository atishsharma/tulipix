// One grid cell. Square-cropped 256px thumbnail, hover actions, selection ring.
// Matches `PhotoCell` in ui/page_photos.slint.

import 'dart:io';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/photos.dart';
import 'photos_controller.dart';

class PhotoTileView extends StatefulWidget {
  const PhotoTileView({
    super.key,
    required this.tile,
    required this.controller,
    required this.selected,
    required this.onTap,
    required this.onToggleSelect,
  });

  final PhotoTile tile;
  final PhotosController controller;
  final bool selected;
  final VoidCallback onTap;
  final VoidCallback onToggleSelect;

  @override
  State<PhotoTileView> createState() => _PhotoTileViewState();
}

class _PhotoTileViewState extends State<PhotoTileView> {
  String? _thumb;
  bool _hovered = false;

  @override
  void initState() {
    super.initState();
    _resolveThumb();
  }

  @override
  void didUpdateWidget(PhotoTileView old) {
    super.didUpdateWidget(old);
    if (old.tile.itemId != widget.tile.itemId) {
      _thumb = null;
      _resolveThumb();
    }
  }

  Future<void> _resolveThumb() async {
    final path = await widget.controller.thumbFor(widget.tile);
    // The grid recycles cells while decodes are in flight, so a late result
    // belongs to whichever item the cell now holds — not necessarily this one.
    if (mounted && path != null) setState(() => _thumb = path);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final thumb = _thumb;
    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: widget.onTap,
        onSecondaryTap: widget.onToggleSelect,
        child: Stack(
          fit: StackFit.expand,
          children: [
            Container(
              decoration: BoxDecoration(
                color: t.nTile,
                borderRadius: BorderRadius.circular(2),
              ),
              clipBehavior: Clip.antiAlias,
              child: thumb == null
                  ? const SizedBox.shrink()
                  : Image.file(
                      File(thumb),
                      fit: BoxFit.cover,
                      filterQuality: FilterQuality.medium,
                      // A file that vanished between the query and the paint is
                      // a stale row, not a crash.
                      errorBuilder: (_, __, ___) => Icon(
                          Icons.broken_image_outlined,
                          color: t.nInk2,
                          size: 20),
                    ),
            ),
            if (widget.selected)
              Container(
                decoration: BoxDecoration(
                  border: Border.all(color: Tokens.secPhotos, width: 3),
                  borderRadius: BorderRadius.circular(2),
                ),
              ),
            if (widget.tile.starred)
              const Positioned(
                left: 6,
                bottom: 6,
                child: _TileGlyph(
                    icon: Icons.star_rounded, color: Color(0xFFFFC107)),
              ),
            if (widget.tile.trashed)
              Positioned(
                right: 6,
                bottom: 6,
                child: _TileGlyph(icon: Icons.delete_outline, color: t.nInk2),
              ),
            if (_hovered || widget.selected)
              Positioned(
                top: 4,
                left: 4,
                child: _SelectDot(
                  selected: widget.selected,
                  onTap: widget.onToggleSelect,
                ),
              ),
          ],
        ),
      ),
    );
  }
}

class _TileGlyph extends StatelessWidget {
  const _TileGlyph({required this.icon, required this.color});

  final IconData icon;
  final Color color;

  @override
  Widget build(BuildContext context) => DecoratedBox(
        decoration: const BoxDecoration(
          // A white star on an overexposed sky is invisible; the scrim is what
          // makes every badge readable on every photo.
          gradient:
              RadialGradient(colors: [Color(0x66000000), Color(0x00000000)]),
        ),
        child: Icon(icon, size: 18, color: color),
      );
}

class _SelectDot extends StatelessWidget {
  const _SelectDot({required this.selected, required this.onTap});

  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => GestureDetector(
        onTap: onTap,
        child: Container(
          width: 22,
          height: 22,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: selected ? Tokens.secPhotos : const Color(0x59000000),
            border: Border.all(color: Colors.white, width: 1.5),
          ),
          child: selected
              ? const Icon(Icons.check, size: 14, color: Colors.white)
              : null,
        ),
      );
}
