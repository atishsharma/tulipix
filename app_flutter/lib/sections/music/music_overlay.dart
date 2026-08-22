// Where the two floating players actually live.
//
// Above every section, not inside the Music page — which is the whole point of
// them. The Music page owns the bottom bar; this owns the things that outlive
// leaving the page.

import 'package:flutter/material.dart';

import 'mini_player.dart';
import 'music_controller.dart';
import 'zen_player.dart';

class MusicOverlay extends StatelessWidget {
  const MusicOverlay({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final c = MusicController.instance;
    return AnimatedBuilder(
      animation: c,
      builder: (context, _) {
        final now = c.state?.now;
        final anything = now != null && (now.loaded || now.title.isNotEmpty);
        return Stack(
          children: [
            child,
            if (c.miniOpen && anything && !c.zenOpen)
              c.miniBubble
                  ? Positioned(
                      right: 0,
                      bottom: 120,
                      child: MiniBubble(controller: c),
                    )
                  : _DraggableMini(controller: c),
            if (c.zenOpen && anything)
              Positioned.fill(child: ZenPlayer(controller: c)),
          ],
        );
      },
    );
  }
}

/// The mini, placed. Dragged by its body, resized from the bottom-right corner,
/// and clamped so it can never be dragged off the edge and lost.
class _DraggableMini extends StatelessWidget {
  const _DraggableMini({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, box) {
        final w = kMiniSize.width * controller.miniScale;
        final h = kMiniSize.height * controller.miniScale;
        // No remembered position yet: bottom-right, clear of the player bar.
        final pos = controller.miniPos ??
            Offset(
              (box.maxWidth - w - 24).clamp(0.0, double.infinity),
              (box.maxHeight - h - 140).clamp(0.0, double.infinity),
            );
        final clamped = Offset(
          pos.dx.clamp(0.0, (box.maxWidth - w).clamp(0.0, double.infinity)),
          pos.dy.clamp(0.0, (box.maxHeight - h).clamp(0.0, double.infinity)),
        );
        return Positioned(
          left: clamped.dx,
          top: clamped.dy,
          width: w,
          height: h,
          child: Stack(
            clipBehavior: Clip.none,
            children: [
              GestureDetector(
                behavior: HitTestBehavior.deferToChild,
                onPanUpdate: (d) => controller.moveMini(Offset(
                  (clamped.dx + d.delta.dx)
                      .clamp(0.0, (box.maxWidth - w).clamp(0.0, 1e9)),
                  (clamped.dy + d.delta.dy)
                      .clamp(0.0, (box.maxHeight - h).clamp(0.0, 1e9)),
                )),
                child: MiniPlayer(controller: controller),
              ),
              Positioned(
                right: 0,
                bottom: 0,
                child: MouseRegion(
                  cursor: SystemMouseCursors.resizeDownRight,
                  child: GestureDetector(
                    // Uniform scale, not free resize: the mini's layout is a
                    // fixed composition and stretching one axis would only make
                    // the record an oval.
                    onPanUpdate: (d) => controller.scaleMini(
                      controller.miniScale +
                          (d.delta.dx + d.delta.dy) / (2 * kMiniSize.width),
                    ),
                    child: const SizedBox(
                      width: 22,
                      height: 22,
                      child: Icon(Icons.drag_handle, size: 14),
                    ),
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
