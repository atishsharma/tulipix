// The keymap, which is the part of the player that can be wrong quietly.
//
// Twenty-one default bindings were written by hand into one map; two of them
// landing on the same key would mean one action silently never fires, and
// nothing on screen would say so. That is what the first test is for.

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:tulipix/playback/video_controls.dart';
import 'package:tulipix/playback/video_keys.dart';

void main() {
  // `KeyBind.of` reads the live modifier state from `HardwareKeyboard`, which
  // is a binding singleton. The app always has one; a plain `test()` does not.
  TestWidgetsFlutterBinding.ensureInitialized();

  test('no two default bindings claim the same key', () {
    final seen = <KeyBind, VideoAction>{};
    for (final entry in kDefaultBinds.entries) {
      final clash = seen[entry.value];
      expect(
        clash,
        isNull,
        reason: '${entry.key.name} and ${clash?.name} both want '
            '${entry.value.label}',
      );
      seen[entry.value] = entry.key;
    }
  });

  test('every action has a default', () {
    for (final action in VideoAction.values) {
      expect(kDefaultBinds[action], isNotNull, reason: action.name);
    }
  });

  test('a bind survives encoding, modifiers included', () {
    const bind = KeyBind(65, ctrl: true, alt: true);
    final back = KeyBind.decode(bind.encode());
    expect(back, bind);
    expect(back!.shift, isFalse);
  });

  test('rubbish decodes to null rather than a wrong key', () {
    expect(KeyBind.decode('nonsense'), isNull);
    expect(KeyBind.decode(':notanumber'), isNull);
    expect(KeyBind.decode(null), isNull);
    expect(KeyBind.decode(42), isNull);
  });

  test('a modifier on its own is not a shortcut', () {
    expect(KeyBind.of(_down(LogicalKeyboardKey.controlLeft)), isNull);
    expect(KeyBind.of(_down(LogicalKeyboardKey.shiftRight)), isNull);
    expect(KeyBind.of(_down(LogicalKeyboardKey.keyA)), isNotNull);
  });

  test('the defaults resolve to the actions they name', () {
    final keymap = VideoKeymap.instance;
    expect(keymap.resolve(_down(LogicalKeyboardKey.space)), VideoAction.playPause);
    expect(keymap.resolve(_down(LogicalKeyboardKey.arrowRight)), VideoAction.seekFwd5);
    expect(keymap.resolve(_down(LogicalKeyboardKey.keyL)), VideoAction.seekFwd10);
    // The one the user asked for by name: Escape puts it in the corner, it
    // does not end playback.
    expect(keymap.resolve(_down(LogicalKeyboardKey.escape)), VideoAction.minimise);
  });

  test('a held key resolves, so seeking can run while it is down', () {
    const repeat = KeyRepeatEvent(
      physicalKey: PhysicalKeyboardKey.arrowRight,
      logicalKey: LogicalKeyboardKey.arrowRight,
      timeStamp: Duration.zero,
    );
    expect(VideoKeymap.instance.resolve(repeat), VideoAction.seekFwd5);
  });

  test('only seeking repeats', () {
    expect(kRepeatableActions, {
      VideoAction.seekBack5,
      VideoAction.seekFwd5,
      VideoAction.seekBack10,
      VideoAction.seekFwd10,
    });
    // The ones that would be actively bad held down.
    expect(kRepeatableActions, isNot(contains(VideoAction.playPause)));
    expect(kRepeatableActions, isNot(contains(VideoAction.screenshot)));
    expect(kRepeatableActions, isNot(contains(VideoAction.close)));
  });

  test('a key up is never a shortcut', () {
    const up = KeyUpEvent(
      physicalKey: PhysicalKeyboardKey.space,
      logicalKey: LogicalKeyboardKey.space,
      timeStamp: Duration.zero,
    );
    expect(VideoKeymap.instance.resolve(up), isNull);
  });

  test('assigning a taken key takes it off whoever had it', () {
    final keymap = VideoKeymap.instance;
    addTearDown(keymap.resetAll);

    final space = kDefaultBinds[VideoAction.playPause]!;
    keymap.assign(VideoAction.screenshot, space);

    expect(keymap.bind(VideoAction.screenshot), space);
    expect(keymap.bind(VideoAction.playPause), isNull);
    expect(keymap.resolve(_down(LogicalKeyboardKey.space)), VideoAction.screenshot);
  });

  test('unbinding leaves the key meaning nothing', () {
    final keymap = VideoKeymap.instance;
    addTearDown(keymap.resetAll);

    keymap.assign(VideoAction.mute, null);
    expect(keymap.bind(VideoAction.mute), isNull);
    expect(keymap.resolve(_down(LogicalKeyboardKey.keyM)), isNull);
  });

  test('reset puts every default back', () {
    final keymap = VideoKeymap.instance;
    keymap.assign(VideoAction.playPause, null);
    keymap.resetAll();
    expect(keymap.bind(VideoAction.playPause), kDefaultBinds[VideoAction.playPause]);
  });

  group('clock', () {
    test('drops the hour when there is not one', () {
      expect(fmtDuration(const Duration(seconds: 5)), '0:05');
      expect(fmtDuration(const Duration(minutes: 3, seconds: 7)), '3:07');
    });

    test('keeps it when there is', () {
      expect(fmtDuration(const Duration(hours: 1, minutes: 2, seconds: 3)), '1:02:03');
      expect(fmtDuration(const Duration(hours: 12)), '12:00:00');
    });

    test('a negative remaining reads as zero, not as minus something', () {
      expect(fmtDuration(const Duration(seconds: -4)), '0:00');
    });
  });
}

KeyDownEvent _down(LogicalKeyboardKey key) => KeyDownEvent(
      physicalKey: PhysicalKeyboardKey.space,
      logicalKey: key,
      timeStamp: Duration.zero,
    );
