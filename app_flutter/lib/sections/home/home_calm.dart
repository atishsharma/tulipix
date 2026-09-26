// Home · Calm — the quiet one.
//
// docs/mockups/NewSections/home-calm-deck.html. One thing first, the rest a
// breath away: a soft light that follows the hour, a greeting and a single
// suggestion, a new photo, three things to pick up again, every section on a
// quiet shelf, and a few small comforts. What is waiting is one gentle
// sentence, never a red badge.

import 'dart:async';
import 'dart:io' show File;
import 'dart:math' as math;
import 'dart:ui' show ImageFilter;

import 'package:flutter/material.dart';

import '../../design/decode.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../shell/sidebar.dart';
import '../../src/rust/api/home.dart';
import '../music/music_controller.dart';
import 'home_controller.dart';
import 'home_modern.dart';
import 'home_shared.dart';

/// The hour's words, and which way its light leans.
///
/// The light itself is the theme's: the design language's accent (or the
/// brand colour under Standard), pulled a little toward [lean] so morning
/// still reads warmer than night in every language.
typedef _Mood = ({String hello, String sub, String suggest, Color lean});

/// One Pick up again card: a real row's, or one of the deck's samples.
typedef _Again = ({
  Widget art,
  String title,
  String sub,
  double frac,
  Color bar,
  VoidCallback go,
});

_Mood _mood(DayPart p) => switch (p) {
      DayPart.morning => (
          hello: 'Good morning',
          sub: 'Take your time. Everything else can wait.',
          suggest: 'To begin the day',
          lean: const Color(0xFFF59E0B),
        ),
      DayPart.day => (
          hello: 'Hello again',
          sub: 'Halfway through the day. A small thing, then a break.',
          suggest: 'For the afternoon',
          lean: const Color(0xFF10B981),
        ),
      DayPart.evening => (
          hello: 'Welcome home',
          sub: 'The day is done. Nothing here needs you tonight.',
          suggest: 'For tonight',
          lean: const Color(0xFFF97316),
        ),
      DayPart.night => (
          hello: 'Rest well',
          sub: 'A few quiet pages, and then sleep.',
          suggest: 'Before sleep',
          lean: const Color(0xFF6366F1),
        ),
    };

/// A card as the design language draws one, or the plain panel under
/// Standard.
Decoration _card(BuildContext context, double radius) {
  final t = context.tokens;
  return context.skin.surface(SurfaceRole.card, radius: radius) ??
      BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(radius),
        border: Border.all(color: t.outline),
      );
}

class CalmHome extends StatefulWidget {
  const CalmHome({super.key, required this.controller, required this.state});

  final HomeController controller;
  final HomeState state;

  @override
  State<CalmHome> createState() => _CalmHomeState();
}

