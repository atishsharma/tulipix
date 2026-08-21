// Connection — how do I get in?
//
// The three ways in (code, address, PIN) sit inside one bordered panel: apart
// they read as three unrelated fields, together they read as the one thing the
// card is for. Under it, the network picker when more than one link qualifies,
// and the paired devices as a grid of round buttons.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/transfer.dart';
import 'qr_view.dart';
import 'transfer_controller.dart';
import 'transfer_widgets.dart';

/// Height of one paired-device cell: the circle, the gap, and one line of
/// caption. `Outline`'s `rowH` has to leave room for two of these.
const _chipExtent = 66.0;

/// The glyph a device type gets. Anything unrecognised gets the desktop icon
/// rather than no icon: an unknown browser is still a device.
IconData deviceGlyph(String kind) => switch (kind) {
      'Android' || 'iPhone' => Icons.smartphone,
      'iPad' => Icons.tablet_mac,
      'Mac' => Icons.laptop_mac,
      _ => Icons.desktop_windows_outlined,
    };

class ConnectionCard extends StatelessWidget {
  const ConnectionCard({
    super.key,
    required this.controller,
    required this.state,
    required this.qrInverted,
    required this.onQrInvert,
    required this.onDeviceTap,
  });

  final TransferController controller;
  final TransferState state;
  final bool qrInverted;
  final ValueChanged<bool> onQrInvert;
  final ValueChanged<TransferDevice> onDeviceTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final code = controller.qr;
    return TCard(
      title: 'Connection',
      subtitle: 'Scan the code, or type the address and PIN.',
      icon: Icons.link,
      tint: Tint.accent,
      children: [
        // This is the card's stretchy panel, as it is in the Slint page: the
        // device grid below is a fixed two rows, so whatever height is left
        // over lands here rather than as a gap under the grid.
        Expanded(
          child: Container(
            padding: const EdgeInsets.all(14),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(14),
              color: wash(Tint.accent, 0.05),
              border: Border.all(color: edge(Tint.accent, 0.42), width: 1.5),
            ),
            child: PanelBody(
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.center,
                children: [
                  // The QR carries a single-use pairing key, not the PIN — a photo
                  // of this screen taken later is worthless.
                  _QrPlate(
                    code: state.running ? code : null,
                    onTap: state.running && code != null
                        ? () => showQrDialog(
                              context,
                              code: code,
                              url: state.url,
                              startInverted: qrInverted,
                              onInvert: onQrInvert,
                            )
                        : null,
                  ),
                  const SizedBox(width: 14),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        const _FieldLabel('ADDRESS'),
                        const SizedBox(height: 5),
                        ValueField(
                          value: state.running ? state.url : '—',
                          size: 13,
                          tone: Tint.accent,
                          weight: FontWeight.w700,
                          // Opens this machine's browser on the phone page, already
                          // paired — the URL carries a fresh single-use key, the
                          // same mechanism the QR uses. Without it the desktop was
                          // asked for the PIN it is itself displaying, every time.
                          action: state.running ? 'Open' : '',
                          onAction: controller.openUrl,
                        ),
                        const SizedBox(height: 10),
                        const _FieldLabel('PIN'),
                        const SizedBox(height: 5),
                        ValueField(
                          value: state.running ? state.pin : '——————',
                          size: 22,
                          tone: Tint.accent,
                          weight: FontWeight.w800,
                        ),
                        // The same machine by name, for a phone whose browser
                        // resolves .local. Under the address rather than instead of
                        // it: plenty do not.
                        if (state.hostUrl.isNotEmpty) ...[
                          const SizedBox(height: 8),
                          Text(
                            'also ${state.hostUrl}',
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 10.5, color: t.textDim),
                          ),
                        ],
                        // Somebody grinding the PIN is silent from this end
                        // otherwise: the guesser gets a 429 and the desktop shows
                        // nothing.
                        if (state.attempts.isNotEmpty) ...[
                          const SizedBox(height: 8),
                          Text(
                            state.attempts,
                            // Two lines is two addresses; a third is a list, and a
                            // list belongs somewhere it can be read rather than in
                            // a card pinned to one height.
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                            style: const TextStyle(
                              fontSize: 11,
                              fontWeight: FontWeight.w600,
                              color: Tokens.error,
                            ),
                          ),
                        ],
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),

        // Both a hotspot and a Wi-Fi link commonly qualify; only the person
        // holding the phone knows which.
        if (state.ifaces.length > 1) ...[
          const SizedBox(height: 12),
          const _FieldLabel('THIS NETWORK'),
          const SizedBox(height: 7),
          Wrap(
            spacing: 6,
            runSpacing: 6,
            children: [
              for (final f in state.ifaces)
                _IfaceChip(
                  iface: f,
                  active: f.ip == state.iface,
                  onTap: () => controller.setIface(f.ip),
                ),
            ],
          ),
        ],

        const SizedBox(height: 12),
        // Here rather than in Send: pairing is how a device gets in, which is
        // this card's whole subject. Two rows of five is exactly `deviceMax`,
        // so the box never scrolls and never grows.
        Outline(
          label: 'PAIRED DEVICES',
          note: '${state.devices.length} / ${state.deviceMax}',
          empty: state.running
              ? 'Nothing paired yet. Scan the code, or type the address and PIN on the phone.'
              : 'Start sharing, then scan the code from the phone.',
          isEmpty: state.devices.isEmpty,
          rows: 2,
          // Two chip rows plus the gap between them, stated once rather
          // than as a number that has to be kept in step by hand.
          rowH: _chipExtent + 4,
          // Red once it is full — the count is the only warning that the next
          // phone will be turned away, and a sentence saying so made the card
          // scroll.
          tint: state.devices.length >= state.deviceMax
              ? Tokens.error
              : Tint.pair,
          children: [
            // One grid, not a list of rows: five to a row, which is what the
            // Slint card computes by index arithmetic for the same reason.
            // `mainAxisExtent`, not `childAspectRatio`: a chip is a 42px
            // circle over one line of caption whatever the card is wide, and
            // an aspect ratio makes the row grow with the column until two
            // rows no longer fit in the box that holds them.
            GridView(
              padding: EdgeInsets.zero,
              physics: const NeverScrollableScrollPhysics(),
              shrinkWrap: true,
              gridDelegate: const SliverGridDelegateWithFixedCrossAxisCount(
                crossAxisCount: 5,
                mainAxisExtent: _chipExtent,
                mainAxisSpacing: 4,
                crossAxisSpacing: 4,
              ),
              children: [
                for (final d in state.devices)
                  _DeviceChip(device: d, onTap: () => onDeviceTap(d)),
              ],
            ),
          ],
        ),
      ],
    );
  }
}

