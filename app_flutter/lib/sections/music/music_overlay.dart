// Where the two floating players actually live.
//
// Above every section, not inside the Music page — which is the whole point of
// them. The Music page owns the bottom bar; this owns the things that outlive
// leaving the page.

import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import '../../shell/shell_controller.dart';
import '../../shell/window.dart';
import 'mini_player.dart';
import 'mini_widget.dart';
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
    if (event.logicalKey == LogicalKeyboardKey.escape) return _escape(c);
    return false;
  }

  /// Escape, as one back button for the whole app.
  ///
  /// It used to be four unrelated bindings and a gap: zen bound it through
  /// `CallbackShortcuts`, which is dispatched up the focus chain and therefore
  /// never fired — the same reason the space bar did nothing before it moved
  /// here. The rule now is a ladder, innermost first, and the first rung that
  /// applies wins.
  ///
  /// Deliberately *not* here: fullscreen video and the photo viewer. Both hold
  /// focus and answer Escape from their own `Focus.onKeyEvent`, and a handler
  /// registered on `HardwareKeyboard` runs before the focus chain is walked at
  /// all — so consuming Escape here would take it away from them. Their rung
  /// is "something has focus", which is checked first.
  bool _escape(MusicController c) {
    // A focused text field owns Escape: it means "stop editing", not "leave
    // the page". Dropping focus rather than navigating is what every desktop
    // search box does, and it is also what keeps a fullscreen player or the
    // photo viewer -- which hold focus -- answering Escape themselves.
    final focus = FocusManager.instance.primaryFocus;
    if (focus != null && focus != FocusManager.instance.rootScope) {
      if (_typing()) {
        focus.unfocus();
        return true;
      }
      return false;
    }

    // The two surfaces that take the whole window.
    if (c.miniIsWidget) {
      c.closeWidget();
      return true;
    }
    if (c.zenOpen) {
      c.closeZen();
      return true;
    }
    // Escape docks the open mini, the way main.slint's window-level handler
    // does.
    if (c.miniOpen && !c.miniBubble) {
      c.setMiniBubble(true);
      return true;
    }
    // A dialog is the next thing in, and nothing binds Escape to closing one.
    final nav = Navigator.maybeOf(context, rootNavigator: true);
    if (nav != null && nav.canPop()) {
      nav.pop();
      return true;
    }
    // Everything below belongs to Music, so only while Music is the section on
    // screen -- Escape on the Photos page should not quietly close a queue
    // panel three sections away.
    if (ShellController.instance.section != Section.music) return false;
    if (c.panel.isNotEmpty) {
      c.setPanel(c.panel);
      return true;
    }
    if (c.canGoBack) {
      c.goBack();
      return true;
    }
    if (c.state?.detailOpen ?? false) {
      c.closeDetail();
      return true;
    }
    return false;
  }

  @override
  Widget build(BuildContext context) {
    final c = MusicController.instance;
    // `live`: the mini, the zen player and the widget all show the position.
    return AnimatedBuilder(
      animation: c.live,
      builder: (context, _) {
        final now = c.state?.now;
        final anything = now != null && (now.loaded || now.title.isNotEmpty);
        // Escape docks the open mini, the way main.slint's window-level key
        // handler does. Innermost wins, so the zen player's own Escape still
        // closes the zen player, and a dialog is above `home` and out of this
        // subtree entirely.
        // The window IS the widget now — the runner has shrunk it to the
        // widget's box and put it above everything, and there is no shell left
        // to draw. `open_mini` in miniwin.rs hides the main window at exactly
        // this point; with one window, not drawing the app is the same thing.
        if (c.miniIsWidget && anything) {
          return _WidgetWindow(controller: c);
        }
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

/// The widget, filling the window it has become.
///
/// No `Positioned`, no clamping and no bubble: the box is the window, and the
/// window is the widget's box. What replaces the drag is the window's own move
/// — a frameless toplevel cannot move itself, so the body hands the drag to the
/// compositor, and the grip in the corner resizes the window through the same
/// `widgetScale` the settings picker sets.
class _WidgetWindow extends StatelessWidget {
  const _WidgetWindow({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) => Stack(
        clipBehavior: Clip.none,
        children: [
          // A pan rather than a pointer-down, unlike the caption row: the whole
          // body is the drag surface here and every control in the widget is
          // inside it. Tap and pan share the arena, so a click still reaches
          // the button under it and only actual movement starts the move.
          //
          // ponytail: the compositor keeps the pointer once it takes the move,
          // so the pan never gets its up. Recognisers reset on the next
          // pointer-down, which is why this needs no cooling timer; if a stuck
          // drag ever shows up, that timer is the fix Slint used.
          GestureDetector(
            behavior: HitTestBehavior.deferToChild,
            onPanStart: (_) => beginWindowDrag(),
            child: SizedBox.expand(
              child: MiniWidgetCard(controller: controller),
            ),
          ),
          Positioned(
            // Inside the PANEL, not the window: the window carries the shadow
            // margin, and a grip pinned to its corner would float in the
            // shadow. 3px in from the panel edge, which is the gap that makes
            // the arc legible against it.
            right: kMiniPad.right * controller.widgetScale + 3,
            bottom: kMiniPad.bottom * controller.widgetScale + 3,
            child: MouseRegion(
              cursor: SystemMouseCursors.resizeUpLeftDownRight,
              child: GestureDetector(
                onPanUpdate: (d) => controller.scaleWidget(
                  controller.widgetScale +
                      d.delta.dx / controller.widgetStyle.base.width,
                ),
                child: _ResizeArc(
                  // The pill is half the height of the others and a 20px arc in
                  // its corner is most of it.
                  size: (controller.widgetStyle == MiniStyle.pill ? 16 : 20) *
                      controller.widgetScale,
                  stroke: 2 * controller.widgetScale,
                ),
              ),
            ),
          ),
        ],
      );
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
    // The card, and only the card. The three widget classes are not drawn here
    // at all — they take the window, and `_WidgetWindow` above is where they
    // land.
    final box = controller.miniWindow;
    final w = box.width;
    final h = box.height;
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
            right: 3,
            bottom: 3,
            child: MouseRegion(
              cursor: SystemMouseCursors.resizeUpLeftDownRight,
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
                // The same corner ring the widget uses — MusicMini's
                // bottom-right handle in ui/main.slint is this arc too.
                child: _ResizeArc(
                  size: 20 * controller.miniScale,
                  stroke: 2 * controller.miniScale,
                ),
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

/// The corner you drag to resize, on the card and on the widget both.
///
/// An ARC that runs parallel to the corner it sits in, not a ring stamped over
/// it — the `Path { MoveTo 0,100; ArcTo r=100 -> 100,0 }` in
/// ui/mini_widget.slint, whose comment is the reason: "a circle read as a
/// button; this reads as the corner itself being grabbable, and the gap is what
/// makes it legible against the edge". A circle is what this was, and it read
/// as a button there too.
///
/// The hit box is bigger than the mark, because a 2px arc is not a target.
class _ResizeArc extends StatefulWidget {
  const _ResizeArc({required this.size, required this.stroke});

  /// The hit box, and the arc's radius: the quarter circle is centred on the
  /// box's top-left, so it passes through the corner the box sits in.
  final double size;
  final double stroke;

  @override
  State<_ResizeArc> createState() => _ResizeArcState();
}

class _ResizeArcState extends State<_ResizeArc> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) => MouseRegion(
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: CustomPaint(
          size: Size.square(widget.size),
          painter: _ArcPainter(
            stroke: widget.stroke,
            // Accent while it is grabbable, the panel's strong outline
            // otherwise — `Theme.outline-strong` there.
            color: _hover
                ? MusicController.instance.accent
                : context.tokens.outlineStrong,
          ),
        ),
      );
}

class _ArcPainter extends CustomPainter {
  const _ArcPainter({required this.stroke, required this.color});

  final double stroke;
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    // Centred on the top-left of the box with a radius of the box, so it runs
    // from the bottom-left corner to the top-right one through the middle of
    // the bottom-right — parallel to the panel corner, 3px outside it.
    canvas.drawArc(
      Rect.fromCircle(center: Offset.zero, radius: size.width),
      math.pi / 2,
      -math.pi / 2,
      false,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = stroke
        ..strokeCap = StrokeCap.round
        ..color = color,
    );
  }

  @override
  bool shouldRepaint(_ArcPainter old) =>
      old.color != color || old.stroke != stroke;
}
