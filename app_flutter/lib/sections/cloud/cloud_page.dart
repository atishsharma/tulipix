// The Cloud shell: remotes down the left, whatever one of them contains in the
// middle, and the transfer tools in a drawer on the right.

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/cloud.dart';
import 'cloud_connect.dart';
import 'cloud_controller.dart';
import 'cloud_tools.dart';

class CloudPage extends StatefulWidget {
  const CloudPage({super.key});

  @override
  State<CloudPage> createState() => _CloudPageState();
}

class _CloudPageState extends State<CloudPage> {
  final CloudController _c = CloudController();
  final TextEditingController _search = TextEditingController();

  @override
  void initState() {
    super.initState();
    _c.refresh();
  }

  @override
  void dispose() {
    _search.dispose();
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
          color: t.nCanvas,
          child: Stack(
            children: [
              Column(
                children: [
                  _Header(controller: _c, search: _search),
                  if (_c.transfer != null) _TransferBar(controller: _c),
                  if (_c.error != null) _ErrorBanner(controller: _c),
                  if (st != null && !st.rcloneOk) const _NoRclone(),
                  if (st != null && st.status.isNotEmpty)
                    _StatusBanner(message: st.status),
                  Expanded(
                    child: st == null
                        ? FirstLoad(error: _c.error, onRetry: _c.refresh)
                        : Row(
                            crossAxisAlignment: CrossAxisAlignment.stretch,
                            children: [
                              SizedBox(
                                width: 260,
                                child: _Remotes(controller: _c, state: st),
                              ),
                              Container(width: 1, color: t.nHair),
                              Expanded(
                                  child: _Browser(controller: _c, state: st)),
                              if (st.toolsOpen) ...[
                                Container(width: 1, color: t.nHair),
                                SizedBox(
                                  width: 420,
                                  child: CloudTools(controller: _c, state: st),
                                ),
                              ],
                            ],
                          ),
                  ),
                ],
              ),
              if (st != null && st.connectOpen)
                Positioned.fill(
                    child: ConnectWizard(controller: _c, state: st)),
            ],
          ),
        );
      },
    );
  }
}

class _Header extends StatelessWidget {
  const _Header({required this.controller, required this.search});

  final CloudController controller;
  final TextEditingController search;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = controller.state;
    return Container(
      padding: const EdgeInsets.fromLTRB(20, 14, 20, 12),
      decoration: BoxDecoration(
        color: t.panel,
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          const Icon(Icons.cloud_outlined, color: Tokens.secCloud, size: 22),
          const SizedBox(width: 10),
          Text('Cloud',
              style: TextStyle(
                  fontSize: 20, fontWeight: FontWeight.w800, color: t.nInk)),
          const SizedBox(width: 14),
          if (st != null && st.rcloneVersion.isNotEmpty)
            Text(st.rcloneVersion,
                style: TextStyle(fontSize: 11, color: t.nInk3)),
          const Spacer(),
          if (st != null && st.activeRemote.isNotEmpty) ...[
            SizedBox(
              width: 240,
              child: TextField(
                controller: search,
                decoration: const InputDecoration(
                  isDense: true,
                  hintText: 'Filter this folder…',
                  prefixIcon: Icon(Icons.search, size: 18),
                  border: OutlineInputBorder(),
                ),
                onChanged: (v) =>
                    controller.send(CloudCmd.search(text: v.trim())),
              ),
            ),
            const SizedBox(width: 8),
            IconButton(
              tooltip: 'Reload',
              icon: const Icon(Icons.refresh),
              onPressed: () => controller.send(const CloudCmd.reload()),
            ),
          ],
          IconButton(
            tooltip: 'Transfer tools',
            icon: Icon(Icons.swap_horiz,
                color: (st?.toolsOpen ?? false) ? Tokens.secCloud : null),
            onPressed: () => controller.send(const CloudCmd.toolsToggle()),
          ),
          FilledButton.icon(
            style: FilledButton.styleFrom(backgroundColor: Tokens.secCloud),
            icon: const Icon(Icons.add, size: 18),
            label: const Text('Connect'),
            onPressed: () => controller.send(const CloudCmd.connectOpen()),
          ),
        ],
      ),
    );
  }
}

