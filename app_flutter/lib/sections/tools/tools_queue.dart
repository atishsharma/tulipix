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
                onPressed: () => controller.setQueue(false),
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
      padding: const EdgeInsets.fromLTRB(16, 9, 8, 9),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // One line: what it is, how it is going, and what you can do about
          // it. The message used to sit on a line of its own under the name,
          // which made a list of six finished jobs twelve rows tall for two
          // words of "Wrote 4 files."
          Row(
            children: [
              Icon(look.icon, size: 15, color: look.colour),
              const SizedBox(width: 8),
              Expanded(
                child: Text.rich(
                  TextSpan(children: [
                    TextSpan(
                      text: job.label,
                      style: TextStyle(
                        fontSize: 12.5,
                        fontWeight: FontWeight.w600,
                        color: t.nInk,
                      ),
                    ),
                    if (job.message.isNotEmpty)
                      TextSpan(
                        text: '  ·  ${job.message}',
                        style: TextStyle(
                          fontSize: 11.5,
                          color: job.state == 'failed' ? Tokens.error : t.nInk2,
                        ),
                      ),
                  ]),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
              ),
              if (running) ...[
                const SizedBox(width: 8),
                Text('${(progress * 100).round()}%',
                    style: TextStyle(
                      fontSize: 11,
                      color: look.colour,
                      fontFeatures: const [FontFeature.tabularFigures()],
                    )),
              ],
              const SizedBox(width: 6),
              // Last in the row, after the Expanded, so the cluster sits on
              // the right edge whatever length the name in front of it is.
              _JobActions(
                  controller: controller, job: job, hasReport: hasReport),
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
        ],
      ),
    );
  }
}

/// Extensions the app's own player will take. A collage is opened in a file
/// manager; a trimmed video is watched here.
const Set<String> _playable = {
  'mp4',
  'mkv',
  'webm',
  'mov',
  'avi',
  'm4v',
  'mpg',
  'mpeg',
  'ts',
  'flv',
  'mp3',
  'm4a',
  'aac',
  'opus',
  'ogg',
  'flac',
  'wav',
  'aiff',
  'wma',
};

/// What a job lets you do, as buttons.
///
/// This was a three-dot menu, which is two clicks and a read to find out that
/// a finished job offers one thing. Every action a job has is a state away
/// from every other — a queued job cannot be retried, a finished one cannot be
/// paused — so the row never carries more than four of these at once.
class _JobActions extends StatelessWidget {
  const _JobActions({
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
    final out = job.output;
    final ext = out.contains('.') ? out.split('.').last.toLowerCase() : '';
    void act(String a) =>
        controller.send(ToolsCmd.queueAction(id: job.id, action: a));

    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        if (hasReport)
          _Act(
            icon: Icons.article_outlined,
            tip: 'Show the report',
            onTap: () => controller.send(ToolsCmd.showResult(id: job.id)),
          ),
        if (out.isNotEmpty) ...[
          _Act(
            icon: Icons.folder_open,
            tip: 'Show where it went',
            onTap: () => controller.send(ToolsCmd.openDownload(path: out)),
          ),
          if (_playable.contains(ext))
            _Act(
              icon: Icons.play_circle_outline,
              tip: 'Play it here',
              onTap: () => controller.send(ToolsCmd.playDownload(path: out)),
            ),
        ],
        if (active)
          _Act(icon: Icons.pause, tip: 'Pause', onTap: () => act('pause')),
        if (job.state == 'paused')
          _Act(
              icon: Icons.play_arrow,
              tip: 'Resume',
              onTap: () => act('resume')),
        if (active)
          _Act(
              icon: Icons.stop_circle_outlined,
              tip: 'Cancel',
              onTap: () => act('cancel')),
        if (job.state == 'failed' || job.state == 'cancelled')
          _Act(
              icon: Icons.refresh,
              tip: 'Run it again',
              onTap: () => act('retry')),
        if (job.state == 'queued') ...[
          _Act(
              icon: Icons.keyboard_arrow_up,
              tip: 'Sooner',
              onTap: () => act('up')),
          _Act(
              icon: Icons.keyboard_arrow_down,
              tip: 'Later',
              onTap: () => act('down')),
        ],
        _Act(
            icon: Icons.close,
            tip: 'Take it off the list',
            onTap: () => act('remove')),
      ],
    );
  }
}

/// One of them. Small, quiet, and only coloured under the pointer — a row of
/// eight loud icons is worse than the menu it replaced.
class _Act extends StatefulWidget {
  const _Act({required this.icon, required this.tip, required this.onTap});

  final IconData icon;
  final String tip;
  final VoidCallback onTap;

  @override
  State<_Act> createState() => _ActState();
}

class _ActState extends State<_Act> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Tooltip(
      message: widget.tip,
      waitDuration: const Duration(milliseconds: 450),
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: Container(
            margin: const EdgeInsets.only(right: 2),
            padding: const EdgeInsets.all(4),
            decoration: BoxDecoration(
              color: _hover ? Tokens.secTools.withValues(alpha: 0.14) : null,
              borderRadius: BorderRadius.circular(6),
            ),
            child: Icon(widget.icon,
                size: 16, color: _hover ? Tokens.secTools : t.nInk2),
          ),
        ),
      ),
    );
  }
}

/// How tall either panel stands when open. Enough for six or seven rows,
/// which is as much of either as anyone reads at a glance.
const double _dockHeight = 224;

