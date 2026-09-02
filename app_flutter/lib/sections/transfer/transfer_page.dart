// Transfer — the desktop half of "a phone on the same Wi-Fi opens a URL".
//
// Three cards across the top, one per question a person actually has:
//
//   Connection — how do I get in?      QR, address, PIN, what it works with.
//   Send       — how do I give a file? One big target.
//   Receive    — where do files land?  The inbox, and what is arriving.
//
// Recent Transfers is a real table below them, full width.
//
// Ports ui/page_transfer.slint. Identity: orange → amber, the one hue the six
// other sections leave free, so a glance at the accent says which section this
// is.

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/transfer.dart';
import 'certificate_card.dart';
import 'connection_card.dart';
import 'receive_card.dart';
import 'send_card.dart';
import 'transfer_controller.dart';
import 'transfer_dialogs.dart';
import 'transfer_widgets.dart';
import 'transfers_table.dart';

/// One height for all three cards, so none of them moves when a file is chosen
/// or a phone pairs. Every card has one panel that stretches into whatever
/// slack is left, so the extra never shows as a gap.
const _cardHeight = 620.0;

/// Below this the three columns stop being readable and stack instead.
///
/// Three ways of arriving at the number: the Connection card has a 140px QR
/// plate beside the address and PIN, Send has two buttons side by side, and
/// Receive has a folder path to show. Under about 380px each, all three stop
/// working — so three of those plus two 20px gutters plus the page's own
/// padding is where the layout gives up and stacks.
const _threeColumnMin = 1240.0;

class TransferPage extends StatefulWidget {
  const TransferPage({super.key, this.visible = true});

  /// The shell keeps this page alive behind the others, so it has to be told
  /// when it is being looked at.
  final bool visible;

  @override
  State<TransferPage> createState() => _TransferPageState();
}

class _TransferPageState extends State<TransferPage> {
  final _c = TransferController();

  /// Which way round the enlarged code is drawn. Reset by nothing: whoever
  /// flipped it did so because their screen or their eyes wanted it that way,
  /// and it should still be that way next time.
  bool _qrInverted = false;

  @override
  void initState() {
    super.initState();
    _c.setVisible(widget.visible);
    _c.start();
  }

  @override
  void didUpdateWidget(TransferPage old) {
    super.didUpdateWidget(old);
    if (old.visible != widget.visible) _c.setVisible(widget.visible);
  }

  @override
  void dispose() {
    // The server is not stopped here. Sharing lasts as long as the app does, so
    // a phone can keep pulling a 4 GB file while the desktop is used for
    // something else. Stop is an explicit button, or app exit.
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: t.nCanvas,
      child: ListenableBuilder(
        listenable: _c,
        builder: (context, _) {
          final state = _c.state;
          if (state == null) {
            return FirstLoad(error: _c.error, onRetry: _c.start);
          }
          return Column(
            children: [
              _Header(controller: _c, state: state),
              Expanded(
                child: ListView(
                  padding: const EdgeInsets.all(20),
                  children: [
                    _Cards(
                      controller: _c,
                      state: state,
                      qrInverted: _qrInverted,
                      onQrInvert: (v) => setState(() => _qrInverted = v),
                      onDeviceTap: _openDevice,
                    ),
                    const SizedBox(height: 20),
                    TransfersTable(
                      controller: _c,
                      state: state,
                      onClearHistory: _clearHistory,
                    ),
                    if (state.trustUrl.isNotEmpty) ...[
                      const SizedBox(height: 20),
                      // With a real certificate there is no warning to explain,
                      // so the strip that explains it gives way to the card that
                      // got you one.
                      if (state.certHost.isEmpty)
                        _TrustStrip(
                          trustUrl: state.trustUrl,
                          fingerprint: state.fingerprint,
                        ),
                      const SizedBox(height: 12),
                      CertificateCard(controller: _c, state: state),
                    ],
                    const SizedBox(height: 20),
                    const _Footer(),
                  ],
                ),
              ),
            ],
          );
        },
      ),
    );
  }

  Future<void> _openDevice(TransferDevice device) async {
    final answer = await showDevice(context, device);
    if (answer == null) return;
    final (action, name) = answer;
    switch (action) {
      case DeviceAction.forget:
        await _c.forget(device.token);
      case DeviceAction.save:
        await _c.renameDevice(device.token, name);
      case DeviceAction.close:
        break;
    }
  }

  Future<void> _clearHistory() async {
    if (await confirmClearHistory(context)) await _c.clearHistory();
  }
}

/// The three cards, side by side while there is room for them.
class _Cards extends StatelessWidget {
  const _Cards({
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
    final cards = <Widget>[
      ConnectionCard(
        controller: controller,
        state: state,
        qrInverted: qrInverted,
        onQrInvert: onQrInvert,
        onDeviceTap: onDeviceTap,
        // Straight to the controller, like the interface picker beside it:
        // pairing has no dialog of its own to open first — the digits arrive
        // on the next snapshot, on both machines at once.
        onPair: (base) => controller.send(TransferCmd.pairPeer(base: base)),
      ),
      SendCard(
        controller: controller,
        state: state,
        // Like pairing beside it: one command, and the lanes it opens arrive
        // on the next snapshot rather than through a callback of their own.
        onSendTo: (bases) =>
            controller.send(TransferCmd.sendTray(bases: bases)),
      ),
      ReceiveCard(controller: controller, state: state),
    ];
    return LayoutBuilder(
      builder: (context, box) {
        // Slint pins min == max to force three equal columns, because a stretch
        // factor only divides leftover space and these cards have very
        // different preferred widths. An Expanded each is the same decision.
        if (box.maxWidth >= _threeColumnMin) {
          return SizedBox(
            height: _cardHeight,
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                for (var i = 0; i < cards.length; i++) ...[
                  if (i > 0) const SizedBox(width: 20),
                  Expanded(child: cards[i]),
                ],
              ],
            ),
          );
        }
        return Column(
          children: [
            for (var i = 0; i < cards.length; i++) ...[
              if (i > 0) const SizedBox(height: 20),
              SizedBox(height: _cardHeight, child: cards[i]),
            ],
          ],
        );
      },
    );
  }
}

