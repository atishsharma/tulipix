// Bloom colours — the popup Material 3 Expressive opens from Design language.
//
// Android's "Wallpaper & style", with the cover that is playing as the
// wallpaper: a colour source, a scheme style, a contrast level, and whether
// the section colours lean toward the seed. The preview is drawn in the
// scheme being picked; Apply writes the `ui.bloom.*` keys and the app
// re-tones from the next shell snapshot (see lib/design/bloom.dart).
// Mockup: docs/mockups/NewSections/settings-bloom-colours-deck.html.

import 'dart:io';

import 'package:flutter/material.dart';

import '../../design/bloom.dart';
import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/settings.dart';
import '../../src/rust/api/shell.dart' show desktopAccent;
import '../music/music_controller.dart';
import 'settings_controller.dart';

Future<void> showBloomDialog(BuildContext context, SettingsController c) =>
    showDialog<void>(
      context: context,
      builder: (_) => _BloomDialog(controller: c),
    );

const _styles = [
  (DynamicSchemeVariant.tonalSpot, 'Tonal spot', 'Calm; keeps your colour'),
  (DynamicSchemeVariant.expressive, 'Expressive', 'Hues swing wide'),
  (DynamicSchemeVariant.vibrant, 'Vibrant', 'Loud and saturated'),
  (DynamicSchemeVariant.fidelity, 'Fidelity', 'True to the seed'),
  (DynamicSchemeVariant.content, 'Content', 'Made for cover art'),
  (DynamicSchemeVariant.rainbow, 'Rainbow', 'Colourful, grey surfaces'),
  (DynamicSchemeVariant.fruitSalad, 'Fruit salad', 'Playful, shifted hues'),
  (DynamicSchemeVariant.neutral, 'Neutral', 'Almost grey'),
  (DynamicSchemeVariant.monochrome, 'Monochrome', 'Pure greys'),
];

const _basic = [
  Color(0xFFEC4899),
  Color(0xFFEF4444),
  Color(0xFFF97316),
  Color(0xFFEAB308),
  Color(0xFF22C55E),
  Color(0xFF14B8A6),
  Color(0xFF3B82F6),
  Color(0xFF8B5CF6),
];

class _BloomDialog extends StatefulWidget {
  const _BloomDialog({required this.controller});

  final SettingsController controller;

  @override
  State<_BloomDialog> createState() => _BloomDialogState();
}

class _BloomDialogState extends State<_BloomDialog> {
  final _shell = ShellController.instance;
  final _music = MusicController.instance;

  late String _source;
  late Color _pick;
  late DynamicSchemeVariant _style;
  late double _contrast;
  late bool _harmonise;
  bool? _dark;
  Color? _desktop;
  bool _saving = false;
  late final TextEditingController _hex;

  @override
  void initState() {
    super.initState();
    final st = _shell.state;
    _source = st?.bloomSource ?? 'cover';
    _pick = hexColor(st?.bloomSeed ?? '') ?? bloom.seed;
    _style = bloomStyle(st?.bloomStyle ?? '');
    _contrast = st?.bloomContrast ?? 0;
    _harmonise = st?.bloomHarmonise ?? false;
    _hex = TextEditingController(text: colorHex(_pick).toUpperCase());
    // Read here rather than in every snapshot: on Linux it runs gsettings.
    desktopAccent().then((v) {
      if (mounted) setState(() => _desktop = hexColor(v));
    });
  }

  @override
  void dispose() {
    _hex.dispose();
    super.dispose();
  }

  Color get _seed => switch (_source) {
        'desktop' => _desktop ?? Tokens.secMusic,
        'pick' => _pick,
        _ => _music.accent,
      };

  void _choose(Color c) => setState(() {
        _source = 'pick';
        _pick = c;
        _hex.text = colorHex(c).toUpperCase();
      });

