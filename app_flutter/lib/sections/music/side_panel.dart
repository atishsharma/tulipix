// Queue and Lyrics, in one right-docked panel.
//
// They were two separate 260px strips that pushed up out of the player bar,
// which is not what the Slint build does and not what either of them wants to
// be. `np.p5.atmusic.sidebar-panels` is a single 432px column docked to the
// right edge, running from under the two-row header down to the top of the
// player, with the two views behind chips at its head and a close button
// beside them. A lyric sheet is tall and narrow; a queue is a list. Neither
// reads in a 260px letterbox.
//
// Clicking the page to its left dismisses it, as there.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'meta_manager.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';
import 'music_widgets.dart';

/// The docked width. 432 in ui/page_music.slint.
const double kSidePanelWidth = 432;

/// Where the panel is, so a pointer landing anywhere else can dismiss it.
///
/// The dismiss target used to be the page to its left, which is what Slint
/// draws — but Slint's is inside a window the panel also has to share with a
/// header and a player, and clicking either of those left it up. The check
/// lives in the overlay that wraps the whole app now, and this is how it knows
/// what "outside" means.
final GlobalKey sidePanelKey = GlobalKey();

class SidePanel extends StatelessWidget {
  const SidePanel({super.key, required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final which = controller.panel;
    return Row(
      children: [
        // The dismiss target. Everything left of the panel, as in Slint —
        // opaque so the grid underneath does not take the hover either.
        Expanded(
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: () => controller.setPanel(which),
            child: const SizedBox.expand(),
          ),
        ),
        // Slides in from the edge it is docked to, as in Slint: `x` starts a
        // few pixels off the wall at zero opacity and the init handler drops
        // it home. A `TweenAnimationBuilder` with no explicit end state does
        // the same thing — it runs once, on first build, and a panel that is
        // already up is not rebuilt into existence again.
        TweenAnimationBuilder<double>(
          tween: Tween<double>(begin: 0, end: 1),
          duration: const Duration(milliseconds: 220),
          curve: Curves.easeOut,
          builder: (context, v, child) => Opacity(
            opacity: v,
            child: Transform.translate(
              offset: Offset((1 - v) * 28, 0),
              child: child,
            ),
          ),
          child: SizedBox(
            width: kSidePanelWidth,
            child: DecoratedBox(
              decoration: BoxDecoration(
                color: t.panel2,
                // No cast shadow: it bled onto the player bar in Slint too.
                border: Border(left: BorderSide(color: t.outline)),
              ),
              child: Padding(
                padding: const EdgeInsets.all(18),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    _Tabs(controller: controller),
                    const SizedBox(height: 12),
                    Expanded(
                      child: which == 'lyrics'
                          ? _Lyrics(controller: controller)
                          : _Queue(controller: controller),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

class _Tabs extends StatelessWidget {
  const _Tabs({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final which = controller.panel;
    // The two chips share the row rather than huddling at its left end: they
    // are this panel's tabs, and a tab that is 90px wide in a 432px column
    // reads as a button someone forgot to lay out.
    return Row(
      children: [
        Expanded(
          child: MusicChip(
            icon: Icons.queue_music,
            label: 'Queue',
            active: which == 'queue',
            tint: const Color(0xFF3B82F6),
            tint2: const Color(0xFF6366F1),
            expand: true,
            onTap: () => controller.showPanel('queue'),
          ),
        ),
        const SizedBox(width: 8),
        Expanded(
          child: MusicChip(
            icon: Icons.lyrics_outlined,
            label: 'Lyrics',
            active: which == 'lyrics',
            tint: const Color(0xFFEC4899),
            tint2: const Color(0xFF8B5CF6),
            expand: true,
            onTap: () => controller.showPanel('lyrics'),
          ),
        ),
        const SizedBox(width: 8),
        SizedBox(
          width: 30,
          height: 30,
          child: Material(
            color: t.nChip,
            shape: const CircleBorder(),
            clipBehavior: Clip.antiAlias,
            child: InkWell(
              onTap: () => controller.setPanel(which),
              child: Icon(Icons.close, size: 15, color: t.nInk2),
            ),
          ),
        ),
      ],
    );
  }
}

class _Queue extends StatelessWidget {
  const _Queue({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final queue = controller.state?.queue ?? const <Track>[];
    if (queue.isEmpty) {
      return const MusicEmpty(
        icon: Icons.queue_music_outlined,
        title: 'The queue is empty',
        body: 'Play an album, a playlist or a folder and it lands here.',
      );
    }
    return Column(
      children: [
        Row(
          children: [
            Text(
              'Up next · ${queue.length}',
              style: TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: 13,
                fontWeight: FontWeight.w700,
                color: t.nInk2,
              ),
            ),
            const Spacer(),
            TextButton(
              onPressed: () => confirmThen(
                context,
                controller,
                title: 'Clear the queue?',
                body: 'Everything lined up after the current track is '
                    'dropped. What is playing keeps playing.',
                action: 'Clear queue',
                cmd: const MusicCmd.queueClear(),
              ),
              child: const Text('Clear'),
            ),
          ],
        ),
        Expanded(
          child: ReorderableListView.builder(
            buildDefaultDragHandles: true,
            // Space between the rows, not only inside them: this list is
            // dragged, and rows that touch give the pointer nothing to aim
            // between.
            padding: const EdgeInsets.symmetric(vertical: 4),
            itemCount: queue.length,
            // onReorderItem, unlike the deprecated onReorder, already
            // accounts for the lifted row.
            onReorderItem: (from, to) =>
                controller.send(MusicCmd.queueMove(from: from, to: to)),
            itemBuilder: (_, i) => Padding(
              key: ValueKey(queue[i].itemId),
              padding: const EdgeInsets.only(bottom: 6),
              child: TrackRow(
                controller: controller,
                track: queue[i],
                index: i,
                dense: true,
                draggable: true,
                onPlay: () => controller.send(MusicCmd.queuePlayAt(index: i)),
                onRemove: () => controller
                    .send(MusicCmd.queueRemove(itemId: queue[i].itemId)),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

/// The lyric sheet: a Find-lyrics chip and the sync nudge above, and below
/// them a fixed window of lines centred on the one being sung.
///
/// Not a scroll view. Slint draws ten slots — four back, the live line, five
/// forward — each fading with distance, and the text moves through them rather
/// than the list scrolling under a highlight. The effect is a teleprompter,
/// and it is the reason the panel never has a scrollbar fighting the song.
class _Lyrics extends StatelessWidget {
  const _Lyrics({required this.controller});

  final MusicController controller;

  /// The window, as offsets from the live line.
  static const List<int> _slots = [-4, -3, -2, -1, 0, 1, 2, 3, 4, 5];

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = controller.state;
    final lines = st?.lyrics ?? const <LyricLine>[];
    final plain = st?.lyricsPlain ?? '';
    final itemId = st?.now.itemId ?? 0;
    final active = controller.activeLyric;
    // The manager's search state, borrowed. It is open on *this* track only
    // when the song it was opened for is the one playing — the manager can also
    // be driven from the Songs grid, and that search belongs to whatever row
    // was clicked there, not to the deck.
    final finding = st != null &&
        st.mgrMode != 'list' &&
        st.mgrItemId == itemId &&
        itemId != 0;

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            // Slint's `lyrics-find-open`: the chip flips the panel between the
            // words and a form to go looking for them, and flips back when one
            // is picked. It is the same LRCLIB search the Tags & Lyrics manager
            // runs — one query, one set of results, one Save — reached from
            // whichever of the two happens to be open.
            MusicChip(
              icon: finding ? Icons.chevron_left : Icons.search,
              label: finding ? 'Back to lyrics' : 'Find lyrics',
              active: finding,
              tint: const Color(0xFF14B8A6),
              tint2: const Color(0xFF22C55E),
              onTap: itemId == 0
                  ? () {}
                  : () => finding
                      ? controller.send(const MusicCmd.mgrBack())
                      : controller.send(MusicCmd.mgrSearchOpen(itemId: itemId)),
            ),
            const Spacer(),
            if (!finding && lines.isNotEmpty) ...[
              _Nudge(
                label: '−',
                onTap: () =>
                    controller.send(const MusicCmd.lyricsOffset(deltaMs: -250)),
              ),
              const SizedBox(width: 6),
              Text(
                '${((st?.lyricsOffsetMs ?? 0) / 1000).toStringAsFixed(2)}s',
                style: TextStyle(fontSize: 11, color: t.nInk3),
              ),
              const SizedBox(width: 6),
              _Nudge(
                label: '+',
                onTap: () =>
                    controller.send(const MusicCmd.lyricsOffset(deltaMs: 250)),
              ),
            ],
          ],
        ),
        const SizedBox(height: 8),
        if (finding)
          Expanded(child: LyricSearch(controller: controller, st: st))
        else
          Expanded(
            child: switch ((lines.isEmpty, plain.isEmpty)) {
              (true, true) => Center(
                  child: Text(
                    itemId == 0
                        ? 'Play something first.'
                        : 'No lyrics yet — tap “Find lyrics”.',
                    textAlign: TextAlign.center,
                    style: TextStyle(fontSize: 13, color: t.nInk2),
                  ),
                ),
              // Unsynced words are a page, not a teleprompter: there is no line
              // to centre on, so it scrolls.
              (true, false) => SingleChildScrollView(
                  child: Text(
                    plain,
                    textAlign: TextAlign.center,
                    style: TextStyle(fontSize: 15, color: t.nInk2),
                  ),
                ),
              _ => Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    for (final off in _slots)
                      Padding(
                        padding: const EdgeInsets.symmetric(vertical: 3.5),
                        child: _Line(
                          text: _at(lines, active + off),
                          off: off,
                        ),
                      ),
                  ],
                ),
            },
          ),
      ],
    );
  }

  static String _at(List<LyricLine> lines, int i) =>
      i >= 0 && i < lines.length ? lines[i].text : '';
}

class _Line extends StatelessWidget {
  const _Line({required this.text, required this.off});

  final String text;
  final int off;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final d = off.abs();
    return AnimatedDefaultTextStyle(
      duration:
          t.reduceMotion ? Duration.zero : const Duration(milliseconds: 220),
      curve: Curves.easeOut,
      style: TextStyle(
        fontFamily: Tokens.fontFamily,
        fontSize: off == 0 ? 24 : (d == 1 ? 18 : 15),
        fontWeight: d <= 1 ? FontWeight.w800 : FontWeight.w500,
        color: (off == 0 ? Tokens.secMusic : t.nInk2).withValues(
          // The fade stands in for a blur: the three middle lines are in
          // focus and everything further out recedes.
          alpha: d <= 1 ? 1.0 : (d == 2 ? 0.58 : (d == 3 ? 0.4 : 0.26)),
        ),
      ),
      child: Text(text, textAlign: TextAlign.center),
    );
  }
}

class _Nudge extends StatelessWidget {
  const _Nudge({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      width: 26,
      height: 26,
      child: Material(
        color: t.nChip,
        shape: const CircleBorder(),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onTap,
          child: Center(
            child: Text(
              label,
              style: TextStyle(
                fontSize: 14,
                fontWeight: FontWeight.w700,
                color: t.nInk2,
              ),
            ),
          ),
        ),
      ),
    );
  }
}