class _CalmHomeState extends State<CalmHome>
    with SingleTickerProviderStateMixin {
  late final AnimationController _breath = AnimationController(
    vsync: this,
    duration: const Duration(seconds: 10),
  );
  Timer? _clock;
  DateTime _now = DateTime.now();

  /// "Not now" on the suggestion, for this session: a dismissal that outlived
  /// the app would hide a film someone came back to a week later.
  static final Set<String> _notNow = {};
  bool _gentleOpen = false;
  bool _breathing = true;

  @override
  void initState() {
    super.initState();
    _clock = Timer.periodic(const Duration(seconds: 20), (_) {
      if (mounted && DateTime.now().minute != _now.minute) {
        setState(() => _now = DateTime.now());
      }
    });
    WidgetsBinding.instance.addPostFrameCallback((_) => _syncBreath());
  }

  void _syncBreath() {
    if (!mounted) return;
    if (_breathing && !context.tokens.reduceMotion) {
      _breath.repeat(reverse: true);
    } else {
      _breath.stop();
    }
  }

  @override
  void dispose() {
    _clock?.cancel();
    _breath.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.state;
    final part = dayPart(_now);
    final m = _mood(part);
    final name = firstName(st);
    final rows = cardOn(st, 'continue') ? st.continueRows : <HomeContinue>[];
    final suggest = rows
        .where((r) => !_notNow.contains('${r.kind}/${r.id}/${r.path}'))
        .firstOrNull;
    // Past the suggestion, or everything when the suggestion is all there is:
    // an empty row there read as the card having gone missing.
    final rest = rows.where((r) => r != suggest).toList();
    final again = (rest.isEmpty ? rows : rest).take(3).toList();
    final photosOn = sectionOn(Section.photos) && cardOn(st, 'photos');
    final dated = photosOn && st.onThisDay.isNotEmpty;
    final photos = !photosOn
        ? <HomeTile>[]
        : (dated ? st.onThisDay : st.recentPhotos).take(10).toList();
    final skin = context.skin;
    final accent = Color.lerp(skin.accent ?? Tokens.brand, m.lean, 0.35)!;
    final glowA = t.dark ? 0.14 : 0.22;

    return Stack(
      children: [
        // The light: two soft glows in the hour's colours.
        Positioned(
          top: -220,
          left: -160,
          child: _Glow(color: accent.withValues(alpha: glowA), size: 620),
        ),
        Positioned(
          top: -120,
          right: -200,
          child: _Glow(
              color: (skin.accentSoft ?? m.lean).withValues(alpha: glowA),
              size: 560),
        ),
        LayoutBuilder(builder: (context, box) {
          final wide = box.maxWidth >= 980;
          final pad = box.maxWidth >= 700 ? 44.0 : 20.0;
          return SingleChildScrollView(
            padding: EdgeInsets.fromLTRB(pad, 36, pad, 40),
            child: Center(
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 1180),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    if (cardOn(st, 'hero')) ...[
                      _hello(t, m, accent, part, name),
                      const SizedBox(height: 30),
                    ],
                    _one(t, m, accent, suggest, photos, dated, wide),
                    const SizedBox(height: 18),
                    _gentle(t, st),
                    _label(t, 'Pick up again'),
                    _again(t, again, part, wide),
                    const SizedBox(height: 28),
                    _comforts(t, st, accent, wide),
                    const SizedBox(height: 28),
                    _shelf(t, st, box.maxWidth),
                  ],
                ),
              ),
            ),
          );
        }),
      ],
    );
  }

  Widget _label(Tokens t, String s) => Padding(
        padding: const EdgeInsets.only(top: 34, bottom: 12),
        child: Text(s.toUpperCase(),
            style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w600,
                letterSpacing: 1.6,
                color: t.textDim)),
      );

  Widget _hello(Tokens t, _Mood m, Color accent, DayPart part, String name) {
    final hh = _now.hour.toString().padLeft(2, '0');
    final mm = _now.minute.toString().padLeft(2, '0');
    final glass = _GlassClock(
      time: '$hh:$mm',
      date: widget.state.dateLine,
      accent: accent,
      second: context.skin.accentSoft ?? m.lean,
    );
    final greet = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(dayPartName(part).toUpperCase(),
            style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w600,
                letterSpacing: 1.8,
                color: accent)),
        const SizedBox(height: 8),
        Text(name.isEmpty ? '${m.hello}.' : '${m.hello}, $name.',
            style: TextStyle(
                fontSize: 38,
                height: 1.1,
                fontWeight: FontWeight.w400,
                color: t.text)),
        const SizedBox(height: 8),
        Text(m.sub, style: TextStyle(fontSize: 14.5, color: t.textDim)),
      ],
    );
    // The glass is the page's centrepiece; on a narrow window it takes its
    // own line under the greeting rather than squeezing it.
    return LayoutBuilder(
      builder: (context, box) => box.maxWidth < 720
          ? Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [greet, const SizedBox(height: 22), glass],
            )
          : Row(
              children: [
                Expanded(child: greet),
                const SizedBox(width: 24),
                glass,
              ],
            ),
    );
  }

  Widget _one(Tokens t, _Mood m, Color accent, HomeContinue? s,
      List<HomeTile> photos, bool dated, bool wide) {
    final card = Container(
      padding: const EdgeInsets.all(22),
      decoration: _card(context, 24),
      child: s == null
          ? Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(m.suggest.toUpperCase(),
                    style: TextStyle(
                        fontSize: 11,
                        fontWeight: FontWeight.w600,
                        letterSpacing: 1.6,
                        color: accent)),
                const SizedBox(height: 10),
                Text('Nothing half-finished.',
                    style: TextStyle(fontSize: 26, color: t.text)),
                const SizedBox(height: 6),
                Text('A good moment to start something, or to do nothing.',
                    style: TextStyle(fontSize: 13.5, color: t.textDim)),
              ],
            )
          : Row(
              children: [
                ClipRRect(
                  borderRadius: BorderRadius.circular(16),
                  child: SizedBox(
                      width: 120, height: 160, child: RowCover(row: s)),
                ),
                const SizedBox(width: 20),
                // As tall as the cover, padded top and bottom: the label
                // lines up under its top edge and the pills sit on its
                // bottom one.
                Expanded(
                  child: Container(
                    height: 160,
                    padding: const EdgeInsets.symmetric(vertical: 8),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(m.suggest.toUpperCase(),
                            style: TextStyle(
                                fontSize: 11,
                                fontWeight: FontWeight.w600,
                                letterSpacing: 1.6,
                                color: accent)),
                        const SizedBox(height: 8),
                        Text(s.title,
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontSize: 24, height: 1.15, color: t.text)),
                        const SizedBox(height: 4),
                        Text(
                            [s.author, s.sub]
                                .where((x) => x.isNotEmpty)
                                .join(' · '),
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 13.5, color: t.textDim)),
                        const Spacer(),
                        Wrap(
                          spacing: 10,
                          runSpacing: 8,
                          children: [
                            PillBtn(
                              label: switch (s.kind) {
                                'video' => 'Keep watching',
                                'book' => 'Keep reading',
                                _ => 'Keep listening',
                              },
                              fill: accent,
                              ink: Colors.white,
                              onTap: () => resumeRow(s),
                            ),
                            PillBtn(
                              label: 'Not now',
                              solid: false,
                              fill: t.panel2,
                              ink: t.textDim,
                              onTap: () => setState(() =>
                                  _notNow.add('${s.kind}/${s.id}/${s.path}')),
                            ),
                          ],
                        ),
                      ],
                    ),
                  ),
                ),
              ],
            ),
    );
    // On this day: up to ten photos shot on today's date in earlier years,
    // else the ten newest, one at a time; else the deck's sample.
    final memory = Tap(
      onTap: () => ShellController.instance.go(Section.photos),
      child: ClipRRect(
        borderRadius: BorderRadius.circular(24),
        child: SizedBox(
          height: 260,
          child: Stack(
            fit: StackFit.expand,
            children: [
              if (photos.isNotEmpty)
                _Slides(photos: photos, dated: dated, now: _now)
              else
                const DecoratedBox(
                  decoration: BoxDecoration(
                    gradient: LinearGradient(
                      begin: Alignment.topCenter,
                      end: Alignment.bottomCenter,
                      colors: [
                        Color(0xFFF3C9A5),
                        Color(0xFFE7A98B),
                        Color(0xFF9DB4C0),
                        Color(0xFF6F8FA3),
                      ],
                      stops: [0, 0.38, 0.62, 1],
                    ),
                  ),
                ),
              // The slides draw their own shade, under their caption.
              if (photos.isEmpty) ...[
                const DecoratedBox(
                  decoration: BoxDecoration(
                    gradient: LinearGradient(
                      begin: Alignment.bottomCenter,
                      end: Alignment.topCenter,
                      colors: [Color(0x80000000), Color(0x00000000)],
                      stops: [0, 0.55],
                    ),
                  ),
                ),
                const _Caption(
                    'On this day, 2023', 'The last light over Lisbon'),
              ],
              Positioned(
                right: 18,
                bottom: 18,
                child: Container(
                  height: 32,
                  padding: const EdgeInsets.symmetric(horizontal: 14),
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: const Color(0x33FFFFFF),
                    borderRadius: BorderRadius.circular(999),
                  ),
                  child: const Text('See the day',
                      style: TextStyle(fontSize: 12, color: Colors.white)),
                ),
              ),
            ],
          ),
        ),
      ),
    );
    // The memory is a fixed 260; the suggestion stretches to match it. Its
    // cover is boxed to a fixed size, so the height question never reaches
    // the cover's LayoutBuilder.
    return wide
        ? IntrinsicHeight(
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Expanded(flex: 5, child: card),
                const SizedBox(width: 20),
                Expanded(flex: 4, child: memory),
              ],
            ),
          )
        : Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [card, const SizedBox(height: 16), memory],
          );
  }

  /// What is waiting, as one sentence; the list only when asked for. The
  /// deck's three samples stand in while nothing is.
  Widget _gentle(Tokens t, HomeState st) {
    void go(Section s) => ShellController.instance.go(s);
    var items = [
      for (final d in st.finDues)
        (
          c: Tokens.secFinances,
          text: '${d.name}, ${d.amount}',
          where: 'Due ${d.due}',
          act: 'Open',
          go: () => go(Section.finances),
        ),
      if (st.counts.papersUnfiled > 0)
        (
          c: Tokens.secPapers,
          text: '${plural(st.counts.papersUnfiled, 'scan')} to put away',
          where: 'Papers',
          act: 'Open',
          go: () => go(Section.papers),
        ),
      if (st.counts.toolsRunning > 0)
        (
          c: Tokens.secTools,
          text: '${plural(st.counts.toolsRunning, 'job')} running',
          where: 'Tools',
          act: 'Open',
          go: () => go(Section.tools),
        ),
    ];
    if (items.isEmpty) {
      items = [
        (
          c: Tokens.secFinances,
          text: 'Electricity bill, due Friday',
          where: 'Finances',
          act: 'Mark paid',
          go: () => go(Section.finances),
        ),
        (
          c: Tokens.secPapers,
          text: 'Five scans to put away',
          where: 'Papers',
          act: 'Open',
          go: () => go(Section.papers),
        ),
        (
          c: Tokens.secCloud,
          text: 'One file saved in two places',
          where: 'Cloud',
          act: 'Choose',
          go: () => go(Section.cloud),
        ),
      ];
    }
    final n = items.length;
    final words = n == 1
        ? 'One small thing'
        : n <= 9
            ? '${const [
                '',
                '',
                'Two',
                'Three',
                'Four',
                'Five',
                'Six',
                'Seven',
                'Eight',
                'Nine'
              ][n]} small things'
            : '$n small things';
    final skin = context.skin;
    final leaf = skin.accent ?? Tokens.brand;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 14),
      decoration: _card(context, 18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Container(
                width: 34,
                height: 34,
                decoration: BoxDecoration(
                  color: leaf.withValues(alpha: 0.16),
                  shape: BoxShape.circle,
                ),
                child:
                    Icon(skin.icon(Icons.spa_outlined), size: 17, color: leaf),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Text.rich(
                  TextSpan(children: [
                    TextSpan(
                        text: words,
                        style: TextStyle(
                            fontWeight: FontWeight.w600, color: t.text)),
                    const TextSpan(
                        text: ', when you are ready. None of them is urgent.'),
                  ]),
                  style: TextStyle(fontSize: 13.5, color: t.textDim),
                ),
              ),
              Tap(
                onTap: () => setState(() => _gentleOpen = !_gentleOpen),
                child: Text(_gentleOpen ? 'Hide' : 'Show',
                    style: TextStyle(
                        fontSize: 12.5,
                        fontWeight: FontWeight.w600,
                        color: t.textDim)),
              ),
            ],
          ),
          if (_gentleOpen)
            for (final i in items)
              Padding(
                padding: const EdgeInsets.only(top: 10, left: 48),
                child: Row(
                  children: [
                    Container(
                      width: 6,
                      height: 6,
                      decoration:
                          BoxDecoration(color: i.c, shape: BoxShape.circle),
                    ),
                    const SizedBox(width: 12),
                    Expanded(
                      child: Text(i.text,
                          style: TextStyle(fontSize: 13, color: t.text)),
                    ),
                    Text(i.where,
                        style: TextStyle(fontSize: 12, color: t.textDim)),
                    const SizedBox(width: 12),
                    PillBtn(
                      label: i.act,
                      solid: false,
                      fill: t.panel2,
                      ink: t.textDim,
                      onTap: i.go,
                    ),
                  ],
                ),
              ),
        ],
      ),
    );
  }

  /// Three things to pick up again, never more. The deck's samples for the
  /// hour stand in while nothing is half-finished.
  Widget _again(Tokens t, List<HomeContinue> rows, DayPart part, bool wide) {
    Widget grad(List<int> c) => DecoratedBox(
          decoration: BoxDecoration(
            gradient: LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [Color(c[0]), Color(c[1])],
            ),
          ),
        );
    final List<_Again> items = rows.isNotEmpty
        ? [
            for (final r in rows)
              (
                art: RowCover(row: r),
                title: r.title,
                sub: r.sub.isEmpty ? r.author : r.sub,
                frac: r.frac,
                bar: kindColor(r.kind),
                go: () => resumeRow(r),
              ),
          ]
        : [
            for (final (title, sub, frac, c, s) in _sampleAgain[part]!)
              (
                art: grad(c),
                title: title,
                sub: sub,
                frac: frac,
                bar: Tokens.accentOf(s),
                go: () => ShellController.instance.go(s),
              ),
          ];
    Widget card(_Again r) => Tap(
          onTap: r.go,
          child: Container(
            padding: const EdgeInsets.all(14),
            decoration: _card(context, 20),
            child: Row(
              children: [
                ClipRRect(
                  borderRadius: BorderRadius.circular(16),
                  child: SizedBox(width: 64, height: 64, child: r.art),
                ),
                const SizedBox(width: 14),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(r.title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 14,
                              fontWeight: FontWeight.w600,
                              color: t.text)),
                      Text(r.sub,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 12, color: t.textDim)),
                      const SizedBox(height: 8),
                      ThinBar(frac: r.frac, color: r.bar),
                    ],
                  ),
                ),
              ],
            ),
          ),
        );
    return wide
        ? Row(
            children: [
              for (var i = 0; i < 3; i++) ...[
                if (i > 0) const SizedBox(width: 14),
                Expanded(
                    child: i < items.length
                        ? card(items[i])
                        : const SizedBox.shrink()),
              ],
            ],
          )
        : Column(
            children: [
              for (var i = 0; i < items.length; i++) ...[
                if (i > 0) const SizedBox(height: 10),
                card(items[i]),
              ],
            ],
          );
  }

  /// The deck's Pick up again, by the hour: title, where you are, how far,
  /// the cover's two colours, and where it opens.
  static const Map<DayPart, List<(String, String, double, List<int>, Section)>>
      _sampleAgain = {
    DayPart.morning: [
      (
        'Hard Fork',
        'The AI week · 22 min left',
        0.30,
        [0xFFE98A6B, 0xFFF2B880],
        Section.music
      ),
      (
        'Yesterday’s entry',
        'A draft, two lines',
        0.30,
        [0xFFF3D9A4, 0xFFE9B872],
        Section.journal
      ),
      (
        'The Overstory',
        'Page 214 of 502',
        0.43,
        [0xFF355C4D, 0xFF8DB59A],
        Section.books
      ),
    ],
    DayPart.day: [
      (
        'Lisbon trip movie',
        '2:14 of 4:00 cut',
        0.56,
        [0xFFF2B35E, 0xFFF7D68A],
        Section.studio
      ),
      (
        'Hard Fork',
        'The AI week · 22 min left',
        0.30,
        [0xFFE98A6B, 0xFFF2B880],
        Section.music
      ),
      (
        'Slow Horses',
        'Episode 4 · 38 min left',
        0.55,
        [0xFF3E3A4F, 0xFF7A5C77],
        Section.videos
      ),
    ],
    DayPart.evening: [
      (
        'Slow Horses',
        'Episode 4 · 38 min left',
        0.55,
        [0xFF3E3A4F, 0xFF7A5C77],
        Section.videos
      ),
      (
        'Mushroom risotto',
        'Step 3 of 7',
        0.40,
        [0xFFF5E6C8, 0xFFD3A86B],
        Section.kitchen
      ),
      (
        'Past Lives',
        '1 h 04 min left',
        0.18,
        [0xFF6C7BB8, 0xFFB28AB6],
        Section.videos
      ),
    ],
    DayPart.night: [
      (
        'Project Hail Mary',
        'Chapter 12 · 1 h 12 min in',
        0.62,
        [0xFF1F2540, 0xFF3B4466],
        Section.books
      ),
      (
        'The Overstory',
        'Page 214 of 502',
        0.43,
        [0xFF355C4D, 0xFF8DB59A],
        Section.books
      ),
      (
        'Today’s entry',
        'Two lines so far',
        0.20,
        [0xFFF3D9A4, 0xFFE9B872],
        Section.journal
      ),
    ],
  };

  Widget _shelf(Tokens t, HomeState st, double width) {
    final list = ShellController.instance.sections
        .where((s) => s != Section.home && s != Section.settings)
        .toList();
    // Three across, every card the same 4:1 — low, so a long sidebar still
    // fits on a screen — with a floor that keeps the icon, name and line
    // inside it on a narrow window. Two only where three would be unreadable.
    final cols = width >= 520 ? 3 : 2;
    return LayoutBuilder(builder: (context, box) {
      final w = (box.maxWidth - 12 * (cols - 1)) / cols;
      final h = math.max(w / 4, 78.0);
      return Wrap(
        spacing: 12,
        runSpacing: 12,
        children: [
          for (final s in list)
            SizedBox(
              width: w,
              height: h,
              child: Hover(
                onTap: () => ShellController.instance.go(s),
                builder: (context, hov) {
                  final c = Tokens.accentOf(s);
                  final line = sectionLine(s, st);
                  return Container(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 14, vertical: 8),
                    decoration: _card(context, 16),
                    // Over whatever the skin drew, so the outline shows in
                    // every design language.
                    foregroundDecoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(16),
                      border: Border.all(
                          color: hov ? c : Colors.transparent, width: 1.5),
                    ),
                    child: Column(
                      mainAxisAlignment: MainAxisAlignment.center,
                      children: [
                        Icon(
                            context.skin.icon(
                                kSectionMeta[s]?.icon ?? Icons.circle_outlined),
                            size: 20,
                            color: c),
                        const SizedBox(height: 4),
                        Text(kSectionMeta[s]?.label ?? s.name,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontSize: 14,
                                fontWeight: FontWeight.w600,
                                color: t.text)),
                        Text(line.isNotEmpty ? line : _about[s] ?? '',
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            textAlign: TextAlign.center,
                            style: TextStyle(fontSize: 11.5, color: t.textDim)),
                      ],
                    ),
                  );
                },
              ),
            ),
        ],
      );
    });
  }

  /// Three words for a section the counts have nothing to say about.
  static const Map<Section, String> _about = {
    Section.photos: 'Every picture, together',
    Section.videos: 'Films and shows',
    Section.music: 'Songs and radio',
    Section.books: 'Read and listen',
    Section.cloud: 'Your online drives',
    Section.tools: 'Convert, compress, rename',
    Section.transfer: 'Share with phones',
    Section.finances: 'Money, calmly kept',
    Section.feeds: 'News you follow',
    Section.journal: 'Write the day',
    Section.kitchen: 'Recipes and meals',
    Section.papers: 'Documents kept safe',
    Section.voice: 'Notes, said aloud',
    Section.places: 'Trips and maps',
    Section.studio: 'Make something new',
    Section.archive: 'Old files, catalogued',
    Section.arcade: 'Games to play',
  };

  Widget _comforts(Tokens t, HomeState st, Color accent, bool wide) {
    // Everything in the middle of its card, the same padding all round.
    Widget box(String k, Widget child) => Container(
          padding: const EdgeInsets.all(22),
          decoration: _card(context, 22),
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            crossAxisAlignment: CrossAxisAlignment.center,
            children: [
              Text(k.toUpperCase(),
                  style: TextStyle(
                      fontSize: 10.5,
                      fontWeight: FontWeight.w600,
                      letterSpacing: 1.4,
                      color: t.textDim)),
              const SizedBox(height: 14),
              child,
            ],
          ),
        );
    final breath = box(
      'A minute to breathe',
      Column(
        children: [
          SizedBox(
            height: 120,
            child: AnimatedBuilder(
              animation: _breath,
              builder: (context, _) {
                final v = Curves.easeInOut.transform(_breath.value);
                return Center(
                  child: Container(
                    width: 60 + 50 * v,
                    height: 60 + 50 * v,
                    alignment: Alignment.center,
                    decoration: BoxDecoration(
                      shape: BoxShape.circle,
                      color: accent.withValues(alpha: 0.18 + 0.12 * v),
                    ),
                    child: Text(
                        _breath.status == AnimationStatus.reverse
                            ? 'out'
                            : 'in',
                        style: TextStyle(fontSize: 16, color: t.text)),
                  ),
                );
              },
            ),
          ),
          const SizedBox(height: 8),
          Tap(
            onTap: () {
              setState(() => _breathing = !_breathing);
              _syncBreath();
            },
            child: Text(_breathing ? 'Pause' : 'Breathe',
                style: TextStyle(
                    fontSize: 12.5,
                    fontWeight: FontWeight.w600,
                    color: t.textDim)),
          ),
        ],
      ),
    );
    final music = cardOn(st, 'player') && sectionOn(Section.music)
        ? _RunningEdge(
            color: accent,
            radius: 22,
            child: box('In the background', const NowPlayingCard(art: 64)),
          )
        : null;
    final q = st.quote;
    final quote = q.text.isEmpty
        ? null
        : box(
            'A line to keep',
            Column(
              children: [
                Text('“${q.text}”',
                    textAlign: TextAlign.center,
                    style: TextStyle(
                        fontSize: 17,
                        height: 1.4,
                        fontStyle: FontStyle.italic,
                        color: t.text)),
                if (q.author.isNotEmpty) ...[
                  const SizedBox(height: 8),
                  Text(q.author,
                      style: TextStyle(fontSize: 12, color: t.textDim)),
                ],
              ],
            ),
          );
    final all = [breath, if (music != null) music, if (quote != null) quote];
    // One row, every card as tall as the tallest — the player grows when its
    // lyrics open, and the other two grow with it. Safe as IntrinsicHeight
    // because the player's LayoutBuilders are boxed to a fixed height.
    return wide
        ? IntrinsicHeight(
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (var i = 0; i < all.length; i++) ...[
                  if (i > 0) const SizedBox(width: 14),
                  Expanded(child: all[i]),
                ],
              ],
            ),
          )
        : Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (var i = 0; i < all.length; i++) ...[
                if (i > 0) const SizedBox(height: 14),
                all[i],
              ],
            ],
          );
  }
}

