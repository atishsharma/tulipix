// Settings — nine tabs over one Settings file.
//
// A left rail of sections and one content stage, as in the Slint build. Six of
// the tabs draw the same widget over different rows: the panel does not know
// what a row means, only what kind it is, so a new setting is a row in Rust and
// nothing here.

import 'package:flutter/material.dart';

import '../../design/pick.dart';
import '../../design/first_load.dart';
import '../../design/tokens.dart';
import '../../design/skin.dart';
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
        // The status pill on the identity row goes to the Status tab, so the
        // page that owns the tab has to hand it the way back.
        'profile' => ProfileTab(
            controller: _c,
            state: st,
            onTab: (v) {
              setState(() => _tab = v);
              _c.send(SettingsCmd.setTab(tab: v));
            },
          ),
        'libraries' => LibrariesTab(controller: _c, state: st),
        // The dashboard, as its own tab — the same page the sidebar's lamp
        // reports on, not a second rendering of it.
        'status' => const StatusPage(visible: true),
        // Same row list as the other data-driven tabs, plus the one thing
        // that is not a setting: what each model wants from this machine.
        'ai' => _AiTab(controller: _c, state: st),
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
        // Under a skin the open item is the skin's latched key, drawn below.
        color: active && context.skin.isStandard
            ? Tokens.secSettings.withValues(alpha: 0.16)
            : Colors.transparent,
        borderRadius: BorderRadius.circular(10),
        child: InkWell(
          borderRadius: BorderRadius.circular(10),
          onTap: onTap,
          child: Container(
            decoration: active
                ? context.skin.control(
                    active: true, tint: Tokens.secSettings, radius: 10)
                : null,
            padding: const EdgeInsets.symmetric(horizontal: 11, vertical: 10),
            child: Row(
              children: [
                Icon(context.skin.icon(icon),
                    size: 17, color: active ? t.text : t.textDim),
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

/// The AI Features tab: the settings rows, with a "Requirements" button that
/// opens the per-model demands as a sheet.
///
/// The requirements are a sheet rather than more rows because they answer a
/// different question -- "can this machine run it" as against "do I want it
/// on" -- and inlining them pushed the actual switches below the fold.
class _AiTab extends StatelessWidget {
  const _AiTab({required this.controller, required this.state});

  final SettingsController controller;
  final SettingsState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 16, 24, 0),
          child: Row(
            children: [
              Expanded(
                child: Text(
                  'Models run on this computer. Nothing is sent anywhere '
                  'unless you switch on cloud help below.',
                  style: TextStyle(fontSize: 12, color: t.textDim),
                ),
              ),
              TextButton.icon(
                onPressed: () => showModalBottomSheet<void>(
                  context: context,
                  isScrollControlled: true,
                  builder: (_) =>
                      _RequirementsSheet(rows: state.aiRequirements),
                ),
                icon: const Icon(Icons.speed_outlined, size: 18),
                label: const Text('Requirements'),
              ),
            ],
          ),
        ),
        Expanded(child: _Panel(controller: controller, rows: state.ai)),
      ],
    );
  }
}

class _RequirementsSheet extends StatelessWidget {
  const _RequirementsSheet({required this.rows});

  final List<SettingItem> rows;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return SafeArea(
      child: ConstrainedBox(
        constraints: BoxConstraints(
          maxHeight: MediaQuery.of(context).size.height * 0.8,
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(24, 18, 12, 4),
              child: Row(
                children: [
                  Expanded(
                    child: Text('Requirements',
                        style: TextStyle(
                            fontSize: 17,
                            fontWeight: FontWeight.w800,
                            color: t.text)),
                  ),
                  IconButton(
                    onPressed: () => Navigator.of(context).pop(),
                    icon: const Icon(Icons.close),
                  ),
                ],
              ),
            ),
            Flexible(
              child: ListView.builder(
                padding: const EdgeInsets.fromLTRB(24, 0, 24, 24),
                shrinkWrap: true,
                itemCount: rows.length,
                // Every row here is a header or a reading -- none of them has
                // a key, so none of them can be clicked into a command.
                itemBuilder: (context, i) => _ReadOnlyRow(row: rows[i]),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _ReadOnlyRow extends StatelessWidget {
  const _ReadOnlyRow({required this.row});

  final SettingItem row;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (row.kind == 'header') {
      return Padding(
        padding: const EdgeInsets.fromLTRB(0, 20, 0, 6),
        child: Text(row.label,
            style: TextStyle(
                fontSize: 10.5,
                fontWeight: FontWeight.w800,
                letterSpacing: 1.0,
                color: t.textDim)),
      );
    }
    return _Frame(
      label: row.label,
      desc: row.desc,
      trailing: Text(row.value,
          style: TextStyle(
            fontSize: 12,
            fontWeight: FontWeight.w600,
            color: _stateColor(row.state, t),
          )),
    );
  }
}

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
            onPressed: r.state == 'busy'
                ? null
                : () => widget.controller.sendAction(r.key),
            child: r.state == 'busy'
                ? const SizedBox(
                    width: 14,
                    height: 14,
                    child: CircularProgressIndicator(strokeWidth: 2))
                : Text(r.value, style: const TextStyle(fontSize: 12)),
          ),
        ),
      // A status reading and its button are one row: the model's install state
      // and the thing you do about it describe the same object, and two rows
      // let them drift.
      'status-action' => _Frame(
          label: r.label,
          desc: r.desc,
          trailing: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (r.frac >= 0) ...[
                SizedBox(
                  width: 92,
                  child: LinearProgressIndicator(
                    value: r.frac.clamp(0.0, 1.0),
                    minHeight: 5,
                    backgroundColor: t.nInk2.withValues(alpha: 0.18),
                  ),
                ),
                const SizedBox(width: 8),
                Text('${(r.frac * 100).round()}%',
                    style: TextStyle(fontSize: 11.5, color: t.textDim)),
              ] else
                Text(r.value,
                    style: TextStyle(
                      fontSize: 12,
                      fontWeight: FontWeight.w600,
                      color: _stateColor(r.state, t),
                    )),
              const SizedBox(width: 12),
              OutlinedButton(
                onPressed: r.state == 'busy'
                    ? null
                    : () => widget.controller.sendAction(r.key),
                child: Text(r.btn, style: const TextStyle(fontSize: 12)),
              ),
            ],
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
                color: _stateColor(r.state, t),
              )),
        ),
    };
  }
}

Color _stateColor(String state, Tokens t) => switch (state) {
      'ok' => Tokens.ok,
      'warn' => Tokens.warn,
      'error' => Tokens.error,
      'busy' => Tokens.brand,
      _ => t.textDim,
    };

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
        Align(
          alignment: Alignment.centerLeft,
          child: FilledButton.icon(
            icon: const Icon(Icons.create_new_folder_outlined, size: 16),
            label: const Text('Watch a folder'),
            onPressed: () async {
              final path = await pickDirectory();
              if (path != null) _add(path);
            },
          ),
        ),
        const SizedBox(height: 8),
        Text('Everything under it is scanned and kept in step.',
            style: TextStyle(fontSize: 11, color: t.textDim)),
      ],
    );
  }

  void _add(String v) {
    final path = v.trim();
    if (path.isEmpty) return;
    widget.controller.send(SettingsCmd.libAdd(path: path));
  }
}
