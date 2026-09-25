// The Places section — docs/mockups/NewSections/places-deck.html.
//
// Four tabs over one snapshot: Trips (suggestions, the kept trips, and a trip
// open as a page of days), Map (every photo's place and the trips' routes),
// Places (towns, and where you want to go) and Been (countries, states, the
// years away, and how trips are found).

import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/section_tabs.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/dialog.dart';
import '../../src/rust/api/places.dart';
import '../journal/journal_controller.dart' show sourceLook;
import '../journal/journal_page.dart' show PhotoThumb;
import '../kitchen/kitchen_page.dart' show Grid, Quiet, Strip, askLine, cardDeco, confirm;
import '../papers/papers_page.dart' show TabPill;
import 'places_controller.dart';

class PlacesPage extends StatefulWidget {
  const PlacesPage({super.key, required this.visible});

  /// On screen. Coming back asks again: Photos may have found more.
  final bool visible;

  @override
  State<PlacesPage> createState() => _PlacesPageState();
}

class _PlacesPageState extends State<PlacesPage> {
  final PlacesController _c = PlacesController();

  @override
  void initState() {
    super.initState();
    _c.refresh();
  }

  @override
  void didUpdateWidget(PlacesPage old) {
    super.didUpdateWidget(old);
    if (widget.visible && !old.visible) _c.refresh(quiet: true);
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        final st = _c.state;
        return ColoredBox(
          color: t.nCanvas,
          child: Column(
            children: [
              _Header(c: _c, st: st),
              if (_c.busy)
                const LinearProgressIndicator(minHeight: 2, color: kPlaces)
              else
                const SizedBox(height: 2),
              if (_c.notice.isNotEmpty)
                Strip(
                  icon: Icons.check_circle_outline,
                  tint: kPlaces,
                  text: _c.notice,
                  onClose: _c.dismissNotice,
                ),
              if (_c.error != null && st != null)
                Strip(
                  icon: Icons.error_outline,
                  tint: Tokens.error,
                  text: plainError(_c.error!),
                  onClose: _c.clearError,
                ),
              Expanded(
                child: st == null
                    ? FirstLoad(error: _c.error, onRetry: _c.refresh)
                    : st.located == 0
                        ? const _Empty()
                        : switch (st.tab) {
                            'map' => _MapTab(st: st),
                            'places' => _PlacesTab(c: _c, st: st),
                            'been' => _BeenTab(c: _c, st: st),
                            _ => st.open != null
                                ? _TripPage(c: _c, v: st.open!, st: st)
                                : _TripsTab(c: _c, st: st),
                          },
              ),
            ],
          ),
        );
      },
    );
  }
}

class _Empty extends StatelessWidget {
  const _Empty();

  @override
  Widget build(BuildContext context) {
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 480),
        child: const Quiet(
          icon: Icons.place_outlined,
          title: 'No photos with a location yet',
          body: 'Places finds your trips in where your photos were taken. '
              'Add a folder of phone photos in Photos, and once they are '
              'scanned, your trips show up here.',
        ),
      ),
    );
  }
}

// ------------------------------------------------------------------ header --

class _Header extends StatelessWidget {
  const _Header({required this.c, required this.st});

  final PlacesController c;
  final PlacesState? st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = this.st;
    final tab = st?.tab ?? 'trips';
    final r = context.skin.controlRadius ?? 10;
    return Container(
      height: 64,
      padding: const EdgeInsets.fromLTRB(22, 0, 20, 0),
      decoration: BoxDecoration(
        color: t.panel,
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          Container(
            width: 32,
            height: 32,
            decoration: BoxDecoration(
              color: kPlaces.withValues(alpha: 0.17),
              borderRadius: BorderRadius.circular(10),
            ),
            child: const Icon(Icons.place_outlined, size: 18, color: kPlaces),
          ),
          const SizedBox(width: 10),
          Text('Places',
              style: TextStyle(
                  fontSize: 19, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(width: 14),
          Expanded(
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: Container(
                padding: const EdgeInsets.all(3),
                decoration: BoxDecoration(
                  color: t.nChip,
                  borderRadius: BorderRadius.circular(r + 3),
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    for (final f in keepTabs('places', placesTabs, (f) => f.id,
                        active: (f) => tab == f.id))
                      TabPill(
                        label: f.label,
                        on: f.id == tab,
                        count: f.id == 'trips' ? st?.suggested.length ?? 0 : 0,
                        radius: r,
                        tint: kPlaces,
                        onTap: () => c.send(PlacesCmd.setTab(tab: f.id)),
                      ),
                  ],
                ),
              ),
            ),
          ),
          if (st != null && st.located > 0)
            Text('${st.located} of ${st.photos} photos have a place',
                style: TextStyle(fontSize: 12, color: t.nInk3)),
          if (st != null && !st.named) ...[
            const SizedBox(width: 12),
            FilledButton.icon(
              style: FilledButton.styleFrom(backgroundColor: kPlaces),
              icon: const Icon(Icons.download_outlined, size: 17),
              label: const Text('Name the towns'),
              onPressed: () => c.send(const PlacesCmd.getPlaceNames()),
            ),
          ],
        ],
      ),
    );
  }
}

