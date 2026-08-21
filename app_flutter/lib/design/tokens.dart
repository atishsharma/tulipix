// Design tokens — transcribed from ui/tokens.slint, literally.
//
// Every value here has a counterpart in that file and nothing here is an
// improvement on it. The temptation to fix the palette while transcribing is
// how a port silently becomes a redesign nobody signed off on. If a colour
// looks wrong, change tokens.slint on beta and re-transcribe.
//
// WCAG: extra-dark bg #050614 on text #f8faff = ~17.8:1 (AAA).

import 'package:flutter/material.dart';

/// The nine section identities. `Section` in tokens.slint.
enum Section {
  photos,
  videos,
  music,
  books,
  cloud,
  tools,
  transfer,
  finances,
  settings,
}

@immutable
class Tokens extends ThemeExtension<Tokens> {
  const Tokens({
    required this.dark,
    required this.oled,
    required this.bg,
    required this.atmosphere,
    required this.panel,
    required this.panel2,
    required this.modal,
    required this.text,
    required this.textDim,
    required this.textInv,
    required this.outline,
    required this.outlineStrong,
    required this.highlightInner,
    required this.glass,
    required this.glassStrong,
    required this.glassBorder,
    required this.nCanvas,
    required this.nCard,
    required this.nChip,
    required this.nTile,
    required this.nHover,
    required this.nHair,
    required this.nInk,
    required this.nInk2,
    required this.nInk3,
    required this.nAccentSoft,
    required this.reduceMotion,
  });

  final bool dark;
  final bool oled;

  // --- Surfaces ---
  final Color bg;
  final Color atmosphere;
  final Color panel;
  final Color panel2;
  final Color modal;

  // --- Text ---
  final Color text;
  final Color textDim;
  final Color textInv;

  // --- Outline / focus / state ---
  final Color outline;
  final Color outlineStrong;
  final Color highlightInner;

  // --- Frosted glass ---
  // Neutral translucent surfaces (sidebar rows, dock buttons, chips): no accent
  // tint, so they read cleanly on both themes.
  final Color glass;
  final Color glassStrong;
  final Color glassBorder;

  // --- Neutral surface ramp ---
  // Sections whose light identity is the Google-neutral white/grey palette
  // (Photos, Cloud explorer) follow the app light/dark via these. Light values
  // reproduce the original hardcoded Google-Photos greys exactly.
  final Color nCanvas;
  final Color nCard;
  final Color nChip;
  final Color nTile;
  final Color nHover;
  final Color nHair;
  final Color nInk;
  final Color nInk2;
  final Color nInk3;
  final Color nAccentSoft;

  final bool reduceMotion;

  // --- Constants (theme-independent in tokens.slint) ---
  static const Color focus = Color(0xFF7C3AED);
  static const Color brand = Color(0xFF7C3AED);
  static const Color brand2 = Color(0xFF4F46E5);

  static const Color secHome = Color(0xFF8B5CF6); // violet
  static const Color secPhotos = Color(0xFF8B5CF6); // violet
  static const Color secVideos = Color(0xFF6366F1); // indigo
  static const Color secMusic = Color(0xFFEC4899); // pink
  static const Color secBooks = Color(0xFF10B981); // emerald
  static const Color secCloud = Color(0xFF14B8A6); // teal
  static const Color secTools = Color(0xFF06B6D4); // cyan
  // Orange is the one hue the six above leave free, so a glance at the accent
  // is enough to say which section you are looking at.
  static const Color secTransfer = Color(0xFFF97316); // orange
  // Lime, not red: this section leans on `error` constantly for overdue and
  // over-budget, and an accent that reads as an alarm would make every screen
  // look like a problem.
  static const Color secFinances = Color(0xFF84CC16); // lime
  static const Color secStatus = Color(0xFF7C3AED); // violet
  static const Color secSettings = Color(0xFF94A3B8); // slate

  static const Color ok = Color(0xFF16A34A);
  static const Color warn = Color(0xFFF59E0B);
  static const Color error = Color(0xFFDC2626);

  static const double radiusSm = 8;
  static const double radiusMd = 14;
  static const double radiusLg = 24;
  static const double sidebarW = 248;

  // Frost depths (ui/frost.slint).
  static const double frostAtmosphere = 80;
  static const double frostPanel = 24;
  static const double frostModal = 40;

  static const String fontFamily = 'Sora';

  // Sora ships as a variable font (wght 100..900). `weightFor` interpolates
  // between these named stops — heavier on lift+hover, lighter on press for the
  // "depressed key" feel.
  static const int weightRest = 400;
  static const int weightHover = 500;
  static const int weightPress = 300;
  static const int weightLift = 600;

  static Color accentOf(Section s) => switch (s) {
        Section.photos => secPhotos,
        Section.videos => secVideos,
        Section.music => secMusic,
        Section.books => secBooks,
        Section.cloud => secCloud,
        Section.tools => secTools,
        Section.transfer => secTransfer,
        Section.finances => secFinances,
        Section.settings => secSettings,
      };

  /// `Weight.value` from tokens.slint.
  FontWeight weightFor({
    bool hovered = false,
    bool pressed = false,
    bool lifted = false,
  }) {
    final int w = reduceMotion
        ? weightRest
        : pressed
            ? weightPress
            : lifted && hovered
                ? weightLift
                : lifted || hovered
                    ? weightHover
                    : weightRest;
    return FontWeight.values[(w ~/ 100) - 1];
  }

