// You & Home — the first Settings tab, and a port of ui/settings_you_home.slint.
//
// Both halves answer "what does the app look like when I open it", which is
// why the Slint build merged Profile, Appearance and Home Layout into one
// entry: choosing a layout and then theming it used to mean crossing the nav
// twice.
//
// The shape is Slint's, top to bottom: an identity banner, a pair of cards for
// Appearance and the mini widget, the four Home layouts as pickable
// schematics, and the credits. What the port leaves out is what the bridge has
// no field for — a cover image and an avatar image are `pick-cover-image` /
// `pick-avatar-image` there and nothing here, because `SaveProfile` carries a
// name, an emoji and a logo index. The emoji IS this build's avatar.

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

class ProfileTab extends StatefulWidget {
  const ProfileTab({
    super.key,
    required this.controller,
    required this.state,
    this.onTab,
  });

  final SettingsController controller;
  final SettingsState state;

  /// Where the status pill goes. The Slint page's `open-status` — the Status
  /// TAB in settings, not the loopback dashboard the sidebar key opens.
  final ValueChanged<String>? onTab;

  @override
  State<ProfileTab> createState() => _ProfileTabState();
}

class _ProfileTabState extends State<ProfileTab> {
  late final TextEditingController _name =
      TextEditingController(text: widget.state.displayName);

  /// The mark being edited. Starts from the saved value and only reaches disk
  /// on Save, which is what makes the row of chips a preview rather than a
  /// stream of writes.
  late int _logo = widget.state.logoChoice;

  /// The plain default mark, picked ON PURPOSE. Slot 0 belongs to the seasonal
  /// swap otherwise, which inside a festival window left no way to ask for the
  /// plain mark at all — every pick of it just showed India again. Session-only,
  /// exactly as in Slint: what is on disk is still `default`.
  bool _defaultPicked = false;

  bool _justSaved = false;

  /// The design language being picked. Null until a tile is clicked; like the
  /// name and the mark, it only reaches disk on Save.
  DesignLanguage? _language;

  DesignLanguage get _savedLanguage => ShellController.instance.designLanguage;

