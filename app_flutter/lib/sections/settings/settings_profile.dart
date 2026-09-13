// You & Home — the first Settings tab, laid out as a control center
// (docs/mockups/control-center-redesign.html).
//
// Top to bottom: the profile (a cropped cover and photo, the name, the version
// and app health), one grid of same-shaped tiles for how the app looks, Home's
// layouts, and the credits, which are exactly as they were.
//
// Everything on the page is staged. Nothing reaches disk until Save changes
// (or Ctrl+S), and Discard puts it all back. The page used to be half and
// half: theme, motion, the layout and the cards wrote on click while the name,
// the logo and the design language waited for a Save inside one card, so it
// was never clear what had been kept. The sidebar shows only what is saved.

import 'dart:io';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/app_mark.dart';
import '../../design/design_language.dart';
import '../../design/tokens.dart';
import '../../design/skin.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/settings.dart';
import '../music/mini_widget.dart';
import '../music/music_controller.dart';
import 'profile_cropper.dart';
import 'settings_controller.dart';

/// The four layouts, in the order they matter in — the default first, which is
/// the order `cage` places them in on the Slint page.
///
/// The display names are Slint's. The ids are NOT: `home.layout` and every
/// `home.hidden.<layout>` key on disk hold `welcome` / `classic` / `stream` /
/// `cinema`, and renaming them would silently reset everyone's card switches.
const List<({String id, String name, String blurb})> kHomeLayoutTiles = [
  (
    id: 'welcome',
    name: 'Focused',
    blurb: 'The default. A big hello, the six things you launch by name, '
        'player at the foot.',
  ),
  (
    id: 'classic',
    name: 'Poweruser',
    blurb: 'Everything at once — cards in rows with the music rail.',
  ),
  (
    id: 'stream',
    name: 'Timeline',
    blurb: 'A feed of what happened, plus a standing rail.',
  ),
  (
    id: 'cinema',
    name: 'Cinema',
    blurb: 'A full-bleed hero of what you were watching, rails on a shelf.',
  ),
];

String _layoutName(String id) => kHomeLayoutTiles
    .firstWhere((l) => l.id == id, orElse: () => kHomeLayoutTiles[1])
    .name;

const List<({String id, String label, Color a, Color b})> _themes = [
  (id: 'system', label: 'System', a: Color(0xFF64748B), b: Color(0xFFE2E8F0)),
  (id: 'light', label: 'Light', a: Color(0xFFF8FAFC), b: Color(0xFFCBD5E1)),
  (id: 'dark', label: 'Dark', a: Color(0xFF1E293B), b: Color(0xFF0F172A)),
  (
    id: 'extra-dark',
    label: 'Extra dark',
    a: Color(0xFF0B0B0F),
    b: Color(0xFF000000)
  ),
];

const List<String> _logoLabels = ['Default', 'Colour', 'Dark', 'White', 'India'];

/// Slot 0 drawn as the plain mark. `appLogoAsset(0)` is the seasonal swap
/// inside a festival window, and two India tiles in one row read as a bug.
const String _plainMark = 'assets/appicons/sidebar-default.png';

class ProfileTab extends StatefulWidget {
  const ProfileTab({
    super.key,
    required this.controller,
    required this.state,
    this.onTab,
  });

  final SettingsController controller;
  final SettingsState state;

  /// Where the status pill goes: the Status TAB in settings, not the loopback
  /// dashboard the sidebar key opens.
  final ValueChanged<String>? onTab;

  @override
  State<ProfileTab> createState() => _ProfileTabState();
}

class _ProfileTabState extends State<ProfileTab> {
  late final TextEditingController _name =
      TextEditingController(text: widget.state.displayName);

  // What is staged. Null means untouched — the saved value shows through — so
  // Discard is clearing these, and a refresh underneath never fights them.
  String? _emoji;

  /// Null untouched, empty remove, otherwise a PNG the cropper rendered.
  Uint8List? _avatar;
  Uint8List? _cover;
  int? _logo;

  /// The plain default mark, picked ON PURPOSE. Inside a festival window slot
  /// 0 previews as the seasonal swap; this makes the preview show the plain
  /// mark instead. Session-only, as in Slint.
  bool _defaultPicked = false;
  DesignLanguage? _language;
  String? _theme;
  bool? _motion;
  MiniStyle? _mini;
  String? _layout;
  final Map<String, bool> _cards = {};
  bool? _musicLeft;

  bool _saving = false;
  bool _justSaved = false;

  SettingsState get _st => widget.state;
  ShellController get _shell => ShellController.instance;

  String get _emojiNow => _emoji ?? _st.avatarEmoji;
  int get _logoNow => _logo ?? _st.logoChoice;
  DesignLanguage get _languageNow => _language ?? _shell.designLanguage;
  String get _themeNow {
    final th = _theme ?? _st.theme;
    return _themes.any((x) => x.id == th) ? th : 'system';
  }

  bool get _motionNow => _motion ?? _st.reduceMotion;
  MiniStyle get _miniNow => _mini ?? MusicController.instance.widgetStyle;
  String get _layoutNow => _layout ?? _st.homeLayout;
  bool _cardOn(HomeCardRow c) => _cards[c.key] ?? c.on_;
  bool get _musicLeftNow => _musicLeft ?? _st.homeMusicLeft;

  /// What Save would write, by name — the save bar lists these.
  List<String> get _changes {
    final st = _st;
    return [
      if (_name.text.trim() != st.displayName.trim()) 'Name',
      if (_avatar != null || (_emoji != null && _emoji != st.avatarEmoji))
        'Profile photo',
      if (_cover != null) 'Cover',
      if (_theme != null && _theme != st.theme) 'Theme',
      if (_motion != null && _motion != st.reduceMotion) 'Reduce motion',
      if (_language != null && _language != _shell.designLanguage)
        'Design language',
      if (_logo != null && _logo != st.logoChoice) 'Sidebar logo',
      if (_mini != null && _mini != MusicController.instance.widgetStyle)
        'Mini player',
      if (_layout != null && _layout != st.homeLayout) 'Home layout',
      if (st.homeCards.any((c) => _cards[c.key] != null && _cards[c.key] != c.on_) ||
          (_musicLeft != null && _musicLeft != st.homeMusicLeft))
        'Home cards',
    ];
  }

  @override
  void initState() {
    super.initState();
    // A TextEditingController notifies its own listeners without rebuilding
    // the widget holding it, so without this the save bar never heard about
    // a new name.
    _name.addListener(_reread);
  }

  void _reread() {
    if (mounted) setState(() {});
  }

  @override
  void dispose() {
    _name.removeListener(_reread);
    _name.dispose();
    super.dispose();
  }

