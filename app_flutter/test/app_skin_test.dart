// The design language across the app: the tokens a language hands every
// section, and the theme it hands the stock widgets.
//
// What it pins, in order of how quietly each would break:
//
//  - Standard untouched: the very Tokens instance it was given and the theme
//    main.dart has always built, so choosing it changes no pixel anywhere.
//  - Contrast, every language, every tier. A palette that reads badly on one
//    tier is invisible from the tier you happen to be looking at.
//  - A retint surviving the theme cross-fade.

import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:tulipix/design/app_theme.dart';
import 'package:tulipix/design/design_language.dart';
import 'package:tulipix/design/skin.dart';
import 'package:tulipix/design/tokens.dart';

double _luminance(Color c) {
  double ch(double v) => v <= 0.03928
      ? v / 12.92
      : math.pow((v + 0.055) / 1.055, 2.4).toDouble();
  return 0.2126 * ch(c.r) + 0.7152 * ch(c.g) + 0.0722 * ch(c.b);
}

double _contrast(Color a, Color b) {
  final x = _luminance(a), y = _luminance(b);
  return (math.max(x, y) + 0.05) / (math.min(x, y) + 0.05);
}

void main() {
  final tiers = {
    'light': Tokens.light(),
    'dark': Tokens.dark(),
    'oled': Tokens.dark(oled: true),
  };
  final languages =
      DesignLanguage.values.where((l) => l != DesignLanguage.standard);

  test('Standard hands back the very tokens it was given', () {
    for (final base in tiers.values) {
      expect(const StandardSkin().retint(base), same(base));
    }
  });

  test("Standard's theme is the theme main.dart has always built", () {
    for (final t in tiers.values) {
      const s = StandardSkin();
      expect(appTheme(t, s), tulipixTheme(t).copyWith(extensions: [t, s]));
    }
  });

  for (final l in languages) {
    for (final e in tiers.entries) {
      final base = e.value;
      final skin = skinFor(l, base);
      final t = skin.retint(base);

      test('${l.id} on ${e.key} has its own ink, panels and hairlines', () {
        expect(t.text, isNot(base.text));
        expect(t.panel, isNot(base.panel));
        expect(t.outline, isNot(base.outline));
        expect(t.nAccentSoft, base.nAccentSoft);
        expect([t.dark, t.oled, t.reduceMotion],
            [base.dark, base.oled, base.reduceMotion]);
        // OLED pages stay off in every language, as they do in Standard.
        if (e.key == 'oled') expect(t.bg, const Color(0xFF000000));
      });

      test('${l.id} on ${e.key} reads', () {
        final page = t.bg;
        for (final surface in [
          page,
          Color.alphaBlend(t.panel, page),
          Color.alphaBlend(t.panel2, page),
          Color.alphaBlend(t.modal, page),
        ]) {
          expect(_contrast(t.text, surface), greaterThanOrEqualTo(4.5),
              reason: 'text on $surface');
          expect(_contrast(t.textDim, surface), greaterThanOrEqualTo(3),
              reason: 'dim text on $surface');
        }
      });

      test('${l.id} on ${e.key} gives the stock widgets its face', () {
        final theme = appTheme(t, skin);
        expect(theme.textTheme.bodyMedium!.fontFamily, skin.fontFamily);
        expect(theme.dialogTheme.backgroundColor, t.modal);
        expect(theme.extension<Tokens>(), same(t));
        expect(theme.extension<AppSkin>(), same(skin));
      });
    }
  }

  test('every language speaks the shell in its own glyphs', () {
    const shellGlyphs = [
      Icons.home_outlined, Icons.image_outlined, Icons.movie_outlined,
      Icons.music_note_outlined, Icons.menu_book_outlined,
      Icons.cloud_outlined, Icons.build_outlined, Icons.share_outlined,
      Icons.account_balance_wallet_outlined, Icons.settings_outlined,
      Icons.person_outline, Icons.chevron_left, Icons.chevron_right,
      Icons.light_mode, Icons.dark_mode, Icons.star_outline, Icons.remove,
      Icons.crop_square, Icons.filter_none, Icons.close, Icons.music_note,
      Icons.fullscreen_exit, Icons.auto_awesome,
      Icons.picture_in_picture_alt,
    ];
    for (final l in languages) {
      final s = skinFor(l, Tokens.light());
      for (final g in shellGlyphs) {
        // Phosphor Fill has no picture-in-picture; Clay keeps Material's.
        if (l == DesignLanguage.claymorphism &&
            g == Icons.picture_in_picture_alt) {
          continue;
        }
        expect(s.icon(g), isNot(g), reason: '${l.id} ${g.codePoint}');
      }
    }
  });

  test('a skin, and a cached theme, are built once per language and tier', () {
    // Zen asks for its theme on every music update; a fresh one each time
    // rebuilt every widget on the page that reads it.
    for (final l in DesignLanguage.values) {
      for (final e in tiers.entries) {
        final fresh = switch (e.key) {
          'light' => Tokens.light(),
          'dark' => Tokens.dark(),
          _ => Tokens.dark(oled: true),
        };
        expect(skinFor(l, fresh), same(skinFor(l, e.value)), reason: l.id);
        expect(themeFor(l, fresh), same(themeFor(l, e.value)), reason: l.id);
      }
    }
    // A tier is still its own skin.
    expect(skinFor(DesignLanguage.expressive, Tokens.light()),
        isNot(same(skinFor(DesignLanguage.expressive, Tokens.dark()))));
  });

  test('a retint survives the theme cross-fade', () {
    final base = Tokens.light();
    final t = skinFor(DesignLanguage.claymorphism, base).retint(base);
    expect(t.lerp(Tokens.dark(), 0.3), same(t));
    expect(Tokens.dark().lerp(t, 0.7), same(t));
  });
}
