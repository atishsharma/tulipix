// Stream — a time-ordered feed of what actually happened, and a standing rail
// of what is true right now.
//
// The feed is merged in Rust from the eight section databases plus the app's
// activity log and capped; this page only draws it. The rail carries the
// player, the money, the transfer door and the library counters.

import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/home.dart';
import 'home_controller.dart';
import 'home_player.dart';
import 'home_shared.dart';

class StreamHome extends StatefulWidget {
  const StreamHome({
    super.key,
    required this.controller,
    required this.state,
  });

  final HomeController controller;
  final HomeState state;

  @override
  State<StreamHome> createState() => _StreamHomeState();
}

class _StreamHomeState extends State<StreamHome> {
  /// Which bar STANDING is reading. −1 means "the newest", which is the month
  /// `finSpent` / `finMonth` already describe.
  int _monSel = -1;

  bool _on(String card) => widget.state.cards.contains(card);

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.state;
    return LayoutBuilder(
      builder: (context, box) {
        // ── Geometry, from ui/page_home_stream.slint ───────────────────────
        const pad = 28.0;
        const headH = 74.0;
        const titleH = 46.0;
        const barH = 56.0;
        final railN = (_on('player') ? 1 : 0) +
            (_on('finances') ? 1 : 0) +
            (_on('transfer') ? 1 : 0) +
            (_on('library') ? 1 : 0);
        // The rail keeps its width until every block is off, then it disappears
        // and the feed takes the whole page.
        final railW = railN > 0 ? 396.0 : 0.0;
        final feedW = box.maxWidth - 2 * pad - railW - (railW > 0 ? pad : 0);

        return Stack(
          children: [
            // ── Header ────────────────────────────────────────────────────
            Positioned(
              left: pad,
              top: 20,
              width: box.maxWidth - 2 * pad,
              height: 34,
              child: Row(
                children: [
                  const AppMark(),
                  const SizedBox(width: 8),
                  Text('Tulipix',
                      style: TextStyle(
                          fontSize: 15,
                          fontWeight: FontWeight.w700,
                          color: t.text)),
                  const SizedBox(width: 10),
                  // Black plate, white ink — the same pill the group captions
                  // in the feed wear, inverted.
                  Container(
                    height: 26,
                    padding: const EdgeInsets.symmetric(horizontal: 13),
                    alignment: Alignment.center,
                    decoration: BoxDecoration(
                      color: Colors.black,
                      borderRadius: BorderRadius.circular(13),
                    ),
                    child: Text(st.greeting,
                        style: const TextStyle(
                            fontSize: 11.5,
                            fontWeight: FontWeight.w700,
                            color: Colors.white)),
                  ),
                  const SizedBox(width: 10),
                  if (_on('quick')) const Expanded(child: LauncherPills()),
                ],
              ),
            ),

            // ── Feed title ────────────────────────────────────────────────
            // Names the column the way the rail's captions name their blocks.
            Positioned(
              left: pad,
              top: headH,
              width: feedW,
              height: 32,
              child: Center(
                child: Container(
                  height: 32,
                  padding: const EdgeInsets.symmetric(horizontal: 17),
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: Colors.white,
                    borderRadius: BorderRadius.circular(16),
                  ),
                  child: const Text('Tulipix : Events Timeline',
                      style: TextStyle(
                          fontSize: 17,
                          fontWeight: FontWeight.w800,
                          letterSpacing: -0.2,
                          color: Colors.black)),
                ),
              ),
            ),

            // ── Feed ──────────────────────────────────────────────────────
            Positioned(
              left: pad,
              top: headH + titleH,
              width: feedW,
              height: math.max(0, box.maxHeight - headH - titleH - pad),
              child: st.events.isEmpty
                  ? const _EmptyFeed()
                  : ListView.builder(
                      // The 3% a side is the LIST's, so the row plates
                      // themselves get narrower.
                      padding: EdgeInsets.fromLTRB(
                          feedW * 0.03, 0, feedW * 0.03, barH + 26),
                      itemCount:
                          st.events.length + (st.feedNote.isEmpty ? 0 : 1),
                      itemBuilder: (context, i) {
                        if (st.feedNote.isNotEmpty && i == 0) {
                          return SizedBox(
                            height: 34,
                            child: Align(
                              alignment: Alignment.centerLeft,
                              child: Text(st.feedNote,
                                  style: TextStyle(
                                      fontSize: 11, color: t.textDim)),
                            ),
                          );
                        }
                        final k = st.feedNote.isEmpty ? i : i - 1;
                        return _FeedRow(
                          event: st.events[k],
                          rowW: feedW * 0.94,
                          last: k == st.events.length - 1,
                          onAction: () => ShellController.instance
                              .go(sectionOf(st.events[k].section)),
                        );
                      },
                    ),
            ),

            // ── Floating filter bar ───────────────────────────────────────
            // The chips left the header for the bottom of the feed column,
            // where a hand already is.
            Positioned(
              left: pad,
              top: box.maxHeight - pad - barH,
              width: feedW,
              height: barH,
              child: Center(
                child: Container(
                  height: barH,
                  padding: const EdgeInsets.symmetric(horizontal: 10),
                  decoration: BoxDecoration(
                    color: t.dark
                        ? const Color(0xEB14141C)
                        : const Color(0xEEFFFFFF),
                    borderRadius: BorderRadius.circular(barH / 2),
                    border: Border.all(color: t.glassBorder),
                    boxShadow: [
                      BoxShadow(
                          color: t.dark
                              ? const Color(0x99000000)
                              : const Color(0x2E000000),
                          blurRadius: 26,
                          offset: const Offset(0, 6)),
                    ],
                  ),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      _FeedChip(
                        label: 'Everything',
                        icon: Icons.grid_view_outlined,
                        accent: Tokens.brand,
                        active: st.feedFilter == 'all',
                        onTap: () => widget.controller
                            .send(const HomeCmd.setFeedFilter(filter: 'all')),
                      ),
                      if (_on('photos') ||
                          _on('videos') ||
                          _on('music') ||
                          _on('books'))
                        _FeedChip(
                          label: 'Media',
                          icon: Icons.movie_outlined,
                          accent: Tokens.secVideos,
                          active: st.feedFilter == 'media',
                          onTap: () => widget.controller.send(
                              const HomeCmd.setFeedFilter(filter: 'media')),
                        ),
                      if (_on('finances'))
                        _FeedChip(
                          label: 'Money',
                          icon: Icons.account_balance_wallet_outlined,
                          accent: Tokens.secFinances,
                          active: st.feedFilter == 'money',
                          onTap: () => widget.controller.send(
                              const HomeCmd.setFeedFilter(filter: 'money')),
                        ),
                      if (_on('transfer') || _on('cloud') || _on('tools'))
                        _FeedChip(
                          label: 'Transfers',
                          icon: Icons.share_outlined,
                          accent: Tokens.secTransfer,
                          active: st.feedFilter == 'devices',
                          onTap: () => widget.controller.send(
                              const HomeCmd.setFeedFilter(filter: 'devices')),
                        ),
                    ],
                  ),
                ),
              ),
            ),

            // ── Rail ──────────────────────────────────────────────────────
            if (railN > 0)
              Positioned(
                left: box.maxWidth - pad - railW,
                top: headH,
                width: railW,
                height: math.max(0, box.maxHeight - headH - pad),
                child: Stack(
                  children: [
                    // The 1px rule that separates the rail from the feed.
                    Positioned(
                      left: -pad + 2,
                      top: 0,
                      bottom: 0,
                      width: 1,
                      child: ColoredBox(color: t.outline),
                    ),
                    // Scrolls: the queue drop-down and a long dues list can
                    // together outrun a short window.
                    ListView(
                      padding: const EdgeInsets.only(bottom: 16),
                      children: [
                        if (_on('player')) ...[
                          StreamRailPlayer(railWidth: railW),
                          const SizedBox(height: 14),
                        ],
                        if (_on('finances')) ...[
                          _Standing(
                            state: st,
                            selected: _monSel,
                            onPick: (i) => setState(() => _monSel = i),
                          ),
                          const SizedBox(height: 14),
                        ],
                        if (_on('transfer')) ...[
                          _Transfers(inbox: st.transferInbox),
                          const SizedBox(height: 14),
                        ],
                        if (_on('library')) _Library(state: st),
                      ],
                    ),
                  ],
                ),
              ),
          ],
        );
      },
    );
  }
}

