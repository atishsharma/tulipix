// Send — how do I give a file?
//
// One big target, a picker for who the next file is for, and the tray of what
// is on offer. A click target rather than a drop target on purpose: the Slint
// build has no OS-level file drop, and this one keeps the same shape so the two
// windows can be compared without one of them growing an affordance.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/transfer.dart';
import 'connection_card.dart' show deviceGlyph;
import 'transfer_controller.dart';
import 'transfer_widgets.dart';

class SendCard extends StatelessWidget {
  const SendCard({super.key, required this.controller, required this.state});

  final TransferController controller;
  final TransferState state;

  @override
  Widget build(BuildContext context) {
    return TCard(
      title: 'Send',
      subtitle: 'Share files with the phone — never the library.',
      icon: Icons.upload,
      tint: Tint.send,
      children: [
        Expanded(
          child: _PickZone(
            onFiles: controller.addFiles,
            onFolder: controller.addFolder,
          ),
        ),
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
          onAction: controller.clearTray,
          trailing: _TargetSelect(
            devices: state.devices,
            target: state.shareTarget,
            targetName: state.shareTargetName,
            onPick: controller.setShareTarget,
          ),
          children: [
            for (final f in state.files)
              _TrayRow(
                file: f,
                onRemove: () => controller.removeFile(f.id),
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