class _Glow extends StatelessWidget {
  const _Glow({required this.color, required this.size});

  final Color color;
  final double size;

  @override
  Widget build(BuildContext context) => IgnorePointer(
        child: Container(
          width: size,
          height: size,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            gradient: RadialGradient(
              colors: [color, color.withValues(alpha: 0)],
            ),
          ),
        ),
      );
}

/// The profile photo, or the emoji, or a person, in a circle — what the
/// sidebar's profile card shows, at the clock's size.
class _Avatar extends StatelessWidget {
  const _Avatar({required this.size, required this.accent});

  final double size;
  final Color accent;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final u = ShellController.instance.state?.user;
    final emoji = u?.avatarEmoji ?? '';
    final photo = u?.avatarPath ?? '';
    final face = Container(
      width: size,
      height: size,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: accent.withValues(alpha: 0.18),
        shape: BoxShape.circle,
      ),
      child: emoji.isEmpty
          ? Icon(context.skin.icon(Icons.person_outline),
              size: size * 0.46, color: t.text)
          : Text(emoji, style: TextStyle(fontSize: size * 0.46)),
    );
    final pic = photo.isEmpty
        ? face
        : ClipOval(
            child: Image.file(
              File(photo),
              key: ValueKey(ShellController.instance.pictureEpoch),
              width: size,
              height: size,
              fit: BoxFit.cover,
              cacheWidth: decodePx(context, size),
              errorBuilder: (_, __, ___) => face,
            ),
          );
    return Hover(
      onTap: () => ShellController.instance.goTab(Section.settings, 'profile'),
      builder: (context, hovered) => AnimatedContainer(
        duration: const Duration(milliseconds: 140),
        padding: const EdgeInsets.all(3),
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          border: Border.all(
              color: hovered ? accent : Colors.transparent, width: 2),
        ),
        child: pic,
      ),
    );
  }
}