// ── Empty is a state with something to say, not a blank column ──────────────

class _EmptyFeed extends StatelessWidget {
  const _EmptyFeed();

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const SizedBox(height: 40),
        Text('Nothing recorded yet',
            style: TextStyle(
                fontSize: 17, fontWeight: FontWeight.w700, color: t.text)),
        const SizedBox(height: 8),
        Text(
            'Play something, import a folder, or send a file — this column is '
            'the record of what the app did.',
            style: TextStyle(fontSize: 12, color: t.textDim)),
      ],
    );
  }
}

// ── One feed row ────────────────────────────────────────────────────────────

/// Four columns, 15 : 45 : 25 : 15 of the row — the feed reads as a table
/// rather than a queue of free-floating parts.
class _FeedRow extends StatelessWidget {
  const _FeedRow({
    required this.event,
    required this.rowW,
    required this.last,
    required this.onAction,
  });

  final HomeEvent event;
  final double rowW;
  final bool last;
  final VoidCallback onAction;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = event.alarm ? Tokens.error : sectionAccent(event.section);
    final col1 = rowW * 0.15;
    final col2 = rowW * 0.45;
    final col3 = rowW * 0.25;
    final col4 = rowW * 0.15;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // The group caption, as a solid white pill with black ink — the day is
        // a marker, not another dim line.
        if (event.head)
          SizedBox(
            height: 34,
            child: Align(
              alignment: Alignment.bottomCenter,
              child: Padding(
                padding: const EdgeInsets.only(bottom: 9),
                child: Container(
                  height: 20,
                  padding: const EdgeInsets.symmetric(horizontal: 11),
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: Colors.white,
                    borderRadius: BorderRadius.circular(10),
                  ),
                  child: Text(event.group,
                      style: const TextStyle(
                          fontSize: 9.5,
                          fontWeight: FontWeight.w800,
                          letterSpacing: 1.5,
                          color: Colors.black)),
                ),
              ),
            ),
          ),
        Hover(
          onTap: () => ShellController.instance.go(sectionOf(event.section)),
          builder: (context, hov) {
            final washAmt =
                hov ? (t.dark ? 0.30 : 0.22) : (t.dark ? 0.16 : 0.11);
            // Mixed into the page colour rather than laid over it with alpha,
            // so the fill is a solid the dot's ring can borrow.
            final fill = mix(t.bg, tint, 1 - washAmt);
            return AnimatedContainer(
              duration: const Duration(milliseconds: 110),
              height: 66,
              decoration: BoxDecoration(
                color: fill,
                borderRadius: BorderRadius.circular(12),
              ),
              // Stretch, so every column is the row's full 66px: the
              // timeline's rule and its dot live in column one and are
              // positioned against that height.
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  // ── 1 · when ────────────────────────────────────────────
                  SizedBox(
                    width: col1,
                    child: Stack(
                      children: [
                        // The line runs the full row height so consecutive
                        // rows join; the last row has no tail to draw.
                        if (!last)
                          Positioned(
                            left: col1 - 13,
                            top: 0,
                            bottom: 0,
                            width: 1,
                            child: ColoredBox(color: t.outline),
                          ),
                        Positioned(
                          left: col1 - 17,
                          top: 0,
                          bottom: 0,
                          child: Center(
                            child: Container(
                              width: 9,
                              height: 9,
                              decoration: BoxDecoration(
                                color: tint,
                                shape: BoxShape.circle,
                                // Ring in the ROW's own fill, so the dot sits
                                // on the line rather than being crossed by it.
                                border: Border.all(color: fill, width: 4),
                              ),
                            ),
                          ),
                        ),
                        SizedBox(
                          width: math.max(0, col1 - 30),
                          child: Column(
                            mainAxisAlignment: MainAxisAlignment.center,
                            children: [
                              Text(event.at,
                                  style: TextStyle(
                                      fontSize: 12,
                                      fontWeight: FontWeight.w600,
                                      color: t.text)),
                              const SizedBox(height: 2),
                              Text(event.day,
                                  style: TextStyle(
                                      fontSize: 9.5, color: t.textDim)),
                            ],
                          ),
                        ),
                      ],
                    ),
                  ),
                  // ── 2 · what — the only left-aligned column ─────────────
                  SizedBox(
                    width: col2,
                    child: Padding(
                      padding: const EdgeInsets.only(left: 12, right: 8),
                      child: Column(
                        mainAxisAlignment: MainAxisAlignment.center,
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(event.title,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                  fontSize: 13,
                                  fontWeight: FontWeight.w600,
                                  color: t.text)),
                          const SizedBox(height: 3),
                          Text(event.sub,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style:
                                  TextStyle(fontSize: 10.5, color: t.textDim)),
                        ],
                      ),
                    ),
                  ),
                  // ── 3 · the things you can do with it ──────────────────
                  SizedBox(
                    width: col3,
                    child: Row(
                      mainAxisAlignment: MainAxisAlignment.center,
                      children: [
                        if (event.id >= 0 &&
                            sectionOf(event.section) != Section.home) ...[
                          ClipRRect(
                            borderRadius: BorderRadius.circular(6),
                            child: SizedBox(
                              width: 54,
                              height: 40,
                              child: LazyCover(
                                section: sectionOf(event.section),
                                id: event.id,
                                tint: tint,
                                icon: sectionIcon(event.section),
                                iconSize: 14,
                              ),
                            ),
                          ),
                          const SizedBox(width: 6),
                        ],
                        if (event.action.isNotEmpty)
                          Flexible(
                            child: _ActionPill(
                              label: event.action,
                              alarm: event.alarm,
                              accent: sectionAccent(event.section),
                              onTap: onAction,
                            ),
                          ),
                      ],
                    ),
                  ),
                  // ── 4 · where it came from ─────────────────────────────
                  // The glyph at rest; hovering the row widens it into a named
                  // pill. One element, so the name can never land on top of the
                  // icon it names.
                  SizedBox(
                    width: col4,
                    child: Center(
                      child: AnimatedContainer(
                        duration: const Duration(milliseconds: 140),
                        curve: Curves.easeOut,
                        height: 34,
                        padding: EdgeInsets.symmetric(horizontal: hov ? 10 : 8),
                        decoration: BoxDecoration(
                          color: hov
                              ? tint
                              : tint.withValues(alpha: t.dark ? 0.30 : 0.20),
                          borderRadius: BorderRadius.circular(11),
                        ),
                        child: Row(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            Icon(sectionIcon(event.section),
                                size: 16, color: hov ? Colors.white : tint),
                            if (hov) ...[
                              const SizedBox(width: 6),
                              Text(sectionName(event.section),
                                  style: const TextStyle(
                                      fontSize: 10,
                                      fontWeight: FontWeight.w700,
                                      letterSpacing: 0.4,
                                      color: Colors.white)),
                            ],
                          ],
                        ),
                      ),
                    ),
                  ),
                ],
              ),
            );
          },
        ),
        // Filled rows need air between them, not a hairline.
        const SizedBox(height: 4),
      ],
    );
  }
}

