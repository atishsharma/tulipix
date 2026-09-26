// First run — ten cards over a dimmed app (docs/mockups/Extra/onboarding.html).
//
// Reached three ways: the first time Tulipix opens, after Settings › Data ›
// Start over, and from Settings › Advanced. The third is why no card may
// assume an empty machine: it reads what is already set and offers to change
// it, rather than presenting blanks.
//
// Every card is skippable, and a skipped card writes nothing — see
// [OnboardingAnswers]. The rail marks skipped steps so the last card can say
// "skipped" rather than claim a value nobody chose.

import 'dart:typed_data';

import 'package:flutter/material.dart';

import '../../design/design_language.dart';
import '../../design/tokens.dart';
import '../../sections/settings/profile_cropper.dart';
import '../../sections/settings/settings_controller.dart' show kHomeLayouts;
import '../../sections/settings/settings_kit.dart';
import '../../src/rust/api/onboarding.dart';
import 'onboarding_controller.dart';

/// Wraps the shell. Inside the lock screen, not outside it: a locked window
/// answers nothing, setup included.
class OnboardingOverlay extends StatefulWidget {
  const OnboardingOverlay({super.key, required this.child});

  final Widget child;

  @override
  State<OnboardingOverlay> createState() => _OnboardingOverlayState();
}

class _OnboardingOverlayState extends State<OnboardingOverlay> {
  final OnboardingController _c = OnboardingController.instance;

  @override
  void initState() {
    super.initState();
    _c.addListener(_onChange);
    WidgetsBinding.instance.addPostFrameCallback((_) => _c.checkFirstRun());
  }

  @override
  void dispose() {
    _c.removeListener(_onChange);
    super.dispose();
  }

  void _onChange() => setState(() {});

  @override
  Widget build(BuildContext context) => Stack(
        fit: StackFit.expand,
        children: [
          // Focus is excluded rather than the app being removed: it keeps its
          // State, so the shell behind is the one that was already there when
          // this is run from Settings.
          ExcludeFocus(excluding: _c.open, child: widget.child),
          if (_c.open) const _Flow(),
        ],
      );
}

// ── the flow ────────────────────────────────────────────────────────────────

/// What each card is called, in the rail and on the summary.
const List<String> _titles = [
  'Welcome',
  'You',
  'Look',
  'Material',
  'Home',
  'Library',
  'Keeping up',
  'Online',
  'Lock',
  'Ready',
];

/// The line under the footer buttons: what this card actually does, in the
/// place where someone is deciding whether to answer it.
const List<String> _why = [
  'Nothing here is permanent — every answer lives in Settings afterwards.',
  'Shown on the home screen and at the top of Settings. Local only.',
  'Applies the moment you finish, including to this window.',
  'The material everything is drawn in. Orthogonal to the theme.',
  'Changeable any time from Settings › You & Home.',
  'No folder is read until you close this.',
  'The watcher runs regardless; this is the backstop sweep.',
  'Every one of these is off until you turn it on.',
  'Covers the window, not the files on disk.',
  'Scanning starts in the background as soon as this closes.',
];

const int _lastCard = 9;

class _Flow extends StatefulWidget {
  const _Flow();

  @override
  State<_Flow> createState() => _FlowState();
}

class _FlowState extends State<_Flow> {
  final OnboardingController _c = OnboardingController.instance;
  final OnboardingAnswers _a = OnboardingAnswers();

  int _at = 0;

  /// Which way the last move went, so a card enters from the side it came
  /// from. Ambiguous direction is the one thing that makes a wizard feel like
  /// a slideshow.
  int _dir = 1;

  final List<bool> _skipped = List<bool>.filled(_titles.length, false);

  /// Suggestions for the library card, fetched once when the flow opens rather
  /// than when the card is reached: walking four home folders takes a moment,
  /// and by card 6 it has long since finished.
  List<SuggestedFolder>? _found;
  final Set<String> _picked = <String>{};

  @override
  void initState() {
    super.initState();
    // The overlay hands this a `const _Flow()`, so a rebuild up there does not
    // reach this State. Without its own listener the Finish button never goes
    // busy while the commit runs.
    _c.addListener(_onController);
    onboardingSuggestions().then((list) {
      if (!mounted) return;
      setState(() {
        _found = list;
        // Everything found is ticked: someone who wants all of it presses Next
        // once, and someone who wants less unticks. The other way round makes
        // the common case the most work.
        _picked.addAll(list.map((f) => f.path));
      });
    }).catchError((_) {
      if (mounted) setState(() => _found = const <SuggestedFolder>[]);
    });
  }