  @override
  void initState() {
    super.initState();
    // `_dirty` reads the field's text, and a TextEditingController notifies its
    // own listeners without rebuilding the widget that holds it -- so typing a
    // new name changed nothing this page could see, `actionDirty` stayed false,
    // and Save stayed dim with `onPressed: null`. Every other control here
    // goes through setState; the text field is the one that does not.
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

  /// What Save writes: the name, the mark and the design language. Theme,
  /// reduce motion and the layout are NOT in here — those go to disk the moment
  /// they are clicked, so counting them as unsaved work would leave the button
  /// lit forever.
  bool get _dirty =>
      !_justSaved &&
      (_name.text != widget.state.displayName ||
          _logo != widget.state.logoChoice ||
          _defaultPicked ||
          (_language != null && _language != _savedLanguage));

  Future<void> _save() async {
    await widget.controller.send(SettingsCmd.saveProfile(
      name: _name.text,
      // Untouched: the emoji is the avatar and has its own editor below.
      emoji: widget.state.avatarEmoji,
      logo: _logo,
    ));
    final language = _language;
    if (language != null && language != _savedLanguage) {
      // The key and spellings the Slint build reads, through the generic text
      // arm — no bridge command of its own.
      await widget.controller.send(SettingsCmd.setText(
        key: 'ui.design-language',
        value: language.id,
      ));
    }
    // The sidebar prints the same name and mark, so it has to hear about it —
    // and the shell snapshot is where the app reads the design language from.
    await ShellController.instance.refresh();
    if (!mounted) return;
    setState(() {
      _defaultPicked = false;
      _language = null;
      _justSaved = true;
    });
    await Future<void>.delayed(const Duration(seconds: 2));
    if (mounted) setState(() => _justSaved = false);
  }

  Future<void> _saveEmoji(String emoji) async {
    await widget.controller.send(SettingsCmd.saveProfile(
      name: _name.text,
      emoji: emoji,
      logo: _logo,
    ));
    await ShellController.instance.refresh();
  }

  Future<void> _editEmoji() async {
    final field = TextEditingController(text: widget.state.avatarEmoji);
    final picked = await showDialog<String>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Avatar'),
        content: SizedBox(
          width: 320,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              TextField(
                controller: field,
                autofocus: true,
                decoration: const InputDecoration(
                  border: OutlineInputBorder(),
                  hintText: '🙂',
                ),
                onSubmitted: (v) => Navigator.pop(context, v),
              ),
              const SizedBox(height: 14),
              Wrap(
                spacing: 6,
                runSpacing: 6,
                children: [
                  for (final e in const [
                    '🙂',
                    '😎',
                    '🎧',
                    '🎬',
                    '📚',
                    '🌊',
                    '🦊',
                    '🐧',
                    '🌙',
                    '⚡',
                    '🍁',
                    '🔮',
                  ])
                    _EmojiChip(
                      emoji: e,
                      onTap: () => Navigator.pop(context, e),
                    ),
                ],
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, field.text),
            child: const Text('Use'),
          ),
        ],
      ),
    );
    field.dispose();
    if (picked != null) await _saveEmoji(picked.trim());
  }

  /// Switching Home out from under someone is not a click to make by accident,
  /// so `Use this layout` asks first — the same dialog the Slint page shows.
  Future<void> _useLayout(String id, String name) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text('Switch Home to $name?'),
        content: const Text(
          'Home is redrawn in the new arrangement. Your card switches for '
          'this layout are kept, and you can switch back at any time.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: Text('Use $name'),
          ),
        ],
      ),
    );
    if (ok == true) {
      await widget.controller.send(SettingsCmd.setHomeLayout(layout: id));
    }
  }

  /// Customize applies the layout first, so the switches and the schematic in
  /// the sheet are the real thing rather than a preview of something else.
  Future<void> _customize(String id) async {
    if (widget.state.homeLayout != id) {
      await widget.controller.send(SettingsCmd.setHomeLayout(layout: id));
    }
    if (!mounted) return;
    await showDialog<void>(
      context: context,
      builder: (_) => _CardsDialog(controller: widget.controller),
    );
  }

  @override
  Widget build(BuildContext context) {
    final st = widget.state;
    return LayoutBuilder(
      builder: (context, box) {
        // 2% of the viewport breathes on both sides at every window width, as
        // the Slint page's padding does.
        final gutter = box.maxWidth * 0.02;
        // Below this the two cards stop being two cards: Slint sizes them off
        // 60% of the page and a half of that is unusable in a narrow window.
        final wide = box.maxWidth - gutter * 2 >= 820;
        return ListView(
          padding: EdgeInsets.fromLTRB(gutter, 18, gutter, 28),
          children: [
            _Identity(
              state: st,
              name: _name,
              language: _language ?? _savedLanguage,
              onLanguage: (l) => setState(() => _language = l),
              onEmoji: _editEmoji,
              onStatus: () => widget.onTab?.call('status'),
              onCopyUrl: () async {
                await Clipboard.setData(
                    const ClipboardData(text: 'https://tulipix.pro'));
                if (context.mounted) {
                  ScaffoldMessenger.maybeOf(context)?.showSnackBar(
                    const SnackBar(content: Text('tulipix.pro copied')),
                  );
                }
              },
            ),
            const SizedBox(height: 18),
            if (wide)
              Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Expanded(flex: 4, child: _appearance(st)),
                  const SizedBox(width: 18),
                  Expanded(flex: 3, child: _miniCard()),
                ],
              )
            else ...[
              _appearance(st),
              const SizedBox(height: 18),
              _miniCard(),
            ],
            const SizedBox(height: 22),
            _HomeHeader(inUse: _layoutName(st.homeLayout)),
            const SizedBox(height: 12),
            _LayoutRow(
              state: st,
              wide: wide,
              onUse: _useLayout,
              onCustomize: _customize,
            ),
            const SizedBox(height: 26),
            const _Credits(),
          ],
        );
      },
    );
  }

  Widget _appearance(SettingsState st) => _Card(
        title: 'Appearance',
        glyph: Icons.contrast,
        tint: const Color(0xFFEC4899),
        // Save lives here, not in the identity row: the name field and the
        // marks are the two things it writes, and it is a click away from both.
        action: _justSaved ? 'Saved ✓' : 'Save changes',
        actionDirty: _dirty,
        onAction: _save,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const _RowLabel('Theme'),
            const SizedBox(height: 8),
            _ThemePicker(
              choice: st.theme,
              onPick: (v) =>
                  widget.controller.send(SettingsCmd.setTheme(theme: v)),
            ),
            const SizedBox(height: 16),
            _SwitchRow(
              title: 'Reduce motion',
              note: 'Drops the transitions that move things across the screen.',
              on: st.reduceMotion,
              onChanged: (v) =>
                  widget.controller.send(SettingsCmd.setReduceMotion(on_: v)),
            ),
            const SizedBox(height: 16),
            const _RowLabel('Sidebar logo'),
            const SizedBox(height: 8),
            _LogoRow(
              selected: _logo,
              defaultPicked: _defaultPicked,
              onPick: (v) => setState(() {
                _logo = v;
                _defaultPicked = v == 0;
              }),
            ),
          ],
        ),
      );

  Widget _miniCard() => AnimatedBuilder(
        animation: MusicController.instance,
        builder: (context, _) {
          final music = MusicController.instance;
          return _Card(
            title: 'Mini Player Widget',
            glyph: Icons.picture_in_picture_alt,
            tint: const Color(0xFF8B5CF6),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const _RowLabel('Desktop widget'),
                const SizedBox(height: 8),
                // Three, because `MiniStyle` has three. The 300x470 card is a
                // different object — the in-app mini player — and it is not a
                // style this picker can choose.
                //
                // The sizes are the styles' own, not a table beside them:
                // `MiniStyle.base` is what the window is actually resized to.
                for (final s in MiniStyle.values)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 6),
                    child: _PickTile(
                      label: s.label,
                      note: '${s.base.width.round()} × '
                          '${s.base.height.round()}',
                      active: music.widgetStyle == s,
                      onTap: () {
                        music.setWidgetStyle(s);
                        // Written through as well, so the Slint build's own
                        // widget opens in the style picked here — it reads
                        // `ui.mini-widget.style`, same key, same three names.
                        widget.controller.send(SettingsCmd.setText(
                          key: 'ui.mini-widget.style',
                          value: s.name,
                        ));
                      },
                    ),
                  ),
                const SizedBox(height: 10),
                Text(
                  'Opened from the title bar, and the window becomes it. The '
                  'mini player is a different thing — a card inside the app, '
                  'opened from the player.',
                  style: TextStyle(
                      fontSize: 11, color: context.tokens.textDim, height: 1.4),
                ),
              ],
            ),
          );
        },
      );
}

