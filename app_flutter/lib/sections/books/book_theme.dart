// The Books palette — transcribed from the `BookTheme` global in
// ui/page_books.slint, literally.
//
// Books is its own bright surface: it follows the app's light/dark/OLED toggle
// but not the shell's section accent. The accent here is the violet the Slint
// page pins (#6c4df6), not `Tokens.secBooks` — using the section colour was
// how the port quietly became a different design. Genesis imports this same
// palette, exactly as page_genesis.slint imports BookTheme from page_books.
//
// The clay/skeuo skins of `Surface` have no counterpart in the Flutter port,
// so the plain branch of each binding is the one transcribed.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';

@immutable
class BookTheme {
  const BookTheme({required this.dark, required this.oled});

  factory BookTheme.of(BuildContext context) {
    final t = context.tokens;
    return BookTheme(dark: t.dark, oled: t.dark && t.oled);
  }

  final bool dark;
  final bool oled;

  static const Color accent = Color(0xFF6C4DF6);

  // The five chip hues the page rotates through, and the section's own reds.
  static const Color violet = Color(0xFF6C4DF6);
  static const Color pink = Color(0xFFE0518F);
  static const Color green = Color(0xFF2FBF71);
  static const Color amber = Color(0xFFF5A623);
  static const Color blue = Color(0xFF3A86FF);
  static const Color magenta = Color(0xFFB5179E);
  static const Color teal = Color(0xFF14B8A6);
  static const Color danger = Color(0xFFD64545);
  static const Color genesisRed = Color(0xFFEF4444);

  /// The rotating palette the rail's genre chips cycle.
  static const List<Color> genreHues = [
    violet,
    pink,
    green,
    amber,
    blue,
    magenta,
  ];

  static const double radius = 18;

  Color get canvas => dark
      ? (oled ? const Color(0xFF000000) : const Color(0xFF14151A))
      : const Color(0xFFF2F2F7);

  Color get card => dark
      ? (oled ? const Color(0xFF0A0A0A) : const Color(0xFF1E1F26))
      : const Color(0xFFFFFFFF);

  Color get tintViolet => dark
      ? (oled ? const Color(0xFF191331) : const Color(0xFF241D3D))
      : const Color(0xFFEFEAFF);

  Color get tintPink => dark
      ? (oled ? const Color(0xFF22151D) : const Color(0xFF2E1F28))
      : const Color(0xFFFDEEF4);

  Color get tintOrange => dark
      ? (oled ? const Color(0xFF221B10) : const Color(0xFF2E2519))
      : const Color(0xFFFFF3E6);

  Color get tintGreen => dark
      ? (oled ? const Color(0xFF0F1D16) : const Color(0xFF17281F))
      : const Color(0xFFE8F8EF);

  Color get tintBlue => dark
      ? (oled ? const Color(0xFF101A2B) : const Color(0xFF182338))
      : const Color(0xFFE7F0FF);

  Color get ink => dark ? const Color(0xFFE8E8EE) : const Color(0xFF1B1B22);
  Color get inkDim => dark ? const Color(0xFF8F8F9A) : const Color(0xFF8A8A94);

  /// A groove: something set *into* the card.
  Color get track => dark
      ? (oled ? const Color(0xFF161616) : const Color(0xFF2A2B33))
      : const Color(0xFFECECF2);

  Color get pillBg => dark
      ? (oled ? const Color(0xFF131316) : const Color(0xFF24252D))
      : const Color(0xFFF4F4F8);

  Color get hairline => dark
      ? (oled ? const Color(0xFF1F1F1F) : const Color(0xFF2C2D36))
      : const Color(0xFFECECF2);

  /// Dark themes get a LIGHT shadow — a soft glow, because dark-on-dark is
  /// invisible. Light themes keep the classic drop.
  Color get shadowSoft =>
      dark ? const Color(0x22FFFFFF) : const Color(0x141B1B3A);

  /// The soft diffuse card shadow every surface on the page carries.
  List<BoxShadow> get cardShadow => [
        BoxShadow(color: shadowSoft, blurRadius: 16, offset: const Offset(0, 6)),
      ];
}

extension BookThemeContext on BuildContext {
  BookTheme get book => BookTheme.of(this);
}
