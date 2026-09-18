// The Audiobooks shelf's ordering, grouping and "next in series".
//
// Three pure functions with no widgets under them, which is exactly why they
// are worth pinning: each one answers a question the deck asks on Home, and
// each one has an edge that reads as "obviously fine" and is not —
// never-played sorting to the top instead of the bottom, a standalone book
// vanishing when grouped by series, and "next" picking a book from a series
// nothing in has been finished.

import 'package:flutter_test/flutter_test.dart';
import 'package:tulipix/sections/music/audiobooks_tab.dart';
import 'package:tulipix/sections/music/music_controller.dart' show fmtMins;
import 'package:tulipix/src/rust/api/music.dart';

BookCard book(
  String title, {
  String author = 'Doyle',
  String series = '',
  int seriesNo = 0,
  bool finished = false,
  double progress = 0,
  double totalS = 3600,
  double speed = 1,
  int lastPlayed = 0,
  int added = 0,
}) =>
    BookCard(
      folder: '/books/$title',
      title: title,
      author: author,
      narrator: '',
      series: series,
      seriesNo: seriesNo,
      art: '',
      chapters: 10,
      finished: finished,
      progress: progress,
      totalS: totalS,
      chapterNow: 1,
      speed: speed,
      lastPlayed: lastPlayed,
      added: added,
    );

void main() {
  test('time left divides by the speed the book is actually heard at', () {
    // An hour, half heard, at 1.25x is 24 minutes — not 30.
    expect(
      bookLeftS(book('Dune', progress: 0.5, speed: 1.25)).round(),
      1440,
    );
    // A speed of 0 is "unset", not "instantaneous".
    expect(bookLeftS(book('Dune', speed: 0)).round(), 3600);
    expect(fmtMins(1440), '24 min');
    expect(fmtMins(3600), '1 h');
    expect(fmtMins(4500), '1 h 15 min');
  });

  test('recently played puts never-played last, not first', () {
    final list = sortBooks([
      book('Never'),
      book('Old', lastPlayed: 100),
      book('Fresh', lastPlayed: 900),
    ], 'recent');
    expect(list.map((b) => b.title), ['Fresh', 'Old', 'Never']);
  });

  test('shortest left ignores how long the book is', () {
    final list = sortBooks([
      book('Long', totalS: 36000, progress: 0.9), // 1 h left
      book('Short', totalS: 3600, progress: 0.1), // 54 min left
    ], 'left');
    expect(list.first.title, 'Short');
  });

  test('a book with no series still has a group', () {
    final groups = groupBooks([
      book('Study', series: 'Holmes', seriesNo: 1),
      book('Gitanjali', series: ''),
    ], 'series');
    expect(groups.map((g) => g.$1), ['Holmes', 'Standalone']);
    expect(groups.last.$2.single.title, 'Gitanjali');
    // Nothing is dropped when nothing is grouped.
    expect(groupBooks([book('A'), book('B')], 'none').single.$2.length, 2);
  });

  test('next in series needs a finished book in that series', () {
    final unstarted = book('Hound', series: 'Holmes', seriesNo: 5);
    // Nothing in the series is finished yet, so there is no "next".
    expect(nextInSeries([unstarted]), isNull);

    final shelf = [
      book('Sign', series: 'Holmes', seriesNo: 2, finished: true),
      unstarted,
      book('Valley', series: 'Holmes', seriesNo: 4),
      // Part-way through is not "next" — you are already in it.
      book('Memoirs', series: 'Holmes', seriesNo: 3, progress: 0.4),
    ];
    expect(nextInSeries(shelf)!.title, 'Valley');
  });
}
