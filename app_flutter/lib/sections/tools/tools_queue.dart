// The queue drawer, and the status rail under the whole section.
//
// The queue slides in when you press Run and otherwise stays out of the way.
// The whole reason the section is a queue and not a series of modal progress
// bars is that you should be able to start a two-hour transcode and go and do
// something else — which also means you should not have to look at it.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/tools.dart';
import 'tools_controller.dart';

class QueuePanel extends StatelessWidget {
  const QueuePanel({super.key, required this.controller, required this.state});

  final ToolsController controller;
  final ToolsState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 14, 8, 8),
          child: Row(
            children: [
              Text('Queue',
                  style: TextStyle(
                      fontSize: 14,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
              const SizedBox(width: 8),
              Text(state.queueStatus,
                  style: TextStyle(fontSize: 11, color: t.nInk2)),
              const Spacer(),
              _WorkerPicker(controller: controller, slots: state.workerSlots),
              IconButton(
                iconSize: 17,
                tooltip: 'Clear finished',
                icon: const Icon(Icons.clear_all),
                onPressed: () => controller.send(const ToolsCmd.clearQueue()),
              ),
              IconButton(
                iconSize: 17,
                tooltip: 'Hide the queue',
                icon: const Icon(Icons.close),
                onPressed: () => controller.setDrawer(false),
              ),
            ],
          ),
        ),
        Expanded(
          child: state.jobs.isEmpty
              ? Center(
                  child: Padding(
                    padding: const EdgeInsets.all(24),
                    child: Text(
                      'Nothing queued.\nPick a tool and press Run.',
                      textAlign: TextAlign.center,
                      style: TextStyle(fontSize: 12.5, color: t.nInk2),
                    ),
                  ),
                )
              : ListView.builder(
                  itemCount: state.jobs.length,
                  itemBuilder: (_, i) => _JobRow(
                    controller: controller,
                    job: state.jobs[i],
                  ),
                ),
        ),
      ],
    );
  }
}

class _WorkerPicker extends StatelessWidget {
  const _WorkerPicker({required this.controller, required this.slots});

  final ToolsController controller;
  final int slots;

  @override
  Widget build(BuildContext context) {
    return PopupMenuButton<int>(
      tooltip: 'How many jobs run at once',
      // More workers is not always faster: ffmpeg already uses every core, so
      // two transcodes at once mostly means both take twice as long.
      onSelected: (v) => controller.send(ToolsCmd.setWorkers(slots: v)),
      itemBuilder: (_) => [
        for (var i = 1; i <= 8; i++)
          CheckedPopupMenuItem(
            value: i,
            checked: slots == i,
            child: Text('$i at a time'),
          ),
      ],
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 8),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(Icons.speed, size: 15),
            const SizedBox(width: 4),
            Text('$slots', style: const TextStyle(fontSize: 12)),
          ],
        ),
      ),
    );
  }
}

class _JobRow extends StatelessWidget {
  const _JobRow({required this.controller, required this.job});

  final ToolsController controller;
  final Job job;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final look = jobLook(job.state);
    final progress = controller.progressOf(job);
    final running = job.state == 'running';
    // A report is the whole output of these two, so the row is the way in.
    final hasReport = job.state == 'done' &&
        (job.kind == 'mediainfo' ||
            job.kind == 'folder_diff' ||
            job.kind == 'hash');

    return Container(
      padding: const EdgeInsets.fromLTRB(16, 10, 8, 10),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(look.icon, size: 15, color: look.colour),
              const SizedBox(width: 8),
              Expanded(
                child: Text(job.label,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 12.5,
                        fontWeight: FontWeight.w600,
                        color: t.nInk)),
              ),
              if (running)
                Text('${(progress * 100).round()}%',
                    style: TextStyle(fontSize: 11, color: look.colour)),
              _JobMenu(controller: controller, job: job, hasReport: hasReport),
            ],
          ),
          if (running || job.state == 'paused') ...[
            const SizedBox(height: 6),
            ClipRRect(
              borderRadius: BorderRadius.circular(2),
              child: LinearProgressIndicator(
                value: progress.clamp(0.0, 1.0),
                minHeight: 3,
                backgroundColor: t.nHover,
                valueColor: AlwaysStoppedAnimation(look.colour),
              ),
            ),
          ],
          if (job.message.isNotEmpty) ...[
            const SizedBox(height: 4),
            Text(job.message,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 11,
                    color: job.state == 'failed' ? Tokens.error : t.nInk2)),
          ],
        ],
      ),
    );
  }
}

class _JobMenu extends StatelessWidget {
  const _JobMenu({
    required this.controller,
    required this.job,
    required this.hasReport,
  });

