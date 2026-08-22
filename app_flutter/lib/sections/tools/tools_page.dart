// The Tools shell: pick a category, pick an operation, fill its form, watch it
// run.
//
// The queue is always visible on the right, because the point of the section is
// that you can start something long and go do something else.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/tools.dart';
import 'tools_controller.dart';
import 'tools_form.dart';
import 'tools_queue.dart';

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
                    ? const Center(child: CircularProgressIndicator())
                    : Row(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          Expanded(
                            flex: 3,
                            child: st.resultOpen
                                ? ResultPanel(controller: _c, state: st)
                                : (st.activeOp.isEmpty
                                    ? _OpGrid(controller: _c, state: st)
                                    : ToolForm(controller: _c, state: st)),
                          ),
                          Container(width: 1, color: t.nHair),
                          SizedBox(
                            width: 380,
                            child: QueuePanel(controller: _c, state: st),
                          ),
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
              if (st != null)
                Text(st.queueStatus,
                    style: TextStyle(fontSize: 12, color: t.nInk2)),
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

/// The operations in the open category, as cards.
class _OpGrid extends StatelessWidget {
  const _OpGrid({required this.controller, required this.state});

  final ToolsController controller;
  final ToolsState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (state.ops.isEmpty) {
      return Center(
        child: Text('Nothing here.',
            style: TextStyle(fontSize: 13, color: t.nInk2)),
      );
    }
    return SingleChildScrollView(
      padding: const EdgeInsets.all(20),
      child: Wrap(
        spacing: 14,
        runSpacing: 14,
        children: [
          for (final op in state.ops)
            _OpCard(
              op: op,
              onTap: () => controller.send(ToolsCmd.openTool(kind: op.kind)),
            ),
        ],
      ),
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
    final tint = tintFor(widget.op.category);
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 130),
          width: 260,
          padding: const EdgeInsets.all(16),
          decoration: BoxDecoration(
            color: _hover ? tint.withValues(alpha: 0.10) : t.nCard,
            borderRadius: BorderRadius.circular(12),
            border: Border.all(color: _hover ? tint : t.nHair),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Row(
                children: [
                  Container(
                    width: 8,
                    height: 8,
                    decoration:
                        BoxDecoration(color: tint, shape: BoxShape.circle),
                  ),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      widget.op.label,
                      style: TextStyle(
                          fontSize: 14,
                          fontWeight: FontWeight.w700,
                          color: t.nInk),
                    ),
                  ),
                ],
              ),
              if (widget.op.info.isNotEmpty) ...[
                const SizedBox(height: 8),
                Text(
                  widget.op.info,
                  maxLines: 3,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11.5, height: 1.4, color: t.nInk2),
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
        builder: (ctx) => AlertDialog(
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
                      color:
                          s.available ? const Color(0xFF2FBF71) : Tokens.error,
                      size: 18,
                    ),
                    title: Text(s.name),
                    subtitle: Text(s.detail),
                    trailing: Text(
                      s.source,
                      style:
                          TextStyle(fontSize: 11, color: context.tokens.nInk2),
                    ),
                  ),
              ],
            ),
          ),
          actions: [
            FilledButton(
                onPressed: () => Navigator.pop(ctx), child: const Text('Done')),
          ],
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
