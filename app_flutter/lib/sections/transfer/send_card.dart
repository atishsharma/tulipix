// Send — how do I give a file?
//
// One big target, a picker for who the next file is for, and the tray of what
// is on offer. A click target rather than a drop target on purpose: the Slint
// build has no OS-level file drop, and this one keeps the same shape so the two
// windows can be compared without one of them growing an affordance.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/transfer.dart';
import 'connection_card.dart' show PeerChip, deviceGlyph, peerChipH;
import 'transfer_controller.dart';
import 'transfer_widgets.dart';

class SendCard extends StatefulWidget {
  const SendCard({
    super.key,
    required this.controller,
    required this.state,
    required this.onSendTo,
  });

  final TransferController controller;
  final TransferState state;

  /// Push the tray to these machines, in the order they were ticked.
  final ValueChanged<List<String>> onSendTo;

  @override
  State<SendCard> createState() => _SendCardState();
}

class _SendCardState extends State<SendCard> {
  /// Which other tulipix machines the tray is aimed at.
  ///
  /// A `Set` because it is a set, and a `LinkedHashSet` — Dart's default —
  /// because the order the chips were ticked in is the order the lanes should
  /// come up in. It lives here rather than on the bridge because nothing
  /// outside this card reads it: a round trip to remember a tick would make
  /// the chip lag the click by a whole poll.
  final _picked = <String>{};

  @override
  Widget build(BuildContext context) {
    final state = widget.state;
    // Only a paired machine can be a destination — an unpaired one has no
    // token to present, so sending to it could only ever produce a failed
    // lane. Its chip belongs in the Connection card, where tapping it pairs.
    final dests = [
      for (final p in state.peers)
        if (p.paired) p
    ];
    // A machine that left the network between the tick and the click is not a
    // destination any more, and must not be counted as one on the button.
    final picked = [
      for (final b in _picked)
        if (dests.any((p) => p.base == b)) b,
    ];

    return TCard(
      title: 'Send',
      subtitle: 'Share files with the phone — never the library.',
      icon: Icons.upload,
      tint: Tint.send,
      // Only once the tray is aimed at something. With nothing ticked — which
      // is every machine that has never paired another tulipix — this card is
      // exactly the card it was before: the phone pulls, and there is nothing
      // to press.
      action: picked.isEmpty ? '' : 'Send to ${picked.length}',
      actionIcon: Icons.send,
      actionEnabled: state.files.isNotEmpty,
      onAction: () => widget.onSendTo(picked),
      children: [
        Expanded(
          // A fan-out and picking files never overlap — nobody chooses a file
          // mid-send — so the lanes take the drop zone's place rather than a
          // block of their own. The card is pinned to one height with its two
          // neighbours, and a result you have to scroll to is a result nobody
          // reads.
          //
          // A result, not a running total: `SendTray` awaits the whole fan-out
          // before it answers, so every lane arrives with its outcome already
          // decided. `waiting` and `sending` never reach here and no bar ever
          // moves — what this shows is which machines got the files. Making it
          // live needs `fanout::send` to report through a channel while it
          // runs, which is a backend change and not a widget one.
          //
          // The next pick clears them: the bridge empties `lanes` on AddFiles,
          // AddFolder and Clear, which is what brings the picker back.
          child: state.lanes.isEmpty
              ? _PickZone(
                  onFiles: widget.controller.addFiles,
                  onFolder: widget.controller.addFolder,
                )
              : ListView(
                  padding: EdgeInsets.zero,
                  children: [
                    for (final l in state.lanes) LaneRow(lane: l),
                  ],
                ),
        ),
        // Drawn only when there is a machine to send to. Amber, like the same
        // chips in the Connection card: both answer "which machine", and the
        // tick here is the sequel to the pairing there.
        if (dests.isNotEmpty) ...[
          const SizedBox(height: 10),
          Outline(
            label: 'SEND TO',
            note: picked.isEmpty ? '' : '${picked.length} ticked',
            // Unreachable: the block is not drawn when there is nothing in it.
            empty: '',
            isEmpty: false,
            // One row, and it scrolls past that. The drop zone above is the
            // only panel here that gives way, so every pixel this box takes is
            // one the drop zone loses.
            rows: 1,
            rowH: peerChipH + 6,
            tint: Tint.pair,
            children: [
              Wrap(
                spacing: 6,
                runSpacing: 6,
                children: [
                  for (final p in dests)
                    PeerChip(
                      peer: p,
                      selected: picked.contains(p.base),
                      // Paired already, so there is nothing to pair: the useful
                      // thing to do with this machine is aim the tray at it.
                      onTap: () => setState(() {
                        if (!_picked.remove(p.base)) _picked.add(p.base);
                      }),
                    ),
                ],
              ),
            ],
          ),
        ],
        const SizedBox(height: 12),
        // What is on offer, always visible. Two rows, then it scrolls inside
        // its own border rather than growing the card.
        Outline(
          label: 'FILES TO SEND',
          note: state.files.isEmpty ? '' : state.total,
          empty:
              'Nothing chosen yet. Until you pick something, a paired phone sees an empty list.',
          isEmpty: state.files.isEmpty,
          rowH: 62,
          tint: Tint.send,
          action: state.files.isEmpty ? '' : 'Clear all',
          onAction: widget.controller.clearTray,
          trailing: _TargetSelect(
            devices: state.devices,
            target: state.shareTarget,
            targetName: state.shareTargetName,
            onPick: widget.controller.setShareTarget,
          ),
          children: [
            for (final f in state.files)
              _TrayRow(
                file: f,
                onRemove: () => widget.controller.removeFile(f.id),
              ),
          ],
        ),
      ],
    );
  }
}

