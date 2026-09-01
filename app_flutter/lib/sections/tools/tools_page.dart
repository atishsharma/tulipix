// The Tools shell: pick a category, pick an operation, fill its form, watch
// what it is about to do, then queue it.
//
// The queue used to sit in the layout permanently. It is a drawer now — the
// right-hand half of an open tool belongs to its preview, and a queue you are
// not watching does not need a third of the window.

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/tools.dart';
import 'tools_controller.dart';
import 'tools_form.dart';
import 'tools_preview.dart';
import 'tools_queue.dart';

/// Wide enough for the form and a preview worth looking at.
const double _formWidth = 400;
const double _drawerWidth = 378;

class ToolsPage extends StatefulWidget {
  const ToolsPage({super.key});

  @override
  State<ToolsPage> createState() => _ToolsPageState();
}

class _ToolsPageState extends State<ToolsPage> {
  final ToolsController _c = ToolsController();
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
          child: Column(
            children: [
              _Header(controller: _c, search: _search),
              if (_c.error != null) _ErrorBanner(controller: _c),
              Expanded(
                child: st == null
                    ? FirstLoad(error: _c.error, onRetry: _c.refresh)
                    : Stack(
                        children: [
                          Positioned.fill(
                            child: _Work(controller: _c, state: st),
                          ),
                          _QueueDrawer(controller: _c, state: st),
                        ],
                      ),
              ),
              if (st != null) ConsoleStrip(controller: _c, state: st),
            ],
          ),
        );
      },
    );
  }
}

/// Catalogue, or the open tool split into form and preview.
class _Work extends StatelessWidget {
  const _Work({required this.controller, required this.state});

  final ToolsController controller;
  final ToolsState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (state.resultOpen) {
      return ResultPanel(controller: controller, state: state);
    }
    if (state.activeOp.isEmpty) {
      return _OpGrid(controller: controller, state: state);
    }
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SizedBox(
          width: _formWidth,
          child: ToolForm(controller: controller, state: state),
        ),
        Container(width: 1, color: t.nHair),
        Expanded(child: PreviewPane(controller: controller, state: state)),
      ],
    );
  }
}

/// The queue, off to the side until it has something to say.
class _QueueDrawer extends StatelessWidget {
  const _QueueDrawer({required this.controller, required this.state});

  final ToolsController controller;
  final ToolsState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final open = controller.drawerOpen;
    return AnimatedPositioned(
      duration: Duration(milliseconds: t.reduceMotion ? 0 : 240),
      curve: Curves.easeOutCubic,
      top: 0,
      bottom: 0,
      right: open ? 0 : -_drawerWidth,
      width: _drawerWidth,
      // Off-screen it is still in the tree, so it must not eat clicks meant
      // for the preview under it.
      child: IgnorePointer(
        ignoring: !open,
        child: Material(
          color: t.panel,
          elevation: open ? 8 : 0,
          child: DecoratedBox(
            decoration: BoxDecoration(
              border: Border(left: BorderSide(color: t.nHair)),
            ),
            child: QueuePanel(controller: controller, state: state),
          ),
        ),
      ),
    );
  }
}

class _Header extends StatelessWidget {
  const _Header({required this.controller, required this.search});

