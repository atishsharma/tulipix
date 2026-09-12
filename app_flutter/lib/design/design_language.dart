// Design languages — which material the Music section is drawn in.
//
// Stored as `ui.design-language`, the key the Slint build already reads.
// `standard` means the same thing in both builds and the rest are Flutter's
// own: Slint maps any name it does not know to Standard, so picking one here
// leaves the Slint build as it was. The other way round too — Slint's `clay`
// and `skeuo` are Standard here, since Flutter dropped both.
//
// The language is orthogonal to the theme. One picks the material, the other
// the lighting, and every language carries Light, Dark and OLED values.

enum DesignLanguage {
  standard(
    'standard',
    'Standard',
    'Tulipix',
    'The look the app ships with: frosted glass and gradient chips.',
  ),
  neumorphism(
    'neumorphism',
    'Neumorphism',
    'Soft Console',
    'Every control pressed out of one soft sheet.',
  ),
  glassmorphism(
    'glass',
    'Glassmorphism',
    'Lightbox',
    'Frosted panels lit by the colours of the cover that is playing.',
  ),
  expressive(
    'expressive',
    'Material 3 Expressive',
    'Bloom',
    'Tonal colour from the cover, and shapes that morph with state.',
  );

  const DesignLanguage(this.id, this.label, this.name, this.blurb);

  /// What `ui.design-language` holds.
  final String id;

  /// The style's common name.
  final String label;

  /// The mockup's name for this app's take on it.
  final String name;
  final String blurb;

  /// Anything empty or unknown is Standard, as it is in Slint: a first run, a
  /// hand-edited file and a name from a newer build all land on the look the
  /// app already had.
  static DesignLanguage fromId(String? id) =>
      values.firstWhere((l) => l.id == id, orElse: () => standard);
}
