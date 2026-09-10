// Authoring the rules a smart playlist already runs on.
//
// `SmartRule` and `evaluate()` have been implemented in `playlists.rs` since
// the section shipped, and two hard-coded presets were the only way to get one.
// This is the missing half: a stacked condition builder over the same engine,
// with a live count that comes from the same `evaluate` the playlist itself
// calls — so the number under the editor is the number that ends up in it.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';

/// How long to wait after an edit before re-counting. Every keystroke in a
/// value box would otherwise be a query over the whole library.
const Duration _kSettle = Duration(milliseconds: 260);

/// Open the rule editor. `playlistId` 0 creates a new playlist.
///
/// Returns the playlist's id when something was saved, so a caller that just
/// created one can open it.
Future<int?> editSmartPlaylist(
  BuildContext context,
  MusicController c, {
  int playlistId = 0,
  String name = '',
}) =>
    showDialog<int>(
      context: context,
      builder: (ctx) => _SmartEditor(
        controller: c,
        playlistId: playlistId,
        initialName: name,
      ),
    );

class _SmartEditor extends StatefulWidget {
  const _SmartEditor({
    required this.controller,
    required this.playlistId,
    required this.initialName,
  });

  final MusicController controller;
  final int playlistId;
  final String initialName;

  @override
  State<_SmartEditor> createState() => _SmartEditorState();
}

class _SmartEditorState extends State<_SmartEditor> {
  /// The fields and operators the engine actually supports, read from it
  /// rather than written out again here — a field added in Rust appears in
  /// this dropdown instead of quietly never being offerable.
  final SmartSchema _schema = musicSmartSchema();

  late final TextEditingController _name =
      TextEditingController(text: widget.initialName);

  String _combine = 'all';
  List<SmartCondition> _conditions = [];
  int _limit = 0;

  int? _count;
  bool _counting = false;
  Timer? _settle;
  String? _error;
  bool _loading = true;

  @override
  void initState() {
    super.initState();
    unawaited(_load());
  }

  @override
  void dispose() {
    _settle?.cancel();
    _name.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    SmartRuleView? rule;
    if (widget.playlistId > 0) {
      try {
        rule = await musicSmartLoad(playlistId: widget.playlistId);
      } catch (_) {
        rule = null;
      }
    }
    if (!mounted) return;
    setState(() {
      _combine = rule?.combine ?? 'all';
      _conditions = List<SmartCondition>.from(rule?.conditions ?? const []);
      _limit = rule?.limit ?? 0;
      _loading = false;
      // A new playlist opens with one empty condition rather than a blank
      // panel: an editor with nothing in it does not show what it is for.
      if (_conditions.isEmpty) _conditions.add(_blank());
    });
    _recount();
  }

  SmartCondition _blank() => SmartCondition(
        field: _schema.fields.first.id,
        op: _schema.ops.first.id,
        value: '',
      );

  SmartRuleView get _rule => SmartRuleView(
        combine: _combine,
        // A condition with no value matches nothing useful and would make the
        // live count a lie; it stays on screen but not in the rule.
        conditions:
            _conditions.where((c) => c.value.trim().isNotEmpty).toList(),
        limit: _limit,
      );

  void _touch() {
    _settle?.cancel();
    setState(() => _counting = true);
    _settle = Timer(_kSettle, _recount);
  }

  Future<void> _recount() async {
    final asked = _rule;
    int? n;
    try {
      n = await musicSmartPreview(rule: asked);
    } catch (_) {
      n = null;
    }
    if (!mounted) return;
    setState(() {
      _count = n;
      _counting = false;
    });
  }

  String _kindOf(String fieldId) => _schema.fields
      .firstWhere(
        (f) => f.id == fieldId,
        orElse: () => _schema.fields.first,
      )
      .kind;

  Future<void> _save() async {
    setState(() => _error = null);
    try {
      final id = await musicSmartSave(
        playlistId: widget.playlistId,
        name: _name.text,
        rule: _rule,
      );
      if (mounted) Navigator.of(context).pop(id);
    } catch (e) {
      if (mounted) setState(() => _error = '$e');
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final size = MediaQuery.sizeOf(context);
    return AlertDialog(
      title: Text(widget.playlistId > 0 ? 'Rules' : 'Smart playlist'),
      content: SizedBox(
        width: 600,
        height: size.height * 0.72,
        child: _loading
            ? const Center(child: CircularProgressIndicator())
            : Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  TextField(
                    controller: _name,
                    decoration: const InputDecoration(
                      isDense: true,
                      labelText: 'Name',
                      border: OutlineInputBorder(),
                    ),
                  ),
                  const SizedBox(height: 14),
                  Row(
                    children: [
                      Text('Match', style: TextStyle(color: t.nInk2)),
                      const SizedBox(width: 10),
                      for (final (id, label) in const [
                        ('all', 'all of these'),
                        ('any', 'any of these'),
                      ])
                        Padding(
                          padding: const EdgeInsets.only(right: 8),
                          child: ChoiceChip(
                            label: Text(label),
                            selected: _combine == id,
                            onSelected: (_) {
                              setState(() => _combine = id);
                              _touch();
                            },
                          ),
                        ),
                    ],
                  ),
                  const SizedBox(height: 10),
                  Expanded(
                    child: ListView.separated(
                      padding: EdgeInsets.zero,
                      itemCount: _conditions.length,
                      separatorBuilder: (_, __) => const SizedBox(height: 8),
                      itemBuilder: (_, i) => _ConditionRow(
                        schema: _schema,
                        condition: _conditions[i],
                        kind: _kindOf(_conditions[i].field),
                        // The last condition cannot be removed: a rule with
                        // none of them is every track in the library, which is
                        // never what anyone meant to build.
                        onRemove: _conditions.length > 1
                            ? () {
                                setState(() => _conditions.removeAt(i));
                                _touch();
                              }
                            : null,
                        onChanged: (c) {
                          setState(() => _conditions[i] = c);
                          _touch();
                        },
                      ),
                    ),
                  ),
                  const SizedBox(height: 8),
                  Row(
                    children: [
                      TextButton.icon(
                        onPressed: () =>
                            setState(() => _conditions.add(_blank())),
                        icon: const Icon(Icons.add, size: 18),
                        label: const Text('Add a condition'),
                      ),
                      const Spacer(),
                      _MatchCount(count: _count, busy: _counting),
                    ],
                  ),
                  Row(
                    children: [
                      Text('Cap at', style: TextStyle(color: t.nInk2)),
                      const SizedBox(width: 10),
                      for (final n in const [0, 25, 50, 100, 500])
                        Padding(
                          padding: const EdgeInsets.only(right: 6),
                          child: ChoiceChip(
                            label: Text(n == 0 ? 'no cap' : '$n'),
                            selected: _limit == n,
                            onSelected: (_) {
                              setState(() => _limit = n);
                              _touch();
                            },
                          ),
                        ),
                    ],
                  ),
                  if (_error != null)
                    Padding(
                      padding: const EdgeInsets.only(top: 8),
                      child: Text(
                        _error!,
                        style: const TextStyle(
                            fontSize: 12, color: Color(0xFFDC2626)),
                      ),
                    ),
                ],
              ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
        FilledButton(
          onPressed: _loading ? null : _save,
          child: const Text('Save'),
        ),
      ],
    );
  }
}

