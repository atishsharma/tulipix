// The reading-statistics panel — how much, how often, and on what.
//
// ui/page_books.slint's version: five big numbers, the twelve-week heat strip
// laid out 12 columns × 7 rows, then the per-book time list. The page owns the
// 620 × 500 card this goes inside.

import 'package:flutter/material.dart';

import '../../src/rust/api/books.dart';
import 'book_theme.dart';
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
    final b = context.book;
    return Padding(
      padding: const EdgeInsets.all(22),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Text('Reading stats',
                  style: TextStyle(
                      fontSize: 20, fontWeight: FontWeight.w800, color: b.ink)),
              const Spacer(),
              IconButton(
                iconSize: 15,
                visualDensity: VisualDensity.compact,
                icon: Icon(Icons.close, color: b.inkDim),
                onPressed: () => controller.send(const BooksCmd.closeStats()),
              ),
            ],
          ),
          const SizedBox(height: 16),
          Row(
            children: [
              for (final m in [
                (v: '${stats.hours.toStringAsFixed(1)}h', l: 'Read'),
                (v: '${stats.streak}d', l: 'Streak'),
                (v: '${stats.finished}', l: 'Finished'),
                (v: '${stats.started}', l: 'Started'),
                (v: '${stats.days}', l: 'Days'),
              ]) ...[
                Expanded(child: _Big(value: m.v, label: m.l)),
                if (m.l != 'Days') const SizedBox(width: 12),
              ],
            ],
          ),
          const SizedBox(height: 16),
          Text('LAST 12 WEEKS',
              style: TextStyle(
                  fontSize: 11,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 0.5,
                  color: b.inkDim)),
          const SizedBox(height: 8),
          _Heat(days: stats.heat),
          const SizedBox(height: 16),
          Text('TIME PER BOOK',
              style: TextStyle(
                  fontSize: 11,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 0.5,
                  color: b.inkDim)),
          const SizedBox(height: 8),
          Expanded(
            child: stats.books.isEmpty
                ? Text('No reading time logged yet.',
                    style: TextStyle(fontSize: 12, color: b.inkDim))
                : ListView.separated(
                    padding: EdgeInsets.zero,
                    itemCount: stats.books.length,
                    separatorBuilder: (_, __) => const SizedBox(height: 4),
                    itemBuilder: (_, i) {
                      final row = stats.books[i];
                      return Row(
                        children: [
                          Expanded(
                            child: Text(row.title,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(fontSize: 13, color: b.ink)),
                          ),
                          const SizedBox(width: 10),
                          Text(row.time,
                              style: TextStyle(
                                  fontSize: 12,
                                  fontWeight: FontWeight.w600,
                                  color: b.inkDim)),
                        ],
                      );
                    },
                  ),
          ),
        ],
      ),
    );
  }
}

class _Big extends StatelessWidget {
  const _Big({required this.value, required this.label});

  final String value;
  final String label;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return Container(
      height: 74,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: b.pillBg,
        borderRadius: BorderRadius.circular(12),
      ),
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Text(value,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                  fontSize: 22,
                  fontWeight: FontWeight.w800,
                  color: BookTheme.accent)),
          const SizedBox(height: 2),
          Text(label,
              style: TextStyle(
                  fontSize: 11, fontWeight: FontWeight.w600, color: b.inkDim)),
        ],
      ),
    );
  }
}

/// 84 days laid out 12 columns × 7 rows, index = column × 7 + row — the last
/// twelve weeks, oldest column first.
class _Heat extends StatelessWidget {
  const _Heat({required this.days});

  final List<bool> days;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    // The bridge sends a year; the panel shows the tail of it.
    final tail = days.length > 84 ? days.sublist(days.length - 84) : days;
    return SizedBox(
      height: 7 * 15,
      child: LayoutBuilder(
        builder: (context, box) {
          final cell = box.maxWidth / 12;
          return Stack(
            children: [
              for (var i = 0; i < tail.length; i++)
                Positioned(
                  left: (i ~/ 7) * cell,
                  top: (i % 7) * 15,
                  width: cell - 4,
                  height: 11,
                  child: DecoratedBox(
                    decoration: BoxDecoration(
                      color: tail[i] ? BookTheme.accent : b.track,
                      borderRadius: BorderRadius.circular(3),
                    ),
                  ),
                ),
            ],
          );
        },
      ),
    );
  }
}
