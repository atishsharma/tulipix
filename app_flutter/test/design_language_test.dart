// Design languages: the stored names, the Standard no-op, and Neumorphism's
// answers on all three tiers.
//
// Pure values — nothing here touches the bridge. What it pins, in order of how
// quietly each would break:
//
//  - The spellings shared with the Slint build. `clay` and `skeuo` are what
//    `design_lang_index` in tulipix-app matches; a rename here would leave the
//    two builds disagreeing about the same settings file.
//  - Standard answering nothing. That is the whole guarantee that choosing it
//    changes no pixel of the section.
//  - OLED drawing edges with a rim rather than a cast, which is the one tier
//    where getting it wrong is invisible on a light screen.

import 'package:flutter/material.dart';
import 'package:flutter_tabler_icons/flutter_tabler_icons.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:tulipix/design/design_language.dart';
import 'package:tulipix/design/languages/neumorphism.dart';
import 'package:tulipix/design/skin.dart';
import 'package:tulipix/design/soft_decoration.dart';
import 'package:tulipix/design/tokens.dart';

void main() {
  group('DesignLanguage', () {
    test('the three it shares with Slint are spelled the way Slint spells them',
        () {
      expect(DesignLanguage.standard.id, 'standard');
      expect(DesignLanguage.claymorphism.id, 'clay');
      expect(DesignLanguage.skeuomorphism.id, 'skeuo');
    });

    test('every id round-trips', () {
      for (final l in DesignLanguage.values) {
        expect(DesignLanguage.fromId(l.id), l);
      }
    });

    test('empty, null and unknown names are Standard', () {
      expect(DesignLanguage.fromId(''), DesignLanguage.standard);
      expect(DesignLanguage.fromId(null), DesignLanguage.standard);
      expect(DesignLanguage.fromId('brutalism'), DesignLanguage.standard);
    });

    test('six options, Standard first', () {
      expect(DesignLanguage.values, hasLength(6));
      expect(DesignLanguage.values.first, DesignLanguage.standard);
    });
  });

  group('skins', () {
    final tiers = {
      'light': Tokens.light(),
      'dark': Tokens.dark(),
      'oled': Tokens.dark(oled: true),
    };

    test('Standard answers nothing, so the section draws what it always drew',
        () {
      const s = StandardSkin();
      expect(s.canvas, isNull);
      expect(s.fontFamily, isNull);
      expect(s.surface(SurfaceRole.card), isNull);
      expect(s.control(active: true), isNull);
      expect(s.icon(Icons.play_arrow), Icons.play_arrow);
    });

    test('a language without a skin yet draws Standard', () {
      for (final l in DesignLanguage.values.where((l) => !l.built)) {
        expect(skinFor(l, Tokens.light()).isStandard, isTrue, reason: l.id);
      }
    });

    for (final e in tiers.entries) {
      test('Neumorphism answers every question on ${e.key}', () {
        final s = skinFor(DesignLanguage.neumorphism, e.value);
        expect(s, isA<NeuSkin>());
        expect(s.canvas, isNotNull);
        expect(s.fontFamily, 'MPLUSRounded1c');
        for (final r in SurfaceRole.values) {
          expect(s.surface(r), isA<SoftDecoration>(), reason: '$r');
        }
        expect(s.control(active: true), isNot(equals(s.control(active: false))));
        expect(s.icon(Icons.play_arrow), TablerIcons.player_play);
      });
    }

    test('OLED keeps the page black and draws edges with a rim, not a cast', () {
      final s = NeuSkin(Tokens.dark(oled: true));
      expect(s.canvas, const Color(0xFF000000));
      final d = s.surface(SurfaceRole.card) as SoftDecoration;
      expect(d.outer, isEmpty);
      expect(d.inner, isNotEmpty);
    });
  });

  testWidgets('SoftDecoration paints raised and sunk shapes', (tester) async {
    final s = NeuSkin(Tokens.light());
    await tester.pumpWidget(
      Directionality(
        textDirection: TextDirection.ltr,
        child: Column(
          children: [
            Container(
                width: 80, height: 40, decoration: s.surface(SurfaceRole.card)),
            Container(
                width: 80, height: 40, decoration: s.surface(SurfaceRole.well)),
          ],
        ),
      ),
    );
    expect(tester.takeException(), isNull);
  });
}
