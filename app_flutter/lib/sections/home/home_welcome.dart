// Welcome — the front door.
//
// A big greeting beside a drawn banner, a bar of the six things you launch by
// name, Continue beside the newest arrivals, the section tiles as "My Hub", and
// the player pinned along the bottom of the page.
//
// Nothing here scrolls: every row's height comes out of the page budget (see
// `free` / `hubH` / `contRowH` / `heroH`) so it all lands on one screen.

import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/home.dart';
import '../music/music_controller.dart';
import 'home_controller.dart';
import 'home_player.dart';
import 'home_shared.dart';

/// The greeting emoji. The list lives here rather than in Rust because the
/// rotation is a UI clock: one every two minutes.
const List<String> kGreetEmojis = [
  '👋',
  '😊',
  '🌞',
  '✨',
  '🎉',
  '🌻',
  '😄',
  '🌈',
  '🍀',
  '💫',
];

class WelcomeHome extends StatefulWidget {
  const WelcomeHome({
    super.key,
    required this.controller,
    required this.state,
  });

  final HomeController controller;
  final HomeState state;

  @override
  State<WelcomeHome> createState() => _WelcomeHomeState();
}

class _WelcomeHomeState extends State<WelcomeHome> {
  int _emoji = 0;
  int _quote = 0;

  /// 0 Photos · 1 Videos · 2 Music · 3 Books, on one 20-second loop. Clicking a
  /// chip both selects it and restarts the clock, so a deliberate pick gets its
  /// full 20s.
  int _raTab = 0;

  Timer? _emojiTick;
  Timer? _quoteTick;
  Timer? _raTick;
  Timer? _pauseTick;
  Timer? _holdTick;

  /// With a track loaded the bar stands at full height. With nothing loaded it
  /// sinks until only a stub shows, and pops back out while the pointer is on
  /// it. Every row above sizes off `barH`, so the page takes the reclaimed
  /// height back on its own.
  bool _pauseSunk = false;
  bool _peek = false;

  /// Held open for 15s after the pointer last entered or left. Without the hold
  /// the bar closes the instant the pointer slips off an edge — and closing
  /// moves the page under the pointer, which lands it back on the bar. The hold
  /// is what stops the flicker.
  bool _hold = false;

  final MusicController _music = MusicController.instance;

  @override
  void initState() {
    super.initState();
    _emojiTick = Timer.periodic(const Duration(minutes: 2), (_) {
      if (mounted) setState(() => _emoji += 1);
    });
    _quoteTick = Timer.periodic(const Duration(minutes: 1), (_) {
      if (mounted) setState(() => _quote += 1);
    });
    _armRa();
    _music.addListener(_onMusic);
  }

  void _armRa() {
    _raTick?.cancel();
    _raTick = Timer.periodic(const Duration(seconds: 20), (_) {
      if (mounted) setState(() => _raTab = (_raTab + 1) % 4);
    });
  }

  /// Resuming clears the sunk flag, which no binding can do — the flag has to
  /// survive `playing` going false again.
  void _onMusic() {
    if (_music.tickPlaying && _pauseSunk) {
      setState(() => _pauseSunk = false);
      _pauseTick?.cancel();
      return;
    }
    final loaded = (_music.now?.title ?? '').isNotEmpty;
    // Start counting the moment playback stops with a track still loaded; a
    // pause of a few seconds is still listening, fifteen of them is not.
    if (loaded && !_music.tickPlaying && !_pauseSunk && _pauseTick == null) {
      _pauseTick = Timer(const Duration(seconds: 15), () {
        _pauseTick = null;
        if (mounted) setState(() => _pauseSunk = true);
      });
    } else if (_music.tickPlaying) {
      _pauseTick?.cancel();
      _pauseTick = null;
    }
  }

  void _kickHold() {
    _holdTick?.cancel();
    _hold = true;
    _holdTick = Timer(const Duration(seconds: 15), () {
      if (mounted) setState(() => _hold = false);
    });
  }

