// What the library knows about itself: where the listening actually went, and
// which recordings it holds twice.
//
// Both read what is already stored — play history has been recorded since the
// first version and until now only fed a "recently played" shelf, and the
// duplicate finder compares the fingerprint the analysis pass writes. Neither
// asks anything of the network, and neither needs a new column.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';

// ------------------------------------------------------------------ shared --

/// The dialog frame both of these sit in: a fixed, generous width so a bar
/// chart has somewhere to be, and a height that stops at 78% of the window so
/// a long list scrolls inside the dialog rather than pushing its buttons off.
Widget _frame(BuildContext context, {required Widget child}) {
  final size = MediaQuery.sizeOf(context);
  return SizedBox(
    width: 640,
    height: size.height * 0.78,
    child: child,
  );
}

Widget _sectionLabel(BuildContext context, String text) {
  final t = context.tokens;
  return Padding(
    padding: const EdgeInsets.only(top: 20, bottom: 8),
    child: Text(
      text,
      style: TextStyle(
        fontFamily: Tokens.fontFamily,
        fontSize: 11,
        fontWeight: FontWeight.w700,
        letterSpacing: 0.7,
        color: t.nInk3,
      ),
    ),
  );
}

// --------------------------------------------------------------- listening --

/// How far back a summary looks. All time last, because the interesting
/// question is usually recent.
const List<(String, int)> _ranges = [
  ('7 days', 7),
  ('30 days', 30),
  ('12 months', 365),
  ('All time', 0),
];

/// Where the listening actually went.
Future<void> listeningSummary(BuildContext context, MusicController c) async {
  var days = 30;
  await showDialog<void>(
    context: context,
    builder: (ctx) => StatefulBuilder(
      builder: (ctx, setLocal) => AlertDialog(
        title: const Text('Listening'),
        content: _frame(
          ctx,
          child: FutureBuilder<Listening>(
            // Keyed on the range, so switching chips re-runs the query rather
            // than showing the old window's figures under the new label.
            key: ValueKey(days),
            future: musicListening(days: days),
            builder: (ctx, snap) {
              final data = snap.data;
              return Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      for (final (label, value) in _ranges)
                        Padding(
                          padding: const EdgeInsets.only(right: 8),
                          child: ChoiceChip(
                            label: Text(label),
                            selected: days == value,
                            onSelected: (_) => setLocal(() => days = value),
                          ),
                        ),
                    ],
                  ),
                  const SizedBox(height: 14),
                  if (data == null)
                    const Expanded(
                      child: Center(child: CircularProgressIndicator()),
                    )
                  else if (data.total.isEmpty || data.total == '—')
                    Expanded(
                      child: Center(
                        child: Text(
                          'Nothing played in this window yet.',
                          style: TextStyle(color: ctx.tokens.nInk3),
                        ),
                      ),
                    )
                  else
                    Expanded(child: _Summary(data: data, controller: c)),
                ],
              );
            },
          ),
        ),
        actions: [
          TextButton(
            onPressed: () {
              Navigator.of(ctx).pop();
              duplicateFinder(context, c);
            },
            child: const Text('Find duplicates'),
          ),
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(),
            child: const Text('Close'),
          ),
        ],
      ),
    ),
  );
}

class _Summary extends StatelessWidget {
  const _Summary({required this.data, required this.controller});

