// The reader's small parts — one Dart class per component in the top third of
// ui/book_reader.slint: ChromeButton, SegItem, EdgeNav, PaperPage, BookSlider,
// VFloatSlider, and the four side panels (contents, search, thumbnails,
// bookmarks) plus the notes panel the port keeps.
//
// The chrome is the section's palette (BookTheme), not the paper's: the bars
// and panels around the page belong to the app, and only the page itself wears
// the reading theme. Slint splits them the same way.

import 'dart:io';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';

import '../../src/rust/api/books.dart';
import 'book_detail.dart';
import 'book_theme.dart';
import 'books_controller.dart';

/// The paper themes. Deliberately independent of the app theme: people read at
/// night in a light app. Transcribed from `paper` / `paper-ink` in
/// book_reader.slint.
@immutable
class ReaderPalette {
  const ReaderPalette({required this.page, required this.ink});

  factory ReaderPalette.of(String theme) => switch (theme) {
        'sepia' =>
          const ReaderPalette(page: Color(0xFFF4ECD8), ink: Color(0xFF4A3F2F)),
        'dark' =>
          const ReaderPalette(page: Color(0xFF1C1C20), ink: Color(0xFFD8D8DC)),
        _ =>
          const ReaderPalette(page: Color(0xFFFDFCF8), ink: Color(0xFF1B1B22)),
      };

  final Color page;
  final Color ink;

  /// The folio and the rules, which Slint draws as the ink at low alpha.
  Color get faint => Color.lerp(ink, page, 0.45)!;

  /// The open-book mockup behind a two-page spread, one per reading theme.
  String get spreadAsset => switch (page.toARGB32()) {
        0xFFF4ECD8 => 'assets/bookhero/Book_Sepia.png',
        0xFF1C1C20 => 'assets/bookhero/Book_Dark.png',
        _ => 'assets/bookhero/Book_Light.png',
      };
}

/// 44px rounded-square chrome icon button; `active` deepens its own hue.
class ChromeButton extends StatefulWidget {
  const ChromeButton({
    super.key,
    required this.onTap,
    this.icon,
    this.caption,
    this.tip = '',
    this.active = false,
    this.accent = BookTheme.accent,
    this.side = 44,
  });

  final VoidCallback onTap;
  final IconData? icon;
  final String? caption;
  final String tip;
  final bool active;
  final Color accent;
  final double side;

  @override
  State<ChromeButton> createState() => _ChromeButtonState();
}

class _ChromeButtonState extends State<ChromeButton> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final a = widget.accent;
    final button = MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 120),
          width: widget.side,
          height: widget.side,
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: a.withValues(
                alpha: widget.active ? 0.26 : (_hover ? 0.18 : 0.10)),
            borderRadius: BorderRadius.circular(12),
            border: Border.all(
                color: a.withValues(alpha: widget.active ? 0.8 : 0.4)),
          ),
          child: widget.caption != null
              ? Text(widget.caption!,
                  style: TextStyle(
                      fontSize: widget.side < 40 ? 13 : 15,
                      fontWeight: FontWeight.w800,
                      color: a))
              : Icon(widget.icon, size: widget.side < 40 ? 16 : 20, color: a),
        ),
      ),
    );
    return widget.tip.isEmpty
        ? button
        : Tooltip(message: widget.tip, child: button);
  }
}

/// One segment of a settings segmented control.
class SegItem extends StatelessWidget {
  const SegItem({
    super.key,
    required this.label,
    required this.active,
    required this.onTap,
    this.textSize = 12,
    this.previewFont,
  });