// ------------------------------------------------------------------- trips --

class _TripsTab extends StatelessWidget {
  const _TripsTab({required this.c, required this.st});

  final PlacesController c;
  final PlacesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final years = {for (final x in st.trips) x.year.toInt()}.toList()..sort();
    final shown = [
      for (final x in st.trips)
        if (c.year == 0 || x.year == c.year) x
    ];
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 22, 24, 30),
      children: [
        if (!st.named)
          Padding(
            padding: const EdgeInsets.only(bottom: 16),
            child: Text(
                'Trips are named after the towns in them once the town table is '
                'here: Name the towns, at the top right (a one-time 3 MB download).',
                style: TextStyle(fontSize: 12.5, color: t.nInk2)),
          ),
        if (st.suggested.isNotEmpty) ...[
          Row(children: [
            const Icon(Icons.auto_awesome, size: 18, color: kPlaces),
            const SizedBox(width: 8),
            Expanded(
              child: Text(
                  'Found in your photos: ${plural(st.suggested.length, 'trip')}',
                  style: TextStyle(
                      fontSize: 16,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
            ),
            if (st.suggested.length > 1)
              FilledButton.icon(
                style: FilledButton.styleFrom(backgroundColor: kPlaces),
                icon: const Icon(Icons.check, size: 17),
                label: const Text('Keep them all'),
                onPressed: () => c.send(const PlacesCmd.keepAll()),
              ),
          ]),
          const SizedBox(height: 4),
          Text(
              'Photos taken more than ${st.minKm} km from ${st.home.isEmpty ? 'home' : st.home}, '
              'no more than ${plural(st.gapDays.toInt(), 'day')} apart. Keep the ones that were trips.',
              style: TextStyle(fontSize: 12.5, color: t.nInk2)),
          const SizedBox(height: 12),
          for (final s in st.suggested.take(12)) _Suggestion(c: c, s: s),
          if (st.suggested.length > 12)
            Text('…and ${st.suggested.length - 12} more, older',
                style: TextStyle(fontSize: 12.5, color: t.nInk3)),
          const SizedBox(height: 22),
        ],
        Row(children: [
          Text('Your trips',
              style: TextStyle(
                  fontSize: 16, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(width: 10),
          Text(plural(st.trips.length, 'trip'),
              style: TextStyle(fontSize: 12.5, color: t.nInk3)),
          const Spacer(),
          if (years.length > 1)
            Wrap(spacing: 6, children: [
              ChoiceChip(
                label: const Text('All'),
                selected: c.year == 0,
                selectedColor: kPlaces.withValues(alpha: 0.18),
                onSelected: (_) => c.setYear(0),
              ),
              for (final y in years.reversed.take(6))
                ChoiceChip(
                  label: Text('$y'),
                  selected: c.year == y,
                  selectedColor: kPlaces.withValues(alpha: 0.18),
                  onSelected: (_) => c.setYear(y),
                ),
            ]),
        ]),
        const SizedBox(height: 12),
        if (st.trips.isEmpty)
          Text(
              st.suggested.isEmpty
                  ? 'No trips found yet. Photos far from home, a few days apart, become one.'
                  : 'Keep a trip above and it gets a page of its own.',
              style: TextStyle(color: t.nInk3))
        else
          Grid(
            min: 230,
            children: [
              for (final x in shown)
                _TripTile(
                    x: x,
                    onTap: () => c.send(PlacesCmd.openTrip(id: x.id))),
            ],
          ),
      ],
    );
  }
}

class _Covers extends StatelessWidget {
  const _Covers({required this.ids, required this.height});

  final List<BigInt> ids;
  final double height;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (ids.isEmpty) {
      return Container(
        height: height,
        color: kPlaces.withValues(alpha: 0.15),
        child: Icon(Icons.landscape_outlined, color: t.nInk3),
      );
    }
    return SizedBox(
      height: height,
      child: Row(
        children: [
          for (var i = 0; i < ids.length; i++) ...[
            if (i > 0) const SizedBox(width: 2),
            Expanded(
              flex: i == 0 ? 2 : 1,
              child: PhotoThumb(id: ids[i].toInt(), size: 0),
            ),
          ],
        ],
      ),
    );
  }
}