  final Listening data;
  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: EdgeInsets.zero,
      children: [
        Row(
          crossAxisAlignment: CrossAxisAlignment.baseline,
          textBaseline: TextBaseline.alphabetic,
          children: [
            Text(
              data.total,
              style: TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: 30,
                fontWeight: FontWeight.w800,
                color: t.nInk,
              ),
            ),
            const SizedBox(width: 10),
            Text(
              data.range.toLowerCase(),
              style: TextStyle(fontSize: 13, color: t.nInk3),
            ),
          ],
        ),
        if (data.peakHour.isNotEmpty) ...[
          _sectionLabel(context, 'BY HOUR'),
          _HourChart(hours: data.hours, peak: data.peakHour),
        ],
        if (data.artists.isNotEmpty) ...[
          _sectionLabel(context, 'ARTISTS'),
          _Bars(rows: data.artists, onTap: (key) => _openArtist(key)),
        ],
        if (data.genres.isNotEmpty) ...[
          _sectionLabel(context, 'GENRES'),
          _Bars(rows: data.genres),
        ],
        if (data.tracks.isNotEmpty) ...[
          _sectionLabel(context, 'ON REPEAT'),
          _Bars(rows: data.tracks),
        ],
        if (data.abandoned.isNotEmpty) ...[
          _sectionLabel(context, 'STARTED AND NEVER FINISHED'),
          for (final row in data.abandoned)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 3),
              child: Row(
                children: [
                  Expanded(
                    child: Text(
                      row.label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 13, color: t.nInk),
                    ),
                  ),
                  Text(
                    '${row.plays} times',
                    style: TextStyle(fontSize: 12, color: t.nInk3),
                  ),
                ],
              ),
            ),
        ],
        const SizedBox(height: 8),
      ],
    );
  }

  void _openArtist(int id) {
    if (id > 0) controller.send(MusicCmd.openArtist(artistId: id));
  }
}

/// One ranked list, drawn as bars. The width is a value the bridge already
/// computed against the biggest row, so nothing is divided here.
class _Bars extends StatelessWidget {
  const _Bars({required this.rows, this.onTap});

  final List<Tally> rows;
  final void Function(int key)? onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      children: [
        for (final row in rows)
          InkWell(
            onTap: onTap == null || row.key == 0 ? null : () => onTap!(row.key),
            borderRadius: BorderRadius.circular(6),
            child: Padding(
              padding: const EdgeInsets.symmetric(vertical: 4, horizontal: 2),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          row.label,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 13, color: t.nInk),
                        ),
                      ),
                      const SizedBox(width: 10),
                      Text(
                        row.value,
                        style: TextStyle(
                          fontFamily: Tokens.fontFamily,
                          fontSize: 12,
                          fontWeight: FontWeight.w700,
                          color: t.nInk2,
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 4),
                  ClipRRect(
                    borderRadius: BorderRadius.circular(3),
                    child: LinearProgressIndicator(
                      value: row.frac.clamp(0.0, 1.0),
                      minHeight: 5,
                      backgroundColor: t.nHair,
                      valueColor:
                          const AlwaysStoppedAnimation<Color>(Tokens.secMusic),
                    ),
                  ),
                ],
              ),
            ),
          ),
      ],
    );
  }
}

/// Twenty-four columns, midnight to midnight.
class _HourChart extends StatelessWidget {
  const _HourChart({required this.hours, required this.peak});

  final List<double> hours;
  final String peak;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SizedBox(
          height: 70,
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              for (var h = 0; h < hours.length; h++)
                Expanded(
                  child: Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 1.5),
                    child: Tooltip(
                      message: '${h.toString().padLeft(2, '0')}:00',
                      child: Container(
                        // A floor of two pixels: an hour with no listening is
                        // still an hour, and a zero-height column reads as a
                        // gap in the axis rather than as a quiet time of day.
                        height: (2 + hours[h] * 66).clamp(2.0, 68.0),
                        decoration: BoxDecoration(
                          color: hours[h] > 0
                              ? Tokens.secMusic.withValues(
                                  alpha: 0.35 + 0.65 * hours[h])
                              : t.nHair,
                          borderRadius: const BorderRadius.vertical(
                            top: Radius.circular(3),
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
            ],
          ),
        ),
        const SizedBox(height: 6),
        Text(
          'Busiest at $peak',
          style: TextStyle(fontSize: 12, color: t.nInk3),
        ),
      ],
    );
  }
}

// -------------------------------------------------------------- duplicates --