class _PickZone extends StatefulWidget {
  const _PickZone({required this.onFiles, required this.onFolder});

  final VoidCallback onFiles;
  final VoidCallback onFolder;

  @override
  State<_PickZone> createState() => _PickZoneState();
}

class _PickZoneState extends State<_PickZone> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onFiles,
        child: Container(
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(14),
            color: wash(Tint.send, _hover ? 0.10 : 0.05),
            border: Border.all(
              color: edge(Tint.send, _hover ? 1.0 : 0.42),
              width: 1.5,
            ),
          ),
          padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 16),
          child: PanelBody(
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Container(
                  width: 56,
                  height: 56,
                  decoration: const BoxDecoration(
                    shape: BoxShape.circle,
                    gradient: LinearGradient(
                      begin: Alignment.topLeft,
                      end: Alignment.bottomRight,
                      colors: [Tint.send, Color(0xFF818CF8)],
                    ),
                  ),
                  child:
                      const Icon(Icons.upload, size: 24, color: Colors.white),
                ),
                const SizedBox(height: 12),
                Text(
                  'Choose files to share',
                  style: TextStyle(
                    fontSize: 14,
                    fontWeight: FontWeight.w600,
                    color: t.text,
                  ),
                ),
                const SizedBox(height: 6),
                Text(
                  'They stay on this machine — the phone pulls them over the link.',
                  textAlign: TextAlign.center,
                  style: TextStyle(fontSize: 11.5, color: t.textDim),
                ),
                const SizedBox(height: 12),
                Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  alignment: WrapAlignment.center,
                  children: [
                    ActionBtn(
                      label: 'Choose files',
                      icon: Icons.folder_open,
                      tint: Tint.send,
                      onTap: widget.onFiles,
                    ),
                    ActionBtn(
                      label: 'Choose folder',
                      icon: Icons.folder,
                      // A shade down from its sibling: same job, second choice,
                      // and two identical fills side by side read as one wide
                      // button.
                      tint: const Color(0xFF4F46E5),
                      onTap: widget.onFolder,
                    ),
                  ],
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// Who the next file is for.
///
/// A dropdown, not a row of chips: ten devices may be paired and the title row
/// is one line of a three-column card. Files are stamped with whoever is picked
/// when they are added, so changing this aims the next pick without
/// re-addressing what is already in the tray.
class _TargetSelect extends StatelessWidget {
  const _TargetSelect({
    required this.devices,
    required this.target,
    required this.targetName,
    required this.onPick,
  });

  final List<TransferDevice> devices;
  final String target;
  final String targetName;
  final ValueChanged<String> onPick;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return PopupMenuButton<String>(
      tooltip: 'Who the next file is for',
      initialValue: target,
      onSelected: onPick,
      position: PopupMenuPosition.under,
      itemBuilder: (context) => [
        // Always first, and always present: it is the default and the way back
        // from having picked one phone.
        const PopupMenuItem(value: '', child: Text('Everyone')),
        for (final d in devices)
          PopupMenuItem(
            value: d.token,
            child: Row(
              children: [
                Icon(deviceGlyph(d.kind), size: 14, color: t.textDim),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(d.name, overflow: TextOverflow.ellipsis),
                ),
              ],
            ),
          ),
      ],
      child: Container(
        height: 26,
        constraints: const BoxConstraints(maxWidth: 140),
        padding: const EdgeInsets.symmetric(horizontal: 9),
        decoration: BoxDecoration(
          color: wash(Tint.send, 0.1),
          borderRadius: BorderRadius.circular(8),
          border: Border.all(color: edge(Tint.send, 0.42)),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Text(
              '→',
              style: TextStyle(
                fontSize: 11,
                fontWeight: FontWeight.w700,
                color: Tint.send,
              ),
            ),
            const SizedBox(width: 5),
            Flexible(
              child: Text(
                target.isEmpty ? 'Everyone' : targetName,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: 10.5,
                  fontWeight: FontWeight.w700,
                  color: t.text,
                ),
              ),
            ),
            const Icon(Icons.expand_more, size: 12, color: Tint.send),
          ],
        ),
      ),
    );
  }
}

