// Receive — where do files land?
//
// The inbox and the two buttons that change it in one panel: they were one
// decision drawn as two blocks. A red border rather than a red field when the
// folder cannot be written to, because the buttons are the fix and so they
// belong inside the warning.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/transfer.dart';
import 'transfer_controller.dart';
import 'transfer_widgets.dart';

class ReceiveCard extends StatelessWidget {
  const ReceiveCard({super.key, required this.controller, required this.state});

  final TransferController controller;
  final TransferState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return TCard(
      title: 'Receive',
      subtitle: 'Everything the phone sends lands here, flat.',
      icon: Icons.download,
      tint: Tint.recv,
      children: [
        Expanded(
          child: Container(
            padding: const EdgeInsets.all(14),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(14),
              color: wash(Tint.recv, 0.05),
              border: Border.all(
                color: state.inboxOk ? edge(Tint.recv, 0.42) : Tokens.error,
                width: 1.5,
              ),
            ),
            child: PanelBody(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Text(
                    'INBOX FOLDER',
                    style: TextStyle(
                      fontSize: 10,
                      letterSpacing: 1.3,
                      fontWeight: FontWeight.w700,
                      color: t.textDim,
                    ),
                  ),
                  const SizedBox(height: 10),
                  Container(
                    height: 44,
                    padding: const EdgeInsets.only(left: 12, right: 6),
                    decoration: BoxDecoration(
                      color: t.panel2,
                      borderRadius: BorderRadius.circular(10),
                      border: Border.all(
                        color: state.inboxOk ? t.outline : Tokens.error,
                      ),
                    ),
                    child: Row(
                      children: [
                        Expanded(
                          child: Text(
                            state.inbox,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 12, color: t.text),
                          ),
                        ),
                        IconButton(
                          tooltip: 'Open the inbox',
                          onPressed: controller.openInbox,
                          padding: EdgeInsets.zero,
                          constraints: const BoxConstraints.tightFor(
                            width: 32,
                            height: 32,
                          ),
                          icon: const Icon(
                            Icons.folder_open,
                            size: 16,
                            color: Tint.recv,
                          ),
                        ),
                      ],
                    ),
                  ),
                  const SizedBox(height: 12),
                  Wrap(
                    spacing: 8,
                    runSpacing: 8,
                    children: [
                      ActionBtn(
                        label: 'Change location',
                        icon: Icons.place_outlined,
                        tint: Tint.recv,
                        onTap: controller.pickInbox,
                      ),
                      ActionBtn(
                        label: 'Open folder',
                        icon: Icons.folder,
                        tint: const Color(0xFF11803A),
                        onTap: controller.openInbox,
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ),
        ),
        const SizedBox(height: 12),
        if (state.inboxOk)
          NoteStrip(
            title: state.uploads.isEmpty ? 'Ready to receive' : 'Receiving',
            body: state.uploads.isEmpty
                ? 'Files the phone sends will appear here.'
                : 'One at a time — parallel writes to one disk are slower.',
            icon: Icons.check_circle_outline,
            tint: Tint.recv,
          )
        else
          const NoteStrip(
            title: 'That folder cannot be written to',
            body: 'Uploads are refused until it can. Downloads still work.',
            icon: Icons.warning_amber_rounded,
            tint: Tokens.error,
          ),
        const SizedBox(height: 12),
        // The same box as Send's, for the same reason: the arrivals list
        // appearing out of nowhere shifted everything under it at the exact
        // moment attention was on it.
        Outline(
          label: 'ARRIVING',
          note: state.uploads.isEmpty
              ? ''
              : '${state.uploads.length} '
                  '${state.uploads.length == 1 ? "file" : "files"}',
          empty:
              'Nothing arriving. What the phone sends lands in the folder above.',
          isEmpty: state.uploads.isEmpty,
          rowH: 62,
          tint: Tint.recv,
          children: [
            for (final u in state.uploads)
              _UploadRow(
                up: u,
                onDismiss: () => controller.dismissUpload(u.id),
              ),
          ],
        ),
      ],
    );
  }
}

/// One upload in flight, or just finished.
///
/// A success disappears on its own a few seconds after it lands — the permanent
/// record is the Recent Transfers row below. A failure has no timer: it is the
/// one thing here worth reading late, so it waits to be dismissed.
class _UploadRow extends StatelessWidget {
  const _UploadRow({required this.up, required this.onDismiss});

  final TransferUpload up;
  final VoidCallback onDismiss;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final failed = up.state == 'failed';
    final tint = up.state == 'done'
        ? Tokens.ok
        : failed
            ? Tokens.error
            : Tint.accent;
    return Container(
      padding: const EdgeInsets.all(10),
      decoration: BoxDecoration(
        color: failed ? wash(Tokens.error, 0.07) : t.panel2,
        borderRadius: BorderRadius.circular(11),
        border: failed ? Border.all(color: edge(Tokens.error, 0.28)) : null,
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  up.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 12.5, color: t.text),
                ),
              ),
              const SizedBox(width: 8),
              Text(
                up.state == 'done'
                    ? 'Done'
                    : failed
                        ? 'Failed'
                        : '${(up.pct * 100).round()}%',
                style: TextStyle(
                  fontSize: 11.5,
                  fontWeight: FontWeight.w700,
                  color: tint,
                ),
              ),
            ],
          ),
          const SizedBox(height: 6),
          ClipRRect(
            borderRadius: BorderRadius.circular(2),
            child: LinearProgressIndicator(
              value: up.state == 'done' ? 1.0 : up.pct,
              minHeight: 4,
              backgroundColor: t.outline,
              valueColor: AlwaysStoppedAnimation(tint),
            ),
          ),
          const SizedBox(height: 6),
          if (!failed)
            // A bare percentage says nothing about whether a 2 GB file is going
            // to take one minute or twenty.
            Row(
              children: [
                Expanded(
                  child: Text(
                    up.moved.isNotEmpty ? up.moved : up.size,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 10.5, color: t.textDim),
                  ),
                ),
                if (up.rate.isNotEmpty)
                  Text(
                    up.rate,
                    style: TextStyle(
                      fontSize: 10.5,
                      fontWeight: FontWeight.w700,
                      color: tint,
                    ),
                  ),
              ],
            )
          else
            // The phone is the sender, so nothing on this end can re-pull the
            // file. Saying where the retry actually lives beats a button that
            // cannot work.
            Row(
              children: [
                Expanded(
                  child: Text(
                    'Send it again from the phone',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 10.5, color: t.textDim),
                  ),
                ),
                TextButton(
                  onPressed: onDismiss,
                  style: TextButton.styleFrom(
                    minimumSize: const Size(0, 24),
                    padding: const EdgeInsets.symmetric(horizontal: 9),
                    foregroundColor: Tokens.error,
                  ),
                  child: const Text(
                    'Dismiss',
                    style: TextStyle(
                      fontSize: 10.5,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
              ],
            ),
        ],
      ),
    );
  }
}
