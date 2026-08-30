// The video player's keyboard, and the editor for it.
//
// Every key the player answers is rebindable, which is why they are a map
// rather than a `switch` over `LogicalKeyboardKey` — the switch was the whole
// keyboard before this, and there was no way to change any of it.
//
// One key, one action: assigning a key that is already taken unbinds the
// action that had it rather than leaving two claims on the same press. The map
// is stored in settings.json under `player.shortcuts` as JSON, and an action
// missing from that file falls back to its default, so a keymap saved by an
// older build still loads when new actions are added below it.

import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../src/rust/api/videos.dart';

/// Where the keymap is kept. `player.`-prefixed, which is what the bridge's
/// preference hatch will accept.
const String kShortcutsKey = 'player.shortcuts';

/// Everything the player can be asked to do from the keyboard.
///
/// The order here is the order the editor lists them in, and [group] is where
/// the headings fall.
enum VideoAction {
  playPause('Play / pause', 'Playback'),
  speedUp('Speed up', 'Playback'),
  speedDown('Slow down', 'Playback'),
  speedReset('Normal speed', 'Playback'),

  seekBack5('Back 5 seconds', 'Seeking'),
  seekFwd5('Forward 5 seconds', 'Seeking'),
  seekBack10('Back 10 seconds', 'Seeking'),
  seekFwd10('Forward 10 seconds', 'Seeking'),
  prevChapter('Previous chapter', 'Seeking'),
  nextChapter('Next chapter', 'Seeking'),

  volumeUp('Volume up', 'Audio'),
  volumeDown('Volume down', 'Audio'),
  mute('Mute', 'Audio'),
  cycleAudio('Next audio track', 'Audio'),

  toggleSubtitles('Subtitles on / off', 'Subtitles'),
  cycleSubtitle('Next subtitle track', 'Subtitles'),
  findSubtitles('Find subtitles online', 'Subtitles'),

  screenshot('Save a screenshot', 'Window'),
  fullscreen('Fullscreen', 'Window'),
  minimise('Minimise to the corner', 'Window'),
  close('Close the player', 'Window');

  const VideoAction(this.label, this.group);

  final String label;
  final String group;
}

/// A key, with its modifiers. Value type, so a lookup is an `==`.
@immutable
class KeyBind {
  const KeyBind(this.keyId,
      {this.ctrl = false, this.shift = false, this.alt = false});

  /// What is pressed right now, or null if it is only a modifier — holding
  /// Ctrl is not a shortcut waiting to be assigned.
  static KeyBind? of(KeyEvent event) {
    final key = event.logicalKey;
    if (_modifiers.contains(key)) return null;
    final keys = HardwareKeyboard.instance;
    return KeyBind(
      key.keyId,
      ctrl: keys.isControlPressed,
      shift: keys.isShiftPressed,
      alt: keys.isAltPressed,
    );
  }

  final int keyId;
  final bool ctrl;
  final bool shift;
  final bool alt;

  String encode() =>
      '${ctrl ? 'c' : ''}${shift ? 's' : ''}${alt ? 'a' : ''}:$keyId';

  static KeyBind? decode(Object? raw) {
    if (raw is! String) return null;
    final at = raw.indexOf(':');
    if (at < 0) return null;
    final id = int.tryParse(raw.substring(at + 1));
    if (id == null) return null;
    final mods = raw.substring(0, at);
    return KeyBind(
      id,
      ctrl: mods.contains('c'),
      shift: mods.contains('s'),
      alt: mods.contains('a'),
    );
  }

  /// What the editor and the tooltips print.
  String get label {
    final parts = [
      if (ctrl) 'Ctrl',
      if (alt) 'Alt',
      if (shift) 'Shift',
      _name,
    ];
    return parts.join(' + ');
  }

  String get _name {
    final named = _keyNames[keyId];
    if (named != null) return named;
    final key = LogicalKeyboardKey.findKeyByKeyId(keyId);
    final printed = key?.keyLabel.trim() ?? '';
    return printed.isEmpty ? 'Key $keyId' : printed.toUpperCase();
  }