/// The bottom of the section: a bar split in half, console on the left and
/// queue on the right, each opening on its own.
///
/// The queue used to be a drawer over the right-hand side, which meant it
/// covered the preview — the one thing worth looking at while a job runs. Down
/// here both can be open at once, and neither hides the work.
class BottomDock extends StatefulWidget {
  const BottomDock({super.key, required this.controller, required this.state});

  final ToolsController controller;
  final ToolsState state;

  @override
  State<BottomDock> createState() => _BottomDockState();
}

class _BottomDockState extends State<BottomDock> {
  final ScrollController _scroll = ScrollController();

  @override
  void didUpdateWidget(BottomDock old) {
    super.didUpdateWidget(old);
    // Follow the tail: a console you have to scroll by hand shows you the
    // beginning of the problem, not the end of it.
    if (widget.controller.consoleOpen && _scroll.hasClients) {
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
    final c = widget.controller;
    final st = widget.state;
    final running = st.jobs.any((j) => j.state == 'running');
    final queued = st.jobs.where((j) => j.state == 'queued').length;
    // Only the half you opened is painted. Filling the whole width and
    // leaving one side blank made a panel that reads as full width with a
    // hole in it, which is exactly what it looked like.
    Widget half(bool open, Widget panel) => Expanded(
          child: open
              ? Container(
                  decoration: BoxDecoration(
                    color: t.nCard,
                    border: Border(top: BorderSide(color: t.nHair)),
                  ),
                  child: panel,
                )
              : const SizedBox.shrink(),
        );

    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        if (c.consoleOpen || c.queueOpen)
          SizedBox(
            height: _dockHeight,
            child: Row(
              children: [
                // Each keeps to its own half whether or not the other is
                // open, so opening the second one never shoves the first
                // across the screen mid-read.
                half(c.consoleOpen, _ConsolePanel(state: st, scroll: _scroll)),
                if (c.consoleOpen && c.queueOpen)
                  Container(width: 1, color: t.nHair),
                half(c.queueOpen, QueuePanel(controller: c, state: st)),
              ],
            ),
          ),
        Container(
          decoration: BoxDecoration(
            color: t.nCard,
            border: Border(top: BorderSide(color: t.nHair)),
          ),
          child: Row(
            children: [
              Expanded(
                child: _DockTab(
                  icon: Icons.terminal,
                  label: 'Console',
                  detail: _lastLine(st.log),
                  open: c.consoleOpen,
                  onTap: () => c.setConsole(!c.consoleOpen),
                  trailing: TextButton(
                    onPressed: () => c.send(const ToolsCmd.clearLog()),
                    child: const Text('Clear', style: TextStyle(fontSize: 11)),
                  ),
                ),
              ),
              Container(width: 1, height: 26, color: t.nHair),
              Expanded(
                child: _DockTab(
                  icon: Icons.list_alt,
                  label: 'Queue',
                  detail: st.queueStatus,
                  open: c.queueOpen,
                  busy: running,
                  badge: queued,
                  onTap: () => c.setQueue(!c.queueOpen),
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }

  String _lastLine(String log) {
    for (final line in log.split('\n').reversed) {
      if (line.trim().isNotEmpty) return line.trim();
    }
    return '';
  }
}

/// One half of the bar. Clicking anywhere on it opens or closes its panel.
class _DockTab extends StatelessWidget {
  const _DockTab({
    required this.icon,
    required this.label,
    required this.detail,
    required this.open,
    required this.onTap,
    this.trailing,
    this.busy = false,
    this.badge = 0,
  });

  final IconData icon;
  final String label;
  final String detail;
  final bool open;
  final VoidCallback onTap;
  final Widget? trailing;
  final bool busy;
  final int badge;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onTap,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(14, 6, 8, 6),
        child: Row(
          children: [
            if (busy)
              const SizedBox(
                width: 11,
                height: 11,
                child: CircularProgressIndicator(
                    strokeWidth: 2, color: Tokens.secTools),
              )
            else
              Icon(icon, size: 14, color: open ? Tokens.secTools : t.nInk3),
            const SizedBox(width: 8),
            Text(
              label,
              style: TextStyle(
                  fontSize: 11.5,
                  fontWeight: FontWeight.w700,
                  color: open ? Tokens.secTools : t.nInk2),
            ),
            if (badge > 0) ...[
              const SizedBox(width: 6),
              Container(
                padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 1),
                decoration: BoxDecoration(
                  color: Tokens.secTools.withValues(alpha: 0.16),
                  borderRadius: BorderRadius.circular(999),
                ),
                child: Text('$badge',
                    style: const TextStyle(
                        fontSize: 10,
                        fontWeight: FontWeight.w700,
                        color: Tokens.secTools)),
              ),
            ],
            if (detail.isNotEmpty) ...[
              const SizedBox(width: 10),
              Expanded(
                child: Text(
                  detail,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                      fontFamily: 'monospace', fontSize: 11, color: t.nInk3),
                ),
              ),
            ] else
              const Spacer(),
            if (trailing != null) trailing!,
            Icon(open ? Icons.expand_more : Icons.expand_less,
                size: 16, color: t.nInk3),
          ],
        ),
      ),
    );
  }
}

class _ConsolePanel extends StatelessWidget {
  const _ConsolePanel({required this.state, required this.scroll});

  final ToolsState state;
  final ScrollController scroll;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ColoredBox(
      color: t.nCanvas,
      child: SingleChildScrollView(
        controller: scroll,
        padding: const EdgeInsets.fromLTRB(16, 10, 16, 12),
        child: SelectableText(
          state.log.isEmpty ? 'Nothing has run yet.' : state.log,
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
