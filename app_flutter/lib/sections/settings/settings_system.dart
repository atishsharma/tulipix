// Settings › Security, Backup & Data and Advanced, as tiles over the rows
// Rust already sends (docs/mockups/settings-tabs.html).
//
// Two things here are new: a backup can be brought back (Restore), and the
// reset that used to sit in Status's library popup lives in Backup & Data,
// next to the backups that make it safe to press.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/app_mark.dart';
import '../../platform/pick.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/lock/lock_controller.dart';
import '../../shell/onboarding/onboarding_controller.dart';
import '../../shell/shell_controller.dart';
import '../../shell/vitals.dart';
import '../../shell/window.dart' show WindowChrome;
import '../../src/rust/api/logs.dart';
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

/// The slug at the end of a `profile-…:<slug>` action key.
String _slugOf(String key) => key.substring(key.indexOf(':') + 1);

/// A status row's colour. Shared by the tabs in this file: the bridge sends
/// the same four words everywhere, and two copies of this drifted once.
Color? _stateTint(String state) => switch (state) {
      'ok' => Tokens.ok,
      'warn' => Tokens.warn,
      'error' => Tokens.error,
      _ => null,
    };

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
    // What is actually set up to unlock with, once the switch is on: the
    // fingerprint row, and one row per security key — plugged in, or already
    // set up. The bridge decides which exist; this draws what it sends.
    final passkeyWays = [
      for (final r in state.security)
        if (r.key.startsWith('passkey-') ||
            (r.kind == 'status' &&
                (r.label == 'Fingerprint' || r.label == 'Security key')))
          r,
    ];
    rows.use(passkeyWays.map((r) => r.key));
    final enc = rows.key('db-encrypt');
    // What encryption is actually doing: off, on, or waiting for a restart.
    // A build without SQLCipher has the reading and no switch at all.
    final encState = rows.label('Encryption');
    final encUnsupported = rows.label('Encrypt the library database');
    final sandbox = rows.label('OS sandbox');
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
        // Two tiles stacked on the left; the PIN, the wallpapers and the
        // protections down the right at their full height. TileGrid packs
        // rows and has no tile two rows tall, so this tab lays itself out.
        LayoutBuilder(builder: (context, box) {
          Widget part(String label, Widget body) => Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Text(label.toUpperCase(),
                      style: TextStyle(
                          fontSize: 10.5,
                          fontWeight: FontWeight.w600,
                          letterSpacing: 0.6,
                          color: t.textDim)),
                  const SizedBox(height: 8),
                  body,
                ],
              );

          final idle = auto == null
              ? null
              : SettingsTile(
                  icon: Icons.lock_clock_outlined,
                  tint: Tokens.secBooks,
                  title: 'Lock when idle',
                  note: 'After a while without a key press or the pointer '
                      'moving',
                  trailing: [
                    SettingSwitch(
                        on: auto.on_, onChanged: (v) => toggle(auto, v)),
                  ],
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      if (after != null)
                        // Still settable while off, so it is ready when it
                        // is switched on; dimmed, because it does nothing yet.
                        Opacity(
                          opacity: auto.on_ ? 1 : 0.5,
                          child: Seg(
                            full: true,
                            options: after.options,
                            labels: [for (final o in after.options) _short(o)],
                            value: after.value,
                            onPick: (v) => c.send(SettingsCmd.setText(
                                key: 'lock.after', value: v)),
                          ),
                        ),
                      const SizedBox(height: 10),
                      _note(
                          context,
                          'It dims and counts down for ten seconds first. A '
                          'playing video never locks, and music keeps '
                          'playing.'),
                    ],
                  ),
                );

          final screen = face.isEmpty
              ? null
              : SettingsTile(
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
                          for (final r in face)
                            RowLine(row: r, controller: c),
                        ]),
                      ),
                    ],
                  ),
                );

          final parts = [
            if (pin != null)
              part(
                'PIN · four to eight digits',
                Column(
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
            if (wall != null)
              part(
                'Wallpapers',
                Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
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
                            onTap: () => c.send(const SettingsCmd.setText(
                                key: 'lock.wallpapers', value: '')),
                          ),
                        ],
                      ],
                    ),
                    const SizedBox(height: 10),
                    _note(
                        context,
                        'The first ten pictures, one every twelve seconds, '
                        'with a slow zoom.'),
                  ],
                ),
              ),
            if (passkey != null || enc != null || sandbox != null)
              part(
                'Protection',
                Lines([
                  if (passkey != null)
                    SettingLine(
                      title: 'Unlock with a passkey',
                      note: 'A fingerprint or a security key, instead of '
                          'typing the PIN. The PIN still works',
                      trailing: SettingSwitch(
                          on: passkey.on_,
                          onChanged: (v) => toggle(passkey, v)),
                    ),
                  for (final r in passkeyWays)
                    SettingLine(
                      title: r.label,
                      note: r.desc,
                      trailing: r.btn.isEmpty
                          ? StateChip(r.value, tint: _stateTint(r.state))
                          : Row(
                              mainAxisSize: MainAxisSize.min,
                              children: [
                                StateChip(r.value, tint: _stateTint(r.state)),
                                const SizedBox(width: 8),
                                SmallBtn(
                                  label: r.btn,
                                  danger: r.btn == 'Remove',
                                  onTap: () => c.sendAction(r.key),
                                ),
                              ],
                            ),
                    ),
                  if (enc != null)
                    SettingLine(
                      title: 'Encrypt the library database',
                      note: 'From the next start. Protects the index if the '
                          'disk is stolen; your files are never touched',
                      trailing: SettingSwitch(
                          on: enc.on_, onChanged: (v) => toggle(enc, v)),
                    ),
                  if (enc == null && encUnsupported != null)
                    SettingLine(
                      title: 'Encrypt the library database',
                      note: encUnsupported.value,
                      trailing: const StateChip('Not in this build'),
                    ),
                  if (encState != null)
                    SettingLine(
                      title: 'Encryption',
                      note: encState.value,
                      trailing: StateChip(
                          switch (encState.state) {
                            'ok' => 'On',
                            'warn' => 'At next start',
                            _ => 'Off',
                          },
                          tint: _stateTint(encState.state)),
                    ),
                  if (sandbox != null)
                    SettingLine(
                      title: 'OS sandbox',
                      note: 'Whether the system walls this copy off. A '
                          'reading only',
                      trailing: StateChip(sandbox.value,
                          tint: sandbox.state == 'ok' ? Tokens.ok : null),
                    ),
                ]),
              ),
          ];

          final side = parts.isEmpty
              ? null
              : SettingsTile(
                  icon: Icons.shield_outlined,
                  tint: Tokens.secTransfer,
                  title: 'PIN, wallpapers & protection',
                  note: 'What unlocks it, the picture when nothing plays, and '
                      'the locks',
                  trailing: [
                    if (pin != null)
                      StateChip(hasPin ? 'PIN set' : 'No PIN',
                          tint: hasPin ? Tokens.ok : null),
                  ],
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      for (var i = 0; i < parts.length; i++) ...[
                        if (i > 0)
                          Divider(height: 29, thickness: 1, color: t.outline),
                        parts[i],
                      ],
                    ],
                  ),
                );

          final left = [idle, screen].whereType<Widget>().toList();

          // One column on a narrow window, as TileGrid does.
          if (box.maxWidth < 760) {
            final all = [...left, if (side != null) side];
            return Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (var i = 0; i < all.length; i++) ...[
                  if (i > 0) const SizedBox(height: TileGrid.gap),
                  IntrinsicHeight(child: all[i]),
                ],
              ],
            );
          }
          return IntrinsicHeight(
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Expanded(
                  flex: 4,
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      for (var i = 0; i < left.length; i++) ...[
                        if (i > 0) const SizedBox(height: TileGrid.gap),
                        // The last takes whatever height the right side
                        // leaves, so both columns end on one line.
                        if (i == left.length - 1)
                          Expanded(child: left[i])
                        else
                          left[i],
                      ],
                    ],
                  ),
                ),
                const SizedBox(width: TileGrid.gap),
                Expanded(flex: 2, child: side ?? const SizedBox.shrink()),
              ],
            ),
          );
        }),
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

  /// The file another app keeps its library in. Rust works out which app from
  /// the file itself.
  Future<void> _import() async {
    final path = await pickFile(
      label: 'Library to import',
      extensions: const ['ini', 'xml', 'db', 'lrcat', 'dat'],
    );
    if (path != null) await controller.sendAction('import:$path');
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
    rows.use(['backup', 'open-logs', 'open-data', 'crash-open', 'crash-clear']);
    final multi = rows.key('multi-user');
    // One row per profile, plus the name box and Add, once the switch is on.
    final profiles = [
      for (final r in state.data)
        if (r.key.startsWith('profile-use:') ||
            r.key.startsWith('profile-rename:') ||
            r.key.startsWith('profile-delete:'))
          r,
    ];
    final newProfile = rows.key('profile.new-name');
    rows.use(['profile-add', ...profiles.map((r) => r.key)]);
    // One line per profile; the Delete rows sit beside them, keyed by slug.
    final profileLines = [
      for (final r in profiles)
        if (!r.key.startsWith('profile-delete:')) r,
    ];
    final deleteKeys = {
      for (final r in profiles)
        if (r.key.startsWith('profile-delete:')) _slugOf(r.key): r.key,
    };
    // One row per crash dump the bridge found, newest first. Nothing is sent
    // anywhere: Report opens a pre-filled issue in the browser.
    final crashes = [
      for (final r in state.data)
        if (r.key.startsWith('crash-report:')) r,
    ];
    rows.use(crashes.map((r) => r.key));
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
          // Three columns, as every tile under it: at four its edge sat past
          // theirs.
          (
            span: multi == null ? 6 : 3,
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
              span: multi.on_ ? 6 : 3,
              child: SettingsTile(
                icon: Icons.group_outlined,
                tint: Tokens.secPhotos,
                title: 'Profiles',
                note: 'Several people on this computer account, each with '
                    'their own library',
                trailing: [
                  if (multi.on_ && profiles.isNotEmpty)
                    StateChip('${profileLines.length}'),
                  SettingSwitch(
                    on: multi.on_,
                    onChanged: (v) =>
                        c.send(SettingsCmd.toggle(key: multi.key, on_: v)),
                  ),
                ],
                child: !multi.on_
                    ? _note(
                        context,
                        'Off, everyone using this computer account shares one '
                        'library and one set of settings.')
                    : Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          _note(
                              context,
                              'Nothing is shared between profiles — each has '
                              'its own watched folders, settings and PIN. '
                              'Opening one restarts Tulipix.'),
                          const SizedBox(height: 10),
                          Container(
                            clipBehavior: Clip.antiAlias,
                            decoration: BoxDecoration(
                              borderRadius: BorderRadius.circular(12),
                              border: Border.all(color: t.outline),
                            ),
                            child: Column(
                              children: [
                                for (var i = 0; i < profileLines.length; i++)
                                  _ProfileLine(
                                    row: profileLines[i],
                                    first: i == 0,
                                    onTap: () =>
                                        c.sendAction(profileLines[i].key),
                                    onDelete: deleteKeys.containsKey(
                                            _slugOf(profileLines[i].key))
                                        ? () => c.sendAction(deleteKeys[
                                            _slugOf(profileLines[i].key)]!)
                                        : null,
                                  ),
                              ],
                            ),
                          ),
                          if (newProfile != null) ...[
                            const SizedBox(height: 10),
                            FieldBox(
                              value: newProfile.value,
                              hint: 'A name — “Mum”, “The kids”',
                              onSubmit: (v) => c.send(SettingsCmd.setText(
                                  key: newProfile.key, value: v)),
                              trailing: [
                                SmallBtn(
                                  label: 'Add',
                                  icon: Icons.person_add_alt,
                                  onTap: () => c.sendAction('profile-add'),
                                ),
                              ],
                            ),
                          ],
                        ],
                      ),
              ),
            ),
          (
            span: 3,
            child: SettingsTile(
              icon: Icons.upload_file_outlined,
              tint: Tokens.secPhotos,
              title: 'Export the library',
              note: 'Items, albums, playlists, tags and ratings, as JSON',
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _note(
                      context,
                      'Paths and what you did with them, never the files. Saved '
                      'to the exports folder, which opens when it is done.'),
                  const SizedBox(height: 12),
                  SmallBtn(
                    label: 'Export',
                    icon: Icons.upload_outlined,
                    onTap: () => c.sendAction('export'),
                  ),
                ],
              ),
            ),
          ),
          (
            span: 3,
            child: SettingsTile(
              icon: Icons.download_outlined,
              tint: Tokens.secMusic,
              title: 'Import from another app',
              note: 'Picasa stars, iTunes play counts and ratings',
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _note(
                      context,
                      'Pick a .picasa.ini or an iTunes Library.xml. Plex, '
                      'Lightroom and foobar2000 libraries are recognised, but '
                      'not imported yet.'),
                  const SizedBox(height: 12),
                  SmallBtn(
                    label: 'Choose a file…',
                    icon: Icons.folder_open_outlined,
                    onTap: _import,
                  ),
                ],
              ),
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
                    onView: () => showDialog<void>(
                      context: context,
                      builder: (_) => const _LogViewer(),
                    ),
                  ),
                ],
              ),
            ),
          ),
          (
            span: 3,
            child: SettingsTile(
              icon: Icons.bug_report_outlined,
              tint: Tokens.warn,
              title: 'Crash reports',
              note: 'Kept on this computer. Nothing is ever sent on its own',
              trailing: [
                StateChip(
                  crashes.isEmpty ? 'None' : '${crashes.length}',
                  tint: crashes.isEmpty ? Tokens.ok : Tokens.warn,
                ),
              ],
              child: crashes.isEmpty
                  ? _note(
                      context,
                      'Tulipix has not crashed on this computer. If it does, '
                      'the report lands here and Report opens an issue with '
                      'the details filled in — you decide whether to post it.')
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
                              for (var i = 0; i < crashes.length; i++)
                                _CrashLine(
                                  row: crashes[i],
                                  first: i == 0,
                                  onReport: () =>
                                      c.sendAction(crashes[i].key),
                                ),
                            ],
                          ),
                        ),
                        const SizedBox(height: 12),
                        Wrap(
                          spacing: 8,
                          runSpacing: 8,
                          children: [
                            SmallBtn(
                              label: 'Open the folder',
                              icon: Icons.folder_open_outlined,
                              onTap: () => c.sendAction('crash-open'),
                            ),
                            SmallBtn(
                              label: 'Delete all',
                              icon: Icons.delete_outline,
                              danger: true,
                              onTap: () => c.sendAction('crash-clear'),
                            ),
                          ],
                        ),
                      ],
                    ),
            ),
          ),
          (
            span: 3,
            child: SettingsTile(
              icon: Icons.auto_awesome_outlined,
              tint: Tokens.brand,
              title: 'Set up again',
              note: 'The ten cards from the first run, over the app as it is',
              fill: true,
              child: Spread(
                gap: 12,
                crossAxisAlignment: CrossAxisAlignment.start,
                [
                  _note(
                      context,
                      'Walks the same setup you saw the first time — your '
                      'name, the look, which folders to read. Nothing is '
                      'changed by a card you skip, so you can run it to alter '
                      'one thing and leave the rest exactly as it is.'),
                  SmallBtn(
                    label: 'Run setup',
                    icon: Icons.play_arrow_outlined,
                    onTap: OnboardingController.instance.start,
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
              fill: true,
              child: Spread(
                gap: 12,
                crossAxisAlignment: CrossAxisAlignment.start,
                [
                  _note(
                      context,
                      'Your files are never touched — only what Tulipix knows '
                      'about them. Back up first if you might want your '
                      'settings back. Close and reopen Tulipix when it '
                      'finishes.'),
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

/// One crash dump: when it happened, what panicked, and Report. The bridge
/// has already written the line — `label` is the time, `desc` the panic and
/// where it was, `value` the version it happened on.
class _CrashLine extends StatelessWidget {
  const _CrashLine({
    required this.row,
    required this.first,
    required this.onReport,
  });

  final SettingItem row;
  final bool first;
  final VoidCallback onReport;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 9, 10, 9),
      decoration: BoxDecoration(
        color: t.bg,
        border: first ? null : Border(top: BorderSide(color: t.outline)),
      ),
      child: Row(
        children: [
          const Icon(Icons.error_outline, size: 16, color: Tokens.warn),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text('${row.label} · ${row.value}',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 12.5, color: t.text)),
                if (row.desc.isNotEmpty)
                  Text(row.desc,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 11.5, color: t.textDim)),
              ],
            ),
          ),
          const SizedBox(width: 12),
          SmallBtn(label: 'Report', onTap: onReport),
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
    this.onView,
  });

  final IconData icon;
  final String title;
  final String path;
  final VoidCallback onOpen;

  /// Reads the folder's contents inside the app. Only the logs have one — a
  /// folder of JSONL is not something to send anyone to a file manager for.
  final VoidCallback? onView;

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
          if (onView != null) ...[
            SmallBtn(
                label: 'View',
                icon: Icons.subject_outlined,
                onTap: onView!),
            const SizedBox(width: 6),
          ],
          SmallBtn(label: 'Open', icon: Icons.open_in_new, onTap: onOpen),
        ],
      ),
    );
  }
}

