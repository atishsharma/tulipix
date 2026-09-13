// Settings › Security, Backup & Data and Advanced, as tiles over the rows
// Rust already sends (docs/mockups/settings-tabs.html).
//
// Two things here are new: a backup can be brought back (Restore), and the
// reset that used to sit in Status's library popup lives in Backup & Data,
// next to the backups that make it safe to press.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/app_mark.dart';
import '../../design/pick.dart';
import '../../design/tokens.dart';
import '../../shell/lock/lock_controller.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/settings.dart';
import '../../src/rust/api/status.dart';
import 'settings_controller.dart';
import 'settings_kit.dart';

const List<String> _weekdays = [
  'Monday',
  'Tuesday',
  'Wednesday',
  'Thursday',
  'Friday',
  'Saturday',
  'Sunday',
];
const List<String> _months = [
  'January',
  'February',
  'March',
  'April',
  'May',
  'June',
  'July',
  'August',
  'September',
  'October',
  'November',
  'December',
];

String _two(int n) => n.toString().padLeft(2, '0');

/// "Thursday, 10 September · 14:02", with the year when it is not this one.
String _when(int secs) {
  final d = DateTime.fromMillisecondsSinceEpoch(secs * 1000);
  final year = d.year == DateTime.now().year ? '' : ' ${d.year}';
  return '${_weekdays[d.weekday - 1]}, ${d.day} ${_months[d.month - 1]}$year'
      ' · ${_two(d.hour)}:${_two(d.minute)}';
}

/// Today, Yesterday, 3 days ago — by calendar day, not by 24 hours.
String _ago(int secs) {
  final d = DateTime.fromMillisecondsSinceEpoch(secs * 1000);
  final now = DateTime.now();
  final days = DateTime(now.year, now.month, now.day)
      .difference(DateTime(d.year, d.month, d.day))
      .inDays;
  return switch (days) {
    <= 0 => 'Today',
    1 => 'Yesterday',
    < 30 => '$days days ago',
    _ => '${d.day} ${_months[d.month - 1]}',
  };
}

Widget _note(BuildContext context, String text) => Text(text,
    style: TextStyle(fontSize: 11.5, color: context.tokens.textDim));

// ── security ────────────────────────────────────────────────────────────────

class SecurityTab extends StatelessWidget {
  const SecurityTab({super.key, required this.controller, required this.state});

  final SettingsController controller;
  final SettingsState state;

  static const _face = [
    'lock.show-music',
    'lock.media-controls',
    'lock.show-lyrics',
    'lock.show-video',
    'lock.show-glance',
    'lock.motion',
  ];

  /// "10 minutes" reads as "10 min" on a button.
  static String _short(String o) =>
      o.replaceAll(' minutes', ' min').replaceAll(' minute', ' min');

  /// A PIN is typed into a dialog, never into a settings row. What comes back
  /// goes straight to Rust, which salts and hashes it.
  Future<void> _setPin(BuildContext context) async {
    final pin = await showDialog<String>(
      context: context,
      builder: (_) => const PinDialog(),
    );
    if (pin != null) {
      await controller.send(SettingsCmd.setText(key: 'lock.pin', value: pin));
    }
  }