/// Never wraps: "Mark paid" on two lines was the mockup's one real bug.
class _ActionPill extends StatelessWidget {
  const _ActionPill({
    required this.label,
    required this.alarm,
    required this.accent,
    required this.onTap,
  });

  final String label;
  final bool alarm;
  final Color accent;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Hover(
      onTap: onTap,
      builder: (context, hov) => AnimatedContainer(
        duration: const Duration(milliseconds: 110),
        height: 28,
        padding: const EdgeInsets.symmetric(horizontal: 12),
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: hov
              ? (alarm ? Tokens.error : accent).withValues(alpha: 0.22)
              : (alarm ? Tokens.error.withValues(alpha: 0.12) : t.glass),
          borderRadius: BorderRadius.circular(14),
          border: Border.all(
              color:
                  alarm ? Tokens.error.withValues(alpha: 0.5) : t.glassBorder),
        ),
        child: Text(label,
            maxLines: 1,
            softWrap: false,
            overflow: TextOverflow.fade,
            style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w600,
                color: alarm ? Tokens.error : t.text)),
      ),
    );
  }
}

class _FeedChip extends StatelessWidget {
  const _FeedChip({
    required this.label,
    required this.icon,
    required this.accent,
    required this.active,
    required this.onTap,
  });

  final String label;
  final IconData icon;
  final Color accent;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Hover(
      onTap: onTap,
      builder: (context, hov) => AnimatedContainer(
        duration: const Duration(milliseconds: 140),
        height: 40,
        margin: const EdgeInsets.symmetric(horizontal: 1),
        padding: const EdgeInsets.symmetric(horizontal: 15),
        decoration: BoxDecoration(
          color: active ? accent : (hov ? t.glassStrong : Colors.transparent),
          borderRadius: BorderRadius.circular(20),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 15, color: active ? Colors.white : t.textDim),
            const SizedBox(width: 8),
            Text(label,
                style: TextStyle(
                    fontSize: 11.5,
                    fontWeight: active ? FontWeight.w700 : FontWeight.w500,
                    color: active ? Colors.white : t.textDim)),
          ],
        ),
      ),
    );
  }
}

