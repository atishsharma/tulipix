// The form one operation asks for, and the two panels that show what came back.
//
// The field schema comes from the bridge, so this file renders eight kinds of
// row and knows nothing about any particular tool.

import 'package:flutter/material.dart';

import '../../design/pick.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/tools.dart';
import 'tools_controller.dart';

class ToolForm extends StatelessWidget {
  const ToolForm({super.key, required this.controller, required this.state});

  final ToolsController controller;
  final ToolsState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 18, 24, 32),
      children: [
        Row(
          children: [
            TextButton.icon(
              icon: const Icon(Icons.arrow_back, size: 18),
              label: const Text('All tools'),
              onPressed: () => controller.send(const ToolsCmd.closeTool()),
            ),
            const Spacer(),
            TextButton(
              onPressed: () => controller.send(const ToolsCmd.reset()),
              child: const Text('Reset'),
            ),
            const SizedBox(width: 8),
            FilledButton.icon(
              style: FilledButton.styleFrom(backgroundColor: Tokens.secTools),
              icon: const Icon(Icons.play_arrow, size: 18),
              label: const Text('Run'),
              onPressed: controller.busy
                  ? null
                  : () => controller.send(const ToolsCmd.run()),
            ),
          ],
        ),
        const SizedBox(height: 12),
        Text(state.activeLabel,
            style: TextStyle(
                fontSize: 24, fontWeight: FontWeight.w800, color: t.nInk)),
        if (state.activeInfo.isNotEmpty) ...[
          const SizedBox(height: 6),
          Text(state.activeInfo,
              style: TextStyle(fontSize: 13, height: 1.5, color: t.nInk2)),
        ],
        if (state.error.isNotEmpty) ...[
          const SizedBox(height: 14),
          Container(
            padding: const EdgeInsets.all(12),
            decoration: BoxDecoration(
              color: Tokens.error.withValues(alpha: 0.12),
              borderRadius: BorderRadius.circular(8),
            ),
            child: Text(state.error,
                style: const TextStyle(fontSize: 12, color: Tokens.error)),
          ),
        ],
        const SizedBox(height: 20),
        if (state.fields.isEmpty)
          Text('This tool takes no options — press Run.',
              style: TextStyle(fontSize: 13, color: t.nInk2))
        else
          for (final f in state.fields)
            _FieldRow(controller: controller, field: f),
      ],
    );
  }
}

class _FieldRow extends StatelessWidget {
  const _FieldRow({required this.controller, required this.field});

  final ToolsController controller;
  final Field field;

  void _set(String v) =>
      controller.send(ToolsCmd.setField(key: field.key, value: v));

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(bottom: 18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Text(field.label,
                  style: TextStyle(
                      fontSize: 12.5,
                      fontWeight: FontWeight.w600,
                      color: t.nInk)),
              if (field.required_)
                const Text(' *', style: TextStyle(color: Tokens.error)),
            ],
          ),
          const SizedBox(height: 6),
          _control(context),
          if (field.hint.isNotEmpty) ...[
            const SizedBox(height: 4),
            Text(field.hint, style: TextStyle(fontSize: 11, color: t.nInk3)),
          ],
        ],
      ),
    );
  }

  Widget _control(BuildContext context) {
    switch (field.kind) {
      case 'toggle':
        return Switch(
          value: field.value == 'true',
          activeThumbColor: Tokens.secTools,
          onChanged: (v) => _set(v ? 'true' : 'false'),
        );
      case 'dropdown':
        return DropdownButtonFormField<String>(
          initialValue: field.options.contains(field.value)
              ? field.value
              : (field.options.isEmpty ? null : field.options.first),
          decoration: const InputDecoration(
              isDense: true, border: OutlineInputBorder()),
          items: [
            for (final o in field.options)
              DropdownMenuItem(value: o, child: Text(o)),
          ],
          onChanged: (v) => v == null ? null : _set(v),
        );
      case 'slider':
        final value = double.tryParse(field.value) ?? field.min;
        return Row(
          children: [
            Expanded(
              child: Slider(
                value: value.clamp(field.min, field.max),
                min: field.min,
                max: field.max,
                divisions: (field.max - field.min).round().clamp(1, 400),
                activeColor: Tokens.secTools,
                label: value.round().toString(),
                onChanged: (v) => _set(v.round().toString()),
              ),
            ),
            SizedBox(
              width: 48,
              child: Text(value.round().toString(),
                  textAlign: TextAlign.right,
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w600,
                      color: context.tokens.nInk)),
            ),
          ],
        );
      case 'files':
        // Still one path per line, and still editable — a tool run over forty
        // files is usually a paste, not forty clicks — but the chooser fills it
        // and appends rather than replacing, so a second pick adds to the list.
        return _Text(
          value: field.value,
          hint: '/path/to/one\n/path/to/another',
          maxLines: 5,
          onChanged: _set,
          onBrowse: () async {
            final picked = await pickFiles();
            if (picked.isEmpty) return;
            final existing = field.value.trim();
            _set(existing.isEmpty
                ? picked.join('\n')
                : '$existing\n${picked.join('\n')}');
          },
        );
      default:
        return _Text(
          value: field.value,
          hint: switch (field.kind) {
            'file' => '/path/to/file',
            'folder' => '/path/to/folder',
            'number' => '0',
            _ => '',
          },
          maxLines: 1,
          onChanged: _set,
          onBrowse: switch (field.kind) {
            'file' => () async {
                final p = await pickFile(label: 'Any file');
                if (p != null) _set(p);
              },
            'folder' => () async {
                final p = await pickDirectory();
                if (p != null) _set(p);
              },
            _ => null,
          },
        );
    }
  }
}