/// A light running round the card's edge while music plays; still otherwise,
/// and never with reduced motion.
class _RunningEdge extends StatefulWidget {
  const _RunningEdge(
      {required this.color, required this.radius, required this.child});

  final Color color;
  final double radius;
  final Widget child;

  @override
  State<_RunningEdge> createState() => _RunningEdgeState();
}

class _RunningEdgeState extends State<_RunningEdge>
    with SingleTickerProviderStateMixin {
  final _music = MusicController.instance;
  late final AnimationController _spin = AnimationController(
    vsync: this,
    duration: const Duration(seconds: 4),
  );

  @override
  void initState() {
    super.initState();
    _music.addListener(_sync);
    WidgetsBinding.instance.addPostFrameCallback((_) => _sync());
  }

  bool get _on => _music.tickPlaying && mounted && !context.tokens.reduceMotion;

  // The controller notifies on every state change; only a flip of playing
  // starts or stops the light, so the rest cost nothing here.
  void _sync() {
    if (!mounted) return;
    if (_on == _spin.isAnimating) return;
    setState(() => _on ? _spin.repeat() : _spin.stop());
  }

  @override
  void dispose() {
    _music.removeListener(_sync);
    _spin.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => CustomPaint(
        foregroundPainter: _spin.isAnimating
            ? _EdgePainter(_spin, widget.color, widget.radius)
            : null,
        child: widget.child,
      );
}

class _EdgePainter extends CustomPainter {
  _EdgePainter(this.turn, this.color, this.radius) : super(repaint: turn);

  final Animation<double> turn;
  final Color color;
  final double radius;

  @override
  void paint(Canvas canvas, Size size) {
    final rect = Offset.zero & size;
    final clear = color.withValues(alpha: 0);
    canvas.drawRRect(
      RRect.fromRectAndRadius(rect.deflate(1), Radius.circular(radius)),
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2
        ..shader = SweepGradient(
          transform: GradientRotation(turn.value * 2 * math.pi),
          colors: [clear, color, clear, clear],
          stops: const [0, 0.12, 0.3, 1],
        ).createShader(rect),
    );
  }

  @override
  bool shouldRepaint(_EdgePainter old) =>
      old.color != color || old.radius != radius;
}

/// The memory card's two lines, bottom left, clear of the pill.
class _Caption extends StatelessWidget {
  const _Caption(this.over, this.line);

  final String over;
  final String line;

  @override
  Widget build(BuildContext context) => Positioned(
        left: 22,
        right: 130,
        bottom: 18,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(over,
                style: const TextStyle(fontSize: 12, color: Color(0xD9FFFFFF))),
            const SizedBox(height: 2),
            Text(line,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                    fontSize: 20, height: 1.2, color: Colors.white)),
          ],
        ),
      );
}