  /// Tell the rail. After the frame: the rail is a sibling mid-build.
  void _publishDirty(bool dirty) {
    final flag = widget.controller.profileDirty;
    if (flag.value == dirty) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) flag.value = dirty;
    });
  }

  Future<void> _save() async {
    if (_saving || _changes.isEmpty) return;
    setState(() => _saving = true);
    final c = widget.controller;
    final st = _st;
    final savedLanguage = _shell.designLanguage;
    final savedMini = MusicController.instance.widgetStyle;
    try {
      await c.send(SettingsCmd.saveProfile(
        name: _name.text.trim(),
        emoji: _emojiNow,
        logo: _logoNow,
        avatar: _avatar,
        cover: _cover,
      ));
      // Staged edits stay staged on a failure, so nothing typed is lost.
      if (c.error != null) return;
      final language = _language;
      if (language != null && language != savedLanguage) {
        await c.send(SettingsCmd.setText(
            key: 'ui.design-language', value: language.id));
      }
      final theme = _theme;
      if (theme != null && theme != st.theme) {
        await c.send(SettingsCmd.setTheme(theme: theme));
      }
      final motion = _motion;
      if (motion != null && motion != st.reduceMotion) {
        await c.send(SettingsCmd.setReduceMotion(on_: motion));
      }
      final layout = _layout;
      if (layout != null && layout != st.homeLayout) {
        await c.send(SettingsCmd.setHomeLayout(layout: layout));
      }
      for (final card in st.homeCards) {
        final on = _cards[card.key];
        if (on != null && on != card.on_) {
          await c.send(SettingsCmd.homeCardSet(key: card.key, on_: on));
        }
      }
      final left = _musicLeft;
      if (left != null && left != st.homeMusicLeft) {
        await c.send(SettingsCmd.toggle(key: 'home.music-left', on_: left));
      }
      final mini = _mini;
      if (mini != null && mini != savedMini) {
        MusicController.instance.setWidgetStyle(mini);
        // The Slint build's own widget reads the same key and names.
        await c.send(SettingsCmd.setText(
            key: 'ui.mini-widget.style', value: mini.name));
      }
      if (_avatar != null || _cover != null) {
        // Same file names, new pictures: drop the cached ones — live ones
        // included — and bump the epoch so every Image resolves again.
        final now = c.state;
        for (final p in {
          st.avatarPath,
          st.coverPath,
          now?.avatarPath ?? '',
          now?.coverPath ?? '',
        }) {
          if (p.isNotEmpty) await FileImage(File(p)).evict();
        }
        _shell.pictureEpoch++;
      }
      // The sidebar prints the name, photo and mark, and the shell snapshot
      // is where the app reads its theme and design language from.
      await _shell.refresh();
    } finally {
      if (mounted) setState(() => _saving = false);
    }
    if (!mounted || c.error != null) return;
    _clearStaged();
    setState(() => _justSaved = true);
    await Future<void>.delayed(const Duration(seconds: 2));
    if (mounted) setState(() => _justSaved = false);
  }

  void _clearStaged() {
    _emoji = null;
    _avatar = null;
    _cover = null;
    _logo = null;
    _defaultPicked = false;
    _language = null;
    _theme = null;
    _motion = null;
    _mini = null;
    _layout = null;
    _cards.clear();
    _musicLeft = null;
  }

  void _discard() => setState(() {
        _name.text = _st.displayName;
        _clearStaged();
      });

  static Future<Uint8List?> _read(String path) async {
    try {
      return await File(path).readAsBytes();
    } catch (_) {
      return null;
    }
  }

  /// What the cropper starts from: the staged crop, else the saved file.
  Future<Uint8List?> _current(Uint8List? staged, String saved) async {
    if (staged != null) return staged.isEmpty ? null : staged;
    return saved.isEmpty ? null : _read(saved);
  }

  Future<void> _editAvatar() async {
    final st = _st;
    final source = await _current(_avatar, st.avatarPath);
    if (!mounted) return;
    final r = await showCropper(
      context,
      kind: CropKind.avatar,
      source: source,
      emoji: _emojiNow,
      hasPicture: source != null,
    );
    if (r == null || !mounted) return;
    setState(() {
      final png = r.png;
      if (png != null) {
        _avatar = png;
      } else if (r.remove) {
        _avatar = st.avatarPath.isEmpty ? null : Uint8List(0);
      }
      if (r.emoji != null) _emoji = r.emoji;
    });
  }

  Future<void> _editCover() async {
    final st = _st;
    final source = await _current(_cover, st.coverPath);
    if (!mounted) return;
    final r = await showCropper(
      context,
      kind: CropKind.cover,
      source: source,
      hasPicture: source != null,
    );
    if (r == null || !mounted) return;
    setState(() {
      final png = r.png;
      if (png != null) {
        _cover = png;
      } else if (r.remove) {
        _cover = st.coverPath.isEmpty ? null : Uint8List(0);
      }
    });
  }

  Future<void> _copyUrl() async {
    await Clipboard.setData(const ClipboardData(text: 'https://tulipix.pro'));
    if (mounted) {
      ScaffoldMessenger.maybeOf(context)?.showSnackBar(
        const SnackBar(content: Text('tulipix.pro copied')),
      );
    }
  }

  ImageProvider? get _avatarImage {
    final a = _avatar;
    if (a != null) return a.isEmpty ? null : MemoryImage(a);
    return _st.avatarPath.isEmpty ? null : FileImage(File(_st.avatarPath));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final changes = _changes;
    _publishDirty(changes.isNotEmpty);
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.keyS, control: true): () {
          _save();
        },
      },
      child: Focus(
        autofocus: true,
        child: Column(
          children: [
            Expanded(
              child: LayoutBuilder(
                builder: (context, box) {
                  final gutter = math.max(16.0, box.maxWidth * 0.02);
                  final width = box.maxWidth - gutter * 2;
                  return ListView(
                    padding: EdgeInsets.fromLTRB(gutter, 18, gutter, 24),
                    children: [
                      Text('You & Home',
                          style: TextStyle(
                              fontSize: 22,
                              fontWeight: FontWeight.w800,
                              color: t.text)),
                      const SizedBox(height: 2),
                      Text(
                          'Your profile, how Tulipix looks, and what Home '
                          'shows',
                          style: TextStyle(fontSize: 12.5, color: t.textDim)),
                      const SizedBox(height: 14),
                      _hero(t),
                      const SizedBox(height: 16),
                      _tiles(width),
                      const SizedBox(height: 16),
                      _layoutTile(width >= 900),
                      const SizedBox(height: 22),
                      const _Credits(),
                    ],
                  );
                },
              ),
            ),
            _saveBar(t, changes),
          ],
        ),
      ),
    );
  }

  // ── profile ───────────────────────────────────────────────────────────────

  Widget _hero(Tokens t) => Container(
        decoration:
            context.skin.surface(SurfaceRole.card, radius: 20) ??
                BoxDecoration(
                  color: t.panel2,
                  borderRadius: BorderRadius.circular(20),
                  border: Border.all(color: t.outline),
                ),
        clipBehavior: Clip.antiAlias,
        child: Column(
          children: [
            // 4:1, the cropper's mask, clamped so a very narrow or very wide
            // window still gets a sane band.
            LayoutBuilder(
              builder: (context, box) => SizedBox(
                height: (box.maxWidth / 4).clamp(120.0, 220.0),
                child: Stack(
                  fit: StackFit.expand,
                  children: [
                    _coverFace(),
                    Positioned(
                      right: 12,
                      top: 12,
                      child: _GlassButton(
                        icon: Icons.image_outlined,
                        label: 'Change cover',
                        onTap: _editCover,
                      ),
                    ),
                  ],
                ),
              ),
            ),
            Padding(
              padding: const EdgeInsets.fromLTRB(20, 0, 20, 18),
              child: Row(
                children: [
                  // Only the photo rides up over the seam, so the row keeps a
                  // short box and the circle hangs out of the top of it.
                  SizedBox(
                    width: 104,
                    height: 56,
                    child: OverflowBox(
                      maxHeight: 104,
                      alignment: Alignment.topCenter,
                      child: Transform.translate(
                        offset: const Offset(0, -48),
                        child: _avatarButton(t),
                      ),
                    ),
                  ),
                  const SizedBox(width: 18),
                  _nameField(t),
                  const SizedBox(width: 14),
                  _VersionChip(
                    label: 'Tulipix v${_st.appVersion} · tulipix.pro',
                    onTap: _copyUrl,
                  ),
                  const Spacer(),
                  Flexible(
                    child:
                        _StatusPill(onTap: () => widget.onTab?.call('status')),
                  ),
                ],
              ),
            ),
          ],
        ),
      );

  Widget _coverFace() {
    final c = _cover;
    if (c != null && c.isNotEmpty) {
      return Image.memory(c, fit: BoxFit.cover, gaplessPlayback: true);
    }
    final saved = _st.coverPath;
    if (c == null && saved.isNotEmpty) {
      return Image.file(
        File(saved),
        key: ValueKey(_shell.pictureEpoch),
        fit: BoxFit.cover,
        errorBuilder: (context, error, stack) => const _CoverGradient(),
      );
    }
    return const _CoverGradient();
  }

  Widget _avatarButton(Tokens t) => Tooltip(
        message: 'Change profile photo',
        child: MouseRegion(
          cursor: SystemMouseCursors.click,
          child: GestureDetector(
            onTap: _editAvatar,
            child: SizedBox(
              width: 104,
              height: 104,
              child: Stack(
                clipBehavior: Clip.none,
                children: [
                  Container(
                    padding: const EdgeInsets.all(4),
                    decoration:
                        BoxDecoration(shape: BoxShape.circle, color: t.panel2),
                    child: _AvatarFace(
                        size: 96, image: _avatarImage, emoji: _emojiNow),
                  ),
                  Positioned(
                    right: 2,
                    bottom: 0,
                    child: Container(
                      width: 30,
                      height: 30,
                      decoration: BoxDecoration(
                        color: Tokens.brand,
                        shape: BoxShape.circle,
                        border: Border.all(color: t.panel2, width: 3),
                      ),
                      child: const Icon(Icons.photo_camera,
                          size: 14, color: Colors.white),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      );

  // Capped at 12 characters: the Focused layout sizes its greeting off a width
  // factor and wraps past that. Rust clamps again on save and on load.
  Widget _nameField(Tokens t) => SizedBox(
        width: 220,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: _name,
              maxLength: 12,
              style: TextStyle(
                  fontSize: 20, fontWeight: FontWeight.w800, color: t.text),
              decoration: InputDecoration(
                isDense: true,
                counterText: '',
                hintText: 'Your name',
                hintStyle: TextStyle(
                  fontSize: 20,
                  fontWeight: FontWeight.w800,
                  color: t.textDim.withValues(alpha: 0.45),
                ),
                enabledBorder: UnderlineInputBorder(
                    borderSide: BorderSide(color: t.outline, width: 2)),
                focusedBorder: const UnderlineInputBorder(
                    borderSide: BorderSide(color: Tokens.brand, width: 2)),
              ),
              onSubmitted: (_) => _save(),
            ),
            const SizedBox(height: 4),
            Row(
              children: [
                Expanded(
                  child: Text('Shown on Home and in the sidebar',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 11.5, color: t.textDim)),
                ),
                Text('${_name.text.characters.length}/12',
                    style: TextStyle(
                        fontSize: 11,
                        color: t.textDim,
                        fontFeatures: const [FontFeature.tabularFigures()])),
              ],
            ),
          ],
        ),
      );

  // ── the tiles ─────────────────────────────────────────────────────────────

  /// Three across on a wide page, two on a middling one, one on a narrow one.
  /// Every row is one height, set by its tallest tile.
  Widget _tiles(double width) {
    const gap = 14.0;
    Widget row(List<Widget> tiles) => IntrinsicHeight(
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (var i = 0; i < tiles.length; i++) ...[
                if (i > 0) const SizedBox(width: gap),
                Expanded(child: tiles[i]),
              ],
            ],
          ),
        );
    if (width >= 900) {
      // Home cards spans the last two columns, so the mini tile is sized to
      // exactly one of the three above it.
      final third = (width - gap * 2) / 3;
      return Column(
        children: [
          row([_themeTile(), _languageTile(), _logoTile()]),
          const SizedBox(height: gap),
          IntrinsicHeight(
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                SizedBox(width: third, child: _miniTile()),
                const SizedBox(width: gap),
                Expanded(child: _cardsTile()),
              ],
            ),
          ),
        ],
      );
    }
    if (width >= 600) {
      return Column(
        children: [
          row([_themeTile(), _languageTile()]),
          const SizedBox(height: gap),
          row([_logoTile(), _miniTile()]),
          const SizedBox(height: gap),
          _cardsTile(),
        ],
      );
    }
    return Column(
      children: [
        for (final tile in [
          _themeTile(),
          _languageTile(),
          _logoTile(),
          _miniTile(),
          _cardsTile(),
        ]) ...[
          tile,
          const SizedBox(height: gap),
        ],
      ],
    );
  }

  Widget _themeTile() => _Tile(
        icon: Icons.contrast,
        tint: const Color(0xFFEC4899),
        title: 'Theme & motion',
        note: 'Light, dark, and how much moves',
        child: Column(
          children: [
            for (var r = 0; r < 2; r++) ...[
              if (r > 0) const SizedBox(height: 8),
              Row(
                children: [
                  for (final th in _themes.sublist(r * 2, r * 2 + 2)) ...[
                    if (th != _themes[r * 2]) const SizedBox(width: 8),
                    Expanded(
                      child: _ThemeSwatch(
                        theme: th,
                        active: _themeNow == th.id,
                        onTap: () => setState(() => _theme = th.id),
                      ),
                    ),
                  ],
                ],
              ),
            ],
            const SizedBox(height: 10),
            _QuickToggle(
              title: 'Reduce motion',
              note: 'Drops the transitions that move things across the '
                  'screen',
              on: _motionNow,
              onChanged: (v) => setState(() => _motion = v),
            ),
          ],
        ),
      );

  Widget _languageTile() => _Tile(
        icon: Icons.layers_outlined,
        tint: Tokens.brand,
        title: 'Design language',
        note: 'The material Music is drawn in',
        child: Column(
          children: [
            for (final l in DesignLanguage.values) ...[
              if (l.index > 0) const SizedBox(height: 6),
              _LanguageRow(
                language: l,
                active: l == _languageNow,
                onTap: () => setState(() => _language = l),
              ),
            ],
          ],
        ),
      );

  Widget _logoTile() {
    final t = context.tokens;
    final l = _logoNow;
    return _Tile(
      icon: Icons.star_outline,
      tint: const Color(0xFF0EA5E9),
      title: 'Sidebar logo',
      note: 'The mark at the top of the sidebar',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
            decoration: BoxDecoration(
              color: t.bg,
              borderRadius: BorderRadius.circular(13),
              border: Border.all(color: t.outline),
            ),
            child: Row(
              children: [
                ClipRRect(
                  borderRadius: BorderRadius.circular(9),
                  child: Image.asset(
                    l == 0 && _defaultPicked ? _plainMark : appLogoAsset(l),
                    width: 28,
                    height: 28,
                    fit: BoxFit.cover,
                  ),
                ),
                const SizedBox(width: 10),
                Text('Tulipix',
                    style: TextStyle(
                        fontSize: 14,
                        fontWeight: FontWeight.w800,
                        color: t.text)),
                const Spacer(),
                Text('preview',
                    style: TextStyle(fontSize: 10.5, color: t.textDim)),
              ],
            ),
          ),
          const SizedBox(height: 10),
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [
              for (var i = 0; i < 5; i++)
                _Pick(
                  active: l == i,
                  radius: 10,
                  padding: const EdgeInsets.fromLTRB(5, 5, 10, 5),
                  onTap: () => setState(() {
                    _logo = i;
                    _defaultPicked = i == 0;
                  }),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      ClipRRect(
                        borderRadius: BorderRadius.circular(6),
                        child: Image.asset(
                          i == 0 ? _plainMark : appLogoAsset(i),
                          width: 20,
                          height: 20,
                          fit: BoxFit.cover,
                        ),
                      ),
                      const SizedBox(width: 7),
                      Text(_logoLabels[i],
                          style: TextStyle(
                              fontSize: 12,
                              fontWeight: FontWeight.w600,
                              color: l == i ? t.text : t.textDim)),
                    ],
                  ),
                ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _miniTile() {
    final t = context.tokens;
    return _Tile(
      icon: Icons.picture_in_picture_alt,
      tint: const Color(0xFF8B5CF6),
      title: 'Mini player widget',
      note: 'The window’s pocket-sized form',
      child: Column(
        children: [
          // Three, because `MiniStyle` has three, and the sizes are the
          // styles' own: `base` is what the window is actually resized to.
          for (final s in MiniStyle.values) ...[
            if (s.index > 0) const SizedBox(height: 7),
            _Pick(
              active: _miniNow == s,
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
              onTap: () => setState(() => _mini = s),
              child: Row(
                children: [
                  SizedBox(
                    width: 46,
                    height: 28,
                    child: Center(child: _MiniShape(size: s.base)),
                  ),
                  const SizedBox(width: 12),
                  Text(s.label,
                      style: TextStyle(
                          fontSize: 12.5,
                          fontWeight: FontWeight.w700,
                          color: t.text)),
                  const Spacer(),
                  Text(
                      '${s.base.width.round()} × ${s.base.height.round()}',
                      style: TextStyle(
                          fontSize: 11,
                          color: t.textDim,
                          fontFeatures: const [FontFeature.tabularFigures()])),
                ],
              ),
            ),
          ],
        ],
      ),
    );
  }

  Widget _cardsTile() {
    final t = context.tokens;
    final cards = _st.homeCards;
    final on = cards.where(_cardOn).length;
    final items = <Widget>[
      for (final c in cards)
        _CardToggle(
          label: c.label,
          on: _cardOn(c),
          onTap: () => setState(() => _cards[c.key] = !_cardOn(c)),
        ),
      // Only Poweruser has a rail to swap.
      if (_layoutNow == 'classic')
        _CardToggle(
          label: 'Music rail on the left',
          on: _musicLeftNow,
          onTap: () => setState(() => _musicLeft = !_musicLeftNow),
        ),
    ];
    const cols = 3;
    return _Tile(
      icon: Icons.grid_view,
      tint: const Color(0xFF34D399),
      title: 'Home cards',
      note: 'What Home shows, in every layout',
      trailing: [
        Text('$on of ${cards.length}',
            style: TextStyle(
                fontSize: 11,
                color: t.textDim,
                fontFeatures: const [FontFeature.tabularFigures()])),
        const SizedBox(width: 4),
        TextButton(
          style: TextButton.styleFrom(visualDensity: VisualDensity.compact),
          onPressed: () => setState(() {
            for (final c in cards) {
              _cards[c.key] = true;
            }
          }),
          child: const Text('Reset', style: TextStyle(fontSize: 12)),
        ),
      ],
      child: Column(
        children: [
          for (var i = 0; i < items.length; i += cols) ...[
            if (i > 0) const SizedBox(height: 7),
            Row(
              children: [
                for (var j = i; j < i + cols; j++) ...[
                  if (j > i) const SizedBox(width: 7),
                  Expanded(
                      child: j < items.length
                          ? items[j]
                          : const SizedBox.shrink()),
                ],
              ],
            ),
          ],
        ],
      ),
    );
  }

  Widget _layoutTile(bool wide) {
    final t = context.tokens;
    final on = {
      for (final c in _st.homeCards)
        if (_cardOn(c)) c.key
    };
    final cards = [
      for (final l in kHomeLayoutTiles)
        _LayoutCard(
          layout: l,
          active: _layoutNow == l.id,
          cards: on,
          onTap: () => setState(() => _layout = l.id),
        ),
    ];
    const gap = 12.0;
    Widget row(List<Widget> xs) => IntrinsicHeight(
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              for (var i = 0; i < xs.length; i++) ...[
                if (i > 0) const SizedBox(width: gap),
                Expanded(child: xs[i]),
              ],
            ],
          ),
        );
    return _Tile(
      icon: Icons.home_outlined,
      tint: const Color(0xFFF97316),
      title: 'Home layout',
      note: 'All four draw the same library; each arranges it differently',
      trailing: [
        Text('In use · ${_layoutName(_st.homeLayout)}',
            style: TextStyle(fontSize: 11, color: t.textDim)),
      ],
      child: wide
          ? row(cards)
          : Column(
              children: [
                row(cards.sublist(0, 2)),
                const SizedBox(height: gap),
                row(cards.sublist(2)),
              ],
            ),
    );
  }

  // ── the save bar ──────────────────────────────────────────────────────────

  Widget _saveBar(Tokens t, List<String> changes) {
    final dirty = changes.isNotEmpty;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 11),
      decoration: BoxDecoration(
        color: t.panel2,
        border: Border(top: BorderSide(color: t.outline)),
      ),
      child: Row(
        children: [
          Container(
            width: 9,
            height: 9,
            decoration: BoxDecoration(
              color: dirty ? Tokens.warn : Tokens.ok,
              shape: BoxShape.circle,
            ),
          ),
          const SizedBox(width: 12),
          Text(
            dirty
                ? '${changes.length} ${changes.length == 1 ? 'change' : 'changes'} not saved'
                : _justSaved
                    ? 'Saved'
                    : 'Everything saved',
            style: TextStyle(
                fontSize: 13, fontWeight: FontWeight.w700, color: t.text),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              dirty
                  ? changes.join(' · ')
                  : 'The sidebar and Home show what you see here',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(fontSize: 12, color: t.textDim),
            ),
          ),
          TextButton(
            onPressed: dirty && !_saving ? _discard : null,
            child: const Text('Discard'),
          ),
          const SizedBox(width: 8),
          FilledButton(
            onPressed: dirty && !_saving ? _save : null,
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (_saving) ...[
                  const SizedBox(
                    width: 13,
                    height: 13,
                    child: CircularProgressIndicator(
                        strokeWidth: 2, color: Colors.white),
                  ),
                  const SizedBox(width: 8),
                ],
                const Text('Save changes'),
                const SizedBox(width: 8),
                Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(5),
                    border: Border.all(color: Colors.white.withValues(alpha: 0.35)),
                  ),
                  child: const Text('Ctrl S',
                      style: TextStyle(fontSize: 10.5, letterSpacing: 0.3)),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

// ── pieces ──────────────────────────────────────────────────────────────────

/// One control tile: a tinted glyph, a title and a line under it, whatever
/// sits at the right of the header, then the controls. Every tile on the page
/// is this shape, which is what makes the grid read as one panel.
class _Tile extends StatelessWidget {
  const _Tile({
    required this.icon,
    required this.tint,
    required this.title,
    required this.note,
    required this.child,
    this.trailing = const [],
  });

  final IconData icon;
  final Color tint;
  final String title;
  final String note;
  final Widget child;
  final List<Widget> trailing;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: context.skin.surface(SurfaceRole.card, radius: 18) ??
          BoxDecoration(
            color: t.panel2,
            borderRadius: BorderRadius.circular(18),
            border: Border.all(color: t.outline),
          ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Container(
                width: 34,
                height: 34,
                decoration: context.skin
                        .control(active: true, tint: tint, radius: 11) ??
                    BoxDecoration(
                      color: tint.withValues(alpha: 0.16),
                      borderRadius: BorderRadius.circular(11),
                    ),
                child: Icon(context.skin.icon(icon), size: 17, color: tint),
              ),
              const SizedBox(width: 11),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(title,
                        style: TextStyle(
                            fontSize: 14,
                            fontWeight: FontWeight.w700,
                            color: t.text)),
                    Text(note,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11.5, color: t.textDim)),
                  ],
                ),
              ),
              ...trailing,
            ],
          ),
          const SizedBox(height: 14),
          child,
        ],
      ),
    );
  }
}

