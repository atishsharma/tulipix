// The sidebar — a port of ui/sidebar.slint.
//
// Two widths, not a drawer: collapsed is a 68px icon rail, expanded is a
// labelled column, and the transition is the width animating rather than a
// panel sliding over the page. What is on it, top to bottom: the brand mark,
// Home, the eight Applications, the health lamp beside Lock, and a footer
// dock of three buttons (Settings · Theme · Collapse).
//
// The active row is a double outline — an outer ring in the section accent and
// an inner ring in neutral ink. No cast shadow under a nav row in any theme:
// the active row is pressed *into* the shell, and a recess that also floats
// reads as neither.

import 'dart:io';

import 'package:flutter/material.dart';

import '../design/app_mark.dart';
import '../design/skin.dart';
import '../design/tokens.dart';
import '../src/rust/api/shell.dart';
import 'lock/lock_controller.dart';
import 'shell_controller.dart';

const double kSidebarCollapsed = 68;
const double kSidebarExpanded = 174;

/// Section → its accent, label and glyph. One table rather than a switch at
/// each of the three places that needs one.
const Map<Section, ({String label, IconData icon})> kSectionMeta = {
  Section.home: (label: 'Home', icon: Icons.home_outlined),
  Section.photos: (label: 'Photos', icon: Icons.image_outlined),
  Section.videos: (label: 'Videos', icon: Icons.movie_outlined),
  Section.music: (label: 'Music', icon: Icons.music_note_outlined),
  Section.books: (label: 'Books', icon: Icons.menu_book_outlined),
  Section.cloud: (label: 'Cloud', icon: Icons.cloud_outlined),
  Section.tools: (label: 'Tools', icon: Icons.build_outlined),
  Section.transfer: (label: 'Transfer', icon: Icons.share_outlined),
  Section.finances: (
    label: 'Finances',
    icon: Icons.account_balance_wallet_outlined
  ),
  Section.feeds: (label: 'Feeds', icon: Icons.rss_feed),
  Section.journal: (label: 'Journal', icon: Icons.edit_note),
  Section.kitchen: (label: 'Kitchen', icon: Icons.restaurant_outlined),
  Section.papers: (label: 'Papers', icon: Icons.description_outlined),
  Section.settings: (label: 'Settings', icon: Icons.settings_outlined),
};

/// The section's accent. `Tokens.accentOf` is the table; this is the name the
/// three Home layouts and the sidebar reach for.
Color accentFor(Section s) => Tokens.accentOf(s);

/// The twelve sections under the APPLICATIONS header, in the order a fresh
/// install shows them.
///
/// The default, not the list. What is actually drawn is
/// `ShellController.sections`, which Settings → Sections writes — this is what
/// that falls back to before the first snapshot lands.
const List<Section> kApplications = [
  Section.photos,
  Section.videos,
  Section.music,
  Section.books,
  Section.cloud,
  Section.tools,
  Section.transfer,
  Section.finances,
  Section.feeds,
  Section.journal,
  Section.kitchen,
  Section.papers,
];

class Sidebar extends StatelessWidget {
  const Sidebar({
    super.key,
    required this.controller,
    required this.onCycleTheme,
    required this.themeIcon,
  });