  final String label;
  final bool active;
  final VoidCallback onTap;
  final double textSize;
  final String? previewFont;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return GestureDetector(
      onTap: onTap,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        child: Container(
          height: 34,
          alignment: Alignment.center,
          padding: const EdgeInsets.symmetric(horizontal: 6),
          decoration: BoxDecoration(
            color: active ? BookTheme.accent : b.pillBg,
            borderRadius: BorderRadius.circular(9),
            border: Border.all(color: active ? BookTheme.accent : b.hairline),
          ),
          child: Text(label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: textSize,
                  fontWeight: FontWeight.w600,
                  fontFamily: previewFont,
                  color: active ? Colors.white : b.ink)),
        ),
      ),
    );
  }
}

/// Floating circular chevron at a spread edge — card fill plus an accent
/// outline, so it stays visible on every reading theme.
class EdgeNav extends StatefulWidget {
  const EdgeNav({
    super.key,
    required this.icon,
    required this.tip,
    required this.onTap,
  });

  final IconData icon;
  final String tip;
  final VoidCallback onTap;

  @override
  State<EdgeNav> createState() => _EdgeNavState();
}

class _EdgeNavState extends State<EdgeNav> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return Tooltip(
      message: widget.tip,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: Container(
            width: 48,
            height: 48,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: _hover ? b.tintViolet : b.card,
              shape: BoxShape.circle,
              border: Border.all(
                  color: BookTheme.accent.withValues(alpha: 0.55), width: 1.5),
              boxShadow: const [
                BoxShadow(
                    color: Color(0x22000000),
                    blurRadius: 16,
                    offset: Offset(0, 2)),
              ],
            ),
            child: Icon(widget.icon, size: 22, color: BookTheme.accent),
          ),
        ),
      ),
    );
  }
}

/// One paper page: the folio `‹ n ›` pinned to the top edge, then the body
/// column. No paper or shadow of its own — it sits on the open-book mockup the
/// spread draws underneath.
/// `body` as one span, or three with the read-aloud sentence lit in the middle.
///
/// Found by searching the page text rather than by offset: the bridge's
/// sentence carries a page index, not a character range into the laid-out page
/// -- and a sentence that came out of this page's text is in this page's text.
/// A page it is not on returns the plain span, which is what makes this safe to
/// hand to both halves of a spread.
TextSpan _readAloudSpans(String body, String? sentence, Color ink) {
  final s = sentence?.trim() ?? '';
  // Two characters is not a sentence, and a one-character needle would light
  // the first letter of an unrelated word.
  final at = s.length < 3 ? -1 : body.indexOf(s);
  if (at < 0) return TextSpan(text: body);
  return TextSpan(children: [
    TextSpan(text: body.substring(0, at)),
    TextSpan(
      text: body.substring(at, at + s.length),
      // A wash behind the words, not a change of ink: the reader has four
      // themes and each picks its own ink, so recolouring the text would
      // fight whichever one is up.
      style: TextStyle(backgroundColor: ink.withValues(alpha: 0.15)),
    ),
    TextSpan(text: body.substring(at + s.length)),
  ]);
}

class PaperPage extends StatelessWidget {
  const PaperPage({
    super.key,
    required this.body,
    required this.heading,
    required this.folio,
    required this.palette,
    required this.prefs,
    required this.onPrev,
    required this.onNext,
    this.highlight,
  });

  final String body;
  final String heading;
  final int folio;
  final ReaderPalette palette;
  final ReaderPrefs prefs;
  final VoidCallback onPrev;
  final VoidCallback onNext;

  /// The sentence Read Aloud is speaking. Lit where it falls on this page; a
  /// page that does not contain it is drawn exactly as before.
  final String? highlight;

  /// Horizontal padding, both sides combined — the engine's own MARGINS table,
  /// so what is rendered matches what was paginated.
  static double padX(int marginIndex) => switch (marginIndex) {
        0 => 56,
        2 => 150,
        _ => 96,
      };

  static String? family(int typeface) => switch (typeface) {
        1 => null, // the platform sans, which is the default
        2 => 'monospace',
        _ => 'serif',
      };

  static TextAlign align(int a) => switch (a) {
        0 => TextAlign.left,
        2 => TextAlign.right,
        _ => TextAlign.center,
      };

