// Settings — nine tabs over one Settings file.
//
// A left rail of sections and one content stage, as in the Slint build. Six of
// the tabs draw the same widget over different rows: the panel does not know
// what a row means, only what kind it is, so a new setting is a row in Rust and
// nothing here.

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/settings.dart';
import '../status/status_page.dart';
import 'settings_controller.dart';
import 'settings_profile.dart';

class SettingsPage extends StatefulWidget {
  const SettingsPage({super.key, required this.visible});

  final bool visible;

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends State<SettingsPage> {
  final SettingsController _c = SettingsController();
  String _tab = 'profile';

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (widget.visible) _c.refresh();
    });
  }

  @override
  void didUpdateWidget(SettingsPage old) {
    super.didUpdateWidget(old);
    // Tool probes, cache sizes and the watched list all go stale while you are
    // elsewhere, so arriving is when to re-read them.
    if (widget.visible && !old.visible) _c.refresh();
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        final st = _c.state;
        return ColoredBox(
          color: t.bg,
          child: Row(
            children: [
              _Rail(
                tab: _tab,
                onTab: (v) {
                  setState(() => _tab = v);
                  _c.send(SettingsCmd.setTab(tab: v));
                },
              ),
              Container(width: 1, color: t.outline),
              Expanded(
                child: st == null
                    ? FirstLoad(error: _c.error, onRetry: _c.refresh)
                    : Column(
                        children: [
                          if (_c.error != null)
                            _Bar(
                              text: '${_c.error}',
                              tint: Tokens.error,
                              onClose: _c.clearError,
                            ),
                          if (_c.notice.isNotEmpty)
                            _Bar(
                              text: _c.notice,
                              tint: Tokens.secSettings,
                              onClose: _c.clearNotice,
                            ),
                          Expanded(child: _stage(st)),
                        ],
                      ),
              ),
            ],
          ),
        );
      },
    );
  }

  Widget _stage(SettingsState st) => switch (_tab) {
        'profile' => ProfileTab(controller: _c, state: st),
        'libraries' => LibrariesTab(controller: _c, state: st),
        // The dashboard, as its own tab — the same page the sidebar's lamp
        // reports on, not a second rendering of it.
        'status' => const StatusPage(visible: true),
        _ => _Panel(controller: _c, rows: _c.rowsFor(_tab)),
      };
}

class _Rail extends StatelessWidget {
  const _Rail({required this.tab, required this.onTab});

  final String tab;
  final ValueChanged<String> onTab;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SizedBox(
      width: 208,
      child: ListView(
        padding: const EdgeInsets.symmetric(vertical: 16, horizontal: 10),
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(8, 0, 8, 14),
            child: Text('Settings',
                style: TextStyle(
                    fontSize: 19, fontWeight: FontWeight.w800, color: t.text)),
          ),
          for (final item in kSettingsTabs)
            _RailItem(
              label: item.label,
              icon: item.icon,
              active: tab == item.id,
              onTap: () => onTab(item.id),
            ),
        ],
      ),
    );
  }
}

class _RailItem extends StatelessWidget {
  const _RailItem({
    required this.label,
    required this.icon,
    required this.active,
    required this.onTap,
  });

  final String label;
  final IconData icon;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(bottom: 4),
      child: Material(
        color: active
            ? Tokens.secSettings.withValues(alpha: 0.16)
            : Colors.transparent,
        borderRadius: BorderRadius.circular(10),
        child: InkWell(
          borderRadius: BorderRadius.circular(10),
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 11, vertical: 10),
            child: Row(
              children: [
                Icon(icon, size: 17, color: active ? t.text : t.textDim),
                const SizedBox(width: 10),
                Expanded(
                  child: Text(label,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 12.5,
                          fontWeight:
                              active ? FontWeight.w700 : FontWeight.w500,
                          color: active ? t.text : t.textDim)),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _Bar extends StatelessWidget {
  const _Bar({required this.text, required this.tint, required this.onClose});

  final String text;
  final Color tint;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: tint.withValues(alpha: 0.12),
      child: ListTile(
        dense: true,
        leading: Icon(Icons.info_outline, color: tint, size: 18),
        title: Text(text, style: TextStyle(fontSize: 12.5, color: t.text)),
        trailing: IconButton(
          icon: const Icon(Icons.close, size: 16),
          onPressed: onClose,
        ),
      ),
    );
  }
}

// ── the data-driven panel ───────────────────────────────────────────────────

class _Panel extends StatelessWidget {
  const _Panel({required this.controller, required this.rows});

  final SettingsController controller;
  final List<SettingItem> rows;

  @override
  Widget build(BuildContext context) {
    if (rows.isEmpty) {
      return Center(
        child: Text('Nothing here yet.',
            style: TextStyle(fontSize: 13, color: context.tokens.textDim)),
      );
    }
    return ListView.builder(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 32),
      itemCount: rows.length,
      itemBuilder: (context, i) =>
          SettingRow(controller: controller, row: rows[i]),
    );
  }
}

/// One row of a panel. The five kinds are the whole vocabulary: a header, a
/// switch, a text box, a read-only reading, an action button, or a choice.
class SettingRow extends StatefulWidget {
  const SettingRow({super.key, required this.controller, required this.row});

  final SettingsController controller;
  final SettingItem row;

  @override
  State<SettingRow> createState() => _SettingRowState();
}

class _SettingRowState extends State<SettingRow> {
  late final TextEditingController _text =
      TextEditingController(text: widget.row.value);