/// The log viewer: what the app has been saying to itself, without leaving it.
///
/// The lines are shown as they were written — real paths and all, because this
/// is your own machine and the folder that failed to scan is usually the whole
/// answer. Copy is the one that scrubs, since that is the trip that ends in an
/// issue tracker.
class _LogViewer extends StatefulWidget {
  const _LogViewer();

  @override
  State<_LogViewer> createState() => _LogViewerState();
}

class _LogViewerState extends State<_LogViewer> {
  /// Empty is everything; the rest are `tracing` levels as the bridge spells
  /// them.
  static const List<String> _levels = ['', 'INFO', 'WARN', 'ERROR'];
  static const List<String> _levelLabels = ['All', 'Info', 'Warnings', 'Errors'];

  /// Enough to scroll through and cheap to re-read on every keystroke; the
  /// footer says when it was not the whole of it.
  static const int _limit = 500;

  String _level = '';
  String _search = '';
  LogPage? _page;
  String? _error;
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    setState(() => _busy = true);
    try {
      final p = await logsRead(level: _level, search: _search, limit: _limit);
      if (!mounted) return;
      setState(() {
        _page = p;
        _error = null;
        _busy = false;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = '$e';
        _busy = false;
      });
    }
  }

  Future<void> _copy() async {
    try {
      final text =
          await logsCopy(level: _level, search: _search, limit: _limit);
      await Clipboard.setData(ClipboardData(text: text));
      if (!mounted) return;
      ScaffoldMessenger.of(context).showSnackBar(const SnackBar(
        content: Text('Copied, with paths and keys scrubbed'),
      ));
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = '$e');
    }
  }

  Future<void> _clear() async {
    try {
      await logsClear();
    } catch (e) {
      if (!mounted) return;
      setState(() => _error = '$e');
      return;
    }
    await _load();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final page = _page;
    return AlertDialog(
      title: Row(
        children: [
          const Expanded(child: Text('Logs')),
          if (page != null)
            StateChip(
              page.files == 0
                  ? 'Nothing yet'
                  : '${page.files} ${page.files == 1 ? 'day' : 'days'}',
              tint: page.files == 0 ? Tokens.warn : Tokens.ok,
            ),
        ],
      ),
      content: SizedBox(
        width: 720,
        height: 460,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Seg(
                  options: _levels,
                  labels: _levelLabels,
                  value: _level,
                  onPick: (v) {
                    setState(() => _level = v);
                    _load();
                  },
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: FieldBox(
                    value: _search,
                    hint: 'Find in messages or modules',
                    onSubmit: (v) {
                      setState(() => _search = v.trim());
                      _load();
                    },
                  ),
                ),
                const SizedBox(width: 10),
                SmallBtn(
                  label: 'Refresh',
                  icon: Icons.refresh,
                  onTap: _busy ? null : _load,
                ),
              ],
            ),
            const SizedBox(height: 12),
            Expanded(
              child: Container(
                clipBehavior: Clip.antiAlias,
                decoration: BoxDecoration(
                  color: t.bg,
                  borderRadius: BorderRadius.circular(12),
                  border: Border.all(color: t.outline),
                ),
                child: _body(context),
              ),
            ),
            if (page != null && page.truncated) ...[
              const SizedBox(height: 8),
              _note(
                  context,
                  'Showing the newest $_limit lines. Narrow the search, or '
                  'open the folder for the rest.'),
            ],
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: (_page?.lines.isEmpty ?? true) ? null : _clear,
          child: const Text('Delete all'),
        ),
        TextButton(
          onPressed: (_page?.lines.isEmpty ?? true) ? null : _copy,
          child: const Text('Copy, scrubbed'),
        ),
        FilledButton(
          onPressed: () => Navigator.pop(context),
          child: const Text('Close'),
        ),
      ],
    );
  }

  Widget _body(BuildContext context) {
    if (_error != null) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(20),
          child: Text('Could not read the logs: $_error',
              textAlign: TextAlign.center,
              style: const TextStyle(fontSize: 12.5, color: Tokens.error)),
        ),
      );
    }
    final page = _page;
    if (page == null) {
      return const Center(
        child: SizedBox(
            width: 20, height: 20, child: CircularProgressIndicator(strokeWidth: 2)),
      );
    }
    if (page.lines.isEmpty) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: _note(
              context,
              page.files == 0
                  ? 'Nothing has been logged yet. Tulipix writes one file a '
                      'day here and keeps a fortnight.'
                  : 'No line matches that. Try a wider level, or clear the '
                      'search.'),
        ),
      );
    }
    return ListView.builder(
      padding: EdgeInsets.zero,
      itemCount: page.lines.length,
      itemBuilder: (_, i) => _LogLineRow(line: page.lines[i], first: i == 0),
    );
  }
}

