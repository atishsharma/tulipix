// Where the two floating players actually live.
//
// Above every section, not inside the Music page — which is the whole point of
// them. The Music page owns the bottom bar; this owns the things that outlive
// leaving the page.

import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../src/rust/api/music.dart';
import 'mini_player.dart';
import 'music_controller.dart';
import 'side_panel.dart';
import 'zen_player.dart';

class MusicOverlay extends StatefulWidget {
  const MusicOverlay({super.key, required this.child});

  final Widget child;

  @override
  State<MusicOverlay> createState() => _MusicOverlayState();
}

class _MusicOverlayState extends State<MusicOverlay> {
  @override
  void initState() {
    super.initState();
    HardwareKeyboard.instance.addHandler(_key);
  }

  @override
  void dispose() {
    HardwareKeyboard.instance.removeHandler(_key);
    super.dispose();
  }

  /// Space and Escape, from the keyboard itself rather than from a shortcut.
  ///
  /// `CallbackShortcuts` is dispatched up the *focus* chain, so it only fires
  /// when something inside its subtree holds focus — and on a page where you
  /// have clicked a tile, a chip or nothing at all, the primary focus is the
  /// root scope, which sits above this widget. That is why the space bar did
  /// nothing: the binding was there and was never reached. A keyboard handler
  /// is asked about every key regardless of who has focus, which is what
  /// "space pauses, anywhere in the app" actually means.
  bool _key(KeyEvent event) {
    if (event is! KeyDownEvent) return false;
    final c = MusicController.instance;
    final now = c.state?.now;
    final anything = now != null && (now.loaded || now.title.isNotEmpty);

    if (event.logicalKey == LogicalKeyboardKey.space) {
      // Not while a text field has it: the search box needs its spaces.
      if (!anything || _typing()) return false;
      c.send(const MusicCmd.playPause());
      return true;
    }
    // Escape docks the open mini, the way main.slint's window-level handler
    // does. Zen has its own, one layer in, and wins where it applies.
    if (event.logicalKey == LogicalKeyboardKey.escape &&
        c.miniOpen &&
        !c.miniBubble &&
        !c.zenOpen) {
      c.setMiniBubble(true);
      return true;
    }
    return false;
  }

  @override
  Widget build(BuildContext context) {
    final c = MusicController.instance;
    return AnimatedBuilder(
      animation: c,
      builder: (context, _) {
        final now = c.state?.now;
        final anything = now != null && (now.loaded || now.title.isNotEmpty);
        // Escape docks the open mini, the way main.slint's window-level key
        // handler does. Innermost wins, so the zen player's own Escape still
        // closes the zen player, and a dialog is above `home` and out of this
        // subtree entirely.
        return Listener(
          // Click-away dismissal for the docked Queue / Lyrics panel, from
          // anywhere in the window rather than only from the page beside it.
          // Translucent so it watches without taking anything: the tap still
          // reaches whatever was under it, and closing the panel is a side
          // effect rather than a swallowed click.
          behavior: HitTestBehavior.translucent,
          onPointerDown: (e) => _dismissPanel(c, e.position),
          child: Builder(
            builder: (context) {
              // The LayoutBuilder is ABOVE the Stack, and it has to be.
              // `Positioned` is a ParentDataWidget: the render object it lands on
              // must be a direct child of the RenderStack, and a LayoutBuilder in
              // between makes it a RenderLayoutBuilder's child instead. That
              // throws at build time — and the ErrorWidget that replaces the mini
              // is an *unpositioned* Stack child, so it sized itself to the
              // constraint and painted over the whole app. That is what "the mini
              // player takes the full window" was.
              return LayoutBuilder(
                builder: (context, box) => Stack(
                  children: [
                    widget.child,
                    if (c.miniOpen && anything && !c.zenOpen)
                      c.miniBubble
                          ? _WallBubble(controller: c, area: box.biggest)
                          : _DraggableMini(controller: c, area: box.biggest),
                    if (c.zenOpen && anything)
                      Positioned.fill(child: ZenPlayer(controller: c)),
                  ],
                ),
              );
            },
          ),
        );
      },
    );
  }
}

/// Close the docked panel when the pointer lands outside it.
void _dismissPanel(MusicController c, Offset at) {
  if (c.panel != 'queue' && c.panel != 'lyrics') return;
  final box = sidePanelKey.currentContext?.findRenderObject() as RenderBox?;
  if (box == null || !box.hasSize) return;
  if (!(box.localToGlobal(Offset.zero) & box.size).contains(at)) {
    c.setPanel(c.panel);
  }
}