class _Suggestion extends StatelessWidget {
  const _Suggestion({required this.c, required this.s});

  final PlacesController c;
  final TripCard s;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      margin: const EdgeInsets.only(bottom: 10),
      padding: const EdgeInsets.all(10),
      decoration: cardDeco(context).copyWithBorder(kPlaces),
      child: Row(
        children: [
          SizedBox(
            width: 150,
            child: ClipRRect(
              borderRadius: BorderRadius.circular(10),
              child: _Covers(ids: s.cover, height: 76),
            ),
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text('${s.title} ${s.flags}',
                    style: TextStyle(
                        fontSize: 15,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
                const SizedBox(height: 3),
                Text(
                    '${s.range} · ${plural(s.days.toInt(), 'day')} · '
                    '${plural(s.photos.toInt(), 'photo')}'
                    '${s.km > 0 ? ' · ${s.km} km' : ''}',
                    style: TextStyle(fontSize: 12.5, color: t.nInk2)),
              ],
            ),
          ),
          TextButton(
            onPressed: () =>
                c.send(PlacesCmd.dismiss(start: s.start, end: s.end)),
            child: const Text('Not a trip'),
          ),
          const SizedBox(width: 6),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: kPlaces),
            onPressed: () =>
                c.send(PlacesCmd.keepTrip(start: s.start, end: s.end)),
            child: const Text('Keep'),
          ),
        ],
      ),
    );
  }
}

extension on Decoration {
  /// A card's decoration with the section's border, for a suggestion.
  Decoration copyWithBorder(Color c) {
    final d = this;
    if (d is BoxDecoration) {
      return d.copyWith(
          border: Border.all(color: c.withValues(alpha: 0.45), width: 1.5));
    }
    return d;
  }
}

class _TripTile extends StatelessWidget {
  const _TripTile({required this.x, required this.onTap});

