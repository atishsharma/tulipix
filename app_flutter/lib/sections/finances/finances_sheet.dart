// One modal, sixteen kinds.
//
// Everything the section writes goes through a sheet, and every sheet is the
// same shape: a title, a hint, a list of fields, an error line and a primary
// button. The fields themselves are built and validated in Rust — this renders
// them and reports back what was typed, and knows nothing about what any of
// them mean.
//
// Five kinds carry something instead of, or beside, the form: the exchange-rate
// table, a recurrence's price history, a loan's amortisation, a statement's
// import preview, and the list of detected recurring charges.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/finances.dart';
import 'finances_controller.dart';
import 'finances_widgets.dart';

class FinancesSheet extends StatelessWidget {
  const FinancesSheet({super.key, required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  bool get _wide =>
      st.sheet == 'import' || st.sheet == 'schedule' || st.sheet == 'rates';

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Stack(
      children: [
        // The scrim closes the sheet. A modal you cannot dismiss by clicking
        // away from it is a modal people force-quit the app to escape.
        Positioned.fill(
          child: GestureDetector(
            onTap: () => c.send(const FinancesCmd.closeSheet()),
            child: ColoredBox(color: Colors.black.withValues(alpha: 0.45)),
          ),
        ),
        Center(
          child: ConstrainedBox(
            constraints: BoxConstraints(
              maxWidth: _wide ? 860 : 520,
              maxHeight: 680,
            ),
            child: Material(
              color: t.modal,
              borderRadius: BorderRadius.circular(Tokens.radiusLg),
              clipBehavior: Clip.antiAlias,
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  _Head(c: c, st: st),
                  Flexible(
                    child: SingleChildScrollView(
                      padding: const EdgeInsets.fromLTRB(20, 4, 20, 16),
                      child: _Body(c: c, st: st),
                    ),
                  ),
                  _Foot(c: c, st: st),
                ],
              ),
            ),
          ),
        ),
      ],
    );
  }
}

class _Head extends StatelessWidget {
  const _Head({required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.fromLTRB(20, 18, 12, 10),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(st.sheetTitle,
                    style: TextStyle(
                        fontSize: 17,
                        fontWeight: FontWeight.w800,
                        color: t.nInk)),
                if (st.sheetHint.isNotEmpty)
                  Padding(
                    padding: const EdgeInsets.only(top: 5),
                    child: Text(st.sheetHint,
                        style: TextStyle(
                            fontSize: 11.5, height: 1.35, color: t.nInk2)),
                  ),
              ],
            ),
          ),
          IconButton(
            iconSize: 20,
            tooltip: 'Close',
            onPressed: () => c.send(const FinancesCmd.closeSheet()),
            icon: const Icon(Icons.close),
          ),
        ],
      ),
    );
  }
}

class _Body extends StatelessWidget {
  const _Body({required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        if (st.sheet == 'import') ImportPanel(c: c, st: st),
        if (st.sheet == 'schedule') SchedulePanel(c: c, st: st),
        if (st.sheet == 'prices') PricesPanel(st: st),
        if (st.sheet == 'detect') ProposalPanel(c: c, st: st),
        for (final f in st.form)
          Padding(
            padding: const EdgeInsets.only(bottom: 12),
            child: SheetField(c: c, field: f),
          ),
        if (st.sheet == 'rates') RatesPanel(c: c, st: st),
        // An answer, not a complaint: what an envelope has left, what OCR read,
        // what a prepayment would save.
        if (st.sheetNote.isNotEmpty) _Note(text: st.sheetNote),
      ],
    );
  }
}

class _Note extends StatelessWidget {
  const _Note({required this.text});

  final String text;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      margin: const EdgeInsets.only(top: 4),
      padding: const EdgeInsets.all(11),
      decoration: BoxDecoration(
        color: Tokens.secFinances.withValues(alpha: 0.1),
        borderRadius: BorderRadius.circular(Tokens.radiusSm),
        border: Border.all(color: Tokens.secFinances.withValues(alpha: 0.35)),
      ),
      child: Text(text,
          style: TextStyle(fontSize: 11.5, height: 1.4, color: t.nInk)),
    );
  }
}

