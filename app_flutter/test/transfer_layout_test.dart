// Layout regression for the Transfer page.
//
// Every widget here is pure: nothing calls the bridge, so this runs without the
// native library. What it catches is the class of bug that only shows at run
// time — a Column asking for more height than it was given — which is otherwise
// found by a person looking at a red-and-yellow screen.
//
// Sizes are deliberately cruel as well as realistic: the three cards are
// pinned to one height, so a narrow window is where they break first.

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:tulipix/design/tokens.dart';
import 'package:tulipix/sections/transfer/connection_card.dart';
import 'package:tulipix/sections/transfer/receive_card.dart';
import 'package:tulipix/sections/transfer/send_card.dart';
import 'package:tulipix/sections/transfer/transfer_controller.dart';
import 'package:tulipix/sections/transfer/transfer_widgets.dart';
import 'package:tulipix/sections/transfer/transfers_table.dart';
import 'package:tulipix/src/rust/api/transfer.dart';

TransferState _state({
  bool running = true,
  int devices = 0,
  int files = 0,
  int uploads = 0,
  int rows = 0,
  int ifaces = 1,
  int peers = 0,
  String attempts = '',
  List<TransferLane> lanes = const [],
}) {
  return TransferState(
    running: running,
    status: 'Serving on https://192.168.1.24:8420/. Open until you stop '
        'sharing or close the app.',
    hint: '',
    attempts: attempts,
    url: 'https://192.168.1.24:8420/',
    trustUrl: running ? 'http://192.168.1.24:8421/trust' : '',
    fingerprint: running ? 'AA:BB:CC:DD:EE:FF' * 4 : '',
    hostUrl: running ? 'https://tulipix.local:8420/' : '',
    pin: running ? '481920' : '',
    port: 8420,
    qrRev: 0,
    files: [
      for (var i = 0; i < files; i++)
        TransferFile(
          id: i,
          name: 'a-rather-long-file-name-$i.tar.gz',
          size: '412.4 MB',
          kind: 'GZ',
          to: i.isEven ? '' : 'Pixel 8 Pro',
        ),
    ],
    total: files == 0 ? '' : '$files files · 1.2 GB',
    devices: [
      for (var i = 0; i < devices; i++)
        TransferDevice(
          token: 't$i',
          name: 'Somebody’s Phone $i',
          kind: i.isEven ? 'Android' : 'iPhone',
          label: 'Android · Chrome 141',
          ip: '192.168.1.${40 + i}',
          pin: '481920',
          remaining: '47 hours',
          seen: 'just now',
          busy: i == 0,
        ),
    ],
    deviceMax: 10,
    peers: [
      for (var i = 0; i < peers; i++)
        TransferPeer(
          host: 'tulipix.local',
          ip: '192.168.1.${31 + i}',
          port: 8420,
          base: 'https://192.168.1.${31 + i}:8420',
          paired: i.isEven,
        ),
    ],
    // No widget yet; here so this file compiles against the state the bridge
    // actually hands over.
    offers: const [],
    lanes: lanes,
    uploads: [
      for (var i = 0; i < uploads; i++)
        TransferUpload(
          id: i,
          name: 'incoming-$i.mp4',
          size: '2.1 GB',
          pct: 0.42,
          state: i == 0 ? 'active' : (i == 1 ? 'failed' : 'done'),
          moved: i == 0 ? '880 MB / 2.1 GB' : '',
          rate: i == 0 ? '31.2 MB/s' : '',
        ),
    ],
    rows: [
      for (var i = 0; i < rows; i++)
        TransferLedgerRow(
          id: i,
          direction: i.isEven ? 'in' : 'out',
          name: 'transferred-file-with-a-long-name-$i.pdf',
          path: i == 3 ? '' : '/home/x/Downloads/f$i.pdf',
          size: '12.4 MB',
          peer: 'Somebody’s Phone 0',
          // One of each: completed, failed (retryable), still sending.
          status: i % 3 == 0 ? 'ok' : (i % 3 == 1 ? 'failed' : 'sending'),
          stamp: '12 min ago',
          kind: 'PDF',
          kindlabel: 'PDF Document',
          missing: i == 5,
          pct: 0.42,
          progress: '42% · 3.1 MB/s',
        ),
    ],
    ifaces: [
      for (var i = 0; i < ifaces; i++)
        TransferIface(name: 'wlan$i', ip: '192.168.$i.24'),
    ],
    iface: '192.168.0.24',
    shareTarget: '',
    shareTargetName: '',
    inbox: '/home/somebody/Downloads/Tulipix',
    inboxOk: true,
    page: 0,
    pages: rows > 0 ? 4 : 1,
    sort: 5,
    sortDesc: true,
    // No certificate: the default path, and the one the trust strip belongs to.
    // The certificate card's own states are not a layout risk — it is a column
    // of text in a box that grows.
    certHost: '',
    certWaiting: false,
    certRecord: '',
    certValue: '',
    pairCode: '',
    pairPeer: '',
  );
}

