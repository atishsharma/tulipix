// Recent Transfers — sent and received, full width.
//
// Sorting runs in SQL over the whole ledger, not over the ten rows on screen:
// sorting a page in place would only shuffle that page and leave the rest of
// the table alone. The header and the rows repeat their column widths by hand,
// which is what the Slint page does for the same reason — there is no grid that
// spans a list.

import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/transfer.dart';
import 'transfer_controller.dart';
import 'transfer_widgets.dart';

/// Fixed column widths, shared by the header and every row. Name takes what is
/// left. Size sits beside the filename it describes rather than three columns
/// away.
const _wNum = 30.0;
const _wSize = 110.0;
const _wType = 150.0;
const _wStatus = 160.0;
const _wPeer = 190.0;
const _wTime = 140.0;
const _wOpen = 34.0;

/// Below this the columns stop fitting and the table scrolls sideways instead
/// of squeezing the filename to nothing.
const _minTableWidth = 980.0;

/// `ledger::PAGE_SIZE`. Only the row number needs it — the bridge does the
/// actual paging — and the Slint page carries the same literal for the same
/// reason.
const _pageSize = 10;

const _columns = <String>[
  'Name',
  'Size',
  'Type',
  'Status',
  'From / To',
  'Time',
];

class TransfersTable extends StatelessWidget {
  const TransfersTable({
    super.key,
    required this.controller,
    required this.state,
    required this.onClearHistory,
  });

  final TransferController controller;
  final TransferState state;
  final VoidCallback onClearHistory;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return TCard(
      title: 'Recent transfers',
      subtitle:
          'Sent and received. Click a column to sort. A greyed row is a file that has since moved.',
      icon: Icons.schedule,
      tint: Tint.hist,
      // Nothing to clear when the table is empty, so the button says so rather
      // than sitting there live and doing nothing.
      action: 'Clear history',
      actionIcon: Icons.delete_outline,
      actionEnabled: state.rows.isNotEmpty,
      onAction: onClearHistory,
      children: [
        // The whole table scrolls sideways as one piece rather than the header
        // and the rows drifting apart on a narrow window. The width is
        // resolved here rather than left to the scroll view: a horizontal
        // scroller hands its child unbounded width, and the Name column is the
        // one that takes what is left over.
        LayoutBuilder(
          builder: (context, box) => SingleChildScrollView(
            scrollDirection: Axis.horizontal,
            child: SizedBox(
              width: math.max(_minTableWidth, box.maxWidth),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                mainAxisSize: MainAxisSize.min,
                children: [
                  // Hidden while the ledger is empty — six column pills over
                  // nothing read as a broken table rather than a fresh one.
                  if (state.rows.isNotEmpty)
                    _Head(controller: controller, state: state),
                  if (state.rows.isEmpty)
                    SizedBox(
                      height: 64,
                      child: Center(
                        child: Text(
                          'Nothing yet. Sent and received files are both recorded here.',
                          style: TextStyle(fontSize: 12, color: t.textDim),
                        ),
                      ),
                    ),
                  for (var i = 0; i < state.rows.length; i++)
                    _Row(
                      row: state.rows[i],
                      // Counted through the whole ledger and not restarted per
                      // page, so the number says which transfer this is rather
                      // than where it happens to sit on screen. The 10 is
                      // `ledger::PAGE_SIZE`, hardcoded here exactly as
                      // ui/page_transfer.slint hardcodes it: the bridge paginates
                      // with the real constant, and this is a label.
                      number: state.page * _pageSize + i + 1,
                      onOpen: () => controller.openRow(state.rows[i].id),
                      onRetry: () => controller.retryRow(state.rows[i].id),
                    ),
                ],
              ),
            ),
          ),
        ),
        // Hidden outright on a single page — a Previous and Next that can never
        // do anything is just noise.
        if (state.pages > 1) ...[
          const SizedBox(height: 12),
          Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              _PageBtn(
                label: 'Previous',
                icon: Icons.chevron_left,
                enabled: state.page > 0,
                onTap: () => controller.setPage(state.page - 1),
              ),
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: 12),
                child: Text(
                  'Page ${state.page + 1} of ${state.pages}',
                  style: TextStyle(
                    fontSize: 11.5,
                    fontWeight: FontWeight.w600,
                    color: t.textDim,
                  ),
                ),
              ),
              _PageBtn(
                label: 'Next',
                icon: Icons.chevron_right,
                trailing: true,
                enabled: state.page + 1 < state.pages,
                onTap: () => controller.setPage(state.page + 1),
              ),
            ],
          ),
        ],
      ],
    );
  }
}

class _Head extends StatelessWidget {
  const _Head({required this.controller, required this.state});