  final ShellController controller;
  final VoidCallback onCycleTheme;
  final IconData themeIcon;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = controller;
    final collapsed = c.collapsed;
    final st = c.state;
    // (is Home shown, the applications under the header). Settings has its own
    // row in the dock below, so it is never in this list.
    final apps = (
      c.sections.contains(Section.home),
      [
        for (final s in c.sections)
          if (s != Section.home && s != Section.settings) s
      ],
    );
    return AnimatedContainer(
      duration: const Duration(milliseconds: 160),
      curve: Curves.easeOut,
      width: collapsed ? kSidebarCollapsed : kSidebarExpanded,
      margin: const EdgeInsets.fromLTRB(14, 14, 0, 14),
      decoration: context.skin
              .surface(SurfaceRole.card, radius: Tokens.radiusLg) ??
          BoxDecoration(
            color: t.panel,
            borderRadius: BorderRadius.circular(Tokens.radiusLg),
            border: Border.all(color: t.outline),
          ),
      child: Padding(
        padding: EdgeInsets.all(collapsed ? 10 : 12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _Brand(
              collapsed: collapsed,
              logoChoice: st?.logoChoice ?? 0,
              onTap: c.toggleAppFullscreen,
              fullscreen: c.appFullscreen,
            ),
            const SizedBox(height: 14),
            if (apps.$1)
              _NavRow(
                section: Section.home,
                active: c.section == Section.home,
                collapsed: collapsed,
                onTap: () => c.go(Section.home),
              ),
            if (apps.$2.isNotEmpty) _GroupHeader(collapsed: collapsed),
            // Every application on screen at once: the rows give up height
            // together, open or collapsed, before the rail ever scrolls. Only a
            // window too short for the floor below falls back to a scroller.
            Expanded(
              child: LayoutBuilder(builder: (context, box) {
                final natural = collapsed ? 46.0 : 52.0;
                const gap = 6.0;
                final n = apps.$2.length;
                final scale = n == 0
                    ? 1.0
                    : (box.maxHeight / n / (natural + gap))
                        .clamp(0.0, 1.0)
                        .toDouble();
                final floor = collapsed ? 30.0 : 32.0;
                final h = natural * scale < floor ? floor : natural * scale;
                final g = gap * scale;
                final rows = [
                  for (final s in apps.$2)
                    _NavRow(
                      section: s,
                      active: c.section == s,
                      collapsed: collapsed,
                      height: h,
                      gap: g,
                      badge: switch (s) {
                        Section.finances => st?.financesBadge ?? 0,
                        Section.feeds => st?.feedsUnread ?? 0,
                        _ => 0,
                      },
                      alarm: s == Section.finances &&
                          (st?.financesOverdue ?? false),
                      onTap: () => c.go(s),
                    ),
                ];
                return n * (h + g) <= box.maxHeight + 0.5
                    ? Column(children: rows)
                    : ListView(padding: EdgeInsets.zero, children: rows);
              }),
            ),
            const SizedBox(height: 8),
            Container(height: 1, color: t.outline),
            const SizedBox(height: 8),
            // The lamp beside the lock: half the row each, stacked when the
            // rail is a column of icons.
            if (collapsed) ...[
              _StatusButton(controller: c),
              const SizedBox(height: 6),
              const _LockBtn(),
            ] else
              Row(
                children: [
                  Expanded(child: _StatusButton(controller: c)),
                  const SizedBox(width: 6),
                  const Expanded(child: _LockBtn()),
                ],
              ),
            const SizedBox(height: 8),
            _Dock(
              controller: c,
              collapsed: collapsed,
              onCycleTheme: onCycleTheme,
              themeIcon: themeIcon,
            ),
            if (!collapsed) ...[
              const SizedBox(height: 10),
              _UserCard(state: st, onTap: () => c.go(Section.settings)),
            ],
          ],
        ),
      ),
    );
  }
}

// ── one navigation row ──────────────────────────────────────────────────────

class _NavRow extends StatefulWidget {
  const _NavRow({
    required this.section,
    required this.active,
    required this.collapsed,
    required this.onTap,
    this.badge = 0,
    this.alarm = false,
    this.height,
    this.gap = 6,
  });

  final Section section;
  final bool active;
  final bool collapsed;
  final VoidCallback onTap;

  /// The row's height when the rail is fitting every application in; null
  /// is the full 52 (46 collapsed).
  final double? height;

  /// Space under the row, shrunk in step with [height].
  final double gap;

  /// Count of things wanting attention. 0 hides the badge. Finances sets one
  /// for bills due, Feeds for unread articles.
  final int badge;

  /// Whether that count contains something already late. An overdue bill and a
  /// bill due on Friday are not the same news, and one dot colour for both
  /// teaches you to ignore the colour.
  final bool alarm;

  @override
  State<_NavRow> createState() => _NavRowState();
}