  Future<void> _apply() async {
    setState(() => _saving = true);
    final c = widget.controller;
    await c.send(SettingsCmd.setText(key: 'ui.bloom.source', value: _source));
    if (_source == 'pick') {
      await c.send(
          SettingsCmd.setText(key: 'ui.bloom.seed', value: colorHex(_pick)));
    }
    await c.send(SettingsCmd.setText(key: 'ui.bloom.style', value: _style.name));
    await c.send(
        SettingsCmd.setText(key: 'ui.bloom.contrast', value: '$_contrast'));
    await c.send(
        SettingsCmd.toggle(key: 'ui.bloom.harmonise', on_: _harmonise));
    await _shell.refresh();
    if (mounted) Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    final dark = _dark ?? Theme.of(context).brightness == Brightness.dark;
    return Dialog(
      insetPadding: const EdgeInsets.all(24),
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 940, maxHeight: 820),
        child: ListenableBuilder(
          // The cover's colours follow the record while this is open.
          listenable: _music,
          builder: (context, _) {
            final seed = _seed;
            final s = bloomScheme(seed, _style, dark: dark, contrast: _contrast);
            final controls = _controls(context, seed);
            final preview = _previewColumn(s, dark);
            return Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Flexible(
                  child: LayoutBuilder(
                    builder: (context, box) => box.maxWidth >= 760
                        ? Row(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              Expanded(
                                child: SingleChildScrollView(
                                  padding: const EdgeInsets.fromLTRB(24, 22, 12, 18),
                                  child: controls,
                                ),
                              ),
                              SizedBox(
                                width: 330,
                                child: SingleChildScrollView(
                                  padding: const EdgeInsets.fromLTRB(12, 22, 24, 18),
                                  child: preview,
                                ),
                              ),
                            ],
                          )
                        : SingleChildScrollView(
                            padding: const EdgeInsets.fromLTRB(20, 20, 20, 16),
                            child: Column(
                              crossAxisAlignment: CrossAxisAlignment.stretch,
                              children: [controls, const SizedBox(height: 18), preview],
                            ),
                          ),
                  ),
                ),
                const Divider(height: 1),
                Padding(
                  padding: const EdgeInsets.fromLTRB(24, 12, 20, 14),
                  child: Row(
                    children: [
                      Expanded(
                        child: Text(
                          '${colorHex(seed).toUpperCase()} · '
                          '${_styles.firstWhere((x) => x.$1 == _style).$2}',
                          style: TextStyle(
                              fontSize: 12,
                              color: Theme.of(context).colorScheme.onSurfaceVariant),
                        ),
                      ),
                      TextButton(
                        onPressed: () => setState(() {
                          _source = 'pick';
                          _pick = Tokens.secMusic;
                          _hex.text = colorHex(_pick).toUpperCase();
                          _style = DynamicSchemeVariant.tonalSpot;
                          _contrast = 0;
                          _harmonise = false;
                        }),
                        child: const Text('Reset'),
                      ),
                      const SizedBox(width: 4),
                      TextButton(
                        onPressed: () => Navigator.of(context).pop(),
                        child: const Text('Cancel'),
                      ),
                      const SizedBox(width: 8),
                      FilledButton(
                        onPressed: _saving ? null : _apply,
                        child: const Text('Apply'),
                      ),
                    ],
                  ),
                ),
              ],
            );
          },
        ),
      ),
    );
  }

  Widget _controls(BuildContext context, Color seed) {
    final cs = Theme.of(context).colorScheme;
    final art = _music.state?.now.art ?? '';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const Text('Bloom colours',
            style: TextStyle(fontSize: 22, fontWeight: FontWeight.w600)),
        Text('One colour in; every surface, button and chip toned from it.',
            style: TextStyle(fontSize: 12.5, color: cs.onSurfaceVariant)),
        const _Label('Colour from'),
        SegmentedButton<String>(
          segments: const [
            ButtonSegment(value: 'cover', label: Text('Cover art')),
            ButtonSegment(value: 'desktop', label: Text('Desktop accent')),
            ButtonSegment(value: 'pick', label: Text('Pick a colour')),
          ],
          selected: {_source},
          onSelectionChanged: (v) => setState(() => _source = v.first),
        ),
        const SizedBox(height: 14),
        ...switch (_source) {
          'cover' => [
              Row(
                children: [
                  ClipRRect(
                    borderRadius: BorderRadius.circular(14),
                    child: SizedBox(
                      width: 64,
                      height: 64,
                      child: art.isEmpty
                          ? ColoredBox(
                              color: cs.surfaceContainerHighest,
                              child: Icon(Icons.music_note, color: cs.onSurfaceVariant))
                          : Image.file(File(art), fit: BoxFit.cover),
                    ),
                  ),
                  const SizedBox(width: 14),
                  Expanded(
                    child: Text(
                      art.isEmpty
                          ? 'Nothing is playing, so Music pink stands in. '
                              'Play something and its cover takes over.'
                          : 'Each new record re-tones the app. Tap a colour '
                              'below to keep it instead.',
                      style: TextStyle(fontSize: 12.5, color: cs.onSurfaceVariant),
                    ),
                  ),
                ],
              ),
              const _Label('Colours in this cover'),
              Wrap(spacing: 10, runSpacing: 10, children: [
                for (final c in {_music.accent, _music.accentAlt})
                  _Swatch(c, on: false, onTap: () => _choose(c)),
              ]),
            ],
          'desktop' => [
              Row(children: [
                _Swatch(_desktop ?? Tokens.secMusic, on: true, onTap: () {}),
                const SizedBox(width: 14),
                Expanded(
                  child: Text(
                    _desktop == null
                        ? 'Your desktop reports no accent colour, so Music '
                            'pink stands in.'
                        : 'Your desktop’s accent. Change it there and Bloom '
                            'follows.',
                    style: TextStyle(fontSize: 12.5, color: cs.onSurfaceVariant),
                  ),
                ),
              ]),
            ],
          _ => [
              Wrap(spacing: 10, runSpacing: 10, children: [
                for (final c in _basic)
                  _Swatch(c,
                      on: c.toARGB32() == _pick.toARGB32(), onTap: () => _choose(c)),
              ]),
              const _Label('Any colour'),
              Row(children: [
                Expanded(
                  child: Slider(
                    value: HSLColor.fromColor(_pick).hue,
                    max: 360,
                    activeColor: _pick,
                    onChanged: (h) => _choose(
                        HSLColor.fromColor(_pick).withHue(h % 360).toColor()),
                  ),
                ),
                SizedBox(
                  width: 120,
                  child: TextField(
                    controller: _hex,
                    decoration: InputDecoration(
                      isDense: true,
                      border: const OutlineInputBorder(),
                      prefixIcon: Padding(
                        padding: const EdgeInsets.all(10),
                        child: CircleAvatar(radius: 6, backgroundColor: _pick),
                      ),
                      prefixIconConstraints: const BoxConstraints(),
                    ),
                    style: const TextStyle(fontSize: 13, fontFamily: 'monospace'),
                    onChanged: (v) {
                      final c = hexColor(v.startsWith('#') ? v : '#$v');
                      if (c != null) setState(() => _pick = c);
                    },
                  ),
                ),
              ]),
            ],
        },
        const _Label('Style'),
        LayoutBuilder(builder: (context, box) {
          final w = (box.maxWidth - 16) / 3;
          return Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              for (final x in _styles)
                SizedBox(
                  width: box.maxWidth < 420 ? (box.maxWidth - 8) / 2 : w,
                  child: _StyleTile(
                    seed: seed,
                    style: x.$1,
                    name: x.$2,
                    hint: x.$3,
                    on: x.$1 == _style,
                    onTap: () => setState(() => _style = x.$1),
                  ),
                ),
            ],
          );
        }),
        const _Label('Contrast'),
        SegmentedButton<double>(
          segments: const [
            ButtonSegment(value: 0, label: Text('Standard')),
            ButtonSegment(value: 0.5, label: Text('Medium')),
            ButtonSegment(value: 1, label: Text('High')),
          ],
          selected: {_contrast},
          onSelectionChanged: (v) => setState(() => _contrast = v.first),
        ),
        const SizedBox(height: 8),
        SwitchListTile(
          contentPadding: EdgeInsets.zero,
          title: const Text('Tone the section colours too'),
          subtitle: const Text(
              'Music pink, Books green and the rest lean toward this colour. '
              'Each keeps its own hue.'),
          value: _harmonise,
          onChanged: (v) => setState(() => _harmonise = v),
        ),
      ],
    );
  }

  Widget _previewColumn(ColorScheme s, bool dark) => Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          const _Label('Preview', top: 0),
          SegmentedButton<bool>(
            segments: const [
              ButtonSegment(value: false, label: Text('Light')),
              ButtonSegment(value: true, label: Text('Dark')),
            ],
            selected: {dark},
            onSelectionChanged: (v) => setState(() => _dark = v.first),
          ),
          const SizedBox(height: 12),
          _Preview(s: s, art: _music.state?.now.art ?? ''),
          const _Label('Colours'),
          Wrap(spacing: 6, runSpacing: 6, children: [
            for (final (name, bg, fg) in [
              ('Primary', s.primary, s.onPrimary),
              ('Primary container', s.primaryContainer, s.onPrimaryContainer),
              ('Secondary container', s.secondaryContainer, s.onSecondaryContainer),
              ('Tertiary container', s.tertiaryContainer, s.onTertiaryContainer),
              ('Surface', s.surfaceContainerHighest, s.onSurface),
            ])
              Tooltip(
                message: colorHex(bg).toUpperCase(),
                child: Container(
                  padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 7),
                  decoration: BoxDecoration(
                      color: bg, borderRadius: BorderRadius.circular(10)),
                  child: Text(name, style: TextStyle(fontSize: 11.5, color: fg)),
                ),
              ),
          ]),
        ],
      );
}