/// A choice among siblings. Latched in the brand colour with a soft ring when
/// picked; under a skin, the skin's own key.
class _Pick extends StatelessWidget {
  const _Pick({
    required this.active,
    required this.onTap,
    required this.child,
    this.padding = const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
    this.radius = 12,
  });

  final bool active;
  final VoidCallback onTap;
  final Widget child;
  final EdgeInsets padding;
  final double radius;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final r = BorderRadius.circular(radius);
    return Material(
      color: Colors.transparent,
      borderRadius: r,
      child: InkWell(
        borderRadius: r,
        onTap: onTap,
        child: Container(
          padding: padding,
          decoration: context.skin
                  .control(active: active, tint: Tokens.brand, radius: radius) ??
              BoxDecoration(
                color: active ? Tokens.brand.withValues(alpha: 0.10) : null,
                borderRadius: r,
                border: Border.all(
                  color: active ? Tokens.brand : t.outline,
                  width: active ? 1.5 : 1,
                ),
                boxShadow: active
                    ? [
                        BoxShadow(
                          color: Tokens.brand.withValues(alpha: 0.18),
                          spreadRadius: 3,
                        ),
                      ]
                    : null,
              ),
          child: child,
        ),
      ),
    );
  }
}

class _ThemeSwatch extends StatelessWidget {
  const _ThemeSwatch({
    required this.theme,
    required this.active,
    required this.onTap,
  });