class _NavRowState extends State<_NavRow> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final accent = accentFor(widget.section);
    final meta = kSectionMeta[widget.section]!;
    final h = widget.height ?? (widget.collapsed ? 46.0 : 52.0);
    // A skin draws the open row as its latched control in the section's own
    // colour, and the hovered one as its hover. At rest a row is bare on the
    // rail: nine raised rows would be a keyboard, not a list.
    final skin = context.skin;
    final skinned = widget.active || _hover
        ? skin.control(
            active: widget.active,
            hovered: _hover,
            tint: accent,
            radius: Tokens.radiusMd,
          )
        : null;
    return Padding(
      padding: EdgeInsets.only(bottom: widget.gap),
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 130),
            height: h,
            padding:
                EdgeInsets.symmetric(horizontal: widget.collapsed ? 0 : 12),
            decoration: skinned ??
                (!skin.isStandard
                    ? const BoxDecoration()
                    : BoxDecoration(
                        color: widget.active
                            ? accent.withValues(alpha: 0.16)
                            : _hover
                                ? t.glass
                                : Colors.transparent,
                        borderRadius: BorderRadius.circular(Tokens.radiusMd),
                        border: Border.all(
                          color: widget.active
                              ? accent.withValues(alpha: 0.55)
                              : _hover
                                  ? t.outline
                                  : Colors.transparent,
                          width: widget.active ? 1.5 : 1,
                        ),
                      )),
            child: widget.collapsed
                ? Center(child: _glyph(accent, t))
                : Row(
                    children: [
                      _glyph(accent, t),
                      const SizedBox(width: 11),
                      Expanded(
                        child: Text(meta.label,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                              fontSize: 13,
                              fontWeight: widget.active
                                  ? FontWeight.w700
                                  : FontWeight.w500,
                              color: widget.active
                                  ? (skin.activeInk ?? accent)
                                  : t.text,
                            )),
                      ),
                      if (widget.badge > 0)
                        _Badge(count: widget.badge, alarm: widget.alarm),
                    ],
                  ),
          ),
        ),
      ),
    );
  }

  Widget _glyph(Color accent, Tokens t) {
    final full = widget.collapsed ? 21.0 : 19.0;
    final h = widget.height ?? (widget.collapsed ? 46.0 : 52.0);
    final icon = Icon(
      context.skin.icon(kSectionMeta[widget.section]!.icon),
      // A short row keeps its glyph in proportion rather than touching the edge.
      size: (h * 0.5).clamp(14.0, full).toDouble(),
      color: widget.active ? accent : t.textDim,
    );
    // Collapsed, the badge has no label to sit after, so it rides the glyph.
    if (widget.collapsed && widget.badge > 0) {
      return Stack(
        clipBehavior: Clip.none,
        children: [
          icon,
          Positioned(
            right: -6,
            top: -4,
            child:
                _Badge(count: widget.badge, alarm: widget.alarm, small: true),
          ),
        ],
      );
    }
    return icon;
  }
}

class _Badge extends StatelessWidget {
  const _Badge({required this.count, required this.alarm, this.small = false});

  final int count;
  final bool alarm;
  final bool small;

  @override
  Widget build(BuildContext context) {
    final tint = alarm ? Tokens.error : Tokens.warn;
    return Container(
      height: small ? 15 : 18,
      constraints: BoxConstraints(minWidth: small ? 15 : 18),
      padding: EdgeInsets.symmetric(horizontal: small ? 3 : 5),
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: tint,
        borderRadius: BorderRadius.circular(9),
      ),
      child: Text(count > 99 ? '99+' : '$count',
          style: TextStyle(
              fontSize: small ? 9 : 10.5,
              fontWeight: FontWeight.w800,
              color: Colors.white)),
    );
  }
}

// ── brand, group header, user ───────────────────────────────────────────────