  @override
  bool operator ==(Object other) =>
      other is KeyBind &&
      other.keyId == keyId &&
      other.ctrl == ctrl &&
      other.shift == shift &&
      other.alt == alt;

  @override
  int get hashCode => Object.hash(keyId, ctrl, shift, alt);
}

/// Keys whose `keyLabel` is empty or unhelpful. Everything else prints itself.
final Map<int, String> _keyNames = {
  LogicalKeyboardKey.space.keyId: 'Space',
  LogicalKeyboardKey.arrowLeft.keyId: 'Left',
  LogicalKeyboardKey.arrowRight.keyId: 'Right',
  LogicalKeyboardKey.arrowUp.keyId: 'Up',
  LogicalKeyboardKey.arrowDown.keyId: 'Down',
  LogicalKeyboardKey.escape.keyId: 'Esc',
  LogicalKeyboardKey.enter.keyId: 'Enter',
  LogicalKeyboardKey.backspace.keyId: 'Backspace',
  LogicalKeyboardKey.delete.keyId: 'Delete',
  LogicalKeyboardKey.tab.keyId: 'Tab',
  LogicalKeyboardKey.home.keyId: 'Home',
  LogicalKeyboardKey.end.keyId: 'End',
  LogicalKeyboardKey.pageUp.keyId: 'Page Up',
  LogicalKeyboardKey.pageDown.keyId: 'Page Down',
  LogicalKeyboardKey.bracketLeft.keyId: '[',
  LogicalKeyboardKey.bracketRight.keyId: ']',
};

final Set<LogicalKeyboardKey> _modifiers = {
  LogicalKeyboardKey.control,
  LogicalKeyboardKey.controlLeft,
  LogicalKeyboardKey.controlRight,
  LogicalKeyboardKey.shift,
  LogicalKeyboardKey.shiftLeft,
  LogicalKeyboardKey.shiftRight,
  LogicalKeyboardKey.alt,
  LogicalKeyboardKey.altLeft,
  LogicalKeyboardKey.altRight,
  LogicalKeyboardKey.meta,
  LogicalKeyboardKey.metaLeft,
  LogicalKeyboardKey.metaRight,
};

/// What the player answers out of the box.
///
/// Arrows for the small jump and J/L for the larger one, which is the pairing
/// every other player uses; Escape minimises rather than closes, because
/// closing loses your place in a film and minimising is what you nearly always
/// meant.
final Map<VideoAction, KeyBind> kDefaultBinds = {
  VideoAction.playPause: KeyBind(LogicalKeyboardKey.space.keyId),
  VideoAction.speedUp: KeyBind(LogicalKeyboardKey.bracketRight.keyId),
  VideoAction.speedDown: KeyBind(LogicalKeyboardKey.bracketLeft.keyId),
  VideoAction.speedReset: KeyBind(LogicalKeyboardKey.backspace.keyId),
  VideoAction.seekBack5: KeyBind(LogicalKeyboardKey.arrowLeft.keyId),
  VideoAction.seekFwd5: KeyBind(LogicalKeyboardKey.arrowRight.keyId),
  VideoAction.seekBack10: KeyBind(LogicalKeyboardKey.keyJ.keyId),
  VideoAction.seekFwd10: KeyBind(LogicalKeyboardKey.keyL.keyId),
  VideoAction.prevChapter: KeyBind(LogicalKeyboardKey.pageUp.keyId),
  VideoAction.nextChapter: KeyBind(LogicalKeyboardKey.pageDown.keyId),
  VideoAction.volumeUp: KeyBind(LogicalKeyboardKey.arrowUp.keyId),
  VideoAction.volumeDown: KeyBind(LogicalKeyboardKey.arrowDown.keyId),
  VideoAction.mute: KeyBind(LogicalKeyboardKey.keyM.keyId),
  VideoAction.cycleAudio: KeyBind(LogicalKeyboardKey.keyA.keyId),
  VideoAction.toggleSubtitles: KeyBind(LogicalKeyboardKey.keyV.keyId),
  VideoAction.cycleSubtitle: KeyBind(LogicalKeyboardKey.keyS.keyId),
  VideoAction.findSubtitles: KeyBind(LogicalKeyboardKey.keyD.keyId),
  VideoAction.screenshot: KeyBind(LogicalKeyboardKey.f12.keyId),
  VideoAction.fullscreen: KeyBind(LogicalKeyboardKey.keyF.keyId),
  VideoAction.minimise: KeyBind(LogicalKeyboardKey.escape.keyId),
  VideoAction.close: KeyBind(LogicalKeyboardKey.keyQ.keyId),
};

