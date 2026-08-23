// The optional real certificate.
//
// The default is trust-on-first-use: the server signs its own leaf and the
// phone accepts it once, SSH-style. That stays the default, and there is a good
// reason it has to — installing a user CA on Android makes the device fail Play
// Integrity, so banking apps stop working, and no padlock on a LAN file
// transfer is worth that.
//
// If you own a domain, though, there is a better answer. A name in public DNS
// pointing at this machine's LAN address can carry a certificate a public CA
// issued, and then every phone trusts it with nothing installed and no warning
// to click through. Let's Encrypt cannot reach a private address, so the proof
// is a TXT record instead — DNS-01 — and this card is the three steps of it.
//
// Three things this card cannot fix, and says so where it matters:
//   · Renewal is 90 days, and by hand. The expiry is on the card for that.
//   · Many routers drop public DNS answers pointing into RFC1918 space —
//     dnsmasq's stop-dns-rebind, Pi-hole, some ISP boxes — and where that is on,
//     the name simply does not resolve on your own LAN.
//   · No internet means no DNS means no name, so the self-signed path stays.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/transfer.dart';
import 'transfer_controller.dart';
import 'transfer_widgets.dart';

class CertificateCard extends StatefulWidget {
  const CertificateCard({super.key, required this.controller, required this.state});

  final TransferController controller;
  final TransferState state;

  @override
  State<CertificateCard> createState() => _CertificateCardState();
}

class _CertificateCardState extends State<CertificateCard> {
  final TextEditingController _host = TextEditingController();
  bool _open = false;

  @override
  void dispose() {
    _host.dispose();
    super.dispose();
  }

  Future<void> _request() async {
    final host = _host.text.trim();
    if (host.isEmpty) return;
    await widget.controller.send(TransferCmd.certRequest(host: host));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.state;
    final issued = st.certHost.isNotEmpty;

    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 11),
      decoration: BoxDecoration(
        color: wash(issued ? Tint.recv : Tint.pair, 0.08),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: edge(issued ? Tint.recv : Tint.pair, 0.32)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(issued ? Icons.lock : Icons.lock_outline,
                  size: 15, color: issued ? Tint.recv : Tint.pair),
              const SizedBox(width: 9),
              Expanded(
                child: Text(
                  issued
                      ? 'A real certificate, for ${st.certHost}'
                      : 'Own a domain? Get a real padlock.',
                  style: TextStyle(
                      fontSize: 12, fontWeight: FontWeight.w700, color: t.text),
                ),
              ),
              if (issued)
                TextButton(
                  onPressed: () =>
                      widget.controller.send(const TransferCmd.certForget()),
                  child: const Text('Remove'),
                )
              else if (!st.certWaiting)
                TextButton(
                  onPressed: () => setState(() => _open = !_open),
                  child: Text(_open ? 'Not now' : 'Set it up'),
                ),
            ],
          ),
          if (issued) ...[
            const SizedBox(height: 3),
            Text(
              'Phones trust it with nothing installed. Let’s Encrypt issues for '
              '90 days, so this needs doing again in about three months — and if '
              'the name stops resolving on your LAN, the old accept-once '
              'certificate takes over by itself.',
              style: TextStyle(fontSize: 11, color: t.textDim),
            ),
          ] else if (st.certWaiting) ...[
            const SizedBox(height: 8),
            Text('Add this TXT record, then press Continue.',
                style: TextStyle(fontSize: 11.5, color: t.text)),
            const SizedBox(height: 8),
            _Record(label: 'Name', value: st.certRecord),
            const SizedBox(height: 6),
            _Record(label: 'Value', value: st.certValue),
            const SizedBox(height: 8),
            Text(
              'Check it is visible before continuing — `dig TXT ${st.certRecord}` '
              'from anywhere. A record that has not propagated spends one of the '
              'order’s attempts on a certain failure.',
              style: TextStyle(fontSize: 10.5, color: t.textDim),
            ),
            const SizedBox(height: 10),
            Row(
              children: [
                FilledButton(
                  onPressed: () =>
                      widget.controller.send(const TransferCmd.certConfirm()),
                  child: const Text('Continue'),
                ),
                const SizedBox(width: 8),
                TextButton(
                  onPressed: () =>
                      widget.controller.send(const TransferCmd.certForget()),
                  child: const Text('Cancel'),
                ),
              ],
            ),
          ] else if (_open) ...[
            const SizedBox(height: 8),
            Text(
              'Point a name you control at this machine’s LAN address in public '
              'DNS — an A record, ${st.hostUrl.isEmpty ? 'your LAN IP' : st.hostUrl} — '
              'then ask for a certificate for that name.',
              style: TextStyle(fontSize: 11, color: t.textDim),
            ),
            const SizedBox(height: 10),
            Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: _host,
                    style: TextStyle(fontSize: 12.5, color: t.text),
                    decoration: const InputDecoration(
                      isDense: true,
                      border: OutlineInputBorder(),
                      labelText: 'Hostname',
                      hintText: 'test.tulipix.pro',
                    ),
                    onSubmitted: (_) => _request(),
                  ),
                ),
                const SizedBox(width: 10),
                FilledButton(
                  onPressed: _request,
                  child: const Text('Request'),
                ),
              ],
            ),
            const SizedBox(height: 8),
            Text(
              'Some routers drop public DNS answers that point at a private '
              'address — it looks exactly like a rebinding attack — and where '
              'that is on, the name will not resolve on your own network. The '
              'accept-once certificate stays underneath either way.',
              style: TextStyle(fontSize: 10.5, color: t.textDim),
            ),
          ],
        ],
      ),
    );
  }
}

/// One copyable line. A TXT value is 43 characters of base64url and retyping it
/// into a DNS panel is how this goes wrong.
class _Record extends StatelessWidget {
  const _Record({required this.label, required this.value});

  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        SizedBox(
          width: 46,
          child: Text(label,
              style: TextStyle(fontSize: 10.5, color: t.textDim)),
        ),
        Expanded(
          child: SelectableText(
            value,
            style: TextStyle(
                fontSize: 10.5, fontFamily: 'monospace', color: t.text),
          ),
        ),
        IconButton(
          iconSize: 15,
          tooltip: 'Copy',
          icon: const Icon(Icons.copy_all_outlined),
          onPressed: () => Clipboard.setData(ClipboardData(text: value)),
        ),
      ],
    );
  }
}
