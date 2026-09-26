// Read Aloud — the reader speaking, and the highlight following it.
//
// The loop is HERE, not in Rust. `tts_audio_step` in
// crates/tulipix-sec-books/src/lib.rs synthesises a sentence, spawns an mpv to
// play it, keeps the mpv's IPC socket in a static so a pause can quit it
// mid-sentence, and holds a generation counter so a synth thread that is no
// longer wanted knows to drop its result. Every part of that exists because the
// loop sits on the far side of the UI from the player.
//
// This side already has both. media_kit is libmpv, and the reader's spread is a
// widget away — so the bridge only splits the book and speaks one sentence
// (`booksTtsPlan` / `booksTtsSay`), and everything else is here: play a WAV,
// wait for it to end, move the highlight, turn the page, ask for the next one.
// Stopping is not asking again. [_gen] is the one piece that survives the move,
// because an await cannot be cancelled and a sentence that lands after Stop
// must not resume it.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:media_kit/media_kit.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/books.dart';
import 'books_controller.dart';
import '../../playback/audio_deck.dart' show kDefaultVolume;

class ReadAloud extends ChangeNotifier {
  ReadAloud(this._books);

  final BooksController _books;

  /// Its own player, not the music deck's: read-aloud does not belong in the
  /// queue, must not appear in the mini player, and stopping the music must not
  /// stop the book. One libmpv instance is still cheaper than the process per
  /// sentence the Slint build spawns.
  Player? _player;

  List<TtsSentence> sentences = const [];
  List<String> voices = const [];
  List<double> speeds = const [1.0];
  String voice = '';

  /// Index into [speeds]; 1 is 1.0×, which is where the Slint stepper starts.
  int speedIndex = 1;
  int active = 0;

  /// The bar is up. Separate from [playing] so pausing does not tear the
  /// sentence list down and lose your place.
  bool open = false;
  bool playing = false;

  /// A synthesiser answered: sentences are spoken rather than merely lit.
  bool audio = false;
  bool neural = false;

  /// What to tell the reader when the voice is not the one that was asked for.
  String note = '';
  bool busy = false;

  /// Bumped by everything that invalidates work already in flight — stop,
  /// pause, a jump, a voice or speed change. An await that returns with a stale
  /// generation drops its result instead of speaking it.
  int _gen = 0;

  String get activeText =>
      active >= 0 && active < sentences.length ? sentences[active].text : '';

  double get speed => speeds[speedIndex.clamp(0, speeds.length - 1)];

  /// Segment the open book, pick the engine, and start.
  Future<void> start() async {
    if (busy) return;
    busy = true;
    notifyListeners();
    try {
      final plan = await booksTtsPlan(voice: voice);
      sentences = plan.sentences;
      voices = plan.voices;
      speeds = plan.speeds.toList();
      voice = plan.voice;
      speedIndex = speedIndex.clamp(0, speeds.length - 1);
      audio = plan.audio;
      neural = plan.neural;
      note = plan.note;
      active = 0;
      open = true;
      busy = false;
      playing = true;
      notifyListeners();
      _run(++_gen);
    } catch (e) {
      // Nothing to read is a normal answer for a comic or a scanned PDF, so it
      // goes on the bar rather than into an error dialog.
      busy = false;
      open = true;
      playing = false;
      sentences = const [];
      audio = false;
      note = '$e';
      notifyListeners();
    }
  }

  void playPause() {
    if (sentences.isEmpty) return;
    playing = !playing;
    _gen++;
    if (!playing) {
      _player?.pause();
    } else {
      _run(_gen);
    }
    notifyListeners();
  }

  /// Move to a sentence. Speaking restarts from there rather than finishing the
  /// one that was in the air, which is what a person pressing "next" means.
  void jump(int index) {
    if (sentences.isEmpty) return;
    active = index.clamp(0, sentences.length - 1);
    _gen++;
    _player?.stop();
    notifyListeners();
    if (playing) _run(_gen);
  }

  void step(int delta) => jump(active + delta);

  void cycleSpeed() {
    speedIndex = (speedIndex + 1) % speeds.length;
    _restartSentence();
  }

  void setVoice(String label) {
    if (label == voice) return;
    voice = label;
    // The engine can change with the voice: a Kokoro voice whose style vector
    // is missing falls back to espeak, and the bar has to say so.
    booksTtsPlan(voice: label).then((p) {
      audio = p.audio;
      neural = p.neural;
      note = p.note;
      notifyListeners();
    }).catchError((_) {});
    _restartSentence();
  }

  /// Re-speak the current sentence under new settings. Cheaper than a full
  /// re-plan: the sentence list does not depend on voice or speed.
  void _restartSentence() {
    _gen++;
    _player?.stop();
    notifyListeners();
    if (playing) _run(_gen);
  }

  Future<void> stop() async {
    _gen++;
    playing = false;
    open = false;
    sentences = const [];
    active = 0;
    note = '';
    notifyListeners();
    await _player?.stop();
  }

  @override
  void dispose() {
    _gen++;
    _player?.dispose();
    _player = null;
    super.dispose();
  }