class _Remotes extends StatelessWidget {
  const _Remotes({required this.controller, required this.state});

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.symmetric(vertical: 10),
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 4, 16, 8),
          child: Text('REMOTES',
              style: TextStyle(
                  fontSize: 10,
                  letterSpacing: 1.2,
                  fontWeight: FontWeight.w700,
                  color: t.nInk3)),
        ),
        if (state.remotes.isEmpty)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16),
            child: Text(
              'No remotes yet. Connect brings up rclone’s own list of '
              'providers — Drive, S3, Dropbox, SFTP and eighty others.',
              style: TextStyle(fontSize: 12, height: 1.5, color: t.nInk2),
            ),
          ),
        for (final r in state.remotes)
          _RemoteRow(
            controller: controller,
            remote: r,
            active: r.name == state.activeRemote,
          ),
        if (state.usage.isNotEmpty) ...[
          const SizedBox(height: 18),
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
            child: Text('USAGE',
                style: TextStyle(
                    fontSize: 10,
                    letterSpacing: 1.2,
                    fontWeight: FontWeight.w700,
                    color: t.nInk3)),
          ),
          for (final u in state.usage) _UsageBar(usage: u),
        ],
        const SizedBox(height: 12),
        Center(
          child: TextButton.icon(
            icon: const Icon(Icons.pie_chart_outline, size: 16),
            label: const Text('Check usage'),
            onPressed: () => controller.send(const CloudCmd.loadUsage()),
          ),
        ),
      ],
    );
  }
}

class _RemoteRow extends StatelessWidget {
  const _RemoteRow({
    required this.controller,
    required this.remote,
    required this.active,
  });

  final CloudController controller;
  final Remote remote;
  final bool active;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color:
          active ? Tokens.secCloud.withValues(alpha: 0.14) : Colors.transparent,
      child: InkWell(
        onTap: () => controller.send(CloudCmd.openRemote(name: remote.name)),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(16, 9, 6, 9),
          child: Row(
            children: [
              Icon(
                remote.mounted ? Icons.folder_special : Icons.cloud_queue,
                size: 17,
                color: active ? Tokens.secCloud : t.nInk2,
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(remote.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 13,
                            fontWeight:
                                active ? FontWeight.w700 : FontWeight.w500,
                            color: active ? Tokens.secCloud : t.nInk)),
                    Text(
                      remote.mounted
                          ? 'mounted · ${remote.mountPath}'
                          : remote.backend,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 10.5, color: t.nInk3),
                    ),
                  ],
                ),
              ),
              _RemoteMenu(controller: controller, remote: remote),
            ],
          ),
        ),
      ),
    );
  }
}

class _RemoteMenu extends StatelessWidget {
  const _RemoteMenu({required this.controller, required this.remote});

  final CloudController controller;
  final Remote remote;

  @override
  Widget build(BuildContext context) {
    return PopupMenuButton<String>(
      iconSize: 16,
      tooltip: 'Remote',
      icon: const Icon(Icons.more_vert),
      onSelected: (v) async {
        switch (v) {
          case 'edit':
            controller.send(CloudCmd.editRemote(name: remote.name));
          case 'mount':
            final path = await promptText(
              context,
              title: 'Mount ${remote.name}',
              label: 'Mount point (an empty directory)',
              hint: '/home/you/mnt/${remote.name}',
              confirm: 'Mount',
            );
            if (path != null) {
              await controller.send(CloudCmd.mount(
                name: remote.name,
                path: path,
                cache: 'full',
                maxAge: '1h',
              ));
            }
          case 'unmount':
            controller.send(CloudCmd.unmount(name: remote.name));
          case 'delete':
            final ok = await confirmAction(
              context,
              title: 'Remove ${remote.name}?',
              body: 'Deletes the rclone remote. Nothing in the cloud is '
                  'touched — only the connection on this machine.',
              action: 'Remove',
            );
            if (ok) {
              await controller.send(CloudCmd.deleteRemote(name: remote.name));
            }
        }
      },
      itemBuilder: (_) => [
        const PopupMenuItem(value: 'edit', child: Text('Edit…')),
        if (remote.mounted)
          const PopupMenuItem(value: 'unmount', child: Text('Unmount'))
        else
          const PopupMenuItem(value: 'mount', child: Text('Mount as a drive…')),
        const PopupMenuDivider(),
        const PopupMenuItem(value: 'delete', child: Text('Remove')),
      ],
    );
  }
}