class _FieldLabel extends StatelessWidget {
  const _FieldLabel(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Text(
        text,
        style: TextStyle(
          fontSize: 10,
          letterSpacing: 1.3,
          fontWeight: FontWeight.w700,
          color: context.tokens.textDim,
        ),
      );
}

class _QrPlate extends StatelessWidget {
  const _QrPlate({required this.code, required this.onTap});

  final QrCode? code;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final code = this.code;
    return GestureDetector(
      onTap: onTap,
      child: MouseRegion(
        cursor:
            onTap == null ? SystemMouseCursors.basic : SystemMouseCursors.click,
        child: Container(
          width: 140,
          height: 140,
          decoration: BoxDecoration(
            // The white plate needs a visible edge of its own or it floats in
            // the panel it sits in on the light theme.
            color: code == null ? t.panel2 : Colors.white,
            borderRadius: BorderRadius.circular(14),
            border: Border.all(color: edge(Tint.accent, 0.38), width: 1.5),
          ),
          padding: const EdgeInsets.all(6),
          child: code == null
              ? Center(
                  child: Text(
                    'Not sharing',
                    style: TextStyle(fontSize: 12, color: t.textDim),
                  ),
                )
              : QrView(code: code),
        ),
      ),
    );
  }
}

class _IfaceChip extends StatelessWidget {
  const _IfaceChip({
    required this.iface,
    required this.active,
    required this.onTap,
  });

  final TransferIface iface;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(14),
      child: Container(
        height: 28,
        padding: const EdgeInsets.symmetric(horizontal: 11),
        decoration: BoxDecoration(
          color: active ? wash(Tint.accent, 0.18) : t.panel2,
          borderRadius: BorderRadius.circular(14),
          border: Border.all(color: active ? Tint.accent : t.outline),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(
              iface.ip,
              style: TextStyle(
                fontSize: 11.5,
                fontWeight: FontWeight.w700,
                color: active ? Tint.accent : t.text,
              ),
            ),
            const SizedBox(width: 6),
            Text(
              iface.name,
              style: TextStyle(fontSize: 10.5, color: t.textDim),
            ),
          ],
        ),
      ),
    );
  }
}

/// One paired device, as a round button.
///
/// Busy is a blink inside the circle, not a ring around it: a ring grew the
/// button and clipped against the neighbouring cell, and fill plus border alpha
/// carry the same "this one is moving a file" without a size change.
class _DeviceChip extends StatefulWidget {
  const _DeviceChip({required this.device, required this.onTap});

  final TransferDevice device;
  final VoidCallback onTap;

  @override
  State<_DeviceChip> createState() => _DeviceChipState();
}

class _DeviceChipState extends State<_DeviceChip>
    with SingleTickerProviderStateMixin {
  late final AnimationController _beat = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 1100),
  );

  @override
  void initState() {
    super.initState();
    _sync();
  }

  @override
  void didUpdateWidget(_DeviceChip old) {
    super.didUpdateWidget(old);
    _sync();
  }

  /// Only animates while busy, so an idle list is not asking for a repaint on
  /// every frame.
  void _sync() {
    if (widget.device.busy) {
      if (!_beat.isAnimating) _beat.repeat(reverse: true);
    } else {
      _beat.stop();
      _beat.value = 0;
    }
  }

  @override
  void dispose() {
    _beat.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final d = widget.device;
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        AnimatedBuilder(
          animation: _beat,
          builder: (context, _) {
            final beat = _beat.value;
            final fill = d.busy
                ? wash(Tokens.ok, 0.10 + 0.26 * beat)
                : wash(Tint.pair, 0.11);
            final line = d.busy
                ? edge(Tokens.ok, 0.40 + 0.55 * beat)
                : edge(Tint.pair, 0.32);
            return InkWell(
              onTap: widget.onTap,
              customBorder: const CircleBorder(),
              child: Container(
                width: 42,
                height: 42,
                decoration: BoxDecoration(
                  shape: BoxShape.circle,
                  color: fill,
                  border: Border.all(color: line, width: 1.5),
                ),
                child: Icon(
                  deviceGlyph(d.kind),
                  size: 18,
                  color: d.busy ? Tokens.ok : Tint.pair,
                ),
              ),
            );
          },
        ),
        const SizedBox(height: 4),
        // An explicit line height, because Sora's default metrics leave the
        // caption taller than the cell that holds it and a chip is not
        // allowed to be a different height in two places.
        Text(
          d.name,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          textAlign: TextAlign.center,
          style: TextStyle(
            fontSize: 10.5,
            height: 1.1,
            fontWeight: FontWeight.w600,
            color: t.text,
          ),
        ),
      ],
    );
  }
}