  final TransferController controller;
  final TransferState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    const widths = <double?>[null, _wSize, _wType, _wStatus, _wPeer, _wTime];
    return Container(
      height: 38,
      padding: const EdgeInsets.symmetric(horizontal: 12),
      decoration: BoxDecoration(
        color: t.panel2,
        borderRadius: BorderRadius.circular(9),
      ),
      child: Row(
        children: [
          // Blank head over the row numbers: the column is position, and
          // position is not something to sort by.
          const SizedBox(width: _wNum),
          for (var i = 0; i < _columns.length; i++) ...[
            const SizedBox(width: 12),
            if (widths[i] == null)
              Expanded(
                child: Align(
                  alignment: Alignment.centerLeft,
                  child: SortHead(
                    label: _columns[i],
                    index: i,
                    active: state.sort,
                    desc: state.sortDesc,
                    tint: Tint.columns[i],
                    onTap: () => controller.sortBy(i),
                  ),
                ),
              )
            else
              SortHead(
                label: _columns[i],
                index: i,
                active: state.sort,
                desc: state.sortDesc,
                tint: Tint.columns[i],
                width: widths[i],
                onTap: () => controller.sortBy(i),
              ),
          ],
          const SizedBox(width: 12),
          const SizedBox(width: _wOpen),
        ],
      ),
    );
  }
}

class _Row extends StatefulWidget {
  const _Row({
    required this.row,
    required this.number,
    required this.onOpen,
    required this.onRetry,
  });

  final TransferLedgerRow row;
  final int number;
  final VoidCallback onOpen;
  final VoidCallback onRetry;

  @override
  State<_Row> createState() => _RowState();
}