  Future<void> _pickWallpapers() async {
    final dir = await pickDirectory();
    if (dir != null) {
      await controller
          .send(SettingsCmd.setText(key: 'lock.wallpapers', value: dir));
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = controller;
    final rows = Rows(state.security);
    final auto = rows.key('autolock');
    final after = rows.key('lock.after');
    final pin = rows.key('lock-pin');
    rows.use(['lock-pin-clear', 'lock-wallpapers-browse']);
    final wall = rows.key('lock.wallpapers');
    final passkey = rows.key('passkey');
    final enc = rows.key('db-encrypt');
    final face = _face.map(rows.key).whereType<SettingItem>().toList();
    final rest = rows.rest;

    final hasPin = pin?.state == 'ok';
    bool on(String k) => face.any((r) => r.key == k && r.on_);
    void toggle(SettingItem r, bool v) =>
        c.send(SettingsCmd.toggle(key: r.key, on_: v));

    return SettingsPageBody(
      head: SettingsHead.forTab(
        'security',
        note: 'The lock screen, its PIN, and what it shows while you are away',
        actions: [
          SmallBtn(
            label: 'Lock now',
            icon: Icons.lock_outline,
            large: true,
            tooltip: 'Ctrl+L',
            onTap: LockController.instance.lock,
          ),
        ],
      ),
      children: [
        TileGrid([
          if (auto != null)
            (
              span: 4,
              child: SettingsTile(
                icon: Icons.lock_clock_outlined,
                tint: Tokens.secBooks,
                title: 'Lock when idle',
                note: 'After a while without a key press or the pointer moving',
                trailing: [
                  SettingSwitch(on: auto.on_, onChanged: (v) => toggle(auto, v)),
                ],
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    if (after != null)
                      // Still settable while off, so it is ready when it is
                      // switched on; dimmed, because it does nothing yet.
                      Opacity(
                        opacity: auto.on_ ? 1 : 0.5,
                        child: Seg(
                          full: true,
                          options: after.options,
                          labels: [for (final o in after.options) _short(o)],
                          value: after.value,
                          onPick: (v) => c.send(
                              SettingsCmd.setText(key: 'lock.after', value: v)),
                        ),
                      ),
                    const SizedBox(height: 10),
                    _note(
                        context,
                        'It dims and counts down for ten seconds first. A '
                        'playing video never locks, and music keeps playing.'),
                  ],
                ),
              ),
            ),
          if (pin != null)
            (
              span: 2,
              child: SettingsTile(
                icon: Icons.key_outlined,
                tint: Tokens.secBooks,
                title: 'PIN',
                note: 'Four to eight digits',
                trailing: [
                  StateChip(hasPin ? 'Set' : 'Not set',
                      tint: hasPin ? Tokens.ok : null),
                ],
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Container(
                      padding: const EdgeInsets.fromLTRB(14, 10, 10, 10),
                      decoration: BoxDecoration(
                        color: t.bg,
                        borderRadius: BorderRadius.circular(14),
                        border: Border.all(color: t.outline),
                      ),
                      child: Row(
                        children: [
                          for (var i = 0; i < 4; i++)
                            Container(
                              width: 10,
                              height: 10,
                              margin: const EdgeInsets.only(right: 7),
                              decoration: BoxDecoration(
                                color: hasPin ? t.text : null,
                                shape: BoxShape.circle,
                                border: hasPin
                                    ? null
                                    : Border.all(color: t.textDim),
                              ),
                            ),
                          const Spacer(),
                          SmallBtn(
                            label: hasPin ? 'Change' : 'Set a PIN',
                            primary: !hasPin,
                            onTap: () => _setPin(context),
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(height: 8),
                    if (hasPin)
                      SmallBtn(
                        label: 'Remove the PIN',
                        ghost: true,
                        onTap: () => c.sendAction('lock-pin-clear'),
                      )
                    else
                      _note(context, 'Without one, a click unlocks.'),
                  ],
                ),
              ),
            ),
          if (face.isNotEmpty)
            (
              span: 4,
              child: SettingsTile(
                icon: Icons.wallpaper_outlined,
                tint: Tokens.secMusic,
                title: 'On the lock screen',
                note: 'What shows, and what works, while it is locked',
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    SizedBox(
                      width: 200,
                      child: _LockPreview(
                        music: on('lock.show-music'),
                        smoke: on('lock.motion'),
                      ),
                    ),
                    const SizedBox(width: 16),
                    Expanded(
                      child: Lines([
                        for (final r in face) RowLine(row: r, controller: c),
                      ]),
                    ),
                  ],
                ),
              ),
            ),
          // The wallpapers, and under them the two protections: each alone
          // was a tile with one switch and a lot of room.
          if (wall != null || passkey != null || enc != null)
            (
              span: 2,
              child: SettingsTile(
                icon: Icons.photo_library_outlined,
                tint: Tokens.secTransfer,
                title: 'Wallpapers & protection',
                note: 'The picture when nothing plays, and the locks',
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    if (wall != null) ...[
                    Container(
                      padding: const EdgeInsets.fromLTRB(12, 10, 12, 10),
                      decoration: BoxDecoration(
                        color: t.bg,
                        borderRadius: BorderRadius.circular(12),
                        border: Border.all(color: t.outline),
                      ),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text('Lock screen folder',
                              style: TextStyle(
                                  fontSize: 12.5,
                                  fontWeight: FontWeight.w600,
                                  color: t.text)),
                          Text(
                              wall.value.isEmpty
                                  ? 'None — the smoke gradient'
                                  : tildePath(wall.value),
                              maxLines: 2,
                              overflow: TextOverflow.ellipsis,
                              style:
                                  TextStyle(fontSize: 11, color: t.textDim)),
                        ],
                      ),
                    ),
                    const SizedBox(height: 10),
                    Row(
                      children: [
                        SmallBtn(
                          label: 'Choose',
                          icon: Icons.folder_open_outlined,
                          onTap: _pickWallpapers,
                        ),
                        if (wall.value.isNotEmpty) ...[
                          const SizedBox(width: 6),
                          SmallBtn(
                            label: 'Clear',
                            ghost: true,
                            onTap: () => c.send(SettingsCmd.setText(
                                key: 'lock.wallpapers', value: '')),
                          ),
                        ],
                      ],
                    ),
                    const SizedBox(height: 10),
                    _note(context,
                        'The first ten pictures, one every twelve seconds, '
                        'with a slow zoom.'),
                    ],
                    if (passkey != null || enc != null) ...[
                      const SizedBox(height: 6),
                      Lines([
                        if (passkey != null)
                          SettingLine(
                            title: 'Unlock with a passkey',
                            note: 'This lock screen does not use it yet; it '
                                'asks for the PIN',
                            trailing: SettingSwitch(
                                on: passkey.on_,
                                onChanged: (v) => toggle(passkey, v)),
                          ),
                        if (enc != null)
                          SettingLine(
                            title: 'Encrypt the library database',
                            note: 'From the next start. Protects the index if '
                                'the disk is stolen; your files are never '
                                'touched',
                            trailing: SettingSwitch(
                                on: enc.on_, onChanged: (v) => toggle(enc, v)),
                          ),
                      ]),
                    ],
                  ],
                ),
              ),
            ),
        ]),
        if (rest.isNotEmpty) MoreTile(rows: rest, controller: c),
      ],
    );
  }
}