// ── identity ────────────────────────────────────────────────────────────────

/// Cover, avatar straddling the seam, and everything else on ONE row beneath
/// it: name, version, and app health at the right edge. The name used to sit
/// over the cover gradient, where it was unreadable on every theme.
class _Identity extends StatelessWidget {
  const _Identity({
    required this.state,
    required this.name,
    required this.language,
    required this.onLanguage,
    required this.onEmoji,
    required this.onStatus,
    required this.onCopyUrl,
  });

  final SettingsState state;
  final TextEditingController name;

  /// The design language shown as picked — staged if one was clicked, the
  /// stored one otherwise.
  final DesignLanguage language;
  final ValueChanged<DesignLanguage> onLanguage;
  final VoidCallback onEmoji;
  final VoidCallback onStatus;
  final VoidCallback onCopyUrl;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      decoration: context.skin
              .surface(SurfaceRole.card, radius: Tokens.radiusLg) ??
          BoxDecoration(
            color: t.panel2,
            borderRadius: BorderRadius.circular(Tokens.radiusLg),
            border: Border.all(color: t.outline),
          ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        children: [
          // 4:1, the cropper's mask exactly, clamped at both ends so a very
          // narrow or very wide window still gets a sane band.
          LayoutBuilder(
            builder: (context, box) => SizedBox(
              height: (box.maxWidth / 4).clamp(120.0, 200.0),
              child: const Stack(
                fit: StackFit.expand,
                children: [
                  DecoratedBox(
                    decoration: BoxDecoration(
                      gradient: LinearGradient(
                        begin: Alignment.topLeft,
                        end: Alignment.bottomRight,
                        colors: [
                          Tokens.brand,
                          Tokens.brand2,
                          Color(0xFF0EA5E9),
                        ],
                      ),
                    ),
                  ),
                  // The two blobs the Slint cover draws when no image is set.
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
              ),
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.center,
              children: [
                // Only the avatar rides up over the seam, so the row reserves
                // a short box and the circle hangs out of the top of it.
                SizedBox(
                  width: 92,
                  height: 52,
                  child: OverflowBox(
                    maxHeight: 92,
                    alignment: Alignment.topCenter,
                    child: Transform.translate(
                      offset: const Offset(0, -40),
                      child: _Avatar(emoji: state.avatarEmoji, onTap: onEmoji),
                    ),
                  ),
                ),
                const SizedBox(width: 16),
                // Capped at 12 characters: the Focused layout sizes its
                // greeting off a width factor and wraps past that. Rust clamps
                // again on save and on load.
                SizedBox(
                  width: 150,
                  child: TextField(
                    controller: name,
                    maxLength: 12,
                    style: TextStyle(
                        fontSize: 19,
                        fontWeight: FontWeight.w700,
                        color: t.text),
                    decoration: InputDecoration(
                      isDense: true,
                      counterText: '',
                      hintText: 'Your name',
                      hintStyle: TextStyle(
                        fontSize: 19,
                        fontWeight: FontWeight.w700,
                        color: t.textDim.withValues(alpha: 0.45),
                      ),
                      enabledBorder: UnderlineInputBorder(
                          borderSide: BorderSide(color: t.outline, width: 2)),
                      focusedBorder: const UnderlineInputBorder(
                          borderSide:
                              BorderSide(color: Tokens.brand, width: 2)),
                    ),
                  ),
                ),
                const SizedBox(width: 14),
                _Chip(
                  label: 'Tulipix v${state.appVersion}',
                  icon: Icons.link,
                  fill: const Color(0xFFF97316),
                  onTap: onCopyUrl,
                ),
                const Spacer(),
                Flexible(child: _StatusPill(onTap: onStatus)),
              ],
            ),
          ),
          // Under the name: which material Music is drawn in. Staged like the
          // name and the mark — it goes on Save changes, in the Appearance card.
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const _RowLabel('Design language'),
                const SizedBox(height: 8),
                Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    for (final l in DesignLanguage.values)
                      _LanguageTile(
                        language: l,
                        active: l == language,
                        onTap: () => onLanguage(l),
                      ),
                  ],
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// One design language: a two-tone swatch of its material, the style's name,
/// and underneath this app's name for it.
class _LanguageTile extends StatelessWidget {
  const _LanguageTile({
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
      child: Material(
        color:
            active ? Tokens.brand.withValues(alpha: 0.12) : Colors.transparent,
        borderRadius: BorderRadius.circular(Tokens.radiusSm),
        child: InkWell(
          borderRadius: BorderRadius.circular(Tokens.radiusSm),
          onTap: onTap,
          child: Container(
            width: 184,
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(Tokens.radiusSm),
              border: Border.all(
                color: active ? Tokens.brand : t.outline,
                width: active ? 1.5 : 1,
              ),
            ),
            child: Row(
              children: [
                Container(
                  width: 24,
                  height: 24,
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
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(
                        language.label,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          fontSize: 12.5,
                          fontWeight: FontWeight.w600,
                          color: t.text,
                        ),
                      ),
                      Text(
                        language.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 10.5, color: t.textDim),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
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

class _Avatar extends StatefulWidget {
  const _Avatar({required this.emoji, required this.onTap});

  final String emoji;
  final VoidCallback onTap;

  @override
  State<_Avatar> createState() => _AvatarState();
}

class _AvatarState extends State<_Avatar> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 140),
          width: 92,
          height: 92,
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(26),
            gradient: const LinearGradient(
              begin: Alignment.topLeft,
              end: Alignment.bottomRight,
              colors: [Tokens.brand, Tokens.brand2],
            ),
            border:
                Border.all(color: _hover ? Tokens.focus : t.panel2, width: 4),
          ),
          clipBehavior: Clip.antiAlias,
          child: Stack(
            fit: StackFit.expand,
            children: [
              Center(
                child: widget.emoji.trim().isEmpty
                    ? const Icon(Icons.person, size: 38, color: Colors.white)
                    : Text(widget.emoji, style: const TextStyle(fontSize: 40)),
              ),
              AnimatedOpacity(
                duration: const Duration(milliseconds: 140),
                opacity: _hover ? 1 : 0,
                child: const ColoredBox(
                  color: Color(0x6B000000),
                  child: Center(
                    child: Icon(Icons.edit, size: 24, color: Colors.white),
                  ),
                ),
              ),
            ],
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
          color: tint.withValues(alpha: 0.14),
          borderRadius: BorderRadius.circular(999),
          child: InkWell(
            borderRadius: BorderRadius.circular(999),
            onTap: onTap,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(11, 7, 13, 7),
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
                          fontSize: 11.5,
                          fontWeight: FontWeight.w600,
                          color: tint),
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

class _Chip extends StatelessWidget {
  const _Chip({
    required this.label,
    required this.icon,
    required this.fill,
    required this.onTap,
  });

  final String label;
  final IconData icon;
  final Color fill;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => Material(
        color: fill,
        borderRadius: BorderRadius.circular(999),
        child: InkWell(
          borderRadius: BorderRadius.circular(999),
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(11, 6, 12, 6),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(icon, size: 13, color: Colors.white),
                const SizedBox(width: 6),
                Text(label,
                    style: const TextStyle(
                        fontSize: 11.5,
                        fontWeight: FontWeight.w700,
                        color: Colors.white)),
              ],
            ),
          ),
        ),
      );
}

class _EmojiChip extends StatelessWidget {
  const _EmojiChip({required this.emoji, required this.onTap});

  final String emoji;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => Material(
        color: context.tokens.panel,
        borderRadius: BorderRadius.circular(Tokens.radiusSm),
        child: InkWell(
          borderRadius: BorderRadius.circular(Tokens.radiusSm),
          onTap: onTap,
          child: SizedBox(
            width: 40,
            height: 40,
            child: Center(
                child: Text(emoji, style: const TextStyle(fontSize: 20))),
          ),
        ),
      );
}

// ── the card shell ──────────────────────────────────────────────────────────

class _Card extends StatelessWidget {
  const _Card({
    required this.title,
    required this.glyph,
    required this.tint,
    required this.child,
    this.action,
    this.actionDirty = false,
    this.onAction,
  });

  final String title;
  final IconData glyph;
  final Color tint;
  final Widget child;
  final String? action;
  final bool actionDirty;
  final VoidCallback? onAction;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.fromLTRB(16, 14, 16, 16),
      decoration: context.skin
              .surface(SurfaceRole.card, radius: Tokens.radiusLg) ??
          BoxDecoration(
            color: t.panel2,
            borderRadius: BorderRadius.circular(Tokens.radiusLg),
            border: Border.all(color: t.outline),
          ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Container(
                width: 28,
                height: 28,
                decoration: context.skin
                        .control(active: true, tint: tint, radius: 9) ??
                    BoxDecoration(
                      color: tint.withValues(alpha: 0.16),
                      borderRadius: BorderRadius.circular(9),
                    ),
                child: Icon(context.skin.icon(glyph), size: 15, color: tint),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Text(title,
                    style: TextStyle(
                        fontSize: 14,
                        fontWeight: FontWeight.w700,
                        color: t.text)),
              ),
              if (action != null)
                // Lit only while there is something to write — a Save that
                // stays lit is a Save nobody reads.
                FilledButton(
                  style: FilledButton.styleFrom(
                    visualDensity: VisualDensity.compact,
                    backgroundColor: actionDirty ? Tokens.brand : t.panel,
                    foregroundColor: actionDirty ? Colors.white : t.textDim,
                  ),
                  onPressed: actionDirty ? onAction : null,
                  child: Text(action!, style: const TextStyle(fontSize: 12)),
                ),
            ],
          ),
          const SizedBox(height: 14),
          child,
        ],
      ),
    );
  }
}

class _RowLabel extends StatelessWidget {
  const _RowLabel(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Text(
        text.toUpperCase(),
        style: TextStyle(
            fontSize: 10,
            fontWeight: FontWeight.w800,
            letterSpacing: 0.9,
            color: context.tokens.textDim),
      );
}

class _SwitchRow extends StatelessWidget {
  const _SwitchRow({
    required this.title,
    required this.on,
    required this.onChanged,
    this.note = '',
  });