class _Foot extends StatelessWidget {
  const _Foot({required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // "Save & add another" only where entering several in a row is the actual
    // task. Nobody reconciles two accounts back to back.
    final canRepeat = const {'txn', 'one-off', 'due', 'sub', 'bill'}
        .contains(st.sheet);
    return Container(
      padding: const EdgeInsets.fromLTRB(20, 12, 20, 16),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          if (st.sheetError.isNotEmpty)
            Expanded(
              child: Row(
                children: [
                  const Icon(Icons.error_outline,
                      size: 15, color: Tokens.error),
                  const SizedBox(width: 6),
                  Expanded(
                    child: Text(st.sheetError,
                        style: const TextStyle(
                            fontSize: 11.5, color: Tokens.error)),
                  ),
                ],
              ),
            )
          else
            const Spacer(),
          if (st.sheetBusy)
            const Padding(
              padding: EdgeInsets.only(right: 12),
              child: SizedBox(
                width: 15,
                height: 15,
                child: CircularProgressIndicator(strokeWidth: 2),
              ),
            ),
          TextButton(
            onPressed: () => c.send(const FinancesCmd.closeSheet()),
            child: const Text('Close'),
          ),
          if (st.sheetPrimary.isNotEmpty) ...[
            if (canRepeat) ...[
              const SizedBox(width: 6),
              OutlinedButton(
                onPressed: st.sheetBusy
                    ? null
                    : () => c.send(const FinancesCmd.submitAgain()),
                child: const Text('Save & add another'),
              ),
            ],
            const SizedBox(width: 8),
            FilledButton(
              style: FilledButton.styleFrom(
                backgroundColor: Tokens.secFinances,
                // Lime is a light hue; white on it fails contrast, so the ink
                // on this one button is dark. Same value as `on-accent` in
                // tokens.slint.
                foregroundColor: const Color(0xFF1A2E05),
              ),
              onPressed: st.sheetBusy
                  ? null
                  : () => c.send(const FinancesCmd.submitSheet()),
              child: Text(st.sheetPrimary),
            ),
          ],
        ],
      ),
    );
  }
}

// ------------------------------------------------------------------ fields ---

/// One field of a form, by its kind. Seven kinds cover every sheet in the
/// section, which is what lets sixteen different forms share one renderer.
class SheetField extends StatefulWidget {
  const SheetField({super.key, required this.c, required this.field});

  final FinancesController c;
  final FinField field;

  @override
  State<SheetField> createState() => _SheetFieldState();
}

class _SheetFieldState extends State<SheetField> {
  late final TextEditingController _text =
      TextEditingController(text: widget.field.value);

  @override
  void didUpdateWidget(SheetField old) {
    super.didUpdateWidget(old);
    // Only when Rust changed it under us — a prefill, a receipt scan, a preset.
    // Writing on every rebuild would take the caret out of whatever is being
    // typed.
    if (widget.field.value != old.field.value &&
        widget.field.value != _text.text) {
      _text.text = widget.field.value;
    }
  }

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  void _set(String v) =>
      widget.c.send(FinancesCmd.setField(key: widget.field.key, value: v));

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final f = widget.field;

    if (f.kind == 'note') {
      return Container(
        padding: const EdgeInsets.all(11),
        decoration: BoxDecoration(
          color: t.nTile,
          borderRadius: BorderRadius.circular(Tokens.radiusSm),
        ),
        child: Text(f.hint.isEmpty ? f.label : f.hint,
            style: TextStyle(fontSize: 11.5, height: 1.4, color: t.nInk2)),
      );
    }

    if (f.kind == 'static') {
      return Row(
        children: [
          Expanded(
            child: Text(f.label,
                style: TextStyle(fontSize: 12, color: t.nInk2)),
          ),
          Text(f.value,
              style: TextStyle(
                  fontSize: 13, fontWeight: FontWeight.w600, color: t.nInk)),
        ],
      );
    }

    if (f.kind == 'toggle') {
      return SwitchListTile(
        contentPadding: EdgeInsets.zero,
        dense: true,
        activeThumbColor: Tokens.secFinances,
        title: Text(f.label, style: TextStyle(fontSize: 13, color: t.nInk)),
        subtitle: f.hint.isEmpty
            ? null
            : Text(f.hint, style: TextStyle(fontSize: 11, color: t.nInk2)),
        value: f.value == 'true' || f.value == '1',
        onChanged: (v) => _set(v ? 'true' : 'false'),
      );
    }