  @override
  void dispose() {
    _emojiTick?.cancel();
    _quoteTick?.cancel();
    _raTick?.cancel();
    _pauseTick?.cancel();
    _holdTick?.cancel();
    _music.removeListener(_onMusic);
    super.dispose();
  }

  bool _on(String card) => widget.state.cards.contains(card);

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.state;
    return LayoutBuilder(
      builder: (context, box) {
        // ── Vertical budget, from ui/page_home_welcome.slint ───────────────
        const pad = 26.0;
        const gap = 14.0;
        final hub = homeHubCards(st).where((c) => _on(c.card)).toList();
        final barIdle = (_music.now?.title ?? '').isEmpty || _pauseSunk;
        final barOpen = !barIdle || _peek || _hold;
        final barH = !_on('player')
            ? 0.0
            : (barOpen ? kWelcomeBarH + 14 : kWelcomeStubH);
        final rows = (_on('hero') ? 1 : 0) +
            (_on('quick') ? 1 : 0) +
            ((_on('continue') || _on('library')) ? 1 : 0) +
            (hub.isEmpty ? 0 : 1);
        final free = math.max(
            360.0,
            box.maxHeight -
                barH -
                36 -
                math.max(0, rows - 1) * gap -
                (_on('quick') ? 67 : 0));
        // The hero took the 66px the old top bar and its gap used to cost, so
        // My Hub and Continue keep sizing off the old number.
        final bodyFree = math.max(360.0, free - 66);
        final hubH = hub.isEmpty
            ? 0.0
            : (bodyFree * 0.28).clamp(156.0, 200.0).toDouble();
        // Card padding is all the chrome; the tile keeps 74px for its name
        // line, count pill and paddings, and the disc takes what is left.
        final tileDisc = (hubH - 28 - 74).clamp(28.0, 62.0).toDouble();
        // Continue always draws FOUR slots — filled ones first, empty wells
        // after — so Recently Added has a fixed height to match.
        final contRowH =
            ((bodyFree - hubH) * 0.115).clamp(50.0, 66.0).toDouble();
        // 32 padding + 34 title/tab row + 10 spacing.
        final contH = 4 * contRowH + 76;
        final heroH = _on('hero')
            ? math.max(120.0,
                free - hubH - ((_on('continue') || _on('library')) ? contH : 0))
            : 0.0;
        final bodyW = box.maxWidth - 2 * pad;
        final halfW =
            _on('continue') && _on('library') ? (bodyW - gap) / 2 : bodyW;
        return Stack(
          children: [
            Positioned(
              left: pad,
              top: 0,
              width: bodyW,
              height: math.max(0, box.maxHeight - barH),
              child: SingleChildScrollView(
                // Every row is sized to fit, so there is nothing to scroll —
                // the view is the safety net for absurdly short windows.
                child: Padding(
                  padding: const EdgeInsets.symmetric(vertical: 18),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      if (_on('hero')) ...[
                        SizedBox(
                          height: heroH,
                          child: _Hero(
                            state: st,
                            emoji: kGreetEmojis[_emoji % kGreetEmojis.length],
                            quoteIndex: _quote,
                            heroH: heroH,
                            bodyW: bodyW,
                            music: _music,
                          ),
                        ),
                        const SizedBox(height: gap),
                      ],
                      if (_on('quick')) ...[
                        const SizedBox(height: 67, child: _LaunchBar()),
                        const SizedBox(height: gap),
                      ],
                      if (_on('continue') || _on('library')) ...[
                        SizedBox(
                          height: contH,
                          child: Row(
                            crossAxisAlignment: CrossAxisAlignment.stretch,
                            children: [
                              if (_on('continue'))
                                SizedBox(
                                  width: halfW,
                                  child: _ContinueCard(
                                    controller: widget.controller,
                                    state: st,
                                    rowH: contRowH,
                                  ),
                                ),
                              if (_on('continue') && _on('library'))
                                const SizedBox(width: gap),
                              if (_on('library'))
                                SizedBox(
                                  width: halfW,
                                  child: _RecentlyAdded(
                                    state: st,
                                    tab: _raTab,
                                    bodyH: 4 * contRowH,
                                    cellW:
                                        math.max(0, (halfW - 32 - 3 * 8) / 4),
                                    onTab: (i) {
                                      setState(() => _raTab = i);
                                      _armRa();
                                    },
                                  ),
                                ),
                            ],
                          ),
                        ),
                        const SizedBox(height: gap),
                      ],
                      if (hub.isNotEmpty)
                        SizedBox(
                          height: hubH,
                          child: _WelCard(
                            padding: const EdgeInsets.all(14),
                            child: Row(
                              crossAxisAlignment: CrossAxisAlignment.stretch,
                              children: [
                                for (var i = 0; i < hub.length; i++) ...[
                                  if (i > 0) const SizedBox(width: 6),
                                  Expanded(
                                    child: HubTile(
                                      name: hub[i].name,
                                      count: hub[i].count,
                                      icon: hub[i].icon,
                                      accent: hub[i].accent,
                                      disc: tileDisc,
                                      onTap: () => ShellController.instance
                                          .go(hub[i].section),
                                    ),
                                  ),
                                ],
                              ],
                            ),
                          ),
                        ),
                    ],
                  ),
                ),
              ),
            ),
            // ── The bottom player bar ─────────────────────────────────────
            if (_on('player'))
              Positioned(
                left: pad,
                top: box.maxHeight - barH,
                width: bodyW,
                height: kWelcomeBarH,
                child: MouseRegion(
                  onEnter: (_) => setState(() {
                    _peek = true;
                    _kickHold();
                  }),
                  onExit: (_) => setState(() {
                    _peek = false;
                    _kickHold();
                  }),
                  child: barOpen
                      ? WelcomePlayerBar(width: bodyW)
                      // Sunk: only the top strip is above the floor, so the one
                      // mark it carries is centred in THAT strip.
                      : Container(
                          decoration: BoxDecoration(
                            color: t.dark ? t.panel2 : t.panel,
                            borderRadius: BorderRadius.circular(kCardRadius),
                            border: Border.all(
                                color: Tokens.secMusic
                                    .withValues(alpha: t.dark ? 0.45 : 0.30)),
                          ),
                          child: Align(
                            alignment: Alignment.topCenter,
                            child: Container(
                              margin: const EdgeInsets.only(top: 4),
                              width: 34,
                              height: 18,
                              decoration: BoxDecoration(
                                color: Tokens.secMusic
                                    .withValues(alpha: _peek ? 1.0 : 0.78),
                                borderRadius: BorderRadius.circular(9),
                              ),
                              child: const Icon(Icons.music_note,
                                  size: 11, color: Colors.white),
                            ),
                          ),
                        ),
                ),
              ),
          ],
        );
      },
    );
  }
}

