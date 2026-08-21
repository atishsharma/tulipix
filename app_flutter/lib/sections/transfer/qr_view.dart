// The pairing code.
//
// The bridge hands over a module matrix rather than an image, so this paints
// squares. That is what makes the enlarged view free: it is the same matrix at
// a bigger side, not a second render — and re-rendering would mean spending a
// second single-use pairing key to change a colour.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/transfer.dart';
import 'transfer_widgets.dart';

/// Modules of quiet margin. Four is what the spec requires for a scanner to
/// find the finder patterns against a busy background.
const _quiet = 4;

class QrView extends StatelessWidget {
  const QrView({super.key, required this.code, this.invert = false});

  final QrCode code;
  final bool invert;

  @override
  Widget build(BuildContext context) => CustomPaint(
        painter: _QrPainter(code: code, invert: invert),
        child: const SizedBox.expand(),
      );
}

class _QrPainter extends CustomPainter {
  const _QrPainter({required this.code, required this.invert});

  final QrCode code;
  final bool invert;

  @override
  void paint(Canvas canvas, Size size) {
    final modules = code.size;
    if (modules <= 0) return;
    final span = modules + _quiet * 2;
    // Floor the module size and centre what is left over, so every module is
    // the same whole number of pixels. A fractional module is what makes a
    // scanned code fail on the odd screen.
    final unit = (size.shortestSide / span).floorToDouble();
    if (unit < 1) return;
    final side = unit * span;
    final ox = (size.width - side) / 2;
    final oy = (size.height - side) / 2;

    final ground = invert ? const Color(0xFF0C0E16) : const Color(0xFFFFFFFF);
    final dark = invert ? const Color(0xFFF4F6FF) : const Color(0xFF000000);

    canvas.drawRect(Rect.fromLTWH(ox, oy, side, side), Paint()..color = ground);
    final ink = Paint()..color = dark;
    for (var y = 0; y < modules; y++) {
      for (var x = 0; x < modules; x++) {
        if (!code.modules[y * modules + x]) continue;
        canvas.drawRect(
          Rect.fromLTWH(
            ox + (x + _quiet) * unit,
            oy + (y + _quiet) * unit,
            unit,
            unit,
          ),
          ink,
        );
      }
    }
  }

  @override
  bool shouldRepaint(_QrPainter old) =>
      old.invert != invert || !identical(old.code, code);
}

/// The enlarged code.
///
/// 140px is a scan target for a phone held near the screen; across a desk it is
/// not. Clicking the code blows it up rather than making every session's card
/// bigger for the one time in ten that the distance matters.
Future<void> showQrDialog(
  BuildContext context, {
  required QrCode code,
  required String url,
  required bool startInverted,
  required ValueChanged<bool> onInvert,
}) {
  return showDialog<void>(
    context: context,
    builder: (context) {
      var dark = startInverted;
      return StatefulBuilder(
        builder: (context, setLocal) {
          final t = context.tokens;
          return AlertDialog(
            backgroundColor: t.modal,
            title: const Text(
              'Scan to pair',
              textAlign: TextAlign.center,
              style: TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: 15,
                fontWeight: FontWeight.w800,
              ),
            ),
            content: SizedBox(
              width: 380,
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Container(
                    height: 316,
                    decoration: BoxDecoration(
                      color: dark
                          ? const Color(0xFF0C0E16)
                          : const Color(0xFFFFFFFF),
                      borderRadius: BorderRadius.circular(16),
                      border: Border.all(
                        color: edge(Tint.accent, 0.38),
                        width: 1.5,
                      ),
                    ),
                    child: QrView(code: code, invert: dark),
                  ),
                  const SizedBox(height: 12),
                  Text(
                    url,
                    textAlign: TextAlign.center,
                    style: TextStyle(fontSize: 11.5, color: t.textDim),
                  ),
                  // Said once, here, rather than discovered as a code that will
                  // not scan on somebody's phone.
                  if (dark) ...[
                    const SizedBox(height: 10),
                    Text(
                      'A few scanners only read dark-on-light. Switch back if '
                      'this one will not take.',
                      textAlign: TextAlign.center,
                      style: TextStyle(fontSize: 10.5, color: t.textDim),
                    ),
                  ],
                ],
              ),
            ),
            actionsAlignment: MainAxisAlignment.center,
            actions: [
              ActionBtn(
                label: dark ? 'Normal code' : 'Invert code',
                icon: Icons.contrast,
                tint: Tint.accent,
                onTap: () {
                  setLocal(() => dark = !dark);
                  onInvert(dark);
                },
              ),
              ActionBtn(
                label: 'Close',
                tint: Tint.help,
                onTap: () => Navigator.of(context).pop(),
              ),
            ],
          );
        },
      );
    },
  );
}