// ── Rail blocks ─────────────────────────────────────────────────────────────

class _RailCap extends StatelessWidget {
  const _RailCap({
    required this.label,
    required this.accent,
    this.note = '',
  });

  final String label;
  final Color accent;
  final String note;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      height: 26,
      child: Row(
        children: [
          Text(label,
              style: TextStyle(
                  fontSize: 10,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 1.5,
                  color: accent)),
          const SizedBox(width: 8),
          Expanded(child: Divider(color: t.outline, height: 1)),
          const SizedBox(width: 8),
          Text(note, style: TextStyle(fontSize: 10, color: t.textDim)),
        ],
      ),
    );
  }
}

/// The money that is true regardless of today's feed. Every one of the twelve
/// bars is a key: picking one swaps the figure and the caption above it.
class _Standing extends StatelessWidget {
  const _Standing({
    required this.state,
    required this.selected,
    required this.onPick,
  });

  final HomeState state;
  final int selected;
  final ValueChanged<int> onPick;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final months = state.finMonths;
    final i = selected >= 0 && selected < months.length
        ? selected
        : months.length - 1;
    final label = i >= 0 && i < state.finMonthLabels.length
        ? state.finMonthLabels[i]
        : state.finMonth;
    final spent = monthSpend(state, i) ?? '—';
    final late = state.finDues.isNotEmpty && state.finDues.first.late_;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // The caption names whichever month the bars are on, not always this
        // one.
        _RailCap(label: 'STANDING', accent: Tokens.secFinances, note: label),
        const SizedBox(height: 8),
        SizedBox(
          height: 46,
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              // The number IS the block — liquid was a second figure competing
              // with it, and a figure Home cannot act on.
              Text(spent,
                  style: TextStyle(
                      fontSize: 34,
                      fontWeight: FontWeight.w800,
                      letterSpacing: -1.2,
                      color: late ? Tokens.error : t.text)),
              const SizedBox(width: 10),
              Padding(
                padding: const EdgeInsets.only(bottom: 6),
                child: Text('spent',
                    style: TextStyle(fontSize: 11, color: t.textDim)),
              ),
            ],
          ),
        ),
        const SizedBox(height: 8),
        SizedBox(
          height: 34,
          child: Row(
            children: [
              for (var k = 0; k < months.length; k++) ...[
                if (k > 0) const SizedBox(width: 3),
                Expanded(
                  // The bar's own hit box is the full column height, not the
                  // drawn height — a lean month is 2px tall and would be
                  // unclickable otherwise.
                  child: Hover(
                    onTap: () => onPick(k),
                    builder: (context, hov) => Align(
                      alignment: Alignment.bottomCenter,
                      child: AnimatedContainer(
                        duration: const Duration(milliseconds: 110),
                        height: math.max(2, 34 * math.max(0.04, months[k])),
                        decoration: BoxDecoration(
                          color: k == selected
                              ? Tokens.secFinances
                              : Tokens.secFinances
                                  .withValues(alpha: hov ? 0.6 : 0.32),
                          borderRadius: BorderRadius.circular(2),
                        ),
                      ),
                    ),
                  ),
                ),
              ],
            ],
          ),
        ),
        for (final d in state.finDues)
          SizedBox(
            height: 28,
            child: Row(
              children: [
                Container(
                  width: 6,
                  height: 6,
                  decoration: BoxDecoration(
                    color: d.late_ ? Tokens.error : Tokens.warn,
                    shape: BoxShape.circle,
                  ),
                ),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(d.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 11.5, color: t.text)),
                ),
                const SizedBox(width: 8),
                Text(d.amount,
                    style: TextStyle(
                        fontSize: 11.5,
                        fontWeight: FontWeight.w600,
                        color: t.text)),
                const SizedBox(width: 8),
                // 76px: DD-MM-YY needs more than the 56 the old "in 3 days"
                // wording did, and it was clipping.
                SizedBox(
                  width: 76,
                  child: Text(d.due,
                      textAlign: TextAlign.right,
                      style: TextStyle(
                          fontSize: 10,
                          color: d.late_ ? Tokens.error : t.textDim)),
                ),
                const SizedBox(width: 4),
              ],
            ),
          ),
      ],
    );
  }
}