// ── Panel base ──────────────────────────────────────────────────────────────

class _WelCard extends StatelessWidget {
  const _WelCard({required this.child, this.padding = EdgeInsets.zero});

  final Widget child;
  final EdgeInsets padding;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: padding,
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(kCardRadius),
        border: Border.all(color: t.outline),
      ),
      child: child,
    );
  }
}

// ── Hero ────────────────────────────────────────────────────────────────────

class _Hero extends StatelessWidget {
  const _Hero({
    required this.state,
    required this.emoji,
    required this.quoteIndex,
    required this.heroH,
    required this.bodyW,
    required this.music,
  });

  final HomeState state;
  final String emoji;
  final int quoteIndex;
  final double heroH;
  final double bodyW;
  final MusicController music;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // Row 4 of the greeting column: the spectrum + lyric pill when audio is
    // playing, the quote shelf when it is not. The height is constant either
    // way, so the greeting above does not jump when a track starts.
    final hvizH = (heroH * 0.20).clamp(46.0, 88.0).toDouble();
    final hvizPad = (heroH * 0.06).clamp(10.0, 26.0).toDouble();
    // The width term is a FACTOR, deliberately, not a measurement: 0.060 keeps
    // "Good afternoon, <name> 👋" on one line out to the 12-character name cap
    // Profile enforces, so the headline cannot wrap.
    final greetCol = bodyW * 0.54 - 16;
    final greetSize = math
        .min((heroH - hvizH - hvizPad) * 0.34, greetCol * 0.060)
        .clamp(22.0, 68.0)
        .toDouble();
    final mode = music.now?.mode ?? 'idle';
    final vizAllowed =
        music.tickPlaying && (mode == 'radio' || mode == 'music');

