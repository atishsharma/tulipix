// The Places map.
//
// Every hard part is already in tulipix-photos: `map::tile` reads a PNG out of
// the MBTiles container, `map::cluster_pins` buckets photos into "1042 photos
// here" pins, and `map::bbox_for_photos` gives the opening viewport. What is
// left is a slippy-map widget: project lat/lon to pixels, work out which tiles
// the viewport covers, and ask for those.
//
// No map package. A slippy map is a grid of images at integer zoom, and the
// projection is four lines; a package would bring a tile-fetching HTTP stack
// this app has no use for -- the tiles are on disk, behind the bridge.

import 'dart:io';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/photos.dart';
import 'photos_controller.dart';

const double _tileSize = 256;
const int _minZoom = 2;
const int _maxZoom = 14;

/// Web Mercator, in tiles at zoom `z`. The inverse of the flip that
/// `photos_map_tile` applies for MBTiles' TMS ordering.
double _lonToTileX(double lon, int z) => (lon + 180.0) / 360.0 * (1 << z);

double _latToTileY(double lat, int z) {
  final r = lat * math.pi / 180.0;
  return (1.0 - math.log(math.tan(r) + 1.0 / math.cos(r)) / math.pi) /
      2.0 *
      (1 << z);
}

class PlacesMap extends StatefulWidget {
  const PlacesMap({super.key, required this.controller, required this.state});

  final PhotosController controller;
  final PhotosState state;

  @override
  State<PlacesMap> createState() => _PlacesMapState();
}

class _PlacesMapState extends State<PlacesMap> {
  int _z = 4;

  /// Centre, in tile units at `_z`. Kept in tile space rather than lat/lon so
  /// a drag is a subtraction and not a re-projection every frame.
  double _cx = 0;
  double _cy = 0;
  bool _placed = false;

  final Map<String, Uint8List?> _tiles = {};
  final Set<String> _inFlight = {};

  @override
  void didUpdateWidget(PlacesMap old) {
    super.didUpdateWidget(old);
    if (!_placed) _fitBounds();
  }

  @override
  void initState() {
    super.initState();
    _fitBounds();
  }

  /// Open on everything, which for most libraries is one city and for some is
  /// the whole world.
  void _fitBounds() {
    final b = widget.state.bounds;
    if (b == null) return;
    final lat = (b.minLat + b.maxLat) / 2;
    final lon = (b.minLon + b.maxLon) / 2;
    final spanLon = (b.maxLon - b.minLon).abs().clamp(0.01, 360.0);
    // 360 degrees fits one tile at z0, so the zoom that fits the span is
    // log2(360 / span), minus a tile's worth of margin.
    final z =
        (math.log(360.0 / spanLon) / math.ln2).floor().clamp(_minZoom, 10);
    _z = z;
    _cx = _lonToTileX(lon, z);
    _cy = _latToTileY(lat, z);
    _placed = true;
  }

  Future<void> _fetch(int z, int x, int y) async {
    final key = '$z/$x/$y';
    if (_tiles.containsKey(key) || _inFlight.contains(key)) return;
    _inFlight.add(key);
    try {
      final bytes = await photosMapTile(z: z, x: x, y: y);
      if (!mounted) return;
      setState(() => _tiles[key] = bytes);
    } finally {
      _inFlight.remove(key);
    }
  }

