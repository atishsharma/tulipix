// Glassmorphism — "Lightbox".
//
// The playing cover is the light source: its colours bleed out behind the
// section, and every panel is frosted glass over that glow. At rest a chip is
// bare glyph and label on the glass behind it; the one solid thing on the page
// is the play button, in the ink colour.
//
// The values are the mockup's (docs/mockups/music-glassmorphism.html). Light
// glass stays over half opaque, so dim text never sits on raw aura. OLED keeps
// the aura to the bottom-left corner and leaves the rest of the screen off; the
// glass is drawn by its 1px edge and top sheen there.

import 'package:flutter/material.dart';
import 'package:lucide_icons_flutter/lucide_icons.dart';

import '../design_language.dart';
import '../skin.dart';
import '../soft_decoration.dart';
import '../tokens.dart';

class _Glass {
  const _Glass({
    required this.base,
    required this.ink,
    required this.inkDim,
    required this.accent,
    required this.glass,
    required this.strong,
    required this.edge,
    required this.sheen,
    required this.drop,
    required this.aura,
  });

  /// The page under the aura.
  final Color base;
  final Color ink;
  final Color inkDim;
  final Color accent;

  /// A pane, and a selected one.
  final Color glass;
  final Color strong;

  /// The 1px edge, and the light along its top.
  final Color edge;
  final Color sheen;

  /// The shadow under a pane. Null on OLED, where it would not show.
  final Color? drop;

  /// How hard the aura glows.
  final double aura;
}

class GlassSkin extends AppSkin {
  GlassSkin(Tokens t)
      : oled = t.dark && t.oled,
        _g = !t.dark ? _light : (t.oled ? _oled : _dark);

  final bool oled;
  final _Glass _g;

  static const _light = _Glass(
    base: Color(0xFFF3EFF9),
    ink: Color(0xFF1C1530),
    inkDim: Color(0xFF51486A),
    accent: Color(0xFFD0206F),
    glass: Color(0x85FFFFFF),
    strong: Color(0xDBFFFFFF),
    edge: Color(0xBFFFFFFF),
    sheen: Color(0xE6FFFFFF),
    drop: Color(0x385A2878),
    aura: 0.85,
  );
  static const _dark = _Glass(
    base: Color(0xFF0D0A18),
    ink: Color(0xFFF6F3FF),
    inkDim: Color(0xFFB6ADCD),
    accent: Color(0xFFF472B6),
    glass: Color(0x13FFFFFF),
    strong: Color(0x2EFFFFFF),
    edge: Color(0x24FFFFFF),
    sheen: Color(0x33FFFFFF),
    drop: Color(0x99000000),
    aura: 0.7,
  );
  static const _oled = _Glass(
    base: Color(0xFF000000),
    ink: Color(0xFFF8F6FF),
    inkDim: Color(0xFFA098B8),
    accent: Color(0xFFFF5DB0),
    glass: Color(0x0BFFFFFF),
    strong: Color(0x24FFFFFF),
    edge: Color(0x21FFFFFF),
    sheen: Color(0x38FFFFFF),
    drop: null,
    aura: 0.55,
  );

  @override
  DesignLanguage get language => DesignLanguage.glassmorphism;

  @override
  Color get canvas => _g.base;
  @override
  Color get ink => _g.ink;
  @override
  Color get inkDim => _g.inkDim;
  @override
  Color get accent => _g.accent;
  @override
  Color get accentSoft => _g.accent.withValues(alpha: 0.35);
  @override
  String get fontFamily => 'Lexend';

  /// The page's colour on the solid ink play button.
  @override
  Color get onProminent => _g.base;