  @override
  void dispose() {
    _c.removeListener(_onController);
    super.dispose();
  }

  void _onController() {
    if (mounted) setState(() {});
  }

  void _go(int to, int dir) {
    if (to < 0 || to > _lastCard || to == _at) return;
    setState(() {
      _at = to;
      _dir = dir;
    });
  }

  void _next() {
    if (_at == _lastCard) {
      _finish();
      return;
    }
    _skipped[_at] = false;
    _go(_at + 1, 1);
  }

  void _skip() {
    setState(() => _skipped[_at] = true);
    if (_at == _lastCard) {
      _finish();
      return;
    }
    _go(_at + 1, 1);
  }

  void _skipAll() {
    for (var i = 0; i <= _lastCard; i++) {
      _skipped[i] = true;
    }
    _finish();
  }

  void _finish() {
    _a.folders
      ..clear()
      ..addAll(_skipped[5] ? const <String>[] : _picked);
    _c.finish(_a);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: Colors.transparent,
      child: Stack(
        fit: StackFit.expand,
        children: [
          // The app is still behind, and still visible: this is setup for
          // something, not a splash screen in front of nothing.
          const _Scrim(),
          Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 840, maxHeight: 640),
              child: Padding(
                padding: const EdgeInsets.all(24),
                child: DecoratedBox(
                  decoration: BoxDecoration(
                    color: t.modal,
                    borderRadius: BorderRadius.circular(24),
                    border: Border.all(color: t.outlineStrong),
                    boxShadow: const [
                      BoxShadow(
                          color: Color(0x99000000),
                          blurRadius: 60,
                          offset: Offset(0, 24)),
                    ],
                  ),
                  child: ClipRRect(
                    borderRadius: BorderRadius.circular(24),
                    child: Column(
                      children: [
                        _head(t),
                        Expanded(child: _deck()),
                        _foot(t),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _head(Tokens t) => Container(
        padding: const EdgeInsets.fromLTRB(20, 16, 14, 13),
        decoration: BoxDecoration(
          border: Border(bottom: BorderSide(color: t.outline)),
        ),
        child: Row(
          children: [
            Container(
              width: 26,
              height: 26,
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(9),
                gradient: const LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: [Tokens.brand, Tokens.brand2, Color(0xFF0EA5E9)],
                ),
              ),
            ),
            const SizedBox(width: 10),
            Text('Tulipix',
                style: TextStyle(
                    fontSize: 14, fontWeight: FontWeight.w700, color: t.text)),
            const SizedBox(width: 16),
            Expanded(
              child: Row(
                children: [
                  for (var i = 0; i <= _lastCard; i++) ...[
                    if (i > 0) const SizedBox(width: 5),
                    Expanded(
                      child: _Pip(
                        state: _skipped[i]
                            ? _PipState.skipped
                            : i == _at
                                ? _PipState.now
                                : i < _at
                                    ? _PipState.done
                                    : _PipState.todo,
                        onTap: () => _go(i, i > _at ? 1 : -1),
                        label: _titles[i],
                      ),
                    ),
                  ],
                ],
              ),
            ),
            const SizedBox(width: 14),
            Text('${_at + 1} / ${_titles.length}',
                style: TextStyle(
                    fontSize: 11,
                    fontFeatures: const [FontFeature.tabularFigures()],
                    fontWeight: FontWeight.w600,
                    color: t.textDim)),
            const SizedBox(width: 8),
            SmallBtn(
              label: 'Skip setup',
              ghost: true,
              onTap: _c.committing ? null : _skipAll,
            ),
          ],
        ),
      );

  Widget _deck() => AnimatedSwitcher(
        duration: const Duration(milliseconds: 260),
        switchInCurve: Curves.easeOutCubic,
        switchOutCurve: Curves.easeInCubic,
        transitionBuilder: (child, anim) {
          // The outgoing child animates with the *incoming* direction, so both
          // halves travel the same way rather than meeting in the middle.
          final slide = Tween<Offset>(
            begin: Offset(0.06 * _dir, 0),
            end: Offset.zero,
          ).animate(anim);
          return FadeTransition(
            opacity: anim,
            child: SlideTransition(position: slide, child: child),
          );
        },
        layoutBuilder: (current, previous) => Stack(
          alignment: Alignment.topLeft,
          children: [...previous, if (current != null) current],
        ),
        child: KeyedSubtree(
          key: ValueKey<int>(_at),
          child: Scrollbar(
            child: SingleChildScrollView(
              padding: const EdgeInsets.fromLTRB(28, 24, 28, 24),
              child: _card(_at),
            ),
          ),
        ),
      );

  Widget _foot(Tokens t) => Container(
        padding: const EdgeInsets.fromLTRB(20, 13, 20, 13),
        decoration: BoxDecoration(
          color: t.panel2,
          border: Border(top: BorderSide(color: t.outline)),
        ),
        child: Row(
          children: [
            SmallBtn(
              label: 'Back',
              icon: Icons.chevron_left,
              ghost: true,
              onTap: _at == 0 || _c.committing ? null : () => _go(_at - 1, -1),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Text(_why[_at],
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11.5, color: t.textDim)),
            ),
            const SizedBox(width: 12),
            if (_at != _lastCard)
              SmallBtn(
                label: 'Skip this',
                ghost: true,
                onTap: _c.committing ? null : _skip,
              ),
            const SizedBox(width: 8),
            SmallBtn(
              label: _at == _lastCard ? 'Open Tulipix' : 'Next',
              icon: _at == _lastCard ? Icons.arrow_forward : Icons.chevron_right,
              primary: true,
              busy: _c.committing,
              onTap: _c.committing ? null : _next,
            ),
          ],
        ),
      );

  // ── the ten ───────────────────────────────────────────────────────────────

  Widget _card(int i) => switch (i) {
        0 => _welcome(),
        1 => _you(),
        2 => _look(),
        3 => _material(),
        4 => _home(),
        5 => _library(),
        6 => _keepingUp(),
        7 => _online(),
        8 => _lock(),
        _ => _ready(),
      };

  Widget _welcome() => const _Card(
        kicker: 'Welcome',
        tint: Tokens.brand,
        title: 'Everything you own, in one place — and it stays here.',
        blurb: 'Tulipix reads the folders you already have. Your photos, '
            'films, music and books are never copied, moved or uploaded; what '
            'it builds is an index that lives on this computer. Setting up '
            'takes about two minutes, and you can leave any card as it is.',
        children: [
          _Rows([
            _Row(
              title: 'Anything sent anywhere',
              note: 'Online lookups stay off until you turn them on, card 8',
              trailing: StateChip('Nothing, so far', tint: Tokens.ok),
            ),
            _Row(
              title: 'Where this lives',
              note: 'One folder for settings, one for the index. Both yours',
              trailing: StateChip('On this computer'),
            ),
          ]),
        ],
      );

  Widget _you() => _Card(
        kicker: 'Card 2 · You',
        tint: Tokens.secPhotos,
        title: 'What should Tulipix call you?',
        blurb: 'The name and face on the home screen and at the top of '
            'Settings. There is no account behind it — a local profile, '
            'nothing more.',
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _AvatarBubble(emoji: _a.emoji ?? '🌷', png: _a.avatar),
              const SizedBox(width: 18),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    FieldBox(
                      value: _a.name ?? '',
                      hint: 'Your name',
                      onSubmit: (v) => setState(() =>
                          _a.name = v.trim().isEmpty ? null : v.trim()),
                    ),
                    const SizedBox(height: 12),
                    Wrap(
                      spacing: 8,
                      runSpacing: 8,
                      children: [
                        SmallBtn(
                          label: 'Photo or emoji…',
                          icon: Icons.face_outlined,
                          onTap: _pickAvatar,
                        ),
                        SmallBtn(
                          label: 'Header image…',
                          icon: Icons.panorama_outlined,
                          onTap: _pickCover,
                        ),
                      ],
                    ),
                  ],
                ),
              ),
            ],
          ),
          if (_a.cover != null)
            ClipRRect(
              borderRadius: BorderRadius.circular(12),
              child: Image.memory(_a.cover!,
                  height: 92, width: double.infinity, fit: BoxFit.cover),
            ),
          _note(context,
              'Both open the cropper the Settings profile tab uses. The header '
              'sits behind your name at the top of Settings; skipping it keeps '
              'the gradient.'),
        ],
      );

  Future<void> _pickAvatar() async {
    final r = await showCropper(
      context,
      kind: CropKind.avatar,
      source: _a.avatar,
      emoji: _a.emoji ?? '',
      hasPicture: _a.avatar != null,
    );
    if (r == null || !mounted) return;
    setState(() {
      if (r.png != null) _a.avatar = r.png;
      if (r.remove) _a.avatar = Uint8List(0);
      if (r.emoji != null) _a.emoji = r.emoji;
    });
  }

  Future<void> _pickCover() async {
    final r = await showCropper(
      context,
      kind: CropKind.cover,
      source: _a.cover,
      hasPicture: _a.cover != null,
    );
    if (r == null || !mounted) return;
    setState(() {
      if (r.png != null) _a.cover = r.png;
      if (r.remove) _a.cover = Uint8List(0);
    });
  }

  Widget _look() => _Card(
        kicker: 'Card 3 · The look',
        tint: Tokens.secVideos,
        title: 'Light, dark, or as dark as your screen goes.',
        blurb: 'OLED collapses every dark surface to true black, which '
            'switches those pixels off entirely. On a laptop it mostly just '
            'looks sharper.',
        children: [
          _PickGrid(
            columns: 4,
            picks: [
              (
                id: 'system',
                title: 'Follow system',
                note: 'Changes when your desktop does',
                swatch: _themeSwatch(const [Color(0xFFEDE9F8), Color(0xFF141518)]),
              ),
              (
                id: 'light',
                title: 'Light',
                note: 'Lavender-tinted, not plain white',
                swatch: _themeSwatch(const [Color(0xFFEDE9F8)]),
              ),
              (
                id: 'dark',
                title: 'Dark',
                note: 'The one it ships with',
                swatch: _themeSwatch(const [Color(0xFF141518)]),
              ),
              (
                id: 'extra-dark',
                title: 'OLED black',
                note: 'True black, app-wide',
                swatch: _themeSwatch(const [Color(0xFF000000)]),
              ),
            ],
            value: _a.theme,
            tint: Tokens.secVideos,
            onPick: (id) => setState(() => _a.theme = id),
          ),
          _Rows([
            _Row(
              title: 'Follow the system accent',
              note: 'Uses your desktop colour instead of each section\'s own',
              trailing: _switch(_a.followAccent ?? false,
                  (v) => setState(() => _a.followAccent = v)),
            ),
            _Row(
              title: 'Reduce motion',
              note: 'Cross-fades instead of slides, everywhere including here',
              trailing: _switch(_a.reduceMotion ?? false,
                  (v) => setState(() => _a.reduceMotion = v)),
            ),
            _Row(
              title: 'Honour the desktop text size',
              note: 'Off draws everything at 100 %, whatever the desktop asks',
              trailing: _switch(_a.osFontScale ?? true,
                  (v) => setState(() => _a.osFontScale = v)),
            ),
          ]),
        ],
      );

  Widget _themeSwatch(List<Color> colors) => Container(
        height: 54,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(10),
          gradient: LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: colors.length == 1 ? [colors.first, colors.first] : colors,
            stops: colors.length == 1 ? null : const [0.5, 0.5],
          ),
        ),
      );

  Widget _material() => _Card(
        kicker: 'Card 4 · The material',
        tint: Tokens.secMusic,
        title: 'What should it be drawn in?',
        blurb: 'The design language picks the material; the theme picks the '
            'lighting. Every language carries Light, Dark and OLED, so this '
            'and the last card do not fight.',
        children: [
          _PickGrid(
            columns: 2,
            picks: [
              for (final l in DesignLanguage.values)
                (id: l.id, title: l.name, note: l.blurb, swatch: null),
            ],
            value: _a.designLanguage,
            tint: Tokens.secMusic,
            onPick: (id) => setState(() => _a.designLanguage = id),
          ),
          _note(context,
              'Music is the section that changes most between them. The rest '
              'of the app follows as far as each language reaches.'),
        ],
      );

  Widget _home() => _Card(
        kicker: 'Card 5 · Home',
        tint: Tokens.secCloud,
        title: 'What do you want to see when it opens?',
        blurb: 'Every one draws the same snapshot. What differs is what leads — '
            'the thing you were doing, the things you own, or one big picture.',
        children: [
          _PickGrid(
            columns: 2,
            picks: [
              for (final l in kHomeLayouts)
                (id: l.id, title: l.label, note: l.note, swatch: null),
            ],
            value: _a.homeLayout,
            tint: Tokens.secCloud,
            onPick: (id) => setState(() => _a.homeLayout = id),
          ),
          _Rows([
            _Row(
              title: 'Put music on the left',
              note: 'Swaps the now-playing column to the other side',
              trailing: _switch(_a.musicLeft ?? false,
                  (v) => setState(() => _a.musicLeft = v)),
            ),
          ]),
        ],
      );

  Widget _library() {
    final found = _found;
    return _Card(
      kicker: 'Card 6 · Your library',
      tint: Tokens.secBooks,
      title: 'Which folders should Tulipix read?',
      blurb: found == null
          ? 'Looking in the usual places…'
          : found.isEmpty
              ? 'Nothing turned up in the usual places — add your folders from '
                  'Settings › Libraries once you are in.'
              : 'These were found in the usual places. Untick anything you do '
                  'not want. Nothing is read until you finish.',
      children: [
        if (found == null)
          const Padding(
            padding: EdgeInsets.symmetric(vertical: 24),
            child: Center(
              child: SizedBox(
                  width: 20,
                  height: 20,
                  child: CircularProgressIndicator(strokeWidth: 2)),
            ),
          )
        else
          for (final f in found)
            _FolderRow(
              folder: f,
              on: _picked.contains(f.path),
              onChanged: (v) => setState(
                  () => v ? _picked.add(f.path) : _picked.remove(f.path)),
            ),
        if (found != null && found.isNotEmpty)
          _note(context,
              'A folder can feed more than one section — the chips say which '
              'found something. Add more, and change what each one holds, from '
              'Settings › Libraries.'),
      ],
    );
  }

  Widget _keepingUp() => _Card(
        kicker: 'Card 7 · Keeping up',
        tint: Tokens.secTools,
        title: 'How often should it look again?',
        blurb: 'Tulipix watches your folders for changes as they happen. The '
            'sweep below is the backstop for what a watcher misses — a drive '
            'that was unplugged, files copied while the app was closed.',
        children: [
          _Rows([
            _Row(
              title: 'Full rescan',
              note: 'Reads every watched folder from the top',
              trailing: Seg(
                options: const ['manual', 'hourly', 'daily', 'weekly'],
                labels: const ['Never', 'Hourly', 'Daily', 'Weekly'],
                value: _a.cadence ?? 'daily',
                onPick: (v) => setState(() => _a.cadence = v),
              ),
            ),
            _Row(
              title: 'Wait for mains power',
              note: 'Holds long scans until the laptop is plugged in',
              trailing: _switch(_a.powerAware ?? true,
                  (v) => setState(() => _a.powerAware = v)),
            ),
            _Row(
              title: 'Skip these',
              note: 'One pattern a line, matched against the whole path',
              trailing: SizedBox(
                width: 240,
                child: FieldBox(
                  value: _a.exclusions ?? '',
                  hint: 'node_modules, .git, *.tmp',
                  onSubmit: (v) => setState(() => _a.exclusions = v),
                ),
              ),
            ),
          ]),
        ],
      );

  /// The eight service flags, exactly as Settings › Services & Keys spells
  /// them. All eight ship off; this card is the one place they are all in
  /// front of you at once.
  static const List<({String key, String label, String note})> _services = [
    (
      key: 'api.anilist',
      label: 'AniList',
      note: 'Anime titles, artwork and episode counts'
    ),
    (
      key: 'api.anidb',
      label: 'AniDB',
      note: 'Episode numbering that matches the discs'
    ),
    (
      key: 'api.addic7ed',
      label: 'Addic7ed',
      note: 'Subtitles, alongside OpenSubtitles'
    ),
    (
      key: 'api.discogs',
      label: 'Discogs',
      note: 'Release details and artist biographies'
    ),
    (
      key: 'api.spotify',
      label: 'Spotify',
      note: 'Artist images and related artists. Needs an app key'
    ),
    (
      key: 'api.youtube-data',
      label: 'YouTube',
      note: 'Search and durations without scraping. Needs a key'
    ),
    (
      key: 'api.trakt',
      label: 'Trakt',
      note: 'Sends what you watched to your Trakt history'
    ),
    (
      key: 'api.listenbrainz',
      label: 'ListenBrainz',
      note: 'Scrobbles what you play to your account'
    ),
  ];

  Widget _online() => _Card(
        kicker: 'Card 8 · Online',
        tint: Tokens.warn,
        title: 'Nothing goes online unless you switch it on.',
        blurb: 'Cover art, track listings, film details and subtitles come '
            'from services that need a request per item. Each is a separate '
            'switch, and every key you add is kept in your system keychain — '
            'never in a settings file.',
        children: [
          _Rows([
            for (final s in _services)
              _Row(
                title: s.label,
                note: s.note,
                trailing: _switch(_a.services[s.key] ?? false,
                    (v) => setState(() => _a.services[s.key] = v)),
              ),
          ]),
          _note(context,
              'The ones that need a key say so, and the key goes in Settings › '
              'Services & Keys. A switch on without a key simply finds nothing.'),
        ],
      );

  Widget _lock() => _Card(
        kicker: 'Card 9 · Locking up',
        tint: Tokens.secTransfer,
        title: 'Should Tulipix lock itself?',
        blurb: 'The lock screen covers the window, not the files — someone '
            'with your disk still has your disk. Encrypting the index itself '
            'is Settings › Security, and needs a build with SQLCipher in it.',
        children: [
          _Rows([
            _Row(
              title: 'A PIN',
              note: 'Four to eight digits. Salted and hashed, never stored as '
                  'typed — leave it empty for no lock',
              trailing: SizedBox(
                width: 150,
                child: FieldBox(
                  value: _a.pin ?? '',
                  hint: '••••',
                  secret: true,
                  onSubmit: (v) => setState(
                      () => _a.pin = v.trim().isEmpty ? null : v.trim()),
                ),
              ),
            ),
            _Row(
              title: 'Lock when idle',
              note: 'Playback keeps it awake — a film does not count as idling',
              trailing: Seg(
                options: const ['5 minutes', '15 minutes', '1 hour'],
                labels: const ['5 min', '15 min', '1 hour'],
                value: _a.lockAfter ?? '',
                onPick: (v) => setState(() => _a.lockAfter = v),
              ),
            ),
          ]),
          _note(context,
              'A fingerprint or a security key can be added later from '
              'Settings › Security — both need the reader present to enrol, '
              'which is not a thing to ask for on a first run.'),
        ],
      );

  static const List<({Color tint, String title, String note})> _feats = [
    (
      tint: Tokens.secTools,
      title: 'Tools',
      note: 'Convert, trim, upscale and clean up files without leaving the app.'
    ),
    (
      tint: Tokens.secTransfer,
      title: 'Transfer',
      note: 'Scan a QR code and your phone can push files straight in.'
    ),
    (
      tint: Tokens.secPhotos,
      title: 'Genesis',
      note: 'Find something new by what it is like, not what it is called.'
    ),
    (
      tint: Tokens.secCloud,
      title: 'Cloud',
      note: 'Your own storage, mounted beside the local folders.'
    ),
    (
      tint: Tokens.secFinances,
      title: 'Finances',
      note: 'Where the subscriptions went. Nothing else ever reads it.'
    ),
    (
      tint: Tokens.brand,
      title: 'Ask it things',
      note: 'Point an AI assistant at your library, read-only, over MCP.'
    ),
  ];

  Widget _ready() {
    final services = _a.services.values.where((v) => v).length;
    return _Card(
      kicker: 'Card 10 · Ready',
      tint: Tokens.ok,
      title: "That's it. Here's what people try first.",
      blurb: 'Scanning starts as soon as you close this and runs in the '
          'background — the sections fill in as it goes, so there is nothing '
          'to wait for.',
      children: [
        LayoutBuilder(
          builder: (context, c) => Wrap(
            spacing: 10,
            runSpacing: 10,
            children: [
              for (final f in _feats)
                SizedBox(
                  width: (c.maxWidth - 20) / 3,
                  child: _Feature(tint: f.tint, title: f.title, note: f.note),
                ),
            ],
          ),
        ),
        _Rows([
          _summary('Folders to read', _skipped[5] || _picked.isEmpty,
              '${_picked.length} of ${_found?.length ?? 0}'),
          _summary('Theme', _a.theme == null, _themeLabel(_a.theme)),
          _summary('Material', _a.designLanguage == null,
              DesignLanguage.fromId(_a.designLanguage).name),
          _summary('Online lookups', services == 0, '$services on'),
          _summary('Lock', _a.pin == null, 'a PIN'),
        ]),
      ],
    );
  }

  /// A summary line. `skipped` is not "off" — it is "nobody said", and the
  /// difference is the whole reason a skipped card writes nothing.
  _Row _summary(String title, bool skipped, String value) => _Row(
        title: title,
        note: '',
        trailing: StateChip(skipped ? 'skipped' : value,
            tint: skipped ? null : Tokens.ok),
      );

  static String _themeLabel(String? id) => switch (id) {
        'light' => 'Light',
        'dark' => 'Dark',
        'extra-dark' => 'OLED black',
        _ => 'Follow system',
      };

  Widget _switch(bool on, ValueChanged<bool> onChanged) =>
      SettingSwitch(on: on, onChanged: _c.committing ? null : onChanged);
}