  final ({String id, String label, Color a, Color b}) theme;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return _Pick(
      active: active,
      onTap: onTap,
      child: Row(
        children: [
          Container(
            width: 30,
            height: 20,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(6),
              border: Border.all(color: t.outline),
              gradient: LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: [theme.a, theme.a, theme.b, theme.b],
                stops: const [0, 0.5, 0.5, 1],
              ),
            ),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Text(theme.label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 12.5,
                    fontWeight: FontWeight.w600,
                    color: active ? t.text : t.textDim)),
          ),
        ],
      ),
    );
  }
}

class _QuickToggle extends StatelessWidget {
  const _QuickToggle({
    required this.title,
    required this.note,
    required this.on,
    required this.onChanged,
  });

  final String title;
  final String note;
  final bool on;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 8, 6, 8),
      decoration: BoxDecoration(
        color: t.bg,
        borderRadius: BorderRadius.circular(13),
        border: Border.all(color: t.outline),
      ),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(title,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        color: t.text)),
                Text(note,
                    style: TextStyle(fontSize: 11.5, color: t.textDim)),
              ],
            ),
          ),
          Switch(
            value: on,
            activeThumbColor: Tokens.brand,
            onChanged: onChanged,
          ),
        ],
      ),
    );
  }
}

/// One design language: a two-tone dot of its material, the style's name,
/// and this app's name for it.
class _LanguageRow extends StatelessWidget {
  const _LanguageRow({
    required this.language,
    required this.active,
    required this.onTap,
  });