  final ToolsController controller;
  final Job job;
  final bool hasReport;

  @override
  Widget build(BuildContext context) {
    final active = job.state == 'running' || job.state == 'queued';
    return PopupMenuButton<String>(
      tooltip: 'Job',
      iconSize: 16,
      icon: const Icon(Icons.more_horiz),
      onSelected: (v) {
        if (v == 'result') {
          controller.send(ToolsCmd.showResult(id: job.id));
          return;
        }
        controller.send(ToolsCmd.queueAction(id: job.id, action: v));
      },
      itemBuilder: (_) => [
        if (hasReport)
          const PopupMenuItem(value: 'result', child: Text('Show result')),
        if (job.state == 'running' || job.state == 'queued')
          const PopupMenuItem(value: 'pause', child: Text('Pause')),
        if (job.state == 'paused')
          const PopupMenuItem(value: 'resume', child: Text('Resume')),
        if (active) const PopupMenuItem(value: 'cancel', child: Text('Cancel')),
        if (job.state == 'failed' || job.state == 'cancelled')
          const PopupMenuItem(value: 'retry', child: Text('Retry')),
        if (job.state == 'queued') ...[
          const PopupMenuDivider(),
          const PopupMenuItem(value: 'up', child: Text('Move up')),
          const PopupMenuItem(value: 'down', child: Text('Move down')),
        ],
        const PopupMenuDivider(),
        const PopupMenuItem(value: 'remove', child: Text('Remove')),
      ],
    );
  }
}

/// The bottom rail: one line saying whether anything is running, and the
/// console folded up behind it. Collapsed by default — it is the thing you
/// want only when something has gone wrong.
class ConsoleStrip extends StatefulWidget {
  const ConsoleStrip({
    super.key,
    required this.controller,
    required this.state,
  });

  final ToolsController controller;
  final ToolsState state;

  @override
  State<ConsoleStrip> createState() => _ConsoleStripState();
}

class _ConsoleStripState extends State<ConsoleStrip> {
  bool _open = false;
  final ScrollController _scroll = ScrollController();

  @override
  void didUpdateWidget(ConsoleStrip old) {
    super.didUpdateWidget(old);
    // Follow the tail: a console you have to scroll by hand shows you the
    // beginning of the problem, not the end of it.
    if (_open && _scroll.hasClients) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (_scroll.hasClients) {
          _scroll.jumpTo(_scroll.position.maxScrollExtent);
        }
      });
    }
  }

  @override
  void dispose() {
    _scroll.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final log = widget.state.log;
    final running =
        widget.state.jobs.any((j) => j.state == 'running');
    final tail = _lastLine(log);
    return Container(
      decoration: BoxDecoration(
        color: t.nCard,
        border: Border(top: BorderSide(color: t.nHair)),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 6, 10, 6),
            child: Row(
              children: [
                Container(
                  width: 6,
                  height: 6,
                  decoration: BoxDecoration(
                    shape: BoxShape.circle,
                    color: running ? const Color(0xFF2FBF71) : t.nInk3,
                  ),
                ),
                const SizedBox(width: 9),
                Text(
                  widget.state.queueStatus,
                  style: TextStyle(fontSize: 11, color: t.nInk2),
                ),
                if (tail.isNotEmpty) ...[
                  const SizedBox(width: 12),
                  Expanded(
                    child: Text(
                      tail,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontFamily: 'monospace',
                          fontSize: 11,
                          color: t.nInk3),
                    ),
                  ),
                ] else
                  const Spacer(),
                TextButton(
                  onPressed: () => setState(() => _open = !_open),
                  child: Text(_open ? 'Hide console' : 'Console',
                      style: const TextStyle(fontSize: 11)),
                ),
                TextButton(
                  onPressed: () =>
                      widget.controller.send(const ToolsCmd.clearLog()),
                  child: const Text('Clear',
                      style: TextStyle(fontSize: 11)),
                ),
              ],
            ),
          ),
          if (_open)
            Container(
              height: 150,
              width: double.infinity,
              color: t.nCanvas,
              child: SingleChildScrollView(
                controller: _scroll,
                padding: const EdgeInsets.fromLTRB(16, 10, 16, 12),
                child: SelectableText(
                  log.isEmpty ? 'Nothing has run yet.' : log,
                  style: TextStyle(
                      fontFamily: 'monospace',
                      fontSize: 11,
                      height: 1.5,
                      color: t.nInk2),
                ),
              ),
            ),
        ],
      ),
    );
  }

  String _lastLine(String log) {
    for (final line in log.split('\n').reversed) {
      if (line.trim().isNotEmpty) return line.trim();
    }
    return '';
  }
}