  final TripCard x;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final r = context.skin.panelRadius ?? 14;
    return Material(
      color: Colors.transparent,
      child: InkWell(
        borderRadius: BorderRadius.circular(r),
        onTap: onTap,
        child: Container(
          clipBehavior: Clip.antiAlias,
          decoration: cardDeco(context),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _Covers(ids: x.cover, height: 130),
              Padding(
                padding: const EdgeInsets.fromLTRB(14, 11, 14, 13),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(children: [
                      Expanded(
                        child: Text(x.title,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontSize: 15,
                                fontWeight: FontWeight.w700,
                                color: t.nInk)),
                      ),
                      Text(x.flags, style: const TextStyle(fontSize: 16)),
                    ]),
                    const SizedBox(height: 3),
                    Text(x.range,
                        style: TextStyle(fontSize: 12.5, color: t.nInk2)),
                    const SizedBox(height: 3),
                    Text(
                        '${plural(x.days.toInt(), 'day')} · ${plural(x.photos.toInt(), 'photo')}'
                        '${x.km > 0 ? ' · ${x.km} km' : ''}',
                        style: TextStyle(fontSize: 11.5, color: t.nInk3)),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

// --------------------------------------------------------------- one trip --

class _TripPage extends StatelessWidget {
  const _TripPage({required this.c, required this.v, required this.st});

  final PlacesController c;
  final TripView v;
  final PlacesState st;

  Future<void> _rename(BuildContext context) async {
    final title =
        await askLine(context, 'Rename the trip', 'Its name', initial: v.title);
    if (title != null && title.trim().isNotEmpty) {
      await c.send(PlacesCmd.renameTrip(id: v.id, title: title));
    }
  }

  Future<void> _note(BuildContext context) async {
    final note = await askLine(context, 'A note on the trip',
        'Who you went with, what it was for', initial: v.note);
    if (note != null) await c.send(PlacesCmd.setNote(id: v.id, note: note));
  }

  Future<void> _gpx() async {
    final path = await dialogSaveFile(
      title: 'Save the route',
      fileName: '${v.title.replaceAll(RegExp(r'[\\/:*?"<>|]'), '')}.gpx',
      label: 'GPX track',
      extensions: const ['gpx'],
    );
    if (path == null || path.isEmpty) return;
    try {
      await placesExportGpx(id: v.id, path: path);
      c.say('Saved $path');
    } catch (e) {
      c.say('Could not save it — ${plainError(e)}');
    }
  }

  Future<void> _remove(BuildContext context) async {
    final ok = await confirm(context, 'Forget this trip?',
        '“${v.title}” stops being a trip. Its photos stay in Photos, and it is not suggested again.');
    if (ok) await c.send(PlacesCmd.removeTrip(id: v.id));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return LayoutBuilder(builder: (context, box) {
      final wide = box.maxWidth > 980;
      final side = Column(
        children: [
          Container(
            height: 230,
            clipBehavior: Clip.antiAlias,
            decoration: cardDeco(context),
            child: CustomPaint(
              size: Size.infinite,
              painter: _GeoPainter(
                points: const [],
                routes: [v.route],
                home: st.map.home,
                ground: kPlaces.withValues(alpha: 0.06),
                dot: kPlaces,
                line: kPlaces,
                homeInk: t.nInk3,
              ),
            ),
          ),
          const SizedBox(height: 14),
          _Box(
            icon: Icons.insights_outlined,
            title: 'The trip in numbers',
            child: Wrap(spacing: 8, runSpacing: 8, children: [
              _Stat('${v.daysN}', 'days'),
              _Stat('${v.photos}', 'photos'),
              if (v.km > 0) _Stat('${v.km}', 'km between stops'),
              if (v.stops.isNotEmpty) _Stat('${v.stops.length}', 'towns'),
            ]),
          ),
          const SizedBox(height: 14),
          _Box(
            icon: Icons.ios_share,
            title: 'Keep or share it',
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (ShellController.instance.sections.contains(Section.studio)) ...[
                  FilledButton.icon(
                    style: FilledButton.styleFrom(backgroundColor: kPlaces),
                    icon: const Icon(Icons.movie_creation_outlined, size: 18),
                    label: const Text('Make a movie of it'),
                    onPressed: () => ShellController.instance
                        .goOpen(Section.studio, 'trip', '${v.id}'),
                  ),
                  const SizedBox(height: 8),
                ],
                OutlinedButton.icon(
                  icon: const Icon(Icons.edit_note, size: 18),
                  label: const Text('Write it up in Journal'),
                  onPressed: () => c.send(PlacesCmd.toJournal(id: v.id)),
                ),
                const SizedBox(height: 8),
                OutlinedButton.icon(
                  icon: const Icon(Icons.route_outlined, size: 18),
                  label: const Text('Save the route as GPX'),
                  onPressed: _gpx,
                ),
                const SizedBox(height: 8),
                OutlinedButton.icon(
                  icon: const Icon(Icons.photo_library_outlined, size: 18),
                  label: const Text('See the photos in Photos'),
                  onPressed: () => ShellController.instance.go(Section.photos),
                ),
              ],
            ),
          ),
        ],
      );
      final days = [
        for (final d in v.days) _DayCard(d: d),
      ];
      return ListView(
        padding: const EdgeInsets.fromLTRB(24, 18, 24, 30),
        children: [
          Row(children: [
            TextButton.icon(
              icon: const Icon(Icons.arrow_back, size: 17),
              label: const Text('Trips'),
              onPressed: () => c.send(const PlacesCmd.closeTrip()),
            ),
            const Spacer(),
            PopupMenuButton<String>(
              tooltip: 'More',
              onSelected: (x) => switch (x) {
                'rename' => _rename(context),
                'note' => _note(context),
                'remove' => _remove(context),
                _ => null,
              },
              itemBuilder: (_) => const [
                PopupMenuItem(value: 'rename', child: Text('Rename')),
                PopupMenuItem(value: 'note', child: Text('Add a note')),
                PopupMenuItem(value: 'remove', child: Text('Not a trip')),
              ],
            ),
          ]),
          const SizedBox(height: 6),
          InkWell(
            onTap: () => _rename(context),
            child: Text(v.title,
                style: TextStyle(
                    fontSize: 28,
                    fontWeight: FontWeight.w800,
                    letterSpacing: -0.5,
                    color: t.nInk)),
          ),
          const SizedBox(height: 4),
          Text(
              [
                v.range,
                if (v.stops.isNotEmpty) v.stops.join(' → '),
                if (v.furthest.isNotEmpty) v.furthest,
              ].join(' · '),
              style: TextStyle(fontSize: 13, color: t.nInk2)),
          if (v.note.isNotEmpty) ...[
            const SizedBox(height: 8),
            Text(v.note,
                style: TextStyle(
                    fontSize: 14, fontStyle: FontStyle.italic, color: t.nInk2)),
          ],
          const SizedBox(height: 18),
          if (wide)
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(child: Column(children: days)),
                const SizedBox(width: 18),
                SizedBox(width: 320, child: side),
              ],
            )
          else ...[
            side,
            const SizedBox(height: 16),
            ...days,
          ],
        ],
      );
    });
  }
}

class _DayCard extends StatelessWidget {
  const _DayCard({required this.d});