  /// Speak from [active] until stopped, paused, or out of book.
  Future<void> _run(int gen) async {
    while (playing && gen == _gen && active < sentences.length) {
      final s = sentences[active];
      await _follow(s);
      if (gen != _gen) return;

      if (audio) {
        try {
          final path =
              await booksTtsSay(text: s.text, voice: voice, speed: speed);
          if (gen != _gen || !playing) return;
          final p = _player ??= (Player(
            configuration:
                const PlayerConfiguration(title: 'Tulipix — reading'),
          )..setVolume(kDefaultVolume));
          await p.open(Media(path));
          // `completed` also fires for the previous media, so wait for the
          // edge rather than the current value.
          await p.stream.completed.firstWhere((done) => done);
        } catch (e) {
          // One failed sentence should not end the chapter. Drop to the timer
          // tier and say why, which is also what happens when espeak is
          // uninstalled mid-session.
          if (gen != _gen) return;
          audio = false;
          note = 'Speech stopped working — following the text on a timer. $e';
          notifyListeners();
        }
      } else {
        final ms = await booksTtsPaceMs(text: s.text, speed: speed);
        await Future<void>.delayed(Duration(milliseconds: ms.toInt()));
      }

      if (gen != _gen || !playing) return;
      if (active + 1 >= sentences.length) {
        // Out of book: stay on the last sentence rather than snapping to the
        // start, so "where did it get to" is still answerable.
        playing = false;
        notifyListeners();
        return;
      }
      active++;
      notifyListeners();
    }
  }

  /// Turn the spread when the next sentence is not on it.
  ///
  /// The screen page comes from the bridge because the mapping depends on the
  /// single-page toggle and on right-to-left order — see `books_tts_screen_for`.
  Future<void> _follow(TtsSentence s) async {
    final r = _books.reader;
    if (r == null) return;
    final screen = await booksTtsScreenFor(page: s.page);
    if (screen.toInt() != r.page) {
      await _books.send(BooksCmd.readerJump(page: screen.toInt()));
    }
  }
}

/// The read-aloud bar: what is being read, and the four controls that matter.
///
/// Over the spread rather than in the top bar — it appears only while reading,
/// and the reader's chrome is already full.
class ReadAloudBar extends StatelessWidget {
  const ReadAloudBar({super.key, required this.aloud});

  final ReadAloud aloud;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: aloud,
      builder: (context, _) {
        if (!aloud.open) return const SizedBox.shrink();
        final has = aloud.sentences.isNotEmpty;
        return Align(
          alignment: Alignment.bottomCenter,
          child: Padding(
            padding: const EdgeInsets.only(bottom: 18),
            child: Container(
              constraints: const BoxConstraints(maxWidth: 720),
              padding: const EdgeInsets.fromLTRB(12, 10, 8, 10),
              decoration: BoxDecoration(
                color: t.modal,
                borderRadius: BorderRadius.circular(Tokens.radiusLg),
                border: Border.all(color: t.outline),
                boxShadow: const [
                  BoxShadow(color: Color(0x59000000), blurRadius: 28),
                ],
              ),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Row(
                    children: [
                      _Btn(
                        icon: Icons.skip_previous,
                        tip: 'Previous sentence',
                        onTap: has ? () => aloud.step(-1) : null,
                      ),
                      _Btn(
                        icon: aloud.playing ? Icons.pause : Icons.play_arrow,
                        tip: aloud.playing ? 'Pause' : 'Play',
                        big: true,
                        onTap: has ? aloud.playPause : null,
                      ),
                      _Btn(
                        icon: Icons.skip_next,
                        tip: 'Next sentence',
                        onTap: has ? () => aloud.step(1) : null,
                      ),
                      const SizedBox(width: 6),
                      // Speed as a stepper, not a slider: four stops is the
                      // whole range and a slider would imply more.
                      Tooltip(
                        message: 'Reading speed',
                        child: TextButton(
                          onPressed: has ? aloud.cycleSpeed : null,
                          child: Text('${aloud.speed}×',
                              style: const TextStyle(
                                  fontFeatures: [FontFeature.tabularFigures()],
                                  fontSize: 12.5)),
                        ),
                      ),
                      if (aloud.voices.isNotEmpty) ...[
                        const SizedBox(width: 4),
                        DropdownButton<String>(
                          value: aloud.voice,
                          underline: const SizedBox.shrink(),
                          isDense: true,
                          style: TextStyle(fontSize: 12.5, color: t.text),
                          items: [
                            for (final v in aloud.voices)
                              DropdownMenuItem(value: v, child: Text(v)),
                          ],
                          onChanged: (v) =>
                              v == null ? null : aloud.setVoice(v),
                        ),
                      ],
                      const Spacer(),
                      if (has)
                        Text('${aloud.active + 1} / ${aloud.sentences.length}',
                            style: TextStyle(
                                fontSize: 11.5,
                                color: t.textDim,
                                fontFeatures: const [
                                  FontFeature.tabularFigures()
                                ])),
                      _Btn(
                        icon: Icons.close,
                        tip: 'Stop reading',
                        onTap: aloud.stop,
                      ),
                    ],
                  ),
                  // The engine's own words, when it is not the one that was
                  // asked for. Hidden while the neural voice is running,
                  // because then there is nothing to explain.
                  if (aloud.note.isNotEmpty)
                    Padding(
                      padding: const EdgeInsets.only(top: 6, left: 4, right: 4),
                      child: Row(
                        children: [
                          Icon(Icons.info_outline, size: 13, color: t.textDim),
                          const SizedBox(width: 7),
                          Expanded(
                            child: Text(aloud.note,
                                style: TextStyle(
                                    fontSize: 11.5,
                                    height: 1.35,
                                    color: t.textDim)),
                          ),
                        ],
                      ),
                    ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}

class _Btn extends StatelessWidget {
  const _Btn({
    required this.icon,
    required this.tip,
    required this.onTap,
    this.big = false,
  });

  final IconData icon;
  final String tip;
  final VoidCallback? onTap;
  final bool big;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Tooltip(
      message: tip,
      child: IconButton(
        onPressed: onTap,
        visualDensity: VisualDensity.compact,
        iconSize: big ? 24 : 18,
        color: big ? t.text : t.textDim,
        icon: Icon(icon),
      ),
    );
  }
}