/// The lock screen in small, following the switches beside it: the playing
/// card goes with What's playing, the smoke dims with Moving smoke. Drawn
/// still — the real one animates, this one has no reason to.
class _LockPreview extends StatelessWidget {
  const _LockPreview({required this.music, required this.smoke});

  final bool music;
  final bool smoke;

  static Widget _glow(Alignment at, double radius, Color c) => Positioned.fill(
        child: DecoratedBox(
          decoration: BoxDecoration(
            gradient: RadialGradient(
              center: at,
              radius: radius,
              colors: [c, c.withValues(alpha: 0)],
            ),
          ),
        ),
      );

  @override
  Widget build(BuildContext context) {
    final now = DateTime.now();
    return ClipRRect(
      borderRadius: BorderRadius.circular(14),
      child: Container(
        height: 150,
        color: const Color(0xFF07060B),
        child: Stack(
          children: [
            Positioned.fill(
              child: Opacity(
                opacity: smoke ? 1 : 0.5,
                child: Stack(
                  children: [
                    _glow(const Alignment(-0.5, 0.9), 0.9,
                        const Color(0x8CEC4899)),
                    _glow(const Alignment(0.5, 1), 1, const Color(0x997C3AED)),
                    _glow(const Alignment(0.1, 0.4), 0.7,
                        const Color(0x4D22D3EE)),
                  ],
                ),
              ),
            ),
            Positioned(
              left: 14,
              top: 12,
              child: Text('${_two(now.hour)}:${_two(now.minute)}',
                  style: const TextStyle(
                      fontSize: 34,
                      fontWeight: FontWeight.w300,
                      color: Colors.white,
                      height: 1)),
            ),
            Positioned(
              left: 15,
              top: 52,
              child: Text(
                  '${_weekdays[now.weekday - 1]}, ${now.day} '
                  '${_months[now.month - 1]}',
                  style:
                      const TextStyle(fontSize: 10.5, color: Colors.white70)),
            ),
            if (music)
              Positioned(
                left: 10,
                right: 10,
                bottom: 10,
                child: Container(
                  padding: const EdgeInsets.all(7),
                  decoration: BoxDecoration(
                    color: const Color(0x73101016),
                    borderRadius: BorderRadius.circular(10),
                    border: Border.all(color: const Color(0x1FFFFFFF)),
                  ),
                  child: Row(
                    children: [
                      Container(
                        width: 26,
                        height: 26,
                        decoration: BoxDecoration(
                          borderRadius: BorderRadius.circular(6),
                          gradient: const LinearGradient(
                              colors: [Color(0xFF7C3AED), Color(0xFFDB2777)]),
                        ),
                      ),
                      const SizedBox(width: 8),
                      const Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            Text('Now playing',
                                maxLines: 1,
                                style: TextStyle(
                                    fontSize: 11,
                                    fontWeight: FontWeight.w600,
                                    color: Colors.white)),
                            Text('Title · Artist',
                                maxLines: 1,
                                style: TextStyle(
                                    fontSize: 9.5, color: Colors.white70)),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// Asks for the PIN twice. What comes back goes straight to Rust, which salts
/// and hashes it; nothing here keeps it.
class PinDialog extends StatefulWidget {
  const PinDialog({super.key});

  @override
  State<PinDialog> createState() => _PinDialogState();
}

class _PinDialogState extends State<PinDialog> {
  final TextEditingController _a = TextEditingController();
  final TextEditingController _b = TextEditingController();
  String _error = '';

  @override
  void dispose() {
    _a.dispose();
    _b.dispose();
    super.dispose();
  }

  void _ok() {
    final a = _a.text;
    if (a.length < 4 || a.length > 8) {
      setState(() => _error = 'A PIN is four to eight digits.');
      return;
    }
    if (a != _b.text) {
      setState(() => _error = 'The two PINs are not the same.');
      return;
    }
    Navigator.pop(context, a);
  }

  Widget _field(TextEditingController c, String label,
          {bool autofocus = false, ValueChanged<String>? onSubmitted}) =>
      TextField(
        controller: c,
        autofocus: autofocus,
        obscureText: true,
        maxLength: 8,
        keyboardType: TextInputType.number,
        inputFormatters: [FilteringTextInputFormatter.digitsOnly],
        onSubmitted: onSubmitted,
        decoration: InputDecoration(
          labelText: label,
          counterText: '',
          border: const OutlineInputBorder(),
        ),
      );

  @override
  Widget build(BuildContext context) => AlertDialog(
        title: const Text('Lock screen PIN'),
        content: SizedBox(
          width: 300,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text('Four to eight digits, asked for when the app is locked.',
                  style:
                      TextStyle(fontSize: 12.5, color: context.tokens.textDim)),
              const SizedBox(height: 16),
              _field(_a, 'New PIN',
                  autofocus: true,
                  onSubmitted: (_) => FocusScope.of(context).nextFocus()),
              const SizedBox(height: 12),
              _field(_b, 'The same PIN again', onSubmitted: (_) => _ok()),
              if (_error.isNotEmpty) ...[
                const SizedBox(height: 10),
                Text(_error,
                    style: const TextStyle(fontSize: 12.5, color: Tokens.error)),
              ],
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context),
            child: const Text('Cancel'),
          ),
          FilledButton(onPressed: _ok, child: const Text('Set PIN')),
        ],
      );
}

// ── backup & data ───────────────────────────────────────────────────────────

class DataTab extends StatelessWidget {
  const DataTab({super.key, required this.controller, required this.state});

  final SettingsController controller;
  final SettingsState state;

  /// Shown in the list; older ones stay on disk and are counted.
  static const int _shown = 6;

  Future<void> _restore(BuildContext context, BackupRow b) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Restore this backup?'),
        content: Text(
            'Your settings and watched folders go back to how they were on '
            '${_when(b.secs.toInt())}. What you have now is backed up first, '
            'so this can be undone.'),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Restore'),
          ),
        ],
      ),
    );
    if (ok != true) return;
    await controller.sendAction('backup-restore-${b.id}');
    // The theme, the design language and the name are read through the
    // shell's snapshot, not this page's.
    await ShellController.instance.refresh();
  }

  Future<void> _reset(BuildContext context) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (_) => const _ResetDialog(),
    );
    if (ok != true) return;
    try {
      final s = await statusDispatch(cmd: const StatusCmd.resetApp());
      controller.say(s.error.isNotEmpty ? s.error : s.notice);
    } catch (e) {
      controller.say('Could not reset: $e');
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = controller;
    final rows = Rows(state.data);
    rows.use(['backup', 'open-logs', 'open-data']);
    final multi = rows.key('multi-user');
    final rest = rows.rest;
    final backups = state.backups;
    final data = state.dataPath;

    return SettingsPageBody(
      head: SettingsHead.forTab(
        'data',
        note: 'Keep your settings safe, and find where everything lives',
        actions: [
          SmallBtn(
            label: 'Back up now',
            icon: Icons.archive_outlined,
            large: true,
            primary: true,
            onTap: () => c.sendAction('backup'),
          ),
        ],
      ),
      children: [
        TileGrid([
          (
            span: 4,
            child: SettingsTile(
              icon: Icons.archive_outlined,
              tint: Tokens.secCloud,
              title: 'Backups',
              note: 'Settings and the watched-folder list. The databases '
                  'rebuild from a rescan',
              trailing: [
                StateChip(
                  backups.isEmpty ? 'None yet' : _ago(backups.first.secs.toInt()),
                  tint: backups.isEmpty ? null : Tokens.ok,
                ),
              ],
              child: backups.isEmpty
                  ? _note(
                      context,
                      'No backups yet. Back up now keeps your settings and '
                      'folder list, so they can be brought back.')
                  : Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        Container(
                          clipBehavior: Clip.antiAlias,
                          decoration: BoxDecoration(
                            borderRadius: BorderRadius.circular(12),
                            border: Border.all(color: t.outline),
                          ),
                          child: Column(
                            children: [
                              for (var i = 0;
                                  i < backups.length && i < _shown;
                                  i++)
                                _BackupLine(
                                  backup: backups[i],
                                  first: i == 0,
                                  onRestore: () =>
                                      _restore(context, backups[i]),
                                ),
                            ],
                          ),
                        ),
                        if (backups.length > _shown) ...[
                          const SizedBox(height: 8),
                          _note(context,
                              'And ${backups.length - _shown} older, in the '
                              'data folder.'),
                        ],
                      ],
                    ),
            ),
          ),
          if (multi != null)
            (
              span: 2,
              child: SettingsTile(
                icon: Icons.group_outlined,
                tint: Tokens.secPhotos,
                title: 'One library per user',
                note: 'Each computer account gets its own',
                trailing: [
                  SettingSwitch(
                    on: multi.on_,
                    onChanged: (v) =>
                        c.send(SettingsCmd.toggle(key: multi.key, on_: v)),
                  ),
                ],
                child: _note(context,
                    'Off, everyone on this computer shares one library and '
                    'one set of settings.'),
              ),
            ),
          (
            span: 3,
            child: SettingsTile(
              icon: Icons.storage_outlined,
              tint: const Color(0xFF0EA5E9),
              title: 'Where things live',
              note: 'Open a folder to look, or to attach it to a bug report',
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  _PathRow(
                    icon: Icons.storage_outlined,
                    title: 'Data folder',
                    path: data.isEmpty ? '' : tildePath(data),
                    onOpen: () => c.sendAction('open-data'),
                  ),
                  const SizedBox(height: 8),
                  _PathRow(
                    icon: Icons.article_outlined,
                    title: 'Logs',
                    path: data.isEmpty ? '' : tildePath('$data/logs'),
                    onOpen: () => c.sendAction('open-logs'),
                  ),
                ],
              ),
            ),
          ),
          (
            span: 3,
            child: SettingsTile(
              icon: Icons.delete_outline,
              tint: Tokens.error,
              title: 'Start over',
              note: 'Erase every index, setting and cached thumbnail',
              danger: true,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _note(
                      context,
                      'Your files are never touched — only what Tulipix knows '
                      'about them. Back up first if you might want your '
                      'settings back. Close and reopen Tulipix when it '
                      'finishes.'),
                  const SizedBox(height: 12),
                  SmallBtn(
                    label: 'Reset Tulipix…',
                    icon: Icons.delete_outline,
                    danger: true,
                    onTap: () => _reset(context),
                  ),
                ],
              ),
            ),
          ),
        ]),
        if (rest.isNotEmpty) MoreTile(rows: rest, controller: c),
      ],
    );
  }
}