  final TripDay d;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      margin: const EdgeInsets.only(bottom: 12),
      padding: const EdgeInsets.fromLTRB(16, 14, 16, 14),
      decoration: cardDeco(context),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 78,
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(d.label,
                    style: const TextStyle(
                        fontSize: 18,
                        fontWeight: FontWeight.w800,
                        color: kPlaces)),
                Text(d.dow,
                    style: TextStyle(fontSize: 12, color: t.nInk2)),
              ],
            ),
          ),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(d.towns.isEmpty ? 'Somewhere' : d.towns,
                    style: TextStyle(
                        fontSize: 14.5,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
                if (d.quote.isNotEmpty)
                  Container(
                    margin: const EdgeInsets.only(top: 8),
                    padding: const EdgeInsets.only(left: 12),
                    decoration: const BoxDecoration(
                        border:
                            Border(left: BorderSide(color: kPlaces, width: 3))),
                    child: Text('“${d.quote}”',
                        style: TextStyle(
                            fontSize: 14,
                            height: 1.5,
                            fontStyle: FontStyle.italic,
                            color: t.nInk2)),
                  ),
                if (d.photos.isNotEmpty) ...[
                  const SizedBox(height: 10),
                  Wrap(spacing: 6, runSpacing: 6, children: [
                    for (final id in d.photos)
                      PhotoThumb(id: id.toInt(), size: 64),
                  ]),
                ],
                const SizedBox(height: 8),
                Wrap(spacing: 6, runSpacing: 6, children: [
                  if (d.photoN > 0)
                    _Line(
                        icon: Icons.photo_outlined,
                        tint: Tokens.secPhotos,
                        text: plural(d.photoN.toInt(), 'photo')),
                  for (final l in d.lines)
                    _Line(
                        icon: sourceLook(l.source).icon,
                        tint: sourceLook(l.source).tint,
                        text: l.text),
                ]),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _Line extends StatelessWidget {
  const _Line({required this.icon, required this.tint, required this.text});

  final IconData icon;
  final Color tint;
  final String text;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.fromLTRB(8, 4, 10, 4),
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(99),
      ),
      child: Row(mainAxisSize: MainAxisSize.min, children: [
        Icon(icon, size: 14, color: tint),
        const SizedBox(width: 6),
        Flexible(
          child: Text(text,
              style: TextStyle(fontSize: 12, color: t.nInk2),
              overflow: TextOverflow.ellipsis),
        ),
      ]),
    );
  }
}

class _Box extends StatelessWidget {
  const _Box({required this.icon, required this.title, required this.child});

  final IconData icon;
  final String title;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(16, 14, 16, 16),
      decoration: cardDeco(context),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(children: [
            Icon(icon, size: 17, color: kPlaces),
            const SizedBox(width: 8),
            Expanded(
              child: Text(title,
                  style: TextStyle(
                      fontSize: 14,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
            ),
          ]),
          const SizedBox(height: 10),
          child,
        ],
      ),
    );
  }
}

class _Stat extends StatelessWidget {
  const _Stat(this.value, this.label);

  final String value;
  final String label;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      constraints: const BoxConstraints(minWidth: 110),
      padding: const EdgeInsets.fromLTRB(12, 10, 12, 10),
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(10),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(value,
              style: TextStyle(
                  fontSize: 20, fontWeight: FontWeight.w800, color: t.nInk)),
          Text(label, style: TextStyle(fontSize: 11.5, color: t.nInk3)),
        ],
      ),
    );
  }
}

// --------------------------------------------------------------------- map --

class _MapTab extends StatelessWidget {
  const _MapTab({required this.st});

  final PlacesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final m = st.map;
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 18, 24, 24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
              '${plural(st.located.toInt(), 'photo')} with a place'
              '${m.firstYear > 0 ? ', ${m.firstYear}–${m.lastYear}' : ''} · '
              '${plural(m.routes.length, 'trip route')}',
              style: TextStyle(fontSize: 12.5, color: t.nInk2)),
          const SizedBox(height: 10),
          Expanded(
            child: Container(
              clipBehavior: Clip.antiAlias,
              decoration: cardDeco(context),
              child: CustomPaint(
                size: Size.infinite,
                painter: _GeoPainter(
                  points: m.points,
                  routes: [for (final r in m.routes) r.points],
                  home: m.home,
                  ground: kPlaces.withValues(alpha: 0.05),
                  dot: kPlaces,
                  line: kPlaces2,
                  homeInk: t.nInk,
                ),
              ),
            ),
          ),
          const SizedBox(height: 8),
          Text(
              'Drawn from the photos alone: a dot where photos were taken, bigger '
              'where there are more, and a line for each kept trip. No map is '
              'downloaded and nothing leaves this computer.',
              style: TextStyle(fontSize: 11.5, color: t.nInk3)),
        ],
      ),
    );
  }
}