/// The actions that keep going while the key is held.
///
/// Seeking only. Everything else is a toggle, a jump or a dialog, and none of
/// those improve by happening thirty times a second.
const Set<VideoAction> kRepeatableActions = {
  VideoAction.seekBack5,
  VideoAction.seekFwd5,
  VideoAction.seekBack10,
  VideoAction.seekFwd10,
};

/// The live keymap. One per app: the player is a singleton surface and the
/// editor writes what the next film will read.
class VideoKeymap extends ChangeNotifier {
  static final VideoKeymap instance = VideoKeymap._();

  VideoKeymap._() {
    _binds.addAll(kDefaultBinds);
    _load();
  }

  final Map<VideoAction, KeyBind?> _binds = {};

  KeyBind? bind(VideoAction action) => _binds[action];

  /// The action this press means, or null for a key nothing claims.
  ///
  /// A held key counts: the OS sends `KeyRepeatEvent` for it, and seeking is
  /// meant to run while the key is down. Whether an action *should* repeat is
  /// the caller's call — holding play/pause thirty times a second is not a
  /// feature — so this only says what was pressed.
  VideoAction? resolve(KeyEvent event) {
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) return null;
    final pressed = KeyBind.of(event);
    if (pressed == null) return null;
    for (final entry in _binds.entries) {
      if (entry.value == pressed) return entry.key;
    }
    return null;
  }

  /// Assign, or pass null to unbind.
  ///
  /// A key already spoken for is taken from whoever had it: two actions on one
  /// press means the winner is whichever the map iterates first, which is not
  /// something anyone can reason about from the editor.
  void assign(VideoAction action, KeyBind? bind) {
    if (bind != null) {
      for (final other in _binds.keys.toList()) {
        if (other != action && _binds[other] == bind) _binds[other] = null;
      }
    }
    _binds[action] = bind;
    _save();
    notifyListeners();
  }

  void resetAll() {
    _binds
      ..clear()
      ..addAll(kDefaultBinds);
    // Empty removes the key, so settings.json goes back to not mentioning the
    // player at all rather than storing a copy of the defaults.
    try {
      playerPrefSet(key: kShortcutsKey, value: '');
    } catch (_) {
      // As in `_save`.
    }
    notifyListeners();
  }

  /// Which action currently owns this key, ignoring [besides]. The editor uses
  /// it to warn before a reassignment takes a key off something else.
  VideoAction? owner(KeyBind bind, {VideoAction? besides}) {
    for (final entry in _binds.entries) {
      if (entry.key != besides && entry.value == bind) return entry.key;
    }
    return null;
  }

  void _load() {
    try {
      final raw = playerPrefGet(key: kShortcutsKey);
      if (raw.trim().isEmpty) return;
      final decoded = jsonDecode(raw);
      if (decoded is! Map) return;
      for (final action in VideoAction.values) {
        if (!decoded.containsKey(action.name)) continue;
        // Present but null is a deliberate unbind, which is why this reads
        // `containsKey` rather than treating a missing entry the same way.
        _binds[action] = KeyBind.decode(decoded[action.name]);
      }
    } catch (_) {
      // Everything here is best-effort on purpose, and the catch covers the
      // read as well as the parse: a keymap that cannot be loaded — bad JSON,
      // or no bridge at all — leaves the defaults in place, and a player with
      // the default keyboard beats a player with none.
    }
  }

  void _save() {
    final map = {for (final e in _binds.entries) e.key.name: e.value?.encode()};
    try {
      playerPrefSet(key: kShortcutsKey, value: jsonEncode(map));
    } catch (_) {
      // The binding still applies to this session; only remembering it failed.
    }
  }
}