  static double lineHeight(int lineIndex) => switch (lineIndex) {
        0 => 1.35,
        2 => 1.85,
        _ => 1.55,
      };

  @override
  Widget build(BuildContext context) {
    final p = prefs;
    final ink = palette.ink;
    final pad = padX(p.marginIndex.toInt()) / 2;

    return Stack(
      children: [
        Positioned(
          left: pad,
          right: pad,
          top: 40,
          bottom: 20,
          child: ClipRect(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (heading.isNotEmpty) ...[
                  const SizedBox(height: 12),
                  Text(heading,
                      textAlign: TextAlign.center,
                      style: TextStyle(
                          fontSize: p.fontPx * 1.85,
                          height: 1.15,
                          fontFamily: family(p.typeface.toInt()),
                          fontWeight: FontWeight.w700,
                          color: ink)),
                  const SizedBox(height: 16),
                  _Ornament(ink: ink),
                  const SizedBox(height: 26),
                ],
                Expanded(
                  child: Text.rich(
                    _readAloudSpans(body, highlight, ink),
                    textAlign: align(p.align.toInt()),
                    style: TextStyle(
                      fontSize: p.fontPx,
                      height: lineHeight(p.lineIndex.toInt()),
                      fontFamily: family(p.typeface.toInt()),
                      fontWeight: p.bold ? FontWeight.w600 : FontWeight.w400,
                      color: ink,
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
        // The folio, out of the text flow, with a step either side of it.
        Positioned(
          left: 0,
          right: 0,
          top: 2,
          height: 20,
          child: Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              _FolioStep(
                  icon: Icons.chevron_left, ink: palette.faint, onTap: onPrev),
              const SizedBox(width: 12),
              Text('$folio',
                  style: TextStyle(fontSize: 12, color: palette.faint)),
              const SizedBox(width: 12),
              _FolioStep(
                  icon: Icons.chevron_right, ink: palette.faint, onTap: onNext),
            ],
          ),
        ),
      ],
    );
  }
}

class _FolioStep extends StatelessWidget {
  const _FolioStep({
    required this.icon,
    required this.ink,
    required this.onTap,
  });

  final IconData icon;
  final Color ink;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => GestureDetector(
        onTap: onTap,
        child: MouseRegion(
          cursor: SystemMouseCursors.click,
          child: SizedBox(width: 16, child: Icon(icon, size: 12, color: ink)),
        ),
      );
}

/// Two thin rules and a centre dot, under a chapter opener.
class _Ornament extends StatelessWidget {
  const _Ornament({required this.ink});

  final Color ink;

  @override
  Widget build(BuildContext context) => SizedBox(
        height: 8,
        child: Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Container(width: 54, height: 1, color: ink.withValues(alpha: 0.28)),
            const SizedBox(width: 10),
            Container(
              width: 5,
              height: 5,
              decoration: BoxDecoration(
                  color: ink.withValues(alpha: 0.42), shape: BoxShape.circle),
            ),
            const SizedBox(width: 10),
            Container(width: 54, height: 1, color: ink.withValues(alpha: 0.28)),
          ],
        ),
      );
}

/// The scrub bar: a 12px violet track with a white knob. Click or drag maps x
/// to a page; the wheel steps one spread.
///
/// It commits on release rather than per frame, which is the one place this
/// deviates from the Slint slider: a PDF scrubbed at sixty hertz renders sixty
/// pages nobody sees.
class BookSlider extends StatefulWidget {
  const BookSlider({
    super.key,
    required this.value,
    required this.maximum,
    required this.onChanged,
    this.minimum = 1,
    this.step = 1,
  });

  final int value;
  final int minimum;
  final int maximum;
  final int step;
  final ValueChanged<int> onChanged;

  @override
  State<BookSlider> createState() => _BookSliderState();
}

class _BookSliderState extends State<BookSlider> {
  int? _dragging;

