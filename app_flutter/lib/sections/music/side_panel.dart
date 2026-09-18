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

import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'audiobooks_tab.dart' show addBookmark, editBookmark, kBookTint;
import 'meta_manager.dart';
import 'lyrics_timer.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';
import 'music_widgets.dart';
import 'player_widgets.dart';
import 'podcasts_tab.dart' show kPodTint;
import 'music_motion.dart';
import 'word_search.dart';
import 'youtube/yt_card.dart' show YtThumb, ytRose;

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
                // Fully opaque in every language. Glass's sheet is 80% and a
                // glass `panel2` is see-through too, and the page showing
                // through a queue is a second list fighting the first.
                color: (context.skin.sheet ?? t.panel2).withValues(alpha: 1),
                // No cast shadow: it bled onto the player bar in Slint too.
                border: Border(left: BorderSide(color: t.outline)),
              ),
              // The record's light reaches the queue too, falling from the top
              // so it does not fight the left border. Over the panel colour
              // rather than replacing it: a gradient in the BoxDecoration above
              // would take the place of `t.panel2` outright.
              child: AnimatedContainer(
                duration: Motion.wash,
                curve: Motion.ease,
                decoration: artWash(
                  controller.accent,
                  alt: controller.accentAlt,
                  from: Alignment.topCenter,
                ),
                child: Padding(
                padding: const EdgeInsets.all(18),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    _Tabs(controller: controller),
                    const SizedBox(height: 12),
                    Expanded(child: _body(controller, which)),
                  ],
                ),
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

/// What the panel holds, which is a question about the section as much as
/// about the chip: a podcast has a queue and show notes where an album has a
/// queue and words, and a book has neither — it has bookmarks.
Widget _body(MusicController controller, String which) {
  switch (controller.view) {
    case 'podcasts':
      return which == 'lyrics'
          ? _Notes(controller: controller)
          : _PodQueue(controller: controller);
    case 'audiobooks':
      return _BookPanel(controller: controller);
    default:
      return which == 'lyrics'
          ? _Lyrics(controller: controller)
          : _Queue(controller: controller);
  }
}

