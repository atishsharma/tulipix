// The three things that open over the page: the Help panel, one paired device,
// and the confirmation before the ledger is wiped.
//
// The Help copy is transcribed from ui/page_transfer.slint rather than
// rewritten. It is advice that has been through real support conversations, and
// the port is not the place to improve it. The one block that cannot be static
// is the firewall hint, which Rust builds with this machine's real port and
// link in it — a rule naming the wrong port is worse than no rule.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../design/skin.dart';
import '../../src/rust/api/transfer.dart';
import 'connection_card.dart' show deviceGlyph;
import 'transfer_widgets.dart';

/// The longest a device name may be. Enforced as it is typed rather than
/// complained about afterwards — and again in Rust, which is what stores it.
const _nameMax = 15;

Future<void> showHelp(
  BuildContext context, {
  required int port,
  required String hint,
}) {
  return showDialog<void>(
    context: context,
    builder: (context) {
      final t = context.tokens;
      return Dialog(
        backgroundColor: t.modalSolid,
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 620, maxHeight: 640),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              // Header, outside the scroll so the close button stays put.
              Padding(
                padding: const EdgeInsets.all(20),
                child: Row(
                  children: [
                    Container(
                      width: 42,
                      height: 42,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: wash(Tint.accent, 0.14),
                      ),
                      child: const Icon(
                        Icons.info_outline,
                        size: 20,
                        color: Tint.accent,
                      ),
                    ),
                    const SizedBox(width: 13),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Text(
                            'Connecting a device',
                            style: TextStyle(
                              fontFamily: context.skin.fontFamily ?? Tokens.fontFamily,
                              fontSize: 17,
                              fontWeight: FontWeight.w700,
                              color: t.text,
                            ),
                          ),
                          const SizedBox(height: 3),
                          Text(
                            'Anything with a browser works. No app to install on the other end.',
                            style: TextStyle(fontSize: 12, color: t.textDim),
                          ),
                        ],
                      ),
                    ),
                    IconButton(
                      onPressed: () => Navigator.of(context).pop(),
                      icon: const Icon(Icons.close, size: 16),
                    ),
                  ],
                ),
              ),
              Divider(height: 1, color: t.outline),
              Flexible(
                child: ListView(
                  padding: const EdgeInsets.all(20),
                  children: [
                    const HelpBlock(
                      title: 'Start here',
                      tint: Tint.accent,
                      icon: Icons.wifi,
                      body: '1.  Press Start sharing on this computer.\n'
                          '2.  Put both devices on the same Wi-Fi, or tether one to the other’s hotspot.\n'
                          '3.  Scan the QR code, or type the address into any browser.\n'
                          '4.  Enter the 6-digit PIN. The QR skips this — it carries a single-use key that lasts 60 seconds.',
                    ),
                    // The live one, with this machine's real port and link
                    // substituted in. First, because a person opening this
                    // panel is almost always already stuck.
                    if (hint.isNotEmpty) ...[
                      const SizedBox(height: 14),
                      HelpBlock(
                        title: 'Phone says it cannot connect',
                        tint: Tokens.warn,
                        icon: Icons.shield_outlined,
                        body: hint,
                      ),
                    ],
                    const SizedBox(height: 14),
                    HelpBlock(
                      title: 'Windows',
                      tint: const Color(0xFF0EA5E9),
                      icon: Icons.desktop_windows_outlined,
                      body:
                          'Windows Firewall asks once, on the very first bind, and dismissing that dialog silently blocks every connection afterwards — with no second prompt.\n\n'
                          'Fix it at Settings → Privacy & security → Windows Security → Firewall & network protection → Allow an app through firewall, and tick Tulipix for Private networks.\n\n'
                          'If the network is marked Public, either switch it to Private or open TCP $port directly.',
                    ),
                    const SizedBox(height: 14),
                    const HelpBlock(
                      title: 'macOS',
                      tint: Color(0xFFA78BFA),
                      icon: Icons.keyboard_command_key,
                      body:
                          'macOS asks for Local Network permission the first time an app talks to the LAN. Denied once, it never asks again.\n\n'
                          'Turn it on at System Settings → Privacy & Security → Local Network → Tulipix.\n\n'
                          'Then check System Settings → Network → Firewall. If it is on, either allow incoming connections for Tulipix or make sure Block all incoming connections is off.',
                    ),
                    const SizedBox(height: 14),
                    HelpBlock(
                      title: 'Linux',
                      tint: const Color(0xFF22C55E),
                      icon: Icons.build_outlined,
                      body:
                          'A default-deny firewall drops the phone’s packets before they reach the app, and this end cannot tell — the bind succeeded either way.\n\n'
                          'ufw:       sudo ufw allow in on <interface> to any port $port proto tcp\n'
                          'firewalld: sudo firewall-cmd --add-port=$port/tcp\n\n'
                          'Scope the rule to the one link rather than opening the port everywhere — otherwise it follows the laptop onto the next network it joins.',
                    ),
                    const SizedBox(height: 14),
                    const HelpBlock(
                      title: 'Android',
                      tint: Color(0xFF84CC16),
                      icon: Icons.download,
                      body:
                          'Any browser works. Chrome, Firefox and Samsung Internet are all fine.\n\n'
                          'Using this phone as the hotspot: the laptop is a client on the phone’s network, so pick the hotspot address from THIS NETWORK on the Connection card — that is the one the phone can reach.\n\n'
                          'Downloads land in the browser’s own folder, usually Downloads. Some Android builds warn about files from a “not secure” page; that is the plain-HTTP notice, not a problem with the file.',
                    ),
                    const SizedBox(height: 14),
                    const HelpBlock(
                      title: 'iPhone and iPad',
                      tint: Color(0xFFF472B6),
                      icon: Icons.upload,
                      body:
                          'Safari works, and the Camera app scans the QR straight into it.\n\n'
                          'Turn off Private Wi-Fi Address for this network if the connection is refused after it worked once — a rotating address invalidates the paired device.\n\n'
                          'Uploading photos: pick Photo Library in the file picker. iOS converts HEIC to JPEG on the way out unless Settings → Photos → Transfer to Mac or PC is set to Keep Originals.',
                    ),
                    const SizedBox(height: 14),
                    HelpBlock(
                      title: 'Still nothing',
                      tint: t.textDim,
                      icon: Icons.warning_amber_rounded,
                      body:
                          'Client isolation — an “AP isolation” or “guest network” setting on the router — blocks devices from seeing each other at all, and no firewall change on either end will fix it. A phone hotspot is the quickest way around it.\n\n'
                          'A VPN on either device routes traffic away from the local network. Turn it off and try again.\n\n'
                          'Corporate and campus Wi-Fi very often has isolation on by design. Expect it to fail there.',
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      );
    },
  );
}