/// The editor, as a dialog over the player.
Future<void> showVideoShortcuts(BuildContext context) {
  return showDialog<void>(
    context: context,
    builder: (context) => const _ShortcutEditor(),
  );
}

class _ShortcutEditor extends StatefulWidget {
  const _ShortcutEditor();

  @override
  State<_ShortcutEditor> createState() => _ShortcutEditorState();
}

class _ShortcutEditorState extends State<_ShortcutEditor> {
  /// The row waiting for a key, if any.
  VideoAction? _capturing;

  final FocusNode _focus = FocusNode();

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent event) {
    final action = _capturing;
    if (action == null || event is! KeyDownEvent) return KeyEventResult.ignored;

    // Escape leaves the row as it was; Delete clears it. Neither can be bound
    // from here — Escape because it is the way out of every capture, Delete
    // because it is the way to unbind. Both are still assignable by editing
    // settings.json, which is a fair place to put an unusual choice.
    if (event.logicalKey == LogicalKeyboardKey.escape) {
      setState(() => _capturing = null);
      return KeyEventResult.handled;
    }
    if (event.logicalKey == LogicalKeyboardKey.delete) {
      VideoKeymap.instance.assign(action, null);
      setState(() => _capturing = null);
      return KeyEventResult.handled;
    }

    final bind = KeyBind.of(event);
    if (bind == null) return KeyEventResult.handled;
    VideoKeymap.instance.assign(action, bind);
    setState(() => _capturing = null);
    return KeyEventResult.handled;
  }

  @override
  Widget build(BuildContext context) {
    final keymap = VideoKeymap.instance;
    return Focus(
      focusNode: _focus,
      autofocus: true,
      onKeyEvent: _onKey,
      child: AlertDialog(
        title: const Text('Player shortcuts'),
        content: SizedBox(
          width: 460,
          height: 520,
          child: AnimatedBuilder(
            animation: keymap,
            builder: (context, _) => ListView(
              children: [
                for (final group in _groups)
                  ..._groupRows(context, keymap, group),
              ],
            ),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () {
              keymap.resetAll();
              setState(() => _capturing = null);
            },
            child: const Text('Reset all'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(),
            child: const Text('Done'),
          ),
        ],
      ),
    );
  }

  /// The group headings, in the order the actions declare them.
  static final List<String> _groups = {
    for (final a in VideoAction.values) a.group,
  }.toList();

  List<Widget> _groupRows(
      BuildContext context, VideoKeymap keymap, String group) {
    final theme = Theme.of(context);
    return [
      Padding(
        padding: const EdgeInsets.fromLTRB(4, 16, 4, 6),
        child: Text(
          group.toUpperCase(),
          style: theme.textTheme.labelSmall?.copyWith(letterSpacing: 1.1),
        ),
      ),
      for (final action in VideoAction.values.where((a) => a.group == group))
        _Row(
          action: action,
          bind: keymap.bind(action),
          capturing: _capturing == action,
          onTap: () => setState(() => _capturing = action),
        ),
    ];
  }
}

class _Row extends StatelessWidget {
  const _Row({
    required this.action,
    required this.bind,
    required this.capturing,
    required this.onTap,
  });

  final VideoAction action;
  final KeyBind? bind;
  final bool capturing;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final label = capturing ? 'Press a key…' : bind?.label ?? 'Unbound';
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 2),
      child: Row(
        children: [
          Expanded(child: Text(action.label)),
          const SizedBox(width: 12),
          OutlinedButton(
            onPressed: onTap,
            style: OutlinedButton.styleFrom(
              minimumSize: const Size(132, 34),
              foregroundColor: capturing
                  ? theme.colorScheme.primary
                  : bind == null
                      ? theme.disabledColor
                      : null,
            ),
            child: Text(label, maxLines: 1, overflow: TextOverflow.ellipsis),
          ),
        ],
      ),
    );
  }
}