  void _zoomBy(int delta, Size size, Offset focus) {
    final next = (_z + delta).clamp(_minZoom, _maxZoom);
    if (next == _z) return;
    // Keep the point under the cursor fixed: convert it to tile space at the
    // old zoom, scale, and re-centre on it.
    final fx = _cx + (focus.dx - size.width / 2) / _tileSize;
    final fy = _cy + (focus.dy - size.height / 2) / _tileSize;
    final k = math.pow(2, next - _z).toDouble();
    setState(() {
      _cx = fx * k - (focus.dx - size.width / 2) / _tileSize;
      _cy = fy * k - (focus.dy - size.height / 2) / _tileSize;
      _z = next;
    });
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return LayoutBuilder(
      builder: (context, box) {
        final size = Size(box.maxWidth, box.maxHeight);
        final n = 1 << _z;

        // Which tiles the viewport covers, with a one-tile margin so a drag
        // does not reveal an empty edge before the fetch lands.
        final left = _cx - size.width / 2 / _tileSize;
        final top = _cy - size.height / 2 / _tileSize;
        final x0 = left.floor() - 1;
        final y0 = top.floor() - 1;
        final x1 = (left + size.width / _tileSize).ceil() + 1;
        final y1 = (top + size.height / _tileSize).ceil() + 1;

        final children = <Widget>[];
        for (var x = x0; x <= x1; x++) {
          for (var y = y0; y <= y1; y++) {
            if (y < 0 || y >= n) continue;
            final wx = x % n < 0 ? x % n + n : x % n;
            final key = '$_z/$wx/$y';
            if (!_tiles.containsKey(key)) {
              // Fetching during layout is not allowed; the frame after is.
              WidgetsBinding.instance
                  .addPostFrameCallback((_) => _fetch(_z, wx, y));
            }
            final bytes = _tiles[key];
            if (bytes == null) continue;
            children.add(Positioned(
              left: (x - left) * _tileSize,
              top: (y - top) * _tileSize,
              width: _tileSize,
              height: _tileSize,
              child: Image.memory(
                bytes,
                fit: BoxFit.fill,
                gaplessPlayback: true,
                errorBuilder: (_, __, ___) => const SizedBox.shrink(),
              ),
            ));
          }
        }

        for (final pin in widget.state.pins) {
          final px = (_lonToTileX(pin.lon, _z) - left) * _tileSize;
          final py = (_latToTileY(pin.lat, _z) - top) * _tileSize;
          if (px < -60 ||
              py < -60 ||
              px > size.width + 60 ||
              py > size.height + 60) {
            continue;
          }
          children.add(Positioned(
            left: px - 26,
            top: py - 26,
            child: _Pin(pin: pin, controller: widget.controller),
          ));
        }

        return Listener(
          onPointerSignal: (e) {
            if (e is PointerScrollEvent) {
              _zoomBy(e.scrollDelta.dy < 0 ? 1 : -1, size, e.localPosition);
            }
          },
          child: GestureDetector(
            onPanUpdate: (d) => setState(() {
              _cx -= d.delta.dx / _tileSize;
              _cy = (_cy - d.delta.dy / _tileSize).clamp(0.0, n.toDouble());
            }),
            child: ClipRect(
              child: Stack(
                fit: StackFit.expand,
                children: [
                  ColoredBox(color: t.nTile),
                  ...children,
                  Positioned(
                    right: 14,
                    bottom: 14,
                    child: Column(
                      children: [
                        _ZoomButton(
                          icon: Icons.add,
                          onTap: () => _zoomBy(
                              1, size, Offset(size.width / 2, size.height / 2)),
                        ),
                        const SizedBox(height: 6),
                        _ZoomButton(
                          icon: Icons.remove,
                          onTap: () => _zoomBy(-1, size,
                              Offset(size.width / 2, size.height / 2)),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}

class _Pin extends StatelessWidget {
  const _Pin({required this.pin, required this.controller});

  final MapPin pin;
  final PhotosController controller;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: '${pin.count} ${pin.count == 1 ? 'photo' : 'photos'} here',
      child: Container(
        width: 52,
        height: 52,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          border: Border.all(color: Colors.white, width: 2.5),
          boxShadow: const [
            BoxShadow(color: Color(0x66000000), blurRadius: 8, spreadRadius: 1),
          ],
          image: pin.cover.isEmpty
              ? null
              : DecorationImage(
                  image: FileImage(File(pin.cover)),
                  fit: BoxFit.cover,
                ),
          color: Tokens.secPhotos,
        ),
        child: Align(
          alignment: Alignment.bottomRight,
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
            decoration: BoxDecoration(
              color: Tokens.secPhotos,
              borderRadius: BorderRadius.circular(8),
              border: Border.all(color: Colors.white, width: 1.5),
            ),
            child: Text(
              pin.count > 99 ? '99+' : '${pin.count}',
              style: const TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: 9.5,
                fontWeight: FontWeight.w700,
                color: Colors.white,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _ZoomButton extends StatelessWidget {
  const _ZoomButton({required this.icon, required this.onTap});

  final IconData icon;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: t.nCard,
      shape: const CircleBorder(),
      elevation: 2,
      child: InkWell(
        customBorder: const CircleBorder(),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.all(8),
          child: Icon(icon, size: 18, color: t.nInk),
        ),
      ),
    );
  }
}