    return Stack(
      children: [
        // Right 40%: one drawn banner per theme. Both are transparent PNGs, so
        // they sit on the page background rather than on a plate of their own,
        // and both are BOTTOM-anchored — the chips sit in the space above.
        Positioned(
          left: bodyW * 0.60,
          top: 0,
          width: bodyW * 0.36,
          height: heroH,
          child: ClipRect(
            child: Stack(
              children: [
                Positioned(
                  right: 0,
                  bottom: 0,
                  child: Image.asset(
                    t.dark
                        ? 'assets/illustrations/darkhero.png'
                        : 'assets/illustrations/lighthero.png',
                    // 3478 × 1216 — the box keeps that ratio, so nothing is
                    // cropped and nothing is distorted.
                    height: math.min(heroH, bodyW * 0.36 / 2.8602),
                    fit: BoxFit.contain,
                    errorBuilder: (_, __, ___) => const SizedBox.shrink(),
                  ),
                ),
                // The two chips the top bar used to end with, now above the
                // banner and flush with its right edge.
                Positioned(
                  right: 0,
                  top: 0,
                  height: 44,
                  child: Row(
                    children: [
                      const _HeroStatus(),
                      if (vizAllowed) ...[
                        const SizedBox(width: 10),
                        _VizChip(music: music),
                      ],
                      const SizedBox(width: 10),
                      const HomeAvatar(size: 40),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
        // Left 60%: greeting, tagline, the meta pill, and the fourth row.
        // `end`, so the last row sits on the hero's floor, level with the
        // bottom-anchored banner beside it.
        Positioned(
          left: bodyW * 0.06,
          top: 0,
          width: bodyW * 0.54 - 16,
          height: heroH,
          child: Column(
            mainAxisAlignment: MainAxisAlignment.end,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              // Flexible: `greetSize` is derived from the band, but type
              // metrics land a few pixels either side of the derivation, and
              // these two lines are the ones that can give.
              Flexible(
                child: Text(
                  '${state.greeting} $emoji',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: greetSize,
                    fontWeight: FontWeight.w800,
                    letterSpacing: greetSize * -0.04,
                    color: t.text,
                  ),
                ),
              ),
              SizedBox(height: greetSize * 0.16),
              Flexible(
                child: Text('Your hub for everything that matters.',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: greetSize * 0.42, color: t.textDim)),
              ),
              SizedBox(height: greetSize * 0.16),
              // The date and the library totals are the standing facts of the
              // page, so they are given the one loud colour in the hero. The
              // pill is only as wide as its text — a full-width band would read
              // as an error banner.
              Container(
                height: greetSize * 0.26 * 2.1,
                padding: const EdgeInsets.symmetric(horizontal: 12),
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: const Color(0xFFEF4444).withValues(alpha: 0.82),
                  borderRadius: BorderRadius.circular(greetSize * 0.26 * 1.05),
                ),
                child: Text(
                  state.libraryLine.isEmpty
                      ? state.dateLine
                      : '${state.dateLine}  ·  ${state.libraryLine}',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: greetSize * 0.26,
                      fontWeight: FontWeight.w700,
                      color: Colors.white),
                ),
              ),
              SizedBox(height: hvizPad),
              SizedBox(
                height: hvizH,
                // The spectrum runs 25% narrower than the column so it reads as
                // an accent under the greeting rather than a second band; the
                // quote shelf, which replaces it, takes the full width.
                width: vizAllowed ? bodyW * 0.36 : double.infinity,
                child: vizAllowed
                    ? HeaderViz(controller: music, on: true, lyrics: true)
                    : _HeroQuote(quote: state.quote),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// Two lines — the quotation, then its author — after a quote mark on the left.
/// Shown only when the visualizer is not, so the hero row is never an empty
/// band.
class _HeroQuote extends StatelessWidget {
  const _HeroQuote({required this.quote});

  final HomeQuote quote;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (quote.text.isEmpty) return const SizedBox.shrink();
    return Row(
      crossAxisAlignment: CrossAxisAlignment.center,
      children: [
        Icon(Icons.format_quote,
            size: 16, color: t.textDim.withValues(alpha: 0.7)),
        const SizedBox(width: 10),
        Expanded(
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(quote.text,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w500,
                      color: t.text)),
              if (quote.author.isNotEmpty)
                Text('— ${quote.author}',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 11,
                        fontWeight: FontWeight.w600,
                        color: t.textDim)),
            ],
          ),
        ),
      ],
    );
  }
}

/// The sidebar's health key, rebuilt to hero scale. Two differences, both
/// because this one does not leave the app: single click, and it opens the
/// in-window Status page rather than the loopback dashboard.
class _HeroStatus extends StatefulWidget {
  const _HeroStatus();

  @override
  State<_HeroStatus> createState() => _HeroStatusState();
}

class _HeroStatusState extends State<_HeroStatus> {
  bool _pulse = true;
  Timer? _tick;

  @override
  void initState() {
    super.initState();
    // A pulse means "something is happening": idle and OK are steady, so the
    // page stops repainting once the app settles.
    _tick = Timer.periodic(const Duration(milliseconds: 900), (_) {
      final level = ShellController.instance.statusLevel;
      if (!mounted) return;
      if (level == 'busy' || level == 'problem') {
        setState(() => _pulse = !_pulse);
      } else if (!_pulse) {
        setState(() => _pulse = true);
      }
    });
  }

  @override
  void dispose() {
    _tick?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final shell = ShellController.instance;
    return AnimatedBuilder(
      animation: shell,
      builder: (context, _) {
        // Busy is green — a scan running is the app working, not a warning.
        final lamp = switch (shell.statusLevel) {
          'ok' => const Color(0xFF22C55E),
          'busy' => const Color(0xFF10B981),
          'problem' => Tokens.error,
          _ => Tokens.secSettings,
        };
        return Hover(
          onTap: () => shell.go(Section.settings),
          builder: (context, hov) => AnimatedContainer(
            duration: const Duration(milliseconds: 140),
            width: 162,
            height: 40,
            padding: const EdgeInsets.only(left: 11, right: 24),
            decoration: BoxDecoration(
              color: lamp.withValues(alpha: hov ? 0.16 : 0.09),
              borderRadius: BorderRadius.circular(10),
              border:
                  Border.all(color: lamp.withValues(alpha: hov ? 0.62 : 0.38)),
            ),
            child: Stack(
              clipBehavior: Clip.none,
              children: [
                Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text('Status',
                        style: TextStyle(
                            fontSize: 12,
                            fontWeight: FontWeight.w700,
                            color: t.text)),
                    Text(shell.statusNote,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 10, color: t.textDim)),
                  ],
                ),
                // The lamp breathes on its own timer with a fixed glow ring:
                // the fill fades, the ring does not, so it reads as a lamp
                // turning up and down rather than as something appearing.
                Positioned(
                  right: -13,
                  top: 0,
                  bottom: 0,
                  child: Center(
                    child: Container(
                      width: 15,
                      height: 15,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        border: Border.all(color: lamp.withValues(alpha: 0.3)),
                      ),
                      child: Center(
                        child: AnimatedContainer(
                          duration: const Duration(milliseconds: 850),
                          width: 9,
                          height: 9,
                          decoration: BoxDecoration(
                            color: lamp.withValues(alpha: _pulse ? 1.0 : 0.25),
                            shape: BoxShape.circle,
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }
}

class _VizChip extends StatelessWidget {
  const _VizChip({required this.music});

  final MusicController music;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Hover(
      onTap: () => music.setVisOn(!music.visOn),
      builder: (context, hov) => Container(
        width: 40,
        height: 40,
        decoration: BoxDecoration(
          color: music.visOn ? music.accent : (hov ? t.panel2 : t.panel),
          shape: BoxShape.circle,
          border: Border.all(color: music.visOn ? music.accent : t.outline),
        ),
        child: Icon(Icons.graphic_eq,
            size: 17, color: music.visOn ? Colors.white : t.textDim),
      ),
    );
  }
}

// ── Launch bar ──────────────────────────────────────────────────────────────

class _LaunchBar extends StatelessWidget {
  const _LaunchBar();

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return _WelCard(
      padding: const EdgeInsets.symmetric(horizontal: 8),
      // Stretch, or the 1px rules between the launchers have no height to draw.
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (var i = 0; i < kLaunchers.length; i++) ...[
            if (i > 0) Container(width: 1, color: t.outline),
            Expanded(
              child: Hover(
                onTap: () => launch(kLaunchers[i].id),
                builder: (context, hov) => AnimatedContainer(
                  duration: const Duration(milliseconds: 120),
                  decoration: BoxDecoration(
                    color: hov ? t.glass : Colors.transparent,
                    borderRadius: BorderRadius.circular(14),
                  ),
                  child: Row(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      Container(
                        width: 34,
                        height: 34,
                        decoration: BoxDecoration(
                          color: kLaunchers[i]
                              .accent
                              .withValues(alpha: t.dark ? 0.24 : 0.14),
                          borderRadius: BorderRadius.circular(10),
                        ),
                        child: Icon(kLaunchers[i].icon,
                            size: 16, color: kLaunchers[i].accent),
                      ),
                      const SizedBox(width: 10),
                      Flexible(
                        child: Text(kLaunchers[i].label,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontSize: 12.5,
                                fontWeight: FontWeight.w600,
                                color: t.text)),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ],
        ],
      ),
    );
  }
}

// ── Continue ────────────────────────────────────────────────────────────────

class _ContinueCard extends StatelessWidget {
  const _ContinueCard({
    required this.controller,
    required this.state,
    required this.rowH,
  });

  final HomeController controller;
  final HomeState state;
  final double rowH;

  @override
  Widget build(BuildContext context) {
    final rows = state.continueRows;
    final n = math.min(4, rows.length);
    return _WelCard(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // Title and tabs share ONE row — the tabs never push the rows down.
          SizedBox(
            height: 34,
            child: Row(
              children: [
                Expanded(
                  child: Text('Continue',
                      style: TextStyle(
                          fontSize: 14,
                          fontWeight: FontWeight.w700,
                          color: context.tokens.text)),
                ),
                ContinueTabs(
                  active: state.continueFilter,
                  onPick: (id) =>
                      controller.send(HomeCmd.setContinueFilter(filter: id)),
                ),
              ],
            ),
          ),
          const SizedBox(height: 10),
          // FOUR slots, always. Filled ones first, empty wells after — and they
          // divide what the card gives them rather than each taking a computed
          // height, which is off by the card's own hairline.
          Expanded(
            child: LayoutBuilder(
              builder: (context, box) => Column(
                children: [
                  for (var i = 0; i < 4; i++)
                    Expanded(
                      child: Padding(
                        padding: const EdgeInsets.symmetric(vertical: 2),
                        child: i < n
                            ? _ContinueRow(
                                row: rows[i], height: box.maxHeight / 4 - 4)
                            : _Well(first: i == 0 && n == 0),
                      ),
                    ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// One in-progress row: cover, title over author, then the kind tag and the
/// progress pill on the right.
class _ContinueRow extends StatelessWidget {
  const _ContinueRow({required this.row, required this.height});

  final HomeContinue row;
  final double height;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final accent = kindColor(row.kind);
    final cw = (height - 8) * 3 / 4;
    return Hover(
      onTap: () => ShellController.instance.go(kindSection(row.kind)),
      builder: (context, hov) => AnimatedContainer(
        duration: const Duration(milliseconds: 130),
        padding: const EdgeInsets.symmetric(horizontal: 7),
        decoration: BoxDecoration(
          // Hover fills with the row's own kind accent and takes an accent
          // edge — a panel-to-panel lift was invisible on a page of panels.
          color: hov
              ? accent.withValues(alpha: t.dark ? 0.28 : 0.18)
              : t.panel.withValues(alpha: 0.55),
          borderRadius: BorderRadius.circular(12),
          border: Border.all(
              color: hov ? accent.withValues(alpha: 0.45) : t.outline),
        ),
        child: Row(
          children: [
            ClipRRect(
              borderRadius: BorderRadius.circular(8),
              child: SizedBox(
                width: cw,
                height: height - 8,
                child: LazyCover(
                  section: kindSection(row.kind),
                  id: row.id,
                  tint: accent,
                  icon: kindIcon(row.kind),
                  iconSize: 15,
                ),
              ),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(row.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 13,
                          fontWeight: FontWeight.w800,
                          color: t.text)),
                  Text(row.author.isEmpty ? row.sub : row.author,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 10.5, color: t.textDim)),
                ],
              ),
            ),
            const SizedBox(width: 8),
            KindTag(
                kind: row.kind,
                accent: accent,
                height: math.min(34, height - 8)),
            const SizedBox(width: 8),
            SizedBox(
              width: 172,
              child: ProgressPill(sub: row.sub, frac: row.frac, accent: accent),
            ),
            const SizedBox(width: 2),
          ],
        ),
      ),
    );
  }
}

/// The wells that keep the card four rows tall.
class _Well extends StatelessWidget {
  const _Well({required this.first});

  /// The empty-state line rides the first well only.
  final bool first;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14),
      alignment: Alignment.centerLeft,
      decoration: BoxDecoration(
        color: t.text.withValues(alpha: 0.03),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: t.text.withValues(alpha: 0.06)),
      ),
      child: first
          ? Text(
              'Nothing half-finished yet — what you start shows up here.',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(fontSize: 11, color: t.textDim),
            )
          : null,
    );
  }
}

// ── Recently Added ──────────────────────────────────────────────────────────

class _RecentlyAdded extends StatelessWidget {
  const _RecentlyAdded({
    required this.state,
    required this.tab,
    required this.bodyH,
    required this.cellW,
    required this.onTab,
  });

  final HomeState state;
  final int tab;
  final double bodyH;
  final double cellW;
  final ValueChanged<int> onTab;

  static const List<({String label, Color accent, Section section})> _tabs = [
    (label: 'Photos', accent: Tokens.secPhotos, section: Section.photos),
    (label: 'Videos', accent: Tokens.secVideos, section: Section.videos),
    (label: 'Music', accent: Tokens.secMusic, section: Section.music),
    (label: 'Books', accent: Tokens.secBooks, section: Section.books),
  ];

  List<HomeTile> get _tiles => switch (tab) {
        0 => state.recentPhotos,
        1 => state.recentVideos,
        2 => state.recentSongs,
        _ => state.recentBooks,
      };

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tiles = _tiles;
    final cellH = math.max(0.0, (bodyH - 8) / 2);
    return _WelCard(
      padding: const EdgeInsets.all(16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SizedBox(
            height: 34,
            child: Row(
              children: [
                Expanded(
                  child: Text('Recently Added',
                      style: TextStyle(
                          fontSize: 14,
                          fontWeight: FontWeight.w700,
                          color: t.text)),
                ),
                for (var i = 0; i < _tabs.length; i++) ...[
                  if (i > 0) const SizedBox(width: 6),
                  HomeChip(
                    label: _tabs[i].label,
                    on: tab == i,
                    accent: _tabs[i].accent,
                    onTap: () => onTab(i),
                  ),
                ],
              ],
            ),
          ),
          const SizedBox(height: 10),
          Expanded(
            child: tiles.isEmpty
                ? Align(
                    alignment: Alignment.centerLeft,
                    child: Text(
                        'No ${_tabs[tab].label.toLowerCase()} indexed yet.',
                        style: TextStyle(fontSize: 12, color: t.textDim)),
                  )
                // Photos and Videos share a 4 × 2 grid; Music and Books get one
                // row of four, because their cards carry a title line.
                : tab <= 1
                    ? _grid(tiles, cellW, cellH, columns: 4, rows: 2)
                    : _grid(tiles, cellW, bodyH, columns: 4, rows: 1),
          ),
        ],
      ),
    );
  }

  Widget _grid(
    List<HomeTile> tiles,
    double cellW,
    double cellH, {
    required int columns,
    required int rows,
  }) {
    final max = columns * rows;
    final section = _tabs[tab].section;
    final accent = _tabs[tab].accent;
    return Stack(
      children: [
        for (var i = 0; i < math.min(max, tiles.length); i++)
          Positioned(
            left: (i % columns) * (cellW + 8),
            top: (i ~/ columns) * (cellH + 8),
            width: cellW,
            height: cellH,
            child: _RaTile(
              tile: tiles[i],
              section: section,
              accent: accent,
              // Music and Books carry a title line under the art; the two
              // picture tabs do not — a filename under a photo is noise.
              withLabel: tab >= 2,
              play: tab == 1,
            ),
          ),
      ],
    );
  }
}

class _RaTile extends StatelessWidget {
  const _RaTile({
    required this.tile,
    required this.section,
    required this.accent,
    required this.withLabel,
    required this.play,
  });

  final HomeTile tile;
  final Section section;
  final Color accent;
  final bool withLabel;
  final bool play;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Hover(
      onTap: () => ShellController.instance.go(section),
      builder: (context, hov) {
        final art = AnimatedContainer(
          duration: const Duration(milliseconds: 120),
          decoration: BoxDecoration(
            color: accent.withValues(alpha: 0.14),
            borderRadius: BorderRadius.circular(10),
            border: Border.all(
                color: hov ? accent : Colors.transparent, width: hov ? 2 : 0),
          ),
          clipBehavior: Clip.antiAlias,
          child: Stack(
            fit: StackFit.expand,
            children: [
              LazyCover(
                section: section,
                id: tile.id,
                tint: accent,
                icon: kSectionIcon(section),
              ),
              AnimatedOpacity(
                duration: const Duration(milliseconds: 120),
                opacity: hov ? 1 : 0,
                child: const ColoredBox(color: Color(0x59000000)),
              ),
              if (play || hov)
                Center(
                  child: Opacity(
                    opacity: play ? (hov ? 1 : 0.82) : (hov ? 1 : 0),
                    child: Container(
                      width: 30,
                      height: 30,
                      decoration:
                          BoxDecoration(color: accent, shape: BoxShape.circle),
                      child: const Icon(Icons.play_arrow,
                          size: 13, color: Colors.white),
                    ),
                  ),
                ),
            ],
          ),
        );
        if (!withLabel) return art;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(child: art),
            const SizedBox(height: 8),
            Text(tile.label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                textAlign: TextAlign.center,
                style: TextStyle(
                    fontSize: 13, fontWeight: FontWeight.w600, color: t.text)),
          ],
        );
      },
    );
  }
}

IconData kSectionIcon(Section s) => switch (s) {
      Section.photos => Icons.image_outlined,
      Section.videos => Icons.movie_outlined,
      Section.music => Icons.music_note_outlined,
      _ => Icons.menu_book_outlined,
    };