/// The memory card's photos, one every three seconds, each fading into the
/// next; the caption follows the photo on screen.
class _Slides extends StatefulWidget {
  const _Slides({required this.photos, required this.dated, required this.now});

  final List<HomeTile> photos;
  final bool dated;
  final DateTime now;

  @override
  State<_Slides> createState() => _SlidesState();
}

class _SlidesState extends State<_Slides> {
  Timer? _timer;
  int _at = 0;

  @override
  void initState() {
    super.initState();
    _timer = Timer.periodic(const Duration(seconds: 3), (_) {
      // Offstage, or one photo only: nothing to turn.
      if (!mounted || widget.photos.length < 2) return;
      if (!TickerMode.valuesOf(context).enabled) return;
      setState(() => _at = (_at + 1) % widget.photos.length);
    });
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final p = widget.photos[_at % widget.photos.length];
    final ago = widget.now.year - (int.tryParse(p.sub) ?? widget.now.year);
    return Stack(
      fit: StackFit.expand,
      children: [
        AnimatedSwitcher(
          duration: const Duration(milliseconds: 900),
          layoutBuilder: (current, previous) => Stack(
            fit: StackFit.expand,
            children: [...previous, if (current != null) current],
          ),
          child: LazyCover(
            key: ValueKey(p.id),
            section: Section.photos,
            id: p.id,
            tint: Tokens.secPhotos,
            alignment: Alignment.topCenter,
          ),
        ),
        const DecoratedBox(
          decoration: BoxDecoration(
            gradient: LinearGradient(
              begin: Alignment.bottomCenter,
              end: Alignment.topCenter,
              colors: [Color(0x80000000), Color(0x00000000)],
              stops: [0, 0.55],
            ),
          ),
        ),
        widget.dated
            ? _Caption('On this day, ${p.sub}',
                ago == 1 ? 'A year ago today' : '$ago years ago today')
            : const _Caption(
                'From your photos', 'The latest from your library'),
      ],
    );
  }
}

