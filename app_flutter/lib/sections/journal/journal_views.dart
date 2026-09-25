// Journal's other three tabs — Calendar, On this day and Insights — and the
// search results that stand in for whichever one is showing.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/journal.dart';
import 'journal_controller.dart';
import 'journal_page.dart';

// ---------------------------------------------------------------- calendar --

class CalendarView extends StatelessWidget {
  const CalendarView({super.key, required this.c, required this.st});

  final JournalController c;
  final JournalState st;

  static const _modes = [
    ('photos', 'Photos'),
    ('mood', 'Mood'),
    ('words', 'Words'),
  ];
  static const _dow = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final m = st.month;
    return ListView(
      padding: const EdgeInsets.fromLTRB(26, 20, 26, 40),
      children: [
        Row(
          children: [
            Expanded(
              child: Text(m.title,
                  style: TextStyle(
                      fontSize: 22,
                      fontWeight: FontWeight.w800,
                      color: t.nInk)),
            ),
            for (final (id, label) in _modes) ...[
              _Seg(
                label: label,
                on: m.mode == id,
                onTap: () => c.send(JournalCmd.setCalMode(mode: id)),
              ),
              const SizedBox(width: 6),
            ],
            const SizedBox(width: 8),
            IconButton(
              tooltip: 'Previous month',
              onPressed: () => c.send(const JournalCmd.setMonth(delta: -1)),
              icon: const Icon(Icons.chevron_left),
            ),
            IconButton(
              tooltip: 'Next month',
              onPressed: () => c.send(const JournalCmd.setMonth(delta: 1)),
              icon: const Icon(Icons.chevron_right),
            ),
          ],
        ),
        const SizedBox(height: 14),
        LayoutBuilder(builder: (context, box) {
          const gap = 8.0;
          final w = (box.maxWidth - gap * 6) / 7;
          final h = (w * 0.62).clamp(64.0, 110.0).toDouble();
          return Column(
            children: [
              Row(
                children: [
                  for (var i = 0; i < 7; i++) ...[
                    if (i > 0) const SizedBox(width: gap),
                    SizedBox(
                      width: w,
                      child: Text(_dow[i],
                          style: TextStyle(
                              fontSize: 11.5,
                              fontWeight: FontWeight.w600,
                              color: t.nInk3)),
                    ),
                  ],
                ],
              ),
              const SizedBox(height: 8),
              Wrap(
                spacing: gap,
                runSpacing: gap,
                children: [
                  for (var i = 0; i < m.lead; i++) SizedBox(width: w, height: h),
                  for (final d in m.cells)
                    SizedBox(
                      width: w,
                      height: h,
                      child: _Cell(c: c, d: d, mode: m.mode),
                    ),
                ],
              ),
            ],
          );
        }),
        const SizedBox(height: 16),
        Row(
          children: [
            for (var i = 1; i <= 5; i++) ...[
              _Dot(color: moodColor(i)!),
              const SizedBox(width: 5),
              Text(moodLabels[i - 1],
                  style: TextStyle(fontSize: 12, color: t.nInk2)),
              const SizedBox(width: 12),
            ],
            const Spacer(),
            Text(m.summary, style: TextStyle(fontSize: 12, color: t.nInk3)),
          ],
        ),
      ],
    );
  }
}

class _Seg extends StatelessWidget {
  const _Seg({required this.label, required this.on, required this.onTap});

  final String label;
  final bool on;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: on ? kJournal.withValues(alpha: 0.16) : t.nChip,
      borderRadius: BorderRadius.circular(99),
      child: InkWell(
        borderRadius: BorderRadius.circular(99),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
          child: Text(label,
              style: TextStyle(
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                  color: on ? kJournal2 : t.nInk2)),
        ),
      ),
    );
  }
}

class _Dot extends StatelessWidget {
  const _Dot({required this.color, this.ring});

  final Color color;
  final Color? ring;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 9,
      height: 9,
      decoration: BoxDecoration(
        color: color,
        shape: BoxShape.circle,
        border: ring == null ? null : Border.all(color: ring!, width: 1.5),
      ),
    );
  }
}