    if (f.kind == 'dropdown') {
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          _Label(label: f.label, required_: f.required_),
          const SizedBox(height: 5),
          LabelPicker(
            value: f.value,
            options: f.options,
            width: double.infinity,
            onChanged: _set,
          ),
          if (f.hint.isNotEmpty) _Hint(text: f.hint),
        ],
      );
    }

    // text | number | money | date all render as a box; only the keyboard and
    // the affordance beside them differ.
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        _Label(label: f.label, required_: f.required_),
        const SizedBox(height: 5),
        TextField(
          controller: _text,
          keyboardType: f.kind == 'number' || f.kind == 'money'
              ? const TextInputType.numberWithOptions(decimal: true, signed: true)
              : TextInputType.text,
          style: TextStyle(fontSize: 13, color: t.nInk),
          decoration: InputDecoration(
            isDense: true,
            border: const OutlineInputBorder(),
            hintText: f.kind == 'date' ? 'YYYY-MM-DD' : null,
            suffixIcon: f.kind == 'date'
                ? IconButton(
                    iconSize: 17,
                    tooltip: 'Pick a date',
                    icon: const Icon(Icons.calendar_today_outlined),
                    onPressed: () => _pickDate(context),
                  )
                : null,
          ),
          // On submit, not on every keystroke: each change is a round trip that
          // rebuilds the whole snapshot, and doing that per character would
          // make the box feel like it was fighting back.
          onEditingComplete: () => _set(_text.text),
          onSubmitted: _set,
          onTapOutside: (_) => _set(_text.text),
        ),
        if (f.hint.isNotEmpty) _Hint(text: f.hint),
      ],
    );
  }

  Future<void> _pickDate(BuildContext context) async {
    final now = DateTime.now();
    final seed = DateTime.tryParse(_text.text) ?? now;
    final picked = await showDatePicker(
      context: context,
      initialDate: seed,
      firstDate: DateTime(now.year - 12),
      lastDate: DateTime(now.year + 12),
    );
    if (picked == null) return;
    final iso = '${picked.year.toString().padLeft(4, '0')}-'
        '${picked.month.toString().padLeft(2, '0')}-'
        '${picked.day.toString().padLeft(2, '0')}';
    _text.text = iso;
    _set(iso);
  }
}

class _Label extends StatelessWidget {
  const _Label({required this.label, required this.required_});

  final String label;
  final bool required_;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      children: [
        Text(label,
            style: TextStyle(
                fontSize: 11.5, fontWeight: FontWeight.w600, color: t.nInk2)),
        if (required_)
          const Padding(
            padding: EdgeInsets.only(left: 3),
            child: Text('*',
                style: TextStyle(fontSize: 11.5, color: Tokens.error)),
          ),
      ],
    );
  }
}

class _Hint extends StatelessWidget {
  const _Hint({required this.text});

  final String text;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(top: 4),
      child: Text(text,
          style: TextStyle(fontSize: 10.5, height: 1.3, color: t.nInk3)),
    );
  }
}

// ------------------------------------------------------- the five extras -----

/// The exchange-rate table. Read-only above the two fields that add or correct
/// one: a rate the section fetched and a rate the user typed are the same row,
/// and `edited` is what says which it is.
class RatesPanel extends StatelessWidget {
  const RatesPanel({super.key, required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        Row(
          children: [
            Text('Known rates',
                style: TextStyle(
                    fontSize: 12, fontWeight: FontWeight.w700, color: t.nInk)),
            const Spacer(),
            TextButton.icon(
              onPressed: st.ratesBusy
                  ? null
                  : () => c.send(const FinancesCmd.ratesRefresh()),
              icon: st.ratesBusy
                  ? const SizedBox(
                      width: 13,
                      height: 13,
                      child: CircularProgressIndicator(strokeWidth: 2))
                  : const Icon(Icons.refresh, size: 16),
              label: const Text('Fetch now', style: TextStyle(fontSize: 12)),
            ),
          ],
        ),
        const SizedBox(height: 6),
        if (st.rates.isEmpty)
          const EmptyNote(
            icon: Icons.currency_exchange,
            title: 'Nothing here is in another currency yet.',
            detail: 'Rates appear once something is recorded in one.',
          )
        else
          for (final r in st.rates)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 4),
              child: Row(
                children: [
                  SizedBox(
                    width: 54,
                    child: Text(r.code,
                        style: TextStyle(
                            fontSize: 12.5,
                            fontWeight: FontWeight.w700,
                            color: t.nInk)),
                  ),
                  Expanded(
                    child: Text(r.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11.5, color: t.nInk2)),
                  ),
                  if (!r.known)
                    const Text('not set',
                        style: TextStyle(fontSize: 11, color: Tokens.warn))
                  else ...[
                    Text(r.rate,
                        style: TextStyle(
                            fontSize: 12,
                            fontWeight: FontWeight.w600,
                            color: t.nInk)),
                    const SizedBox(width: 10),
                    SizedBox(
                      width: 96,
                      child: Text(r.inverse,
                          textAlign: TextAlign.right,
                          style: TextStyle(fontSize: 11, color: t.nInk3)),
                    ),
                  ],
                  const SizedBox(width: 8),
                  Tooltip(
                    message: r.live ? 'Fetched' : 'Set by hand — ${r.edited}',
                    child: Icon(r.live ? Icons.cloud_done_outlined : Icons.edit,
                        size: 13, color: t.nInk3),
                  ),
                ],
              ),
            ),
        const SizedBox(height: 14),
      ],
    );
  }
}

