// Neumorphism — "Soft Console".
//
// Every control is pressed out of one soft sheet, and light falling across it
// is all that tells a button from the ground: things you press stand up out of
// the surface, things that hold a value are sunk into it, and a selected tab or
// a toggle that is on is pressed in.
//
// The values are the mockup's (docs/mockups/music-neumorphism.html), one set
// per tier. On OLED a shadow has nowhere to fall against #000, so the shadow
// pair becomes a 1px rim of light — top-left when raised, bottom-right when
// sunk — plus a faint hairline, and every other pixel stays off.
//
// Low contrast is this style's known weak spot, so text never sits on a shadow
// tone and every active state also turns the accent on: a selected tab still
// reads with the shadows switched off.

import 'package:flutter/material.dart';
import 'package:flutter_tabler_icons/flutter_tabler_icons.dart';

import '../design_language.dart';
import '../skin.dart';
import '../soft_decoration.dart';
import '../tokens.dart';

class _Palette {
  const _Palette({
    required this.bg,
    required this.hi,
    required this.lo,
    required this.ink,
    required this.inkDim,
    required this.accent,
    required this.accentSoft,
  });

  /// The sheet.
  final Color bg;

  /// The light cast, up and to the left.
  final Color hi;

  /// The dark cast, down and to the right.
  final Color lo;
  final Color ink;
  final Color inkDim;
  final Color accent;
  final Color accentSoft;
}

class NeuSkin extends MusicSkin {
  NeuSkin(Tokens t)
      : oled = t.dark && t.oled,
        _p = !t.dark ? _light : (t.oled ? _oled : _dark);

  final bool oled;
  final _Palette _p;

  // Ink on ground is at least 9:1 on every tier, muted ink at least 4.9:1.
  static const _light = _Palette(
    bg: Color(0xFFE4E6EE),
    hi: Color(0xFFFFFFFF),
    lo: Color(0xFFBCC0D1),
    ink: Color(0xFF363950),
    inkDim: Color(0xFF5F637B),
    accent: Color(0xFFD63C84),
    accentSoft: Color(0xFFF2A7C9),
  );
  static const _dark = _Palette(
    bg: Color(0xFF272A35),
    hi: Color(0xFF343848),
    lo: Color(0xFF191B23),
    ink: Color(0xFFDCDEE9),
    inkDim: Color(0xFF9195AB),
    accent: Color(0xFFF45FA5),
    accentSoft: Color(0xFF7B3A5F),
  );
  static const _oled = _Palette(
    bg: Color(0xFF000000),
    hi: Color(0x29FFFFFF),
    lo: Color(0x0AFFFFFF),
    ink: Color(0xFFEDEEF5),
    inkDim: Color(0xFF8A8DA3),
    accent: Color(0xFFFF5AAC),
    accentSoft: Color(0xFF5C1F40),
  );

  @override
  DesignLanguage get language => DesignLanguage.neumorphism;

  @override
  Color get canvas => _p.bg;
  @override
  Color get ink => _p.ink;
  @override
  Color get inkDim => _p.inkDim;
  @override
  Color get accent => _p.accent;
  @override
  Color get accentSoft => _p.accentSoft;
  @override
  String get fontFamily => 'MPLUSRounded1c';

  /// Standing up out of the sheet: a dark cast down-right, a light cast up-left.
  SoftDecoration raised(double radius, {double depth = 7, Gradient? gradient}) =>
      oled
          ? SoftDecoration(
              color: _p.bg,
              gradient: gradient,
              radius: radius,
              inner: [
                SoftShadow(_p.hi, const Offset(1.5, 1.5), 0),
                SoftShadow(_p.lo, const Offset(-1, -1), 0),
              ],
              border: const Color(0x0FFFFFFF),
            )
          : SoftDecoration(
              color: _p.bg,
              gradient: gradient,
              radius: radius,
              outer: [
                SoftShadow(_p.lo, Offset(depth, depth), depth * 2.2),
                SoftShadow(_p.hi, Offset(-depth, -depth), depth * 2.2),
              ],
            );

  /// Sunk into the sheet: the same pair, falling inside the edge.
  SoftDecoration sunk(double radius, {double depth = 5}) => oled
      ? SoftDecoration(
          color: _p.bg,
          radius: radius,
          inner: [
            SoftShadow(_p.hi, const Offset(-1.5, -1.5), 0),
            SoftShadow(_p.lo, const Offset(1, 1), 0),
          ],
          border: const Color(0x0AFFFFFF),
        )
      : SoftDecoration(
          color: _p.bg,
          radius: radius,
          inner: [
            SoftShadow(_p.lo, Offset(depth, depth), depth * 2),
            SoftShadow(_p.hi, Offset(-depth, -depth), depth * 2),
          ],
        );

  @override
  Decoration surface(SurfaceRole role, {double radius = 16}) => switch (role) {
        SurfaceRole.card => raised(radius),
        SurfaceRole.art => raised(radius, depth: 3),
        SurfaceRole.well => sunk(radius),
        SurfaceRole.bar => raised(radius, depth: 8),
      };