// ── pieces ──────────────────────────────────────────────────────────────────

Widget _note(BuildContext context, String text) => Text(text,
    style: TextStyle(
        fontSize: 11.5, height: 1.5, color: context.tokens.textDim));

class _Scrim extends StatelessWidget {
  const _Scrim();

  @override
  Widget build(BuildContext context) => const DecoratedBox(
        decoration: BoxDecoration(
          gradient: RadialGradient(
            center: Alignment(0, -1),
            radius: 1.2,
            colors: [Color(0x2E7C3AED), Color(0xB806070C)],
          ),
        ),
        child: SizedBox.expand(),
      );
}

/// The lede plus whatever the card holds. Every card is this shape, so the
/// heading sits in the same place on all ten.
class _Card extends StatelessWidget {
  const _Card({
    required this.kicker,
    required this.tint,
    required this.title,
    required this.blurb,
    required this.children,
  });

  final String kicker;
  final Color tint;
  final String title;
  final String blurb;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(kicker.toUpperCase(),
            style: TextStyle(
                fontSize: 10.5,
                fontWeight: FontWeight.w700,
                letterSpacing: 1.1,
                color: tint)),
        const SizedBox(height: 9),
        Text(title,
            style: TextStyle(
                fontSize: 24,
                height: 1.18,
                fontWeight: FontWeight.w800,
                letterSpacing: -0.4,
                color: t.text)),
        const SizedBox(height: 9),
        ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 560),
          child: Text(blurb,
              style:
                  TextStyle(fontSize: 13, height: 1.55, color: t.textDim)),
        ),
        for (final child in children) ...[
          const SizedBox(height: 18),
          child,
        ],
      ],
    );
  }
}

