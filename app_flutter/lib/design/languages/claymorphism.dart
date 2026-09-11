// Claymorphism — "Clay Studio".
//
// Every control is a small moulded object: a soft drop below it, light pressed
// into its top-left, shade into its bottom-right. Each library tab keeps its
// own clay, so the row reads as ten places rather than one selected thing and
// nine grey ones, and a pressed control squashes rather than sinks.
//
// The values are the mockup's (docs/mockups/music-claymorphism.html), one set
// per tier. On OLED the drop becomes a faint violet glow, so the clay still
// floats on black.

import 'package:flutter/material.dart';

import '../design_language.dart';
import '../skin.dart';
import '../soft_decoration.dart';
import '../tokens.dart';

/// Phosphor's Fill glyphs, from its own font (fonts/PhosphorFill.ttf, MIT).
/// Not phosphor_flutter: its IconData subclass stopped compiling when IconData
/// became a final class, and 2.1.0 is its last release.
abstract final class _Ph {
  static const _f = 'PhosphorFill';
  static const play = IconData(0xe3d0, fontFamily: _f);
  static const pause = IconData(0xe39e, fontFamily: _f);
  static const skipBack = IconData(0xe5a4, fontFamily: _f);
  static const skipForward = IconData(0xe5a6, fontFamily: _f);
  static const stop = IconData(0xe46c, fontFamily: _f);
  static const shuffle = IconData(0xe422, fontFamily: _f);
  static const repeat = IconData(0xe3f6, fontFamily: _f);
  static const repeatOnce = IconData(0xe3f8, fontFamily: _f);
  static const heart = IconData(0xe2a8, fontFamily: _f);
  static const star = IconData(0xe46a, fontFamily: _f);
  static const equalizer = IconData(0xebbc, fontFamily: _f);
  static const slidersHorizontal = IconData(0xe434, fontFamily: _f);
  static const queue = IconData(0xe6ac, fontFamily: _f);
  static const microphoneStage = IconData(0xe75c, fontFamily: _f);
  static const playlist = IconData(0xe6aa, fontFamily: _f);
  static const moon = IconData(0xe330, fontFamily: _f);
  static const screencast = IconData(0xe404, fontFamily: _f);
  static const speakerSimpleHigh = IconData(0xe450, fontFamily: _f);
  static const gear = IconData(0xe270, fontFamily: _f);
  static const arrowsOut = IconData(0xe0a2, fontFamily: _f);
  static const speakerHigh = IconData(0xe44a, fontFamily: _f);
  static const speakerLow = IconData(0xe44c, fontFamily: _f);
  static const speakerNone = IconData(0xe44e, fontFamily: _f);
  static const speakerX = IconData(0xe45c, fontFamily: _f);
  static const vinylRecord = IconData(0xecac, fontFamily: _f);
  static const microphone = IconData(0xe326, fontFamily: _f);
  static const bookOpen = IconData(0xe0e6, fontFamily: _f);
  static const radio = IconData(0xe77e, fontFamily: _f);
  static const house = IconData(0xe2c2, fontFamily: _f);
  static const musicNote = IconData(0xe33c, fontFamily: _f);
  static const disc = IconData(0xe564, fontFamily: _f);
  static const user = IconData(0xe4c2, fontFamily: _f);
  static const folder = IconData(0xe24a, fontFamily: _f);
  static const clockCounterClockwise = IconData(0xe1a0, fontFamily: _f);
  static const downloadSimple = IconData(0xe20c, fontFamily: _f);
  static const magnifyingGlass = IconData(0xe30c, fontFamily: _f);
  static const x = IconData(0xe4f6, fontFamily: _f);
  static const caretLeft = IconData(0xe138, fontFamily: _f);
  static const caretRight = IconData(0xe13a, fontFamily: _f);
  static const caretDown = IconData(0xe136, fontFamily: _f);
  static const caretUp = IconData(0xe13c, fontFamily: _f);
  static const sortAscending = IconData(0xe444, fontFamily: _f);
  static const arrowUp = IconData(0xe08e, fontFamily: _f);
  static const arrowDown = IconData(0xe03e, fontFamily: _f);
  static const arrowClockwise = IconData(0xe036, fontFamily: _f);
  static const tag = IconData(0xe478, fontFamily: _f);
  static const trash = IconData(0xe4a6, fontFamily: _f);
  static const tray = IconData(0xe4aa, fontFamily: _f);
  static const copy = IconData(0xe1ca, fontFamily: _f);
  // The shell.
  static const image = IconData(0xe2ca, fontFamily: _f);
  static const filmStrip = IconData(0xe792, fontFamily: _f);
  static const cloud = IconData(0xe1aa, fontFamily: _f);
  static const wrench = IconData(0xe5d4, fontFamily: _f);
  static const shareNetwork = IconData(0xe408, fontFamily: _f);
  static const wallet = IconData(0xe68a, fontFamily: _f);
  static const sun = IconData(0xe472, fontFamily: _f);
  static const minus = IconData(0xe32a, fontFamily: _f);
  static const square = IconData(0xe45e, fontFamily: _f);
  static const cornersIn = IconData(0xe1ce, fontFamily: _f);
  static const sparkle = IconData(0xe6a2, fontFamily: _f);
}

