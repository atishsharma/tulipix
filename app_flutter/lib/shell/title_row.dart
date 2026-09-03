// The app's own title row — a port of ui/title_row.slint, minus the frame.
//
// Like Slint's, it REPLACES the row the OS draws rather than adding a second
// one: the window goes undecorated and this becomes the caption row, so the
// Mini Player button sits WITH the window buttons instead of in a strip below
// them. `TULIPIX_NATIVE_FRAME=1` puts the system frame back and drops this row
// to its mark-and-title half — the same escape hatch, and the same name, as
// `csd_enabled()` in crates/tulipix-app/src/miniwin.rs.
//
// The OS window title follows the same string either way, so the taskbar entry
// and the alt-tab card name the song too.
//
// An undecorated GTK window loses its resize border with its frame, which is
// what [WindowResizeEdges] at the bottom of this file puts back: eight handles
// that hand the resize to the compositor, since a frameless window can no more
// resize itself than it can move itself.

import 'dart:async';

import 'package:flutter/gestures.dart' show kPrimaryButton;
import 'package:flutter/material.dart';

import '../design/app_mark.dart';
import '../design/tokens.dart';
import 'shell_controller.dart';
import '../sections/music/music_controller.dart';
import 'window.dart';

/// The row's height, and what the shell above it has to leave clear.
const double kTitleRowHeight = 38;

/// How long a paused track keeps the title before it goes back to the app's own
/// name. Slint's `idlet` timer, and the same reasoning: a paused track is not
/// what the window is, and a title bar naming something that stopped four
/// minutes ago is stale rather than informative.
const Duration kTitleSettle = Duration(seconds: 3);

class AppTitleRow extends StatefulWidget {
  const AppTitleRow({super.key});

  @override
  State<AppTitleRow> createState() => _AppTitleRowState();
}

class _AppTitleRowState extends State<AppTitleRow> {
  final MusicController _music = MusicController.instance;
  final WindowChrome _chrome = WindowChrome.instance;

  /// The track has been paused long enough to stop being the window's name.
  bool _settled = false;
  Timer? _idle;

  /// The last string handed to the window manager. The controller notifies on
  /// every tick — once a second while something plays — and the title only
  /// changes when the track does, so this is what keeps a method call off the
  /// channel 59 times out of 60.
  String? _pushed;

  @override
  void initState() {
    super.initState();
    _music.addListener(_onMusic);
    _chrome.addListener(_onChrome);
    // Whatever is already loaded when the shell is built. The controller does
    // not notify for state it took before anything was listening.
    WidgetsBinding.instance.addPostFrameCallback((_) => _onMusic());
  }

  @override
  void dispose() {
    _music.removeListener(_onMusic);
    _chrome.removeListener(_onChrome);
    _idle?.cancel();
    super.dispose();
  }

  void _onChrome() {
    if (mounted) setState(() {});
  }

  void _onMusic() {
    final playing = _music.tickPlaying;
    final track = _music.state?.now.title ?? '';

    if (playing) {
      // Starting again cancels the countdown and takes the title back.
      _idle?.cancel();
      _idle = null;
      if (_settled && mounted) setState(() => _settled = false);
    } else if (track.isNotEmpty && !_settled && _idle == null) {
      // Armed once, not restarted on every tick — a timer reset by the thing
      // it is timing never fires.
      _idle = Timer(kTitleSettle, () {
        _idle = null;
        if (!mounted) return;
        setState(() => _settled = true);
        // The row redraws itself from `setState`, but the window manager only
        // hears what it is told — without this the taskbar kept naming a track
        // that stopped three seconds ago, which is the exact staleness the
        // timer exists to end.
        _pushTitle(_titleFor(_music.state?.now.title ?? ''));
      });
    }

    _pushTitle(_titleFor(track));
  }

  /// A track that is loaded but never started is idle from the start; so is no
  /// track at all. There is nothing to name in either case.
  bool get _idleNow => (_music.state?.now.title ?? '').isEmpty || _settled;

  String _titleFor(String track) =>
      track.isEmpty || _settled ? 'Tulipix' : 'Tulipix — $track';