/// One line: the clock, the level, the module that said it, the message.
class _LogLineRow extends StatelessWidget {
  const _LogLineRow({required this.line, required this.first});

  final LogLine line;
  final bool first;

  /// Only the two levels worth noticing are coloured. Green on every INFO
  /// line makes a healthy log look like an alarm.
  static Color? _tint(String level) => switch (level.toUpperCase()) {
        'ERROR' => Tokens.error,
        'WARN' => Tokens.warn,
        _ => null,
      };

  /// The target is `tulipix_videos::anime`; the crate prefix is on every line
  /// and says nothing, so only the last part is drawn.
  static String _module(String target) {
    final i = target.lastIndexOf('::');
    return i < 0 ? target : target.substring(i + 2);
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final d = DateTime.fromMillisecondsSinceEpoch(line.secs.toInt() * 1000);
    final tint = _tint(line.level) ?? t.textDim;
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 6, 12, 6),
      decoration: BoxDecoration(
        border: first ? null : Border(top: BorderSide(color: t.outline)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('${_two(d.hour)}:${_two(d.minute)}:${_two(d.second)}',
              style: _mono.copyWith(fontSize: 11, color: t.textDim)),
          const SizedBox(width: 10),
          SizedBox(
            width: 46,
            child: Text(line.level.toUpperCase(),
                style: _mono.copyWith(
                    fontSize: 10,
                    fontWeight: FontWeight.w700,
                    color: tint)),
          ),
          const SizedBox(width: 6),
          SizedBox(
            width: 108,
            child: Text(_module(line.target),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: _mono.copyWith(fontSize: 11, color: t.textDim)),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: SelectableText(line.message,
                style: _mono.copyWith(fontSize: 11.5, color: t.text)),
          ),
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
//
// A control center in three tabs. Overview opens on five vitals that say
// whether anything is wrong before any list does, then the everyday switches,
// only what needs you, and this build. Tools and Platform hold the detail.
// Every row is one the Rust side already sends: the redesign moves them, it
// adds no setting.

/// No action where the fix is not a button: a slow start is read, not fixed.
typedef _Issue = ({
  Color tint,
  String title,
  String note,
  String? action,
  VoidCallback? onTap,
});

typedef _VitalData = ({
  String label,
  String value,
  String unit,
  String note,
  double? frac,
  Color tint,
});

const TextStyle _mono = TextStyle(
    fontFamily: 'monospace', fontFamilyFallback: ['Menlo', 'Consolas']);

class AdvancedTab extends StatefulWidget {
  const AdvancedTab({super.key, required this.controller, required this.state});

  final SettingsController controller;
  final SettingsState state;

  @override
  State<AdvancedTab> createState() => _AdvancedTabState();
}

class _AdvancedTabState extends State<AdvancedTab> {
  /// overview | tools | platform.
  String _tab = 'overview';

  SettingsController get _c => widget.controller;

  /// The tool rows are readings named by the tool, in the order drawn.
  static const Map<String, String> _tools = {
    'ffmpeg': 'Converts and reads media',
    'ffprobe': 'Reads durations and streams',
    'yt-dlp': 'Downloads from video sites',
    'whisper-cli': 'Turns speech into text, for Transcribe',
    'mpv': 'Plays music and video, inside the app',
    'rclone': 'Cloud sync',
    'exiftool': 'Photo metadata',
  };

  /// The platform readings, by label, in the order drawn.
  static const List<String> _platform = [
    'System tray',
    'Share sheet',
    'Shortcuts (AppIntents)',
    'Desktop widgets',
    'Live Activities',
  ];

  void _go(String tab) => setState(() => _tab = tab);

  Future<void> _browseToolsDir() async {
    final dir = await pickDirectory(
        title: 'Choose the folder holding ffmpeg, yt-dlp and the rest');
    if (dir != null) {
      await _c.send(SettingsCmd.setText(key: 'tools.bin-dir', value: dir));
    }
  }

  /// Asks first, as Slint does: the folder may hold the only working copy of
  /// a tool.
  Future<void> _resetToolsDir() async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Reset the tools folder?'),
        content: const Text('Tulipix goes back to its bundled tools and the '
            'ones installed on your system.'),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Reset'),
          ),
        ],
      ),
    );
    if (ok == true) {
      await _c.send(const SettingsCmd.setText(key: 'tools.bin-dir', value: ''));
    }
  }

  /// The accent and the text scale are applied from the shell's snapshot, so
  /// flipping either refreshes that too.
  Future<void> _flip(SettingItem r, bool v, {bool shell = false}) async {
    await _c.send(SettingsCmd.toggle(key: r.key, on_: v));
    if (shell) await ShellController.instance.refresh();
  }

  Future<void> _copy(String text, String what) async {
    await Clipboard.setData(ClipboardData(text: text));
    _c.say('Copied $what.');
  }

  /// Items [per] to a row, every row as tall as its tallest. A row the items
  /// do not fill keeps their widths.
  static Widget _rowsOf(List<Widget> items, {int per = 3, double gap = 10}) =>
      Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          for (var i = 0; i < items.length; i += per) ...[
            if (i > 0) SizedBox(height: gap),
            IntrinsicHeight(
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  for (var j = i; j < i + per; j++) ...[
                    if (j > i) SizedBox(width: gap),
                    Expanded(
                        child: j < items.length
                            ? items[j]
                            : const SizedBox.shrink()),
                  ],
                ],
              ),
            ),
          ],
        ],
      );

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.state;
    final rows = Rows(st.advanced);
    final tools =
        _tools.keys.map(rows.label).whereType<SettingItem>().toList();
    final speech = rows.label('Speech model');
    final binDir = rows.key('tools.bin-dir');
    final ytAuto = rows.key('ytdlp.auto-update');
    final clients = rows.key('ytdlp.player-clients');
    final power = rows.key('power-aware');
    final powerNow = rows.label('Power source');
    final cache = rows.label('Thumbnail cache');
    rows.use(['clear-thumbs']);
    final notif = rows.key('notifications');
    final accent = rows.key('follow-system-accent');
    final fontScale = rows.key('follow-os-font-scale');
    final alloc = rows.label('Allocator');
    // The MCP server, and the four readings that only exist while it is on.
    final mcp = rows.key('mcp-server');
    final mcpWrite = rows.key('mcp.write');
    final mcpTools = rows.label('Tools offered');
    final mcpReads = rows.label('Reads');
    final mcpCommand = rows.label('Command');
    final mcpConfig = rows.label('Claude Desktop config');
    final platform =
        _platform.map(rows.label).whereType<SettingItem>().toList();
    final renderer = rows.label('Renderer');
    final player = rows.label('Player embedding');
    final formats = rows.label('Audio formats');
    final onnx = rows.label('ONNX editor ops');
    final gapless = rows.label('Gapless unverified');
    final video = rows.label('Video formats');
    final toolFormats = rows.label('Tools formats');
    final books = rows.label('Book formats');
    final rest = rows.rest;

    final missing = tools.where((r) => r.state == 'error').length;
    final startup = Vitals.startupMs;
    final mem = Vitals.residentMb;
    final frames = Vitals.slowFrames;
    final cacheMb = int.tryParse((cache?.value ?? '').split(' ').first) ?? 0;
    final dirSet = (binDir?.value ?? '').trim().isNotEmpty;
    final framed = WindowChrome.instance.custom;

    // Only what is off, each with the button that fixes it.
    final issues = <_Issue>[
      for (final r in tools)
        if (r.state == 'error')
          (
            tint: Tokens.error,
            title: '${r.label} is missing',
            note: '${_tools[r.label] ?? 'It'} is off until it is found',
            action: 'Get it',
            onTap: () => _c.sendAction('tool-update:${r.label}'),
          )
        else if (r.state == 'warn')
          (
            tint: Tokens.warn,
            title: '${r.label} comes from your system',
            note: 'Whatever version is installed',
            action: 'Review',
            onTap: () => _go('tools'),
          ),
      if (speech != null && speech.state != 'ok')
        (
          tint: Tokens.warn,
          title: 'No speech model',
          note: 'Transcribe needs a ggml .bin beside whisper-cli',
          action: 'Tools',
          onTap: () => _go('tools'),
        ),
      if ((startup ?? 0) >= kStartupBudgetMs)
        (
          tint: Tokens.warn,
          title: 'A slow start',
          note: '$startup ms to the first frame',
          action: null,
          onTap: null,
        ),
      if (mem >= 500)
        (
          tint: Tokens.warn,
          title: 'High memory',
          note: '$mem MB for the whole app',
          action: null,
          onTap: null,
        ),
      if (frames >= kSlowFrameBudget)
        (
          tint: Tokens.warn,
          title: 'Many slow frames',
          note: '$frames over 16 ms this session',
          action: null,
          onTap: null,
        ),
      for (final r in platform)
        if (r.state == 'warn')
          (
            tint: Tokens.warn,
            title: '${r.label}: ${r.value.toLowerCase()}',
            note: 'The desktop did not answer for it',
            action: 'Platform',
            onTap: () => _go('platform'),
          ),
    ];

    final details = [
      'Tulipix ${st.appVersion}',
      if (renderer != null) 'Renderer: ${renderer.value}',
      if (player != null) 'Player: ${player.value}',
      if (onnx != null) 'ONNX editor ops: ${onnx.value}',
      if (alloc != null) 'Allocator: ${alloc.value}',
      'Startup: ${startup ?? '?'} ms · Memory: $mem MB · Slow frames: $frames',
      for (final r in tools) '${r.label}: ${r.value}',
      if (speech != null) 'Speech model: ${speech.value}',
      'Window frame: ${framed ? 'drawn by Tulipix' : 'the system'}',
      for (final r in platform) '${r.label}: ${r.value}',
      if (powerNow != null) 'Power: ${powerNow.value}',
      if (formats != null) 'Audio formats: ${formats.value}',
      if (gapless != null) 'Gapless unverified: ${gapless.value}',
      if (video != null) 'Video formats: ${video.value}',
      if (toolFormats != null) 'Tools formats: ${toolFormats.value}',
      if (books != null) 'Book formats: ${books.value}',
    ].join('\n');

    // ── overview ──
    Widget? control(SettingItem? r, IconData icon, String title, String note,
            Color tint, {bool shell = false}) =>
        r == null
            ? null
            : _ControlTile(
                icon: icon,
                title: title,
                note: note,
                tint: tint,
                on: r.on_,
                onChanged: (v) => _flip(r, v, shell: shell),
              );
    final controls = [
      control(notif, Icons.notifications_outlined, 'Notifications',
          'Downloads, Tools jobs, bills due', Tokens.brand),
      control(power, Icons.battery_std_outlined, 'Battery aware',
          'Rescans wait on a low battery', Tokens.ok),
      control(ytAuto, Icons.update, 'Keep yt-dlp current',
          'Checks weekly, updates itself', Tokens.secTools),
      control(accent, Icons.palette_outlined, 'System accent',
          'Sections take your desktop colour', Tokens.brand2, shell: true),
      control(fontScale, Icons.format_size, 'OS text size',
          'Follow the desktop\'s scale', Tokens.brand2, shell: true),
    ].whereType<Widget>().toList();

    // This build: who made it and what it opens, last on Overview.
    final gaps = gapless == null || gapless.value == 'None'
        ? const <String>{}
        : gapless.value
            .split(' · ')
            .map((s) => s.trim().toLowerCase())
            .toSet();
    Widget formatRow(String title, SettingItem? r, String count) {
      final exts = r!.value.split(' · ');
      return Padding(
        padding: const EdgeInsets.symmetric(vertical: 10),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SizedBox(
              width: 96,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(title,
                      style: TextStyle(
                          fontSize: 12,
                          fontWeight: FontWeight.w600,
                          color: t.text)),
                  Text(count.isEmpty ? '${exts.length} formats' : count,
                      style: TextStyle(fontSize: 11, color: t.textDim)),
                ],
              ),
            ),
            Expanded(
              child: Wrap(
                spacing: 5,
                runSpacing: 5,
                children: [
                  for (final f in exts)
                    _FormatChip(f, unverified: gaps.contains(f.toLowerCase())),
                ],
              ),
            ),
          ],
        ),
      );
    }

    Widget kv(String k, String v) => Padding(
          padding: const EdgeInsets.symmetric(vertical: 4),
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

    Widget overview() => Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Expanded(
                  child: Text('Right now, read as the page draws',
                      style: TextStyle(fontSize: 12, color: t.textDim)),
                ),
                SmallBtn(
                  label: 'Refresh',
                  icon: Icons.refresh,
                  onTap: () => _c.send(const SettingsCmd.refresh()),
                ),
              ],
            ),
            const SizedBox(height: 10),
            // The answer before the detail.
            _Vitals([
              (
                label: 'Tools',
                value: '${tools.length - missing}',
                unit: 'of ${tools.length} ready',
                note: missing == 0 ? 'All found' : '$missing missing',
                frac: tools.isEmpty
                    ? null
                    : (tools.length - missing) / tools.length,
                tint: missing == 0 ? Tokens.ok : Tokens.warn,
              ),
              (
                label: 'Startup',
                value: startup == null ? '…' : '$startup',
                unit: 'ms',
                note: 'Budget ${kStartupBudgetMs ~/ 1000} s',
                frac: (startup ?? 0) / kStartupBudgetMs,
                tint: (startup ?? 0) < kStartupBudgetMs ? Tokens.ok : Tokens.warn,
              ),
              (
                label: 'Memory',
                value: '$mem',
                unit: 'MB',
                note: 'Whole app, player included',
                frac: mem / 500,
                tint: mem < 500 ? Tokens.ok : Tokens.warn,
              ),
              (
                label: 'Slow frames',
                value: '$frames',
                unit: 'this session',
                note: 'Over 16 ms · fine under $kSlowFrameBudget',
                frac: frames / kSlowFrameBudget,
                tint: frames < kSlowFrameBudget ? Tokens.ok : Tokens.warn,
              ),
              (
                label: 'Thumbnail cache',
                value: cacheMb >= 1024
                    ? (cacheMb / 1024).toStringAsFixed(1)
                    : '$cacheMb',
                unit: cacheMb >= 1024 ? 'GB' : 'MB',
                note: 'Redrawn when needed',
                frac: null,
                tint: Tokens.secTools,
              ),
            ]),
            const SizedBox(height: TileGrid.gap),
            TileGrid([
              (
                span: 4,
                child: SettingsTile(
                  icon: Icons.tune,
                  tint: Tokens.brand,
                  title: 'Quick controls',
                  note: 'The switches you reach for, one tap each',
                  child: _rowsOf(controls),
                ),
              ),
              (
                span: 2,
                child: SettingsTile(
                  icon: issues.isEmpty
                      ? Icons.check_circle_outline
                      : Icons.error_outline,
                  tint: issues.isEmpty ? Tokens.ok : Tokens.warn,
                  title: 'Needs attention',
                  note: issues.isEmpty
                      ? 'Nothing right now'
                      : 'Only what is off, each with its fix',
                  child: issues.isEmpty
                      ? Text(
                          'Every tool is found, and startup, memory and '
                          'frames are inside budget.',
                          style: TextStyle(fontSize: 12, color: t.textDim))
                      : Column(
                          crossAxisAlignment: CrossAxisAlignment.stretch,
                          children: [
                            for (var i = 0; i < issues.length; i++) ...[
                              if (i > 0) const SizedBox(height: 8),
                              _IssueLine(issues[i]),
                            ],
                          ],
                        ),
                ),
              ),
              (
                span: 6,
                child: SettingsTile(
                  icon: Icons.handyman_outlined,
                  tint: Tokens.secTools,
                  title: 'Tools at a glance',
                  note: 'Where each one is found. Tools updates them and sets '
                      'the folder',
                  trailing: [
                    SmallBtn(label: 'Manage tools', onTap: () => _go('tools')),
                  ],
                  child: Wrap(
                    spacing: 8,
                    runSpacing: 8,
                    children: [for (final r in tools) _ToolPill(r)],
                  ),
                ),
              ),
              (
                span: 2,
                child: SettingsTile(
                  icon: Icons.memory,
                  tint: Tokens.secSettings,
                  title: 'This build',
                  note: '',
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      Center(
                        child: AppMark(
                          size: 56,
                          radius: 14,
                          choice:
                              ShellController.instance.state?.logoChoice ?? 0,
                        ),
                      ),
                      const SizedBox(height: 10),
                      Text('Tulipix',
                          textAlign: TextAlign.center,
                          style: TextStyle(
                              fontSize: 22,
                              fontWeight: FontWeight.w700,
                              letterSpacing: -0.1,
                              color: t.text)),
                      Text('Version ${st.appVersion}',
                          textAlign: TextAlign.center,
                          style: TextStyle(fontSize: 12, color: t.textDim)),
                      const SizedBox(height: 8),
                      Text('© 2026 — Developed by Atish',
                          textAlign: TextAlign.center,
                          style: TextStyle(fontSize: 11.5, color: t.textDim)),
                      const SizedBox(height: 12),
                      if (renderer != null) kv('Renderer', renderer.value),
                      if (player != null) kv('Player', player.value),
                      if (onnx != null) kv('ONNX editor ops', onnx.value),
                      if (alloc != null) kv('Allocator', alloc.value),
                    ],
                  ),
                ),
              ),
              (
                span: 4,
                child: SettingsTile(
                  icon: Icons.description_outlined,
                  tint: Tokens.secTools,
                  title: 'Supported file formats',
                  note: gaps.isEmpty
                      ? 'What each section opens'
                      : 'Outlined in amber: gapless playback not checked yet',
                  child: Lines([
                    if (formats != null) formatRow('Audio', formats, ''),
                    if (video != null) formatRow('Video', video, ''),
                    if (toolFormats != null)
                      formatRow('Tools', toolFormats, 'Convert to · Save as'),
                    if (books != null) formatRow('Books', books, ''),
                  ]),
                ),
              ),
            ]),
          ],
        );

    // ── tools ──
    String sourceOf(SettingItem r) => r.value.split(' · ').first;
    String detailOf(SettingItem r) {
      final version = r.value.split(' · ').skip(1).join(' · ');
      return switch (r.state) {
        'error' => 'Not found',
        'warn' => 'Version unknown',
        _ => version.isEmpty ? 'Ready' : version,
      };
    }

    // The folder and yt-dlp's options side by side, then the tools three to a
    // row: seven and the speech model leave two on the last.
    Widget toolsTab() => TileGrid([
          if (binDir != null)
            (
              span: 3,
              child: SettingsTile(
                icon: Icons.folder_open_outlined,
                tint: Tokens.secTools,
                title: 'Tools folder',
                note: 'Optional. A copy here wins over every other, and '
                    'applies at once',
                fill: true,
                child: Spread(
                  gap: 12,
                  [
                    Row(
                      children: [
                        Expanded(
                          child: RowControl(
                              row: binDir, controller: _c, below: true),
                        ),
                        const SizedBox(width: 8),
                        SmallBtn(
                          label: 'Browse…',
                          icon: Icons.folder_open_outlined,
                          onTap: _browseToolsDir,
                        ),
                        if (dirSet) ...[
                          const SizedBox(width: 8),
                          SmallBtn(
                              label: 'Reset',
                              ghost: true,
                              onTap: _resetToolsDir),
                        ],
                      ],
                    ),
                    _Lookup(dirSet: dirSet),
                  ],
                ),
              ),
            ),
          if (ytAuto != null || clients != null)
            (
              span: 3,
              child: SettingsTile(
                icon: Icons.download_outlined,
                tint: Tokens.secTools,
                title: 'yt-dlp options',
                note: 'How the downloader keeps up as sites change',
                fill: true,
                child: Lines(spread: true, [
                  if (ytAuto != null)
                    SettingLine(
                      title: 'Keep up to date',
                      note: 'Checks weekly',
                      trailing: SettingSwitch(
                          on: ytAuto.on_, onChanged: (v) => _flip(ytAuto, v)),
                    ),
                  if (clients != null)
                    SettingLine(
                      title: 'Player clients',
                      note: 'Blank: yt-dlp\'s own. Only if an issue asks',
                      below: true,
                      trailing:
                          RowControl(row: clients, controller: _c, below: true),
                    ),
                ]),
              ),
            ),
          for (final r in tools)
            (
              span: 2,
              child: _ToolCard(
                name: r.label,
                what: _tools[r.label] ?? '',
                source: sourceOf(r),
                state: r.state,
                detail: detailOf(r),
                // yt-dlp updates itself; the others open their download page.
                action: r.state == 'error' ? 'Get' : 'Update',
                primary: r.state == 'error',
                busy: st.taskKey == 'tool-update:${r.label}',
                tooltip: r.label == 'yt-dlp'
                    ? 'Download the newest yt-dlp'
                    : 'Open the ${r.label} download page',
                onAction: () => _c.sendAction('tool-update:${r.label}'),
              ),
            ),
          if (speech != null)
            (
              span: 2,
              child: _ToolCard(
                name: 'Speech model',
                what: 'What whisper-cli listens with',
                source: speech.state == 'ok' ? 'Found' : 'Missing',
                state: speech.state,
                detail: speech.value,
              ),
            ),
        ]);

    // ── platform ──
    final scale = MediaQuery.textScalerOf(context).scale(100).round();
    Widget platformTab() => TileGrid([
          (
            span: 3,
            child: SettingsTile(
              icon: Icons.palette_outlined,
              tint: Tokens.brand2,
              title: 'Look & feel',
              note: 'Let the desktop decide, or keep Tulipix\'s own',
              child: Lines([
                if (accent != null)
                  Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      SettingLine(
                        title: 'Follow system accent',
                        note: accent.desc,
                        trailing: SettingSwitch(
                            on: accent.on_,
                            onChanged: (v) => _flip(accent, v, shell: true)),
                      ),
                      // The section colours as they are drawn now: one
                      // colour when the desktop's accent is followed.
                      Padding(
                        padding: const EdgeInsets.only(bottom: 10),
                        child: Row(
                          children: [
                            for (final s in const [
                              Section.home,
                              Section.videos,
                              Section.music,
                              Section.books,
                              Section.tools,
                              Section.transfer,
                            ])
                              Container(
                                width: 16,
                                height: 16,
                                margin: const EdgeInsets.only(right: 5),
                                decoration: BoxDecoration(
                                  color: Tokens.accentOf(s),
                                  borderRadius: BorderRadius.circular(5),
                                ),
                              ),
                          ],
                        ),
                      ),
                    ],
                  ),
                if (fontScale != null)
                  Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      SettingLine(
                        title: 'Honour OS font scale',
                        note: fontScale.desc,
                        trailing: SettingSwitch(
                            on: fontScale.on_,
                            onChanged: (v) =>
                                _flip(fontScale, v, shell: true)),
                      ),
                      Padding(
                        padding: const EdgeInsets.only(bottom: 10),
                        child: Row(
                          children: [
                            Text('Aa',
                                style: TextStyle(
                                    fontSize: 18,
                                    fontWeight: FontWeight.w600,
                                    color: t.textDim)),
                            const SizedBox(width: 10),
                            Text('$scale % right now',
                                style: _mono.copyWith(
                                    fontSize: 11, color: t.textDim)),
                          ],
                        ),
                      ),
                    ],
                  ),
                if (notif != null)
                  SettingLine(
                    title: 'Notifications',
                    note: notif.desc,
                    trailing: SettingSwitch(
                        on: notif.on_, onChanged: (v) => _flip(notif, v)),
                  ),
              ]),
            ),
          ),
          (
            span: 3,
            child: SettingsTile(
              icon: Icons.desktop_windows_outlined,
              tint: Tokens.secSettings,
              title: 'What the desktop gives',
              note: 'Readings only. The OS sandbox is in Security',
              child: Lines([
                SettingLine(
                  title: 'Window frame',
                  trailing: StateChip(
                      framed ? 'Drawn by Tulipix' : 'The system’s',
                      tint: Tokens.ok),
                ),
                for (final r in platform)
                  SettingLine(
                    title: r.label,
                    trailing: StateChip(r.value, tint: _stateTint(r.state)),
                  ),
              ]),
            ),
          ),
          (
            span: 4,
            child: SettingsTile(
              icon: Icons.photo_library_outlined,
              tint: Tokens.secTools,
              title: 'Thumbnail cache',
              note: 'Every thumbnail is drawn again the next time it is '
                  'needed',
              trailing: [
                SmallBtn(
                  label: 'Clear the cache',
                  onTap:
                      cacheMb == 0 ? null : () => _c.sendAction('clear-thumbs'),
                ),
              ],
              child: Text(cache?.value ?? '—',
                  style: TextStyle(
                      fontSize: 26,
                      fontWeight: FontWeight.w700,
                      color: t.text,
                      fontFeatures: const [FontFeature.tabularFigures()])),
            ),
          ),
          (
            span: 2,
            child: SettingsTile(
              icon: Icons.battery_std_outlined,
              tint: Tokens.ok,
              title: 'Background work',
              note: 'What may run while you are not looking',
              child: Lines([
                if (power != null)
                  SettingLine(
                    title: 'Battery aware',
                    note: power.desc,
                    trailing: SettingSwitch(
                        on: power.on_, onChanged: (v) => _flip(power, v)),
                  ),
                if (powerNow != null)
                  SettingLine(
                    title: 'Power now',
                    trailing: StateChip(powerNow.value,
                        tint: _stateTint(powerNow.state)),
                  ),
              ]),
            ),
          ),
          if (mcp != null)
            (
              span: 6,
              child: SettingsTile(
                icon: Icons.hub_outlined,
                tint: Tokens.brand,
                title: 'MCP server',
                note: 'An AI agent on this computer, reading your library',
                trailing: [
                  StateChip(mcp.on_ ? 'On' : 'Off',
                      tint: mcp.on_ ? Tokens.ok : null),
                ],
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Lines([
                      SettingLine(
                        title: 'MCP server',
                        note: mcp.desc,
                        trailing: SettingSwitch(
                            on: mcp.on_, onChanged: (v) => _flip(mcp, v)),
                      ),
                      if (mcpWrite != null)
                        SettingLine(
                          title: 'Let agents change things',
                          note: mcpWrite.desc,
                          trailing: SettingSwitch(
                              on: mcpWrite.on_,
                              onChanged: (v) => _flip(mcpWrite, v)),
                        ),
                      if (mcpTools != null)
                        SettingLine(
                          title: 'Tools offered',
                          trailing: StateChip(mcpTools.value,
                              tint: _stateTint(mcpTools.state)),
                        ),
                      if (mcpReads != null)
                        SettingLine(
                          title: 'Reads',
                          trailing: StateChip(mcpReads.value),
                        ),
                    ]),
                    if (mcpCommand != null) ...[
                      const SizedBox(height: 12),
                      _Snippet(
                        label: 'Command',
                        text: mcpCommand.value,
                        onCopy: () => _copy(mcpCommand.value, 'the command'),
                      ),
                    ],
                    if (mcpConfig != null) ...[
                      const SizedBox(height: 8),
                      _Snippet(
                        label: 'Claude Desktop config',
                        text: mcpConfig.value,
                        note: 'Paste into claude_desktop_config.json, then '
                            'restart Claude Desktop',
                        onCopy: () =>
                            _copy(mcpConfig.value, 'the config block'),
                      ),
                    ],
                  ],
                ),
              ),
            ),
        ]);

    return SettingsPageBody(
      head: SettingsHead.forTab(
        'advanced',
        note: 'The tools Tulipix runs, how it is performing, what your desktop '
            'gives it, and how this copy was built',
        actions: [
          SmallBtn(
            label: 'Copy diagnostics',
            icon: Icons.copy_outlined,
            large: true,
            onTap: () async {
              await Clipboard.setData(ClipboardData(text: details));
              _c.say('Copied the diagnostics.');
            },
          ),
        ],
      ),
      children: [
        Align(
          alignment: Alignment.centerLeft,
          child: Seg(
            options: const ['overview', 'tools', 'platform'],
            labels: [
              'Overview',
              missing == 0 ? 'Tools' : 'Tools · $missing',
              'Platform',
            ],
            value: _tab,
            onPick: _go,
          ),
        ),
        switch (_tab) {
          'tools' => toolsTab(),
          'platform' => platformTab(),
          _ => overview(),
        },
        if (rest.isNotEmpty) MoreTile(rows: rest, controller: _c),
      ],
    );
  }
}