class _Clay {
  const _Clay({
    required this.bg,
    required this.card,
    required this.ink,
    required this.inkDim,
    required this.accent,
    required this.drop,
    required this.inHi,
    required this.inLo,
  });

  /// The table the clay sits on.
  final Color bg;

  /// Uncoloured clay: cards, wells, controls at rest.
  final Color card;
  final Color ink;
  final Color inkDim;
  final Color accent;

  /// The shadow the object drops below itself.
  final Color drop;

  /// The glaze catching light, top-left inside the edge.
  final Color inHi;

  /// The shade inside the bottom-right edge.
  final Color inLo;
}

class ClaySkin extends AppSkin {
  ClaySkin(Tokens t)
      : dark = t.dark,
        oled = t.dark && t.oled,
        _c = !t.dark ? _light : (t.oled ? _oled : _dark);

  final bool dark;
  final bool oled;
  final _Clay _c;

  static const _light = _Clay(
    bg: Color(0xFFEEE7FA),
    card: Color(0xFFFFFFFF),
    ink: Color(0xFF2B1B47),
    inkDim: Color(0xFF65577F),
    accent: Color(0xFFEE4F9B),
    drop: Color(0x4D603EA0),
    inHi: Color(0xD9FFFFFF),
    inLo: Color(0x29543490),
  );
  static const _dark = _Clay(
    bg: Color(0xFF1D1531),
    card: Color(0xFF2E2549),
    ink: Color(0xFFF4EEFF),
    inkDim: Color(0xFFB9ACD6),
    accent: Color(0xFFF45FA8),
    drop: Color(0x99000000),
    inHi: Color(0x26FFFFFF),
    inLo: Color(0x61000000),
  );
  static const _oled = _Clay(
    bg: Color(0xFF000000),
    card: Color(0xFF15101F),
    ink: Color(0xFFF7F2FF),
    inkDim: Color(0xFFA99CC7),
    accent: Color(0xFFFF5DB0),
    drop: Color(0x4D966EFF),
    inHi: Color(0x24FFFFFF),
    inLo: Color(0xA6000000),
  );

  @override
  DesignLanguage get language => DesignLanguage.claymorphism;

  @override
  Color get canvas => _c.bg;
  @override
  Color get ink => _c.ink;
  @override
  Color get inkDim => _c.inkDim;
  @override
  Color get accent => _c.accent;
  @override
  Color get accentSoft => clayOf(_c.accent);

