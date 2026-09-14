// First run: what the ten cards collect, and what happens when they close.
//
// Answers are staged here and written nowhere until the last card. Two reasons:
// a flow you can walk backwards through must not have already applied card 3
// by the time you are on card 7, and a scan started from the library card
// would be fighting for the disk while someone is still reading.
//
// Every field is nullable, and null means *not answered* rather than "the
// default". Skipping a card writes nothing at all — it does not write a
// default over what is there. That matters on the third way in: someone who
// opens this from Settings › Advanced to change one thing must not have the
// other nine reset around them.


import 'package:flutter/foundation.dart';

import '../../src/rust/api/onboarding.dart';
import '../../src/rust/api/settings.dart';
import '../lock/lock_controller.dart';
import '../shell_controller.dart';

/// What the flow collected. Anything still null was skipped.
class OnboardingAnswers {
  // Card 2 — you.
  String? name;
  String? emoji;

  /// A cropped PNG from the shared cropper, or null for "leave it".
  Uint8List? avatar;
  Uint8List? cover;

  // Card 3 — the look.
  String? theme;
  bool? followAccent;
  bool? reduceMotion;
  bool? osFontScale;

  // Card 4 — the material.
  String? designLanguage;

  // Card 5 — home.
  String? homeLayout;
  bool? musicLeft;

  // Card 6 — the library. Empty is the same as skipped: nothing to add.
  final List<String> folders = [];

  // Card 7 — keeping up.
  String? cadence;
  bool? powerAware;
  String? exclusions;

  // Card 8 — online. Only the ones actually touched.
  final Map<String, bool> services = {};

  // Card 9 — locking up.
  String? pin;
  String? lockAfter;

  bool get touchedProfile =>
      name != null || emoji != null || avatar != null || cover != null;
}

class OnboardingController extends ChangeNotifier {
  static final OnboardingController instance = OnboardingController._();

  OnboardingController._();

  bool _open = false;
  bool get open => _open;

  /// True while the last card is writing. The flow stays up and its buttons go
  /// quiet: a settings write is a dozen round trips and closing on the first
  /// one would leave the rest landing behind a window that had gone.
  bool _committing = false;
  bool get committing => _committing;

  /// Asked once, before the first frame. A machine that has been through this
  /// never sees it again — the flag is the one the Slint build already reads,
  /// so a library set up in either front end is set up in both.
  Future<void> checkFirstRun() async {
    try {
      if (!await onboardingNeeded()) return;
    } catch (_) {
      // A bridge that cannot answer is not a reason to block the app behind a
      // setup screen. Worst case someone sets their folders up by hand.
      return;
    }
    _open = true;
    notifyListeners();
  }

  /// Settings › Advanced › Run the setup again.
  void start() {
    if (_open) return;
    _open = true;
    notifyListeners();
  }

  /// Write what was answered, then close. Called by Finish *and* by Skip
  /// setup: skipping is an answer, and asking again next launch would not be
  /// taking it.
  Future<void> finish(OnboardingAnswers a) async {
    if (_committing) return;
    _committing = true;
    notifyListeners();
    try {
      await _commit(a);
    } catch (e) {
      // Half-written is still better than not written, and the pages that own
      // these settings all show what actually landed.
      debugPrint('onboarding: $e');
    }
    try {
      await onboardingFinish();
    } catch (_) {}
    _committing = false;
    _open = false;
    notifyListeners();
    // The theme, the design language and the accent all ride the shell
    // snapshot; the lock reads its own keys. Neither notices a write on its
    // own, so both are told.
    await ShellController.instance.refresh();
    await LockController.instance.reload();
  }

  /// Everything goes through the settings commands that already exist, one at
  /// a time and in card order. No setting has a second write path because of
  /// this flow.
  Future<void> _commit(OnboardingAnswers a) async {
    // The profile is one command for four fields, so the three nobody touched
    // have to be carried over rather than blanked. This snapshot is where they
    // come from.
    final now = await settingsDispatch(cmd: const SettingsCmd.refresh());

    if (a.touchedProfile) {
      await settingsDispatch(
        cmd: SettingsCmd.saveProfile(
          name: a.name ?? now.displayName,
          emoji: a.emoji ?? now.avatarEmoji,
          logo: now.logoChoice,
          avatar: a.avatar,
          cover: a.cover,
        ),
      );
    }

    if (a.theme != null) {
      await settingsDispatch(cmd: SettingsCmd.setTheme(theme: a.theme!));
    }
    await _toggle('follow-system-accent', a.followAccent);
    if (a.reduceMotion != null) {
      await settingsDispatch(
          cmd: SettingsCmd.setReduceMotion(on_: a.reduceMotion!));
    }
    await _toggle('follow-os-font-scale', a.osFontScale);

    await _text('ui.design-language', a.designLanguage);

    if (a.homeLayout != null) {
      await settingsDispatch(
          cmd: SettingsCmd.setHomeLayout(layout: a.homeLayout!));
    }
    await _toggle('home.music-left', a.musicLeft);

    // Last, among the settings: adding a folder is what the first scan reads,
    // and the scan is kicked off by the watcher once the list changes. Every
    // other answer is already in place by the time it looks.
    for (final path in a.folders) {
      await settingsDispatch(cmd: SettingsCmd.libAdd(path: path));
    }

    await _text('scan.cadence', a.cadence);
    await _toggle('power-aware', a.powerAware);
    await _text('scan.exclude', a.exclusions);

    for (final e in a.services.entries) {
      await _toggle(e.key, e.value);
    }

    await _text('lock.after', a.lockAfter);
    // The PIN goes last of all: it is the one write that changes whether the
    // next launch asks for something, and everything above should already be
    // saved by the time it does.
    await _text('lock.pin', a.pin);
  }

  Future<void> _toggle(String key, bool? on) async {
    if (on == null) return;
    await settingsDispatch(cmd: SettingsCmd.toggle(key: key, on_: on));
  }

  Future<void> _text(String key, String? value) async {
    if (value == null) return;
    await settingsDispatch(cmd: SettingsCmd.setText(key: key, value: value));
  }
}