class _Label extends StatelessWidget {
  const _Label(this.text, {this.top = 18});

  final String text;
  final double top;

  @override
  Widget build(BuildContext context) => Padding(
        padding: EdgeInsets.only(top: top, bottom: 8),
        child: Text(text.toUpperCase(),
            style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.7,
                color: Theme.of(context).colorScheme.onSurfaceVariant)),
      );
}

/// Android's swatch: the seed's primary over its secondary and tertiary. A
/// circle, a rounded square once picked.
class _Swatch extends StatelessWidget {
  const _Swatch(this.seed, {required this.on, required this.onTap});

  final Color seed;
  final bool on;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final s = bloomScheme(seed, DynamicSchemeVariant.tonalSpot, dark: false);
    final r = on ? 14.0 : 26.0;
    return Tooltip(
      message: colorHex(seed).toUpperCase(),
      child: GestureDetector(
        onTap: onTap,
        child: MouseRegion(
          cursor: SystemMouseCursors.click,
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 200),
            width: 54,
            height: 54,
            padding: const EdgeInsets.all(3),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(r + 3),
              border: Border.all(
                  width: 2,
                  color: on
                      ? Theme.of(context).colorScheme.onSurface
                      : Colors.transparent),
            ),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(r),
              child: Column(children: [
                Expanded(child: ColoredBox(color: s.primary, child: const SizedBox.expand())),
                Expanded(
                  child: Row(children: [
                    Expanded(child: ColoredBox(color: s.secondaryContainer, child: const SizedBox.expand())),
                    Expanded(child: ColoredBox(color: s.tertiaryContainer, child: const SizedBox.expand())),
                  ]),
                ),
              ]),
            ),
          ),
        ),
      ),
    );
  }
}

