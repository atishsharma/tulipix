// The transfer drawer: sync, copy, verify, dedupe, saved jobs and the activity
// log.
//
// All five are the same shape — two paths and some flags — which is why they
// share a drawer rather than being five screens.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/cloud.dart';
import 'cloud_controller.dart';

const List<({String id, String label, IconData icon})> _tabs = [
  (id: 'sync', label: 'Sync', icon: Icons.sync),
  (id: 'copy', label: 'Copy', icon: Icons.content_copy),
  (id: 'verify', label: 'Verify', icon: Icons.fact_check_outlined),
  (id: 'dedupe', label: 'Dedupe', icon: Icons.filter_none),
  (id: 'jobs', label: 'Jobs', icon: Icons.schedule),
  (id: 'log', label: 'Activity', icon: Icons.history),
];

class CloudTools extends StatelessWidget {
  const CloudTools({super.key, required this.controller, required this.state});

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Container(
          padding: const EdgeInsets.fromLTRB(12, 12, 6, 8),
          decoration: BoxDecoration(
            border: Border(bottom: BorderSide(color: t.nHair)),
          ),
          child: Row(
            children: [
              Expanded(
                child: SingleChildScrollView(
                  scrollDirection: Axis.horizontal,
                  child: Row(
                    children: [
                      for (final tab in _tabs)
                        Padding(
                          padding: const EdgeInsets.only(right: 6),
                          child: _Tab(
                            tab: tab,
                            active: state.toolsTab == tab.id,
                            onTap: () async {
                              await controller
                                  .send(CloudCmd.setToolsTab(name: tab.id));
                            },
                          ),
                        ),
                    ],
                  ),
                ),
              ),
              IconButton(
                iconSize: 18,
                icon: const Icon(Icons.close),
                onPressed: () => controller.send(const CloudCmd.toolsToggle()),
              ),
            ],
          ),
        ),
        Expanded(
          child: switch (state.toolsTab) {
            'copy' => _Copy(controller: controller, state: state),
            'verify' => _Verify(controller: controller, state: state),
            'dedupe' => _Dedupe(controller: controller, state: state),
            'jobs' => _Jobs(controller: controller, state: state),
            'log' => _Log(controller: controller, state: state),
            _ => _Sync(controller: controller, state: state),
          },
        ),
      ],
    );
  }
}

class _Tab extends StatelessWidget {
  const _Tab({required this.tab, required this.active, required this.onTap});

  final ({String id, String label, IconData icon}) tab;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: active ? Tokens.secCloud.withValues(alpha: 0.16) : t.nChip,
      borderRadius: BorderRadius.circular(14),
      child: InkWell(
        borderRadius: BorderRadius.circular(14),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(tab.icon,
                  size: 13, color: active ? Tokens.secCloud : t.nInk2),
              const SizedBox(width: 5),
              Text(tab.label,
                  style: TextStyle(
                      fontSize: 11.5,
                      fontWeight: active ? FontWeight.w700 : FontWeight.w500,
                      color: active ? Tokens.secCloud : t.nInk)),
            ],
          ),
        ),
      ),
    );
  }
}

/// A labelled path box. Both sides accept `remote:path` or a local path, which
/// is what makes one form cover upload, download and cloud-to-cloud.
class _PathField extends StatefulWidget {
  const _PathField({
    required this.label,
    required this.value,
    required this.onChanged,
    this.hint = 'remote:path  or  /local/path',
  });

  final String label;
  final String value;
  final ValueChanged<String> onChanged;
  final String hint;

  @override
  State<_PathField> createState() => _PathFieldState();
}

class _PathFieldState extends State<_PathField> {
  late final TextEditingController _c =
      TextEditingController(text: widget.value);

  @override
  void didUpdateWidget(_PathField old) {
    super.didUpdateWidget(old);
    if (widget.value != _c.text && widget.value != old.value) {
      _c.text = widget.value;
    }
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.only(bottom: 14),
        child: TextField(
          controller: _c,
          decoration: InputDecoration(
            isDense: true,
            labelText: widget.label,
            hintText: widget.hint,
            border: const OutlineInputBorder(),
          ),
          onChanged: widget.onChanged,
        ),
      );
}

