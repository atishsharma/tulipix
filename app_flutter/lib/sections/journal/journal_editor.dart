// What you write in an entry: Markdown, drawn as you type.
//
// The entry is kept as plain Markdown in journal.db, so search, export and the
// word count read it as they always have. The field draws it — headings
// larger, **bold** bold, _italic_ slanted, quotes quieter, done to-dos struck
// through — with the marks left in, dimmed, so nothing you type is hidden.
// A small bar puts the marks in for you.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';

class MarkdownController extends TextEditingController {
  MarkdownController({super.text});

  static final _heading = RegExp(r'^(#{1,3}) ');
  static final _done = RegExp(r'^- \[[xX]\] ');
  static final _item = RegExp(r'^(- \[ \] |[-*] |\d+\. )');
  static final _marks = RegExp(r'\*\*(.+?)\*\*|(?<![\w])_(.+?)_(?![\w])');

  @override
  TextSpan buildTextSpan({
    required BuildContext context,
    TextStyle? style,
    required bool withComposing,
  }) {
    // An input method mid-word draws its own underline; leave it be.
    if (withComposing && value.isComposingRangeValid) {
      return super.buildTextSpan(
          context: context, style: style, withComposing: withComposing);
    }
    final base = style ?? const TextStyle();
    final ink = base.color ?? context.tokens.nInk;
    final dim = base.copyWith(color: ink.withValues(alpha: 0.35));
    final spans = <InlineSpan>[];
    final lines = text.split('\n');
    for (var i = 0; i < lines.length; i++) {
      final line = lines[i];
      var ls = base;
      var cut = 0;
      if (_heading.firstMatch(line) case final h?) {
        final size = base.fontSize ?? 16;
        ls = base.copyWith(
            fontWeight: FontWeight.w700,
            fontSize: size * (h.group(1)!.length == 1 ? 1.35 : 1.18));
        cut = h.end;
      } else if (line.startsWith('> ')) {
        ls = base.copyWith(
            fontStyle: FontStyle.italic, color: ink.withValues(alpha: 0.7));
        cut = 2;
      } else if (_done.hasMatch(line)) {
        ls = base.copyWith(
            decoration: TextDecoration.lineThrough,
            color: ink.withValues(alpha: 0.5));
        cut = 6;
      } else if (_item.firstMatch(line) case final m?) {
        cut = m.end;
      }
      if (cut > 0) {
        spans.add(TextSpan(
            text: line.substring(0, cut),
            style: dim.copyWith(fontSize: ls.fontSize)));
      }
      _inline(line.substring(cut), ls, dim.copyWith(fontSize: ls.fontSize),
          spans);
      if (i < lines.length - 1) spans.add(TextSpan(text: '\n', style: base));
    }
    return TextSpan(style: base, children: spans);
  }

  static void _inline(
      String s, TextStyle st, TextStyle dim, List<InlineSpan> out) {
    var at = 0;
    for (final m in _marks.allMatches(s)) {
      if (m.start > at) out.add(TextSpan(text: s.substring(at, m.start), style: st));
      final bold = m.group(1) != null;
      final n = bold ? 2 : 1;
      out
        ..add(TextSpan(text: s.substring(m.start, m.start + n), style: dim))
        ..add(TextSpan(
            text: s.substring(m.start + n, m.end - n),
            style: bold
                ? st.copyWith(fontWeight: FontWeight.w700)
                : st.copyWith(fontStyle: FontStyle.italic)))
        ..add(TextSpan(text: s.substring(m.end - n, m.end), style: dim));
      at = m.end;
    }
    if (at < s.length) out.add(TextSpan(text: s.substring(at), style: st));
  }
}

/// Bold, italic, a heading, a list, a to-do and a quote, as Markdown into
/// [text]. Taps count as inside the field, so the cursor stays where it was.
class MarkdownBar extends StatelessWidget {
  const MarkdownBar({super.key, required this.text, required this.onEdit});

  final TextEditingController text;

  /// Called after every change, so the entry saves as if typed.
  final VoidCallback onEdit;

  TextSelection get _sel => text.selection.isValid
      ? text.selection
      : TextSelection.collapsed(offset: text.text.length);

  void _wrap(String m) {
    final sel = _sel;
    final inner = sel.textInside(text.text);
    final start = sel.start + m.length;
    text.value = TextEditingValue(
      text: text.text.replaceRange(sel.start, sel.end, '$m$inner$m'),
      selection: TextSelection(
          baseOffset: start, extentOffset: start + inner.length),
    );
    onEdit();
  }

  /// Put [p] at the start of every line the selection touches, or take it
  /// off when all of them already have it.
  void _prefix(String p) {
    final s = text.text;
    final sel = _sel;
    final start = sel.start == 0 ? 0 : s.lastIndexOf('\n', sel.start - 1) + 1;
    var end = s.indexOf('\n', sel.end);
    if (end < 0) end = s.length;
    final lines = s.substring(start, end).split('\n');
    final all = lines.every((l) => l.startsWith(p));
    final block = [
      for (final l in lines) all ? l.substring(p.length) : '$p$l',
    ].join('\n');
    text.value = TextEditingValue(
      text: s.replaceRange(start, end, block),
      selection: TextSelection.collapsed(offset: start + block.length),
    );
    onEdit();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget b(IconData icon, String tip, VoidCallback onTap) => Tooltip(
          message: tip,
          child: InkWell(
            canRequestFocus: false,
            borderRadius: BorderRadius.circular(6),
            onTap: onTap,
            child: Padding(
              padding: const EdgeInsets.all(5),
              child: Icon(icon, size: 17, color: t.nInk3),
            ),
          ),
        );
    return TextFieldTapRegion(
      child: Wrap(
        spacing: 2,
        children: [
          b(Icons.format_bold, 'Bold', () => _wrap('**')),
          b(Icons.format_italic, 'Italic', () => _wrap('_')),
          b(Icons.title, 'Heading', () => _prefix('## ')),
          b(Icons.format_list_bulleted, 'List', () => _prefix('- ')),
          b(Icons.check_box_outlined, 'To do', () => _prefix('- [ ] ')),
          b(Icons.format_quote, 'Quote', () => _prefix('> ')),
        ],
      ),
    );
  }
}
