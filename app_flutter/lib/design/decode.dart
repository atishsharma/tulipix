// How large to decode a picture from disk.
//
// The art caches hold whatever the source had — album covers up to 3000²,
// book covers near 1500×2100 — and Flutter decodes the whole file unless told
// otherwise: 36 MB of pixels for one 160 px tile, decoded again for every tile
// that shows it. Passed as `cacheWidth`, this decodes at the size drawn.

import 'dart:io' show File;
import 'dart:math' as math;

import 'package:flutter/widgets.dart';

/// Physical pixels for a picture drawn [w]×[h] logical pixels, on the longer
/// side, so `BoxFit.cover` still has enough to fill the box. Rounded up to a
/// 128 px step, so a window being resized does not decode the file again for
/// every pixel it moves. An unbounded side is ignored; null — full size — only
/// when neither side has a bound.
int? decodePx(BuildContext context, double w, [double h = 0]) {
  final side = math.max(w.isFinite ? w : 0.0, h.isFinite ? h : 0.0);
  if (side <= 0) return null;
  final px = side * MediaQuery.devicePixelRatioOf(context);
  return (px / 128).ceil() * 128;
}

/// `Image.file` decoded at the size it is laid out at, for a picture whose box
/// is decided by its parent rather than given to it.
class FileArt extends StatelessWidget {
  const FileArt(
    this.path, {
    super.key,
    this.fit = BoxFit.cover,
    this.alignment = Alignment.center,
    this.gaplessPlayback = false,
    this.filterQuality = FilterQuality.medium,
    this.errorBuilder,
  });

  final String path;
  final BoxFit fit;
  final AlignmentGeometry alignment;
  final bool gaplessPlayback;
  final FilterQuality filterQuality;
  final ImageErrorWidgetBuilder? errorBuilder;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
        builder: (context, box) => Image.file(
          File(path),
          fit: fit,
          alignment: alignment,
          gaplessPlayback: gaplessPlayback,
          filterQuality: filterQuality,
          cacheWidth: decodePx(context, box.maxWidth, box.maxHeight),
          errorBuilder: errorBuilder,
        ),
      );
}
