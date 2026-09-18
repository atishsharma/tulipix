// The Finances section's state.
//
// Seven tabs over one database, and one snapshot behind all seven. That is the
// same arrangement the Slint build uses and for the same reason: loading only
// the visible tab leaves the other six showing whatever they last had, which is
// exactly how a tab comes to be opened onto stale figures.
//
// Everything ephemeral stays on this side — which drill-down is open, what has
// been typed into a field that has not been submitted, the busy label on the
// sample-data buttons. Everything that is a claim about money is computed in
// Rust, where it is tested.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../platform/pick.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/finances.dart';

/// The seven tabs, in the order the design states them: what happened, where it
/// happened, where the money is, what to do next, then the three lists of
/// things that repeat.
class FinTab {
  const FinTab(this.id, this.label, this.icon);
  final String id;
  final String label;
  final IconData icon;
}

const List<FinTab> finTabs = [
  FinTab('overview', 'Overview', Icons.pie_chart_outline),
  FinTab('txns', 'Transactions', Icons.trending_up),
  FinTab('accounts', 'Accounts', Icons.credit_card_outlined),
  FinTab('planning', 'Planning', Icons.event_note_outlined),
  FinTab('bills', 'Bills', Icons.receipt_long_outlined),
  FinTab('subs', 'Subscriptions', Icons.repeat),
  FinTab('dues', 'Lending', Icons.people_outline),
];

class FinancesController extends ChangeNotifier {
  FinancesController() {
    _events = financesEvents().listen(_onEvent, onError: (Object e) {
      error = e;
      notifyListeners();
    });
  }

  FinancesState? state;
  Object? error;
  bool busy = false;

  /// What the section noticed on its first tick — one banner, dismissable, and
  /// never re-raised. Kept in Dart because it is a notification, not state: a
  /// snapshot taken a second later must not resurrect it.
  ({String title, String body})? notice;

  /// Which of the two sample-data buttons is running. Dart's own, because it is
  /// the side that knows which one it pressed, and `busy` alone cannot say.
  String demoBusy = '';

  StreamSubscription<FinancesEvent>? _events;

  void _onEvent(FinancesEvent event) {
    switch (event) {
      case FinancesEvent_Notice(:final title, :final body):
        notice = (title: title, body: body);
        notifyListeners();
      case FinancesEvent_Failed(:final message):
        error = message;
        notifyListeners();
    }
  }

  String get tab => state?.tab ?? 'overview';
  bool get sheetOpen => (state?.sheet ?? '').isNotEmpty;

  Future<void> send(FinancesCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      state = await financesDispatch(cmd: cmd);
    } catch (e) {
      error = e;
    } finally {
      busy = false;
      demoBusy = '';
      notifyListeners();
    }
  }

  Future<void> refresh() => send(const FinancesCmd.refresh());

  /// The two places this section reads a file the user picked. The chooser used
  /// to be `rfd`, inside the bridge; the extension list still comes from there,
  /// because the importer is what has to read the file.
  Future<void> importStatement() async {
    final path = await pickFile(
      label: 'Statements',
      extensions: financesImportExtensions(),
    );
    if (path == null) return;
    await send(FinancesCmd.importPick(path: path));
  }

  Future<void> scanReceipt() async {
    final path = await pickFile(
      label: 'Images',
      extensions: const ['jpg', 'jpeg', 'png', 'webp', 'tif', 'tiff', 'bmp'],
    );
    if (path == null) return;
    await send(FinancesCmd.scanReceipt(path: path));
  }

  /// Sample data. The label is set here rather than read off the snapshot: both
  /// operations take about the same time and look identical, so the chip has to
  /// say which one is running.
  Future<void> demo({required bool add}) {
    demoBusy = add ? 'Adding sample data…' : 'Clearing the section…';
    notifyListeners();
    return send(
        add ? const FinancesCmd.demoAdd() : const FinancesCmd.demoRemove());
  }

  void clearError() {
    error = null;
    notifyListeners();
  }

  void dismissNotice() {
    notice = null;
    notifyListeners();
  }

  @override
  void dispose() {
    _events?.cancel();
    super.dispose();
  }
}

// ------------------------------------------------------------------ colour ---

/// Hues cross the boundary as `0xAARRGGBB` ints, never as a colour type — a
/// struct carrying a platform colour would be one the Rust tests could not
/// construct.
Color hue(int argb) => Color(argb);

/// The four tones every stat, row and flag is tagged with. `flat` is the
/// section's own ink, so a figure that is merely a figure does not shout.
Color toneColour(BuildContext context, String tone) {
  final t = context.tokens;
  return switch (tone) {
    'ok' || 'good' => Tokens.ok,
    'warn' => Tokens.warn,
    'bad' || 'error' => Tokens.error,
    'accent' => Tokens.secFinances,
    _ => t.nInk,
  };
}

/// Severity as the insight cards wear it.
({Color colour, IconData icon}) severityLook(String severity) =>
    switch (severity) {
      'bad' => (colour: Tokens.error, icon: Icons.error_outline),
      'warn' => (colour: Tokens.warn, icon: Icons.warning_amber_outlined),
      'good' => (colour: Tokens.ok, icon: Icons.check_circle_outline),
      _ => (colour: Tokens.secFinances, icon: Icons.info_outline),
    };

/// A money string that already carries its own sign. Expense rows are drawn in
/// the section's ink and income in green — the sign is on the string, so the
/// colour only has to agree with it rather than encode it a second time.
Color amountColour(BuildContext context, String kind) {
  final t = context.tokens;
  return switch (kind) {
    'income' => Tokens.ok,
    'transfer' => t.nInk2,
    _ => t.nInk,
  };
}

/// Days-until as the tables phrase it. Negative is overdue, which is the one
/// case that gets a word rather than a count.
String dueIn(int days) {
  if (days < -1) return '${-days} days late';
  if (days == -1) return 'Yesterday';
  if (days == 0) return 'Today';
  if (days == 1) return 'Tomorrow';
  return 'in $days days';
}