/// `BrandHeader` in ui/sidebar.slint: the chosen mark, then the wordmark.
///
/// The mark used to be a drawn gradient "T" here, on a note saying the logo
/// assets were not in the Flutter bundle — they were, all five of them, and the
/// `profile.logo` the Settings picker saved was read by nothing. It is the
/// snapshot's `logoChoice` now, which is the same setting the Slint build
/// reads, so the two windows open wearing the same mark.
class _Brand extends StatelessWidget {
  const _Brand({
    required this.collapsed,
    required this.logoChoice,
    required this.onTap,
    required this.fullscreen,
  });

  final bool collapsed;
  final int logoChoice;

  /// The mark is the door to logo-fullscreen -- `logo-clicked` in
  /// ui/main.slint, which flips `app-fullscreen` and asks the window manager
  /// for it. The way back out is the same mark, or the bar the top edge peeks.
  final VoidCallback onTap;
  final bool fullscreen;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // 36px on a 9px corner — `Surface.glyph-radius`, and the same size the
    // Slint header gives it.
    final Widget mark = Tooltip(
      message: fullscreen ? 'Leave fullscreen' : 'Fullscreen',
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        child: GestureDetector(
          onTap: onTap,
          child: AppMark(size: 36, radius: 9, choice: logoChoice),
        ),
      ),
    );
    if (collapsed) return Center(child: mark);
    return Row(
      children: [
        mark,
        const SizedBox(width: 12),
        Expanded(
          child: Text('Tulipix',
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontSize: 17,
                fontWeight: FontWeight.w700,
                letterSpacing: -0.1,
                color: t.text,
              )),
        ),
      ],
    );
  }
}

class _GroupHeader extends StatelessWidget {
  const _GroupHeader({required this.collapsed});

  final bool collapsed;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (collapsed) {
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 8),
        child: Center(
          child: Container(width: 20, height: 1, color: t.outline),
        ),
      );
    }
    return Padding(
      padding: const EdgeInsets.fromLTRB(6, 10, 6, 6),
      child: Text('APPLICATIONS',
          style: TextStyle(
              fontSize: 9.5,
              fontWeight: FontWeight.w800,
              letterSpacing: 1.1,
              color: t.textDim)),
    );
  }
}

class _UserCard extends StatelessWidget {
  const _UserCard({required this.state, required this.onTap});