/// A fan-out in flight: one machine finished, one still moving, one refused.
/// The three states a lane can be caught in, and the only one of them that has
/// something to say afterwards is the failure.
TransferState _stateWithLanes() => _state(files: 2, lanes: const [
      TransferLane(
        name: '192.168.1.31',
        pct: 1,
        state: 'done',
        detail: '',
      ),
      TransferLane(
        name: '192.168.1.32',
        pct: 0.25,
        state: 'sending',
        detail: '5 MB / 20 MB',
      ),
      TransferLane(
        name: '192.168.1.33',
        pct: 0,
        state: 'failed',
        detail: 'connection refused',
      ),
    ]);

Widget _host(Widget child, {double width = 360, double height = 560}) {
  return MaterialApp(
    theme: tulipixTheme(Tokens.dark()),
    home: Scaffold(
      body: Center(
        child: SizedBox(width: width, height: height, child: child),
      ),
    ),
  );
}

/// Pump and report every layout error, not just the first — the framework
/// prints the first in full and swallows the detail of the rest, which is
/// exactly backwards when a card has three of them.
Future<void> pumpClean(WidgetTester tester, Widget widget) async {
  final errs = <FlutterErrorDetails>[];
  final prev = FlutterError.onError;
  FlutterError.onError = errs.add;
  await tester.pumpWidget(widget);
  FlutterError.onError = prev;
  if (errs.isNotEmpty) {
    for (final e in errs) {
      // ignore: avoid_print
      print('LAYOUT: ${e.exception}');
      final at = RegExp(
        r'(lib/sections/transfer/\w+\.dart:\d+:\d+)',
      ).firstMatch(e.toString());
      // ignore: avoid_print
      print('   at ${at?.group(1) ?? "unknown"}');
    }
    fail('${errs.length} layout error(s); see above');
  }
}