/// Identity, the live address, and the one control that opens or closes the
/// port. Help sits beside it, because the moment a person needs it is the
/// moment sharing looks on and the phone still says it cannot connect.
class _Header extends StatelessWidget {
  const _Header({required this.controller, required this.state});

  final TransferController controller;
  final TransferState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 76,
      padding: const EdgeInsets.symmetric(horizontal: 24),
      decoration: BoxDecoration(
        color: t.panel,
        border: Border(bottom: BorderSide(color: t.outline)),
      ),
      child: Row(
        children: [
          Container(
            width: 40,
            height: 40,
            decoration: BoxDecoration(
              color: wash(Tint.accent, 0.16),
              borderRadius: BorderRadius.circular(12),
            ),
            child: const Icon(Icons.share, size: 20, color: Tint.accent),
          ),
          const SizedBox(width: 14),
          Text(
            'Transfer',
            style: TextStyle(
              fontFamily: Tokens.fontFamily,
              fontSize: 23,
              fontWeight: FontWeight.w700,
              color: t.text,
            ),
          ),
          const SizedBox(width: 16),
          // The bind error, verbatim, when there is one — a silent failure here
          // is indistinguishable from a dismissed firewall prompt.
          Expanded(
            child: Row(
              children: [
                Icon(Icons.link, size: 14, color: t.textDim),
                const SizedBox(width: 8),
                Expanded(
                  child: Text(
                    state.status,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 12.5, color: t.textDim),
                  ),
                ),
              ],
            ),
          ),
          if (state.running) ...[
            Container(
              height: 28,
              padding: const EdgeInsets.symmetric(horizontal: 11),
              decoration: BoxDecoration(
                color: wash(Tokens.ok, 0.14),
                borderRadius: BorderRadius.circular(14),
              ),
              child: const Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(Icons.wifi, size: 13, color: Tokens.ok),
                  SizedBox(width: 6),
                  Text(
                    'Online',
                    style: TextStyle(
                      fontSize: 11.5,
                      fontWeight: FontWeight.w700,
                      color: Tokens.ok,
                    ),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 12),
          ],
          // Solid either way — an outlined Stop beside a filled Start read as
          // the disabled one of the pair.
          ActionBtn(
            label: state.running ? 'Stop sharing' : 'Start sharing',
            tint: state.running ? Tokens.error : Tint.accent,
            width: 132,
            onTap: controller.busy ? null : controller.toggleSharing,
          ),
          const SizedBox(width: 10),
          ActionBtn(
            label: 'Help',
            icon: Icons.info_outline,
            // Slate: neither of the two things the page does, so it takes
            // neither of their colours.
            tint: Tint.help,
            onTap: () => showHelp(context, port: state.port, hint: state.hint),
          ),
        ],
      ),
    );
  }
}

/// The one thing a phone cannot discover on its own: it meets the browser's
/// certificate warning and has no way to learn there is a page that makes it go
/// away, so the desktop is where that address has to be readable.
class _TrustStrip extends StatelessWidget {
  const _TrustStrip({required this.trustUrl, required this.fingerprint});

  final String trustUrl;
  final String fingerprint;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 11),
      decoration: BoxDecoration(
        color: wash(Tint.pair, 0.08),
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: edge(Tint.pair, 0.32)),
      ),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Container(
            width: 30,
            height: 30,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              color: wash(Tint.pair, 0.18),
            ),
            child: const Icon(
              Icons.shield_outlined,
              size: 15,
              color: Tint.pair,
            ),
          ),
          const SizedBox(width: 11),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  'First time on a phone? The browser asks about the certificate once.',
                  style: TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w700,
                    color: t.text,
                  ),
                ),
                const SizedBox(height: 3),
                Text(
                  'The QR goes to $trustUrl — tap Continue, then Advanced and '
                  'Proceed. The phone remembers this certificate from then on, '
                  'and a different one on this address asks again. Nothing to '
                  'install.',
                  style: TextStyle(fontSize: 11, color: t.textDim),
                ),
                // The half of trust-on-first-use that makes it worth anything:
                // something to compare the phone's warning against. Shown here
                // because this is the end nobody can tamper with from the
                // network.
                if (fingerprint.isNotEmpty) ...[
                  const SizedBox(height: 6),
                  SelectableText(
                    fingerprint,
                    style: TextStyle(
                      fontSize: 10,
                      fontFamily: 'monospace',
                      color: t.textDim,
                    ),
                  ),
                ],
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _Footer extends StatelessWidget {
  const _Footer();

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        Icon(Icons.shield_outlined, size: 14, color: t.textDim),
        const SizedBox(width: 8),
        Text(
          'Transfers go straight between your devices — no relay, no account, '
          'nothing stored on a server.',
          style: TextStyle(fontSize: 11.5, color: t.textDim),
        ),
      ],
    );
  }
}