  @override
  void didUpdateWidget(SettingRow old) {
    super.didUpdateWidget(old);
    // A refresh that did not change this value must not move the caret out
    // from under someone still typing in it.
    if (widget.row.value != old.row.value && widget.row.value != _text.text) {
      _text.text = widget.row.value;
    }
  }

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final r = widget.row;
    return switch (r.kind) {
      'header' => Padding(
          padding: const EdgeInsets.fromLTRB(0, 22, 0, 8),
          child: Text(r.label,
              style: TextStyle(
                  fontSize: 10.5,
                  fontWeight: FontWeight.w800,
                  letterSpacing: 1.0,
                  color: t.textDim)),
        ),
      'toggle' => _Frame(
          label: r.label,
          desc: r.desc,
          trailing: Switch(
            value: r.on_,
            activeThumbColor: Tokens.secSettings,
            onChanged: (v) =>
                widget.controller.send(SettingsCmd.toggle(key: r.key, on_: v)),
          ),
        ),
      'text' => _Frame(
          label: r.label,
          desc: r.desc,
          trailing: SizedBox(
            width: 280,
            child: TextField(
              controller: _text,
              style: TextStyle(fontSize: 12.5, color: t.text),
              decoration: const InputDecoration(
                isDense: true,
                border: OutlineInputBorder(),
              ),
              // On submit, not on change: every keystroke would be a settings
              // file write.
              onSubmitted: (v) => widget.controller
                  .send(SettingsCmd.setText(key: r.key, value: v)),
            ),
          ),
        ),
      'choice' => _Frame(
          label: r.label,
          desc: r.desc,
          trailing: DropdownButton<String>(
            value: r.options.contains(r.value) ? r.value : r.options.first,
            underline: const SizedBox.shrink(),
            items: [
              for (final o in r.options)
                DropdownMenuItem(
                    value: o,
                    child: Text(o, style: const TextStyle(fontSize: 12.5))),
            ],
            onChanged: (v) => v == null
                ? null
                : widget.controller
                    .send(SettingsCmd.setText(key: r.key, value: v)),
          ),
        ),
      'action' => _Frame(
          label: r.label,
          desc: r.desc,
          trailing: OutlinedButton(
            onPressed: () =>
                widget.controller.send(SettingsCmd.action(key: r.key)),
            child: Text(r.value, style: const TextStyle(fontSize: 12)),
          ),
        ),
      // status
      _ => _Frame(
          label: r.label,
          desc: r.desc,
          trailing: Text(r.value,
              style: TextStyle(
                fontSize: 12,
                fontWeight: FontWeight.w600,
                color: switch (r.state) {
                  'ok' => Tokens.ok,
                  'warn' => Tokens.warn,
                  'error' => Tokens.error,
                  'busy' => Tokens.brand,
                  _ => t.textDim,
                },
              )),
        ),
    };
  }
}

class _Frame extends StatelessWidget {
  const _Frame({
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

// ── libraries ───────────────────────────────────────────────────────────────

class LibrariesTab extends StatefulWidget {
  const LibrariesTab({
    super.key,
    required this.controller,
    required this.state,
  });

  final SettingsController controller;
  final SettingsState state;

  @override
  State<LibrariesTab> createState() => _LibrariesTabState();
}

class _LibrariesTabState extends State<LibrariesTab> {
  final TextEditingController _path = TextEditingController();

  @override
  void dispose() {
    _path.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final rows = widget.state.libraries;
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 32),
      children: [
        Text('Watched folders',
            style: TextStyle(
                fontSize: 17, fontWeight: FontWeight.w800, color: t.text)),
        const SizedBox(height: 6),
        Text(
            'These are the only paths Tulipix reads. Nothing outside them is '
            'scanned, and nothing inside them is ever moved or rewritten.',
            style: TextStyle(fontSize: 12, color: t.textDim)),
        const SizedBox(height: 16),
        if (rows.isEmpty)
          Text('No folders yet. Add one below and run a rescan.',
              style: TextStyle(fontSize: 12.5, color: t.textDim)),
        for (final r in rows)
          ListTile(
            dense: true,
            contentPadding: EdgeInsets.zero,
            leading: Icon(
                r.exists ? Icons.folder_outlined : Icons.folder_off_outlined,
                size: 20,
                color: r.exists ? Tokens.secBooks : Tokens.error),
            title:
                Text(r.path, style: TextStyle(fontSize: 12.5, color: t.text)),
            subtitle: r.exists
                ? null
                : const Text('No longer on disk',
                    style: TextStyle(fontSize: 11, color: Tokens.error)),
            trailing: IconButton(
              tooltip: 'Stop watching',
              icon: const Icon(Icons.close, size: 16),
              onPressed: () =>
                  widget.controller.send(SettingsCmd.libRemove(path: r.path)),
            ),
          ),
        const SizedBox(height: 18),
        Row(
          children: [
            Expanded(
              child: TextField(
                controller: _path,
                style: TextStyle(fontSize: 12.5, color: t.text),
                decoration: const InputDecoration(
                  isDense: true,
                  border: OutlineInputBorder(),
                  hintText: '/home/you/Media',
                  labelText: 'Add a folder',
                ),
                onSubmitted: _add,
              ),
            ),
            const SizedBox(width: 10),
            FilledButton(
              onPressed: () => _add(_path.text),
              child: const Text('Watch'),
            ),
          ],
        ),
        const SizedBox(height: 8),
        Text(
            'A typed path, not a file dialog: the picker belongs to the shell '
            'and is not ported yet.',
            style: TextStyle(fontSize: 11, color: t.textDim)),
      ],
    );
  }

  void _add(String v) {
    final path = v.trim();
    if (path.isEmpty) return;
    _path.clear();
    widget.controller.send(SettingsCmd.libAdd(path: path));
  }
}