  /// Panes over the base. A dialog is strong glass composited onto the base,
  /// so it stays opaque enough to read over anything behind it.
  @override
  Tokens retint(Tokens base) => tokensFrom(
        base,
        page: _g.base,
        atmosphere: _g.base,
        panel: _g.glass,
        panel2: _g.strong,
        modal: Color.alphaBlend(_g.strong, _g.base),
        ink: _g.ink,
        inkDim: _g.inkDim,
        inkInv: _g.base,
        hair: _g.edge,
        light: _g.sheen,
        fill: _g.glass,
        fillStrong: _g.strong,
      );
  /// The docked queue and lyrics: the pane as it looks over the bare page, at
  /// 80%. At the pane's own tint the list sat on the aura and on whatever the
  /// page drew under it.
  @override
  Color get sheet => Color.alphaBlend(_g.glass, _g.base).withValues(alpha: 0.8);

  @override
  double get controlRadius => 14;
  @override
  double get panelRadius => 20;

  SoftDecoration pane(double radius, {bool strong = false}) {
    final drop = _g.drop;
    return SoftDecoration(
      color: strong ? _g.strong : _g.glass,
      radius: radius,
      outer: [if (drop != null) SoftShadow(drop, const Offset(0, 10), 30)],
      inner: [SoftShadow(_g.sheen, const Offset(0, 1), 0)],
      border: _g.edge,
    );
  }

  @override
  Decoration surface(SurfaceRole role, {double radius = 16}) => switch (role) {
        // A cover is its own light; it gets an edge, not a pane.
        SurfaceRole.art => SoftDecoration(
            color: const Color(0x00000000),
            radius: radius,
            border: _g.edge,
          ),
        _ => pane(radius),
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
    if (prominent) {
      return SoftDecoration(
        color: pressed ? _g.inkDim : _g.ink,
        radius: radius,
        outer: [
          SoftShadow(_g.accent.withValues(alpha: 0.6), const Offset(0, 8), 26),
        ],
      );
    }
    if (active) return pane(radius, strong: true);
    if (hovered || pressed) return pane(radius);
    return SoftDecoration(color: const Color(0x00000000), radius: radius);
  }

  @override
  Widget pageBackdrop({required Color accent, Color? alt}) {
    final a = _g.aura;
    final second = alt ?? const Color(0xFF8B5CF6);
    return IgnorePointer(
      child: DecoratedBox(
        decoration: BoxDecoration(
          gradient: RadialGradient(
            center: const Alignment(-0.85, 1.0),
            radius: oled ? 0.55 : 0.95,
            colors: [
              accent.withValues(alpha: a * 0.75),
              accent.withValues(alpha: 0),
            ],
          ),
        ),
        child: oled
            ? null
            : DecoratedBox(
                decoration: BoxDecoration(
                  gradient: RadialGradient(
                    center: const Alignment(0.8, -0.9),
                    radius: 0.8,
                    colors: [
                      second.withValues(alpha: a * 0.5),
                      second.withValues(alpha: 0),
                    ],
                  ),
                ),
              ),
      ),
    );
  }

  // No backdrop blur behind the panes, so `frame` is the base's. Every pane
  // sits on the aura — two radial gradients — and nothing scrolls beneath
  // one, so a blur of it drew the same smooth gradient back. It was invisible,
  // and on Impeller's GL backend each pane cost an offscreen layer and its
  // blur passes on every frame: twenty-four of them held My Music under 10 fps,
  // and the raster thread near 100%, whenever music played. A pane is its tint,
  // its edge and its sheen.

