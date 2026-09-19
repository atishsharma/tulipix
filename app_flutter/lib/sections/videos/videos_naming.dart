// How to name files so the library reads them: the info popup beside
// "+ Add folder" on the Movies and TV tabs.
//
// Every example here is one of the twenty paths the tests in
// crates/tulipix-videos/src/naming.rs check, in the same order, with what that
// parser reads out of it. Change a layout there and change it here: the popup
// is a promise, and the tests are what keep it.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';

/// One layout: the path, what the library reads from it, and why you would
/// name a file this way.
typedef _Layout = ({String path, String reads, String note});

const List<_Layout> _showLayouts = [
  (
    path: 'TV/Severance/Season 01/Severance - S01E04 - The You You Are.mkv',
    reads: 'Severance · Season 1 · Episode 4',
    note: 'The best one. Show folder, season folder, SxxEyy, then the title.',
  ),
  (
    path: 'TV/Severance/Season 1/S01E04.mkv',
    reads: 'Severance · Season 1 · Episode 4',
    note: 'The file needs only its tag; the folders name the show.',
  ),
  (
    path: 'TV/Severance.S01E04.1080p.WEB-DL.x265-GROUP.mkv',
    reads: 'Severance · Season 1 · Episode 4',
    note: 'A release name as downloaded. The text before the tag is the show.',
  ),
  (
    path: 'TV/Doctor Who (2005)/Season 04/Doctor Who (2005) - S04E10.mkv',
    reads: 'Doctor Who · Season 4 · Episode 10',
    note: 'A year in brackets is fine; it is kept out of the show\u2019s name.',
  ),
  (
    path: 'TV/The Bear/Season 01/The Bear - 1x05 - Sheridan.mkv',
    reads: 'The Bear · Season 1 · Episode 5',
    note: 'Season x episode works as well as SxxEyy.',
  ),
  (
    path: 'TV/Dark/Season 01/Dark - S01E01-E02.mkv',
    reads: 'Dark · Season 1 · Episode 1',
    note: 'A double episode in one file is filed under its first number.',
  ),
  (
    path: 'TV/Sherlock/Specials/Sherlock - S00E01 - The Abominable Bride.mkv',
    reads: 'Sherlock · Specials · Episode 1',
    note: 'Season 0, or a folder called Specials, holds the one-offs.',
  ),
  (
    path: 'TV/Chernobyl/Season 1/Episode 03.mkv',
    reads: 'Chernobyl · Season 1 · Episode 3',
    note: 'Inside a season folder, "Episode 03", "E03" or "03" is enough.',
  ),
  (
    path: 'TV/Frieren/[SubsPlease] Sousou no Frieren - 12 [1080p].mkv',
    reads: 'Sousou no Frieren · Season 1 · Episode 12',
    note: 'Anime releases: a [Group] tag in front, then " - 12".',
  ),
  (
    path: 'TV/Fleabag/Fleabag Season 2 Episode 5.mp4',
    reads: 'Fleabag · Season 2 · Episode 5',
    note: 'Spelled out in words works too.',
  ),
];

const List<_Layout> _movieLayouts = [
  (
    path: 'Movies/Arrival (2016)/Arrival (2016).mkv',
    reads: 'Arrival · 2016',
    note: 'The best one. A folder per movie keeps its extras beside it.',
  ),
  (
    path: 'Movies/Arrival (2016).mkv',
    reads: 'Arrival · 2016',
    note: 'One folder of films, each named with its year.',
  ),
  (
    path: 'Arrival.2016.1080p.BluRay.x264-GROUP.mkv',
    reads: 'Arrival · 2016',
    note: 'A release name as downloaded: everything after the year is ignored.',
  ),
  (
    path: 'Dune Part Two (2024) [2160p HDR].mkv',
    reads: 'Dune Part Two · 2024',
    note: 'Quality in square brackets after the year is fine.',
  ),
  (
    path: 'Parasite [2019].mkv',
    reads: 'Parasite · 2019',
    note: 'The year can be in square brackets too.',
  ),
  (
    path: "Aliens (1986) {edition-Director's Cut}.mkv",
    reads: 'Aliens · 1986',
    note: 'Curly-brace labels, like an edition, are ignored.',
  ),
  (
    path: 'Kill Bill Vol 1 (2003) - cd2.mkv',
    reads: 'Kill Bill Vol 1 · 2003',
    note: 'A film split into parts: "- cd1", "- part 2", "- disc1".',
  ),
  (
    path: 'Blade Runner 2049 (2017).mkv',
    reads: 'Blade Runner 2049 · 2017',
    note: 'Numbers in a title stay in it. The bracketed year is the year.',
  ),
  (
    path: 'Movies/Spirited Away.mkv',
    reads: 'Spirited Away · no year',
    note: 'No year? Put it under a folder called Movies or Films.',
  ),
  (
    path: 'Movies/Interstellar (2014)/movie.mkv',
    reads: 'Interstellar · 2014',
    note: 'A file with a plain name takes its folder’s name and year.',
  ),
];