class _BackupLine extends StatelessWidget {
  const _BackupLine({
    required this.backup,
    required this.first,
    required this.onRestore,
  });

  final BackupRow backup;
  final bool first;
  final VoidCallback onRestore;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final b = backup;
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 9, 10, 9),
      decoration: BoxDecoration(
        color: t.bg,
        border: first ? null : Border(top: BorderSide(color: t.outline)),
      ),
      child: Row(
        children: [
          Icon(Icons.archive_outlined, size: 16, color: t.textDim),
          const SizedBox(width: 12),
          Expanded(
            child: Text(_when(b.secs.toInt()),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12.5, color: t.text)),
          ),
          Text(
              '${b.files} ${b.files == 1 ? 'file' : 'files'} · '
              '${humanBytes(b.bytes.toDouble())}',
              style: TextStyle(fontSize: 11.5, color: t.textDim)),
          const SizedBox(width: 12),
          SmallBtn(label: 'Restore', onTap: onRestore),
        ],
      ),
    );
  }
}

class _PathRow extends StatelessWidget {
  const _PathRow({
    required this.icon,
    required this.title,
    required this.path,
    required this.onOpen,
  });

  final IconData icon;
  final String title;
  final String path;
  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 10, 10, 10),
      decoration: BoxDecoration(
        color: t.bg,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: t.outline),
      ),
      child: Row(
        children: [
          Icon(icon, size: 16, color: t.textDim),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(title,
                    style: TextStyle(
                        fontSize: 12.5,
                        fontWeight: FontWeight.w600,
                        color: t.text)),
                Text(path.isEmpty ? '—' : path,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11, color: t.textDim)),
              ],
            ),
          ),
          SmallBtn(label: 'Open', icon: Icons.open_in_new, onTap: onOpen),
        ],
      ),
    );
  }
}