/// A scheme style, with the three colours it makes of this seed.
class _StyleTile extends StatelessWidget {
  const _StyleTile({
    required this.seed,
    required this.style,
    required this.name,
    required this.hint,
    required this.on,
    required this.onTap,
  });

  final Color seed;
  final DynamicSchemeVariant style;
  final String name;
  final String hint;
  final bool on;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final cs = Theme.of(context).colorScheme;
    final x = bloomScheme(seed, style, dark: false);
    Widget bar(Color c, int flex) => Expanded(
          flex: flex,
          child: Container(
            height: 14,
            margin: const EdgeInsets.only(right: 3),
            decoration:
                BoxDecoration(color: c, borderRadius: BorderRadius.circular(5)),
          ),
        );
    return Material(
      color: on ? cs.secondaryContainer : cs.surfaceContainerHighest,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(16),
        side: BorderSide(width: 2, color: on ? cs.primary : Colors.transparent),
      ),
      child: InkWell(
        borderRadius: BorderRadius.circular(16),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(10, 9, 7, 9),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(children: [
                bar(x.primary, 2),
                bar(x.secondaryContainer, 1),
                bar(x.tertiaryContainer, 1),
              ]),
              const SizedBox(height: 6),
              Text(name,
                  style: TextStyle(
                      fontSize: 12.5,
                      fontWeight: FontWeight.w600,
                      color: on ? cs.onSecondaryContainer : cs.onSurface)),
              Text(hint,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 10.5, color: cs.onSurfaceVariant)),
            ],
          ),
        ),
      ),
    );
  }
}