  final String title;
  final String note;
  final bool on;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
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
                Text(note, style: TextStyle(fontSize: 11, color: t.textDim)),
            ],
          ),
        ),
        Switch(
          value: on,
          activeThumbColor: Tokens.brand,
          onChanged: onChanged,
        ),
      ],
    );
  }
}

// ── appearance controls ─────────────────────────────────────────────────────

class _ThemePicker extends StatelessWidget {
  const _ThemePicker({required this.choice, required this.onPick});

  final String choice;
  final ValueChanged<String> onPick;

  static const List<({String id, String label, Color a, Color b})> _themes = [
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

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final at = _themes.any((x) => x.id == choice) ? choice : 'system';
    return Row(
      children: [
        for (final th in _themes)
          Expanded(
            child: Padding(
              padding: const EdgeInsets.only(right: 8),
              child: Material(
                color: Colors.transparent,
                child: InkWell(
                  borderRadius: BorderRadius.circular(Tokens.radiusSm),
                  onTap: () => onPick(th.id),
                  child: Container(
                    padding: const EdgeInsets.all(6),
                    decoration: BoxDecoration(
                      borderRadius: BorderRadius.circular(Tokens.radiusSm),
                      border: Border.all(
                        color: at == th.id ? Tokens.brand : t.outline,
                        width: at == th.id ? 2 : 1,
                      ),
                    ),
                    child: Column(
                      children: [
                        Container(
                          height: 26,
                          decoration: BoxDecoration(
                            borderRadius: BorderRadius.circular(5),
                            gradient: LinearGradient(
                              begin: Alignment.topLeft,
                              end: Alignment.bottomRight,
                              colors: [th.a, th.b],
                            ),
                          ),
                        ),
                        const SizedBox(height: 5),
                        Text(
                          th.label,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            fontSize: 10.5,
                            fontWeight:
                                at == th.id ? FontWeight.w700 : FontWeight.w500,
                            color: at == th.id ? t.text : t.textDim,
                          ),
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            ),
          ),
      ],
    );
  }
}

/// Six tiles: what the sidebar is wearing, then the five marks to pick from.
class _LogoRow extends StatelessWidget {
  const _LogoRow({
    required this.selected,
    required this.defaultPicked,
    required this.onPick,
  });

  final int selected;
  final bool defaultPicked;
  final ValueChanged<int> onPick;

  static const List<String> _labels = [
    'Default',
    'Colour',
    'Dark',
    'White',
    'India',
  ];

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return LayoutBuilder(
      builder: (context, box) {
        const gap = 8.0;
        final cell = ((box.maxWidth - 6 * gap) / 6).clamp(30.0, 46.0);
        return Row(
          children: [
            Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text('ACTIVE',
                    style: TextStyle(
                        fontSize: 8,
                        fontWeight: FontWeight.w700,
                        letterSpacing: 0.8,
                        color: t.textDim)),
                const SizedBox(height: 4),
                // Preview only — the five beside it are what changes it. The
                // seasonal swap shows HERE, which is why the Default chip below
                // always draws the plain mark: two India tiles in one row read
                // as a bug.
                Container(
                  width: cell,
                  height: cell,
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(12),
                    border: Border.all(color: Tokens.brand, width: 2),
                  ),
                  clipBehavior: Clip.antiAlias,
                  child: Image.asset(
                    selected == 0 && defaultPicked
                        ? 'assets/appicons/sidebar-default.png'
                        : appLogoAsset(selected),
                    fit: BoxFit.cover,
                  ),
                ),
              ],
            ),
            const SizedBox(width: gap * 2),
            for (var i = 0; i < 5; i++)
              Padding(
                padding: const EdgeInsets.only(top: 16, right: gap),
                child: _LogoChip(
                  // Always the plain mark for slot 0, even in a festival
                  // window.
                  asset: i == 0
                      ? 'assets/appicons/sidebar-default.png'
                      : appLogoAsset(i),
                  label: _labels[i],
                  size: cell,
                  selected: selected == i,
                  onTap: () => onPick(i),
                ),
              ),
          ],
        );
      },
    );
  }
}

class _LogoChip extends StatelessWidget {
  const _LogoChip({
    required this.asset,
    required this.label,
    required this.size,
    required this.selected,
    required this.onTap,
  });