void main() {
  final c = TransferController();

  group('the three cards fit the height they are pinned to', () {
    // The widths are the three-column layout at 1040 (the narrowest it is
    // allowed) and at a comfortable 1600.
    for (final width in [300.0, 360.0, 520.0]) {
      testWidgets('Connection at ${width}px', (tester) async {
        await pumpClean(
          tester,
          _host(
            ConnectionCard(
              controller: c,
              state: _state(
                  devices: 10,
                  ifaces: 3,
                  peers: 3,
                  attempts: '192.168.1.9 — 3 wrong of 5'),
              qrInverted: false,
              onQrInvert: (_) {},
              onDeviceTap: (_) {},
              onPair: (_) {},
            ),
            width: width,
          ),
        );
      });

      testWidgets('Send at ${width}px', (tester) async {
        await pumpClean(
          tester,
          _host(
            SendCard(
              controller: c,
              state: _state(files: 6, devices: 3),
              onSendTo: (_) {},
            ),
            width: width,
          ),
        );
      });

      testWidgets('Receive at ${width}px', (tester) async {
        await pumpClean(
          tester,
          _host(
            ReceiveCard(controller: c, state: _state(uploads: 3)),
            width: width,
          ),
        );
      });
    }
  });

  testWidgets('the empty cards fit too', (tester) async {
    for (final card in <Widget>[
      ConnectionCard(
        controller: c,
        state: _state(running: false),
        qrInverted: false,
        onQrInvert: (_) {},
        onDeviceTap: (_) {},
        onPair: (_) {},
      ),
      SendCard(
        controller: c,
        state: _state(running: false),
        onSendTo: (_) {},
      ),
      ReceiveCard(controller: c, state: _state(running: false)),
    ]) {
      await tester.pumpWidget(_host(card));
    }
  });

  testWidgets('the peer grid separates paired machines from found ones',
      (tester) async {
    final tapped = <String>[];
    await pumpClean(
      tester,
      _host(ConnectionCard(
        controller: c,
        state: _state(peers: 3),
        qrInverted: false,
        onQrInvert: (_) {},
        onDeviceTap: (_) {},
        onPair: tapped.add,
      )),
    );

    expect(find.byType(PeerChip), findsNWidgets(3));
    expect(find.text('Pair'), findsOneWidget,
        reason: 'one of the three is unpaired, and only that one offers to pair');
    // Pairing is what an unpaired chip is for; a paired one sends.
    await tester.tap(find.text('192.168.1.32'));
    await tester.pump();
    expect(tapped, ['https://192.168.1.32:8420'],
        reason: 'tapping a found peer starts pairing with that machine');
  });

  testWidgets('no peers on the network leaves no empty grid behind',
      (tester) async {
    await pumpClean(
      tester,
      _host(ConnectionCard(
        controller: c,
        state: _state(devices: 10, ifaces: 3),
        qrInverted: false,
        onQrInvert: (_) {},
        onDeviceTap: (_) {},
        onPair: (_) {},
      )),
    );
    expect(find.byType(PeerChip), findsNothing);
    // Not just "no chips" — an empty `Outline` draws none either, and it is the
    // box being absent that leaves the QR plate its height.
    expect(find.text('OTHER TULIPIX MACHINES'), findsNothing,
        reason: 'an empty box would still cost the panel its height');
  });

  testWidgets('a fan-out draws one lane per destination, failures included',
      (tester) async {
    await pumpClean(
      tester,
      _host(SendCard(
        controller: c,
        state: _stateWithLanes(),
        onSendTo: (_) {},
      )),
    );

    expect(find.byType(LaneRow), findsNWidgets(3));
    // The failure is legible without opening anything, and it keeps its row
    // rather than dropping out of a list of three.
    expect(find.text('connection refused'), findsOneWidget);
    // And the lanes stand in for the drop zone rather than being stacked under
    // it: the card is pinned to one height, and a fourth block would take it
    // out of the only panel that gives way.
    expect(find.text('Choose files to share'), findsNothing);
  });

  testWidgets('picking two destinations sends to both, in the order ticked',
      (tester) async {
    List<String>? sent;
    await pumpClean(
      tester,
      _host(
        SendCard(
          controller: c,
          state: _state(files: 1, peers: 4),
          onSendTo: (bases) => sent = bases,
        ),
        // Wide enough that both chips sit on the box's one row. Narrower and
        // the second wraps out of sight and cannot be tapped — which is a fact
        // about the box's height, and the test below is where that belongs.
        width: 560,
      ),
    );

    // Peers 0 and 2 are the paired ones in the helper, and only those two can
    // be aimed at: an unpaired machine has no token to present.
    expect(find.byType(PeerChip), findsNWidgets(2));
    expect(find.text('192.168.1.32'), findsNothing,
        reason: 'pairing belongs to the Connection card, not to this one');

    // Ticked in the reverse of the order they are listed in, because the order
    // that matters is the one the person picked.
    await tester.tap(find.text('192.168.1.33'));
    await tester.pump();
    await tester.tap(find.text('192.168.1.31'));
    await tester.pump();
    await tester.tap(find.text('Send to 2'));
    await tester.pump();

    expect(sent, [
      'https://192.168.1.33:8420',
      'https://192.168.1.31:8420',
    ], reason: 'one send, two destinations, in the order they were ticked');
  });

  testWidgets('the destination box leaves the drop zone its budget',
      (tester) async {
    // Measured twice, and what is asserted is the *difference*. The drop
    // zone's absolute height is the card height less the header, the card
    // padding, two gaps, `FILES TO SEND`, and the drop zone's own 16px
    // padding and 1.5px border — six terms, any one of which being a few
    // pixels off turns an absolute floor into a test that is red before the
    // regression it is meant to catch exists. The difference depends on
    // `SEND TO` and nothing else, which is the thing under test.
    Future<double> zone(int peers) async {
      await pumpClean(
        tester,
        _host(
          SendCard(
            controller: c,
            state: _state(files: 6, devices: 3, peers: peers),
            onSendTo: (_) {},
          ),
          // The narrowest a card ever really is. `_threeColumnMin` is checked
          // against the width the page's ListView already padded, so the real
          // floor is (1240 - 40 of gutters) / 3 = 400; 387 is under it on
          // purpose, because a budget that survives the cruel case survives
          // the real one. It is also under `Outline`'s 340px control-stacking
          // threshold once the card and box padding come off, so `FILES TO
          // SEND` is at its tallest here — the worst case for what is left.
          width: 387,
          // What transfer_page pins the three cards to. The 560 the sweep
          // above uses is deliberately crueller than the real thing; this one
          // has to be the real thing, because it measures whether the content
          // fits rather than whether it throws.
          height: 620,
        ),
      );
      // The only `PanelBody` in this card is the drop zone's.
      return tester.getSize(find.byType(PanelBody)).height;
    }

    // No paired peer, no destination box: the drop zone has the whole budget.
    final free = await zone(0);
    // Two of four peers are paired, so the box is drawn.
    final taken = await zone(4);

    // `SEND TO` costs the 10px gap above it, `Outline`'s own chrome — 3 of
    // border, 20 of padding, its label line and an 8px gap, call it 44 — and
    // one row of `peerChipH + 6`. About 92. A second row is another 38, and
    // growing by a row is the obvious way for this block to eat the panel
    // that gives way. 110 sits between the two with room on both sides.
    //
    // Asserting the space rather than what is in it, deliberately: the drop
    // zone scrolls rather than overflowing, so a squeezed one throws nothing
    // and pumpClean stays green, and the test font is far wider than the real
    // one, so measuring the *content* would fail here for reasons a person
    // never sees.
    expect(free - taken, lessThan(110),
        reason: 'SEND TO has taken height the drop zone was budgeted');
  });

  testWidgets('the ledger lays out with every row state in it', (tester) async {
    await pumpClean(
      tester,
      _host(
        SingleChildScrollView(
          child: TransfersTable(
            controller: c,
            state: _state(rows: 10),
            onClearHistory: () {},
          ),
        ),
        width: 1200,
        height: 800,
      ),
    );
  });

  testWidgets('and when it is empty', (tester) async {
    await pumpClean(
      tester,
      _host(
        SingleChildScrollView(
          child: TransfersTable(
            controller: c,
            state: _state(),
            onClearHistory: () {},
          ),
        ),
        width: 1200,
        height: 800,
      ),
    );
  });
}
