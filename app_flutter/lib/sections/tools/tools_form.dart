// The form one operation asks for, and the two panels that show what came back.
//
// The field schema comes from the bridge, so this file renders nine kinds of
// row and knows nothing about any particular tool.

import 'dart:io' show FileSystemEntity, Platform;

import 'package:desktop_drop/desktop_drop.dart';
import 'package:flutter/material.dart';

import '../../platform/pick.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/tools.dart';
import 'tools_controller.dart';
import 'tools_preview.dart';

class ToolForm extends StatelessWidget {
  const ToolForm({super.key, required this.controller, required this.state});

  final ToolsController controller;
  final ToolsState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = activeTint(state);
    return ColoredBox(
      color: t.panel,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // Pinned: Run must not scroll away under a form with fifteen fields.
          Container(
            padding: const EdgeInsets.fromLTRB(8, 8, 12, 8),
            decoration: BoxDecoration(
              border: Border(bottom: BorderSide(color: t.nHair)),
            ),
            child: Row(
              children: [
                TextButton.icon(
                  icon: const Icon(Icons.arrow_back, size: 18),
                  label: const Text('All tools'),
                  onPressed: controller.closeTool,
                ),
                const Spacer(),
                TextButton(
                  onPressed: controller.reset,
                  child: const Text('Reset'),
                ),
                const SizedBox(width: 8),
                FilledButton.icon(
                  style:
                      FilledButton.styleFrom(backgroundColor: Tokens.secTools),
                  icon: const Icon(Icons.play_arrow, size: 18),
                  label: const Text('Run'),
                  onPressed: controller.busy ? null : controller.run,
                ),
              ],
            ),
          ),
          Expanded(
            child: ListView(
              padding: const EdgeInsets.fromLTRB(16, 16, 16, 30),
              children: [
                Row(
                  children: [
                    Container(
                      width: 38,
                      height: 38,
                      decoration: BoxDecoration(
                        color: tint.withValues(alpha: 0.16),
                        borderRadius: BorderRadius.circular(11),
                      ),
                      child: Icon(opIcon(state.activeOp, state.category),
                          size: 20, color: tint),
                    ),
                    const SizedBox(width: 11),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(state.activeLabel,
                              style: TextStyle(
                                  fontSize: 18,
                                  fontWeight: FontWeight.w800,
                                  color: t.nInk)),
                          Text(
                            state.category.toUpperCase(),
                            style: TextStyle(
                                fontSize: 9.5,
                                letterSpacing: 1,
                                fontWeight: FontWeight.w700,
                                color: tint),
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
                if (state.activeInfo.isNotEmpty) ...[
                  const SizedBox(height: 10),
                  Text(state.activeInfo,
                      style:
                          TextStyle(fontSize: 12, height: 1.6, color: t.nInk2)),
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
                        style:
                            const TextStyle(fontSize: 12, color: Tokens.error)),
                  ),
                ],
                const SizedBox(height: 20),
                if (state.fields.isEmpty)
                  Text('This tool takes no options — press Run.',
                      style: TextStyle(fontSize: 13, color: t.nInk2))
                else
                  for (final f in state.fields)
                    _FieldRow(
                      controller: controller,
                      field: f,
                      state: state,
                    ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _FieldRow extends StatelessWidget {
  const _FieldRow({
    required this.controller,
    required this.field,
    required this.state,
  });

  final ToolsController controller;
  final Field field;

  /// The rest of the form, for the one thing a field cannot know alone: what
  /// to suggest naming an output after.
  final ToolsState state;

  /// Every path in this form arrives through here, and so does every slider
  /// pixel. The controller debounces the preview behind it.
  void _set(String v) => controller.setField(field.key, v);

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

  /// What to call a file that does not exist yet.
  ///
  /// The source's name, the operation, and an extension picked in this order:
  /// what the field itself accepts, then what the form says it is converting
  /// to, then whatever the source was. "output.mp4" would be worse than all
  /// three, and an extensionless name leaves ffmpeg with no muxer.
  String _suggestedName() {
    String stem = 'output';
    String ext = field.ext.isEmpty ? '' : field.ext.first;
    String sourceExt = '';
    String target = '';
    for (final f in state.fields) {
      final v = f.value.trim();
      if (v.isEmpty) continue;
      if (f.key == 'target_ext' || f.key == 'format') {
        target = v;
      } else if (f.key == 'input' || f.key == 'inputs' || f.key == 'files') {
        final base = v.split('\n').first.split(Platform.pathSeparator).last;
        final dot = base.lastIndexOf('.');
        stem = dot > 0 ? base.substring(0, dot) : base;
        sourceExt = dot > 0 ? base.substring(dot + 1) : '';
      }
    }
    if (ext.isEmpty) ext = target.isNotEmpty ? target : sourceExt;
    final suffix = ext.isEmpty ? '' : '.$ext';
    return '$stem-${state.activeOp.replaceAll('_', '-')}$suffix';
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
      case 'file':
        return _PathRow(
          value: field.value,
          icon: Icons.insert_drive_file_outlined,
          placeholder: 'Choose a file…',
          wantsDirectory: false,
          onPicked: _set,
          onChoose: () => pickFile(
            label: _filterLabel(field.ext),
            extensions: field.ext,
          ),
        );
      case 'folder':
        return _PathRow(
          value: field.value,
          icon: Icons.folder_outlined,
          placeholder: 'Choose a folder…',
          wantsDirectory: true,
          onPicked: _set,
          onChoose: pickDirectory,
        );
      case 'save':
        return _PathRow(
          value: field.value,
          icon: Icons.save_outlined,
          placeholder:
              field.required_ ? 'Choose where to save…' : 'Beside the source',
          wantsDirectory: false,
          onPicked: _set,
          onChoose: () => pickSaveLocation(
            suggestedName: _suggestedName(),
            label: _filterLabel(field.ext),
            extensions: field.ext,
          ),
        );
      case 'files':
        return _FileList(
          value: field.value,
          extensions: field.ext,
          onChanged: _set,
        );
      default:
        return _Text(
          value: field.value,
          hint: field.kind == 'number' ? '0' : '',
          onChanged: _set,
        );
    }
  }
}

/// The name the dialog puts on its filter row. The portal's filter wants one.
String _filterLabel(List<String> ext) =>
    ext.isEmpty ? 'Any file' : ext.take(4).join(', ').toUpperCase();

String _baseName(String path) {
  final parts = path.split(Platform.pathSeparator).where((s) => s.isNotEmpty);
  return parts.isEmpty ? path : parts.last;
}

/// One path, chosen and never typed.
///
/// The dashed-looking outline is the affordance: an empty row looks like a
/// slot, a filled one looks like a value. Cancelling the dialog returns null
/// and leaves the field exactly as it was — clearing is the X, deliberately.
class _PathRow extends StatefulWidget {
  const _PathRow({
    required this.value,
    required this.icon,
    required this.placeholder,
    required this.wantsDirectory,
    required this.onPicked,
    required this.onChoose,
  });

  final String value;
  final IconData icon;
  final String placeholder;

  /// A folder row takes a folder and nothing else; a file row takes the
  /// reverse. Dropping the wrong one is a miss, not a job that fails later.
  final bool wantsDirectory;
  final ValueChanged<String> onPicked;
  final Future<String?> Function() onChoose;

  @override
  State<_PathRow> createState() => _PathRowState();
}

class _PathRowState extends State<_PathRow> {
  bool _over = false;

  void _onDrop(DropDoneDetails d) {
    setState(() => _over = false);
    for (final item in d.files) {
      if (FileSystemEntity.isDirectorySync(item.path) ==
          widget.wantsDirectory) {
        widget.onPicked(item.path);
        return;
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final filled = widget.value.trim().isNotEmpty;
    return DropTarget(
      onDragEntered: (_) => setState(() => _over = true),
      onDragExited: (_) => setState(() => _over = false),
      onDragDone: _onDrop,
      child: Tooltip(
        message: filled ? widget.value : '',
        waitDuration: const Duration(milliseconds: 600),
        child: Container(
          padding: const EdgeInsets.fromLTRB(11, 6, 6, 6),
          decoration: BoxDecoration(
            color: _over ? Tokens.secTools.withValues(alpha: 0.10) : t.nCard,
            borderRadius: BorderRadius.circular(10),
            border: Border.all(
              color: _over ? Tokens.secTools : (filled ? t.nHair : t.nInk3),
            ),
          ),
          child: Row(
            children: [
              Icon(widget.icon,
                  size: 16, color: filled ? Tokens.secTools : t.nInk3),
              const SizedBox(width: 9),
              Expanded(
                child: Text(
                  _over
                      ? 'Drop it here'
                      : (filled ? _baseName(widget.value) : widget.placeholder),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontFamily: filled && !_over ? 'monospace' : null,
                    fontSize: filled && !_over ? 11.5 : 12,
                    fontStyle:
                        filled && !_over ? FontStyle.normal : FontStyle.italic,
                    color:
                        _over ? Tokens.secTools : (filled ? t.nInk : t.nInk3),
                  ),
                ),
              ),
              const SizedBox(width: 8),
              TextButton(
                style: TextButton.styleFrom(
                  padding: const EdgeInsets.symmetric(horizontal: 10),
                  minimumSize: const Size(0, 30),
                  tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                ),
                onPressed: () async {
                  final picked = await widget.onChoose();
                  if (picked != null && picked.isNotEmpty) {
                    widget.onPicked(picked);
                  }
                },
                child: Text(filled ? 'Change' : 'Choose',
                    style: const TextStyle(fontSize: 11.5)),
              ),
              if (filled)
                IconButton(
                  iconSize: 15,
                  visualDensity: VisualDensity.compact,
                  tooltip: 'Clear',
                  icon: const Icon(Icons.close),
                  onPressed: () => widget.onPicked(''),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Several paths. Stored the way the bridge wants them — one per line — and
/// shown as rows you can drop one at a time.
class _FileList extends StatefulWidget {
  const _FileList({
    required this.value,
    required this.extensions,
    required this.onChanged,
  });

  final String value;
  final List<String> extensions;
  final ValueChanged<String> onChanged;

  @override
  State<_FileList> createState() => _FileListState();
}

class _FileListState extends State<_FileList> {
  bool _over = false;

  List<String> get _paths => [
        for (final l in widget.value.split('\n'))
          if (l.trim().isNotEmpty) l.trim(),
      ];

  /// Adds rather than replaces: picking a second time in a different folder is
  /// how a list of forty gets built.
  void _add(Iterable<String> more) {
    final add = [
      for (final p in more)
        if (!FileSystemEntity.isDirectorySync(p)) p
    ];
    if (add.isEmpty) return;
    widget.onChanged([..._paths, ...add].join('\n'));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final paths = _paths;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        DropTarget(
          onDragEntered: (_) => setState(() => _over = true),
          onDragExited: (_) => setState(() => _over = false),
          onDragDone: (d) {
            setState(() => _over = false);
            _add([for (final f in d.files) f.path]);
          },
          child: Container(
            padding: const EdgeInsets.fromLTRB(11, 6, 6, 6),
            decoration: BoxDecoration(
              color: _over ? Tokens.secTools.withValues(alpha: 0.10) : t.nCard,
              borderRadius: BorderRadius.circular(10),
              border: Border.all(
                color: _over
                    ? Tokens.secTools
                    : (paths.isEmpty ? t.nInk3 : t.nHair),
              ),
            ),
            child: Row(
              children: [
                Icon(Icons.file_copy_outlined,
                    size: 16, color: paths.isEmpty ? t.nInk3 : Tokens.secTools),
                const SizedBox(width: 9),
                Expanded(
                  child: Text(
                    _over
                        ? 'Drop them here'
                        : (paths.isEmpty
                            ? 'Choose files…'
                            : '${paths.length} file${paths.length == 1 ? '' : 's'}'),
                    style: TextStyle(
                      fontSize: 12,
                      fontStyle: paths.isEmpty || _over
                          ? FontStyle.italic
                          : FontStyle.normal,
                      color: _over
                          ? Tokens.secTools
                          : (paths.isEmpty ? t.nInk3 : t.nInk),
                    ),
                  ),
                ),
                TextButton(
                  style: TextButton.styleFrom(
                    padding: const EdgeInsets.symmetric(horizontal: 10),
                    minimumSize: const Size(0, 30),
                    tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                  ),
                  onPressed: () async {
                    final picked = await pickFiles(
                      label: _filterLabel(widget.extensions),
                      extensions: widget.extensions,
                    );
                    _add(picked);
                  },
                  child: const Text('Add', style: TextStyle(fontSize: 11.5)),
                ),
                if (paths.isNotEmpty)
                  IconButton(
                    iconSize: 15,
                    visualDensity: VisualDensity.compact,
                    tooltip: 'Remove all',
                    icon: const Icon(Icons.close),
                    onPressed: () => widget.onChanged(''),
                  ),
              ],
            ),
          ),
        ),
        for (var i = 0; i < paths.length; i++)
          Padding(
            padding: const EdgeInsets.only(top: 4),
            child: Container(
              padding: const EdgeInsets.fromLTRB(9, 3, 3, 3),
              decoration: BoxDecoration(
                color: t.nCard,
                borderRadius: BorderRadius.circular(8),
                border: Border.all(color: t.nHair),
              ),
              child: Row(
                children: [
                  Expanded(
                    child: Tooltip(
                      message: paths[i],
                      waitDuration: const Duration(milliseconds: 600),
                      child: Text(
                        _baseName(paths[i]),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontFamily: 'monospace',
                            fontSize: 10.5,
                            color: t.nInk2),
                      ),
                    ),
                  ),
                  IconButton(
                    iconSize: 14,
                    visualDensity: VisualDensity.compact,
                    tooltip: 'Remove',
                    icon: const Icon(Icons.close),
                    onPressed: () => widget.onChanged(
                      [...paths.sublist(0, i), ...paths.sublist(i + 1)]
                          .join('\n'),
                    ),
                  ),
                ],
              ),
            ),
          ),
      ],
    );
  }
}

/// A text field that does not rebuild itself out from under the cursor: it owns
/// its controller and only pushes on change. Paths do not come through here any
/// more — every one of them is chosen.
class _Text extends StatefulWidget {
  const _Text({
    required this.value,
    required this.hint,
    required this.onChanged,
  });

  final String value;
  final String hint;
  final ValueChanged<String> onChanged;

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
        decoration: InputDecoration(
          isDense: true,
          hintText: widget.hint,
          border: const OutlineInputBorder(),
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