/// Typing the word is the confirmation: arming a reset and meaning it are two
/// deliberate acts, not one misplaced click.
class _ResetDialog extends StatefulWidget {
  const _ResetDialog();

  @override
  State<_ResetDialog> createState() => _ResetDialogState();
}

class _ResetDialogState extends State<_ResetDialog> {
  final TextEditingController _typed = TextEditingController();

  /// From the crate that checks it, so the two cannot disagree.
  final String _phrase = statusResetPhrase();

  @override
  void dispose() {
    _typed.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final armed = _typed.text == _phrase;
    return AlertDialog(
      title: const Text('Reset Tulipix?'),
      content: SizedBox(
        width: 380,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
                'Erases every index, setting, thumbnail and watched-folder '
                'entry. Your media files are never touched. Close and reopen '
                'Tulipix when it finishes.',
                style: TextStyle(fontSize: 12.5, color: t.textDim)),
            const SizedBox(height: 14),
            TextField(
              controller: _typed,
              autofocus: true,
              onChanged: (_) => setState(() {}),
              decoration: InputDecoration(
                isDense: true,
                labelText: 'Type $_phrase to confirm',
                border: const OutlineInputBorder(),
              ),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context, false),
          child: const Text('Cancel'),
        ),
        FilledButton(
          style: FilledButton.styleFrom(backgroundColor: Tokens.error),
          onPressed: armed ? () => Navigator.pop(context, true) : null,
          child: const Text('Erase everything'),
        ),
      ],
    );
  }
}

