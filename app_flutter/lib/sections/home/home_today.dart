// Home · Today — "what now?" for every section at once.
//
// docs/mockups/NewSections/home-today-deck.html. A hero that follows the hour,
// a Needs you strip of what is waiting, one Continue rail for everything
// half-finished, and a tile per section that is on, ranked by the time of day.
//
// ponytail: the deck's edit mode (drag, resize, pin) and its Ctrl K search are
// not here. Tiles rank by the hour alone; pinning needs a `home.today.pins`
// setting, and search an index over every section.

import 'package:flutter/material.dart';

import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../shell/sidebar.dart';
import '../../src/rust/api/home.dart';
import '../music/music_controller.dart';
import 'home_controller.dart';
import 'home_modern.dart';
import 'home_shared.dart';

/// Which sections lead at which hour. Sections not named keep the sidebar's
/// order after these.
const Map<DayPart, List<Section>> _rank = {
  DayPart.morning: [
    Section.feeds,
    Section.journal,
    Section.finances,
    Section.papers,
    Section.music,
    Section.photos,
  ],
  DayPart.day: [
    Section.papers,
    Section.tools,
    Section.cloud,
    Section.transfer,
    Section.finances,
    Section.photos,
  ],
  DayPart.evening: [
    Section.videos,
    Section.music,
    Section.kitchen,
    Section.photos,
    Section.books,
    Section.arcade,
  ],
  DayPart.night: [
    Section.books,
    Section.music,
    Section.journal,
    Section.videos,
  ],
};

class TodayHome extends StatelessWidget {
  const TodayHome({super.key, required this.controller, required this.state});

  final HomeController controller;
  final HomeState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = state;
    final part = dayPart();
    final rows = cardOn(st, 'continue') ? st.continueRows : <HomeContinue>[];
    final needs = _needs(st);
    final on = ShellController.instance.sections
        .where((s) => s != Section.home && s != Section.settings)
        .toList();
    final lead = _rank[part]!.where(on.contains).toList();
    final tiles = [...lead, ...on.where((s) => !lead.contains(s))];

    return LayoutBuilder(builder: (context, box) {
      final pad = box.maxWidth >= 700 ? 30.0 : 18.0;
      return SingleChildScrollView(
        padding: EdgeInsets.fromLTRB(pad, 24, pad, 36),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
                'Good ${dayPartName(part).toLowerCase()}'
                '${firstName(st).isEmpty ? '' : ', ${firstName(st)}'}',
                style: TextStyle(
                    fontSize: 24,
                    fontWeight: FontWeight.w800,
                    letterSpacing: -0.4,
                    color: t.text)),
            Text(st.dateLine,
                style: TextStyle(fontSize: 12.5, color: t.textDim)),
            const SizedBox(height: 18),
            if (cardOn(st, 'hero')) ...[
              _Hero(st: st, part: part, needs: needs.length, rows: rows.length),
              const SizedBox(height: 26),
            ],
            ModernHead('Needs you',
                note: needs.isEmpty ? 'nothing today' : '${needs.length}'),
            if (needs.isEmpty)
              Text('Nothing is waiting on you.',
                  style: TextStyle(fontSize: 13, color: t.textDim))
            else
              ModernRail(
                height: 92,
                gap: 12,
                children: [for (final n in needs) _NeedCard(need: n)],
              ),
            if (rows.isNotEmpty) ...[
              const SizedBox(height: 26),
              const ModernHead('Continue', note: 'wherever you stopped'),
              ModernRail(
                height: 86,
                gap: 12,
                children: [for (final r in rows) _ContinueChip(row: r)],
              ),
            ],
            const SizedBox(height: 26),
            ModernHead('Your sections',
                note: 'ordered for the ${dayPartName(part).toLowerCase()}'),
            _Bento(st: st, tiles: tiles, width: box.maxWidth - pad * 2),
          ],
        ),
      );
    });
  }
}

typedef _Need = ({
  String title,
  String where,
  Color color,
  IconData icon,
  bool late,
  VoidCallback go,
});

List<_Need> _needs(HomeState st) {
  void to(Section s) => ShellController.instance.go(s);
  return [
    for (final d in st.finDues)
      (
        title: '${d.name} · ${d.amount}',
        where: d.late_ ? 'Overdue since ${d.due}' : 'Due ${d.due}',
        color: Tokens.secFinances,
        icon: Icons.receipt_long_outlined,
        late: d.late_,
        go: () => to(Section.finances),
      ),
    if (st.counts.toolsRunning > 0 || st.counts.toolsQueued > 0)
      (
        title: [
          if (st.counts.toolsRunning > 0)
            '${plural(st.counts.toolsRunning, 'job')} running',
          if (st.counts.toolsQueued > 0)
            '${count(st.counts.toolsQueued)} queued',
        ].join(', '),
        where: 'Tools',
        color: Tokens.secTools,
        icon: Icons.build_outlined,
        late: false,
        go: () => to(Section.tools),
      ),
  ];
}

