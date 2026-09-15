// The Material theme for a design language.
//
// Standard is today's theme exactly: `tulipixTheme`, untouched, with the skin
// riding beside the tokens. Every other language restyles the stock widgets
// the sections use — buttons, fields, sliders, switches, chips, cards,
// dialogs, menus, tooltips — in its own face, colours and corners.
//
// A theme cannot draw an inner shadow or a backdrop blur. Neumorphism's stock
// buttons are flat here and Glass's dialogs are strong glass without the blur;
// the hand-tuning phase closes those gaps where a section needs it.

import 'package:flutter/material.dart';

import 'design_language.dart';
import 'skin.dart';
import 'tokens.dart';

/// [appTheme] for [language] over [base], built once per language and tier.
///
/// For a theme asked for on every rebuild — zen's, which rebuilds on every
/// music update. A ThemeData built fresh each time is never `==` the last
/// (`Tokens` and `AppSkin` carry no value equality), so `Theme` told every
/// widget on the page that reads it to rebuild too. The same object tells
/// nobody anything.
ThemeData themeFor(DesignLanguage language, Tokens base) =>
    _themes[(language, base.dark, base.oled, base.reduceMotion)] ??= () {
      final skin = skinFor(language, base);
      return appTheme(skin.retint(base), skin);
    }();

final _themes = <(DesignLanguage, bool, bool, bool), ThemeData>{};

ThemeData appTheme(Tokens t, AppSkin skin) {
  final base = tulipixTheme(t).copyWith(extensions: [t, skin]);
  if (skin.isStandard) return base;

  final face = skin.fontFamily;
  final control = BorderRadius.circular(skin.controlRadius ?? 12);
  final panel = BorderRadius.circular(skin.panelRadius ?? 16);
  final shape = RoundedRectangleBorder(borderRadius: control);
  // Solid versions of the two surfaces: Glass's are translucent, and a stock
  // widget filled with 8% white over whatever scrolls under it is unreadable.
  final card = Color.alphaBlend(t.panel, t.bg);
  final raised = Color.alphaBlend(t.panel2, t.bg);
  final label = TextStyle(fontFamily: face, fontWeight: FontWeight.w600);

  return base.copyWith(
    colorScheme: base.colorScheme.copyWith(
      surface: card,
      surfaceContainer: raised,
      onSurface: t.text,
      onSurfaceVariant: t.textDim,
      outline: t.outlineStrong,
      outlineVariant: t.outline,
    ),
    textTheme: base.textTheme.apply(
      fontFamily: face,
      bodyColor: t.text,
      displayColor: t.text,
    ),
    iconTheme: base.iconTheme.copyWith(color: t.text),
    // Filled buttons keep `primary` — Tulipix violet, or the section colour
    // a call site passes. The language supplies the shape, never the hue.
    filledButtonTheme: FilledButtonThemeData(
      style: FilledButton.styleFrom(shape: shape, textStyle: label),
    ),
    elevatedButtonTheme: ElevatedButtonThemeData(
      style: ElevatedButton.styleFrom(
        backgroundColor: raised,
        foregroundColor: t.text,
        elevation: 0,
        shape: shape,
        textStyle: label,
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: OutlinedButton.styleFrom(
        foregroundColor: t.text,
        side: BorderSide(color: t.outlineStrong),
        shape: shape,
        textStyle: label,
      ),
    ),
    textButtonTheme: TextButtonThemeData(
      style: TextButton.styleFrom(shape: shape, textStyle: label),
    ),
    // Corners only, not a fill and not per-state borders. A theme fill, or an
    // enabled and a focused border, reaches every field, the borderless ones
    // sunk inside a search pill too, and drew a box inside the pill. Now a
    // field that says `border: InputBorder.none` gets none, and one that says
    // nothing gets this outline, in the scheme's outline colour at rest and
    // its primary when focused.
    inputDecorationTheme: InputDecorationTheme(
      hintStyle: TextStyle(fontFamily: face, color: t.textDim),
      border: OutlineInputBorder(borderRadius: control),
    ),
    sliderTheme: base.sliderTheme.copyWith(inactiveTrackColor: t.glassStrong),
    switchTheme: SwitchThemeData(
      trackOutlineColor: WidgetStatePropertyAll(t.outline),
    ),
    checkboxTheme: CheckboxThemeData(
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(4)),
      side: BorderSide(color: t.outlineStrong, width: 1.5),
    ),
    chipTheme: ChipThemeData(
      backgroundColor: raised,
      side: BorderSide(color: t.outline),
      shape: shape,
      labelStyle: TextStyle(fontFamily: face, color: t.text),
    ),
    cardTheme: CardThemeData(
      color: card,
      elevation: 0,
      shape: RoundedRectangleBorder(
        borderRadius: panel,
        side: BorderSide(color: t.outline),
      ),
    ),
    // `modalSolid` / `panelSolid`, not the raw tokens: those are 0xE6 and 0xD0
    // on the light palette, and this block overrides the opaque ones
    // `tulipixTheme` sets — so a language got see-through dialogs back.
    dialogTheme: DialogThemeData(
      backgroundColor: t.modalSolid,
      surfaceTintColor: Colors.transparent,
      shape: RoundedRectangleBorder(borderRadius: panel),
    ),
    popupMenuTheme: PopupMenuThemeData(
      color: t.panelSolid,
      surfaceTintColor: Colors.transparent,
      shape: shape,
      textStyle: TextStyle(fontFamily: face, color: t.text),
    ),
    menuTheme: MenuThemeData(
      style: MenuStyle(
        backgroundColor: WidgetStatePropertyAll(t.modal),
        shape: WidgetStatePropertyAll(shape),
      ),
    ),
    tooltipTheme: TooltipThemeData(
      decoration: BoxDecoration(color: t.text, borderRadius: control),
      textStyle: TextStyle(fontFamily: face, color: t.textInv),
    ),
    scrollbarTheme: ScrollbarThemeData(
      thumbColor: WidgetStatePropertyAll(t.textDim.withValues(alpha: 0.4)),
    ),
    progressIndicatorTheme:
        ProgressIndicatorThemeData(linearTrackColor: t.glassStrong),
  );
}