/// A door while the section is closed — the server only binds while Transfer is
/// open — so this offers the action and the inbox.
class _Transfers extends StatelessWidget {
  const _Transfers({required this.inbox});

  final String inbox;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const _RailCap(
            label: 'TRANSFERS',
            accent: Tokens.secTransfer,
            note: 'local network'),
        const SizedBox(height: 8),
        SizedBox(
          height: 34,
          child: Row(
            children: [
              Hover(
                onTap: () => ShellController.instance.go(Section.transfer),
                builder: (context, hov) => Container(
                  width: 134,
                  height: 34,
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    gradient: const LinearGradient(
                      begin: Alignment.topLeft,
                      end: Alignment.bottomRight,
                      colors: [Color(0xFFF97316), Color(0xFFFBBF24)],
                    ),
                    borderRadius: BorderRadius.circular(17),
                  ),
                  padding: const EdgeInsets.symmetric(horizontal: 10),
                  child: const Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Icon(Icons.bolt, size: 14, color: Color(0xFF231000)),
                      SizedBox(width: 7),
                      Flexible(
                        child: Text('Start sharing',
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontSize: 11.5,
                                fontWeight: FontWeight.w700,
                                color: Color(0xFF231000))),
                      ),
                    ],
                  ),
                ),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Text(
                    inbox.isEmpty ? 'Inbox not set yet' : 'Inbox $inbox',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 10.5, color: t.textDim)),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// The counter table — the rail's map of the app. All eight sections, and the