/// What a recurrence has cost, every time it changed. The only reason a quiet
/// price rise is visible at all.
class PricesPanel extends StatelessWidget {
  const PricesPanel({super.key, required this.st});

  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (st.prices.isEmpty) {
      return const EmptyNote(
        icon: Icons.history,
        title: 'One price so far.',
        detail: 'A second appears the first time the amount changes.',
      );
    }
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final p in st.prices)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 5),
            child: Row(
              children: [
                Container(
                  width: 7,
                  height: 7,
                  decoration: const BoxDecoration(
                      color: Tokens.secFinances, shape: BoxShape.circle),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: Text(p.from,
                      style: TextStyle(fontSize: 12, color: t.nInk2)),
                ),
                Text(p.amount,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
              ],
            ),
          ),
      ],
    );
  }
}

/// A loan's amortisation, ten rows at a time. Paging never goes back to the
/// database: the whole schedule is held, and a page is a slice of it.
class SchedulePanel extends StatelessWidget {
  const SchedulePanel({super.key, required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    TextStyle head() => TextStyle(
        fontSize: 10.5, fontWeight: FontWeight.w700, color: t.nInk2);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        Row(
          children: [
            SizedBox(width: 36, child: Text('#', style: head())),
            SizedBox(width: 96, child: Text('Due', style: head())),
            Expanded(child: Text('EMI', textAlign: TextAlign.right, style: head())),
            Expanded(
                child: Text('Principal',
                    textAlign: TextAlign.right, style: head())),
            Expanded(
                child:
                    Text('Interest', textAlign: TextAlign.right, style: head())),
            Expanded(
                child:
                    Text('Balance', textAlign: TextAlign.right, style: head())),
          ],
        ),
        Divider(height: 12, color: t.nHair),
        for (final r in st.schedule)
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 4),
            child: Row(
              children: [
                SizedBox(
                    width: 36,
                    child: Text('${r.n}',
                        style: TextStyle(fontSize: 11.5, color: t.nInk3))),
                SizedBox(
                    width: 96,
                    child: Text(r.due,
                        style: TextStyle(fontSize: 11.5, color: t.nInk2))),
                Expanded(
                    child: Text(r.emi,
                        textAlign: TextAlign.right,
                        style: TextStyle(fontSize: 11.5, color: t.nInk))),
                Expanded(
                    child: Text(r.principal,
                        textAlign: TextAlign.right,
                        style: const TextStyle(
                            fontSize: 11.5, color: Tokens.ok))),
                Expanded(
                    child: Text(r.interest,
                        textAlign: TextAlign.right,
                        style: TextStyle(
                            fontSize: 11.5,
                            color: r.interestPct > 60
                                ? Tokens.error
                                : t.nInk2))),
                Expanded(
                    child: Text(r.balance,
                        textAlign: TextAlign.right,
                        style: TextStyle(
                            fontSize: 11.5,
                            fontWeight: FontWeight.w600,
                            color: t.nInk))),
              ],
            ),
          ),
        const SizedBox(height: 8),
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            IconButton(
              iconSize: 17,
              onPressed: st.schedulePage <= 1
                  ? null
                  : () => c.send(const FinancesCmd.scheduleStep(delta: -1)),
              icon: const Icon(Icons.chevron_left),
            ),
            Text('${st.schedulePage} / ${st.schedulePages}',
                style: TextStyle(fontSize: 11.5, color: t.nInk2)),
            IconButton(
              iconSize: 17,
              onPressed: st.schedulePage >= st.schedulePages
                  ? null
                  : () => c.send(const FinancesCmd.scheduleStep(delta: 1)),
              icon: const Icon(Icons.chevron_right),
            ),
          ],
        ),
      ],
    );
  }
}