/// The same recording, more than once.
Future<void> duplicateFinder(BuildContext context, MusicController c) async {
  await showDialog<void>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Duplicates'),
      content: _frame(
        ctx,
        child: FutureBuilder<List<DupeGroup>>(
          future: musicDuplicates(),
          builder: (ctx, snap) {
            if (snap.connectionState != ConnectionState.done) {
              return const Center(child: CircularProgressIndicator());
            }
            final groups = snap.data ?? const <DupeGroup>[];
            if (groups.isEmpty) {
              return Center(
                child: Padding(
                  padding: const EdgeInsets.all(24),
                  child: Text(
                    'No duplicates found.\n\n'
                    'Recordings are matched on length and on what they '
                    'actually sound like, not on their tags — so this needs '
                    'the library analysis in Settings to have run.',
                    textAlign: TextAlign.center,
                    style: TextStyle(fontSize: 13, color: ctx.tokens.nInk3),
                  ),
                ),
              );
            }
            return ListView.separated(
              padding: EdgeInsets.zero,
              itemCount: groups.length,
              separatorBuilder: (_, __) => const SizedBox(height: 6),
              itemBuilder: (_, i) => _DupeCard(group: groups[i], controller: c),
            );
          },
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(ctx).pop(),
          child: const Text('Close'),
        ),
      ],
    ),
  );
}

class _DupeCard extends StatelessWidget {
  const _DupeCard({required this.group, required this.controller});

  final DupeGroup group;
  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: t.nHair),
      ),
      child: Column(
        children: [
          for (final copy in group.copies)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 5),
              child: Row(
                children: [
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          copy.track.title.isEmpty
                              ? copy.track.path
                              : copy.track.title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            fontSize: 13,
                            fontWeight:
                                copy.keep ? FontWeight.w700 : FontWeight.w400,
                            color: t.nInk,
                          ),
                        ),
                        Text(
                          [
                            if (copy.track.album.isNotEmpty) copy.track.album,
                            if (copy.quality.isNotEmpty) copy.quality,
                          ].join(' · '),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 11.5, color: t.nInk3),
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(width: 10),
                  if (copy.keep)
                    Container(
                      padding: const EdgeInsets.symmetric(
                          horizontal: 8, vertical: 3),
                      decoration: BoxDecoration(
                        color: Tokens.secMusic.withValues(alpha: 0.16),
                        borderRadius: BorderRadius.circular(20),
                      ),
                      child: const Text(
                        'Keep',
                        style: TextStyle(
                          fontFamily: Tokens.fontFamily,
                          fontSize: 11,
                          fontWeight: FontWeight.w700,
                          color: Tokens.secMusic,
                        ),
                      ),
                    )
                  else ...[
                    Text(
                      '${copy.confidence}%',
                      style: TextStyle(
                        fontFamily: Tokens.fontFamily,
                        fontSize: 12,
                        fontWeight: FontWeight.w700,
                        color: t.nInk3,
                      ),
                    ),
                    const SizedBox(width: 6),
                    // Removal is one row at a time and always confirmed: this
                    // deletes a file, and a bulk "remove all the copies I say
                    // are worse" is the one action here nobody can undo.
                    IconButton(
                      tooltip: 'Delete this copy',
                      iconSize: 18,
                      visualDensity: VisualDensity.compact,
                      icon: const Icon(Icons.delete_outline),
                      onPressed: () => _remove(context, copy),
                    ),
                  ],
                ],
              ),
            ),
        ],
      ),
    );
  }

  Future<void> _remove(BuildContext context, DupeCopy copy) async {
    final title =
        copy.track.title.isEmpty ? copy.track.path : copy.track.title;
    final ok = await confirm(
      context,
      title: 'Delete this copy?',
      body: '$title\n${copy.quality}\n\n'
          'The file is deleted from disk. The copy marked Keep stays.',
    );
    if (!ok) return;
    controller.send(MusicCmd.deleteTrack(itemId: copy.track.itemId));
  }
}