/// What came back from the device popup.
enum DeviceAction { close, save, forget }

/// One paired device: what to call it, what it paired with, how long it has
/// left, and the one destructive thing that can be done to it.
///
/// Works off a copy of the device, not an index into the list: the list is
/// replaced wholesale on every tick, and an index would start pointing at a
/// different phone the moment one of them expired.
Future<(DeviceAction, String)?> showDevice(
  BuildContext context,
  TransferDevice device,
) {
  final field = TextEditingController(text: device.name);
  return showDialog<(DeviceAction, String)>(
    context: context,
    builder: (context) {
      final t = context.tokens;
      return Dialog(
        backgroundColor: t.modalSolid,
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 400),
          child: Padding(
            padding: const EdgeInsets.all(22),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Row(
                  children: [
                    Container(
                      width: 48,
                      height: 48,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: wash(Tint.accent, 0.14),
                      ),
                      child: Icon(
                        deviceGlyph(device.kind),
                        size: 21,
                        color: Tint.accent,
                      ),
                    ),
                    const SizedBox(width: 13),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Text(
                            device.name,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                              fontFamily: context.skin.fontFamily ?? Tokens.fontFamily,
                              fontSize: 16,
                              fontWeight: FontWeight.w700,
                              color: t.text,
                            ),
                          ),
                          const SizedBox(height: 3),
                          // The full User-Agent reading, which is more than
                          // fits under the round button but is what identifies
                          // the thing.
                          Text(
                            '${device.label} · last seen ${device.seen}',
                            style: TextStyle(
                              fontSize: 11.5,
                              color: t.textDim,
                            ),
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 14),
                Divider(height: 1, color: t.outline),
                const SizedBox(height: 14),
                _NameField(field: field, kind: device.kind),
                const SizedBox(height: 14),
                Row(
                  children: [
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          const _Caption('PIN USED'),
                          const SizedBox(height: 5),
                          ValueField(
                            value: device.pin.isEmpty ? '—' : device.pin,
                            size: 15,
                            tone: Tint.accent,
                            weight: FontWeight.w800,
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          const _Caption('ADDRESS'),
                          const SizedBox(height: 5),
                          ValueField(
                            value: device.ip.isEmpty ? '—' : device.ip,
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 12),
                const _Caption('TIME LEFT'),
                const SizedBox(height: 5),
                ValueField(value: device.remaining),
                const SizedBox(height: 16),
                Divider(height: 1, color: t.outline),
                const SizedBox(height: 14),
                Row(
                  children: [
                    // Forget is the destructive one, so it keeps its distance
                    // from Save rather than sitting beside it.
                    DangerBtn(
                      label: 'Forget',
                      icon: Icons.delete_outline,
                      width: 112,
                      height: 38,
                      onTap: () => Navigator.of(
                        context,
                      ).pop((DeviceAction.forget, '')),
                    ),
                    const Spacer(),
                    ActionBtn(
                      label: 'Close',
                      tint: Tint.help,
                      height: 38,
                      onTap: () => Navigator.of(
                        context,
                      ).pop((DeviceAction.close, '')),
                    ),
                    const SizedBox(width: 10),
                    ActionBtn(
                      label: 'Save name',
                      tint: Tint.pair,
                      height: 38,
                      onTap: () => Navigator.of(
                        context,
                      ).pop((DeviceAction.save, field.text.trim())),
                    ),
                  ],
                ),
              ],
            ),
          ),
        ),
      );
    },
  ).whenComplete(field.dispose);
}