/// The five vitals across the top of Overview: a dot, the figure, how far it
/// is into its budget, and what it means. Outside the tile grid, so it may
/// measure.
class _Vitals extends StatelessWidget {
  const _Vitals(this.vitals);

  final List<_VitalData> vitals;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget card(_VitalData v) => Container(
          padding: const EdgeInsets.fromLTRB(14, 13, 14, 12),
          decoration: context.skin.surface(SurfaceRole.card, radius: 16) ??
              BoxDecoration(
                color: t.panel2,
                borderRadius: BorderRadius.circular(16),
                border: Border.all(color: t.outline),
              ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Container(
                    width: 7,
                    height: 7,
                    decoration:
                        BoxDecoration(color: v.tint, shape: BoxShape.circle),
                  ),
                  const SizedBox(width: 7),
                  Expanded(
                    child: Text(v.label,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 11.5,
                            fontWeight: FontWeight.w500,
                            color: t.textDim)),
                  ),
                ],
              ),
              const SizedBox(height: 6),
              Text.rich(
                TextSpan(children: [
                  TextSpan(
                      text: v.value,
                      style: TextStyle(
                          fontSize: 22,
                          fontWeight: FontWeight.w700,
                          color: t.text,
                          fontFeatures: const [FontFeature.tabularFigures()])),
                  TextSpan(
                      text: ' ${v.unit}',
                      style: TextStyle(fontSize: 12, color: t.textDim)),
                ]),
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
              const SizedBox(height: 8),
              if (v.frac != null)
                _Meter(v.frac!, v.tint)
              else
                const SizedBox(height: 5),
              const SizedBox(height: 7),
              Text(v.note,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11, color: t.textDim)),
            ],
          ),
        );
    return LayoutBuilder(
      builder: (context, box) {
        final per = box.maxWidth >= 900
            ? vitals.length
            : box.maxWidth >= 560
                ? 3
                : 2;
        return _AdvancedTabState._rowsOf(
            [for (final v in vitals) card(v)], per: per, gap: 12);
      },
    );
  }
}