class _UsageBar extends StatelessWidget {
  const _UsageBar({required this.usage});

  final UsageRow usage;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final frac =
        usage.total > 0 ? (usage.used / usage.total).clamp(0.0, 1.0) : 0.0;
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 4, 16, 10),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(usage.remote, style: TextStyle(fontSize: 11.5, color: t.nInk)),
          const SizedBox(height: 4),
          ClipRRect(
            borderRadius: BorderRadius.circular(2),
            child: LinearProgressIndicator(
              value: frac,
              minHeight: 4,
              backgroundColor: t.nHover,
              valueColor: AlwaysStoppedAnimation(
                  frac > 0.9 ? Tokens.error : Tokens.secCloud),
            ),
          ),
          const SizedBox(height: 3),
          Text(
            usage.total > 0
                ? '${fmtBytes(usage.used)} of ${fmtBytes(usage.total)}'
                : fmtBytes(usage.used),
            style: TextStyle(fontSize: 10, color: t.nInk3),
          ),
        ],
      ),
    );
  }
}

class _Browser extends StatelessWidget {
  const _Browser({required this.controller, required this.state});

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (state.activeRemote.isEmpty) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              width: 96,
              height: 96,
              decoration: BoxDecoration(
                color: Tokens.secCloud.withValues(alpha: 0.14),
                shape: BoxShape.circle,
              ),
              child: const Icon(Icons.cloud_outlined,
                  size: 40, color: Tokens.secCloud),
            ),
            const SizedBox(height: 16),
            Text('Pick a remote',
                style: TextStyle(
                    fontSize: 20, fontWeight: FontWeight.w700, color: t.nInk)),
            const SizedBox(height: 6),
            Text('Or connect a new one — everything rclone supports.',
                style: TextStyle(fontSize: 13, color: t.nInk2)),
          ],
        ),
      );
    }

    return Column(
      children: [
        _Crumbs(controller: controller, state: state),
        if (state.busy) const LinearProgressIndicator(minHeight: 2),
        _ColumnHeads(controller: controller, state: state),
        Expanded(
          child: state.entries.isEmpty
              ? Center(
                  child: Text(state.busy ? 'Listing…' : 'This folder is empty.',
                      style: TextStyle(fontSize: 13, color: t.nInk2)),
                )
              : ListView.builder(
                  itemCount: state.entries.length,
                  itemBuilder: (_, i) => _EntryRow(
                    controller: controller,
                    state: state,
                    entry: state.entries[i],
                  ),
                ),
        ),
        if (state.output.isNotEmpty)
          _Output(controller: controller, state: state),
      ],
    );
  }
}

class _Crumbs extends StatelessWidget {
  const _Crumbs({required this.controller, required this.state});

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 8, 12, 8),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          IconButton(
            iconSize: 18,
            tooltip: 'Up',
            icon: const Icon(Icons.arrow_upward),
            onPressed: state.path.isEmpty
                ? null
                : () => controller.send(const CloudCmd.up()),
          ),
          Expanded(
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: Row(
                children: [
                  for (final crumb in controller.crumbs) ...[
                    InkWell(
                      onTap: () =>
                          controller.send(CloudCmd.navigate(path: crumb.path)),
                      child: Padding(
                        padding: const EdgeInsets.symmetric(
                            horizontal: 6, vertical: 4),
                        child: Text(crumb.label,
                            style: TextStyle(fontSize: 12.5, color: t.nInk)),
                      ),
                    ),
                    Icon(Icons.chevron_right, size: 14, color: t.nInk3),
                  ],
                ],
              ),
            ),
          ),
          IconButton(
            iconSize: 18,
            tooltip: 'New folder',
            icon: const Icon(Icons.create_new_folder_outlined),
            onPressed: () async {
              final name = await promptText(
                context,
                title: 'New folder',
                label: 'Name',
                confirm: 'Create',
              );
              if (name != null) {
                await controller.send(CloudCmd.newFolder(name: name));
              }
            },
          ),
          IconButton(
            iconSize: 18,
            tooltip: 'Upload a file here',
            icon: const Icon(Icons.upload_file),
            onPressed: () async {
              final path = await promptText(
                context,
                title: 'Upload',
                label: 'Local file or folder',
                hint: '/home/you/photo.jpg',
                confirm: 'Upload',
              );
              if (path != null) {
                await controller.send(CloudCmd.uploadFile(local: path));
              }
            },
          ),
        ],
      ),
    );
  }
}