/// The clock and the profile on one pane of glass: the page's glows and two
/// of its own, blurred through a tinted, lit pane with a bright rim. Colours
/// come from the accent (the skin's, leaned to the hour), so it matches
/// every theme; light and dark only change how much white is in the glass.
class _GlassClock extends StatelessWidget {
  const _GlassClock({
    required this.time,
    required this.date,
    required this.accent,
    required this.second,
  });

  final String time;
  final String date;
  final Color accent;
  final Color second;

  static const _r = 30.0;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final dark = t.dark;
    const w = Colors.white;
    return Stack(
      clipBehavior: Clip.none,
      children: [
        // Colour for the glass to bend: two blobs just behind it.
        Positioned(
          left: -30,
          top: -40,
          child: _Glow(
              color: accent.withValues(alpha: dark ? 0.55 : 0.45), size: 190),
        ),
        Positioned(
          right: -40,
          bottom: -50,
          child: _Glow(
              color: second.withValues(alpha: dark ? 0.5 : 0.4), size: 210),
        ),
        DecoratedBox(
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(_r),
            boxShadow: [
              BoxShadow(
                color: accent.withValues(alpha: dark ? 0.28 : 0.22),
                blurRadius: 48,
                offset: const Offset(0, 20),
              ),
              BoxShadow(
                color: Colors.black.withValues(alpha: dark ? 0.35 : 0.08),
                blurRadius: 18,
                offset: const Offset(0, 6),
              ),
            ],
          ),
          child: ClipRRect(
            borderRadius: BorderRadius.circular(_r),
            child: BackdropFilter(
              filter: ImageFilter.blur(sigmaX: 26, sigmaY: 26),
              child: CustomPaint(
                foregroundPainter: _Rim(accent: accent, dark: dark, radius: _r),
                child: Container(
                  padding: const EdgeInsets.fromLTRB(18, 18, 28, 18),
                  decoration: BoxDecoration(
                    gradient: LinearGradient(
                      begin: Alignment.topLeft,
                      end: Alignment.bottomRight,
                      colors: [
                        w.withValues(alpha: dark ? 0.16 : 0.62),
                        Color.lerp(w, accent, 0.35)!
                            .withValues(alpha: dark ? 0.07 : 0.34),
                        Color.lerp(w, second, 0.5)!
                            .withValues(alpha: dark ? 0.10 : 0.40),
                      ],
                      stops: const [0, 0.55, 1],
                    ),
                  ),
                  child: Stack(
                    clipBehavior: Clip.none,
                    children: [
                      // The sheen: a soft oval of light across the top, the
                      // curve a real pane of glass catches.
                      Positioned(
                        left: -40,
                        right: -40,
                        top: -70,
                        height: 110,
                        child: IgnorePointer(
                          child: DecoratedBox(
                            decoration: BoxDecoration(
                              shape: BoxShape.rectangle,
                              borderRadius: const BorderRadius.all(
                                  Radius.elliptical(400, 60)),
                              gradient: LinearGradient(
                                begin: Alignment.topCenter,
                                end: Alignment.bottomCenter,
                                colors: [
                                  w.withValues(alpha: dark ? 0.16 : 0.5),
                                  w.withValues(alpha: 0),
                                ],
                              ),
                            ),
                          ),
                        ),
                      ),
                      Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          // A ring in the two accents round the face.
                          Container(
                            padding: const EdgeInsets.all(3),
                            decoration: BoxDecoration(
                              shape: BoxShape.circle,
                              gradient: SweepGradient(
                                colors: [accent, second, accent],
                              ),
                            ),
                            child: Container(
                              padding: const EdgeInsets.all(2),
                              decoration: BoxDecoration(
                                shape: BoxShape.circle,
                                color: dark
                                    ? const Color(0x66000000)
                                    : const Color(0xB3FFFFFF),
                              ),
                              child: _Avatar(size: 76, accent: accent),
                            ),
                          ),
                          const SizedBox(width: 20),
                          Column(
                            mainAxisSize: MainAxisSize.min,
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              // The time, lit from the accent down to the ink.
                              ShaderMask(
                                blendMode: BlendMode.srcIn,
                                shaderCallback: (r) => LinearGradient(
                                  begin: Alignment.topLeft,
                                  end: Alignment.bottomRight,
                                  colors: [
                                    Color.lerp(
                                        accent, t.text, dark ? 0.1 : 0.3)!,
                                    t.text,
                                  ],
                                ).createShader(r),
                                child: Text(time,
                                    style: const TextStyle(
                                        fontSize: 64,
                                        height: 1,
                                        letterSpacing: -2,
                                        fontWeight: FontWeight.w200,
                                        fontFeatures: [
                                          FontFeature.tabularFigures()
                                        ],
                                        color: Colors.white)),
                              ),
                              const SizedBox(height: 8),
                              Text(date,
                                  style: TextStyle(
                                      fontSize: 13,
                                      fontWeight: FontWeight.w500,
                                      letterSpacing: 0.3,
                                      color: t.text.withValues(alpha: 0.72))),
                            ],
                          ),
                        ],
                      ),
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

/// The glass's edge: bright where the light comes from, top left, fading
/// round, and warmed by the accent at the far corner.
class _Rim extends CustomPainter {
  const _Rim({required this.accent, required this.dark, required this.radius});

  final Color accent;
  final bool dark;
  final double radius;

  @override
  void paint(Canvas canvas, Size size) {
    final rect = Offset.zero & size;
    const w = Colors.white;
    canvas.drawRRect(
      RRect.fromRectAndRadius(rect.deflate(0.75), Radius.circular(radius)),
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1.5
        ..shader = LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [
            w.withValues(alpha: dark ? 0.55 : 0.95),
            w.withValues(alpha: dark ? 0.08 : 0.25),
            accent.withValues(alpha: dark ? 0.55 : 0.45),
          ],
          stops: const [0, 0.5, 1],
        ).createShader(rect),
    );
  }

  @override
  bool shouldRepaint(_Rim old) =>
      old.accent != accent || old.dark != dark || old.radius != radius;
}