class _Tabs extends StatelessWidget {
  const _Tabs({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final which = controller.panel;
    final pod = controller.view == 'podcasts';
    final book = controller.view == 'audiobooks';
    // The two chips share the row rather than huddling at its left end: they
    // are this panel's tabs, and a tab that is 90px wide in a 432px column
    // reads as a button someone forgot to lay out. A book has one view, not
    // two, so its row is one chip wide.
    return Row(
      children: [
        Expanded(
          child: MusicChip(
            icon: book ? Icons.bookmarks_outlined : Icons.queue_music,
            label: book ? 'Bookmarks' : 'Queue',
            active: which == 'queue' || book,
            tint: const Color(0xFF3B82F6),
            tint2: const Color(0xFF6366F1),
            expand: true,
            onTap: () => controller.showPanel('queue'),
          ),
        ),
        if (!book) ...[
          const SizedBox(width: 8),
          Expanded(
            child: MusicChip(
              icon: pod ? Icons.notes_outlined : Icons.lyrics_outlined,
              label: pod ? 'Notes' : 'Lyrics',
              active: which == 'lyrics',
              tint: const Color(0xFFEC4899),
              tint2: const Color(0xFF8B5CF6),
              expand: true,
              onTap: () => controller.showPanel('lyrics'),
            ),
          ),
        ],
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
    // YouTube keeps its own queue: video ids, not library tracks.
    if (controller.now?.mode == 'youtube' &&
        (controller.state?.ytQueue.isNotEmpty ?? false)) {
      return _YtQueue(controller: controller);
    }
    final queue = controller.state?.queue ?? const <Track>[];
    if (queue.isEmpty) {
      return Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          const MusicEmpty(
            icon: Icons.queue_music_outlined,
            title: 'The queue is empty',
            body: 'Play an album, a playlist or a folder and it lands here.',
          ),
          const SizedBox(height: 12),
          // An empty queue is exactly where "keep going" is worth offering. It
          // builds a run from the last few things played, so it needs something
          // to have been played -- the command says so itself when nothing has.
          OutlinedButton.icon(
            onPressed: () => controller.send(const MusicCmd.stationStart()),
            icon: const Icon(Icons.auto_awesome, size: 16),
            label: const Text('Keep playing something like this'),
          ),
        ],
      );
    }
    // Tracks nobody chose. The station marks its own in `play_queue.source`,
    // so they can be taken back out without touching anything queued by hand.
    final suggested = controller.state?.queueSuggested ?? 0;
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
            if (suggested > 0) ...[
              const SizedBox(width: 8),
              Tooltip(
                message: '$suggested suggested — tap to remove them',
                child: InkWell(
                  borderRadius: BorderRadius.circular(20),
                  onTap: () =>
                      controller.send(const MusicCmd.stationStop()),
                  child: Container(
                    padding: const EdgeInsets.symmetric(
                        horizontal: 9, vertical: 3),
                    decoration: BoxDecoration(
                      color: Tokens.secMusic.withValues(alpha: 0.16),
                      borderRadius: BorderRadius.circular(20),
                    ),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        const Icon(Icons.auto_awesome,
                            size: 12, color: Tokens.secMusic),
                        const SizedBox(width: 5),
                        Text(
                          '$suggested suggested',
                          style: const TextStyle(
                            fontFamily: Tokens.fontFamily,
                            fontSize: 11,
                            fontWeight: FontWeight.w700,
                            color: Tokens.secMusic,
                          ),
                        ),
                        const SizedBox(width: 4),
                        const Icon(Icons.close,
                            size: 12, color: Tokens.secMusic),
                      ],
                    ),
                  ),
                ),
              ),
            ],
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

/// The YouTube queue: what "Play all", Play next and Add to queue built.
/// Rows before the one playing are what already played; the playing one
/// cannot be removed here, which is Next's job.
class _YtQueue extends StatelessWidget {
  const _YtQueue({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = controller.state!;
    final queue = st.ytQueue;
    final at = st.ytQueuePos.toInt();
    return Column(
      children: [
        Row(
          children: [
            Text(
              'YouTube queue · ${at + 1} of ${queue.length}',
              style: TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: 13,
                fontWeight: FontWeight.w700,
                color: t.nInk2,
              ),
            ),
            const Spacer(),
            TextButton(
              onPressed: queue.length > 1
                  ? () => controller.send(const MusicCmd.ytQueueClear())
                  : null,
              child: const Text('Clear'),
            ),
          ],
        ),
        Expanded(
          child: ReorderableListView.builder(
            buildDefaultDragHandles: true,
            padding: const EdgeInsets.symmetric(vertical: 4),
            itemCount: queue.length,
            onReorderItem: (from, to) =>
                controller.send(MusicCmd.ytQueueMove(from: from, to: to)),
            itemBuilder: (_, i) {
              final v = queue[i];
              final current = i == at;
              return Padding(
                key: ValueKey('yt-$i-${v.videoId}'),
                padding: const EdgeInsets.only(bottom: 6),
                child: Material(
                  color: current
                      ? ytRose.withValues(alpha: 0.12)
                      : Colors.transparent,
                  borderRadius: BorderRadius.circular(Tokens.radiusSm),
                  child: InkWell(
                    borderRadius: BorderRadius.circular(Tokens.radiusSm),
                    onTap: current
                        ? null
                        : () => controller
                            .send(MusicCmd.ytQueuePlayAt(index: i)),
                    child: Padding(
                      padding: const EdgeInsets.fromLTRB(6, 4, 28, 4),
                      child: Row(
                        children: [
                          SizedBox(
                            width: 80,
                            height: 45,
                            child: YtThumb(controller: controller, video: v),
                          ),
                          const SizedBox(width: 10),
                          Expanded(
                            child: Column(
                              crossAxisAlignment: CrossAxisAlignment.start,
                              children: [
                                Text(v.title,
                                    maxLines: 2,
                                    overflow: TextOverflow.ellipsis,
                                    style: TextStyle(
                                        fontSize: 12,
                                        fontWeight: FontWeight.w600,
                                        color: current
                                            ? ytRose
                                            : i < at
                                                ? t.nInk3
                                                : t.nInk)),
                                Text(v.channel,
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                    style: TextStyle(
                                        fontSize: 11, color: t.nInk3)),
                              ],
                            ),
                          ),
                          if (!current)
                            IconButton(
                              tooltip: 'Remove',
                              iconSize: 16,
                              icon: const Icon(Icons.close),
                              onPressed: () => controller
                                  .send(MusicCmd.ytQueueRemove(index: i)),
                            ),
                        ],
                      ),
                    ),
                  ),
                ),
              );
            },
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
            const SizedBox(width: 8),
            // The other direction: not "find the words for this song" but
            // "find the song with these words". Every stored lyric sheet is
            // searchable and nothing has ever searched them.
            MusicChip(
              icon: Icons.travel_explore,
              label: 'Search words',
              active: false,
              onTap: () => searchLyrics(context, controller),
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
              // to centre on, so it scrolls. It is also the one state where
              // timing them by hand is worth offering — there are words, and
              // nothing has put times on them.
              (true, false) => Column(
                  children: [
                    Padding(
                      padding: const EdgeInsets.only(bottom: 8),
                      child: MusicChip(
                        icon: Icons.timer_outlined,
                        label: 'Time these words',
                        active: false,
                        tint: const Color(0xFFEC4899),
                        tint2: const Color(0xFF8B5CF6),
                        onTap: () => timeLyrics(context, controller),
                      ),
                    ),
                    Expanded(
                      child: SingleChildScrollView(
                        child: Text(
                          plain,
                          textAlign: TextAlign.center,
                          style: TextStyle(fontSize: 15, color: t.nInk2),
                        ),
                      ),
                    ),
                  ],
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
                          // Only the live line needs it, and only when the
                          // sheet is timed at all.
                          progress: off == 0
                              ? _through(controller, lines, active)
                              : 0.0,
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

  /// How far through the live line the playhead is, 0..1.
  ///
  /// The line's own span, so a held note does not race and a quick line does
  /// not crawl. Returns 0 when there is no next line to measure against — the
  /// last line of a song has no end but the end of the track, and lighting it
  /// up over four minutes of outro would be worse than not lighting it at all.
  static double _through(
    MusicController controller,
    List<LyricLine> lines,
    int active,
  ) {
    if (active < 0 || active + 1 >= lines.length) return 0.0;
    final start = lines[active].atMs;
    final end = lines[active + 1].atMs;
    if (end <= start) return 0.0;
    final st = controller.state;
    // The same offset the nudge buttons set: the highlight has to move with
    // the line, not against it.
    final at = controller.tickPos * 1000 - (st?.lyricsOffsetMs ?? 0);
    return ((at - start) / (end - start)).clamp(0.0, 1.0);
  }
}

class _Line extends StatelessWidget {
  const _Line({
    required this.text,
    required this.off,
    this.progress = 0.0,
  });

  final String text;
  final int off;

  /// How far through this line the playhead is, 0..1. Only meaningful on the
  /// live line; every other slot passes 0.
  final double progress;

  /// The live line, with the words already sung lit and the rest waiting.
  ///
  /// Spread evenly across the line's span, which is a guess: words are not
  /// evenly spaced and only enhanced LRC carries per-word stamps, which almost
  /// nothing in the wild does. Weighted by word length so a long word holds the
  /// beam longer than "a", which is as close as an even split can get. It reads
  /// as following the singer; it is not a transcript-grade alignment, and a
  /// line that lands half a word out is what the offset nudge is for.
  Widget _sung(BuildContext context, TextStyle style) {
    final t = context.tokens;
    final words = text.split(' ');
    final weights =
        words.map((w) => w.trim().isEmpty ? 1 : w.characters.length).toList();
    final total = weights.fold<int>(0, (a, b) => a + b);
    if (total == 0) return Text(text, textAlign: TextAlign.center);

    final lit = progress * total;
    var walked = 0;
    final spans = <TextSpan>[];
    for (var i = 0; i < words.length; i++) {
      walked += weights[i];
      // A word counts as sung once the beam has passed its middle, so it
      // lights while it is being said rather than after.
      final done = lit >= walked - weights[i] / 2;
      spans.add(TextSpan(
        text: i == words.length - 1 ? words[i] : '${words[i]} ',
        style: TextStyle(
          color: done ? Tokens.secMusic : t.nInk2.withValues(alpha: 0.55),
        ),
      ));
    }
    return Text.rich(TextSpan(style: style, children: spans),
        textAlign: TextAlign.center);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final d = off.abs();
    if (off == 0 && progress > 0 && !t.reduceMotion && text.isNotEmpty) {
      return _sung(
        context,
        const TextStyle(
          fontFamily: Tokens.fontFamily,
          fontSize: 24,
          fontWeight: FontWeight.w800,
        ),
      );
    }
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

// ------------------------------------------------- podcasts: the queue -----

/// The episode queue, docked.
///
/// It used to be a dialog off the sub-tab row, which is the wrong shape for a
/// list you reorder while something is playing: the dialog covered the row it
/// was launched from, and closing it to reach the transport lost your place.
class _PodQueue extends StatelessWidget {
  const _PodQueue({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = controller.state;
    final queue = st?.podQueue ?? const <Episode>[];
    final left = queue.fold<double>(
        0, (n, e) => n + (e.durationS - e.positionS).clamp(0.0, e.durationS));
    return Column(
      children: [
        Row(
          children: [
            Text(
              queue.isEmpty
                  ? 'Up next'
                  : 'Up next · ${queue.length} · ${fmtMins(left)}',
              style: TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: 13,
                fontWeight: FontWeight.w700,
                color: t.nInk2,
              ),
            ),
            const Spacer(),
            if (queue.isNotEmpty)
              TextButton(
                onPressed: () =>
                    controller.send(const MusicCmd.podQueueClear()),
                child: const Text('Clear'),
              ),
          ],
        ),
        Expanded(
          child: queue.isEmpty
              ? const MusicEmpty(
                  icon: Icons.queue_music_outlined,
                  title: 'Nothing queued',
                  body: 'Use Play next or Queue on any episode, or Queue all '
                      'on the New tab.',
                )
              : ReorderableListView.builder(
                  buildDefaultDragHandles: true,
                  padding: const EdgeInsets.symmetric(vertical: 4),
                  itemCount: queue.length,
                  // onReorderItem, unlike the deprecated onReorder, already
                  // accounts for the lifted row.
                  onReorderItem: (from, to) =>
                      controller.send(MusicCmd.podQueueMove(from: from, to: to)),
                  itemBuilder: (_, i) => Padding(
                    key: ValueKey(queue[i].id),
                    padding: const EdgeInsets.only(bottom: 6),
                    child: _QueueRow(
                      controller: controller,
                      episode: queue[i],
                    ),
                  ),
                ),
        ),
        Padding(
          padding: const EdgeInsets.only(top: 8),
          child: Row(
            children: [
              Transform.scale(
                scale: 0.7,
                child: Switch(
                  value: st?.podQueueAuto ?? false,
                  onChanged: (v) =>
                      controller.send(MusicCmd.podSetQueueAuto(auto: v)),
                ),
              ),
              Expanded(
                child: Text(
                  'Queue new episodes of pinned shows',
                  style: TextStyle(fontSize: 11.5, color: t.nInk2),
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

class _QueueRow extends StatelessWidget {
  const _QueueRow({required this.controller, required this.episode});

  final MusicController controller;
  final Episode episode;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final e = episode;
    final now = controller.now?.mode == 'podcast' &&
        controller.now?.key == '${e.id}';
    return Material(
      color: now ? kPodTint.withValues(alpha: 0.12) : t.nCard,
      borderRadius: BorderRadius.circular(Tokens.radiusMd),
      child: InkWell(
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        onTap: () => controller.send(MusicCmd.podPlay(episodeId: e.id)),
        child: Padding(
          padding: const EdgeInsets.all(8),
          child: Row(
            children: [
              MusicArt(
                controller: controller,
                kind: 'podcast',
                artKey: e.art,
                size: 40,
                radius: 8,
                fallback: Icons.podcasts,
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(e.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 12,
                            fontWeight: FontWeight.w600,
                            color: now ? kPodTint : t.nInk)),
                    Text(
                      '${e.show_} · ${fmtMins(e.durationS - e.positionS)}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 10.5, color: t.nInk2),
                    ),
                  ],
                ),
              ),
              IconButton(
                iconSize: 15,
                tooltip: 'Remove',
                icon: const Icon(Icons.close),
                onPressed: () =>
                    controller.send(MusicCmd.podQueueRemove(episodeId: e.id)),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Show notes, beside the list rather than over it. A feed's notes are the only
/// place a chapter list or a link ever appears, and they are read while the
/// episode plays — which is why they are not a dialog any more.
class _Notes extends StatelessWidget {
  const _Notes({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = controller.state;
    if (st == null || st.podTranscriptTitle.isEmpty) {
      return const MusicEmpty(
        icon: Icons.notes_outlined,
        title: 'No notes open',
        body: 'The notes button on any episode row opens what its feed '
            'carries — a summary, a chapter list, the links mentioned.',
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Expanded(
              child: Text(st.podTranscriptTitle,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
            ),
            IconButton(
              iconSize: 16,
              tooltip: 'Close the notes',
              icon: const Icon(Icons.close),
              onPressed: () =>
                  controller.send(const MusicCmd.podTranscriptClose()),
            ),
          ],
        ),
        const SizedBox(height: 8),
        Expanded(
          child: SingleChildScrollView(
            child: SelectableText(
              st.podTranscriptText.isEmpty
                  ? 'This episode carries no notes.'
                  : st.podTranscriptText,
              style: TextStyle(fontSize: 12, height: 1.6, color: t.nInk2),
            ),
          ),
        ),
      ],
    );
  }
}

// ----------------------------------------------- audiobooks: bookmarks -----

/// The book half of the panel: where you stand, what you marked, and the four
/// switches that are about your ears rather than about the book.
class _BookPanel extends StatelessWidget {
  const _BookPanel({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = controller.state;
    if (st == null) return const SizedBox.shrink();
    final book = st.bookDetail;
    final marks = st.bookBookmarks;
    final stats = st.bookStats;
    return ListView(
      padding: EdgeInsets.zero,
      children: [
        if (book != null)
          Padding(
            padding: const EdgeInsets.only(bottom: 10),
            child: Text(book.title,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 13,
                    fontWeight: FontWeight.w700,
                    color: t.nInk)),
          ),
        Container(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 12),
          decoration: BoxDecoration(
            color: t.nCard,
            borderRadius: BorderRadius.circular(Tokens.radiusMd),
            border: Border.all(color: t.nHair),
          ),
          child: Wrap(
            spacing: 18,
            runSpacing: 10,
            children: [
              _Fig(
                value: fmtMins(stats.leftS),
                label: book == null
                    ? 'left'
                    : 'left at ${book.speed > 0 ? book.speed : st.bookSpeed}×',
              ),
              _Fig(value: fmtMins(stats.weekS), label: 'this week'),
              _Fig(
                  value: '${stats.streakDays} '
                      'day${stats.streakDays == 1 ? '' : 's'}',
                  label: 'streak'),
              _Fig(
                  value: '${stats.finishedYear}', label: 'finished this year'),
            ],
          ),
        ),
        const SizedBox(height: 14),
        Row(
          children: [
            Text('Bookmarks',
                style: TextStyle(
                    fontSize: 13,
                    fontWeight: FontWeight.w700,
                    color: t.nInk)),
            const SizedBox(width: 8),
            Text('${marks.length}',
                style: TextStyle(fontSize: 11, color: t.nInk3)),
            const Spacer(),
            TextButton.icon(
              icon: const Icon(Icons.add, size: 15),
              label: const Text('Add'),
              onPressed: controller.now?.mode == 'book'
                  ? () => addBookmark(context, controller)
                  : null,
            ),
          ],
        ),
        if (marks.isEmpty)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 6),
            child: Text(
              'Nothing marked yet. While a book is playing, B saves the exact '
              'second — and a note says why.',
              style: TextStyle(fontSize: 11.5, color: t.nInk2),
            ),
          ),
        for (var i = 0; i < marks.length; i++)
          _BookmarkRow(controller: controller, index: i, mark: marks[i]),
        const SizedBox(height: 16),
        Text('This book',
            style: TextStyle(
                fontSize: 13, fontWeight: FontWeight.w700, color: t.nInk)),
        const SizedBox(height: 8),
        _BookSettings(controller: controller, st: st),
        const SizedBox(height: 24),
      ],
    );
  }
}

class _Fig extends StatelessWidget {
  const _Fig({required this.value, required this.label});

  final String value;
  final String label;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(value,
            style: TextStyle(
                fontSize: 14, fontWeight: FontWeight.w700, color: t.nInk)),
        Text(label, style: TextStyle(fontSize: 10.5, color: t.nInk3)),
      ],
    );
  }
}

class _BookmarkRow extends StatelessWidget {
  const _BookmarkRow({
    required this.controller,
    required this.index,
    required this.mark,
  });

  final MusicController controller;
  final int index;
  final Bookmark mark;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      margin: const EdgeInsets.only(bottom: 6),
      padding: const EdgeInsets.fromLTRB(10, 8, 6, 8),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        border: Border.all(color: t.nHair),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              InkWell(
                borderRadius: BorderRadius.circular(4),
                onTap: () =>
                    controller.send(MusicCmd.bookmarkJump(index: index)),
                child: Padding(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 4, vertical: 2),
                  child: Text(mark.when,
                      style: const TextStyle(
                          fontSize: 11,
                          fontWeight: FontWeight.w700,
                          color: kBookTint)),
                ),
              ),
              const Spacer(),
              IconButton(
                iconSize: 15,
                tooltip: 'Play from here',
                icon: const Icon(Icons.play_arrow),
                onPressed: () =>
                    controller.send(MusicCmd.bookmarkJump(index: index)),
              ),
              IconButton(
                iconSize: 15,
                tooltip: 'Rename, or add a note',
                icon: const Icon(Icons.edit_outlined),
                onPressed: () =>
                    editBookmark(context, controller, index, mark),
              ),
              IconButton(
                iconSize: 15,
                tooltip: 'Remove',
                icon: const Icon(Icons.close),
                onPressed: () =>
                    controller.send(MusicCmd.bookmarkRemove(index: index)),
              ),
            ],
          ),
          if (mark.label.isNotEmpty)
            Text(mark.label,
                style: TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                    color: t.nInk)),
          if (mark.note.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(top: 2),
              child: Text(mark.note,
                  style: TextStyle(
                      fontSize: 11, height: 1.4, color: t.nInk2)),
            ),
        ],
      ),
    );
  }
}

class _BookSettings extends StatelessWidget {
  const _BookSettings({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    void save({int? skip, bool? rewind, bool? trim, bool? boost}) =>
        controller.send(MusicCmd.bookSettings(
          skipS: skip ?? st.bookSkipS,
          rewindPause: rewind ?? st.bookRewindPause,
          trimSilence: trim ?? st.bookTrimSilence,
          boostVoices: boost ?? st.bookBoostVoices,
        ));
    Widget sw(String label, bool value, ValueChanged<bool> onChanged) => Row(
          children: [
            Transform.scale(
              scale: 0.7,
              child: Switch(value: value, onChanged: onChanged),
            ),
            Expanded(
              child: Text(label,
                  style: TextStyle(fontSize: 11.5, color: t.nInk2)),
            ),
          ],
        );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Text('Skip back and forward',
                style: TextStyle(fontSize: 11.5, color: t.nInk3)),
            const SizedBox(width: 10),
            for (final v in const [10, 30, 60])
              Padding(
                padding: const EdgeInsets.only(right: 6),
                child: SortChip(
                  label: '$v s',
                  active: st.bookSkipS == v,
                  onTap: () => save(skip: v),
                ),
              ),
          ],
        ),
        const SizedBox(height: 4),
        sw('Rewind 5 s after a pause', st.bookRewindPause,
            (v) => save(rewind: v)),
        sw('Trim silence', st.bookTrimSilence, (v) => save(trim: v)),
        sw('Boost voices', st.bookBoostVoices, (v) => save(boost: v)),
      ],
    );
  }
}