  final String asset;
  final String label;
  final double size;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Tooltip(
      message: label,
      child: GestureDetector(
        onTap: onTap,
        child: MouseRegion(
          cursor: SystemMouseCursors.click,
          child: Container(
            width: size,
            height: size,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(12),
              border: Border.all(
                color: selected ? Tokens.brand : t.outline,
                width: selected ? 2 : 1,
              ),
            ),
            clipBehavior: Clip.antiAlias,
            child: Image.asset(asset, fit: BoxFit.cover),
          ),
        ),
      ),
    );
  }
}

class _PickTile extends StatelessWidget {
  const _PickTile({
    required this.label,
    required this.note,
    required this.active,
    required this.onTap,
  });

  final String label;
  final String note;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final skin = context.skin;
    return Material(
      color: active && skin.isStandard
          ? Tokens.brand.withValues(alpha: 0.12)
          : Colors.transparent,
      borderRadius: BorderRadius.circular(Tokens.radiusSm),
      child: InkWell(
        borderRadius: BorderRadius.circular(Tokens.radiusSm),
        onTap: onTap,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 11, vertical: 9),
          decoration: skin.control(
                active: active,
                tint: Tokens.brand,
                radius: Tokens.radiusSm,
              ) ??
              BoxDecoration(
                borderRadius: BorderRadius.circular(Tokens.radiusSm),
                border: Border.all(
                  color: active ? Tokens.brand : t.outline,
                  width: active ? 1.5 : 1,
                ),
              ),
          child: Row(
            children: [
              Icon(
                active ? Icons.radio_button_checked : Icons.radio_button_off,
                size: 15,
                color: active ? Tokens.brand : t.textDim,
              ),
              const SizedBox(width: 9),
              Expanded(
                child: Text(label,
                    style: TextStyle(
                        fontSize: 12.5,
                        fontWeight: FontWeight.w600,
                        color: t.text)),
              ),
              Text(note,
                  style: TextStyle(
                      fontSize: 10.5,
                      color: t.textDim,
                      fontFeatures: const [FontFeature.tabularFigures()])),
            ],
          ),
        ),
      ),
    );
  }
}