/// How far into its budget a figure is.
class _Meter extends StatelessWidget {
  const _Meter(this.frac, this.tint);

  final double frac;
  final Color tint;

  @override
  Widget build(BuildContext context) => ClipRRect(
        borderRadius: BorderRadius.circular(3),
        child: LinearProgressIndicator(
          value: frac.clamp(0.0, 1.0),
          minHeight: 5,
          color: tint,
          backgroundColor: context.tokens.outline,
        ),
      );
}

/// A switch as a tile: tinted when on, the whole face is the button.
class _ControlTile extends StatelessWidget {
  const _ControlTile({
    required this.icon,
    required this.title,
    required this.note,
    required this.tint,
    required this.on,
    required this.onChanged,
  });

  final IconData icon;
  final String title;
  final String note;
  final Color tint;
  final bool on;
  final ValueChanged<bool> onChanged;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: Colors.transparent,
      child: InkWell(
        borderRadius: BorderRadius.circular(16),
        onTap: () => onChanged(!on),
        child: AnimatedContainer(
          duration: t.reduceMotion
              ? Duration.zero
              : const Duration(milliseconds: 200),
          padding: const EdgeInsets.all(13),
          decoration: BoxDecoration(
            color: on ? Color.alphaBlend(tint.withValues(alpha: 0.10), t.panel) : t.panel,
            borderRadius: BorderRadius.circular(16),
            border: Border.all(
                color: on ? tint.withValues(alpha: 0.55) : t.outline),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(
                children: [
                  Container(
                    width: 32,
                    height: 32,
                    decoration: BoxDecoration(
                      color: on ? tint : t.outline,
                      borderRadius: BorderRadius.circular(10),
                    ),
                    child: Icon(icon,
                        size: 17, color: on ? Colors.white : t.textDim),
                  ),
                  const Spacer(),
                  SettingSwitch(on: on, onChanged: onChanged),
                ],
              ),
              const SizedBox(height: 10),
              Text(title,
                  style: TextStyle(
                      fontSize: 13, fontWeight: FontWeight.w600, color: t.text)),
              const SizedBox(height: 2),
              Text(note,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11.5, color: t.textDim)),
            ],
          ),
        ),
      ),
    );
  }
}