/// Places on a flat projection fitted to what is drawn: latitude up, longitude
/// across, squeezed by the middle latitude's cosine so distances look right.
class _GeoPainter extends CustomPainter {
  _GeoPainter({
    required this.points,
    required this.routes,
    required this.home,
    required this.ground,
    required this.dot,
    required this.line,
    required this.homeInk,
  });

  final List<GeoPoint> points;
  final List<List<GeoPoint>> routes;
  final GeoPoint? home;
  final Color ground;
  final Color dot;
  final Color line;
  final Color homeInk;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawRect(Offset.zero & size, Paint()..color = ground);
    final all = [
      ...points,
      for (final r in routes) ...r,
      if (home != null) home!,
    ];
    if (all.isEmpty) return;
    var minLat = all.first.lat, maxLat = all.first.lat;
    var minLon = all.first.lon, maxLon = all.first.lon;
    for (final p in all) {
      minLat = math.min(minLat, p.lat);
      maxLat = math.max(maxLat, p.lat);
      minLon = math.min(minLon, p.lon);
      maxLon = math.max(maxLon, p.lon);
    }
    final k = math.cos((minLat + maxLat) / 2 * math.pi / 180)
        .abs()
        .clamp(0.2, 1.0)
        .toDouble();
    final spanX = math.max((maxLon - minLon) * k, 0.02);
    final spanY = math.max(maxLat - minLat, 0.02);
    const pad = 28.0;
    final scale = math.min((size.width - pad * 2) / spanX,
        (size.height - pad * 2) / spanY);
    final ox = (size.width - spanX * scale) / 2;
    final oy = (size.height - spanY * scale) / 2;
    Offset at(GeoPoint p) => Offset(ox + (p.lon - minLon) * k * scale,
        oy + (maxLat - p.lat) * scale);

    final maxN = points.fold<int>(1, (m, p) => math.max(m, p.n.toInt()));
    for (final p in points) {
      final r = 2.5 + 9 * math.sqrt(p.n.toInt() / maxN);
      canvas.drawCircle(at(p), r, Paint()..color = dot.withValues(alpha: 0.30));
      canvas.drawCircle(at(p), math.min(r, 2.5), Paint()..color = dot);
    }
    final dash = Paint()
      ..color = line
      ..strokeWidth = 2.4
      ..strokeCap = StrokeCap.round;
    for (final r in routes) {
      for (var i = 0; i + 1 < r.length; i++) {
        final a = at(r[i]), b = at(r[i + 1]);
        final len = (b - a).distance;
        if (len == 0) continue;
        final dir = (b - a) / len;
        for (var d = 0.0; d < len; d += 9) {
          canvas.drawLine(a + dir * d, a + dir * math.min(d + 5, len), dash);
        }
      }
      for (final p in r) {
        canvas.drawCircle(at(p), 5, Paint()..color = Colors.white);
        canvas.drawCircle(at(p), 3.5, Paint()..color = line);
      }
    }
    if (home != null) {
      final h = at(home!);
      canvas.drawCircle(h, 8, Paint()..color = homeInk.withValues(alpha: 0.2));
      canvas.drawCircle(
          h,
          5,
          Paint()
            ..color = homeInk
            ..style = PaintingStyle.stroke
            ..strokeWidth = 2);
    }
  }

  @override
  bool shouldRepaint(_GeoPainter old) =>
      old.points != points || old.routes != routes || old.ground != ground;
}

// ------------------------------------------------------------------ places --

class _PlacesTab extends StatefulWidget {
  const _PlacesTab({required this.c, required this.st});

  final PlacesController c;
  final PlacesState st;

  @override
  State<_PlacesTab> createState() => _PlacesTabState();
}

class _PlacesTabState extends State<_PlacesTab> {
  String _q = '';

