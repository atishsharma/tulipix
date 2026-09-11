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
import 'package:lucide_icons_flutter/lucide_icons.dart';
import 'package:material_symbols_icons/symbols.dart';

import 'package:tulipix/design/design_language.dart';
import 'package:tulipix/design/languages/claymorphism.dart';
import 'package:tulipix/design/languages/expressive.dart';
import 'package:tulipix/design/languages/glassmorphism.dart';
import 'package:tulipix/design/languages/neumorphism.dart';
import 'package:tulipix/design/languages/skeuomorphism.dart';
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

    test('every language but Standard has a skin of its own', () {
      for (final l in DesignLanguage.values) {
        expect(skinFor(l, Tokens.light()).language, l, reason: l.id);
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

    for (final e in tiers.entries) {
      test('Clay answers every question on ${e.key}', () {
        final s = skinFor(DesignLanguage.claymorphism, e.value);
        expect(s, isA<ClaySkin>());
        expect(s.fontFamily, 'Fredoka');
        expect(s.pressScale, lessThan(1));
        for (final r in SurfaceRole.values) {
          expect(s.surface(r), isA<SoftDecoration>(), reason: '$r');
        }
        // Two tabs with two tints are two clays.
        final pink = s.control(active: true, tint: const Color(0xFFEC4899))!
            as SoftDecoration;
        final teal = s.control(active: true, tint: const Color(0xFF14B8A6))!
            as SoftDecoration;
        expect(pink.color, isNot(teal.color));
        expect(s.icon(Icons.play_arrow).fontFamily, 'PhosphorFill');
      });
    }

    for (final e in tiers.entries) {
      test('Glass answers every question on ${e.key}', () {
        final s = skinFor(DesignLanguage.glassmorphism, e.value);
        expect(s, isA<GlassSkin>());
        expect(s.fontFamily, 'Lexend');
        for (final r in SurfaceRole.values) {
          expect(s.surface(r), isNotNull, reason: '$r');
        }
        // A chip at rest is bare glass, but still the skin's, never Standard.
        expect(s.control(active: false), isNotNull);
        expect(s.icon(Icons.play_arrow), LucideIcons.play);
        expect(s.pageBackdrop(accent: const Color(0xFFEC4899)), isNotNull);
      });

      test('Unibody answers every question on ${e.key}', () {
        final s = skinFor(DesignLanguage.skeuomorphism, e.value);
        expect(s, isA<UnibodySkin>());
        expect(s.fontFamily, 'Geist');
        expect(s.numberFamily, 'Doto');
        expect(s.iconFill, 1);
        // Screens are black on every tier.
        final well = s.surface(SurfaceRole.well)! as SoftDecoration;
        expect(well.color.toARGB32() & 0xFFFFFF, lessThan(0x111111));
        expect(s.icon(Icons.play_arrow), Symbols.play_arrow_rounded);
      });

      test('Expressive answers every question on ${e.key}', () {
        final s = skinFor(DesignLanguage.expressive, e.value);
        expect(s, isA<ExpressiveSkin>());
        expect(s.fontFamily, 'GoogleSansFlex');
        // Shape carries state: a playing play button is not the paused one.
        expect(
          s.control(active: true, prominent: true, radius: 42),
          isNot(equals(s.control(active: false, prominent: true, radius: 42))),
        );
        for (final r in SurfaceRole.values) {
          expect((s.surface(r)! as BoxDecoration).boxShadow, isNull,
              reason: '$r has a shadow');
        }
      });

      test('a play button filled with the accent has its own ink on ${e.key}',
          () {
        for (final l in [
          DesignLanguage.claymorphism,
          DesignLanguage.skeuomorphism,
          DesignLanguage.expressive,
        ]) {
          expect(skinFor(l, e.value).onProminent, isNotNull, reason: l.id);
        }
      });
    }

    test('Expressive on OLED keeps the roles and takes the page to black', () {
      expect(ExpressiveSkin(Tokens.dark(oled: true)).canvas,
          const Color(0xFF000000));
    });

    test('Neumorphism keeps its play disc raised while playing', () {
      final s = NeuSkin(Tokens.light());
      expect(s.control(active: true, prominent: true),
          s.control(active: false, prominent: true));
    });

    test('Neumorphism ignores a tint and does not squash', () {
      final s = NeuSkin(Tokens.light());
      expect(s.control(active: true, tint: const Color(0xFFEC4899)),
          s.control(active: true));
      expect(s.pressScale, 1);
    });

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

  testWidgets('a translucent pane keeps its cast outside its face',
      (tester) async {
    const d = SoftDecoration(
      color: Color(0x14FFFFFF),
      radius: 12,
      outer: [SoftShadow(Color(0xFF000000), Offset(0, 10), 30)],
    );
    await tester.pumpWidget(const Directionality(
      textDirection: TextDirection.ltr,
      child: Center(
        child: SizedBox(
            width: 100, height: 60, child: DecoratedBox(decoration: d)),
      ),
    ));
    // Clipped out of the shape before it is drawn, as CSS draws a box-shadow.
    expect(tester.renderObject(find.byType(DecoratedBox)),
        paints..clipPath()..rrect(color: const Color(0xFF000000)));
  });

  testWidgets('the jog wheel plays, and steps both ways', (tester) async {
    var played = 0, prev = 0, next = 0;
    final s = UnibodySkin(Tokens.light());
    await tester.pumpWidget(MaterialApp(
      theme: ThemeData(extensions: [Tokens.light(), s]),
      home: Scaffold(
        body: Center(
          child: JogWheel(
            skin: s,
            playing: false,
            onPlay: () => played++,
            onPrev: () => prev++,
            onNext: () => next++,
          ),
        ),
      ),
    ));
    await tester.tap(find.byTooltip('Play'));
    await tester.tap(find.byTooltip('Previous'));
    await tester.tap(find.byTooltip('Next'));
    expect([played, prev, next], [1, 1, 1]);
  });

  testWidgets('the wave lies flat when paused', (tester) async {
    await tester.pumpWidget(MaterialApp(
      home: Scaffold(
        body: WavySeek(
          slot: SeekSlot(pos: 30, dur: 100, playing: false, onSeek: (_) {}),
          color: const Color(0xFFA3175E),
          track: const Color(0xFFFFD9E4),
        ),
      ),
    ));
    await tester.pumpAndSettle();
    final paint = tester.widget<CustomPaint>(find.byKey(WavySeek.paintKey));
    expect((paint.painter! as WavePainter).amplitude, 0);
  });
}