/// One thing that is off: a severity stripe, what and why, and its fix.
class _IssueLine extends StatelessWidget {
  const _IssueLine(this.issue);

  final _Issue issue;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ClipRRect(
      borderRadius: BorderRadius.circular(12),
      child: Container(
        decoration: BoxDecoration(
          color: t.panel,
          borderRadius: BorderRadius.circular(12),
          border: Border.all(color: t.outline),
        ),
        child: IntrinsicHeight(
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Container(width: 4, color: issue.tint),
              Expanded(
                child: Padding(
                  padding: const EdgeInsets.fromLTRB(11, 9, 8, 9),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(issue.title,
                          style: TextStyle(
                              fontSize: 12.5,
                              fontWeight: FontWeight.w600,
                              color: t.text)),
                      Text(issue.note,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 11.5, color: t.textDim)),
                    ],
                  ),
                ),
              ),
              if (issue.action != null)
                Center(
                  child: Padding(
                    padding: const EdgeInsets.only(right: 9),
                    child: SmallBtn(label: issue.action!, onTap: issue.onTap),
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A tool on Overview: a dot for its state, its name, where it comes from.
class _ToolPill extends StatelessWidget {
  const _ToolPill(this.row);

  final SettingItem row;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 11, vertical: 6),
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(999),
        border: Border.all(color: t.outline),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 7,
            height: 7,
            decoration: BoxDecoration(
                color: stateTint(row.state, t), shape: BoxShape.circle),
          ),
          const SizedBox(width: 8),
          Text(row.label, style: _mono.copyWith(fontSize: 11.5, color: t.text)),
          const SizedBox(width: 6),
          Text(row.value,
              style: TextStyle(fontSize: 11, color: t.textDim)),
        ],
      ),
    );
  }
}

