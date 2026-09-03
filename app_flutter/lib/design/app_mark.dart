// The Tulipix mark, and which of the five bundled logos it draws.
//
// Settings › Profile picks one and it rides the shell snapshot as
// `ShellState.logoChoice` — the same `profile.logo` setting the Slint build
// reads, so a mark chosen in one build is the mark the other opens with.
//
// This used to live in home_shared.dart and always drew `sidebar-default.png`;
// the sidebar drew a gradient "T" and read the setting not at all. All five
// assets have been in the bundle the whole time.

import 'package:flutter/material.dart';

import 'tokens.dart';

/// 0 default · 1 colour · 2 dark · 3 white · 4 India.
///
/// Matches `BrandHeader.logo-choice` in ui/sidebar.slint arm for arm, including
/// the reason 4 exists separately from the seasonal default.
String appLogoAsset(int choice, {DateTime? now}) => switch (choice) {
      1 => 'assets/appicons/sidebar-color.png',
      2 => 'assets/appicons/sidebar-dark.png',
      3 => 'assets/appicons/sidebar-white.png',
      // 4 is the India mark picked ON PURPOSE — the same asset the season swaps
      // in, but chosen, so it holds all year.
      4 => 'assets/appicons/logo_india.png',
      _ => isFestivalSeason(now)
          ? 'assets/appicons/logo_india.png'
          : 'assets/appicons/sidebar-default.png',
    };

/// August, and the back half of January.
///
/// A second copy of `festival_season()` in crates/tulipix-app/src/main.rs, and
/// deliberately so: it is a date predicate on the drawing side, and carrying it
/// across would mean a new snapshot field, a codegen run and a rebuild for two
/// comparisons. If the window ever moves, it moves in both.
bool isFestivalSeason([DateTime? now]) {
  final d = now ?? DateTime.now();
  return d.month == 8 || (d.month == 1 && d.day >= 15);
}

/// The mark, at whatever size the thing drawing it has room for.
class AppMark extends StatelessWidget {
  const AppMark({
    super.key,
    this.size = 22,
    this.radius = 7,
    this.choice = 0,
  });

  final double size;
  final double radius;

  /// Which logo. Callers that have the shell snapshot pass
  /// `state.logoChoice`; the ones that are only decorating take the default.
  final int choice;

  @override
  Widget build(BuildContext context) => ClipRRect(
        borderRadius: BorderRadius.circular(radius),
        child: Image.asset(
          appLogoAsset(choice),
          width: size,
          height: size,
          fit: BoxFit.cover,
          errorBuilder: (_, __, ___) => Container(
            width: size,
            height: size,
            color: Tokens.brand,
            child: Icon(Icons.hexagon, size: size * 0.6, color: Colors.white),
          ),
        ),
      );
}