class _Cell extends StatelessWidget {
  const _Cell({required this.c, required this.d, required this.mode});

  final JournalController c;
  final DayCell d;
  final String mode;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final r = BorderRadius.circular(10);
    if (d.future) {
      return Container(
        padding: const EdgeInsets.all(8),
        decoration: BoxDecoration(
          borderRadius: r,
          border: Border.all(color: t.nHair),
        ),
        child: Text('${d.n}',
            style: TextStyle(
                fontSize: 12,
                fontWeight: FontWeight.w700,
                color: t.nInk3.withValues(alpha: 0.5))),
      );
    }
    final mood = moodColor(d.mood);
    final photo = mode == 'photos' && d.photo != 0;
    final Color? fill = switch (mode) {
      'mood' => mood?.withValues(alpha: 0.22),
      'words' => d.words > 0
          ? kJournal.withValues(
              alpha: 0.08 + (d.words / 600).clamp(0.0, 1.0).toDouble() * 0.5)
          : null,
      _ => null,
    };
    final ink = photo ? Colors.white : t.nInk;
    final label = d.written
        ? (d.words > 0 ? plural(d.words, 'word') : moodLabels[d.mood - 1])
        : 'not written';
    return Material(
      color: fill ?? t.nChip,
      borderRadius: r,
      clipBehavior: Clip.antiAlias,
      child: InkWell(
        onTap: () => c.send(JournalCmd.go(day: d.day)),
        child: Stack(
          fit: StackFit.expand,
          children: [
            if (photo) ...[
              PhotoThumb(id: d.photo, size: 0),
              const DecoratedBox(
                decoration: BoxDecoration(
                  gradient: LinearGradient(
                    begin: Alignment.topCenter,
                    end: Alignment.bottomCenter,
                    colors: [Color(0x33000000), Color(0x00000000), Color(0x88000000)],
                  ),
                ),
              ),
            ],
            if (d.today)
              DecoratedBox(
                decoration: BoxDecoration(
                  borderRadius: r,
                  border: Border.all(color: kJournal2, width: 2),
                ),
              ),
            Padding(
              padding: const EdgeInsets.all(8),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Text('${d.n}',
                          style: TextStyle(
                              fontSize: 12.5,
                              fontWeight: FontWeight.w800,
                              color: ink)),
                      const Spacer(),
                      if (mood != null) _Dot(color: mood, ring: Colors.white),
                    ],
                  ),
                  const Spacer(),
                  Text(label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 11,
                          color: photo
                              ? Colors.white.withValues(alpha: 0.9)
                              : d.written
                                  ? t.nInk2
                                  : t.nInk3)),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ------------------------------------------------------------- on this day --

class OtdView extends StatelessWidget {
  const OtdView({super.key, required this.c, required this.st});

  final JournalController c;
  final JournalState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.fromLTRB(26, 22, 26, 40),
      children: [
        Text('${dayMonth(st.day)}, other years',
            style: TextStyle(
                fontSize: 19, fontWeight: FontWeight.w800, color: t.nInk)),
        const SizedBox(height: 16),
        if (st.otd.isEmpty)
          const _Quiet(
            icon: Icons.history_outlined,
            title: 'Nothing from this day in other years yet',
            body:
                'Entries, photos and music from this date will show here as the years go by.',
          )
        else
          Grid(
            min: 300,
            children: [for (final o in st.otd) _OtdCardView(c: c, o: o)],
          ),
      ],
    );
  }
}

class _OtdCardView extends StatelessWidget {
  const _OtdCardView({required this.c, required this.o});