  final DesignLanguage language;
  final bool active;
  final VoidCallback onTap;

  /// Each language's material in two stops, from its mockup.
  static const Map<DesignLanguage, (Color, Color)> _swatch = {
    DesignLanguage.standard: (Tokens.brand, Tokens.brand2),
    DesignLanguage.neumorphism: (Color(0xFFE4E6EE), Color(0xFFBCC0D1)),
    DesignLanguage.glassmorphism: (Color(0xFFF472B6), Color(0xFF8B5CF6)),
    DesignLanguage.expressive: (Color(0xFFFFD9E4), Color(0xFFA3175E)),
  };

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final (a, b) = _swatch[language]!;
    return Tooltip(
      message: language.blurb,
      waitDuration: const Duration(milliseconds: 500),
      child: _Pick(
        active: active,
        onTap: onTap,
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 7),
        child: Row(
          children: [
            Container(
              width: 20,
              height: 20,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                gradient: LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: [a, b],
                ),
                border: Border.all(color: t.outline),
              ),
            ),
            const SizedBox(width: 10),
            Text(language.label,
                style: TextStyle(
                    fontSize: 12.5,
                    fontWeight: FontWeight.w700,
                    color: active ? t.text : t.textDim)),
            const SizedBox(width: 8),
            Expanded(
              child: Text(language.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11, color: t.textDim)),
            ),
          ],
        ),
      ),
    );
  }
}