class _ColumnHeads extends StatelessWidget {
  const _ColumnHeads({required this.controller, required this.state});

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget head(String label, String key, {double? width, bool flex = false}) {
      final active = state.sortKey == key;
      final child = InkWell(
        onTap: () => controller.send(CloudCmd.setSort(key: key)),
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: 6),
          child: Row(
            children: [
              Text(label,
                  style: TextStyle(
                      fontSize: 11,
                      fontWeight: FontWeight.w600,
                      color: active ? Tokens.secCloud : t.nInk2)),
              if (active)
                Icon(
                    state.sortAsc ? Icons.arrow_drop_up : Icons.arrow_drop_down,
                    size: 16,
                    color: Tokens.secCloud),
            ],
          ),
        ),
      );
      return flex
          ? Expanded(child: child)
          : SizedBox(width: width, child: child);
    }

    return Container(
      padding: const EdgeInsets.fromLTRB(16, 0, 52, 0),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          const SizedBox(width: 26),
          head('Name', 'name', flex: true),
          head('Size', 'size', width: 110),
          head('Modified', 'date', width: 130),
        ],
      ),
    );
  }
}

class _EntryRow extends StatefulWidget {
  const _EntryRow({
    required this.controller,
    required this.state,
    required this.entry,
  });

  final CloudController controller;
  final CloudState state;
  final Entry entry;

  @override
  State<_EntryRow> createState() => _EntryRowState();
}