  Future<void> _wish() async {
    final name = await askLine(context, 'Somewhere you want to go', 'A place');
    if (name == null || name.trim().isEmpty || !mounted) return;
    final note = await askLine(context, 'A note', 'When, why, with whom');
    await widget.c.send(PlacesCmd.addWish(name: name, note: note ?? ''));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.st;
    final c = widget.c;
    final q = _q.toLowerCase();
    final towns = [
      for (final x in st.towns)
        if (q.isEmpty ||
            x.name.toLowerCase().contains(q) ||
            x.region.toLowerCase().contains(q) ||
            x.country.toLowerCase().contains(q))
          x
    ];
    final list = Container(
      decoration: cardDeco(context),
      child: Column(children: [
        for (final x in towns.take(300))
          ListTile(
            leading: Text(x.flag, style: const TextStyle(fontSize: 20)),
            title: Text(x.name,
                style: TextStyle(fontWeight: FontWeight.w700, color: t.nInk)),
            subtitle: Text(
                [
                  if (x.region.isNotEmpty) x.region,
                  x.country,
                  '${plural(x.days.toInt(), 'day')} · ${plural(x.photos.toInt(), 'photo')}',
                  x.first == x.last ? x.first : '${x.first} – ${x.last}',
                ].join(' · '),
                style: TextStyle(fontSize: 12, color: t.nInk2)),
            trailing: x.home
                ? const Chip(label: Text('Home'))
                : TextButton(
                    onPressed: () =>
                        c.send(PlacesCmd.setHome(lat: x.lat, lon: x.lon)),
                    child: const Text('Make home'),
                  ),
          ),
      ]),
    );
    final wishes = _Box(
      icon: Icons.star_outline,
      title: 'Want to go',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (final w in st.wishes)
            ListTile(
              contentPadding: EdgeInsets.zero,
              title: Text(w.name,
                  style:
                      TextStyle(fontWeight: FontWeight.w600, color: t.nInk)),
              subtitle: w.note.isEmpty ? null : Text(w.note),
              trailing: IconButton(
                tooltip: 'Remove',
                icon: const Icon(Icons.close, size: 17),
                onPressed: () => c.send(PlacesCmd.removeWish(id: w.id)),
              ),
            ),
          if (st.wishes.isEmpty)
            Text('Nowhere yet.', style: TextStyle(color: t.nInk3)),
          const SizedBox(height: 8),
          OutlinedButton.icon(
            icon: const Icon(Icons.add, size: 17),
            label: const Text('Add a place'),
            onPressed: _wish,
          ),
        ],
      ),
    );
    return LayoutBuilder(builder: (context, box) {
      final wide = box.maxWidth > 900;
      final main = Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(children: [
            Expanded(
              child: Text(
                  st.named
                      ? '${plural(st.towns.length, 'town')} you have photos from'
                      : 'Towns need the town table: Name the towns, top right.',
                  style: TextStyle(fontSize: 13, color: t.nInk2)),
            ),
            SizedBox(
              width: 220,
              child: TextField(
                decoration: const InputDecoration(
                    isDense: true,
                    hintText: 'Find a place',
                    prefixIcon: Icon(Icons.search, size: 17)),
                onChanged: (v) => setState(() => _q = v),
              ),
            ),
          ]),
          const SizedBox(height: 12),
          if (st.towns.isNotEmpty) list,
        ],
      );
      return ListView(
        padding: const EdgeInsets.fromLTRB(24, 20, 24, 30),
        children: [
          if (wide)
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(child: main),
                const SizedBox(width: 18),
                SizedBox(width: 320, child: wishes),
              ],
            )
          else ...[
            wishes,
            const SizedBox(height: 16),
            main,
          ],
        ],
      );
    });
  }
}

// -------------------------------------------------------------------- been --

class _BeenTab extends StatelessWidget {
  const _BeenTab({required this.c, required this.st});

  final PlacesController c;
  final PlacesState st;