/// A style's outline, its own proportions, fitted into the glyph box.
class _MiniShape extends StatelessWidget {
  const _MiniShape({required this.size});

  final Size size;

  @override
  Widget build(BuildContext context) {
    final k = math.min(44 / size.width, 28 / size.height);
    return Container(
      width: size.width * k,
      height: size.height * k,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(4),
        gradient: const LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [Color(0xFFF472B6), Tokens.brand],
        ),
      ),
    );
  }
}

class _CardToggle extends StatelessWidget {
  const _CardToggle({
    required this.label,
    required this.on,
    required this.onTap,
  });

  final String label;
  final bool on;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final r = BorderRadius.circular(10);
    return Tooltip(
      message: label,
      waitDuration: const Duration(milliseconds: 700),
      child: Material(
        color: Colors.transparent,
        borderRadius: r,
        child: InkWell(
          borderRadius: r,
          onTap: onTap,
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
            decoration: context.skin
                    .control(active: on, tint: Tokens.brand, radius: 10) ??
                BoxDecoration(
                  borderRadius: r,
                  border: Border.all(
                    color: on
                        ? Tokens.brand.withValues(alpha: 0.45)
                        : t.outline,
                  ),
                ),
            child: Row(
              children: [
                Expanded(
                  child: Text(label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 12,
                          fontWeight: FontWeight.w600,
                          color: on ? t.text : t.textDim)),
                ),
                const SizedBox(width: 8),
                _MiniSwitch(on: on),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _MiniSwitch extends StatelessWidget {
  const _MiniSwitch({required this.on});

  final bool on;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    const d = Duration(milliseconds: 150);
    return AnimatedContainer(
      duration: d,
      width: 26,
      height: 15,
      padding: const EdgeInsets.all(2),
      decoration: BoxDecoration(
        color: on ? Tokens.brand : t.panel,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: on ? Tokens.brand : t.outline),
      ),
      child: AnimatedAlign(
        duration: d,
        alignment: on ? Alignment.centerRight : Alignment.centerLeft,
        child: Container(
          width: 9,
          height: 9,
          decoration: BoxDecoration(
            color: on ? Colors.white : t.textDim,
            shape: BoxShape.circle,
          ),
        ),
      ),
    );
  }
}

class _LayoutCard extends StatelessWidget {
  const _LayoutCard({
    required this.layout,
    required this.active,
    required this.cards,
    required this.onTap,
  });

  final ({String id, String name, String blurb}) layout;
  final bool active;
  final Set<String> cards;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return _Pick(
      active: active,
      onTap: onTap,
      radius: 14,
      padding: const EdgeInsets.all(10),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          AspectRatio(
            aspectRatio: 16 / 9,
            child: _LayoutThumb(kind: layout.id, cards: cards),
          ),
          const SizedBox(height: 9),
          Row(
            children: [
              Expanded(
                child: Text(layout.name,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        color: t.text)),
              ),
              Container(
                padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(6),
                  border: Border.all(
                    color: active
                        ? Tokens.brand.withValues(alpha: 0.5)
                        : t.outline,
                  ),
                ),
                child: Text(active ? 'IN USE' : 'USE',
                    style: TextStyle(
                        fontSize: 9.5,
                        fontWeight: FontWeight.w700,
                        letterSpacing: 0.5,
                        color: active ? Tokens.brand : t.textDim)),
              ),
            ],
          ),
          const SizedBox(height: 4),
          Text(layout.blurb,
              style: TextStyle(fontSize: 11.5, height: 1.4, color: t.textDim)),
        ],
      ),
    );
  }
}

class _AvatarFace extends StatelessWidget {
  const _AvatarFace({
    required this.size,
    required this.image,
    required this.emoji,
  });

  final double size;
  final ImageProvider? image;
  final String emoji;

  @override
  Widget build(BuildContext context) {
    final fallback = Container(
      width: size,
      height: size,
      alignment: Alignment.center,
      decoration: const BoxDecoration(
        shape: BoxShape.circle,
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [Color(0xFF312E81), Tokens.brand],
        ),
      ),
      child: emoji.trim().isEmpty
          ? Icon(Icons.person, size: size * 0.42, color: Colors.white)
          : Text(emoji, style: TextStyle(fontSize: size * 0.5)),
    );
    final img = image;
    if (img == null) return fallback;
    return ClipOval(
      child: Image(
        image: img,
        key: img is FileImage
            ? ValueKey(ShellController.instance.pictureEpoch)
            : null,
        width: size,
        height: size,
        fit: BoxFit.cover,
        gaplessPlayback: true,
        errorBuilder: (context, error, stack) => fallback,
      ),
    );
  }
}

/// The cover when there is no picture: the gradient and the two blobs the
/// Slint cover draws.
class _CoverGradient extends StatelessWidget {
  const _CoverGradient();

  @override
  Widget build(BuildContext context) => const Stack(
        fit: StackFit.expand,
        children: [
          DecoratedBox(
            decoration: BoxDecoration(
              gradient: LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: [Tokens.brand, Tokens.brand2, Color(0xFF0EA5E9)],
              ),
            ),
          ),
          Positioned(
            right: -60,
            top: -55,
            child: _Blob(size: 190, color: Color(0x2AFFFFFF)),
          ),
          Positioned(
            left: -40,
            top: 44,
            child: _Blob(size: 130, color: Color(0x22000000)),
          ),
        ],
      );
}

class _Blob extends StatelessWidget {
  const _Blob({required this.size, required this.color});

  final double size;
  final Color color;

  @override
  Widget build(BuildContext context) => Container(
        width: size,
        height: size,
        decoration: BoxDecoration(color: color, shape: BoxShape.circle),
      );
}

