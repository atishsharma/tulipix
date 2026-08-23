// You & Home — the first Settings tab.
//
// Both halves answer "what does the app look like when I open it", which is
// why the Slint build merged Profile, Appearance and Home Layout into one
// entry: choosing a layout and then theming it used to mean crossing the nav
// twice.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/settings.dart';
import 'settings_controller.dart';

class ProfileTab extends StatefulWidget {
  const ProfileTab({
    super.key,
    required this.controller,
    required this.state,
  });

  final SettingsController controller;
  final SettingsState state;

  @override
  State<ProfileTab> createState() => _ProfileTabState();
}

class _ProfileTabState extends State<ProfileTab> {
  late final TextEditingController _name =
      TextEditingController(text: widget.state.displayName);
  late final TextEditingController _emoji =
      TextEditingController(text: widget.state.avatarEmoji);
  late int _logo = widget.state.logoChoice;

  @override
  void dispose() {
    _name.dispose();
    _emoji.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    await widget.controller.send(SettingsCmd.saveProfile(
      name: _name.text,
      emoji: _emoji.text,
      logo: _logo,
    ));
    // The sidebar prints the same name and mark, so it has to hear about it.
    await ShellController.instance.refresh();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.state;
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 32),
      children: [
        const _Head('Profile'),
        Row(
          crossAxisAlignment: CrossAxisAlignment.end,
          children: [
            SizedBox(
              width: 280,
              child: TextField(
                controller: _name,
                style: TextStyle(fontSize: 13, color: t.text),
                decoration: const InputDecoration(
                  isDense: true,
                  border: OutlineInputBorder(),
                  labelText: 'Display name',
                ),
                onSubmitted: (_) => _save(),
              ),
            ),
            const SizedBox(width: 12),
            SizedBox(
              width: 96,
              child: TextField(
                controller: _emoji,
                style: TextStyle(fontSize: 13, color: t.text),
                decoration: const InputDecoration(
                  isDense: true,
                  border: OutlineInputBorder(),
                  labelText: 'Avatar',
                  hintText: '🙂',
                ),
                onSubmitted: (_) => _save(),
              ),
            ),
            const SizedBox(width: 12),
            FilledButton(onPressed: _save, child: const Text('Save')),
          ],
        ),
        const SizedBox(height: 6),
        Text('Shown on the sidebar, under the mark.',
            style: TextStyle(fontSize: 11.5, color: t.textDim)),
        const _Head('Appearance'),
        _Row(
          label: 'Theme',
          desc: 'System follows the desktop; extra-dark is the OLED tier.',
          trailing: DropdownButton<String>(
            value: const ['system', 'light', 'dark', 'extra-dark']
                    .contains(st.theme)
                ? st.theme
                : 'system',
            underline: const SizedBox.shrink(),
            items: const [
              DropdownMenuItem(value: 'system', child: Text('System')),
              DropdownMenuItem(value: 'light', child: Text('Light')),
              DropdownMenuItem(value: 'dark', child: Text('Dark')),
              DropdownMenuItem(value: 'extra-dark', child: Text('Extra dark')),
            ],
            onChanged: (v) => v == null
                ? null
                : widget.controller.send(SettingsCmd.setTheme(theme: v)),
          ),
        ),
        _Row(
          label: 'Reduce motion',
          desc: 'Drops the transitions that move things across the screen.',
          trailing: Switch(
            value: st.reduceMotion,
            activeThumbColor: Tokens.secSettings,
            onChanged: (v) =>
                widget.controller.send(SettingsCmd.setReduceMotion(on_: v)),
          ),
        ),
        _Row(
          label: 'Sidebar mark',
          desc: 'Which logo the sidebar wears.',
          trailing: DropdownButton<int>(
            value: _logo,
            underline: const SizedBox.shrink(),
            items: const [
              DropdownMenuItem(value: 0, child: Text('Default')),
              DropdownMenuItem(value: 1, child: Text('Colour')),
              DropdownMenuItem(value: 2, child: Text('Dark')),
              DropdownMenuItem(value: 3, child: Text('White')),
              DropdownMenuItem(value: 4, child: Text('India')),
            ],
            onChanged: (v) {
              if (v == null) return;
              setState(() => _logo = v);
              _save();
            },
          ),
        ),
        const _Head('Home layout'),
        Wrap(
          spacing: 10,
          runSpacing: 10,
          children: [
            for (final l in kHomeLayouts)
              _LayoutChip(
                layout: l,
                active: st.homeLayout == l.id,
                onTap: () => widget.controller
                    .send(SettingsCmd.setHomeLayout(layout: l.id)),
              ),
          ],
        ),
        const SizedBox(height: 18),
        Row(
          children: [
            Text('Cards on the Home page',
                style: TextStyle(
                    fontSize: 13, fontWeight: FontWeight.w600, color: t.text)),
            const Spacer(),
            TextButton(
              onPressed: () =>
                  widget.controller.send(const SettingsCmd.homeCardsReset()),
              child: const Text('Reset', style: TextStyle(fontSize: 12)),
            ),
          ],
        ),
        for (final card in st.homeCards)
          _Row(
            label: card.label,
            desc: '',
            trailing: Switch(
              value: card.on_,
              activeThumbColor: Tokens.secSettings,
              onChanged: (v) => widget.controller
                  .send(SettingsCmd.homeCardSet(key: card.key, on_: v)),
            ),
          ),
        const _Head('About'),
        _Row(
          label: 'Tulipix',
          desc: 'The Flutter build of the shell, running beside the Slint one.',
          trailing: Text(st.appVersion,
              style: TextStyle(fontSize: 12, color: t.textDim)),
        ),
      ],
    );
  }
}

class _Head extends StatelessWidget {
  const _Head(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.fromLTRB(0, 22, 0, 10),
        child: Text(text.toUpperCase(),
            style: TextStyle(
                fontSize: 10.5,
                fontWeight: FontWeight.w800,
                letterSpacing: 1.0,
                color: context.tokens.textDim)),
      );
}

class _Row extends StatelessWidget {
  const _Row({
    required this.label,
    required this.desc,
    required this.trailing,
  });

  final String label;
  final String desc;
  final Widget trailing;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 7),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(label,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w600,
                        color: t.text)),
                if (desc.isNotEmpty) ...[
                  const SizedBox(height: 2),
                  Text(desc,
                      style: TextStyle(fontSize: 11.5, color: t.textDim)),
                ],
              ],
            ),
          ),
          const SizedBox(width: 20),
          trailing,
        ],
      ),
    );
  }
}

class _LayoutChip extends StatelessWidget {
  const _LayoutChip({
    required this.layout,
    required this.active,
    required this.onTap,
  });

  final ({String id, String label, String note}) layout;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: active ? Tokens.brand.withValues(alpha: 0.14) : t.panel,
      borderRadius: BorderRadius.circular(Tokens.radiusMd),
      child: InkWell(
        borderRadius: BorderRadius.circular(Tokens.radiusMd),
        onTap: onTap,
        child: Container(
          width: 176,
          padding: const EdgeInsets.all(13),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(Tokens.radiusMd),
            border: Border.all(
                color:
                    active ? Tokens.brand.withValues(alpha: 0.5) : t.outline),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(layout.label,
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      color: t.text)),
              const SizedBox(height: 3),
              Text(layout.note,
                  style: TextStyle(fontSize: 11, color: t.textDim)),
            ],
          ),
        ),
      ),
    );
  }
}