  Future<void> _rules(BuildContext context) async {
    final km = await askLine(context, 'Away from home is further than',
        'Kilometres', initial: '${st.minKm}');
    if (km == null || !context.mounted) return;
    final gap = await askLine(context, 'A gap that ends a trip',
        'Days without a photo away', initial: '${st.gapDays}');
    if (gap == null || !context.mounted) return;
    final n = await askLine(context, 'Fewest photos for a trip', 'Photos',
        initial: '${st.minPhotos}');
    if (n == null) return;
    await c.send(PlacesCmd.setRules(
      minKm: int.tryParse(km.trim()) ?? st.minKm.toInt(),
      gapDays: int.tryParse(gap.trim()) ?? st.gapDays.toInt(),
      minPhotos: int.tryParse(n.trim()) ?? st.minPhotos.toInt(),
    ));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final b = st.been;
    final maxDays =
        b.years.fold<int>(1, (m, y) => math.max(m, y.days.toInt()));
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 30),
      children: [
        Wrap(spacing: 10, runSpacing: 10, children: [
          _Stat('${b.countries.length}', 'countries'),
          _Stat('${b.regions.length}', 'states and regions'),
          _Stat('${b.towns}', 'towns and cities'),
          _Stat('${b.km}', 'km on kept trips'),
          _Stat('${b.daysAway}', 'days away'),
        ]),
        if (!st.named) ...[
          const SizedBox(height: 12),
          Text('Countries, states and towns need the town table: Name the towns, top right.',
              style: TextStyle(color: t.nInk2)),
        ],
        const SizedBox(height: 16),
        Grid(min: 340, children: [
          _Box(
            icon: Icons.public,
            title: 'Countries',
            child: Column(children: [
              for (final x in b.countries)
                Padding(
                  padding: const EdgeInsets.symmetric(vertical: 5),
                  child: Row(children: [
                    Text(x.flag, style: const TextStyle(fontSize: 20)),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Text(x.name,
                          style: TextStyle(
                              fontWeight: FontWeight.w600, color: t.nInk)),
                    ),
                    Text(
                        '${plural(x.regions.toInt(), 'region')} · since ${x.firstYear}',
                        style: TextStyle(fontSize: 12, color: t.nInk3)),
                  ]),
                ),
              if (b.countries.isEmpty)
                Text('None named yet.', style: TextStyle(color: t.nInk3)),
            ]),
          ),
          _Box(
            icon: Icons.emoji_events_outlined,
            title: 'Firsts and furthests',
            child: Column(children: [
              for (final f in b.firsts)
                Padding(
                  padding: const EdgeInsets.symmetric(vertical: 5),
                  child: Row(children: [
                    Expanded(
                      child: Text(f.label,
                          style: TextStyle(fontSize: 13, color: t.nInk2)),
                    ),
                    Flexible(
                      child: Text(f.value,
                          textAlign: TextAlign.right,
                          style: TextStyle(
                              fontWeight: FontWeight.w600, color: t.nInk)),
                    ),
                  ]),
                ),
              if (b.firsts.isEmpty)
                Text('Keep a trip and this fills in.',
                    style: TextStyle(color: t.nInk3)),
            ]),
          ),
          _Box(
            icon: Icons.calendar_month_outlined,
            title: 'Days away, by year',
            child: SizedBox(
              height: 130,
              child: b.years.isEmpty
                  ? Text('No kept trips yet.', style: TextStyle(color: t.nInk3))
                  : Row(
                      crossAxisAlignment: CrossAxisAlignment.end,
                      children: [
                        for (final y in b.years)
                          Expanded(
                            child: Padding(
                              padding: const EdgeInsets.symmetric(horizontal: 4),
                              child: Column(
                                mainAxisAlignment: MainAxisAlignment.end,
                                children: [
                                  Text('${y.days}',
                                      style: TextStyle(
                                          fontSize: 11, color: t.nInk2)),
                                  const SizedBox(height: 4),
                                  Container(
                                    height: 90 * y.days.toInt() / maxDays,
                                    decoration: BoxDecoration(
                                      gradient: const LinearGradient(
                                        begin: Alignment.bottomCenter,
                                        end: Alignment.topCenter,
                                        colors: [kPlaces, kPlaces2],
                                      ),
                                      borderRadius: BorderRadius.circular(6),
                                    ),
                                  ),
                                  const SizedBox(height: 4),
                                  Text('${y.year}',
                                      style: TextStyle(
                                          fontSize: 11, color: t.nInk3)),
                                ],
                              ),
                            ),
                          ),
                      ],
                    ),
            ),
          ),
          _Box(
            icon: Icons.home_outlined,
            title: 'Home, and how trips are found',
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                    st.home.isEmpty
                        ? 'Home is not known yet.'
                        : 'Home is ${st.home}${st.homeAuto ? ', where most photos were taken' : ''}.',
                    style: TextStyle(fontSize: 13, color: t.nInk)),
                if (!st.homeAuto)
                  TextButton(
                    onPressed: () =>
                        c.send(const PlacesCmd.setHome(lat: 0, lon: 0)),
                    child: const Text('Work it out from the photos again'),
                  ),
                const SizedBox(height: 6),
                Text(
                    'A trip is photos more than ${st.minKm} km from home, no more '
                    'than ${plural(st.gapDays.toInt(), 'day')} apart, at least '
                    '${st.minPhotos} of them. A different home is a town\'s '
                    '“Make home” on Places.',
                    style: TextStyle(fontSize: 12.5, color: t.nInk2)),
                const SizedBox(height: 6),
                TextButton(
                  onPressed: () => _rules(context),
                  child: const Text('Change the rules'),
                ),
              ],
            ),
          ),
        ]),
        if (b.regions.isNotEmpty) ...[
          const SizedBox(height: 16),
          _Box(
            icon: Icons.map_outlined,
            title: 'States and regions',
            child: Wrap(spacing: 6, runSpacing: 6, children: [
              for (final r in b.regions) Chip(label: Text(r)),
            ]),
          ),
        ],
      ],
    );
  }
}