  factory Tokens.light({bool reduceMotion = false}) => Tokens(
        dark: false,
        oled: false,
        bg: const Color(0xFFEDE9F8),
        atmosphere: const Color(0xFFF6F3FD),
        panel: const Color(0xD0FFFFFF),
        panel2: const Color(0xFFFBFAFF),
        modal: const Color(0xE6FFFFFF),
        text: const Color(0xFF1A1040),
        textDim: const Color(0xC71A1040), // rgba(26,16,64,0.78)
        textInv: const Color(0xFFFFFFFF),
        outline: const Color(0x1A1A1040), // rgba(26,16,64,0.10)
        outlineStrong: const Color(0x331A1040), // 0.20
        highlightInner: const Color(0xD9FFFFFF), // rgba(255,255,255,0.85)
        glass: const Color(0x0D1A1040), // 0.05
        glassStrong: const Color(0x1A1A1040), // 0.10
        glassBorder: const Color(0x1F1A1040), // 0.12
        nCanvas: const Color(0xFFFFFFFF),
        nCard: const Color(0xFFF8F9FA),
        nChip: const Color(0xFFF1F3F4),
        nTile: const Color(0xFFE8EAED),
        nHover: const Color(0xFFF1F3F4),
        nHair: const Color(0xFFE8EAED),
        nInk: const Color(0xFF202124),
        nInk2: const Color(0xFF5F6368),
        nInk3: const Color(0xFF3C4043),
        nAccentSoft: const Color(0xFFE8F0FE),
        reduceMotion: reduceMotion,
      );

  /// `oled` collapses every dark surface to pure black so true blacks switch
  /// off OLED pixels. App-wide, not a Photos-only tier.
  factory Tokens.dark({bool oled = false, bool reduceMotion = false}) => Tokens(
        dark: true,
        oled: oled,
        bg: oled ? const Color(0xFF000000) : const Color(0xFF050614),
        atmosphere: oled ? const Color(0xFF000000) : const Color(0xFF0D0F1A),
        panel: oled ? const Color(0xFF0A0A0A) : const Color(0xFF161827),
        panel2: oled ? const Color(0xFF0D0D0D) : const Color(0xFF1D2033),
        modal: oled ? const Color(0xFF0A0A0A) : const Color(0xFF1A1C2E),
        text: const Color(0xFFF8FAFF),
        textDim: const Color(0xA8F8FAFF), // rgba(248,250,255,0.66)
        textInv: const Color(0xFF050614),
        outline: const Color(0x1AFFFFFF), // 0.10
        outlineStrong: const Color(0x2EFFFFFF), // 0.18
        highlightInner: const Color(0x0FFFFFFF), // 0.06
        glass: const Color(0x12FFFFFF), // 0.07
        glassStrong: const Color(0x24FFFFFF), // 0.14
        glassBorder: const Color(0x29FFFFFF), // 0.16
        nCanvas: oled ? const Color(0xFF000000) : const Color(0xFF141518),
        nCard: oled ? const Color(0xFF0A0A0A) : const Color(0xFF1B1D22),
        nChip: oled ? const Color(0xFF131316) : const Color(0xFF26282E),
        nTile: oled ? const Color(0xFF161616) : const Color(0xFF2A2D33),
        nHover: oled ? const Color(0xFF1A1A1A) : const Color(0xFF2F3239),
        nHair: oled ? const Color(0xFF1F1F1F) : const Color(0xFF33363D),
        nInk: const Color(0xFFE8EAED),
        nInk2: const Color(0xFF9AA0A6),
        nInk3: const Color(0xFFBDC1C6),
        nAccentSoft: const Color(0xFF1A2740),
        reduceMotion: reduceMotion,
      );

  @override
  Tokens copyWith({bool? dark, bool? oled, bool? reduceMotion}) {
    // The palette is derived from these three switches, never patched colour by
    // colour — a half-swapped theme is the bug this shape prevents.
    final bool d = dark ?? this.dark;
    final bool rm = reduceMotion ?? this.reduceMotion;
    return d
        ? Tokens.dark(oled: oled ?? this.oled, reduceMotion: rm)
        : Tokens.light(reduceMotion: rm);
  }

  @override
  Tokens lerp(ThemeExtension<Tokens>? other, double t) {
    // Themes swap, they do not cross-fade: an interpolated OLED tier is not a
    // tier the design system defines.
    if (other is! Tokens) return this;
    return t < 0.5 ? this : other;
  }
}

extension TokensOf on BuildContext {
  Tokens get tokens => Theme.of(this).extension<Tokens>()!;
}

/// Material theme carrying the tokens. Widgets read `context.tokens`; this only
/// keeps stock Material widgets (dialogs, scrollbars, text fields) in step.
ThemeData tulipixTheme(Tokens t) {
  final base = t.dark ? ThemeData.dark() : ThemeData.light();
  return base.copyWith(
    scaffoldBackgroundColor: t.bg,
    canvasColor: t.panel,
    dividerColor: t.outline,
    extensions: [t],
    colorScheme: base.colorScheme.copyWith(
      primary: Tokens.brand,
      secondary: Tokens.brand2,
      surface: t.panel,
      error: Tokens.error,
    ),
    textTheme: base.textTheme.apply(
      fontFamily: Tokens.fontFamily,
      bodyColor: t.text,
      displayColor: t.text,
    ),
  );
}