class _TrayRow extends StatelessWidget {
  const _TrayRow({required this.file, required this.onRemove});

  final TransferFile file;
  final VoidCallback onRemove;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 58,
      padding: const EdgeInsets.symmetric(horizontal: 10),
      decoration: BoxDecoration(
        color: t.panel2,
        borderRadius: BorderRadius.circular(11),
      ),
      child: Row(
        children: [
          Container(
            width: 32,
            height: 32,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: wash(Tint.send, 0.15),
              borderRadius: BorderRadius.circular(9),
            ),
            child: Text(
              file.kind,
              style: const TextStyle(
                fontSize: 9,
                fontWeight: FontWeight.w800,
                color: Tint.send,
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
                  file.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 12.5, color: t.text),
                ),
                const SizedBox(height: 2),
                Row(
                  children: [
                    Flexible(
                      child: Text(
                        file.size,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 10.5, color: t.textDim),
                      ),
                    ),
                    // Only when it is for one device. "For everyone" on every
                    // row of the common case would be noise on every row.
                    if (file.to.isNotEmpty) ...[
                      const SizedBox(width: 6),
                      Flexible(
                        child: Text(
                          '→ ${file.to}',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                            fontSize: 10.5,
                            fontWeight: FontWeight.w700,
                            color: Tint.send,
                          ),
                        ),
                      ),
                    ],
                  ],
                ),
              ],
            ),
          ),
          const SizedBox(width: 8),
          RowBtn(label: 'Remove', onTap: onRemove),
        ],
      ),
    );
  }
}

/// How one destination of a fan-out ended.
///
/// Indigo, because a lane is giving. A failed lane keeps its row and turns red
/// rather than vanishing: the useful thing to know about a fan-out is which two
/// of the three machines actually got the files, and why the third did not.
///
/// The bar is an outcome rather than a running total — the bridge answers only
/// once every lane has finished, so it paints full or empty and never anything
/// between. `waiting` is drawn indeterminate for the day the backend streams
/// its lanes and the intermediate states start arriving.
class LaneRow extends StatelessWidget {
  const LaneRow({super.key, required this.lane});

  final TransferLane lane;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final failed = lane.state == 'failed';
    final tint = failed
        ? Tokens.error
        : lane.state == 'done'
            ? Tokens.ok
            : Tint.send;
    return Padding(
      padding: const EdgeInsets.only(bottom: 9),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          Row(
            children: [
              Icon(
                failed ? Icons.error_outline : Icons.laptop,
                size: 14,
                color: tint,
              ),
              const SizedBox(width: 7),
              Expanded(
                child: Text(
                  lane.name,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w700,
                    color: t.text,
                  ),
                ),
              ),
              // What the lane moved in total, the reason when it failed, and
              // nothing at all once it landed — a lane that worked has nothing
              // left to say. Never a running total: see the note on the
              // `Expanded` above.
              if (lane.detail.isNotEmpty) ...[
                const SizedBox(width: 8),
                Flexible(
                  child: Text(
                    lane.detail,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    textAlign: TextAlign.right,
                    style: TextStyle(
                      fontSize: 10.5,
                      color: failed ? Tokens.error : t.textDim,
                    ),
                  ),
                ),
              ],
            ],
          ),
          const SizedBox(height: 5),
          ClipRRect(
            borderRadius: BorderRadius.circular(2),
            child: LinearProgressIndicator(
              // Null is indeterminate: a lane that has not started is not a
              // send that has moved no bytes, and they should not look alike.
              value: lane.state == 'waiting' ? null : lane.pct,
              minHeight: 4,
              backgroundColor: t.outline,
              valueColor: AlwaysStoppedAnimation(tint),
            ),
          ),
        ],
      ),
    );
  }
}