enum _PipState { todo, now, done, skipped }

class _Pip extends StatelessWidget {
  const _Pip({required this.state, required this.onTap, required this.label});

  final _PipState state;
  final VoidCallback onTap;
  final String label;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Tooltip(
      message: label,
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(3),
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: 6),
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 260),
            height: 4,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(2),
              color: switch (state) {
                _PipState.now => Tokens.brand,
                _PipState.done => Tokens.brand.withValues(alpha: 0.55),
                // Dimmer than untouched, so a skipped step reads as passed
                // over rather than as still to come.
                _PipState.skipped => t.outlineStrong,
                _PipState.todo => t.outline,
              },
              boxShadow: state == _PipState.now
                  ? [
                      BoxShadow(
                          color: Tokens.brand.withValues(alpha: 0.35),
                          blurRadius: 8)
                    ]
                  : null,
            ),
          ),
        ),
      ),
    );
  }
}

/// A bordered stack of label / note / control rows — the same object the
/// settings tiles use, without the `SettingItem` plumbing.
class _Rows extends StatelessWidget {
  const _Rows(this.rows);

  final List<_Row> rows;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return DecoratedBox(
      decoration: BoxDecoration(
        color: t.panel2,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: t.outline),
      ),
      child: Column(
        children: [
          for (var i = 0; i < rows.length; i++)
            DecoratedBox(
              decoration: BoxDecoration(
                border: i == 0
                    ? null
                    : Border(top: BorderSide(color: t.outline)),
              ),
              child: rows[i],
            ),
        ],
      ),
    );
  }
}