  final JournalController c;
  final OtdCard o;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final ago = o.yearsAgo == 1 ? '1 year ago' : '${o.yearsAgo} years ago';
    return Container(
      clipBehavior: Clip.antiAlias,
      decoration: cardDeco(context),
      child: InkWell(
        onTap: () => c.send(JournalCmd.go(day: o.day)),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            SizedBox(height: 132, child: _Mosaic(ids: ints(o.photos))),
            Padding(
              padding: const EdgeInsets.fromLTRB(14, 12, 14, 14),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Container(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
                    decoration: BoxDecoration(
                      color: kJournal.withValues(alpha: 0.14),
                      borderRadius: BorderRadius.circular(6),
                    ),
                    child: Text('$ago · ${o.year}',
                        style: const TextStyle(
                            fontSize: 11,
                            fontWeight: FontWeight.w700,
                            color: kJournal2)),
                  ),
                  const SizedBox(height: 8),
                  Text(o.written ? '“${o.text}”' : o.text,
                      style: TextStyle(
                          fontFamily: kSerif,
                          fontSize: 15,
                          height: 1.45,
                          color: t.nInk)),
                  if (o.meta.isNotEmpty) ...[
                    const SizedBox(height: 8),
                    Text(o.meta.join(' · '),
                        style: TextStyle(fontSize: 12, color: t.nInk3)),
                  ],
                  if (!o.written) ...[
                    const SizedBox(height: 10),
                    OutlinedButton.icon(
                      style: OutlinedButton.styleFrom(
                          visualDensity: VisualDensity.compact),
                      onPressed: () =>
                          c.send(JournalCmd.writeOn(day: o.day)),
                      icon: const Icon(Icons.edit_outlined, size: 15),
                      label: const Text('Write it now'),
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

/// One large photo and two small ones, as the deck lays them out; fewer
/// photos fill what they can, and none is the journal's own gradient.
class _Mosaic extends StatelessWidget {
  const _Mosaic({required this.ids});

  final List<int> ids;

  @override
  Widget build(BuildContext context) {
    if (ids.isEmpty) {
      return const DecoratedBox(
        decoration: BoxDecoration(
          gradient: LinearGradient(
            colors: [Color(0xFFFCD34D), kJournal2],
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
          ),
        ),
      );
    }
    if (ids.length < 3) {
      return Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (var i = 0; i < ids.length; i++) ...[
            if (i > 0) const SizedBox(width: 2),
            Expanded(child: PhotoThumb(id: ids[i], size: 0)),
          ],
        ],
      );
    }
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(flex: 2, child: PhotoThumb(id: ids[0], size: 0)),
        const SizedBox(width: 2),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(child: PhotoThumb(id: ids[1], size: 0)),
              const SizedBox(height: 2),
              Expanded(child: PhotoThumb(id: ids[2], size: 0)),
            ],
          ),
        ),
      ],
    );
  }
}

// ---------------------------------------------------------------- insights --

class InsightsView extends StatelessWidget {
  const InsightsView({super.key, required this.c, required this.st});