/// The surface a tool card sits on.
class _Card extends StatelessWidget {
  const _Card({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(14),
      decoration: context.skin.surface(SurfaceRole.card, radius: 16) ??
          BoxDecoration(
            color: t.panel2,
            borderRadius: BorderRadius.circular(16),
            border: Border.all(color: t.outline),
          ),
      child: child,
    );
  }
}

/// One tool: its name, what it does, where this copy comes from, and its fix.
class _ToolCard extends StatelessWidget {
  const _ToolCard({
    required this.name,
    required this.what,
    required this.source,
    required this.state,
    required this.detail,
    this.action,
    this.primary = false,
    this.busy = false,
    this.tooltip,
    this.onAction,
  });

  final String name;
  final String what;
  final String source;
  final String state;
  final String detail;
  final String? action;
  final bool primary;

  /// Its update is running (yt-dlp's, in the job slot).
  final bool busy;
  final String? tooltip;
  final VoidCallback? onAction;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return _Card(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisAlignment: MainAxisAlignment.spaceBetween,
        children: [
          Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Row(
                children: [
                  Expanded(
                    child: Text(name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: _mono.copyWith(
                            fontSize: 13,
                            fontWeight: FontWeight.w600,
                            color: t.text)),
                  ),
                  StateChip(source, tint: stateTint(state, t)),
                ],
              ),
              const SizedBox(height: 6),
              Text(what,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11.5, color: t.textDim)),
            ],
          ),
          const SizedBox(height: 12),
          Row(
            children: [
              Expanded(
                child: Text(detail,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: _mono.copyWith(fontSize: 11, color: t.textDim)),
              ),
              if (action != null)
                SmallBtn(
                  label: action!,
                  ghost: !primary,
                  primary: primary,
                  busy: busy,
                  tooltip: tooltip,
                  onTap: busy ? null : onAction,
                ),
            ],
          ),
        ],
      ),
    );
  }
}

