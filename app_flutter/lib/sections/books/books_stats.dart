// The reading-statistics panel: how much, how often, and on what.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/books.dart';
import 'books_controller.dart';

class ReadingStatsPanel extends StatelessWidget {
  const ReadingStatsPanel({
    super.key,
    required this.controller,
    required this.stats,
  });

  final BooksController controller;
  final ReadingStats stats;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 18, 24, 40),
      children: [
        Row(
          children: [
            TextButton.icon(
              icon: const Icon(Icons.arrow_back, size: 18),
              label: const Text('Back to the shelf'),
              onPressed: () => controller.send(const BooksCmd.closeStats()),
            ),
          ],
        ),
        const SizedBox(height: 8),
        Text('Reading',
            style: TextStyle(
                fontSize: 26, fontWeight: FontWeight.w800, color: t.nInk)),
        const SizedBox(height: 20),
        Row(
          children: [
            _Big(value: stats.hours.toStringAsFixed(1), label: 'hours'),
            _Big(value: '${stats.days}', label: 'days read'),
            _Big(value: '${stats.streak}', label: 'day streak'),
            _Big(value: '${stats.finished}', label: 'finished'),
            _Big(value: '${stats.started}', label: 'started'),
          ],
        ),
        const SizedBox(height: 28),
        Text('The last year',
            style: TextStyle(
                fontSize: 14, fontWeight: FontWeight.w700, color: t.nInk)),
        const SizedBox(height: 10),
        _Heat(days: stats.heat),
        const SizedBox(height: 28),
        Text('Where the hours went',
            style: TextStyle(
                fontSize: 14, fontWeight: FontWeight.w700, color: t.nInk)),
        const SizedBox(height: 10),
        if (stats.books.isEmpty)
          Text('Nothing tracked yet.',
              style: TextStyle(fontSize: 12, color: t.nInk2))
        else
          for (final b in stats.books)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 5),
              child: Row(
                children: [
                  Expanded(
                    child: Text(b.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 13, color: t.nInk)),
                  ),
                  Text(b.time,
                      style: const TextStyle(
                          fontSize: 12,
                          fontWeight: FontWeight.w600,
                          color: Tokens.secBooks)),
                ],
              ),
            ),
      ],
    );
  }
}

class _Big extends StatelessWidget {
  const _Big({required this.value, required this.label});

  final String value;
  final String label;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Expanded(
      child: Container(
        margin: const EdgeInsets.only(right: 12),
        padding: const EdgeInsets.symmetric(vertical: 18),
        decoration: BoxDecoration(
          color: t.nCard,
          borderRadius: BorderRadius.circular(12),
          border: Border.all(color: t.nHair),
        ),
        child: Column(
          children: [
            Text(value,
                style: const TextStyle(
                    fontSize: 28,
                    fontWeight: FontWeight.w800,
                    color: Tokens.secBooks)),
            Text(label, style: TextStyle(fontSize: 11, color: t.nInk2)),
          ],
        ),
      ),
    );
  }
}

/// A year of reading days, oldest first, wrapped into week columns — the shape
/// everyone already knows how to read.
class _Heat extends StatelessWidget {
  const _Heat({required this.days});

  final List<bool> days;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (days.isEmpty) {
      return Text('No reading recorded yet.',
          style: TextStyle(fontSize: 12, color: t.nInk2));
    }
    const cell = 10.0;
    const gap = 2.0;
    final weeks = (days.length / 7).ceil();
    return SizedBox(
      height: 7 * (cell + gap),
      child: SingleChildScrollView(
        scrollDirection: Axis.horizontal,
        child: Row(
          children: [
            for (var w = 0; w < weeks; w++)
              Padding(
                padding: const EdgeInsets.only(right: gap),
                child: Column(
                  children: [
                    for (var d = 0; d < 7; d++)
                      Padding(
                        padding: const EdgeInsets.only(bottom: gap),
                        child: Container(
                          width: cell,
                          height: cell,
                          decoration: BoxDecoration(
                            color: (w * 7 + d) < days.length && days[w * 7 + d]
                                ? Tokens.secBooks
                                : t.nHover,
                            borderRadius: BorderRadius.circular(2),
                          ),
                        ),
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