  final JournalController c;
  final JournalState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final i = st.insights;
    final moods = ints(i.moods);
    final up = i.wordsChange >= 0;
    return ListView(
      padding: const EdgeInsets.fromLTRB(26, 22, 26, 40),
      children: [
        Grid(
          min: 210,
          children: [
            _Stat(
              label: 'Words this month',
              value: thousands(i.wordsMonth),
              foot: i.hasChange
                  ? _Change(
                      text: '${i.wordsChange.abs()}% ${up ? 'up' : 'down'}',
                      up: up)
                  : const _Foot('nothing last month to compare'),
            ),
            _Stat(
              label: 'Days written',
              value: '${i.daysWritten} / ${i.daysSoFar}',
              foot: _Foot(i.streak > 0
                  ? '${i.streak}-day streak'
                  : 'no streak running'),
            ),
            _Stat(
              label: 'New places',
              value: '${i.newPlaces}',
              foot: _Foot(i.topPlace.isEmpty
                  ? 'name a place in an entry'
                  : 'most: ${i.topPlace}'),
            ),
            _Stat(
              label: 'Photos kept in entries',
              value: thousands(i.photosKept),
              foot: _Foot('of ${thousands(i.photosTaken)} taken'),
            ),
          ],
        ),
        const SizedBox(height: 14),
        Container(
          decoration: cardDeco(context),
          padding: const EdgeInsets.fromLTRB(14, 12, 14, 14),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  Text('Mood, last 30 days',
                      style: TextStyle(
                          fontSize: 13,
                          fontWeight: FontWeight.w700,
                          color: t.nInk)),
                  const Spacer(),
                  Text(i.moodLine,
                      style: TextStyle(fontSize: 12, color: t.nInk3)),
                ],
              ),
              const SizedBox(height: 14),
              SizedBox(
                height: 96,
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.end,
                  children: [
                    for (var k = 0; k < moods.length; k++) ...[
                      if (k > 0) const SizedBox(width: 5),
                      Expanded(
                        child: Tooltip(
                          message: moods[k] == 0
                              ? 'No mood'
                              : moodLabels[moods[k] - 1],
                          child: Container(
                            height: moods[k] == 0 ? 4 : 96 * moods[k] / 5,
                            decoration: BoxDecoration(
                              color: moodColor(moods[k]) ??
                                  t.nInk3.withValues(alpha: 0.2),
                              borderRadius: const BorderRadius.vertical(
                                  top: Radius.circular(4)),
                            ),
                          ),
                        ),
                      ),
                    ],
                  ],
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: 14),
        Grid(
          min: 280,
          children: [
            for (final card in i.cards)
              _Insight(
                icon: card.kind == 'music'
                    ? Icons.music_note_outlined
                    : Icons.photo_outlined,
                tint: card.kind == 'music' ? Tokens.secMusic : Tokens.secPhotos,
                title: card.title,
                child: Text(card.body,
                    style: TextStyle(
                        fontSize: 12.5, height: 1.4, color: t.nInk2)),
              ),
            if (i.tags.isNotEmpty)
              _Insight(
                icon: Icons.sell_outlined,
                tint: kJournal2,
                title: 'Most used tags',
                child: Wrap(
                  spacing: 6,
                  runSpacing: 6,
                  children: [
                    for (final tag in i.tags)
                      Container(
                        padding: const EdgeInsets.symmetric(
                            horizontal: 8, vertical: 4),
                        decoration: BoxDecoration(
                          color: t.nChip,
                          borderRadius: BorderRadius.circular(99),
                        ),
                        child: Text.rich(TextSpan(children: [
                          TextSpan(
                              text: '#${tag.tag} ',
                              style: TextStyle(
                                  fontSize: 12,
                                  fontWeight: FontWeight.w600,
                                  color: t.nInk)),
                          TextSpan(
                              text: '${tag.n}',
                              style: TextStyle(
                                  fontSize: 11, color: t.nInk3)),
                        ])),
                      ),
                  ],
                ),
              ),
          ],
        ),
        if (i.cards.isEmpty && i.tags.isEmpty)
          Text(
              'Patterns show up after a few weeks of entries with moods — which music your best days had, whether photos mean more words.',
              style: TextStyle(fontSize: 12.5, color: t.nInk3)),
        const SizedBox(height: 18),
        Text(
            'Worked out on this computer from your own entries and sections. Nothing is sent anywhere.',
            style: TextStyle(fontSize: 12, color: t.nInk3)),
      ],
    );
  }
}

class _Stat extends StatelessWidget {
  const _Stat({required this.label, required this.value, required this.foot});

  final String label;
  final String value;
  final Widget foot;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      decoration: cardDeco(context),
      padding: const EdgeInsets.fromLTRB(14, 12, 14, 14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(label, style: TextStyle(fontSize: 12, color: t.nInk3)),
          const SizedBox(height: 4),
          Text(value,
              style: TextStyle(
                  fontSize: 24, fontWeight: FontWeight.w800, color: t.nInk)),
          const SizedBox(height: 6),
          foot,
        ],
      ),
    );
  }
}

class _Foot extends StatelessWidget {
  const _Foot(this.text);

  final String text;

  @override
  Widget build(BuildContext context) =>
      Text(text, style: TextStyle(fontSize: 12, color: context.tokens.nInk3));
}

class _Change extends StatelessWidget {
  const _Change({required this.text, required this.up});