class _RowState extends State<_Row> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final r = widget.row;
    final incoming = r.direction == 'in';
    final way = incoming ? Tint.recv : Tint.accent;
    final statusColour = r.status == 'ok'
        ? Tokens.ok
        : r.status == 'sending'
            ? Tint.accent
            : Tokens.error;
    final openable = r.path.isNotEmpty;
    // Only a send can be re-offered: an upload is the phone's to repeat, and
    // only while the file is still where it was read from.
    final retryable =
        r.status == 'failed' && !incoming && !r.missing && r.path.isNotEmpty;

    return MouseRegion(
      cursor: openable ? SystemMouseCursors.click : SystemMouseCursors.basic,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: openable ? widget.onOpen : null,
        child: Opacity(
          // A row whose file has been moved or deleted is greyed, not hidden:
          // knowing something arrived and then went is useful.
          opacity: r.missing ? 0.55 : 1.0,
          child: Container(
            height: 64,
            padding: const EdgeInsets.symmetric(horizontal: 12),
            decoration: BoxDecoration(
              color: _hover ? t.panel2 : Colors.transparent,
              borderRadius: BorderRadius.circular(10),
            ),
            child: Row(
              children: [
                SizedBox(
                  width: _wNum,
                  child: Text(
                    '${widget.number}',
                    textAlign: TextAlign.center,
                    style: TextStyle(
                      fontSize: 11.5,
                      fontWeight: FontWeight.w700,
                      color: t.textDim,
                    ),
                  ),
                ),
                const SizedBox(width: 12),

                // Name — icon tile, filename, what kind of file.
                Expanded(
                  child: Row(
                    children: [
                      Container(
                        width: 34,
                        height: 34,
                        alignment: Alignment.center,
                        decoration: BoxDecoration(
                          color: wash(way, 0.14),
                          borderRadius: BorderRadius.circular(10),
                        ),
                        child: Text(
                          r.kind,
                          style: TextStyle(
                            fontSize: 9,
                            fontWeight: FontWeight.w800,
                            color: way,
                          ),
                        ),
                      ),
                      const SizedBox(width: 11),
                      Expanded(
                        child: Column(
                          mainAxisSize: MainAxisSize.min,
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              r.name,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                fontSize: 12.5,
                                fontWeight: FontWeight.w600,
                                color: t.text,
                              ),
                            ),
                            const SizedBox(height: 2),
                            Text(
                              r.kindlabel,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                fontSize: 10.5,
                                color: t.textDim,
                              ),
                            ),
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 12),

                SizedBox(
                  width: _wSize,
                  child: Text(
                    r.size,
                    textAlign: TextAlign.center,
                    style: TextStyle(fontSize: 12, color: t.text),
                  ),
                ),
                const SizedBox(width: 12),

                // Type — which way it went.
                SizedBox(
                  width: _wType,
                  child: Row(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      Icon(
                        incoming ? Icons.arrow_downward : Icons.arrow_upward,
                        size: 15,
                        color: way,
                      ),
                      const SizedBox(width: 7),
                      Text(
                        incoming ? 'Received' : 'Sent',
                        style: TextStyle(
                          fontSize: 12,
                          fontWeight: FontWeight.w600,
                          color: way,
                        ),
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 12),

                // Status. A file still on the wire says how far along it is and
                // how fast, over its own bar — the row is written on the first
                // chunk, so without this a 4 GB download claimed to be done at
                // once.
                SizedBox(
                  width: _wStatus,
                  child: Row(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      Icon(
                        r.status == 'ok'
                            ? Icons.check
                            : r.status == 'sending'
                                ? Icons.arrow_upward
                                : Icons.warning_amber_rounded,
                        size: 15,
                        color: statusColour,
                      ),
                      const SizedBox(width: 7),
                      Flexible(
                        child: Column(
                          mainAxisSize: MainAxisSize.min,
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              r.status == 'ok'
                                  ? 'Completed'
                                  : r.status == 'sending'
                                      ? (r.progress.isNotEmpty
                                          ? r.progress
                                          : 'Sending…')
                                      : r.status,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                fontSize: 12,
                                fontWeight: FontWeight.w600,
                                color: statusColour,
                              ),
                            ),
                            if (r.status == 'sending') ...[
                              const SizedBox(height: 3),
                              ClipRRect(
                                borderRadius: BorderRadius.circular(1.5),
                                child: LinearProgressIndicator(
                                  value: r.pct,
                                  minHeight: 3,
                                  backgroundColor: t.outline,
                                  valueColor: const AlwaysStoppedAnimation(
                                    Tint.accent,
                                  ),
                                ),
                              ),
                            ],
                          ],
                        ),
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 12),

                SizedBox(
                  width: _wPeer,
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      // The caption is dropped on a row that carries a retry
                      // button: the row is 64px, and "To" above a name above a
                      // button is one line more than that holds. The arrow in
                      // the Type column already says which way it went.
                      if (!retryable)
                        Text(
                          incoming ? 'From' : 'To',
                          style: TextStyle(fontSize: 10, color: t.textDim),
                        ),
                      Text(
                        r.peer,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          fontSize: 11.5,
                          fontWeight: FontWeight.w600,
                          color: t.text,
                        ),
                      ),
                      // Offers the file again, and drops the failed row so the
                      // table does not keep a record that has been superseded.
                      // Only for a send: an upload is the phone's to repeat,
                      // and only while the file is still where it was read
                      // from.
                      if (retryable)
                        TextButton.icon(
                          onPressed: widget.onRetry,
                          icon: const Icon(Icons.refresh, size: 11),
                          label: const Text(
                            'Send again',
                            style: TextStyle(
                              fontSize: 10,
                              fontWeight: FontWeight.w600,
                            ),
                          ),
                          style: TextButton.styleFrom(
                            minimumSize: const Size(0, 18),
                            padding: const EdgeInsets.symmetric(horizontal: 6),
                            visualDensity: VisualDensity.compact,
                            tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                            foregroundColor: Tint.accent,
                          ),
                        ),
                    ],
                  ),
                ),
                const SizedBox(width: 12),

                SizedBox(
                  width: _wTime,
                  child: Text(
                    r.stamp,
                    textAlign: TextAlign.center,
                    style: TextStyle(fontSize: 11.5, color: t.textDim),
                  ),
                ),
                const SizedBox(width: 12),

                SizedBox(
                  width: _wOpen,
                  child: Opacity(
                    opacity: openable ? 1.0 : 0.25,
                    child: IconButton(
                      tooltip: openable ? 'Show in the file manager' : null,
                      onPressed: openable ? widget.onOpen : null,
                      padding: EdgeInsets.zero,
                      constraints: const BoxConstraints.tightFor(
                        width: _wOpen,
                        height: _wOpen,
                      ),
                      icon: Icon(
                        Icons.folder_open,
                        size: 15,
                        color: t.textDim,
                      ),
                    ),
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

class _PageBtn extends StatelessWidget {
  const _PageBtn({
    required this.label,
    required this.icon,
    required this.enabled,
    required this.onTap,
    this.trailing = false,
  });

  final String label;
  final IconData icon;
  final bool enabled;
  final VoidCallback onTap;

  /// Chevron after the label instead of before it, for Next.
  final bool trailing;

  @override
  Widget build(BuildContext context) {
    final child = Text(
      label,
      style: const TextStyle(fontSize: 11.5, fontWeight: FontWeight.w600),
    );
    final glyph = Icon(icon, size: 15);
    return OutlinedButton(
      onPressed: enabled ? onTap : null,
      style: OutlinedButton.styleFrom(
        foregroundColor: Tint.hist,
        side: BorderSide(color: edge(Tint.hist, 0.4)),
        minimumSize: const Size(0, 32),
        padding: const EdgeInsets.symmetric(horizontal: 12),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: trailing
            ? [child, const SizedBox(width: 5), glyph]
            : [glyph, const SizedBox(width: 5), child],
      ),
    );
  }
}