// ── advanced ────────────────────────────────────────────────────────────────

class AdvancedTab extends StatelessWidget {
  const AdvancedTab({super.key, required this.controller, required this.state});

  final SettingsController controller;
  final SettingsState state;

  /// The tool rows are readings named by the tool, in the order drawn.
  static const Map<String, String> _tools = {
    'ffmpeg': 'Converts and reads media',
    'ffprobe': 'Reads durations and streams',
    'yt-dlp': 'Downloads from video sites',
    'mpv': 'Plays video',
    'rclone': 'Cloud sync',
    'exiftool': 'Photo metadata',
  };
  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = controller;
    final rows = Rows(state.advanced);
    final tools =
        _tools.keys.map(rows.label).whereType<SettingItem>().toList();
    final missing = tools.where((r) => r.state == 'error').length;
    final binDir = rows.key('tools.bin-dir');
    final ytAuto = rows.key('ytdlp.auto-update');
    final clients = rows.key('ytdlp.player-clients');
    final power = rows.key('power-aware');
    final cache = rows.label('Thumbnail cache');
    rows.use(['clear-thumbs']);
    final notif = rows.key('notifications');
    final crash = rows.key('crash-upload');
    final renderer = rows.label('Renderer');
    final player = rows.label('Player embedding');
    final formats = rows.label('Audio formats');
    final rest = rows.rest;