const String _showsTree = '''
TV/
  Severance/
    Season 01/
      Severance - S01E01 - Good News About Hell.mkv
      Severance - S01E02 - Half Loop.mkv
    Season 02/
  Sherlock/
    Specials/''';

const String _moviesTree = '''
Movies/
  Arrival (2016)/
    Arrival (2016).mkv
  Blade Runner 2049 (2017)/
    Blade Runner 2049 (2017).mkv''';

/// The guide, opened on the tab it was asked from.
Future<void> openNamingGuide(BuildContext context, {required bool shows}) =>
    showDialog<void>(
      context: context,
      builder: (ctx) => _NamingGuide(shows: shows),
    );

class _NamingGuide extends StatefulWidget {
  const _NamingGuide({required this.shows});

  final bool shows;

  @override
  State<_NamingGuide> createState() => _NamingGuideState();
}

class _NamingGuideState extends State<_NamingGuide> {
  late bool _shows = widget.shows;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final layouts = _shows ? _showLayouts : _movieLayouts;
    const mono = TextStyle(fontFamily: 'monospace', fontSize: 12, height: 1.45);
    return AlertDialog(
      backgroundColor: t.modalSolid,
      titlePadding: const EdgeInsets.fromLTRB(24, 20, 24, 0),
      contentPadding: const EdgeInsets.fromLTRB(24, 14, 24, 0),
      title: Row(
        children: [
          Expanded(
            child: Text(
              _shows ? 'How to name your shows' : 'How to name your movies',
              style: TextStyle(
                  fontSize: 18, fontWeight: FontWeight.w800, color: t.nInk),
            ),
          ),
          SegmentedButton<bool>(
            segments: const [
              ButtonSegment(value: false, label: Text('Movies')),
              ButtonSegment(value: true, label: Text('Shows')),
            ],
            selected: {_shows},
            showSelectedIcon: false,
            onSelectionChanged: (v) => setState(() => _shows = v.first),
          ),
        ],
      ),
      content: SizedBox(
        width: 640,
        child: SingleChildScrollView(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Text(
                _shows
                    ? 'A file is an episode when its name carries the season and '
                        'episode. Any of these ten layouts is read; the first is '
                        'the one to use when you are organising from scratch.'
                    : 'A file is a movie when its name carries a year, or when it '
                        'sits under a folder called Movies or Films. Anything '
                        'else stays in Local as a personal video.',
                style: TextStyle(fontSize: 13, color: t.nInk2, height: 1.45),
              ),
              const SizedBox(height: 14),
              Text('Folders',
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
              const SizedBox(height: 6),
              Container(
                padding: const EdgeInsets.fromLTRB(14, 4, 14, 12),
                decoration: BoxDecoration(
                  color: t.nTile,
                  borderRadius: BorderRadius.circular(10),
                ),
                child: SelectableText(
                  _shows ? _showsTree : _moviesTree,
                  style: mono.copyWith(color: t.nInk),
                ),
              ),
              const SizedBox(height: 16),
              Text('Ten names it reads',
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
              const SizedBox(height: 4),
              for (final (i, l) in layouts.indexed)
                Padding(
                  padding: const EdgeInsets.symmetric(vertical: 8),
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      SizedBox(
                        width: 26,
                        child: Text(
                          '${i + 1}',
                          style: TextStyle(
                            fontSize: 13,
                            fontWeight: FontWeight.w800,
                            color: i == 0 ? Tokens.secVideos : t.nInk3,
                          ),
                        ),
                      ),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            SelectableText(l.path,
                                style: mono.copyWith(color: t.nInk)),
                            const SizedBox(height: 3),
                            Text.rich(
                              TextSpan(children: [
                                TextSpan(
                                  text: l.reads,
                                  style: const TextStyle(
                                      fontWeight: FontWeight.w700,
                                      color: Tokens.secVideos),
                                ),
                                TextSpan(text: '   ${l.note}'),
                              ]),
                              style: TextStyle(
                                  fontSize: 12, color: t.nInk2, height: 1.4),
                            ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
              const SizedBox(height: 8),
              Text(
                _shows
                    ? 'With a TMDB key, each show\u2019s poster and overview are '
                        'filled in from its name. Renamed files already in the '
                        'library are re-read when you rescan.'
                    : 'With a TMDB key, the poster, overview and runtime are filled '
                        'in from the name and year. Renamed files already in the '
                        'library are re-read when you rescan.',
                style: TextStyle(fontSize: 12, color: t.nInk3, height: 1.45),
              ),
              const SizedBox(height: 12),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Done'),
        ),
      ],
    );
  }
}