  /// Lucide's 2px outlines for each Material icon the skinned widgets draw.
  /// The ±30 skips keep Material's, which carry the numeral.
  static final Map<IconData, IconData> _glyphs = {
    // Transport.
    Icons.play_arrow: LucideIcons.play,
    Icons.play_arrow_rounded: LucideIcons.play,
    Icons.pause: LucideIcons.pause,
    Icons.skip_previous: LucideIcons.skipBack,
    Icons.skip_next: LucideIcons.skipForward,
    Icons.stop: LucideIcons.square,
    Icons.shuffle: LucideIcons.shuffle,
    Icons.repeat: LucideIcons.repeat,
    Icons.repeat_one: LucideIcons.repeat1,
    Icons.favorite: LucideIcons.heart,
    Icons.favorite_border: LucideIcons.heart,
    Icons.star: LucideIcons.star,
    Icons.star_border: LucideIcons.star,
    Icons.equalizer: LucideIcons.chartNoAxesColumn,
    // The bar's extras.
    Icons.tune: LucideIcons.slidersHorizontal,
    Icons.queue_music: LucideIcons.listEnd,
    Icons.lyrics_outlined: LucideIcons.micVocal,
    Icons.playlist_add: LucideIcons.listPlus,
    Icons.bedtime_outlined: LucideIcons.moon,
    Icons.cast: LucideIcons.cast,
    Icons.speaker: LucideIcons.speaker,
    Icons.picture_in_picture_alt: LucideIcons.pictureInPicture2,
    Icons.settings_outlined: LucideIcons.settings,
    Icons.open_in_full: LucideIcons.maximize2,
    Icons.volume_up: LucideIcons.volume2,
    Icons.volume_down: LucideIcons.volume1,
    Icons.volume_mute: LucideIcons.volume1,
    Icons.volume_off: LucideIcons.volumeX,
    // The five categories and the ten library tabs.
    Icons.library_music_outlined: LucideIcons.disc3,
    Icons.mic_none_outlined: LucideIcons.podcast,
    Icons.menu_book_outlined: LucideIcons.bookAudio,
    Icons.radio_outlined: LucideIcons.radio,
    Icons.home_outlined: LucideIcons.house,
    Icons.music_note_outlined: LucideIcons.music,
    Icons.grid_view_outlined: LucideIcons.disc3,
    Icons.person_outline: LucideIcons.userRound,
    Icons.queue_music_outlined: LucideIcons.listMusic,
    Icons.folder_outlined: LucideIcons.folder,
    Icons.rotate_left: LucideIcons.history,
    Icons.download_outlined: LucideIcons.download,
    // Header, pagers, sorting, row actions.
    Icons.music_note: LucideIcons.music,
    Icons.search: LucideIcons.search,
    Icons.close: LucideIcons.x,
    Icons.mic_none: LucideIcons.mic,
    Icons.chevron_left: LucideIcons.chevronLeft,
    Icons.chevron_right: LucideIcons.chevronRight,
    Icons.expand_more: LucideIcons.chevronDown,
    Icons.expand_less: LucideIcons.chevronUp,
    Icons.keyboard_arrow_up: LucideIcons.chevronUp,
    Icons.keyboard_arrow_down: LucideIcons.chevronDown,
    Icons.sort: LucideIcons.arrowUpDown,
    Icons.arrow_upward: LucideIcons.arrowUp,
    Icons.arrow_downward: LucideIcons.arrowDown,
    Icons.refresh: LucideIcons.refreshCw,
    Icons.label_outline: LucideIcons.tags,
    Icons.delete_outline: LucideIcons.trash2,
    Icons.move_to_inbox_outlined: LucideIcons.fileInput,
    Icons.content_copy_outlined: LucideIcons.copy,
    // The shell: sections, dock, caption buttons, chat.
    Icons.image_outlined: LucideIcons.image,
    Icons.movie_outlined: LucideIcons.film,
    Icons.cloud_outlined: LucideIcons.cloud,
    Icons.build_outlined: LucideIcons.wrench,
    Icons.share_outlined: LucideIcons.share2,
    Icons.account_balance_wallet_outlined: LucideIcons.wallet,
    Icons.light_mode: LucideIcons.sun,
    Icons.dark_mode: LucideIcons.moon,
    Icons.star_outline: LucideIcons.star,
    Icons.remove: LucideIcons.minus,
    Icons.crop_square: LucideIcons.square,
    Icons.filter_none: LucideIcons.copy,
    Icons.fullscreen_exit: LucideIcons.minimize2,
    Icons.auto_awesome: LucideIcons.sparkles,
  };

  @override
  IconData icon(IconData material) => _glyphs[material] ?? material;
}