class _Runner extends StatelessWidget {
  const _Runner({
    required this.controller,
    required this.label,
    required this.icon,
    required this.onRun,
    this.extra,
  });

  final CloudController controller;
  final String label;
  final IconData icon;
  final VoidCallback onRun;
  final Widget? extra;

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.only(top: 6),
        child: Row(
          children: [
            if (extra != null) Expanded(child: extra!) else const Spacer(),
            FilledButton.icon(
              style: FilledButton.styleFrom(backgroundColor: Tokens.secCloud),
              icon: Icon(icon, size: 18),
              label: Text(label),
              onPressed: controller.busy ? null : onRun,
            ),
          ],
        ),
      );
}

class _Sync extends StatelessWidget {
  const _Sync({required this.controller, required this.state});

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    void set(String k, String v) =>
        controller.send(CloudCmd.setField(key: k, value: v));

    return ListView(
      padding: const EdgeInsets.all(16),
      children: [
        Text(
          'Sync makes the destination match the source — including deleting '
          'what is no longer there. Bisync keeps both sides in step instead.',
          style: TextStyle(fontSize: 11.5, height: 1.5, color: t.nInk2),
        ),
        const SizedBox(height: 14),
        _PathField(
          label: 'Source',
          value: state.syncSrc,
          onChanged: (v) => set('sync_src', v),
        ),
        _PathField(
          label: 'Destination',
          value: state.syncDst,
          onChanged: (v) => set('sync_dst', v),
        ),
        _Choice(
          label: 'Direction',
          options: const ['oneway', 'bisync'],
          value: state.syncDirection,
          onPick: (v) => set('sync_direction', v),
        ),
        Row(
          children: [
            Expanded(
              child: _Small(
                label: 'Bandwidth limit',
                hint: '2M, 500k…',
                value: state.syncBwlimit,
                onChanged: (v) => set('sync_bwlimit', v),
              ),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: _Small(
                label: 'Parallel transfers',
                hint: '4',
                value: state.syncTransfers,
                onChanged: (v) => set('sync_transfers', v),
              ),
            ),
          ],
        ),
        const SizedBox(height: 10),
        _Choice(
          label: 'Schedule',
          options: const ['manual', 'hourly', 'daily', 'weekly'],
          value: state.syncInterval,
          onPick: (v) => set('sync_interval', v),
        ),
        _Runner(
          controller: controller,
          label: 'Run sync',
          icon: Icons.sync,
          onRun: () => controller.send(const CloudCmd.runSync()),
          extra: TextButton.icon(
            icon: const Icon(Icons.bookmark_add_outlined, size: 16),
            label: const Text('Save as a job'),
            onPressed: () => controller.send(const CloudCmd.saveJob()),
          ),
        ),
      ],
    );
  }
}

class _Copy extends StatelessWidget {
  const _Copy({required this.controller, required this.state});

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    void set(String k, String v) =>
        controller.send(CloudCmd.setField(key: k, value: v));

    return ListView(
      padding: const EdgeInsets.all(16),
      children: [
        Text(
          'Copy adds without removing. Server-side keeps the bytes inside the '
          'provider when both ends are the same one — much faster, and it does '
          'not touch your connection.',
          style: TextStyle(fontSize: 11.5, height: 1.5, color: t.nInk2),
        ),
        const SizedBox(height: 14),
        _PathField(
          label: 'Source',
          value: state.copySrc,
          onChanged: (v) => set('copy_src', v),
        ),
        _PathField(
          label: 'Destination',
          value: state.copyDst,
          onChanged: (v) => set('copy_dst', v),
        ),
        SwitchListTile(
          contentPadding: EdgeInsets.zero,
          dense: true,
          title: const Text('Move (delete the source afterwards)'),
          value: state.copyMove,
          activeThumbColor: Tokens.secCloud,
          onChanged: (v) => set('copy_move', v ? 'true' : 'false'),
        ),
        SwitchListTile(
          contentPadding: EdgeInsets.zero,
          dense: true,
          title: const Text('Server-side when possible'),
          value: state.copyServerside,
          activeThumbColor: Tokens.secCloud,
          onChanged: (v) => set('copy_serverside', v ? 'true' : 'false'),
        ),
        _Runner(
          controller: controller,
          label: state.copyMove ? 'Move' : 'Copy',
          icon: Icons.content_copy,
          onRun: () => controller.send(const CloudCmd.runCopy()),
        ),
      ],
    );
  }
}

