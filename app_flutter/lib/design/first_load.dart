// The gap between "asked" and "answered", and what to draw when the answer
// never comes.
//
// Every section page keeps its error banner inside the `state != null` branch,
// because that is where the rest of the page is. That leaves one state with no
// rendering at all: the *first* call failed, so there is no state to hang a
// banner off, and the page spins forever over an exception nobody sees. This is
// that state — a spinner until something goes wrong, then the reason and a way
// to ask again.

import 'package:flutter/material.dart';

import 'tokens.dart';

class FirstLoad extends StatelessWidget {
  const FirstLoad({super.key, required this.error, required this.onRetry});

  /// The controller's `error`. Null while the first call is still in flight.
  final Object? error;

  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (error == null) {
      return const Center(child: CircularProgressIndicator());
    }
    return Center(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 460),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(Icons.error_outline, size: 34, color: Tokens.error),
            const SizedBox(height: 12),
            Text("This section couldn't load",
                style: TextStyle(
                    fontSize: 15, fontWeight: FontWeight.w700, color: t.text)),
            const SizedBox(height: 8),
            Text('$error',
                textAlign: TextAlign.center,
                style: TextStyle(fontSize: 12.5, color: t.textDim)),
            const SizedBox(height: 14),
            OutlinedButton.icon(
              onPressed: onRetry,
              icon: const Icon(Icons.refresh, size: 16),
              label: const Text('Try again'),
            ),
          ],
        ),
      ),
    );
  }
}