// ── home layouts ────────────────────────────────────────────────────────────

class _HomeHeader extends StatelessWidget {
  const _HomeHeader({required this.inUse});

  final String inUse;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      children: [
        Container(
          width: 30,
          height: 30,
          decoration: BoxDecoration(
            color: Tokens.brand.withValues(alpha: 0.16),
            borderRadius: BorderRadius.circular(10),
          ),
          child: const Icon(Icons.grid_view, size: 15, color: Tokens.brand),
        ),
        const SizedBox(width: 10),
        Expanded(
          child: Text('Home Layouts',
              style: TextStyle(
                  fontSize: 15, fontWeight: FontWeight.w700, color: t.text)),
        ),
        Container(
          padding: const EdgeInsets.symmetric(horizontal: 11, vertical: 5),
          decoration: BoxDecoration(
            color: Tokens.brand.withValues(alpha: 0.18),
            borderRadius: BorderRadius.circular(12),
          ),
          child: Text('IN USE · ${inUse.toUpperCase()}',
              style: const TextStyle(
                  fontSize: 10,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 0.6,
                  color: Tokens.brand)),
        ),
      ],
    );
  }
}

class _LayoutRow extends StatelessWidget {
  const _LayoutRow({
    required this.state,
    required this.wide,
    required this.onUse,
    required this.onCustomize,
  });