class _Verify extends StatelessWidget {
  const _Verify({required this.controller, required this.state});

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    void set(String k, String v) =>
        controller.send(CloudCmd.setField(key: k, value: v));

    return ListView(
      padding: const EdgeInsets.all(16),
      children: [
        Text(
          'Checks that two trees hold the same bytes, by hash where the '
          'provider offers one. Nothing is written.',
          style: TextStyle(fontSize: 11.5, height: 1.5, color: t.nInk2),
        ),
        const SizedBox(height: 14),
        _PathField(
          label: 'Source',
          value: state.verifySrc,
          onChanged: (v) => set('verify_src', v),
        ),
        _PathField(
          label: 'Compare against',
          value: state.verifyDst,
          onChanged: (v) => set('verify_dst', v),
        ),
        _Runner(
          controller: controller,
          label: 'Verify',
          icon: Icons.fact_check_outlined,
          onRun: () => controller.send(const CloudCmd.runVerify()),
        ),
      ],
    );
  }
}

class _Dedupe extends StatelessWidget {
  const _Dedupe({required this.controller, required this.state});

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    void set(String k, String v) =>
        controller.send(CloudCmd.setField(key: k, value: v));

    return ListView(
      padding: const EdgeInsets.all(16),
      children: [
        Text(
          'Some providers — Drive especially — allow two files with the same '
          'name in one folder. This picks which one survives.',
          style: TextStyle(fontSize: 11.5, height: 1.5, color: t.nInk2),
        ),
        const SizedBox(height: 14),
        _PathField(
          label: 'Folder',
          value: state.dedupeTarget,
          hint: 'remote:path',
          onChanged: (v) => set('dedupe_target', v),
        ),
        _Choice(
          label: 'Keep',
          options: const [
            'newest',
            'oldest',
            'largest',
            'smallest',
            'first',
            'rename',
            'skip',
          ],
          value: state.dedupeMode,
          onPick: (v) => set('dedupe_mode', v),
        ),
        _Runner(
          controller: controller,
          label: 'Dedupe',
          icon: Icons.filter_none,
          onRun: () => controller.send(const CloudCmd.runDedupe()),
        ),
      ],
    );
  }
}

class _Jobs extends StatelessWidget {
  const _Jobs({required this.controller, required this.state});

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (state.jobs.isEmpty) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Text(
            'No saved jobs.\nFill in Sync and press “Save as a job”.',
            textAlign: TextAlign.center,
            style: TextStyle(fontSize: 12.5, color: t.nInk2),
          ),
        ),
      );
    }
    return ListView.builder(
      itemCount: state.jobs.length,
      itemBuilder: (_, i) {
        final j = state.jobs[i];
        return Container(
          padding: const EdgeInsets.fromLTRB(16, 10, 6, 10),
          decoration: BoxDecoration(
            border: Border(bottom: BorderSide(color: t.nHair)),
          ),
          child: Row(
            children: [
              Switch(
                value: j.enabled,
                activeThumbColor: Tokens.secCloud,
                onChanged: (v) =>
                    controller.send(CloudCmd.toggleJob(id: j.id, enabled: v)),
              ),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text('${j.src}  →  ${j.dst}',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 12, color: t.nInk)),
                    Text(
                      [
                        j.direction,
                        j.intervalS == 0
                            ? 'manual'
                            : '${(j.intervalS / 3600).round()} h',
                        'ran ${fmtWhen(j.lastRun)}',
                        if (j.bwlimit.isNotEmpty) j.bwlimit,
                      ].join('  ·  '),
                      style: TextStyle(fontSize: 10.5, color: t.nInk3),
                    ),
                  ],
                ),
              ),
              IconButton(
                iconSize: 17,
                tooltip: 'Run now',
                icon: const Icon(Icons.play_arrow),
                onPressed: () => controller.send(CloudCmd.runJob(id: j.id)),
              ),
              IconButton(
                iconSize: 16,
                tooltip: 'Delete',
                icon: const Icon(Icons.close),
                onPressed: () => controller.send(CloudCmd.deleteJob(id: j.id)),
              ),
            ],
          ),
        );
      },
    );
  }
}