class _Row extends StatelessWidget {
  const _Row({required this.title, required this.note, required this.trailing});

  final String title;
  final String note;
  final Widget trailing;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.fromLTRB(14, 11, 12, 11),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(title,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w600,
                        color: t.text)),
                if (note.isNotEmpty)
                  Text(note,
                      style: TextStyle(
                          fontSize: 11.5, height: 1.4, color: t.textDim)),
              ],
            ),
          ),
          const SizedBox(width: 14),
          trailing,
        ],
      ),
    );
  }
}

typedef _PickData = ({String id, String title, String note, Widget? swatch});

class _PickGrid extends StatelessWidget {
  const _PickGrid({
    required this.columns,
    required this.picks,
    required this.value,
    required this.tint,
    required this.onPick,
  });

  final int columns;
  final List<_PickData> picks;

  /// Null is "not answered". No pick is drawn as chosen until one is, which is
  /// what makes skipping this card mean something.
  final String? value;
  final Color tint;
  final ValueChanged<String> onPick;

  @override
  Widget build(BuildContext context) => LayoutBuilder(
        builder: (context, c) {
          final w = (c.maxWidth - 10 * (columns - 1)) / columns;
          return Wrap(
            spacing: 10,
            runSpacing: 10,
            children: [
              for (final p in picks)
                SizedBox(
                  width: w,
                  child: _Pick(
                    data: p,
                    on: p.id == value,
                    tint: tint,
                    onTap: () => onPick(p.id),
                  ),
                ),
            ],
          );
        },
      );
}