/// value is a string so the last ones can print what they actually count.
class _Library extends StatelessWidget {
  const _Library({required this.state});

  final HomeState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = state.counts;
    final rows = <({String label, String value, Section section, Color tint})>[
      (
        label: 'Photos',
        value: '${c.photos}',
        section: Section.photos,
        tint: Tokens.secPhotos
      ),
      (
        label: 'Videos',
        value: '${c.videos}',
        section: Section.videos,
        tint: Tokens.secVideos
      ),
      (
        label: 'Tracks',
        value: '${c.songs}',
        section: Section.music,
        tint: Tokens.secMusic
      ),
      (
        label: 'Books',
        value: '${c.books}',
        section: Section.books,
        tint: Tokens.secBooks
      ),
      (
        label: 'Clouds',
        value: '${c.cloudRemotes}',
        section: Section.cloud,
        tint: Tokens.secCloud
      ),
      (
        label: 'Tools',
        value: '${state.toolCount}',
        section: Section.tools,
        tint: Tokens.secTools
      ),
      (
        label: 'Finances',
        value: state.finSpent.isEmpty ? '—' : state.finSpent,
        section: Section.finances,
        tint: Tokens.secFinances
      ),
    ];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _RailCap(
            label: 'LIBRARY',
            accent: Tokens.secPhotos,
            note: state.libraryLine),
        const SizedBox(height: 6),
        for (final r in rows)
          Hover(
            onTap: () => ShellController.instance.go(r.section),
            builder: (context, hov) => Container(
              height: 26,
              padding: const EdgeInsets.symmetric(horizontal: 4),
              decoration: BoxDecoration(
                color: hov ? t.glass : Colors.transparent,
                borderRadius: BorderRadius.circular(6),
              ),
              child: Row(
                children: [
                  Container(
                    width: 5,
                    height: 5,
                    decoration:
                        BoxDecoration(color: r.tint, shape: BoxShape.circle),
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(r.label,
                        style: TextStyle(fontSize: 11.5, color: t.textDim)),
                  ),
                  Text(r.value,
                      style: TextStyle(
                          fontSize: 11.5,
                          fontWeight: FontWeight.w600,
                          color: t.text)),
                ],
              ),
            ),
          ),
      ],
    );
  }
}