// ── hero ────────────────────────────────────────────────────────────────────

class _Hero extends StatelessWidget {
  const _Hero({
    required this.st,
    required this.part,
    required this.needs,
    required this.rows,
  });

  final HomeState st;
  final DayPart part;
  final int needs;
  final int rows;

  @override
  Widget build(BuildContext context) {
    final photo =
        sectionOn(Section.photos) ? st.recentPhotos.firstOrNull : null;
    final summary = needs == 0 && rows == 0
        ? 'A quiet day. Nothing waiting, nothing half-done.'
        : [
            if (needs > 0)
              '${plural(needs, 'thing')} need${needs == 1 ? 's' : ''} you',
            if (rows > 0) '${count(rows)} to pick up again',
          ].join(' · ');
    final chips = [
      for (final s in (_rank[part] ?? const <Section>[]))
        if (sectionOn(s)) s,
    ].take(4);
    return ClipRRect(
      borderRadius: BorderRadius.circular(22),
      child: SizedBox(
        height: 240,
        child: Stack(
          fit: StackFit.expand,
          children: [
            if (photo != null)
              LazyCover(
                section: Section.photos,
                id: photo.id,
                tint: Tokens.secPhotos,
                alignment: Alignment.topCenter,
              )
            else
              const DecoratedBox(
                decoration: BoxDecoration(
                  gradient: LinearGradient(
                    begin: Alignment.topLeft,
                    end: Alignment.bottomRight,
                    colors: [Tokens.brand2, Tokens.brand, Tokens.secMusic],
                  ),
                ),
              ),
            const DecoratedBox(
              decoration: BoxDecoration(
                gradient: LinearGradient(
                  colors: [Color(0xE6050614), Color(0x66050614)],
                ),
              ),
            ),
            Padding(
              padding: const EdgeInsets.all(26),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisAlignment: MainAxisAlignment.end,
                children: [
                  Text(dayPartName(part).toUpperCase(),
                      style: const TextStyle(
                          fontSize: 11,
                          fontWeight: FontWeight.w800,
                          letterSpacing: 1.6,
                          color: Color(0xB3FFFFFF))),
                  const SizedBox(height: 8),
                  Text(summary,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                          fontSize: 28,
                          height: 1.12,
                          fontWeight: FontWeight.w800,
                          letterSpacing: -0.5,
                          color: Colors.white)),
                  if (st.libraryLine.isNotEmpty) ...[
                    const SizedBox(height: 6),
                    Text(st.libraryLine,
                        style: const TextStyle(
                            fontSize: 12.5, color: Color(0xB3FFFFFF))),
                  ],
                  const SizedBox(height: 16),
                  Wrap(
                    spacing: 8,
                    runSpacing: 8,
                    children: [
                      for (final s in chips)
                        PillBtn(
                          label: kSectionMeta[s]?.label ?? s.name,
                          icon: kSectionMeta[s]?.icon,
                          solid: false,
                          onTap: () => ShellController.instance.go(s),
                        ),
                    ],
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ── needs you ───────────────────────────────────────────────────────────────

class _NeedCard extends StatelessWidget {
  const _NeedCard({required this.need});

  final _Need need;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = need.late ? Tokens.warn : need.color;
    return SizedBox(
      width: 280,
      child: PanelCard(
        padding: 14,
        onTap: need.go,
        child: Row(
          children: [
            Container(
              width: 38,
              height: 38,
              decoration: BoxDecoration(
                color: c.withValues(alpha: 0.16),
                borderRadius: BorderRadius.circular(11),
              ),
              child: Icon(need.icon, size: 19, color: c),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  Text(need.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 13.5,
                          fontWeight: FontWeight.w700,
                          color: t.text)),
                  Text(need.where,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 12,
                          color: need.late ? Tokens.warn : t.textDim)),
                ],
              ),
            ),
            Icon(Icons.chevron_right, size: 18, color: t.textDim),
          ],
        ),
      ),
    );
  }
}

// ── continue ────────────────────────────────────────────────────────────────

class _ContinueChip extends StatelessWidget {
  const _ContinueChip({required this.row});

  final HomeContinue row;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      width: 270,
      child: PanelCard(
        padding: 10,
        onTap: () => resumeRow(row),
        child: Row(
          children: [
            ClipRRect(
              borderRadius: BorderRadius.circular(10),
              child: SizedBox(width: 62, height: 62, child: RowCover(row: row)),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  Text(row.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 13.5,
                          fontWeight: FontWeight.w700,
                          color: t.text)),
                  Text(row.sub.isEmpty ? row.author : row.sub,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 11.5, color: t.textDim)),
                  const SizedBox(height: 7),
                  ThinBar(frac: row.frac, color: kindColor(row.kind)),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ── the tiles ───────────────────────────────────────────────────────────────

class _Bento extends StatelessWidget {
  const _Bento({required this.st, required this.tiles, required this.width});

  final HomeState st;
  final List<Section> tiles;
  final double width;

  @override
  Widget build(BuildContext context) {
    final cols = width >= 1200
        ? 4
        : width >= 800
            ? 3
            : 2;
    const gap = 12.0;
    final w = (width - gap * (cols - 1)) / cols;
    return Wrap(
      spacing: gap,
      runSpacing: gap,
      children: [
        for (var i = 0; i < tiles.length; i++)
          SizedBox(
            // The two the hour leads with get double width.
            width: i < 2 && cols > 2 ? w * 2 + gap : w,
            height: 150,
            child: _Tile(st: st, s: tiles[i], big: i < 2 && cols > 2),
          ),
      ],
    );
  }
}

class _Tile extends StatelessWidget {
  const _Tile({required this.st, required this.s, required this.big});

  final HomeState st;
  final Section s;
  final bool big;

  /// Up to three pictures for the sections Home has pictures for.
  List<Widget> _strip() {
    List<Widget> covers(Section sec, List<HomeTile> list, IconData icon) => [
          for (final x in list.take(big ? 4 : 3))
            LazyCover(
              section: sec,
              id: x.id,
              tint: Tokens.accentOf(sec),
              icon: icon,
              alignment: sec == Section.photos
                  ? Alignment.topCenter
                  : Alignment.center,
            ),
        ];
    return switch (s) {
      Section.photos =>
        covers(Section.photos, st.recentPhotos, Icons.image_outlined),
      Section.videos =>
        covers(Section.videos, st.recentVideos, Icons.movie_outlined),
      Section.books =>
        covers(Section.books, st.recentBooks, Icons.menu_book_outlined),
      Section.music => covers(Section.music, st.recentSongs, Icons.music_note),
      _ => const [],
    };
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = Tokens.accentOf(s);
    final meta = kSectionMeta[s];
    final strip = _strip();
    final now = s == Section.music ? MusicController.instance.now : null;
    final line = now != null && now.loaded && now.title.isNotEmpty
        ? 'Playing · ${now.title}'
        : sectionLine(s, st);
    return Tap(
      onTap: () => ShellController.instance.go(s),
      child: Container(
        padding: const EdgeInsets.all(14),
        decoration: context.skin.surface(SurfaceRole.card, radius: 18) ??
            BoxDecoration(
              borderRadius: BorderRadius.circular(18),
              border: Border.all(color: t.outline),
              gradient: LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: [
                  Color.alphaBlend(c.withValues(alpha: 0.10), t.panel),
                  t.panel
                ],
                stops: const [0, 0.7],
              ),
            ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Container(
                  width: 32,
                  height: 32,
                  decoration: BoxDecoration(
                    color: c.withValues(alpha: 0.18),
                    borderRadius: BorderRadius.circular(10),
                  ),
                  child: Icon(meta?.icon ?? Icons.circle_outlined,
                      size: 17, color: c),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: Text(meta?.label ?? s.name,
                      style: TextStyle(
                          fontSize: 14,
                          fontWeight: FontWeight.w700,
                          color: t.text)),
                ),
              ],
            ),
            const SizedBox(height: 10),
            if (line.isNotEmpty)
              Text(line,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 12.5, color: t.textDim)),
            const Spacer(),
            if (strip.isNotEmpty)
              SizedBox(
                height: 52,
                child: Row(
                  children: [
                    for (var i = 0; i < strip.length; i++) ...[
                      if (i > 0) const SizedBox(width: 6),
                      Expanded(
                        child: ClipRRect(
                          borderRadius: BorderRadius.circular(8),
                          child: strip[i],
                        ),
                      ),
                    ],
                  ],
                ),
              ),
          ],
        ),
      ),
    );
  }
}