/// A button that sits on a picture: dark glass, white type.
class _GlassButton extends StatelessWidget {
  const _GlassButton({
    required this.icon,
    required this.label,
    required this.onTap,
  });

  final IconData icon;
  final String label;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final shape = RoundedRectangleBorder(
      borderRadius: BorderRadius.circular(10),
      side: const BorderSide(color: Color(0x38FFFFFF)),
    );
    return Material(
      color: const Color(0x990A0A0E),
      shape: shape,
      child: InkWell(
        customBorder: shape,
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(icon, size: 16, color: Colors.white),
              const SizedBox(width: 7),
              Text(label,
                  style: const TextStyle(
                      fontSize: 12.5,
                      fontWeight: FontWeight.w600,
                      color: Colors.white)),
            ],
          ),
        ),
      ),
    );
  }
}

class _VersionChip extends StatelessWidget {
  const _VersionChip({required this.label, required this.onTap});

  final String label;
  final VoidCallback onTap;

  static const Color _orange = Color(0xFFF97316);

  @override
  Widget build(BuildContext context) {
    final shape = StadiumBorder(
        side: BorderSide(color: _orange.withValues(alpha: 0.4)));
    return Tooltip(
      message: 'Copy tulipix.pro',
      child: Material(
        color: _orange.withValues(alpha: 0.14),
        shape: shape,
        child: InkWell(
          customBorder: shape,
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 11, vertical: 7),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                const Icon(Icons.copy, size: 13, color: _orange),
                const SizedBox(width: 7),
                Text(label,
                    style: const TextStyle(
                        fontSize: 12,
                        fontWeight: FontWeight.w600,
                        color: _orange)),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _StatusPill extends StatelessWidget {
  const _StatusPill({required this.onTap});

  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final shell = ShellController.instance;
    return AnimatedBuilder(
      animation: shell,
      builder: (context, _) {
        final tint = switch (shell.statusLevel) {
          'ok' => Tokens.ok,
          'warn' => Tokens.warn,
          'error' || 'bad' => Tokens.error,
          _ => context.tokens.textDim,
        };
        return Material(
          color: tint.withValues(alpha: 0.12),
          shape: StadiumBorder(
              side: BorderSide(color: tint.withValues(alpha: 0.4))),
          child: InkWell(
            customBorder: const StadiumBorder(),
            onTap: onTap,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(11, 8, 13, 8),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Container(
                    width: 8,
                    height: 8,
                    decoration:
                        BoxDecoration(color: tint, shape: BoxShape.circle),
                  ),
                  const SizedBox(width: 8),
                  ConstrainedBox(
                    constraints: const BoxConstraints(maxWidth: 220),
                    child: Text(
                      shell.statusNote,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 12,
                          fontWeight: FontWeight.w600,
                          color: context.tokens.text),
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

// ── home layouts ────────────────────────────────────────────────────────────

/// The 16:9 schematic — a port of `LayoutThumb`.
///
/// Rows collapse rather than stretch, so a stripped layout looks stripped:
/// switching a card off takes its block out of every tile.
class _LayoutThumb extends StatelessWidget {
  const _LayoutThumb({required this.kind, required this.cards});

  final String kind;
  final Set<String> cards;

  bool has(String k) => cards.contains(k);

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      decoration: BoxDecoration(
        color: t.bg,
        borderRadius: BorderRadius.circular(9),
        border: Border.all(color: t.outline),
      ),
      clipBehavior: Clip.antiAlias,
      child: Padding(
        padding: const EdgeInsets.all(7),
        child: switch (kind) {
          'cinema' => _cinema(context),
          'stream' => _stream(context),
          'welcome' => _welcome(context),
          _ => _classic(context),
        },
      ),
    );
  }

  static Widget _bar(Color c, {double h = 0}) => Expanded(
        child: Container(
          height: h == 0 ? null : h,
          decoration:
              BoxDecoration(color: c, borderRadius: BorderRadius.circular(3)),
        ),
      );

  Widget _head(BuildContext context, double frac) => Align(
        alignment: Alignment.centerLeft,
        child: FractionallySizedBox(
          widthFactor: frac,
          child: Container(
            height: 5,
            decoration: BoxDecoration(
              color: context.tokens.text.withValues(alpha: 0.32),
              borderRadius: BorderRadius.circular(3),
            ),
          ),
        ),
      );

  // Two rows of tiles, a wide Continue strip, and the rail on the right.
  Widget _classic(BuildContext context) {
    final row1 = [
      if (has('photos')) Tokens.secPhotos,
      if (has('videos')) Tokens.secVideos,
    ];
    final row2 = [
      if (has('books')) Tokens.secBooks,
      if (has('cloud')) Tokens.secCloud,
      if (has('tools')) Tokens.secTools,
      if (has('transfer')) Tokens.secTransfer,
    ];
    final rail = has('player') || has('quick');
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _head(context, 0.44),
        const SizedBox(height: 4),
        Expanded(
          child: Row(
            children: [
              Expanded(
                flex: rail ? 71 : 100,
                child: Column(
                  children: [
                    if (row1.isNotEmpty)
                      Expanded(
                        flex: row2.isEmpty ? 100 : 52,
                        child: Row(
                          children: [
                            for (var i = 0; i < row1.length; i++) ...[
                              if (i > 0) const SizedBox(width: 4),
                              _bar(row1[i].withValues(alpha: 0.55)),
                            ],
                          ],
                        ),
                      ),
                    if (row1.isNotEmpty && row2.isNotEmpty)
                      const SizedBox(height: 4),
                    if (row2.isNotEmpty)
                      Expanded(
                        flex: 48,
                        child: Row(
                          children: [
                            for (var i = 0; i < row2.length; i++) ...[
                              if (i > 0) const SizedBox(width: 4),
                              _bar(row2[i].withValues(alpha: 0.5)),
                            ],
                          ],
                        ),
                      ),
                    if (has('continue')) ...[
                      const SizedBox(height: 4),
                      Container(
                        height: 9,
                        decoration: BoxDecoration(
                          color: Tokens.secMusic.withValues(alpha: 0.42),
                          borderRadius: BorderRadius.circular(3),
                        ),
                      ),
                    ],
                  ],
                ),
              ),
              if (rail) ...[
                const SizedBox(width: 4),
                Expanded(
                  flex: 29,
                  child: Column(
                    children: [
                      if (has('player'))
                        _bar(Tokens.secMusic.withValues(alpha: 0.6)),
                      if (has('player') && has('quick'))
                        const SizedBox(height: 4),
                      if (has('quick'))
                        Expanded(
                          flex: 32,
                          child: Container(
                            decoration: BoxDecoration(
                              color: Tokens.secVideos.withValues(alpha: 0.45),
                              borderRadius: BorderRadius.circular(3),
                            ),
                          ),
                        ),
                    ],
                  ),
                ),
              ],
            ],
          ),
        ),
      ],
    );
  }

  // Hero at full bleed, the player panel right, Continue as a shelf at the foot.
  Widget _cinema(BuildContext context) {
    final t = context.tokens;
    return Stack(
      children: [
        if (has('hero'))
          Positioned.fill(
            child: DecoratedBox(
              decoration: BoxDecoration(
                gradient: LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: [
                    Tokens.secVideos.withValues(alpha: 0.5),
                    Tokens.brand.withValues(alpha: 0.22),
                    const Color(0x00000000),
                  ],
                ),
              ),
            ),
          ),
        Positioned(
            top: 0,
            left: 0,
            child: _pill(t.text.withValues(alpha: 0.7), 34, 5)),
        if (has('hero')) ...[
          Positioned(
              top: 28,
              left: 0,
              child: _pill(t.text.withValues(alpha: 0.85), 60, 8)),
          Positioned(
              top: 39,
              left: 0,
              child: _pill(t.text.withValues(alpha: 0.85), 42, 8)),
          Positioned(top: 52, left: 0, child: _pill(Tokens.secVideos, 21, 5)),
        ],
        if (has('player'))
          Positioned(
            top: 8,
            right: 0,
            child: _block(Tokens.secMusic.withValues(alpha: 0.5), 36, 44),
          ),
        if (has('continue'))
          Positioned(
            left: 0,
            right: 0,
            bottom: 0,
            child: Row(
              children: [
                for (final c in [
                  Tokens.secVideos,
                  Tokens.secBooks,
                  Tokens.secMusic,
                ]) ...[
                  _bar(c.withValues(alpha: 0.5), h: 16),
                  const SizedBox(width: 4),
                ],
                const Spacer(),
              ],
            ),
          ),
      ],
    );
  }

  // A feed down the middle with a standing rail on the right.
  Widget _stream(BuildContext context) {
    final feed = [
      if (has('photos')) Tokens.secPhotos,
      if (has('videos')) Tokens.secVideos,
      if (has('music')) Tokens.secMusic,
      if (has('books')) Tokens.secBooks,
    ];
    final rail = has('player') || has('finances') || has('transfer');
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _head(context, 0.30),
        const SizedBox(height: 4),
        Expanded(
          child: Row(
            children: [
              Expanded(
                flex: rail ? 68 : 100,
                child: Column(
                  children: [
                    for (var i = 0; i < feed.length; i++) ...[
                      if (i > 0) const SizedBox(height: 4),
                      _bar(feed[i].withValues(alpha: 0.5)),
                    ],
                  ],
                ),
              ),
              if (rail) ...[
                const SizedBox(width: 4),
                Expanded(
                  flex: 32,
                  child: Column(
                    children: [
                      if (has('player'))
                        _bar(Tokens.secMusic.withValues(alpha: 0.55)),
                      if (has('player') && has('finances'))
                        const SizedBox(height: 4),
                      if (has('finances'))
                        _bar(Tokens.secFinances.withValues(alpha: 0.5)),
                      if (has('transfer')) ...[
                        const SizedBox(height: 4),
                        _bar(Tokens.secTransfer.withValues(alpha: 0.5)),
                      ],
                    ],
                  ),
                ),
              ],
            ],
          ),
        ),
      ],
    );
  }

  // A big greeting, a row of launchers, the player standing at the foot.
  Widget _welcome(BuildContext context) {
    final t = context.tokens;
    final keys = [
      if (has('photos')) Tokens.secPhotos,
      if (has('videos')) Tokens.secVideos,
      if (has('music')) Tokens.secMusic,
      if (has('books')) Tokens.secBooks,
      if (has('cloud')) Tokens.secCloud,
      if (has('tools')) Tokens.secTools,
    ];
    return Column(
      crossAxisAlignment: CrossAxisAlignment.center,
      children: [
        const Spacer(),
        if (has('hero')) ...[
          _pill(t.text.withValues(alpha: 0.8), 58, 8),
          const SizedBox(height: 4),
          _pill(t.text.withValues(alpha: 0.35), 38, 5),
          const SizedBox(height: 8),
        ],
        if (keys.isNotEmpty)
          SizedBox(
            height: 20,
            child: Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                for (var i = 0; i < keys.length; i++) ...[
                  if (i > 0) const SizedBox(width: 4),
                  Container(
                    width: 15,
                    decoration: BoxDecoration(
                      color: keys[i].withValues(alpha: 0.55),
                      borderRadius: BorderRadius.circular(3),
                    ),
                  ),
                ],
              ],
            ),
          ),
        const Spacer(),
        if (has('player'))
          Container(
            height: 10,
            decoration: BoxDecoration(
              color: Tokens.secMusic.withValues(alpha: 0.55),
              borderRadius: BorderRadius.circular(3),
            ),
          ),
      ],
    );
  }

  static Widget _pill(Color c, double w, double h) => Container(
        width: w,
        height: h,
        decoration:
            BoxDecoration(color: c, borderRadius: BorderRadius.circular(h / 2)),
      );

  static Widget _block(Color c, double w, double h) => Container(
        width: w,
        height: h,
        decoration:
            BoxDecoration(color: c, borderRadius: BorderRadius.circular(3)),
      );
}

// ── credits ─────────────────────────────────────────────────────────────────

class _Credits extends StatelessWidget {
  const _Credits();

