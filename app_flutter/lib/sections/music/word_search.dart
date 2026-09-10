// Searching the words instead of the titles.
//
// Lyrics have been stored since the section shipped — synced timestamps and
// all — and nothing has ever searched them. A half-remembered line is often
// the only thing anyone remembers about a song, and every hit here knows the
// second the words are sung, so finding it and landing on it are one action.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_widgets.dart';

/// How long to wait after the last keystroke before asking.
///
/// The search is a LIKE scan over every lyric row; it returns in a blink, but
/// firing one per character would still queue a dozen for a phrase nobody has
/// finished typing.
const Duration _kSettle = Duration(milliseconds: 220);

Future<void> searchLyrics(BuildContext context, MusicController c) async {
  await showDialog<void>(
    context: context,
    builder: (ctx) => _WordSearch(controller: c),
  );
}

class _WordSearch extends StatefulWidget {
  const _WordSearch({required this.controller});

  final MusicController controller;

  @override
  State<_WordSearch> createState() => _WordSearchState();
}

class _WordSearchState extends State<_WordSearch> {
  final TextEditingController _field = TextEditingController();
  Timer? _settle;
  List<LyricMatch> _hits = const [];
  bool _searching = false;

  /// Which query the results on screen belong to, so a slow answer to an
  /// abandoned query does not replace the answer to the current one.
  String _showing = '';

  @override
  void dispose() {
    _settle?.cancel();
    _field.dispose();
    super.dispose();
  }

  void _onChanged(String value) {
    _settle?.cancel();
    final q = value.trim();
    if (q.length < 3) {
      setState(() {
        _hits = const [];
        _searching = false;
        _showing = q;
      });
      return;
    }
    setState(() => _searching = true);
    _settle = Timer(_kSettle, () => _run(q));
  }

  Future<void> _run(String q) async {
    List<LyricMatch> found;
    try {
      found = await musicLyricSearch(query: q);
    } catch (_) {
      found = const [];
    }
    if (!mounted || _field.text.trim() != q) return;
    setState(() {
      _hits = found;
      _showing = q;
      _searching = false;
    });
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final size = MediaQuery.sizeOf(context);
    return AlertDialog(
      title: const Text('Search the words'),
      content: SizedBox(
        width: 560,
        height: size.height * 0.7,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            TextField(
              controller: _field,
              autofocus: true,
              onChanged: _onChanged,
              decoration: const InputDecoration(
                isDense: true,
                prefixIcon: Icon(Icons.search, size: 18),
                hintText: 'A line you remember…',
                border: OutlineInputBorder(),
              ),
            ),
            const SizedBox(height: 12),
            Expanded(child: _body(t)),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Close'),
        ),
      ],
    );
  }

  Widget _body(dynamic t) {
    if (_searching) {
      return const Center(child: CircularProgressIndicator());
    }
    if (_showing.length < 3) {
      return Center(
        child: Text(
          'Type three letters or more.\n\n'
          'Only songs whose lyrics are stored can be found — the Tags & '
          'Lyrics manager is where they get fetched.',
          textAlign: TextAlign.center,
          style: TextStyle(fontSize: 13, color: t.nInk3),
        ),
      );
    }
    if (_hits.isEmpty) {
      return Center(
        child: Text(
          'No song in your library sings that.',
          style: TextStyle(fontSize: 13, color: t.nInk3),
        ),
      );
    }
    return ListView.separated(
      padding: EdgeInsets.zero,
      itemCount: _hits.length,
      separatorBuilder: (_, __) => const SizedBox(height: 4),
      itemBuilder: (_, i) => _HitRow(
        hit: _hits[i],
        needle: _showing,
        controller: widget.controller,
        onPlayed: () => Navigator.of(context).pop(),
      ),
    );
  }
}

class _HitRow extends StatelessWidget {
  const _HitRow({
    required this.hit,
    required this.needle,
    required this.controller,
    required this.onPlayed,
  });

  final LyricMatch hit;
  final String needle;
  final MusicController controller;
  final VoidCallback onPlayed;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: t.nCard,
      borderRadius: BorderRadius.circular(10),
      child: InkWell(
        borderRadius: BorderRadius.circular(10),
        onTap: () {
          controller.send(
            MusicCmd.lyricJump(itemId: hit.itemId, secs: hit.secs),
          );
          onPlayed();
        },
        child: Padding(
          padding: const EdgeInsets.all(9),
          child: Row(
            children: [
              MusicArt(
                controller: controller,
                kind: 'track',
                artKey: hit.itemId.toString(),
                direct: hit.art.isEmpty ? null : hit.art,
                size: 40,
                radius: 6,
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text.rich(
                      _highlight(hit.line, needle, t),
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                    ),
                    const SizedBox(height: 2),
                    Text(
                      [hit.title, hit.artist]
                          .where((s) => s.isNotEmpty)
                          .join(' · '),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 11, color: t.nInk3),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 8),
              // The timestamp is the promise: tapping lands on the line, it
              // does not just start the song.
              Text(
                hit.at.isEmpty ? '—' : hit.at,
                style: TextStyle(
                  fontFamily: Tokens.fontFamily,
                  fontSize: 12,
                  fontWeight: FontWeight.w700,
                  color: hit.at.isEmpty ? t.nInk3 : Tokens.secMusic,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  /// The matched words, in the accent, inside the line they were found in.
  TextSpan _highlight(String line, String needle, dynamic t) {
    final base = TextStyle(fontSize: 13, color: t.nInk);
    final at = line.toLowerCase().indexOf(needle.toLowerCase());
    if (at < 0) return TextSpan(text: line, style: base);
    return TextSpan(
      style: base,
      children: [
        TextSpan(text: line.substring(0, at)),
        TextSpan(
          text: line.substring(at, at + needle.length),
          style: const TextStyle(
            fontWeight: FontWeight.w800,
            color: Tokens.secMusic,
          ),
        ),
        TextSpan(text: line.substring(at + needle.length)),
      ],
    );
  }
}