class _Caption extends StatelessWidget {
  const _Caption(this.text);

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

class _NameField extends StatefulWidget {
  const _NameField({required this.field, required this.kind});

  final TextEditingController field;
  final String kind;

  @override
  State<_NameField> createState() => _NameFieldState();
}

class _NameFieldState extends State<_NameField> {
  @override
  void initState() {
    super.initState();
    widget.field.addListener(_onEdit);
  }

  @override
  void dispose() {
    widget.field.removeListener(_onEdit);
    super.dispose();
  }

  void _onEdit() => setState(() {});

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        Row(
          children: [
            const Expanded(child: _Caption('NAME')),
            Text(
              '${widget.field.text.characters.length} / $_nameMax',
              style: TextStyle(
                fontSize: 10,
                fontWeight: FontWeight.w700,
                color: t.textDim,
              ),
            ),
          ],
        ),
        const SizedBox(height: 6),
        TextField(
          controller: widget.field,
          autofocus: true,
          // The sixteenth character never appears, so nobody types a name and
          // then loses the end of it.
          maxLength: _nameMax,
          maxLines: 1,
          buildCounter: (_,
                  {required currentLength, required isFocused, maxLength}) =>
              null,
          onSubmitted: (v) => Navigator.of(
            context,
          ).pop((DeviceAction.save, v.trim())),
          style: TextStyle(
            fontSize: 13,
            fontWeight: FontWeight.w600,
            color: t.text,
          ),
          decoration: const InputDecoration(
            isDense: true,
            border: OutlineInputBorder(),
          ),
        ),
        const SizedBox(height: 6),
        Text(
          'Leave it empty to go back to calling it ${widget.kind}.',
          style: TextStyle(fontSize: 10.5, color: t.textDim),
        ),
      ],
    );
  }
}

/// The distinction worth spelling out: this is a log, and people reach for it
/// expecting a delete.
/// Six digits, one question, two buttons.
///
/// The digits are the security of this whole feature. They are derived from
/// both machines' certificate fingerprints, so somebody in the middle can
/// present a certificate of their own but cannot make the two screens agree.
/// That is why the copy says "on both screens", why the barrier does not
/// dismiss it, and why there is no "skip" — a dialog you can wave away is not
/// a comparison anybody made.
///
/// Nothing is issued by pressing "They match" here alone: this answers for
/// this machine, and the far end has its own button.
Future<void> showPairDialog(
  BuildContext context, {
  required String code,
  required String peer,
  required VoidCallback onConfirm,
  required VoidCallback onCancel,
}) {
  return showDialog<void>(
    context: context,
    barrierDismissible: false,
    builder: (context) {
      final t = context.tokens;
      return AlertDialog(
        backgroundColor: t.modalSolid,
        title: const Text('Do these match?'),
        content: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            // One box per digit, so this can be read aloud down a phone line
            // a digit at a time — which is how two machines in two rooms
            // actually get compared.
            Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                for (final d in code.split(''))
                  Container(
                    margin: const EdgeInsets.symmetric(horizontal: 3),
                    width: 34,
                    height: 44,
                    alignment: Alignment.center,
                    decoration: BoxDecoration(
                      color: wash(Tint.pair, 0.12),
                      borderRadius: BorderRadius.circular(8),
                      border: Border.all(color: edge(Tint.pair, 0.42)),
                    ),
                    child: Text(
                      d,
                      style: TextStyle(
                        fontSize: 22,
                        fontWeight: FontWeight.w700,
                        fontFeatures: const [FontFeature.tabularFigures()],
                        color: t.text,
                      ),
                    ),
                  ),
              ],
            ),
            const SizedBox(height: 12),
            Text(
              'Compare these six digits with the ones on $peer. '
              'Only confirm if they are identical on both screens.',
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 11.5, color: t.textDim),
            ),
          ],
        ),
        actions: [
          TextButton(
            onPressed: () {
              onCancel();
              Navigator.of(context).pop();
            },
            child: const Text('They do not'),
          ),
          FilledButton(
            onPressed: () {
              onConfirm();
              Navigator.of(context).pop();
            },
            child: const Text('They match'),
          ),
        ],
      );
    },
  );
}

Future<bool> confirmClearHistory(BuildContext context) async {
  final ok = await showDialog<bool>(
    context: context,
    builder: (context) => AlertDialog(
      backgroundColor: context.tokens.modal,
      icon: const Icon(Icons.delete_outline, color: Tokens.error, size: 28),
      title: const Text('Clear transfer history?'),
      content: const Text(
        'This erases the record of what was sent and received. The files '
        'themselves are not touched, and paired devices stay paired.',
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(false),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: () => Navigator.of(context).pop(true),
          style: FilledButton.styleFrom(backgroundColor: Tokens.error),
          child: const Text('Clear history'),
        ),
      ],
    ),
  );
  return ok ?? false;
}