  /// What the app is actually built on. The Slint page draws sixteen logos off
  /// a `CreditEntry` list with images; the bundle here has no logo assets, so
  /// these are the names — the same list, legible, and nothing to ship.
  static const List<String> _built = [
    'Rust',
    'Flutter',
    'Slint',
    'mpv',
    'FFmpeg',
    'SQLite',
    'axum',
    'tokio',
    'yt-dlp',
    'PDFium',
    'MusicBrainz',
    'TMDB',
    'radio-browser',
    'Open Library',
    'LibriVox',
    'flutter_rust_bridge',
  ];

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      children: [
        Divider(color: t.outline, height: 24),
        Text('Tulipix © 2026 — Developed by Atish Ak Sharma',
            style: TextStyle(fontSize: 11, color: t.textDim)),
        const SizedBox(height: 12),
        Text('BUILT WITH',
            style: TextStyle(
                fontSize: 10,
                fontWeight: FontWeight.w700,
                letterSpacing: 1,
                color: t.textDim)),
        const SizedBox(height: 10),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          alignment: WrapAlignment.center,
          children: [
            for (final name in _built)
              Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
                decoration: BoxDecoration(
                  color: t.panel,
                  borderRadius: BorderRadius.circular(999),
                  border: Border.all(color: t.outline),
                ),
                child: Text(name,
                    style: TextStyle(fontSize: 11, color: t.textDim)),
              ),
          ],
        ),
      ],
    );
  }
}