  final SettingsState state;
  final bool wide;
  final void Function(String id, String name) onUse;
  final void Function(String id) onCustomize;

  @override
  Widget build(BuildContext context) {
    final on = {
      for (final c in state.homeCards)
        if (c.on_) c.key
    };
    final tiles = [
      for (final l in kHomeLayoutTiles)
        _LayoutTile(
          layout: l,
          active: state.homeLayout == l.id,
          cards: on,
          onUse: () => onUse(l.id, l.name),
          onCustomize: () => onCustomize(l.id),
        ),
    ];
    // Four across, or two rows of two — a grid would size its columns off the
    // longest blurb and the tiles would stop matching, which is why Slint
    // places them by hand.
    const gap = 14.0;
    if (wide) {
      return IntrinsicHeight(
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
    }
    return Column(
      children: [
        for (var i = 0; i < tiles.length; i += 2) ...[
          if (i > 0) const SizedBox(height: gap),
          IntrinsicHeight(
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Expanded(child: tiles[i]),
                const SizedBox(width: gap),
                if (i + 1 < tiles.length)
                  Expanded(child: tiles[i + 1])
                else
                  const Spacer(),
              ],
            ),
          ),
        ],
      ],
    );
  }
}

class _LayoutTile extends StatelessWidget {
  const _LayoutTile({
    required this.layout,
    required this.active,
    required this.cards,
    required this.onUse,
    required this.onCustomize,
  });