class _EntryRowState extends State<_EntryRow> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final e = widget.entry;
    return MouseRegion(
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: Material(
        color: _hover ? t.nHover : Colors.transparent,
        child: InkWell(
          onTap: e.isDir
              ? () => widget.controller.send(CloudCmd.navigate(
                    path: widget.state.path.isEmpty
                        ? e.name
                        : '${widget.state.path}/${e.name}',
                  ))
              : null,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(16, 7, 4, 7),
            child: Row(
              children: [
                Icon(
                  e.isDir ? Icons.folder : iconFor(e.name),
                  size: 17,
                  color: e.isDir ? const Color(0xFFF5A623) : t.nInk2,
                ),
                const SizedBox(width: 9),
                Expanded(
                  child: Text(e.name,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 12.5, color: t.nInk)),
                ),
                SizedBox(
                  width: 110,
                  child: Text(e.isDir ? '' : fmtBytes(e.size),
                      style: TextStyle(fontSize: 11.5, color: t.nInk2)),
                ),
                SizedBox(
                  width: 130,
                  child: Text(fmtModTime(e.modified),
                      style: TextStyle(fontSize: 11.5, color: t.nInk2)),
                ),
                _EntryMenu(controller: widget.controller, entry: e),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _EntryMenu extends StatelessWidget {
  const _EntryMenu({required this.controller, required this.entry});

  final CloudController controller;
  final Entry entry;

  @override
  Widget build(BuildContext context) {
    return PopupMenuButton<String>(
      iconSize: 16,
      tooltip: 'More',
      icon: const Icon(Icons.more_horiz),
      onSelected: (v) async {
        switch (v) {
          case 'download':
            final dest = await promptText(
              context,
              title: 'Download ${entry.name}',
              label: 'Save into',
              hint: '/home/you/Downloads',
              confirm: 'Download',
            );
            if (dest != null) {
              await controller
                  .send(CloudCmd.downloadEntry(name: entry.name, dest: dest));
            }
          case 'rename':
            final to = await promptText(
              context,
              title: 'Rename',
              label: 'New name',
              initial: entry.name,
              confirm: 'Rename',
            );
            if (to != null) {
              await controller.send(CloudCmd.rename(from: entry.name, to: to));
            }
          case 'share':
            controller.send(CloudCmd.shareLink(name: entry.name));
          case 'delete':
            final ok = await confirmAction(
              context,
              title: 'Delete ${entry.name}?',
              body: entry.isDir
                  ? 'Deletes the folder and everything in it, in the cloud.'
                  : 'Deletes the file in the cloud.',
            );
            if (ok) {
              await controller.send(
                  CloudCmd.deleteEntry(name: entry.name, isDir: entry.isDir));
            }
        }
      },
      itemBuilder: (_) => [
        const PopupMenuItem(value: 'download', child: Text('Download…')),
        const PopupMenuItem(value: 'rename', child: Text('Rename…')),
        if (!entry.isDir)
          const PopupMenuItem(value: 'share', child: Text('Share link')),
        const PopupMenuDivider(),
        const PopupMenuItem(value: 'delete', child: Text('Delete')),
      ],
    );
  }
}

/// The last multi-line thing rclone said — a check report, a share URL.
class _Output extends StatelessWidget {
  const _Output({required this.controller, required this.state});

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      constraints: const BoxConstraints(maxHeight: 160),
      width: double.infinity,
      decoration: BoxDecoration(
        color: t.nCard,
        border: Border(top: BorderSide(color: t.nHair)),
      ),
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(12),
        child: SelectableText(
          state.output,
          style: TextStyle(
              fontFamily: 'monospace',
              fontSize: 11,
              height: 1.5,
              color: t.nInk2),
        ),
      ),
    );
  }
}

class _TransferBar extends StatelessWidget {
  const _TransferBar({required this.controller});

  final CloudController controller;

  @override
  Widget build(BuildContext context) {
    final x = controller.transfer!;
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 8),
      color: Tokens.secCloud.withValues(alpha: 0.10),
      child: Row(
        children: [
          SizedBox(
            width: 180,
            child: LinearProgressIndicator(
              value: x.frac >= 0 ? x.frac : null,
              minHeight: 4,
              backgroundColor: t.nHover,
              valueColor: const AlwaysStoppedAnimation(Tokens.secCloud),
            ),
          ),
          const SizedBox(width: 12),
          Text(
            x.frac >= 0 ? '${x.label} · ${(x.frac * 100).round()}%' : x.label,
            style: TextStyle(fontSize: 12, color: t.nInk2),
          ),
        ],
      ),
    );
  }
}

class _NoRclone extends StatelessWidget {
  const _NoRclone();

  @override
  Widget build(BuildContext context) => Container(
        width: double.infinity,
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
        color: Tokens.error.withValues(alpha: 0.12),
        child: const Row(
          children: [
            Icon(Icons.warning_amber, size: 18, color: Tokens.error),
            SizedBox(width: 10),
            Expanded(
              child: Text(
                'rclone was not found. Everything in this section drives it, '
                'so nothing here will work until it is on PATH or bundled in tools/.',
                style: TextStyle(fontSize: 12, color: Tokens.error),
              ),
            ),
          ],
        ),
      );
}

class _ErrorBanner extends StatelessWidget {
  const _ErrorBanner({required this.controller});

  final CloudController controller;

  @override
  Widget build(BuildContext context) => Container(
        width: double.infinity,
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 10),
        color: Tokens.error.withValues(alpha: 0.14),
        child: Row(
          children: [
            const Icon(Icons.error_outline, size: 18, color: Tokens.error),
            const SizedBox(width: 10),
            Expanded(
              child: Text('${controller.error}',
                  style: const TextStyle(fontSize: 12, color: Tokens.error)),
            ),
            IconButton(
              iconSize: 16,
              icon: const Icon(Icons.close),
              onPressed: controller.clearError,
            ),
          ],
        ),
      );
}

class _StatusBanner extends StatelessWidget {
  const _StatusBanner({required this.message});

  final String message;

  @override
  Widget build(BuildContext context) => Container(
        width: double.infinity,
        padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 8),
        color: context.tokens.nChip,
        child: SelectableText(message,
            style: TextStyle(fontSize: 12, color: context.tokens.nInk2)),
      );
}