/// The order a tool is looked for in, with the steps that apply lit.
class _Lookup extends StatelessWidget {
  const _Lookup({required this.dirSet});

  final bool dirSet;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget step(int n, String label, bool lit) => Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
          decoration: BoxDecoration(
            color: lit ? Tokens.secTools.withValues(alpha: 0.10) : null,
            borderRadius: BorderRadius.circular(9),
            border: Border.all(
                color: lit
                    ? Tokens.secTools.withValues(alpha: 0.55)
                    : t.outlineStrong),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text('$n',
                  style: _mono.copyWith(fontSize: 10.5, color: t.textDim)),
              const SizedBox(width: 7),
              Text(label,
                  style: TextStyle(
                      fontSize: 11.5, color: lit ? t.text : t.textDim)),
            ],
          ),
        );
    Widget arrow() =>
        Text('→', style: TextStyle(fontSize: 12, color: t.textDim));
    return Wrap(
      spacing: 6,
      runSpacing: 6,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        step(1, 'Your folder', dirSet),
        arrow(),
        step(2, 'App updates', true),
        arrow(),
        step(3, 'Bundled', true),
        arrow(),
        step(4, 'System PATH', true),
      ],
    );
  }
}

/// A switch still waiting for its feature, and the phase that brings it.
class _FormatChip extends StatelessWidget {
  const _FormatChip(this.ext, {this.unverified = false});

  final String ext;
  final bool unverified;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
      decoration: BoxDecoration(
        color: t.bg,
        borderRadius: BorderRadius.circular(6),
        border: Border.all(
            color: unverified ? Tokens.warn.withValues(alpha: 0.7) : t.outline),
      ),
      child: Text(ext.toUpperCase(),
          style: _mono.copyWith(
              fontSize: 10.5,
              fontWeight: FontWeight.w500,
              color: unverified ? Tokens.warn : t.textDim)),
    );
  }
}

/// A block of text meant to be copied rather than read: a command line, a
/// configuration block. Monospaced, scrollable sideways rather than wrapped —
/// a path broken across lines is a path that will not paste.
class _Snippet extends StatelessWidget {
  const _Snippet({
    required this.label,
    required this.text,
    required this.onCopy,
    this.note,
  });

  final String label;
  final String text;
  final String? note;
  final VoidCallback onCopy;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Expanded(
              child: Text(label,
                  style: TextStyle(
                      fontSize: 11.5,
                      fontWeight: FontWeight.w600,
                      color: t.textDim)),
            ),
            SmallBtn(
                label: 'Copy', icon: Icons.copy_outlined, onTap: onCopy),
          ],
        ),
        const SizedBox(height: 6),
        Container(
          padding: const EdgeInsets.fromLTRB(10, 8, 10, 8),
          decoration: BoxDecoration(
            color: t.bg,
            borderRadius: BorderRadius.circular(10),
            border: Border.all(color: t.outline),
          ),
          child: SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: Text(text,
                maxLines: 1,
                softWrap: false,
                style: TextStyle(
                    fontFamily: 'monospace', fontSize: 11.5, color: t.text)),
          ),
        ),
        if (note != null) ...[
          const SizedBox(height: 5),
          Text(note!, style: TextStyle(fontSize: 11, color: t.textDim)),
        ],
      ],
    );
  }
}

/// One profile: its name, where its library lives, and the one thing that can
/// be done to it — Use for another, Rename for the one in use.
class _ProfileLine extends StatelessWidget {
  const _ProfileLine({
    required this.row,
    required this.first,
    required this.onTap,
    this.onDelete,
  });

  final SettingItem row;
  final bool first;
  final VoidCallback onTap;

  /// Null for the profile in use and for the first one: neither can go.
  final VoidCallback? onDelete;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final active = row.state == 'ok';
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 9, 10, 9),
      decoration: BoxDecoration(
        color: t.bg,
        border: first ? null : Border(top: BorderSide(color: t.outline)),
      ),
      child: Row(
        children: [
          Icon(active ? Icons.person : Icons.person_outline,
              size: 16, color: active ? Tokens.ok : t.textDim),
          const SizedBox(width: 12),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(row.label,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 12.5,
                        fontWeight: active ? FontWeight.w600 : FontWeight.w400,
                        color: t.text)),
                Text(row.desc,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11.5, color: t.textDim)),
              ],
            ),
          ),
          const SizedBox(width: 12),
          if (onDelete != null) ...[
            SmallBtn(label: 'Delete', ghost: true, danger: true, onTap: onDelete),
            const SizedBox(width: 6),
          ],
          SmallBtn(label: row.btn, primary: !active, onTap: onTap),
        ],
      ),
    );
  }
}