  final String text;
  final bool up;

  @override
  Widget build(BuildContext context) {
    final tint = up ? Tokens.ok : context.tokens.nInk3;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
      decoration: BoxDecoration(
        color: tint.withValues(alpha: 0.12),
        borderRadius: BorderRadius.circular(6),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(up ? Icons.trending_up : Icons.trending_down,
              size: 13, color: tint),
          const SizedBox(width: 4),
          Text(text,
              style: TextStyle(
                  fontSize: 11, fontWeight: FontWeight.w700, color: tint)),
        ],
      ),
    );
  }
}

class _Insight extends StatelessWidget {
  const _Insight({
    required this.icon,
    required this.tint,
    required this.title,
    required this.child,
  });

  final IconData icon;
  final Color tint;
  final String title;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      decoration: cardDeco(context),
      padding: const EdgeInsets.all(12),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 28,
            height: 28,
            decoration: BoxDecoration(
              color: tint.withValues(alpha: 0.14),
              borderRadius: BorderRadius.circular(8),
            ),
            child: Icon(icon, size: 16, color: tint),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(title,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
                const SizedBox(height: 4),
                child,
              ],
            ),
          ),
        ],
      ),
    );
  }
}

// ------------------------------------------------------------------ search --

class SearchView extends StatelessWidget {
  const SearchView({super.key, required this.c, required this.st});

  final JournalController c;
  final JournalState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.fromLTRB(26, 22, 26, 40),
      children: [
        H2(
          title: 'Entries with “${st.query}”',
          hint: plural(st.results.length, 'result'),
        ),
        const SizedBox(height: 14),
        if (st.results.isEmpty)
          const _Quiet(
            icon: Icons.search_off,
            title: 'Nothing matches',
            body: 'Search looks through what you wrote, places and tags.',
          ),
        for (final h in st.results) ...[
          Container(
            clipBehavior: Clip.antiAlias,
            decoration: cardDeco(context),
            child: InkWell(
              onTap: () => c.send(JournalCmd.go(day: h.day)),
              child: Padding(
                padding: const EdgeInsets.fromLTRB(14, 12, 14, 12),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Text(h.label,
                            style: TextStyle(
                                fontSize: 12,
                                fontWeight: FontWeight.w700,
                                color: t.nInk2)),
                        if (moodColor(h.mood) != null) ...[
                          const SizedBox(width: 8),
                          _Dot(color: moodColor(h.mood)!),
                        ],
                      ],
                    ),
                    const SizedBox(height: 6),
                    Text(h.excerpt,
                        style: TextStyle(
                            fontFamily: kSerif,
                            fontSize: 14.5,
                            height: 1.45,
                            color: t.nInk)),
                  ],
                ),
              ),
            ),
          ),
          const SizedBox(height: 10),
        ],
      ],
    );
  }
}

// ------------------------------------------------------------------ shared --

/// Cards in columns of at least [min] wide, filling the row.
class Grid extends StatelessWidget {
  const Grid({super.key, required this.children, required this.min, this.gap = 14});

  final List<Widget> children;
  final double min;
  final double gap;

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, box) {
        final cols = ((box.maxWidth + gap) / (min + gap)).floor().clamp(1, 99);
        final w = (box.maxWidth - gap * (cols - 1)) / cols;
        return Wrap(
          spacing: gap,
          runSpacing: gap,
          children: [
            for (final c in children) SizedBox(width: w, child: c),
          ],
        );
      },
    );
  }
}

class _Quiet extends StatelessWidget {
  const _Quiet({required this.icon, required this.title, required this.body});

  final IconData icon;
  final String title;
  final String body;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(28),
      decoration: cardDeco(context),
      child: Column(
        children: [
          Icon(icon, size: 30, color: t.nInk3),
          const SizedBox(height: 10),
          Text(title,
              style: TextStyle(
                  fontSize: 15, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(height: 6),
          Text(body,
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 12.5, height: 1.5, color: t.nInk2)),
        ],
      ),
    );
  }
}
