// Bloom colours — the seed Material 3 Expressive grows its scheme from.
//
// Settings › You & Home › Design language › Colours picks it
// (lib/sections/settings/bloom_dialog.dart; the mockup is
// docs/mockups/NewSections/settings-bloom-colours-deck.html). main.dart works
// out the seed from the shell snapshot and the cover playing and sets [bloom]
// before it builds the theme; ExpressiveSkin reads it.

import 'package:flutter/material.dart';

import 'tokens.dart';

/// A seed, a scheme style and a contrast level. A record, so two with the
/// same values are equal and an unchanged snapshot does not rebuild the theme.
typedef Bloom = ({Color seed, DynamicSchemeVariant style, double contrast});

/// What the app is drawn in now.
Bloom bloom = (
  seed: Tokens.secMusic,
  style: DynamicSchemeVariant.tonalSpot,
  contrast: 0.0,
);

/// A stored style name; anything unknown is Tonal spot.
DynamicSchemeVariant bloomStyle(String name) => DynamicSchemeVariant.values
    .firstWhere((v) => v.name == name, orElse: () => DynamicSchemeVariant.tonalSpot);

/// `#rrggbb` as a colour, or null.
Color? hexColor(String hex) {
  if (hex.length != 7 || !hex.startsWith('#')) return null;
  final v = int.tryParse(hex.substring(1), radix: 16);
  return v == null ? null : Color(0xFF000000 | v);
}

/// A colour as `#rrggbb`.
String colorHex(Color c) =>
    '#${(c.toARGB32() & 0xFFFFFF).toRadixString(16).padLeft(6, '0')}';

/// The scheme [seed] grows into, as ExpressiveSkin builds it.
ColorScheme bloomScheme(
  Color seed,
  DynamicSchemeVariant style, {
  required bool dark,
  double contrast = 0,
}) =>
    ColorScheme.fromSeed(
      seedColor: seed,
      brightness: dark ? Brightness.dark : Brightness.light,
      dynamicSchemeVariant: style,
      contrastLevel: contrast,
    );