/// The live count. Its whole job is to make the rule legible before it is
/// saved — a builder that only tells you what it matched after you committed
/// to it is a form, not an editor.
class _MatchCount extends StatelessWidget {
  const _MatchCount({required this.count, required this.busy});

  final int? count;
  final bool busy;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final label = switch ((busy, count)) {
      (true, _) => 'counting…',
      (false, null) => '—',
      (false, final n) => n == 1 ? '1 match' : '$n matches',
    };
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
      decoration: BoxDecoration(
        color: Tokens.secMusic.withValues(alpha: 0.14),
        borderRadius: BorderRadius.circular(20),
      ),
      child: Text(
        label,
        style: TextStyle(
          fontFamily: Tokens.fontFamily,
          fontSize: 12,
          fontWeight: FontWeight.w700,
          color: busy ? t.nInk3 : Tokens.secMusic,
        ),
      ),
    );
  }
}

class _ConditionRow extends StatelessWidget {
  const _ConditionRow({
    required this.schema,
    required this.condition,
    required this.kind,
    required this.onChanged,
    required this.onRemove,
  });

  final SmartSchema schema;
  final SmartCondition condition;
  final String kind;
  final ValueChanged<SmartCondition> onChanged;
  final VoidCallback? onRemove;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: t.nHair),
      ),
      child: Row(
        children: [
          Expanded(
            flex: 4,
            child: DropdownButton<String>(
              value: condition.field,
              isExpanded: true,
              underline: const SizedBox.shrink(),
              items: [
                for (final f in schema.fields)
                  DropdownMenuItem(value: f.id, child: Text(f.label)),
              ],
              onChanged: (v) => v == null
                  ? null
                  // The value is cleared on a field change: "Ambient" is not a
                  // sensible tempo, and carrying it over reads as a bug.
                  : onChanged(SmartCondition(
                      field: v, op: condition.op, value: '')),
            ),
          ),
          const SizedBox(width: 8),
          Expanded(
            flex: 3,
            child: DropdownButton<String>(
              value: condition.op,
              isExpanded: true,
              underline: const SizedBox.shrink(),
              items: [
                for (final o in schema.ops)
                  DropdownMenuItem(value: o.id, child: Text(o.label)),
              ],
              onChanged: (v) => v == null
                  ? null
                  : onChanged(SmartCondition(
                      field: condition.field, op: v, value: condition.value)),
            ),
          ),
          const SizedBox(width: 8),
          Expanded(flex: 4, child: _value(context)),
          IconButton(
            tooltip: onRemove == null
                ? 'A rule needs at least one condition'
                : 'Remove',
            iconSize: 18,
            visualDensity: VisualDensity.compact,
            icon: const Icon(Icons.close),
            onPressed: onRemove,
          ),
        ],
      ),
    );
  }

  Widget _value(BuildContext context) {
    void set(String v) => onChanged(SmartCondition(
        field: condition.field, op: condition.op, value: v));

    // The right input for the field, which is the whole reason the schema
    // carries a kind: a text box for "Loved" would take "yes", "true" and "1"
    // and only one of them would work.
    switch (kind) {
      case 'bool':
        return Row(
          children: [
            Switch(
              value: condition.value == '1',
              onChanged: (on) => set(on ? '1' : '0'),
            ),
            Text(condition.value == '1' ? 'yes' : 'no',
                style: TextStyle(color: context.tokens.nInk2)),
          ],
        );
      case 'number':
      case 'days':
        return TextFormField(
          initialValue: condition.value,
          keyboardType: TextInputType.number,
          decoration: InputDecoration(
            isDense: true,
            border: const OutlineInputBorder(),
            hintText: kind == 'days' ? 'days' : 'number',
          ),
          onChanged: set,
        );
      default:
        return TextFormField(
          initialValue: condition.value,
          decoration: const InputDecoration(
            isDense: true,
            border: OutlineInputBorder(),
            hintText: 'value',
          ),
          onChanged: set,
        );
    }
  }
}