  final ShellState? state;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final u = state?.user;
    final emoji = u?.avatarEmoji ?? '';
    final photo = u?.avatarPath ?? '';
    final face = Container(
      width: 28,
      height: 28,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        color: Tokens.brand.withValues(alpha: 0.18),
        shape: BoxShape.circle,
      ),
      child: emoji.isEmpty
          ? Icon(context.skin.icon(Icons.person_outline),
              size: 15, color: t.text)
          : Text(emoji, style: const TextStyle(fontSize: 14)),
    );
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onTap,
        child: Container(
          padding: const EdgeInsets.all(9),
          decoration: context.skin
                  .surface(SurfaceRole.card, radius: Tokens.radiusMd) ??
              BoxDecoration(
                color: t.glass,
                borderRadius: BorderRadius.circular(Tokens.radiusMd),
                border: Border.all(color: t.glassBorder),
              ),
          child: Row(
            children: [
              // The photo when there is one; the emoji if the file is gone.
              photo.isEmpty
                  ? face
                  : ClipOval(
                      child: Image.file(
                        File(photo),
                        key: ValueKey(ShellController.instance.pictureEpoch),
                        width: 28,
                        height: 28,
                        fit: BoxFit.cover,
                        errorBuilder: (context, error, stack) => face,
                      ),
                    ),
              const SizedBox(width: 9),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(u?.displayName ?? 'Local user',
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 11.5,
                            fontWeight: FontWeight.w700,
                            color: t.text)),
                    Text(u?.secondary ?? '—',
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 9.5, color: t.textDim)),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

// ── health lamp ─────────────────────────────────────────────────────────────

/// Only the light: its words are the tooltip, and the page it opens.
class _StatusButton extends StatefulWidget {
  const _StatusButton({required this.controller});

  final ShellController controller;

  @override
  State<_StatusButton> createState() => _StatusButtonState();
}

class _StatusButtonState extends State<_StatusButton> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final tint = switch (c.statusLevel) {
      'ok' => const Color(0xFF22C55E),
      'busy' => const Color(0xFF10B981),
      'problem' => Tokens.error,
      _ => Tokens.secSettings,
    };
    return Tooltip(
      message: c.statusNote,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          // The dashboard is a Settings tab, as in the Slint build — the
          // sidebar's lamp is a lamp, not a tenth section — so it opens that
          // tab rather than whichever one Settings was last left on.
          onTap: () => c.goTab(Section.settings, 'status'),
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 120),
            height: 34,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: tint.withValues(alpha: _hover ? 0.20 : 0.10),
              borderRadius: BorderRadius.circular(10),
            ),
            child: Container(
              width: 10,
              height: 10,
              decoration: BoxDecoration(
                color: tint,
                shape: BoxShape.circle,
                boxShadow: [
                  BoxShadow(
                      color: tint.withValues(alpha: 0.6),
                      blurRadius: 8,
                      spreadRadius: 1),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// Lock now — the same as Ctrl+L.
class _LockBtn extends StatelessWidget {
  const _LockBtn();

  @override
  Widget build(BuildContext context) => _DockBtn(
        icon: Icons.lock_outline,
        accent: const Color(0xFF10B981),
        tip: 'Lock now · Ctrl+L',
        onTap: LockController.instance.lock,
      );
}

// ── the footer dock ─────────────────────────────────────────────────────────

class _Dock extends StatelessWidget {
  const _Dock({
    required this.controller,
    required this.collapsed,
    required this.onCycleTheme,
    required this.themeIcon,
  });

  final ShellController controller;
  final bool collapsed;
  final VoidCallback onCycleTheme;
  final IconData themeIcon;

  @override
  Widget build(BuildContext context) {
    final buttons = <Widget>[
      _DockBtn(
        icon: Icons.settings_outlined,
        accent: const Color(0xFF6366F1),
        active: controller.section == Section.settings,
        tip: 'Settings',
        onTap: () => controller.go(Section.settings),
      ),
      _DockBtn(
        icon: themeIcon,
        accent: const Color(0xFF14B8A6),
        tip: 'Theme',
        onTap: onCycleTheme,
      ),
      _DockBtn(
        icon: collapsed ? Icons.chevron_right : Icons.chevron_left,
        accent: const Color(0xFFEC4899),
        tip: collapsed ? 'Expand' : 'Collapse',
        onTap: controller.toggleCollapsed,
      ),
    ];
    if (collapsed) {
      return Column(
        children: [
          for (final b in buttons)
            Padding(padding: const EdgeInsets.only(bottom: 6), child: b),
        ],
      );
    }
    return Row(
      children: [
        for (var i = 0; i < buttons.length; i++) ...[
          if (i > 0) const SizedBox(width: 6),
          Expanded(child: buttons[i]),
        ],
      ],
    );
  }
}

class _DockBtn extends StatefulWidget {
  const _DockBtn({
    required this.icon,
    required this.accent,
    required this.tip,
    required this.onTap,
    this.active = false,
  });

  final IconData icon;
  final Color accent;
  final String tip;
  final VoidCallback onTap;
  final bool active;

  @override
  State<_DockBtn> createState() => _DockBtnState();
}

class _DockBtnState extends State<_DockBtn> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Tooltip(
      message: widget.tip,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 120),
            height: 34,
            alignment: Alignment.center,
            decoration: context.skin.control(
                  active: widget.active,
                  hovered: _hover,
                  tint: widget.accent,
                  radius: 10,
                ) ??
                BoxDecoration(
                  color: widget.active || _hover
                      ? widget.accent
                          .withValues(alpha: widget.active ? 0.20 : 0.12)
                      : t.glass,
                  borderRadius: BorderRadius.circular(10),
                ),
            child: Icon(context.skin.icon(widget.icon),
                size: 17,
                color: widget.active || _hover ? widget.accent : t.textDim),
          ),
        ),
      ),
    );
  }
}