    final details = [
      'Tulipix ${state.appVersion}',
      if (renderer != null) 'Renderer: ${renderer.value}',
      if (player != null) 'Player: ${player.value}',
      if (formats != null) 'Audio formats: ${formats.value}',
    ].join('\n');

    Widget cap(String s) => Text(s,
        style: TextStyle(
            fontSize: 10.5,
            fontWeight: FontWeight.w700,
            letterSpacing: 0.8,
            color: t.textDim));
    Widget rule() => Container(
        width: 1,
        margin: const EdgeInsets.symmetric(horizontal: 18),
        color: t.outline);
    Widget chip(String s) => Container(
          padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
          decoration: BoxDecoration(
            color: t.bg,
            borderRadius: BorderRadius.circular(6),
            border: Border.all(color: t.outline),
          ),
          child: Text(s,
              style: TextStyle(
                  fontSize: 10.5,
                  fontWeight: FontWeight.w500,
                  color: t.textDim)),
        );

    Widget kv(String k, String v) => Padding(
          padding: const EdgeInsets.symmetric(vertical: 3),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(k, style: TextStyle(fontSize: 12.5, color: t.textDim)),
              const SizedBox(width: 14),
              Expanded(
                child: Text(v,
                    textAlign: TextAlign.right,
                    style: TextStyle(
                        fontSize: 12,
                        fontWeight: FontWeight.w600,
                        color: t.text)),
              ),
            ],
          ),
        );

    return SettingsPageBody(
      head: SettingsHead.forTab('advanced',
          note: 'The tools Tulipix runs, what it caches, and how it was built'),
      children: [
        TileGrid([
          (
            span: 4,
            child: SettingsTile(
              icon: Icons.handyman_outlined,
              tint: Tokens.secTools,
              title: 'Bundled tools',
              note: 'Your folder first, then updates, then bundled, then the '
                  'system',
              trailing: [
                StateChip(missing == 0 ? 'All found' : '$missing missing',
                    tint: missing == 0 ? Tokens.ok : Tokens.warn),
              ],
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  if (tools.isNotEmpty)
                    Container(
                      clipBehavior: Clip.antiAlias,
                      decoration: BoxDecoration(
                        borderRadius: BorderRadius.circular(12),
                        border: Border.all(color: t.outline),
                      ),
                      child: Column(
                        children: [
                          for (var i = 0; i < tools.length; i++)
                            _ToolLine(
                              row: tools[i],
                              what: _tools[tools[i].label] ?? '',
                              first: i == 0,
                            ),
                        ],
                      ),
                    ),
                  const SizedBox(height: 4),
                  Lines([
                    if (binDir != null) RowLine(row: binDir, controller: c),
                    if (ytAuto != null) RowLine(row: ytAuto, controller: c),
                    if (clients != null) RowLine(row: clients, controller: c),
                  ]),
                ],
              ),
            ),
          ),
          (
            span: 2,
            child: SettingsTile(
              icon: Icons.speed_outlined,
              tint: Tokens.warn,
              title: 'Performance',
              note: 'Background work, the cache, and the platform',
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  if (power != null)
                    Lines([RowLine(row: power, controller: c)]),
                  const SizedBox(height: 8),
                  Row(
                    children: [
                      Text('Thumbnail cache',
                          style: TextStyle(
                              fontSize: 12.5,
                              fontWeight: FontWeight.w600,
                              color: t.text)),
                      const Spacer(),
                      Text(cache?.value ?? '—',
                          style: TextStyle(
                              fontSize: 12.5,
                              fontWeight: FontWeight.w700,
                              color: t.text)),
                    ],
                  ),
                  const SizedBox(height: 10),
                  Align(
                    alignment: Alignment.centerLeft,
                    child: SmallBtn(
                      label: 'Clear the cache',
                      onTap: () => c.sendAction('clear-thumbs'),
                    ),
                  ),
                  const SizedBox(height: 6),
                  Text('Every thumbnail is redrawn the next time it is needed.',
                      style: TextStyle(fontSize: 11, color: t.textDim)),
                  // Notifications and crash reports: two switches that were
                  // a tile of their own beside this one's empty half.
                  if (notif != null || crash != null) ...[
                    const SizedBox(height: 10),
                    Lines([
                      if (notif != null) RowLine(row: notif, controller: c),
                      if (crash != null) RowLine(row: crash, controller: c),
                    ]),
                  ],
                ],
              ),
            ),
          ),
          (
            span: 6,
            child: SettingsTile(
              icon: Icons.memory,
              tint: Tokens.secSettings,
              title: 'This build',
              note: '',
              trailing: [
                SmallBtn(
                  label: 'Copy',
                  icon: Icons.copy_outlined,
                  onTap: () async {
                    await Clipboard.setData(ClipboardData(text: details));
                    c.say('Copied the build details.');
                  },
                ),
              ],
              // Two columns: who made it, and what it runs on.
              child: IntrinsicHeight(
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    SizedBox(
                      width: 230,
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Row(
                            children: [
                              AppMark(
                                size: 56,
                                radius: 14,
                                choice: ShellController
                                        .instance.state?.logoChoice ??
                                    0,
                              ),
                              const SizedBox(width: 14),
                              Expanded(
                                child: Column(
                                  crossAxisAlignment: CrossAxisAlignment.start,
                                  children: [
                                    Text('Tulipix',
                                        style: TextStyle(
                                            fontSize: 22,
                                            fontWeight: FontWeight.w700,
                                            letterSpacing: -0.1,
                                            color: t.text)),
                                    Text('Version ${state.appVersion}',
                                        style: TextStyle(
                                            fontSize: 12, color: t.textDim)),
                                  ],
                                ),
                              ),
                            ],
                          ),
                          const SizedBox(height: 14),
                          Text('© 2026 — Developed by Atish Ak Sharma',
                              style:
                                  TextStyle(fontSize: 11.5, color: t.textDim)),
                        ],
                      ),
                    ),
                    rule(),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          cap('RUNS ON'),
                          const SizedBox(height: 6),
                          if (renderer != null) kv('Renderer', renderer.value),
                          if (player != null) kv('Player', player.value),
                          if (formats != null) ...[
                            const SizedBox(height: 10),
                            Wrap(
                              spacing: 5,
                              runSpacing: 5,
                              children: [
                                for (final f in formats.value.split(' · '))
                                  chip(f.toUpperCase()),
                              ],
                            ),
                          ],
                        ],
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        ]),
        if (rest.isNotEmpty) MoreTile(rows: rest, controller: c),
      ],
    );
  }
}

/// One tool: its name, what it does, and where this copy comes from.
class _ToolLine extends StatelessWidget {
  const _ToolLine({required this.row, required this.what, required this.first});

  final SettingItem row;
  final String what;
  final bool first;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // "Bundled · 2026.08.30": the source, then yt-dlp's version.
    final parts = row.value.split(' · ');
    final source = parts.first;
    final version = parts.skip(1).join(' · ');
    final line = switch (row.state) {
      'error' => '$what — install it, or set a tools folder',
      'warn' => '$what · whatever version is installed',
      _ => version.isEmpty ? what : '$what · $version',
    };
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 8, 10, 8),
      decoration: BoxDecoration(
        color: t.bg,
        border: first ? null : Border(top: BorderSide(color: t.outline)),
      ),
      child: Row(
        children: [
          SizedBox(
            width: 90,
            child: Text(row.label,
                style: TextStyle(
                    fontSize: 12.5, fontWeight: FontWeight.w600, color: t.text)),
          ),
          Expanded(
            child: Text(line,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 11.5, color: t.textDim)),
          ),
          const SizedBox(width: 10),
          StateChip(source, tint: stateTint(row.state, t)),
        ],
      ),
    );
  }
}
