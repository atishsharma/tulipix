// Motion primitives for the music section.
//
// The section had no `AnimationController` anywhere: tracks changed, panels
// opened and tabs switched by snapping between frames. These are the three
// pieces the rest of it builds on -- an entrance, a swap, and a flight -- kept
// together so the durations and curves cannot drift apart per widget.
//
// Every one of them checks `Tokens.reduceMotion`, which the Settings toggle
// already writes, and collapses to the finished state rather than to a
// different animation.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';

/// Shared timings. A track change is slower than a tap because it is not a
/// response to the pointer -- it wants to read as the music moving on.
class Motion {
  const Motion._();

  static const Duration swap = Duration(milliseconds: 320);
  static const Duration flight = Duration(milliseconds: 380);

  /// Per-item delay for a staggered list, and how many items get one. Past a
  /// dozen the stagger stops reading as choreography and starts reading as
  /// lag, so the rest arrive with the twelfth.
  static const Duration step = Duration(milliseconds: 26);
  static const int maxStagger = 12;

  /// How long the room takes to change colour on a new record. Slower than a
  /// swap on purpose: lighting that changes as fast as a control does reads as
  /// a flicker, and this is meant to be felt rather than noticed.
  static const Duration wash = Duration(milliseconds: 900);

  static const Curve enter = Curves.easeOutCubic;
  static const Curve exit = Curves.easeInCubic;
  static const Curve ease = Curves.easeInOut;
}

/// Fade-and-rise entrance, delayed by position in the list.
///
/// The child is built at its resting state and animated *from* below it, so a
/// list that never gets its frame still shows everything -- the failure mode of
/// an entrance animation must not be an invisible page.
class StaggerIn extends StatefulWidget {
  const StaggerIn({
    super.key,
    required this.index,
    required this.child,
    this.offset = 8,
  });

  final int index;
  final Widget child;

  /// How far below its resting place the child starts, in logical pixels.
  final double offset;

  @override
  State<StaggerIn> createState() => _StaggerInState();
}

class _StaggerInState extends State<StaggerIn>
    with SingleTickerProviderStateMixin {
  late final AnimationController _c = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 280),
  );

  @override
  void initState() {
    super.initState();
    final slot = widget.index.clamp(0, Motion.maxStagger);
    Future<void>.delayed(Motion.step * slot, () {
      // The list can scroll this row out of existence before its turn comes.
      if (mounted) _c.forward();
    });
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (context.tokens.reduceMotion) return widget.child;
    final curved = CurvedAnimation(parent: _c, curve: Motion.enter);
    return AnimatedBuilder(
      animation: curved,
      builder: (context, child) => Opacity(
        opacity: curved.value,
        child: Transform.translate(
          offset: Offset(0, widget.offset * (1 - curved.value)),
          child: child,
        ),
      ),
      child: widget.child,
    );
  }
}

/// Cross-dissolve between two states of the same slot.
///
/// Used for the artwork and the title in the player bar, so a track change
/// dissolves instead of popping. `layoutBuilder` keeps the outgoing child
/// stacked under the incoming one rather than beside it, which is what stops
/// the bar jumping while both are on screen.
class CrossFade extends StatelessWidget {
  const CrossFade({
    super.key,
    required this.slotKey,
    required this.child,
    this.duration = Motion.swap,
    this.alignment = Alignment.centerLeft,
  });

  /// What identity means here: the same key is the same thing, redrawn.
  final Object slotKey;
  final Widget child;
  final Duration duration;
  final Alignment alignment;

  @override
  Widget build(BuildContext context) {
    if (context.tokens.reduceMotion) {
      return KeyedSubtree(key: ValueKey(slotKey), child: child);
    }
    return AnimatedSwitcher(
      duration: duration,
      switchInCurve: Motion.enter,
      switchOutCurve: Motion.exit,
      layoutBuilder: (current, previous) => Stack(
        alignment: alignment,
        children: [...previous, if (current != null) current],
      ),
      child: KeyedSubtree(key: ValueKey(slotKey), child: child),
    );
  }
}

/// Where a played track's artwork flies to: the player bar's cover.
///
/// One key for the whole app, because there is one player bar. It is null-safe
/// by construction -- when the bar is not on screen the key has no context and
/// [ArtFlight.toPlayer] simply does not animate.
final GlobalKey playerArtKey = GlobalKey(debugLabel: 'player-bar-art');

/// Send a copy of some artwork flying from one place on screen to another.
///
/// The music section has no routes -- the detail page is a conditional widget
/// swap inside the tab body, not a push -- so `Hero`, which only animates
/// across a route transition, can never fire here. This does the same job
/// without one: it measures both ends in global coordinates, puts a copy in the
/// overlay, moves it, and takes it away once the real destination has been
/// built underneath.
///
/// It is deliberately fire-and-forget. Nothing awaits it and nothing breaks if
/// the destination never appears; the copy removes itself either way.
class ArtFlight {
  const ArtFlight._();

  /// Fly the widget currently at [fromKey] to wherever [toKey] is.
  ///
  /// Both keys must be attached to a laid-out box. A missing or unmounted end
  /// is not an error -- it means the tile scrolled away or the player bar is
  /// not on screen, and the right answer then is simply not to animate.
  static void run(
    BuildContext context, {
    required GlobalKey fromKey,
    required GlobalKey toKey,
    required Widget art,
    double toRadius = 6,
    double fromRadius = 8,
  }) {
    if (context.tokens.reduceMotion) return;
    final from = _rectOf(fromKey);
    final to = _rectOf(toKey);
    if (from == null || to == null) return;
    // A flight that lands where it started is a flicker, not an animation.
    if ((from.center - to.center).distance < 4) return;

    final overlay = Overlay.maybeOf(context);
    if (overlay == null) return;

    final controller = AnimationController(
      vsync: overlay,
      duration: Motion.flight,
    );
    final curve = CurvedAnimation(parent: controller, curve: Motion.enter);

    late final OverlayEntry entry;
    entry = OverlayEntry(
      builder: (context) => AnimatedBuilder(
        animation: curve,
        builder: (context, _) {
          final t = curve.value;
          final rect = Rect.lerp(from, to, t)!;
          return Positioned(
            left: rect.left,
            top: rect.top,
            width: rect.width,
            height: rect.height,
            child: IgnorePointer(
              child: Opacity(
                // Fades out only at the very end, so the copy is gone by the
                // time the real artwork has painted under it.
                opacity: t < .85 ? 1 : 1 - (t - .85) / .15,
                child: ClipRRect(
                  borderRadius:
                      BorderRadius.circular(_lerp(fromRadius, toRadius, t)),
                  child: art,
                ),
              ),
            ),
          );
        },
      ),
    );

    overlay.insert(entry);
    controller.forward().whenComplete(() {
      entry.remove();
      controller.dispose();
    });
  }

  /// The common case: this row's artwork to the player bar.
  ///
  /// Called from the one place a row is played, so every list gets it without
  /// each of them knowing the player bar exists.
  static void toPlayer(
    BuildContext context, {
    required GlobalKey fromKey,
    required Widget art,
  }) =>
      run(context, fromKey: fromKey, toKey: playerArtKey, art: art);

  static Rect? _rectOf(GlobalKey key) {
    final box = key.currentContext?.findRenderObject() as RenderBox?;
    if (box == null || !box.hasSize) return null;
    return box.localToGlobal(Offset.zero) & box.size;
  }
}

double _lerp(double a, double b, double t) => a + (b - a) * t;