/// A parsed statement, waiting to be confirmed. The preview is the whole point
/// of the column mapping above it, so correcting a column re-reads the file at
/// once rather than on Import — a mapping confirmed blind is a ledger to undo.
class ImportPanel extends StatelessWidget {
  const ImportPanel({super.key, required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        if (st.importSummary.isNotEmpty)
          Container(
            padding: const EdgeInsets.all(11),
            margin: const EdgeInsets.only(bottom: 12),
            decoration: BoxDecoration(
              color: st.importNeedsFormat
                  ? Tokens.warn.withValues(alpha: 0.12)
                  : t.nTile,
              borderRadius: BorderRadius.circular(Tokens.radiusSm),
              border: st.importNeedsFormat
                  ? Border.all(color: Tokens.warn.withValues(alpha: 0.5))
                  : null,
            ),
            child: Row(
              children: [
                if (st.importNeedsFormat) ...[
                  const Icon(Icons.warning_amber_outlined,
                      size: 15, color: Tokens.warn),
                  const SizedBox(width: 8),
                ],
                Expanded(
                  child: Text(st.importSummary,
                      style: TextStyle(
                          fontSize: 11.5, height: 1.35, color: t.nInk)),
                ),
              ],
            ),
          ),
        if (st.importRows.isEmpty)
          OutlinedButton.icon(
            onPressed: () => c.send(const FinancesCmd.importPick()),
            icon: const Icon(Icons.file_open_outlined, size: 17),
            label: const Text('Choose a statement'),
          )
        else ...[
          // The column mapping. Its own list, separate from `form`, because
          // changing one of these re-reads the file rather than being held for
          // submit.
          for (final f in st.importMap)
            Padding(
              padding: const EdgeInsets.only(bottom: 10),
              child: SheetField(c: c, field: f),
            ),
          const SizedBox(height: 4),
          Text('First ${st.importRows.length} rows, as they will be read',
              style: TextStyle(
                  fontSize: 11, fontWeight: FontWeight.w600, color: t.nInk2)),
          const SizedBox(height: 6),
          Container(
            constraints: const BoxConstraints(maxHeight: 240),
            decoration: BoxDecoration(
              color: t.nTile,
              borderRadius: BorderRadius.circular(Tokens.radiusSm),
            ),
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
            child: ListView.builder(
              shrinkWrap: true,
              itemCount: st.importRows.length,
              itemBuilder: (context, i) {
                final r = st.importRows[i];
                return Padding(
                  padding: const EdgeInsets.symmetric(vertical: 3),
                  child: Row(
                    children: [
                      SizedBox(
                        width: 90,
                        child: Text(r.date,
                            style:
                                TextStyle(fontSize: 11, color: t.nInk2)),
                      ),
                      Expanded(
                        child: Text(r.description,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(fontSize: 11, color: t.nInk)),
                      ),
                      Text(r.amount,
                          style: TextStyle(
                              fontSize: 11.5,
                              fontWeight: FontWeight.w600,
                              color: r.credit ? Tokens.ok : t.nInk)),
                    ],
                  ),
                );
              },
            ),
          ),
          const SizedBox(height: 10),
          Align(
            alignment: Alignment.centerLeft,
            child: TextButton.icon(
              onPressed: () => c.send(const FinancesCmd.importPick()),
              icon: const Icon(Icons.swap_horiz, size: 16),
              label: const Text('Choose a different file',
                  style: TextStyle(fontSize: 12)),
            ),
          ),
        ],
        const SizedBox(height: 8),
      ],
    );
  }
}

/// Charges that look recurring but have no recurrence behind them. Nothing here
/// exists in the database yet, which is why a proposal is keyed by position
/// rather than by an id.
class ProposalPanel extends StatelessWidget {
  const ProposalPanel({super.key, required this.c, required this.st});

  final FinancesController c;
  final FinancesState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (st.proposals.isEmpty) {
      return const EmptyNote(
        icon: Icons.auto_awesome_outlined,
        title: 'Nothing in the ledger looks like a missed subscription.',
        detail: 'Three or more charges of about the same size, about a month '
            'apart, is what this looks for.',
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      mainAxisSize: MainAxisSize.min,
      children: [
        for (final p in st.proposals)
          Container(
            margin: const EdgeInsets.only(bottom: 8),
            padding: const EdgeInsets.fromLTRB(12, 10, 10, 10),
            decoration: BoxDecoration(
              color: t.nTile,
              borderRadius: BorderRadius.circular(Tokens.radiusSm),
            ),
            child: Row(
              children: [
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(p.name,
                          style: TextStyle(
                              fontSize: 13,
                              fontWeight: FontWeight.w700,
                              color: t.nInk)),
                      const SizedBox(height: 2),
                      Text(
                          '${p.amount} · ${p.cycle} · next ${p.nextDue} · '
                          'seen ${p.occurrences}×  (${p.confidence})',
                          style: TextStyle(fontSize: 11, color: t.nInk2)),
                    ],
                  ),
                ),
                FilledButton.tonal(
                  onPressed: () =>
                      c.send(FinancesCmd.acceptProposal(idx: p.idx)),
                  child: const Text('Add it'),
                ),
              ],
            ),
          ),
      ],
    );
  }
}