class _Log extends StatelessWidget {
  const _Log({required this.controller, required this.state});

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      children: [
        Expanded(
          child: state.xfers.isEmpty
              ? Center(
                  child: Text('Nothing has run yet.',
                      style: TextStyle(fontSize: 12.5, color: t.nInk2)),
                )
              : ListView.builder(
                  itemCount: state.xfers.length,
                  itemBuilder: (_, i) {
                    final x = state.xfers[i];
                    return ListTile(
                      dense: true,
                      leading: Icon(
                        x.ok ? Icons.check_circle : Icons.error_outline,
                        size: 16,
                        color: x.ok ? const Color(0xFF2FBF71) : Tokens.error,
                      ),
                      title: Text(x.detail,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 12, color: t.nInk)),
                      subtitle: Text('${x.kind}  ·  ${fmtWhen(x.at)}',
                          style: TextStyle(fontSize: 10.5, color: t.nInk3)),
                      trailing: IconButton(
                        iconSize: 15,
                        icon: const Icon(Icons.close),
                        onPressed: () =>
                            controller.send(CloudCmd.deleteLogRow(id: x.id)),
                      ),
                    );
                  },
                ),
        ),
        if (state.xfers.isNotEmpty)
          Padding(
            padding: const EdgeInsets.all(8),
            child: TextButton.icon(
              icon: const Icon(Icons.clear_all, size: 16),
              label: const Text('Clear the log'),
              onPressed: () => controller.send(const CloudCmd.clearLog()),
            ),
          ),
      ],
    );
  }
}

class _Choice extends StatelessWidget {
  const _Choice({
    required this.label,
    required this.options,
    required this.value,
    required this.onPick,
  });

  final String label;
  final List<String> options;
  final String value;
  final ValueChanged<String> onPick;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(bottom: 14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(label, style: TextStyle(fontSize: 11.5, color: t.nInk2)),
          const SizedBox(height: 6),
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [
              for (final o in options)
                ChoiceChip(
                  label: Text(o, style: const TextStyle(fontSize: 11.5)),
                  selected: value == o,
                  selectedColor: Tokens.secCloud.withValues(alpha: 0.2),
                  onSelected: (_) => onPick(o),
                ),
            ],
          ),
        ],
      ),
    );
  }
}

class _Small extends StatefulWidget {
  const _Small({
    required this.label,
    required this.hint,
    required this.value,
    required this.onChanged,
  });

  final String label;
  final String hint;
  final String value;
  final ValueChanged<String> onChanged;

  @override
  State<_Small> createState() => _SmallState();
}

class _SmallState extends State<_Small> {
  late final TextEditingController _c =
      TextEditingController(text: widget.value);

  @override
  void didUpdateWidget(_Small old) {
    super.didUpdateWidget(old);
    if (widget.value != _c.text && widget.value != old.value) {
      _c.text = widget.value;
    }
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => TextField(
        controller: _c,
        decoration: InputDecoration(
          isDense: true,
          labelText: widget.label,
          hintText: widget.hint,
          border: const OutlineInputBorder(),
        ),
        onChanged: widget.onChanged,
      );
}