/// A small player drawn in the scheme being picked.
class _Preview extends StatelessWidget {
  const _Preview({required this.s, required this.art});

  final ColorScheme s;
  final String art;

  @override
  Widget build(BuildContext context) {
    Widget chip(String t, Color bg, Color fg, {bool line = false}) => Container(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
          decoration: BoxDecoration(
            color: bg,
            borderRadius: BorderRadius.circular(9),
            border: line ? Border.all(color: s.outline) : null,
          ),
          child: Text(t, style: TextStyle(fontSize: 12, color: fg)),
        );
    Widget round(IconData i) => Container(
          width: 40,
          height: 40,
          decoration: BoxDecoration(
              color: s.surfaceContainerHighest, shape: BoxShape.circle),
          child: Icon(i, size: 18, color: s.onSurfaceVariant),
        );
    return AnimatedContainer(
      duration: const Duration(milliseconds: 250),
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: s.surface,
        borderRadius: BorderRadius.circular(24),
        border: Border.all(color: s.outlineVariant),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(children: [
            ClipRRect(
              borderRadius: BorderRadius.circular(14),
              child: SizedBox(
                width: 56,
                height: 56,
                child: art.isEmpty
                    ? ColoredBox(
                        color: s.primaryContainer,
                        child: Icon(Icons.album, color: s.onPrimaryContainer))
                    : Image.file(File(art), fit: BoxFit.cover),
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                Text('Song title',
                    style: TextStyle(
                        fontSize: 14, fontWeight: FontWeight.w600, color: s.onSurface)),
                Text('Artist · Album',
                    style: TextStyle(fontSize: 12, color: s.onSurfaceVariant)),
              ]),
            ),
          ]),
          const SizedBox(height: 14),
          ClipRRect(
            borderRadius: BorderRadius.circular(4),
            child: LinearProgressIndicator(
              value: 0.45,
              minHeight: 6,
              color: s.primary,
              backgroundColor: s.secondaryContainer,
            ),
          ),
          const SizedBox(height: 12),
          Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              round(Icons.skip_previous),
              const SizedBox(width: 14),
              Container(
                width: 62,
                height: 54,
                decoration: BoxDecoration(
                    color: s.primary, borderRadius: BorderRadius.circular(18)),
                child: Icon(Icons.pause, color: s.onPrimary),
              ),
              const SizedBox(width: 14),
              round(Icons.skip_next),
            ],
          ),
          const SizedBox(height: 12),
          Wrap(spacing: 6, runSpacing: 6, children: [
            chip('Liked', s.secondaryContainer, s.onSecondaryContainer),
            chip('Queue', Colors.transparent, s.onSurfaceVariant, line: true),
            chip('Lyrics', s.tertiaryContainer, s.onTertiaryContainer),
          ]),
          const SizedBox(height: 12),
          Align(
            alignment: Alignment.centerRight,
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 13),
              decoration: BoxDecoration(
                  color: s.tertiaryContainer, borderRadius: BorderRadius.circular(16)),
              child: Text('+ Playlist',
                  style: TextStyle(
                      fontWeight: FontWeight.w600, color: s.onTertiaryContainer)),
            ),
          ),
        ],
      ),
    );
  }
}