class _Pick extends StatelessWidget {
  const _Pick({
    required this.data,
    required this.on,
    required this.tint,
    required this.onTap,
  });

  final _PickData data;
  final bool on;
  final Color tint;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: on ? Color.alphaBlend(tint.withValues(alpha: 0.12), t.panel2) : t.panel2,
      borderRadius: BorderRadius.circular(14),
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(14),
        child: Container(
          padding: const EdgeInsets.all(12),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
                color: on ? tint : t.outline, width: on ? 1.5 : 1),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              if (data.swatch != null) ...[
                data.swatch!,
                const SizedBox(height: 9),
              ],
              Text(data.title,
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      color: t.text)),
              const SizedBox(height: 3),
              Text(data.note,
                  style: TextStyle(
                      fontSize: 11, height: 1.45, color: t.textDim)),
            ],
          ),
        ),
      ),
    );
  }
}

class _AvatarBubble extends StatelessWidget {
  const _AvatarBubble({required this.emoji, required this.png});

  final String emoji;
  final Uint8List? png;

  @override
  Widget build(BuildContext context) {
    final bytes = png;
    return Container(
      width: 76,
      height: 76,
      clipBehavior: Clip.antiAlias,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(24),
        gradient: const LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [Tokens.brand, Tokens.brand2, Color(0xFF0EA5E9)],
        ),
      ),
      child: bytes != null && bytes.isNotEmpty
          ? Image.memory(bytes, fit: BoxFit.cover)
          : Center(child: Text(emoji, style: const TextStyle(fontSize: 34))),
    );
  }
}