  int _at(double dx, double width) {
    final f = width <= 0 ? 0.0 : (dx / width).clamp(0.0, 1.0);
    return widget.minimum + (f * (widget.maximum - widget.minimum)).round();
  }

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final span = widget.maximum - widget.minimum;
    final value = _dragging ?? widget.value;
    final frac = span > 0 ? (value - widget.minimum) / span : 0.0;

    return LayoutBuilder(
      builder: (context, box) => Listener(
        onPointerSignal: (e) {
          if (e is! PointerScrollEvent) return;
          final next = e.scrollDelta.dy > 0 || e.scrollDelta.dx > 0
              ? (widget.value - widget.step)
                  .clamp(widget.minimum, widget.maximum)
              : (widget.value + widget.step)
                  .clamp(widget.minimum, widget.maximum);
          if (next != widget.value) widget.onChanged(next);
        },
        child: GestureDetector(
          onTapDown: (d) =>
              widget.onChanged(_at(d.localPosition.dx, box.maxWidth)),
          onHorizontalDragUpdate: (d) =>
              setState(() => _dragging = _at(d.localPosition.dx, box.maxWidth)),
          onHorizontalDragEnd: (_) {
            final v = _dragging;
            setState(() => _dragging = null);
            if (v != null) widget.onChanged(v);
          },
          child: MouseRegion(
            cursor: SystemMouseCursors.click,
            child: SizedBox(
              height: 24,
              child: Stack(
                alignment: Alignment.centerLeft,
                children: [
                  Container(
                    height: 12,
                    decoration: BoxDecoration(
                      color: b.track,
                      borderRadius: BorderRadius.circular(6),
                    ),
                  ),
                  FractionallySizedBox(
                    widthFactor: frac.clamp(0.0, 1.0),
                    child: Container(
                      height: 12,
                      decoration: BoxDecoration(
                        color: BookTheme.accent,
                        borderRadius: BorderRadius.circular(6),
                      ),
                    ),
                  ),
                  Positioned(
                    left: frac.clamp(0.0, 1.0) * (box.maxWidth - 20),
                    child: Container(
                      width: 20,
                      height: 20,
                      decoration: BoxDecoration(
                        color: Colors.white,
                        shape: BoxShape.circle,
                        border: Border.all(color: BookTheme.accent, width: 2),
                        boxShadow: const [
                          BoxShadow(
                              color: Color(0x26000000),
                              blurRadius: 6,
                              offset: Offset(0, 1)),
                        ],
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// The vertical zoom slider pinned to the stage's right edge: + on top, − at
/// the bottom, the value between them.
class VFloatSlider extends StatelessWidget {
  const VFloatSlider({
    super.key,
    required this.value,
    required this.minimum,
    required this.maximum,
    required this.onChanged,
  });

  final double value;
  final double minimum;
  final double maximum;
  final ValueChanged<double> onChanged;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final span = maximum - minimum;
    final frac = span > 0 ? ((value - minimum) / span).clamp(0.0, 1.0) : 0.0;

    Widget end(String glyph, VoidCallback onTap) => GestureDetector(
          onTap: onTap,
          child: MouseRegion(
            cursor: SystemMouseCursors.click,
            child: Container(
              height: 24,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: BookTheme.accent.withValues(alpha: 0.12),
                borderRadius: BorderRadius.circular(8),
                border:
                    Border.all(color: BookTheme.accent.withValues(alpha: 0.45)),
              ),
              child: Text(glyph,
                  style: const TextStyle(
                      fontSize: 15,
                      fontWeight: FontWeight.w800,
                      color: BookTheme.accent)),
            ),
          ),
        );

    return Column(
      children: [
        end('+', () => onChanged((value + 0.25).clamp(minimum, maximum))),
        const SizedBox(height: 6),
        Expanded(
          child: LayoutBuilder(
            builder: (context, box) {
              void pick(double dy) => onChanged(
                  minimum + (1 - (dy / box.maxHeight).clamp(0.0, 1.0)) * span);
              return GestureDetector(
                onTapDown: (d) => pick(d.localPosition.dy),
                onVerticalDragUpdate: (d) => pick(d.localPosition.dy),
                child: MouseRegion(
                  cursor: SystemMouseCursors.click,
                  child: Stack(
                    alignment: Alignment.topCenter,
                    children: [
                      Container(
                        width: 6,
                        decoration: BoxDecoration(
                          color: b.track,
                          borderRadius: BorderRadius.circular(3),
                        ),
                      ),
                      Positioned(
                        top: box.maxHeight * (1 - frac),
                        height: box.maxHeight * frac,
                        child: Container(
                          width: 6,
                          decoration: BoxDecoration(
                            color: BookTheme.accent,
                            borderRadius: BorderRadius.circular(3),
                          ),
                        ),
                      ),
                      Positioned(
                        top: (1 - frac) * (box.maxHeight - 16),
                        child: Container(
                          width: 16,
                          height: 16,
                          decoration: BoxDecoration(
                            color: Colors.white,
                            shape: BoxShape.circle,
                            border:
                                Border.all(color: BookTheme.accent, width: 2),
                            boxShadow: const [
                              BoxShadow(
                                  color: Color(0x26000000), blurRadius: 5),
                            ],
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
              );
            },
          ),
        ),
        const SizedBox(height: 6),
        end('−', () => onChanged((value - 0.25).clamp(minimum, maximum))),
      ],
    );
  }
}

// ── the side panels ─────────────────────────────────────────────────────────

/// The 310px docked shell every panel shares: card, hairline, 16 padding, and
/// a title row with a small close button.
class ReaderPanel extends StatelessWidget {
  const ReaderPanel({
    super.key,
    required this.title,
    required this.onClose,
    required this.children,
  });

  final String title;
  final VoidCallback onClose;
  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return Container(
      width: 310,
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: b.card,
        border: Border(left: BorderSide(color: b.hairline)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(title,
                    style: TextStyle(
                        fontSize: 17,
                        fontWeight: FontWeight.w700,
                        color: b.ink)),
              ),
              ChromeButton(icon: Icons.close, side: 32, onTap: onClose),
            ],
          ),
          const SizedBox(height: 12),
          ...children,
        ],
      ),
    );
  }
}

class ContentsPanel extends StatelessWidget {
  const ContentsPanel({
    super.key,
    required this.controller,
    required this.reader,
    required this.onClose,
  });

  final BooksController controller;
  final Reader reader;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return ReaderPanel(
      title: 'Contents',
      onClose: onClose,
      children: [
        Expanded(
          child: reader.toc.isEmpty
              ? Center(
                  child: Text('This book has no table of contents.',
                      style: TextStyle(fontSize: 12, color: b.inkDim)))
              : ListView.separated(
                  padding: EdgeInsets.zero,
                  itemCount: reader.toc.length,
                  separatorBuilder: (_, __) => const SizedBox(height: 2),
                  itemBuilder: (_, i) {
                    final row = reader.toc[i];
                    final live = row.chapter >= 0;
                    return _TocRow(
                      row: row,
                      live: live,
                      onTap: live
                          ? () => controller
                              .send(BooksCmd.readerChapter(index: row.chapter))
                          : null,
                    );
                  },
                ),
        ),
      ],
    );
  }
}

class _TocRow extends StatefulWidget {
  const _TocRow({required this.row, required this.live, required this.onTap});

  final TocItem row;
  final bool live;
  final VoidCallback? onTap;

  @override
  State<_TocRow> createState() => _TocRowState();
}

class _TocRowState extends State<_TocRow> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final row = widget.row;
    return MouseRegion(
      cursor: widget.live ? SystemMouseCursors.click : SystemMouseCursors.basic,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: Container(
          height: 34,
          padding: EdgeInsets.fromLTRB(8 + row.depth * 16.0, 0, 10, 0),
          decoration: BoxDecoration(
            color: _hover ? b.pillBg : Colors.transparent,
            borderRadius: BorderRadius.circular(8),
          ),
          child: Row(
            children: [
              if (widget.live) ...[
                Icon(Icons.chevron_right, size: 12, color: b.inkDim),
                const SizedBox(width: 6),
              ],
              Expanded(
                child: Text(row.label,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 13, color: widget.live ? b.ink : b.inkDim)),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class SearchPanel extends StatefulWidget {
  const SearchPanel({
    super.key,
    required this.controller,
    required this.reader,
    required this.onClose,
  });

  final BooksController controller;
  final Reader reader;
  final VoidCallback onClose;

  @override
  State<SearchPanel> createState() => _SearchPanelState();
}

class _SearchPanelState extends State<SearchPanel> {
  late final TextEditingController _q =
      TextEditingController(text: widget.reader.searchQuery);

  @override
  void dispose() {
    _q.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final hits = widget.reader.searchResults;
    return ReaderPanel(
      title: 'Search',
      onClose: widget.onClose,
      children: [
        TextField(
          controller: _q,
          autofocus: true,
          decoration: const InputDecoration(
            isDense: true,
            hintText: 'Find in book',
            border: OutlineInputBorder(),
          ),
          onSubmitted: (v) =>
              widget.controller.send(BooksCmd.readerSearch(query: v)),
        ),
        const SizedBox(height: 12),
        Text(
          _q.text.isEmpty
              ? ''
              : '${hits.length} ${hits.length == 1 ? "result" : "results"}',
          style: TextStyle(fontSize: 12, color: b.inkDim),
        ),
        const SizedBox(height: 6),
        Expanded(
          child: ListView.builder(
            padding: EdgeInsets.zero,
            itemCount: hits.length,
            itemBuilder: (_, i) => _Hit(
              hit: hits[i],
              onTap: () => widget.controller
                  .send(BooksCmd.readerJump(page: hits[i].page)),
            ),
          ),
        ),
      ],
    );
  }
}

class _Hit extends StatefulWidget {
  const _Hit({required this.hit, required this.onTap});

  final SearchHit hit;
  final VoidCallback onTap;

  @override
  State<_Hit> createState() => _HitState();
}

class _HitState extends State<_Hit> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: Container(
          height: 56,
          padding: const EdgeInsets.fromLTRB(8, 0, 10, 0),
          decoration: BoxDecoration(
            color: _hover ? b.pillBg : Colors.transparent,
            borderRadius: BorderRadius.circular(8),
          ),
          child: Row(
            children: [
              SizedBox(
                width: 34,
                child: Text('p${widget.hit.page}',
                    style: const TextStyle(
                        fontSize: 12,
                        fontWeight: FontWeight.w700,
                        color: BookTheme.accent)),
              ),
              Expanded(
                child: Text(widget.hit.snippet,
                    maxLines: 3,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 12, height: 1.3, color: b.ink)),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// The navigation primitive for magazines and comics: pages there have no
/// chapter structure, so a contents list gives you nothing to aim at.
class ThumbsPanel extends StatelessWidget {
  const ThumbsPanel({
    super.key,
    required this.controller,
    required this.reader,
    required this.onClose,
  });

  final BooksController controller;
  final Reader reader;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final thumbs = reader.thumbs;
    final total = reader.pageCount.toInt();
    return ReaderPanel(
      title: 'Pages',
      onClose: onClose,
      children: [
        if (thumbs.length < total)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                      thumbs.isEmpty
                          ? 'Not rendered yet.'
                          : 'Rendering pages… ${thumbs.length} / $total',
                      style: TextStyle(fontSize: 12, color: b.inkDim)),
                ),
                ChromeButton(
                  icon: Icons.refresh,
                  side: 32,
                  tip: 'Render thumbnails',
                  onTap: () => controller.send(const BooksCmd.loadThumbs()),
                ),
              ],
            ),
          ),
        Expanded(
          child: GridView.builder(
            padding: EdgeInsets.zero,
            gridDelegate: const SliverGridDelegateWithFixedCrossAxisCount(
              crossAxisCount: 2,
              mainAxisSpacing: 10,
              crossAxisSpacing: 10,
              mainAxisExtent: 196,
            ),
            itemCount: thumbs.length,
            itemBuilder: (_, i) {
              final t = thumbs[i];
              final here = t.page == reader.page;
              return GestureDetector(
                onTap: () => controller.send(BooksCmd.readerJump(page: t.page)),
                child: MouseRegion(
                  cursor: SystemMouseCursors.click,
                  child: Container(
                    padding: const EdgeInsets.all(6),
                    decoration: BoxDecoration(
                      color: here
                          ? BookTheme.accent.withValues(alpha: 0.18)
                          : Colors.transparent,
                      borderRadius: BorderRadius.circular(8),
                      border: Border.all(
                          color: here ? BookTheme.accent : b.hairline,
                          width: here ? 2 : 1),
                    ),
                    child: Column(
                      children: [
                        Expanded(
                          child: Image.file(File(t.path),
                              fit: BoxFit.contain,
                              errorBuilder: (_, __, ___) =>
                                  ColoredBox(color: b.track)),
                        ),
                        const SizedBox(height: 4),
                        Text('${t.page}',
                            style: TextStyle(
                                fontSize: 11,
                                fontWeight: FontWeight.w700,
                                color: here ? BookTheme.accent : b.inkDim)),
                      ],
                    ),
                  ),
                ),
              );
            },
          ),
        ),
      ],
    );
  }
}

class BookmarksPanel extends StatelessWidget {
  const BookmarksPanel({
    super.key,
    required this.controller,
    required this.reader,
    required this.onClose,
  });

  final BooksController controller;
  final Reader reader;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final marks = reader.bookmarks;
    return ReaderPanel(
      title: 'Bookmarks',
      onClose: onClose,
      children: [
        GestureDetector(
          onTap: () => controller.send(const BooksCmd.toggleBookmark()),
          child: MouseRegion(
            cursor: SystemMouseCursors.click,
            child: Container(
              height: 40,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: BookTheme.amber,
                borderRadius: BorderRadius.circular(10),
              ),
              child: Row(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  const Icon(Icons.bookmark, size: 15, color: Colors.white),
                  const SizedBox(width: 6),
                  Flexible(
                    child: Text(
                        reader.bookmarked
                            ? 'Remove bookmark on this page'
                            : 'Bookmark this page',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                            fontSize: 13,
                            fontWeight: FontWeight.w700,
                            color: Colors.white)),
                  ),
                ],
              ),
            ),
          ),
        ),
        const SizedBox(height: 12),
        if (marks.isEmpty)
          Text('No bookmarks yet.',
              style: TextStyle(fontSize: 12, color: b.inkDim)),
        Expanded(
          child: ListView.builder(
            padding: EdgeInsets.zero,
            itemCount: marks.length,
            itemBuilder: (_, i) {
              final m = marks[i];
              return Container(
                height: 44,
                padding: const EdgeInsets.fromLTRB(8, 0, 6, 0),
                child: Row(
                  children: [
                    Expanded(
                      child: GestureDetector(
                        onTap: () => controller
                            .send(BooksCmd.bookmarkJump(page: m.page)),
                        child: MouseRegion(
                          cursor: SystemMouseCursors.click,
                          child: Row(
                            children: [
                              const Icon(Icons.bookmark,
                                  size: 14, color: BookTheme.amber),
                              const SizedBox(width: 8),
                              Expanded(
                                child: Text(
                                    'Page ${m.page}'
                                    '${m.note.isEmpty ? "" : " · ${m.note}"}',
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                    style: TextStyle(
                                        fontSize: 13,
                                        fontWeight: FontWeight.w600,
                                        color: b.ink)),
                              ),
                            ],
                          ),
                        ),
                      ),
                    ),
                    ChromeButton(
                      icon: Icons.delete_outline,
                      side: 28,
                      accent: BookTheme.danger,
                      onTap: () =>
                          controller.send(BooksCmd.bookmarkRemove(id: m.id)),
                    ),
                  ],
                ),
              );
            },
          ),
        ),
      ],
    );
  }
}

/// Page-anchored notes. The Slint reader declares this panel's model and its
/// four callbacks but never renders it — the port does, so it stays.
class NotesPanel extends StatelessWidget {
  const NotesPanel({
    super.key,
    required this.controller,
    required this.reader,
    required this.onClose,
  });

  final BooksController controller;
  final Reader reader;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final notes = reader.notes;
    return ReaderPanel(
      title: 'Notes',
      onClose: onClose,
      children: [
        GestureDetector(
          onTap: () async {
            final note = await promptText(
              context,
              title: 'Note on page ${reader.page}',
              label: 'Your note',
              confirm: 'Save',
            );
            if (note == null) return;
            final text = reader.leftText;
            await controller.send(BooksCmd.annotAdd(
              snippet: text.isEmpty
                  ? ''
                  : text.substring(0, text.length.clamp(0, 120)),
              note: note,
              color: 'yellow',
            ));
          },
          child: MouseRegion(
            cursor: SystemMouseCursors.click,
            child: Container(
              height: 40,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: BookTheme.accent,
                borderRadius: BorderRadius.circular(10),
              ),
              child: const Row(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  Icon(Icons.add, size: 15, color: Colors.white),
                  SizedBox(width: 6),
                  Flexible(
                    child: Text('Note on this page',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 13,
                            fontWeight: FontWeight.w700,
                            color: Colors.white)),
                  ),
                ],
              ),
            ),
          ),
        ),
        const SizedBox(height: 12),
        if (notes.isEmpty)
          Text('No notes yet.',
              style: TextStyle(fontSize: 12, color: b.inkDim)),
        Expanded(
          child: ListView.builder(
            padding: EdgeInsets.zero,
            itemCount: notes.length,
            itemBuilder: (_, i) {
              final n = notes[i];
              return GestureDetector(
                onTap: () =>
                    controller.send(BooksCmd.bookmarkJump(page: n.page)),
                child: Padding(
                  padding: const EdgeInsets.only(bottom: 8),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Row(
                        children: [
                          Text('Page ${n.page + 1}',
                              style: const TextStyle(
                                  fontSize: 10,
                                  fontWeight: FontWeight.w700,
                                  color: BookTheme.accent)),
                          const Spacer(),
                          ChromeButton(
                            icon: Icons.edit_outlined,
                            side: 26,
                            onTap: () async {
                              final v = await promptText(
                                context,
                                title: 'Edit note',
                                label: 'Note',
                                initial: n.note,
                                confirm: 'Save',
                              );
                              if (v != null) {
                                await controller.send(
                                    BooksCmd.annotSetNote(id: n.id, note: v));
                              }
                            },
                          ),
                          const SizedBox(width: 6),
                          ChromeButton(
                            icon: Icons.close,
                            side: 26,
                            accent: BookTheme.danger,
                            onTap: () =>
                                controller.send(BooksCmd.annotRemove(id: n.id)),
                          ),
                        ],
                      ),
                      if (n.snippet.isNotEmpty)
                        Text('“${n.snippet}”',
                            maxLines: 3,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontSize: 11,
                                fontStyle: FontStyle.italic,
                                color: b.inkDim)),
                      if (n.note.isNotEmpty)
                        Padding(
                          padding: const EdgeInsets.only(top: 4),
                          child: Text(n.note,
                              style: TextStyle(fontSize: 12, color: b.ink)),
                        ),
                    ],
                  ),
                ),
              );
            },
          ),
        ),
      ],
    );
  }
}