  void _pushTitle(String title) {
    if (title == _pushed) return;
    _pushed = title;
    setWindowTitle(title);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: _music,
      builder: (context, _) {
        final track = _music.state?.now.title ?? '';
        final idle = _idleNow;
        // The system frame is still on (TULIPIX_NATIVE_FRAME=1, or a runner
        // that does not answer the channel at all). Its bar IS the title bar —
        // drawing ours under it is two title bars, which is what it looked
        // like. The state above still runs, so the OS bar keeps getting the
        // song through `setTitle`; only the row is gone.
        if (!_chrome.custom) return const SizedBox.shrink();
        return SizedBox(
          height: kTitleRowHeight,
          child: DecoratedBox(
            // A hairline under the row, so the caption reads as chrome rather
            // than as the top of the page.
            decoration: BoxDecoration(
              border: Border(bottom: BorderSide(color: t.outline)),
            ),
            child: Row(
              children: [
                // Everything left of the buttons is the drag handle, which is
                // what a title bar is. The buttons are siblings rather than
                // children so a press on one never starts a move — Slint puts
                // its drag surface under them for the same reason.
                Expanded(
                  child: Listener(
                    behavior: HitTestBehavior.opaque,
                    // On pointer-down, not on a drag gesture: once the
                    // compositor takes the move it keeps the pointer, so the
                    // release never comes back and a Flutter gesture arena
                    // waiting for one would stay armed.
                    onPointerDown: (e) {
                      if (e.buttons == kPrimaryButton) beginWindowDrag();
                    },
                    child: GestureDetector(
                      onDoubleTap: toggleMaximizeWindow,
                      child: Row(
                        children: [
                          const SizedBox(width: 12),
                          // The mark at rest, a note in the record's own colour
                          // while it plays. The swap is the fastest read of
                          // "something is on" there is, and it costs the row
                          // nothing.
                          if (idle)
                            AppMark(
                              size: 16,
                              radius: 5,
                              choice:
                                  ShellController.instance.state?.logoChoice ??
                                      0,
                            )
                          else
                            Icon(Icons.music_note,
                                size: 15, color: _music.accent),
                          const SizedBox(width: 9),
                          Expanded(
                            child: Text(
                              // The same string the window manager was given,
                              // from the same function — two copies of this
                              // rule is how the row and the taskbar end up
                              // disagreeing.
                              _titleFor(track),
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(fontSize: 12, color: t.textDim),
                            ),
                          ),
                        ],
                      ),
                    ),
                  ),
                ),
                // No track, no button. A dead control in the caption row is
                // noise. Unlike the Slint build there is nothing to wait for:
                // its button is gated on the widget WINDOW having been built in
                // the background, and this mini is a card in the tree already.
                // The MINI WIDGET, which is not the mini player. This opens
                // the desktop widget — the window becomes it — and it is the
                // only door to it, the way `mini-widget` in ui/title_row.slint
                // is. The card is the player bar's, and nothing here reaches
                // it.
                if (track.isNotEmpty)
                  _CapBtn(
                    icon: Icons.picture_in_picture_alt,
                    tip: 'Mini widget',
                    onTap: _music.toggleWidget,
                  ),
                // The system trio, in the platform's own order and with the
                // traffic-light hovers every desktop uses — recognisable
                // without a label, which is the whole point of not inventing
                // one.
                Container(width: 1, height: 16, color: t.outline),
                const _CapBtn(
                  icon: Icons.remove,
                  tip: 'Minimise',
                  tone: Color(0xFFD97706),
                  onTap: minimizeWindow,
                ),
                _CapBtn(
                  // A plain square for maximise and overlapping squares for
                  // restore — the shapes every desktop uses. Not the
                  // expand-arrows glyph, which reads as fullscreen.
                  icon:
                      _chrome.maximized ? Icons.filter_none : Icons.crop_square,
                  tip: _chrome.maximized ? 'Restore' : 'Maximise',
                  tone: const Color(0xFF15803D),
                  iconSize: 12,
                  onTap: toggleMaximizeWindow,
                ),
                const _CapBtn(
                  icon: Icons.close,
                  tip: 'Close',
                  tone: Color(0xFFE81123),
                  onTap: closeWindow,
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

/// A caption-row button: square, full height, flat until hovered.
///
/// One blue that works on both themes, darker while held — the accent wash this
/// wore while playing in an earlier Slint pass was a coloured plate sitting in
/// the caption bar doing nothing but shouting.
class _CapBtn extends StatefulWidget {
  const _CapBtn({
    required this.icon,
    required this.onTap,
    this.tip,
    this.tone = const Color(0xFF2563EB),
    this.iconSize = 14,
  });

  final IconData icon;
  final VoidCallback onTap;
  final String? tip;

  /// Hover fill. Each caption button gets its own — the traffic-light
  /// vocabulary, legible on both themes because the plate is a saturated
  /// mid-tone in every case and the glyph on it is always white.
  final Color tone;
  final double iconSize;

  @override
  State<_CapBtn> createState() => _CapBtnState();
}

class _CapBtnState extends State<_CapBtn> {
  bool _hover = false;
  bool _down = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tone = widget.tone;
    final fill = _down
        ? Color.alphaBlend(const Color(0x40000000), tone)
        : (_hover ? tone : Colors.transparent);

    Widget button = MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() {
        _hover = false;
        _down = false;
      }),
      child: GestureDetector(
        onTapDown: (_) => setState(() => _down = true),
        onTapUp: (_) => setState(() => _down = false),
        onTapCancel: () => setState(() => _down = false),
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 90),
          width: 40,
          height: kTitleRowHeight,
          color: fill,
          alignment: Alignment.center,
          child: Padding(
            // One pixel down while held — the whole press feedback.
            padding: EdgeInsets.only(top: _down ? 2 : 0),
            child: Icon(
              widget.icon,
              size: widget.iconSize,
              // Ink at rest, white once the blue is under it.
              color: _hover
                  ? Colors.white
                  : (t.dark ? Colors.white : Colors.black),
            ),
          ),
        ),
      ),
    );
    return widget.tip == null
        ? button
        : Tooltip(message: widget.tip!, child: button);
  }
}

/// The resize border an undecorated window no longer has.
///
/// Eight handles round the edge of whatever it wraps, each handing the resize
/// to the compositor — the same reason the drag is handed over rather than
/// done: a frameless window cannot resize itself any more than it can move
/// itself. Nothing is drawn; these are hit boxes and cursors.
///
/// Wraps the app rather than sitting inside it, and stays out of the way when
/// the frame is the system's or the window is maximised — a maximised window
/// has no edge to pull, and the handles would only eat clicks along it.
class WindowResizeEdges extends StatelessWidget {
  const WindowResizeEdges({super.key, required this.child});

  final Widget child;

  /// How wide the grab strip is. Thin enough not to steal from the sidebar or
  /// the page, wide enough to be findable — the corners get twice it, because
  /// a corner is what people actually aim for.
  static const double thickness = 5;

  @override
  Widget build(BuildContext context) {
    final chrome = WindowChrome.instance;
    final shell = ShellController.instance;
    return AnimatedBuilder(
      // Both, because both can take the handles away: a maximised window has
      // no edge to drag, and logo-fullscreen has none either -- and its top
      // 3px belong to the peek strip, which a north handle would swallow.
      animation: Listenable.merge([chrome, shell]),
      builder: (context, _) {
        if (!chrome.custom || chrome.maximized || shell.appFullscreen) {
          return child;
        }
        const t = thickness;
        const c = thickness * 2;
        return Stack(
          children: [
            child,
            // Edges first, corners over them: a corner is the intersection of
            // two edges and has to win there, or the last few pixels of every
            // corner resize one axis only.
            const _Edge(WindowEdge.north, top: 0, left: c, right: c, height: t),
            const _Edge(WindowEdge.south,
                bottom: 0, left: c, right: c, height: t),
            const _Edge(WindowEdge.west, left: 0, top: c, bottom: c, width: t),
            const _Edge(WindowEdge.east, right: 0, top: c, bottom: c, width: t),
            const _Edge(WindowEdge.northWest,
                top: 0, left: 0, width: c, height: c),
            const _Edge(WindowEdge.northEast,
                top: 0, right: 0, width: c, height: c),
            const _Edge(WindowEdge.southWest,
                bottom: 0, left: 0, width: c, height: c),
            const _Edge(WindowEdge.southEast,
                bottom: 0, right: 0, width: c, height: c),
          ],
        );
      },
    );
  }
}

class _Edge extends StatelessWidget {
  const _Edge(
    this.edge, {
    this.left,
    this.top,
    this.right,
    this.bottom,
    this.width,
    this.height,
  });

  final WindowEdge edge;
  final double? left;
  final double? top;
  final double? right;
  final double? bottom;
  final double? width;
  final double? height;

  @override
  Widget build(BuildContext context) => Positioned(
        left: left,
        top: top,
        right: right,
        bottom: bottom,
        width: width,
        height: height,
        child: MouseRegion(
          cursor: edge.cursor,
          child: Listener(
            behavior: HitTestBehavior.opaque,
            // Same as the drag: on the press, because the compositor keeps the
            // pointer for the whole gesture and the release never returns.
            onPointerDown: (e) {
              if (e.buttons == kPrimaryButton) beginWindowResize(edge);
            },
          ),
        ),
      );
}

/// The bar logo-fullscreen peeks back.
///
/// In logo-fullscreen the frame and the caption row are both gone, so a 3px
/// strip at the very top of the screen is the only chrome left: hovering it
/// slides out a 42px bar with the mark, the title and the two ways out --
/// restore, and close. `fs-peek` in ui/main.slint, including its 250ms grace,
/// which is what stops the hand-off between the bar and a button inside it
/// from flickering the bar shut under the pointer.
///
/// Returns a [Positioned], so it goes straight into a [Stack]'s children.
class FullscreenPeek extends StatefulWidget {
  const FullscreenPeek({super.key});

  @override
  State<FullscreenPeek> createState() => _FullscreenPeekState();
}

class _FullscreenPeekState extends State<FullscreenPeek> {
  bool _open = false;
  Timer? _hide;

  @override
  void dispose() {
    _hide?.cancel();
    super.dispose();
  }

  void _enter() {
    _hide?.cancel();
    if (!_open) setState(() => _open = true);
  }

  void _leave() {
    _hide?.cancel();
    _hide = Timer(const Duration(milliseconds: 250), () {
      if (mounted) setState(() => _open = false);
    });
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (!_open) {
      return Positioned(
        top: 0,
        left: 0,
        right: 0,
        // Not opaque: the strip watches for the pointer without taking the
        // click, so the top 3px of whatever page is up still works.
        child: MouseRegion(
          opaque: false,
          onEnter: (_) => _enter(),
          child: const SizedBox(height: 3),
        ),
      );
    }
    return Positioned(
      top: 0,
      left: 0,
      right: 0,
      child: MouseRegion(
        onEnter: (_) => _enter(),
        onExit: (_) => _leave(),
        child: Container(
          height: 42,
          decoration: BoxDecoration(
            color: t.panel.withValues(alpha: 0.95),
            border: Border(bottom: BorderSide(color: t.outline)),
            boxShadow: const [
              BoxShadow(color: Color(0x40000000), blurRadius: 18),
            ],
          ),
          child: Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              const AppMark(size: 20, radius: 6, choice: 0),
              const SizedBox(width: 10),
              Text('Tulipix',
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      color: t.text)),
              const SizedBox(width: 10),
              _PeekBtn(
                icon: Icons.fullscreen_exit,
                tip: 'Leave fullscreen',
                onTap: () => ShellController.instance.setAppFullscreen(false),
              ),
              const _PeekBtn(
                icon: Icons.close,
                tip: 'Close',
                tone: Color(0xFFE81123),
                onTap: closeWindow,
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _PeekBtn extends StatefulWidget {
  const _PeekBtn({
    required this.icon,
    required this.onTap,
    required this.tip,
    this.tone,
  });

  final IconData icon;
  final VoidCallback onTap;
  final String tip;
  final Color? tone;

  @override
  State<_PeekBtn> createState() => _PeekBtnState();
}

class _PeekBtnState extends State<_PeekBtn> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Tooltip(
      message: widget.tip,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: Container(
            width: 30,
            height: 30,
            decoration: BoxDecoration(
              color: _hover ? (widget.tone ?? t.panel2) : Colors.transparent,
              borderRadius: BorderRadius.circular(8),
            ),
            alignment: Alignment.center,
            child: Icon(
              widget.icon,
              size: 14,
              color: _hover && widget.tone != null ? Colors.white : t.text,
            ),
          ),
        ),
      ),
    );
  }
}