/// One suggestion: what it is, what was found under it, and whether to read it.
class _FolderRow extends StatelessWidget {
  const _FolderRow({
    required this.folder,
    required this.on,
    required this.onChanged,
  });

  final SuggestedFolder folder;
  final bool on;
  final ValueChanged<bool> onChanged;

  static Color _tint(String section) => switch (section) {
        'photos' => Tokens.secPhotos,
        'videos' => Tokens.secVideos,
        'music' => Tokens.secMusic,
        _ => Tokens.secBooks,
      };

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final lead = folder.sections.isEmpty ? 'books' : folder.sections.first;
    return Padding(
      padding: const EdgeInsets.only(bottom: 9),
      child: Material(
        color: t.panel2,
        borderRadius: BorderRadius.circular(14),
        child: InkWell(
          onTap: () => onChanged(!on),
          borderRadius: BorderRadius.circular(14),
          child: Container(
            padding: const EdgeInsets.fromLTRB(12, 11, 12, 11),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(14),
              border: Border.all(
                  color: on ? _tint(lead) : t.outline, width: on ? 1.5 : 1),
            ),
            child: Row(
              children: [
                Icon(Icons.folder_outlined, size: 20, color: _tint(lead)),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(folder.label,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontSize: 12.5,
                              fontFamily: 'monospace',
                              fontWeight: FontWeight.w600,
                              color: t.text)),
                      Text(
                          '${folder.items}${folder.capped ? '+' : ''} '
                          'file${folder.items == 1 ? '' : 's'} · '
                          '${folder.sections.join(', ')}',
                          style:
                              TextStyle(fontSize: 11.5, color: t.textDim)),
                    ],
                  ),
                ),
                const SizedBox(width: 12),
                SettingSwitch(on: on, onChanged: onChanged),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _Feature extends StatelessWidget {
  const _Feature({required this.tint, required this.title, required this.note});

  final Color tint;
  final String title;
  final String note;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: t.panel2,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: t.outline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 8,
            height: 8,
            decoration: BoxDecoration(
                color: tint, borderRadius: BorderRadius.circular(3)),
          ),
          const SizedBox(height: 9),
          Text(title,
              style: TextStyle(
                  fontSize: 12.5, fontWeight: FontWeight.w700, color: t.text)),
          const SizedBox(height: 3),
          Text(note,
              style:
                  TextStyle(fontSize: 11, height: 1.45, color: t.textDim)),
        ],
      ),
    );
  }
}