  final ({String id, String name, String blurb}) layout;
  final bool active;
  final Set<String> cards;
  final VoidCallback onUse;
  final VoidCallback onCustomize;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(14),
      // The layout in use is latched in the brand's colour; the others are the
      // skin's cards.
      decoration: (active
              ? context.skin.control(
                  active: true, tint: Tokens.brand, radius: Tokens.radiusLg)
              : context.skin
                  .surface(SurfaceRole.card, radius: Tokens.radiusLg)) ??
          BoxDecoration(
            color: t.panel2,
            borderRadius: BorderRadius.circular(Tokens.radiusLg),
            border: Border.all(
              color: active ? Tokens.brand : t.outline,
              width: active ? 2 : 1,
            ),
          ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          AspectRatio(
            aspectRatio: 16 / 9,
            child: _LayoutThumb(kind: layout.id, cards: cards),
          ),
          const SizedBox(height: 10),
          Row(
            children: [
              Expanded(
                child: Text(layout.name,
                    style: TextStyle(
                        fontSize: 14,
                        fontWeight: FontWeight.w700,
                        color: t.text)),
              ),
              if (active)
                const Text('IN USE',
                    style: TextStyle(
                        fontSize: 8.5,
                        fontWeight: FontWeight.w700,
                        letterSpacing: 0.6,
                        color: Tokens.brand)),
            ],
          ),
          const SizedBox(height: 4),
          Text(layout.blurb,
              style: TextStyle(fontSize: 11, height: 1.35, color: t.textDim)),
          const SizedBox(height: 10),
          Row(
            children: [
              Expanded(
                child: active
                    ? OutlinedButton(
                        style: OutlinedButton.styleFrom(
                            visualDensity: VisualDensity.compact),
                        onPressed: null,
                        child: const Text('In use',
                            style: TextStyle(fontSize: 11.5)),
                      )
                    : FilledButton(
                        style: FilledButton.styleFrom(
                            visualDensity: VisualDensity.compact),
                        onPressed: onUse,
                        child: const Text('Use this',
                            style: TextStyle(fontSize: 11.5)),
                      ),
              ),
              const SizedBox(width: 6),
              IconButton(
                tooltip: 'Customize cards',
                visualDensity: VisualDensity.compact,
                iconSize: 17,
                onPressed: onCustomize,
                icon: const Icon(Icons.tune),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

/// The 16:9 schematic — a port of `LayoutThumb`.
///
/// Rows collapse rather than stretch, so a stripped layout looks stripped:
/// switching a card off in the sheet takes its block out of every tile.
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

/// The card switches for the layout in use — Slint's Customize popup.
class _CardsDialog extends StatelessWidget {
  const _CardsDialog({required this.controller});

  final SettingsController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: controller,
      builder: (context, _) {
        final st = controller.state;
        if (st == null) return const SizedBox.shrink();
        final on = {
          for (final c in st.homeCards)
            if (c.on_) c.key
        };
        return AlertDialog(
          title: Row(
            children: [
              Expanded(child: Text('${_layoutName(st.homeLayout)} — cards')),
              TextButton(
                onPressed: () =>
                    controller.send(const SettingsCmd.homeCardsReset()),
                child: const Text('Reset', style: TextStyle(fontSize: 12)),
              ),
            ],
          ),
          content: SizedBox(
            width: 520,
            child: SingleChildScrollView(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                mainAxisSize: MainAxisSize.min,
                children: [
                  // The same schematic the tile shows, so switching a card off
                  // is visible before the page is looked at.
                  Center(
                    child: SizedBox(
                      width: 260,
                      child: AspectRatio(
                        aspectRatio: 16 / 9,
                        child: _LayoutThumb(kind: st.homeLayout, cards: on),
                      ),
                    ),
                  ),
                  const SizedBox(height: 14),
                  for (final card in st.homeCards)
                    _SwitchRow(
                      title: card.label,
                      on: card.on_,
                      onChanged: (v) => controller
                          .send(SettingsCmd.homeCardSet(key: card.key, on_: v)),
                    ),
                  if (st.homeLayout == 'classic') ...[
                    Divider(color: t.outline, height: 24),
                    _SwitchRow(
                      title: 'Music rail on the left',
                      note: 'Swaps the rail and the card deck.',
                      on: st.homeMusicLeft,
                      onChanged: (v) => controller.send(
                          SettingsCmd.toggle(key: 'home.music-left', on_: v)),
                    ),
                  ],
                ],
              ),
            ),
          ),
          actions: [
            FilledButton(
              onPressed: () => Navigator.pop(context),
              child: const Text('Done'),
            ),
          ],
        );
      },
    );
  }
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