  @override
  Decoration control({
    required bool active,
    bool hovered = false,
    bool pressed = false,
    bool prominent = false,
    double radius = 18,
    // One sheet, one colour: a tab's own tint does not change what it is
    // pressed out of.
    Color? tint,
  }) {
    if (prominent) {
      // The play button is a convex disc: raised, and lit across its face. It
      // does not latch — playing is not "on" for it — so only a press sinks it.
      if (pressed) return sunk(radius, depth: 4);
      return raised(
        radius,
        depth: hovered ? 8 : 7,
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: const Alignment(0.4, 0.4),
          colors: [_p.hi, _p.bg],
        ),
      );
    }
    if (active || pressed) return sunk(radius, depth: 2);
    return raised(radius, depth: hovered ? 4 : 3);
  }

  /// Tabler's outline glyph for each Material icon the skinned widgets draw.
  /// Fine strokes read as engraved into the sheet; filled ones would sit on it.
  static final Map<IconData, IconData> _glyphs = {
    // Transport.
    Icons.play_arrow: TablerIcons.player_play,
    Icons.play_arrow_rounded: TablerIcons.player_play,
    Icons.pause: TablerIcons.player_pause,
    Icons.skip_previous: TablerIcons.player_skip_back,
    Icons.skip_next: TablerIcons.player_skip_forward,
    Icons.replay_30: TablerIcons.rewind_backward_30,
    Icons.forward_30: TablerIcons.rewind_forward_30,
    Icons.stop: TablerIcons.player_stop,
    Icons.shuffle: TablerIcons.arrows_shuffle,
    Icons.repeat: TablerIcons.repeat,
    Icons.repeat_one: TablerIcons.repeat_once,
    Icons.favorite: TablerIcons.heart_filled,
    Icons.favorite_border: TablerIcons.heart,
    Icons.star: TablerIcons.star_filled,
    Icons.star_border: TablerIcons.star,
    Icons.equalizer: TablerIcons.chart_bar,
    // The bar's extras.
    Icons.tune: TablerIcons.adjustments_horizontal,
    Icons.queue_music: TablerIcons.list_numbers,
    Icons.lyrics_outlined: TablerIcons.microphone_2,
    Icons.playlist_add: TablerIcons.playlist_add,
    Icons.bedtime_outlined: TablerIcons.moon,
    Icons.cast: TablerIcons.cast,
    Icons.speaker: TablerIcons.device_speaker,
    Icons.picture_in_picture_alt: TablerIcons.picture_in_picture,
    Icons.settings_outlined: TablerIcons.settings,
    Icons.open_in_full: TablerIcons.arrows_maximize,
    Icons.volume_up: TablerIcons.volume,
    Icons.volume_down: TablerIcons.volume_2,
    Icons.volume_mute: TablerIcons.volume_3,
    Icons.volume_off: TablerIcons.volume_off,
    // The five categories and the ten library tabs.
    Icons.library_music_outlined: TablerIcons.vinyl,
    Icons.mic_none_outlined: TablerIcons.microphone,
    Icons.menu_book_outlined: TablerIcons.book,
    Icons.radio_outlined: TablerIcons.radio,
    Icons.home_outlined: TablerIcons.home,
    Icons.music_note_outlined: TablerIcons.music,
    Icons.grid_view_outlined: TablerIcons.disc,
    Icons.person_outline: TablerIcons.user,
    Icons.queue_music_outlined: TablerIcons.playlist,
    Icons.folder_outlined: TablerIcons.folder,
    Icons.rotate_left: TablerIcons.history,
    Icons.download_outlined: TablerIcons.download,
    // Header, pagers, sorting, row actions.
    Icons.music_note: TablerIcons.music,
    Icons.search: TablerIcons.search,
    Icons.close: TablerIcons.x,
    Icons.mic_none: TablerIcons.microphone,
    Icons.chevron_left: TablerIcons.chevron_left,
    Icons.chevron_right: TablerIcons.chevron_right,
    Icons.expand_more: TablerIcons.chevron_down,
    Icons.expand_less: TablerIcons.chevron_up,
    Icons.keyboard_arrow_up: TablerIcons.chevron_up,
    Icons.keyboard_arrow_down: TablerIcons.chevron_down,
    Icons.sort: TablerIcons.sort_ascending,
    Icons.arrow_upward: TablerIcons.arrow_up,
    Icons.arrow_downward: TablerIcons.arrow_down,
    Icons.refresh: TablerIcons.refresh,
    Icons.label_outline: TablerIcons.tags,
    Icons.delete_outline: TablerIcons.trash,
    Icons.move_to_inbox_outlined: TablerIcons.file_import,
    Icons.content_copy_outlined: TablerIcons.copy,
  };

  @override
  IconData icon(IconData material) => _glyphs[material] ?? material;
}