/// A text field that does not rebuild itself out from under the cursor: it owns
/// its controller and only pushes on change.
class _Text extends StatefulWidget {
  const _Text({
    required this.value,
    required this.hint,
    required this.maxLines,
    required this.onChanged,
    this.onBrowse,
  });

  final String value;
  final String hint;
  final int maxLines;
  final ValueChanged<String> onChanged;

  /// Null for the fields that are not paths — a bitrate has nothing to browse.
  final Future<void> Function()? onBrowse;

  @override
  State<_Text> createState() => _TextState();
}

class _TextState extends State<_Text> {
  late final TextEditingController _c =
      TextEditingController(text: widget.value);

  @override
  void didUpdateWidget(_Text old) {
    super.didUpdateWidget(old);
    // Only when the value changed elsewhere — Reset, or a new tool — or the
    // caret jumps to the end on every keystroke.
    if (widget.value != _c.text && widget.value != old.value) {
      _c.text = widget.value;
    }
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => TextField(
        controller: _c,
        maxLines: widget.maxLines,
        decoration: InputDecoration(
          isDense: true,
          hintText: widget.hint,
          border: const OutlineInputBorder(),
          suffixIcon: widget.onBrowse == null
              ? null
              : IconButton(
                  tooltip: 'Choose',
                  icon: const Icon(Icons.more_horiz, size: 18),
                  onPressed: widget.onBrowse,
                ),
        ),
        onChanged: widget.onChanged,
      );
}

/// The rich result of a job that produced a report rather than a file.
class ResultPanel extends StatelessWidget {
  const ResultPanel({
    super.key,
    required this.controller,
    required this.state,
  });

  final ToolsController controller;
  final ToolsState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(20, 14, 12, 8),
          child: Row(
            children: [
              Text(state.resultTitle,
                  style: TextStyle(
                      fontSize: 18,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
              const Spacer(),
              IconButton(
                icon: const Icon(Icons.close),
                onPressed: () => controller.send(const ToolsCmd.closeResult()),
              ),
            ],
          ),
        ),
        Expanded(child: _body(context)),
      ],
    );
  }

  Widget _body(BuildContext context) {
    final t = context.tokens;
    switch (state.resultKind) {
      case 'info':
        String? section;
        return ListView.builder(
          padding: const EdgeInsets.symmetric(horizontal: 20),
          itemCount: state.resultInfo.length,
          itemBuilder: (_, i) {
            final row = state.resultInfo[i];
            final head = row.section != section;
            section = row.section;
            return Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                if (head)
                  Padding(
                    padding: const EdgeInsets.only(top: 16, bottom: 6),
                    child: Text(row.section.toUpperCase(),
                        style: const TextStyle(
                            fontSize: 11,
                            letterSpacing: 1,
                            fontWeight: FontWeight.w700,
                            color: Tokens.secTools)),
                  ),
                Padding(
                  padding: const EdgeInsets.symmetric(vertical: 2),
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      SizedBox(
                        width: 190,
                        child: Text(row.key,
                            style: TextStyle(fontSize: 12, color: t.nInk2)),
                      ),
                      Expanded(
                        child: Text(row.value,
                            style: TextStyle(fontSize: 12, color: t.nInk)),
                      ),
                    ],
                  ),
                ),
              ],
            );
          },
        );
      case 'diff':
        if (state.resultDiff.isEmpty) {
          return Center(
            child: Text('The folders match.',
                style: TextStyle(fontSize: 13, color: t.nInk2)),
          );
        }
        return ListView.builder(
          padding: const EdgeInsets.symmetric(horizontal: 20),
          itemCount: state.resultDiff.length,
          itemBuilder: (_, i) {
            final d = state.resultDiff[i];
            final colour = switch (d.kind) {
              'only-a' => const Color(0xFF3A86FF),
              'only-b' => const Color(0xFFB5179E),
              'differs' => const Color(0xFFF5A623),
              _ => t.nInk3,
            };
            return ListTile(
              dense: true,
              leading: Container(
                width: 8,
                height: 8,
                decoration:
                    BoxDecoration(color: colour, shape: BoxShape.circle),
              ),
              title: Text(d.path,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 12.5, color: t.nInk)),
              trailing:
                  Text(d.detail, style: TextStyle(fontSize: 11, color: colour)),
            );
          },
        );
      default:
        return SingleChildScrollView(
          padding: const EdgeInsets.symmetric(horizontal: 20),
          child: SelectableText(
            state.resultText,
            style:
                TextStyle(fontFamily: 'monospace', fontSize: 12, color: t.nInk),
          ),
        );
    }
  }
}