  final ToolsController controller;
  final TextEditingController search;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = controller.state;
    return Container(
      padding: const EdgeInsets.fromLTRB(20, 14, 20, 10),
      decoration: BoxDecoration(
        color: t.panel,
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Column(
        children: [
          Row(
            children: [
              const Icon(Icons.build_outlined,
                  color: Tokens.secTools, size: 22),
              const SizedBox(width: 10),
              Text('Tools',
                  style: TextStyle(
                      fontSize: 20,
                      fontWeight: FontWeight.w800,
                      color: t.nInk)),
              const SizedBox(width: 14),
              if (st != null && st.jobs.isNotEmpty)
                _QueuePill(controller: controller, state: st),
              const Spacer(),
              SizedBox(
                width: 280,
                child: TextField(
                  controller: search,
                  decoration: const InputDecoration(
                    isDense: true,
                    hintText: 'Find a tool…',
                    prefixIcon: Icon(Icons.search, size: 18),
                    border: OutlineInputBorder(),
                  ),
                  onChanged: (v) =>
                      controller.send(ToolsCmd.search(text: v.trim())),
                ),
              ),
              const SizedBox(width: 8),
              _ToolchainButton(controller: controller),
              _DownloadsButton(controller: controller),
            ],
          ),
          const SizedBox(height: 10),
          if (st != null && st.query.isEmpty)
            SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: Row(
                children: [
                  for (final tab in toolTabs)
                    _Tab(
                      tab: tab,
                      active: st.category == tab.id,
                      onTap: () =>
                          controller.send(ToolsCmd.setCategory(name: tab.id)),
                    ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

/// The only trace of the queue in the chrome, and only once there is one.
class _QueuePill extends StatelessWidget {
  const _QueuePill({required this.controller, required this.state});

  final ToolsController controller;
  final ToolsState state;

  @override
  Widget build(BuildContext context) {
    final running = state.jobs.where((j) => j.state == 'running').length;
    final open = controller.drawerOpen;
    return Material(
      color: Tokens.secTools.withValues(alpha: open ? 0.22 : 0.12),
      borderRadius: BorderRadius.circular(999),
      child: InkWell(
        borderRadius: BorderRadius.circular(999),
        onTap: () => controller.setDrawer(!open),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (running > 0)
                const SizedBox(
                  width: 10,
                  height: 10,
                  child: CircularProgressIndicator(
                      strokeWidth: 2, color: Tokens.secTools),
                )
              else
                const Icon(Icons.list_alt, size: 13, color: Tokens.secTools),
              const SizedBox(width: 7),
              Text(
                state.queueStatus == 'Idle'
                    ? '${state.jobs.length} in queue'
                    : state.queueStatus,
                style: const TextStyle(
                    fontSize: 11.5,
                    fontWeight: FontWeight.w600,
                    color: Tokens.secTools),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _Tab extends StatelessWidget {
  const _Tab({required this.tab, required this.active, required this.onTap});

  final ({String id, String label, IconData icon, Color tint}) tab;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(right: 8),
      child: Material(
        color: active ? tab.tint.withValues(alpha: 0.16) : t.nChip,
        borderRadius: BorderRadius.circular(18),
        child: InkWell(
          borderRadius: BorderRadius.circular(18),
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 8),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(tab.icon, size: 15, color: active ? tab.tint : t.nInk2),
                const SizedBox(width: 7),
                Text(
                  tab.label,
                  style: TextStyle(
                    fontSize: 12.5,
                    fontWeight: active ? FontWeight.w700 : FontWeight.w500,
                    color: active ? tab.tint : t.nInk,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// The operations in the open category, as square tiles five to a row.
class _OpGrid extends StatelessWidget {
  const _OpGrid({required this.controller, required this.state});

  final ToolsController controller;
  final ToolsState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (state.ops.isEmpty) {
      return Center(
        child: Text(
          state.query.isEmpty
              ? 'Nothing here.'
              : 'No tool matches “${state.query}”.',
          style: TextStyle(fontSize: 13, color: t.nInk2),
        ),
      );
    }
    final heading = state.query.isEmpty
        ? toolTabs
            .firstWhere((x) => x.id == state.category,
                orElse: () => toolTabs.first)
            .label
        : 'Results';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(20, 18, 20, 12),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.baseline,
            textBaseline: TextBaseline.alphabetic,
            children: [
              Text(heading,
                  style: TextStyle(
                      fontSize: 14,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
              const SizedBox(width: 10),
              Text(
                '${state.ops.length} tool${state.ops.length == 1 ? '' : 's'}',
                style: TextStyle(fontSize: 11.5, color: t.nInk3),
              ),
            ],
          ),
        ),
        Expanded(
          child: LayoutBuilder(
            builder: (context, box) {
              // Five to a row is the shape; below that the squares would be
              // postage stamps, so narrow windows drop columns instead.
              final columns = box.maxWidth >= 720
                  ? 5
                  : box.maxWidth >= 520
                      ? 4
                      : 3;
              return GridView.builder(
                padding: const EdgeInsets.fromLTRB(20, 0, 20, 26),
                gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
                  crossAxisCount: columns,
                  crossAxisSpacing: 12,
                  mainAxisSpacing: 12,
                ),
                itemCount: state.ops.length,
                itemBuilder: (_, i) => _OpCard(
                  op: state.ops[i],
                  onTap: () => controller.openTool(state.ops[i].kind),
                ),
              );
            },
          ),
        ),
      ],
    );
  }
}

class _OpCard extends StatefulWidget {
  const _OpCard({required this.op, required this.onTap});

  final OpRow op;
  final VoidCallback onTap;

  @override
  State<_OpCard> createState() => _OpCardState();
}

class _OpCardState extends State<_OpCard> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // A tool whose binary is not installed keeps its place in the grid and
    // loses its colour. It stays tappable: the form and the preview are where
    // the reason is spelled out, and hiding the tile would only make the
    // missing tool look like a missing feature.
    final off = widget.op.missing.isNotEmpty;
    final tint = off ? t.nInk3 : tintFor(widget.op.category);
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: Duration(milliseconds: t.reduceMotion ? 0 : 130),
          padding: const EdgeInsets.all(13),
          decoration: BoxDecoration(
            color: _hover ? tint.withValues(alpha: 0.09) : t.nCard,
            borderRadius: BorderRadius.circular(13),
            border: Border.all(color: _hover ? tint : t.nHair),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Container(
                width: 34,
                height: 34,
                decoration: BoxDecoration(
                  color: tint.withValues(alpha: 0.16),
                  borderRadius: BorderRadius.circular(10),
                ),
                child: Icon(
                  opIcon(widget.op.kind, widget.op.category),
                  size: 18,
                  color: tint,
                ),
              ),
              const Spacer(),
              Text(
                widget.op.label,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 13,
                    height: 1.25,
                    fontWeight: FontWeight.w700,
                    color: off ? t.nInk3 : t.nInk),
              ),
              if (off) ...[
                const SizedBox(height: 5),
                Row(
                  children: [
                    Icon(Icons.block,
                        size: 11, color: t.nInk3),
                    const SizedBox(width: 4),
                    Flexible(
                      child: Text(
                        'Needs ${widget.op.missing}',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 10.5,
                            height: 1.35,
                            fontWeight: FontWeight.w600,
                            color: t.nInk3),
                      ),
                    ),
                  ],
                ),
              ] else if (widget.op.info.isNotEmpty) ...[
                const SizedBox(height: 4),
                Flexible(
                  child: Text(
                    widget.op.info,
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 10.5, height: 1.35, color: t.nInk3),
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// Which external binaries are present. Shown once here rather than as twenty
/// identical job failures.
class _ToolchainButton extends StatelessWidget {
  const _ToolchainButton({required this.controller});

  final ToolsController controller;

  @override
  Widget build(BuildContext context) {
    final statuses = controller.state?.statuses ?? const <ToolStatus>[];
    final missing = statuses.where((s) => !s.available).length;
    return IconButton(
      tooltip: missing == 0
          ? 'Toolchain'
          : '$missing tool${missing == 1 ? '' : 's'} missing',
      icon: Badge(
        isLabelVisible: missing > 0,
        label: Text('$missing'),
        child: const Icon(Icons.handyman_outlined),
      ),
      onPressed: () => showDialog<void>(
        context: context,
        // AnimatedBuilder, or the dialog is a photograph: `showDialog`'s
        // builder runs once, so an Update that changes the version would leave
        // the row still showing the old one.
        builder: (ctx) => AnimatedBuilder(
          animation: controller,
          builder: (ctx, _) => AlertDialog(
            title: const Text('Toolchain'),
            content: SizedBox(
              width: 520,
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  for (final s in statuses)
                    ListTile(
                      dense: true,
                      leading: Icon(
                        s.available ? Icons.check_circle : Icons.cancel,
                        color: s.available
                            ? const Color(0xFF2FBF71)
                            : Tokens.error,
                        size: 18,
                      ),
                      title: Text(s.name),
                      // The version, where the tool reports one. yt-dlp's is the
                      // number that matters: a download failing with 403 is
                      // nearly always a binary some weeks old, and the row used
                      // to say only "bundled".
                      subtitle: Text(
                        s.version.isEmpty
                            ? s.detail
                            : '${s.detail} · ${s.version}',
                      ),
                      trailing: Row(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Text(
                            s.source,
                            style: TextStyle(
                                fontSize: 11, color: context.tokens.nInk2),
                          ),
                          if (s.updatable && s.available) ...[
                            const SizedBox(width: 8),
                            _UpdateToolButton(
                                controller: controller, name: s.name),
                          ],
                        ],
                      ),
                    ),
                ],
              ),
            ),
            actions: [
              FilledButton(
                  onPressed: () => Navigator.pop(ctx),
                  child: const Text('Done')),
            ],
          ),
        ),
      ),
    );
  }
}

class _DownloadsButton extends StatelessWidget {
  const _DownloadsButton({required this.controller});

  final ToolsController controller;

  @override
  Widget build(BuildContext context) {
    return IconButton(
      tooltip: 'Downloads',
      icon: const Icon(Icons.download_outlined),
      onPressed: () async {
        await controller.send(const ToolsCmd.listDownloads());
        if (!context.mounted) return;
        await showDialog<void>(
          context: context,
          builder: (ctx) => AnimatedBuilder(
            animation: controller,
            builder: (ctx, _) {
              final rows = controller.state?.downloads ?? const <DownloadRow>[];
              return AlertDialog(
                title: const Text('Downloads'),
                content: SizedBox(
                  width: 560,
                  height: 400,
                  child: rows.isEmpty
                      ? const Center(child: Text('Nothing downloaded yet.'))
                      : ListView.builder(
                          itemCount: rows.length,
                          itemBuilder: (_, i) => ListTile(
                            dense: true,
                            title: Text(rows[i].name,
                                maxLines: 1, overflow: TextOverflow.ellipsis),
                            subtitle: Text(
                              '${rows[i].meta}  ·  ${rows[i].path}',
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                            ),
                            trailing: IconButton(
                              icon: const Icon(Icons.delete_outline, size: 18),
                              tooltip: 'Delete the file',
                              onPressed: () => controller.send(
                                  ToolsCmd.removeDownload(path: rows[i].path)),
                            ),
                          ),
                        ),
                ),
                actions: [
                  FilledButton(
                      onPressed: () => Navigator.pop(ctx),
                      child: const Text('Done')),
                ],
              );
            },
          ),
        );
      },
    );
  }
}

class _ErrorBanner extends StatelessWidget {
  const _ErrorBanner({required this.controller});

  final ToolsController controller;

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

/// "Update" beside a tool that has its own release channel — yt-dlp, which
/// breaks on its own schedule as sites change.
///
/// The button reports the outcome by comparing the version before and after:
/// the bridge deliberately writes nothing to the session on success, because
/// the only message channel there is drawn as a red error banner.
class _UpdateToolButton extends StatefulWidget {
  const _UpdateToolButton({required this.controller, required this.name});

  final ToolsController controller;
  final String name;

  @override
  State<_UpdateToolButton> createState() => _UpdateToolButtonState();
}

class _UpdateToolButtonState extends State<_UpdateToolButton> {
  bool _busy = false;

  String? _versionOf(String name) {
    for (final s in widget.controller.state?.statuses ?? const <ToolStatus>[]) {
      if (s.name == name) return s.version;
    }
    return null;
  }

  Future<void> _run() async {
    final before = _versionOf(widget.name);
    setState(() => _busy = true);
    await widget.controller.send(ToolsCmd.updateTool(name: widget.name));
    if (!mounted) return;
    setState(() => _busy = false);
    final after = _versionOf(widget.name);
    final err = widget.controller.state?.error ?? '';
    final message = err.isNotEmpty
        ? err
        : (after != null && after != before && after.isNotEmpty
            ? '${widget.name} updated to $after'
            : '${widget.name} is already up to date');
    ScaffoldMessenger.of(context)
        .showSnackBar(SnackBar(content: Text(message)));
  }

  @override
  Widget build(BuildContext context) {
    if (_busy) {
      return const SizedBox(
        width: 18,
        height: 18,
        child: CircularProgressIndicator(strokeWidth: 2),
      );
    }
    return TextButton(
      onPressed: _run,
      child: const Text('Update', style: TextStyle(fontSize: 11.5)),
    );
  }
}