  /// Dark ink on a pastel chip by day, light on a deep glaze by night — the
  /// accent on a clay of its own colour would not read.
  @override
  Color get activeInk => _c.ink;
  @override
  String get fontFamily => 'Fredoka';
  @override
  double get pressScale => 0.96;

  /// White on the pink gumdrop.
  @override
  Color get onProminent => Colors.white;

  /// The table is the page, the clay is every panel. On the dark tiers the
  /// inner shade is black on near-black, so the hairline is the glaze.
  @override
  Tokens retint(Tokens base) => tokensFrom(
        base,
        page: _c.bg,
        atmosphere: _c.bg,
        panel: _c.card,
        panel2: _c.card,
        modal: _c.card,
        ink: _c.ink,
        inkDim: _c.inkDim,
        inkInv: _c.card,
        hair: dark ? _c.inHi : _c.inLo,
        light: _c.inHi,
      );
  @override
  double get controlRadius => 22;
  @override
  double get panelRadius => 30;

  /// A tab's tint as clay: pastel by day, a deep glaze by night.
  Color clayOf(Color tint) => !dark
      ? Color.lerp(tint, Colors.white, 0.62)!
      : Color.lerp(tint, Colors.black, oled ? 0.66 : 0.55)!;

  /// One moulded object. [small] is a chip or key rather than a panel;
  /// [squashed] is one being pressed, its drop pulled in under it.
  SoftDecoration clay(Color fill, double radius,
          {bool small = false, bool squashed = false}) =>
      SoftDecoration(
        color: fill,
        radius: radius,
        outer: [
          SoftShadow(
            _c.drop,
            Offset(0, squashed ? 2 : (small ? 6 : 12)),
            squashed ? 6 : (small ? 14 : 24),
          ),
        ],
        inner: [
          SoftShadow(
              _c.inHi, Offset(small ? 3 : 5, small ? 3 : 6), small ? 6 : 12),
          SoftShadow(
              _c.inLo, Offset(small ? -3 : -6, small ? -4 : -8), small ? 8 : 14),
        ],
      );

  // Radii run large and by rank: panels puffier than covers, covers than keys.
  @override
  Decoration surface(SurfaceRole role, {double radius = 16}) => switch (role) {
        SurfaceRole.card => clay(_c.card, radius + 10),
        SurfaceRole.art => clay(_c.card, radius + 8, small: true),
        SurfaceRole.bar => clay(_c.card, 38),
        SurfaceRole.well => SoftDecoration(
            color: _c.card,
            radius: radius,
            inner: [
              SoftShadow(_c.inLo, const Offset(3, 4), 8),
              SoftShadow(_c.inHi, const Offset(-2, -2), 5),
            ],
          ),
      };

  @override
  Decoration control({
    required bool active,
    bool hovered = false,
    bool pressed = false,
    bool prominent = false,
    double radius = 18,
    Color? tint,
  }) {
    // The play button is a gumdrop in the accent.
    if (prominent) return clay(_c.accent, radius, squashed: pressed);
    if (active) {
      return clay(clayOf(tint ?? _c.accent), radius,
          small: true, squashed: pressed);
    }
    return clay(
      hovered
          ? Color.alphaBlend(_c.inHi.withValues(alpha: 0.2), _c.card)
          : _c.card,
      radius,
      small: true,
      squashed: pressed,
    );
  }

