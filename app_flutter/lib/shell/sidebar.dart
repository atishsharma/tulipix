// The sidebar — a port of ui/sidebar.slint.
//
// Two widths, not a drawer: collapsed is a 68px icon rail, expanded is a
// labelled column, and the transition is the width animating rather than a
// panel sliding over the page. What is on it, top to bottom: the brand mark,
// Home, the eight Applications, the health lamp, and a footer dock of four
// buttons (Settings · Lock · Theme · Collapse).
//
// The active row is a double outline — an outer ring in the section accent and
// an inner ring in neutral ink. No cast shadow under a nav row in any theme:
// the active row is pressed *into* the shell, and a recess that also floats
// reads as neither.

import 'package:flutter/material.dart';

import '../design/tokens.dart';
import '../src/rust/api/shell.dart';
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
  Section.settings: (label: 'Settings', icon: Icons.settings_outlined),
};

/// The section's accent. `Tokens.accentOf` is the table; this is the name the
/// three Home layouts and the sidebar reach for.
Color accentFor(Section s) => Tokens.accentOf(s);

/// The eight sections under the APPLICATIONS header, in sidebar order.
const List<Section> kApplications = [
  Section.photos,
  Section.videos,
  Section.music,
  Section.books,
  Section.cloud,
  Section.tools,
  Section.transfer,
  Section.finances,
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
    return AnimatedContainer(
      duration: const Duration(milliseconds: 160),
      curve: Curves.easeOut,
      width: collapsed ? kSidebarCollapsed : kSidebarExpanded,
      margin: const EdgeInsets.fromLTRB(8, 8, 0, 8),
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(Tokens.radiusLg),
        border: Border.all(color: t.outline),
      ),
      child: Padding(
        padding: EdgeInsets.all(collapsed ? 10 : 12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _Brand(collapsed: collapsed),
            const SizedBox(height: 14),
            _NavRow(
              section: Section.home,
              active: c.section == Section.home,
              collapsed: collapsed,
              onTap: () => c.go(Section.home),
            ),
            _GroupHeader(collapsed: collapsed),
            // A scroller, not a fixed column: at 700px tall with eight rows and
            // the footer dock, the last application is otherwise cut off.
            Expanded(
              child: ListView(
                padding: EdgeInsets.zero,
                children: [
                  for (final s in kApplications)
                    _NavRow(
                      section: s,
                      active: c.section == s,
                      collapsed: collapsed,
                      badge:
                          s == Section.finances ? (st?.financesBadge ?? 0) : 0,
                      alarm: st?.financesOverdue ?? false,
                      onTap: () => c.go(s),
                    ),
                ],
              ),
            ),
            const SizedBox(height: 8),
            Container(height: 1, color: t.outline),
            const SizedBox(height: 8),
            _StatusButton(controller: c, collapsed: collapsed),
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
  });

  final Section section;
  final bool active;
  final bool collapsed;
  final VoidCallback onTap;

  /// Count of things wanting attention. 0 hides the badge. Only Finances sets
  /// one today; it lives on the row rather than on that one section so a second
  /// section wanting a count needs no second mechanism.
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
    final h = widget.collapsed ? 46.0 : 52.0;
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
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
            decoration: BoxDecoration(
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
            ),
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
                              color: widget.active ? accent : t.text,
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
    final icon = Icon(
      kSectionMeta[widget.section]!.icon,
      size: widget.collapsed ? 21 : 19,
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
      child: Text('$count',
          style: TextStyle(
              fontSize: small ? 9 : 10.5,
              fontWeight: FontWeight.w800,
              color: Colors.white)),
    );
  }
}

// ── brand, group header, user ───────────────────────────────────────────────

/// The mark is drawn rather than an asset: the five logo SVGs the Slint build
/// picks between live in `resources/` and are not in the Flutter bundle yet, so
/// `profile.logo` round-trips through Settings without changing this yet.
class _Brand extends StatelessWidget {
  const _Brand({required this.collapsed});

  final bool collapsed;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final mark = Container(
      width: 34,
      height: 34,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        gradient: const LinearGradient(
          colors: [Tokens.brand, Tokens.brand2],
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
        ),
        borderRadius: BorderRadius.circular(10),
      ),
      child: const Text('T',
          style: TextStyle(
              fontSize: 18, fontWeight: FontWeight.w900, color: Colors.white)),
    );
    if (collapsed) return Center(child: mark);
    return Row(
      children: [
        mark,
        const SizedBox(width: 10),
        Expanded(
          child: Text('Tulipix',
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 16, fontWeight: FontWeight.w800, color: t.text)),
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
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onTap,
        child: Container(
          padding: const EdgeInsets.all(9),
          decoration: BoxDecoration(
            color: t.glass,
            borderRadius: BorderRadius.circular(Tokens.radiusMd),
            border: Border.all(color: t.glassBorder),
          ),
          child: Row(
            children: [
              Container(
                width: 28,
                height: 28,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: Tokens.brand.withValues(alpha: 0.18),
                  shape: BoxShape.circle,
                ),
                child: emoji.isEmpty
                    ? Icon(Icons.person_outline, size: 15, color: t.text)
                    : Text(emoji, style: const TextStyle(fontSize: 14)),
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

class _StatusButton extends StatelessWidget {
  const _StatusButton({required this.controller, required this.collapsed});

  final ShellController controller;
  final bool collapsed;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = switch (controller.statusLevel) {
      'ok' => const Color(0xFF22C55E),
      'busy' => const Color(0xFF10B981),
      'problem' => Tokens.error,
      _ => Tokens.secSettings,
    };
    final dot = Container(
      width: 9,
      height: 9,
      decoration: BoxDecoration(color: tint, shape: BoxShape.circle),
    );
    return Tooltip(
      message: controller.statusNote,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        child: GestureDetector(
          // The dashboard is a Settings tab, as in the Slint build — the
          // sidebar's lamp is a lamp, not a tenth section.
          onTap: () => controller.go(Section.settings),
          child: Container(
            height: collapsed ? 34 : 40,
            padding: EdgeInsets.symmetric(horizontal: collapsed ? 0 : 11),
            decoration: BoxDecoration(
              color: tint.withValues(alpha: 0.10),
              borderRadius: BorderRadius.circular(Tokens.radiusMd),
            ),
            child: collapsed
                ? Center(child: dot)
                : Row(
                    children: [
                      dot,
                      const SizedBox(width: 9),
                      Expanded(
                        child: Text(controller.statusNote,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 10.5, color: t.textDim)),
                      ),
                    ],
                  ),
          ),
        ),
      ),
    );
  }
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
            decoration: BoxDecoration(
              color: widget.active || _hover
                  ? widget.accent.withValues(alpha: widget.active ? 0.20 : 0.12)
                  : t.glass,
              borderRadius: BorderRadius.circular(10),
            ),
            child: Icon(widget.icon,
                size: 17,
                color: widget.active || _hover ? widget.accent : t.textDim),
          ),
        ),
      ),
    );
  }
}