/// Whether the keyboard belongs to a text field right now.
///
/// A `Shortcuts` binding sits above the focused widget, so without this the
/// space bar pauses the music instead of putting a space in the search box.
/// The focus node a TextField installs lives inside `EditableText`, so its
/// context has one as an ancestor and nothing else does.
bool _typing() {
  final focus = FocusManager.instance.primaryFocus;
  return focus?.context?.findAncestorStateOfType<EditableTextState>() != null;
}

/// The mini, placed. Dragged by its body, resized from the bottom-right corner,
/// and clamped so it can never be dragged off the edge and lost.
class _DraggableMini extends StatelessWidget {
  const _DraggableMini({required this.controller, required this.area});

  final MusicController controller;

  /// The space the mini is clamped inside. Measured by the overlay above the
  /// Stack and passed down — see the note there for why it cannot be measured
  /// here.
  final Size area;

  @override
  Widget build(BuildContext context) {
    final w = kMiniSize.width * controller.miniScale;
    final h = kMiniSize.height * controller.miniScale;
    final maxX = math.max(0.0, area.width - w);
    final maxY = math.max(0.0, area.height - h);
    // No remembered position yet: dead centre, which is where main.slint puts
    // it — `music-mini-x = (width - w) / 2`. It used to open bottom-right,
    // over the player bar it is meant to replace.
    final pos = controller.miniPos ?? Offset(maxX / 2, maxY / 2);
    final clamped = Offset(pos.dx.clamp(0.0, maxX), pos.dy.clamp(0.0, maxY));
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
              (clamped.dx + d.delta.dx).clamp(0.0, maxX),
              (clamped.dy + d.delta.dy).clamp(0.0, maxY),
            )),
            child: MiniPlayer(controller: controller),
          ),
          Positioned(
            right: 0,
            bottom: 0,
            child: MouseRegion(
              cursor: SystemMouseCursors.resizeDownRight,
              child: GestureDetector(
                // Uniform scale, not free resize: the mini's layout is a fixed
                // composition and stretching one axis would only make the
                // record an oval. Horizontal travel only, over the reference
                // width — `music-mini-scale + (mouse-x - pressed-x) / 300px`
                // in ui/main.slint. Dragging straight down does nothing there
                // and does nothing here.
                onPanUpdate: (d) => controller.scaleMini(
                  controller.miniScale + d.delta.dx / kMiniSize.width,
                ),
                child: const _ResizeArc(),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// The bubble, pinned to the right wall and draggable along it.
///
/// `x: root.width - 78px`, `y: music-bubble-y < 0px ? height / 2 - 30px : ...`,
/// clamped 8px from the top and 68px from the bottom — all of it from
/// ui/main.slint. Horizontal drag is ignored on purpose: it is pinned.
class _WallBubble extends StatelessWidget {
  const _WallBubble({required this.controller, required this.area});

  final MusicController controller;
  final Size area;

  @override
  Widget build(BuildContext context) {
    final maxY = math.max(8.0, area.height - kBubbleSize - 8);
    final y = (controller.bubbleY ?? (area.height / 2 - kBubbleSize / 2))
        .clamp(8.0, maxY);
    return Positioned(
      left: area.width - kBubbleSize - 18,
      top: y,
      width: kBubbleSize,
      height: kBubbleSize,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onVerticalDragUpdate: (d) =>
            controller.moveBubble((y + d.delta.dy).clamp(8.0, maxY)),
        // A drag that ends where it began is a tap, and a tap brings the mini
        // back — the `if (!self.dragged)` in main.slint's bubble.
        onTap: () => controller.setMiniBubble(false),
        child: MouseRegion(
          cursor: SystemMouseCursors.click,
          child: MiniBubble(controller: controller),
        ),
      ),
    );
  }
}

/// The corner you drag to resize the mini.
///
/// Slint draws a 16px ring inside a 24px hit target at the bottom-right — an
/// arc of the card's own corner, lighting up in the artwork's accent while it
/// is grabbed. The port had `Icons.drag_handle`, which is an equals sign in the
/// corner of a record player.
class _ResizeArc extends StatefulWidget {
  const _ResizeArc();

  @override
  State<_ResizeArc> createState() => _ResizeArcState();
}

class _ResizeArcState extends State<_ResizeArc> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) => MouseRegion(
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: SizedBox(
          width: 24,
          height: 24,
          child: Center(
            child: Container(
              width: 16,
              height: 16,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                border: Border.all(
                  width: 2,
                  color: _hover
                      ? MusicController.instance.accent
                      : const Color(0x55FFFFFF),
                ),
              ),
            ),
          ),
        ),
      );
}