  /// Phosphor's Fill glyph for each Material icon the skinned widgets draw.
  /// Solid shapes read as painted onto the clay; outlines would read as cut.
  static final Map<IconData, IconData> _glyphs = {
    // Transport. The ±30 skips keep Material's, which carry the numeral.
    Icons.play_arrow: _Ph.play,
    Icons.play_arrow_rounded: _Ph.play,
    Icons.pause: _Ph.pause,
    Icons.skip_previous: _Ph.skipBack,
    Icons.skip_next: _Ph.skipForward,
    Icons.stop: _Ph.stop,
    Icons.shuffle: _Ph.shuffle,
    Icons.repeat: _Ph.repeat,
    Icons.repeat_one: _Ph.repeatOnce,
    Icons.favorite: _Ph.heart,
    Icons.favorite_border: _Ph.heart,
    Icons.star: _Ph.star,
    Icons.star_border: _Ph.star,
    Icons.equalizer: _Ph.equalizer,
    // The bar's extras. Picture-in-picture has no Phosphor glyph and keeps
    // Material's.
    Icons.tune: _Ph.slidersHorizontal,
    Icons.queue_music: _Ph.queue,
    Icons.lyrics_outlined: _Ph.microphoneStage,
    Icons.playlist_add: _Ph.playlist,
    Icons.bedtime_outlined: _Ph.moon,
    Icons.cast: _Ph.screencast,
    Icons.speaker: _Ph.speakerSimpleHigh,
    Icons.settings_outlined: _Ph.gear,
    Icons.open_in_full: _Ph.arrowsOut,
    Icons.volume_up: _Ph.speakerHigh,
    Icons.volume_down: _Ph.speakerLow,
    Icons.volume_mute: _Ph.speakerNone,
    Icons.volume_off: _Ph.speakerX,
    // The five categories and the ten library tabs.
    Icons.library_music_outlined: _Ph.vinylRecord,
    Icons.mic_none_outlined: _Ph.microphone,
    Icons.menu_book_outlined: _Ph.bookOpen,
    Icons.radio_outlined: _Ph.radio,
    Icons.home_outlined: _Ph.house,
    Icons.music_note_outlined: _Ph.musicNote,
    Icons.grid_view_outlined: _Ph.disc,
    Icons.person_outline: _Ph.user,
    Icons.queue_music_outlined: _Ph.playlist,
    Icons.folder_outlined: _Ph.folder,
    Icons.rotate_left: _Ph.clockCounterClockwise,
    Icons.download_outlined: _Ph.downloadSimple,
    // Header, pagers, sorting, row actions.
    Icons.music_note: _Ph.musicNote,
    Icons.search: _Ph.magnifyingGlass,
    Icons.close: _Ph.x,
    Icons.mic_none: _Ph.microphone,
    Icons.chevron_left: _Ph.caretLeft,
    Icons.chevron_right: _Ph.caretRight,
    Icons.expand_more: _Ph.caretDown,
    Icons.expand_less: _Ph.caretUp,
    Icons.keyboard_arrow_up: _Ph.caretUp,
    Icons.keyboard_arrow_down: _Ph.caretDown,
    Icons.sort: _Ph.sortAscending,
    Icons.arrow_upward: _Ph.arrowUp,
    Icons.arrow_downward: _Ph.arrowDown,
    Icons.refresh: _Ph.arrowClockwise,
    Icons.label_outline: _Ph.tag,
    Icons.delete_outline: _Ph.trash,
    Icons.move_to_inbox_outlined: _Ph.tray,
    Icons.content_copy_outlined: _Ph.copy,
    // The shell: sections, dock, caption buttons, chat.
    Icons.image_outlined: _Ph.image,
    Icons.movie_outlined: _Ph.filmStrip,
    Icons.cloud_outlined: _Ph.cloud,
    Icons.build_outlined: _Ph.wrench,
    Icons.share_outlined: _Ph.shareNetwork,
    Icons.account_balance_wallet_outlined: _Ph.wallet,
    Icons.light_mode: _Ph.sun,
    Icons.dark_mode: _Ph.moon,
    Icons.star_outline: _Ph.star,
    Icons.remove: _Ph.minus,
    Icons.crop_square: _Ph.square,
    Icons.filter_none: _Ph.copy,
    Icons.fullscreen_exit: _Ph.cornersIn,
    Icons.auto_awesome: _Ph.sparkle,
  };

  @override
  IconData icon(IconData material) => _glyphs[material] ?? material;
}
