// The reader — ui/book_reader.slint's shape: a 72px top bar, the stage with
// its floating chevrons and zoom column, a docked panel on the right, and a
// 72px bottom bar. Both bars collapse in fullscreen and slide back when the
// pointer nears the edge they came from.
//
// Three kinds of book, one set of controls. An EPUB is text the bridge has
// already laid out into pages against the current typography, so a page here is
// a string and changing the font size repaginates on the Rust side; a spread is
// two of those laid on the open-book mockup, at the fractions of that image the
// Slint file measured. A comic is a rendered image per leaf, so a page here is
// a file path the bridge extracted from the archive. A PDF is neither: pdfium
// is in this process, so the file itself crosses and a page is drawn here —
// which is what makes the zoom continuous and the search real rather than a
// picture of a page rasterised at a guessed DPI. Fixed-page books skip the
// mockup entirely and use the flat stage, as they do in Slint.
//
// The one PDF path still rendered in Rust is trim, which crops scanned margins
// by measuring the raster. pdfium has no equivalent, so turning trim on puts
// that book back on the poppler path for as long as it is on.

import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:pdfrx/pdfrx.dart';

import '../../src/rust/api/books.dart';
import 'book_theme.dart';
import 'books_controller.dart';
import 'reader_parts.dart';

/// Which side panel is docked. A view preference of this screen, not of the
/// book — opening the contents should not be a round trip to SQLite.
enum _Panel { none, contents, search, bookmarks, notes, thumbs }

class BookReader extends StatefulWidget {
  const BookReader({super.key, required this.controller});

  final BooksController controller;

  @override
  State<BookReader> createState() => _BookReaderState();
}

class _BookReaderState extends State<BookReader> {
  _Panel _panel = _Panel.none;
  bool _typography = false;
  bool _more = false;
  bool _closeConfirm = false;
  bool _fullscreen = false;
  bool _topReveal = false;
  bool _bottomReveal = false;

  /// PDF zoom. Visual, and reset by the stage per page. An ebook has no visual
  /// zoom — its slider is the font size, which repaginates.
  double _zoom = 1.0;

  BooksController get _c => widget.controller;

  void _panelTo(_Panel p) => setState(() {
        _panel = _panel == p ? _Panel.none : p;
        if (_panel == _Panel.thumbs && _c.reader!.thumbs.isEmpty) {
          _c.send(const BooksCmd.loadThumbs());
        }
      });

  void _step(bool forward) => _c.send(
      forward ? const BooksCmd.readerNext() : const BooksCmd.readerPrev());

  void _escape() {
    if (_typography) {
      setState(() => _typography = false);
    } else if (_more) {
      setState(() => _more = false);
    } else if (_fullscreen) {
      setState(() => _fullscreen = false);
    } else {
      setState(() => _closeConfirm = true);
    }
  }

  @override
  Widget build(BuildContext context) {
    final r = _c.reader!;
    final pal = ReaderPalette.of(r.prefs.theme);
    final b = context.book;

    if (r.error.isNotEmpty) return _Broken(controller: _c, reader: r);

    final barTop = _fullscreen && !_topReveal ? 0.0 : 72.0;
    final barBottom = _fullscreen && !_bottomReveal ? 0.0 : 72.0;

    return CallbackShortcuts(
      bindings: <ShortcutActivator, VoidCallback>{
        const SingleActivator(LogicalKeyboardKey.arrowRight): () =>
            _step(true),
        const SingleActivator(LogicalKeyboardKey.arrowLeft): () =>
            _step(false),
        const SingleActivator(LogicalKeyboardKey.arrowDown): () => _step(true),
        const SingleActivator(LogicalKeyboardKey.arrowUp): () => _step(false),
        const SingleActivator(LogicalKeyboardKey.pageDown): () => _step(true),
        const SingleActivator(LogicalKeyboardKey.pageUp): () => _step(false),
        const SingleActivator(LogicalKeyboardKey.space): () => _step(true),
        const SingleActivator(LogicalKeyboardKey.escape): _escape,
        const SingleActivator(LogicalKeyboardKey.keyB): () =>
            _c.send(const BooksCmd.toggleBookmark()),
        const SingleActivator(LogicalKeyboardKey.keyT): () =>
            _panelTo(_Panel.contents),
      },
      child: Focus(
        autofocus: true,
        child: ColoredBox(
          color: b.canvas,
          child: Stack(
            children: [
              Column(
                children: [
                  AnimatedContainer(
                    duration: const Duration(milliseconds: 150),
                    curve: Curves.easeInOut,
                    height: barTop,
                    child: barTop == 0
                        ? const SizedBox.shrink()
                        : ClipRect(
                            child: OverflowBox(
                              minHeight: 72,
                              maxHeight: 72,
                              alignment: Alignment.topCenter,
                              child: _TopBar(
                                controller: _c,
                                reader: r,
                                panel: _panel,
                                typography: _typography,
                                more: _more,
                                onPanel: _panelTo,
                                onTypography: () => setState(
                                    () => _typography = !_typography),
                                onMore: () => setState(() => _more = !_more),
                              ),
                            ),
                          ),
                  ),
                  Expanded(
                    child: Row(
                      children: [
                        Expanded(
                          child: _Stage(
                            controller: _c,
                            reader: r,
                            palette: pal,
                            zoom: _zoom,
                            fullscreen: _fullscreen,
                            onZoom: (v) => setState(() => _zoom = v),
                            onFullscreen: () => setState(() {
                              _fullscreen = !_fullscreen;
                              _topReveal = false;
                              _bottomReveal = false;
                            }),
                            onEdgeHover: (top, bottom) {
                              if (!_fullscreen) return;
                              if (top == _topReveal &&
                                  bottom == _bottomReveal) {
                                return;
                              }
                              setState(() {
                                _topReveal = top;
                                _bottomReveal = bottom;
                              });
                            },
                          ),
                        ),
                        // The panels dock on the right, beside the page —
                        // never between the page and the way out.
                        if (_panel != _Panel.none)
                          _dockedPanel(r),
                      ],
                    ),
                  ),
                  AnimatedContainer(
                    duration: const Duration(milliseconds: 150),
                    curve: Curves.easeInOut,
                    height: barBottom,
                    child: barBottom == 0
                        ? const SizedBox.shrink()
                        : ClipRect(
                            child: OverflowBox(
                              minHeight: 72,
                              maxHeight: 72,
                              alignment: Alignment.bottomCenter,
                              child:
                                  _BottomBar(controller: _c, reader: r),
                            ),
                          ),
                  ),
                ],
              ),
              if (_typography || _more)
                Positioned.fill(
                  child: GestureDetector(
                    behavior: HitTestBehavior.opaque,
                    onTap: () => setState(() {
                      _typography = false;
                      _more = false;
                    }),
                  ),
                ),
              if (_typography)
                Positioned(
                  right: 20,
                  top: 76,
                  child: _TypographySheet(
                    controller: _c,
                    reader: r,
                    zoom: _zoom,
                    fullscreen: _fullscreen,
                    onZoom: (v) => setState(() => _zoom = v),
                    onFullscreen: () => setState(() {
                      _fullscreen = !_fullscreen;
                      _topReveal = false;
                      _bottomReveal = false;
                      _typography = false;
                    }),
                  ),
                ),
              if (_more)
                Positioned(
                  right: 20,
                  top: 76,
                  child: _MorePopover(
                    controller: _c,
                    reader: r,
                    onDone: () => setState(() => _more = false),
                  ),
                ),
              if (_closeConfirm)
                _CloseConfirm(
                  onCancel: () => setState(() => _closeConfirm = false),
                  onClose: () {
                    setState(() => _closeConfirm = false);
                    _c.send(const BooksCmd.closeReader());
                  },
                ),
            ],
          ),
        ),
      ),
    );
  }

  Widget _dockedPanel(Reader r) {
    void close() => setState(() => _panel = _Panel.none);
    return switch (_panel) {
      _Panel.contents =>
        ContentsPanel(controller: _c, reader: r, onClose: close),
      _Panel.search => SearchPanel(controller: _c, reader: r, onClose: close),
      _Panel.bookmarks =>
        BookmarksPanel(controller: _c, reader: r, onClose: close),
      _Panel.notes => NotesPanel(controller: _c, reader: r, onClose: close),
      _Panel.thumbs => ThumbsPanel(controller: _c, reader: r, onClose: close),
      _Panel.none => const SizedBox.shrink(),
    };
  }
}

class _Broken extends StatelessWidget {
  const _Broken({required this.controller, required this.reader});

  final BooksController controller;
  final Reader reader;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return ColoredBox(
      color: b.canvas,
      child: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(Icons.link_off, size: 44, color: BookTheme.danger),
            const SizedBox(height: 14),
            Text(reader.title,
                style: TextStyle(
                    fontSize: 18, fontWeight: FontWeight.w700, color: b.ink)),
            const SizedBox(height: 6),
            Text(reader.error,
                textAlign: TextAlign.center,
                style: TextStyle(fontSize: 15, color: b.inkDim)),
            const SizedBox(height: 18),
            SizedBox(
              width: 160,
              height: 40,
              child: FilledButton(
                style: FilledButton.styleFrom(
                  backgroundColor: BookTheme.accent,
                  shape: RoundedRectangleBorder(
                      borderRadius: BorderRadius.circular(20)),
                ),
                onPressed: () =>
                    controller.send(const BooksCmd.closeReader()),
                child: const Text('Back to library'),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ── top bar ─────────────────────────────────────────────────────────────────

class _TopBar extends StatelessWidget {
  const _TopBar({
    required this.controller,
    required this.reader,
    required this.panel,
    required this.typography,
    required this.more,
    required this.onPanel,
    required this.onTypography,
    required this.onMore,
  });

  final BooksController controller;
  final Reader reader;
  final _Panel panel;
  final bool typography;
  final bool more;
  final ValueChanged<_Panel> onPanel;
  final VoidCallback onTypography;
  final VoidCallback onMore;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final r = reader;
    return Container(
      height: 72,
      padding: const EdgeInsets.symmetric(horizontal: 20),
      decoration: BoxDecoration(
        color: b.card,
        border: Border(bottom: BorderSide(color: b.hairline)),
      ),
      child: Row(
        children: [
          Tooltip(
            message: 'Back to library',
            child: _RoundBack(
                onTap: () => controller.send(const BooksCmd.closeReader())),
          ),
          const SizedBox(width: 12),
          Flexible(child: _TitlePill(reader: r)),
          if (r.nextId != 0) ...[
            const SizedBox(width: 12),
            Flexible(
              child: _NextInSeries(
                title: r.nextTitle,
                onTap: () =>
                    controller.send(const BooksCmd.readerOpenNext()),
              ),
            ),
          ],
          const Spacer(),
          // The right cluster, in the Slint order and its hues.
          if (r.searchEnabled) ...[
            _InlineSearch(
              controller: controller,
              reader: r,
              open: panel == _Panel.search,
              onFocus: () {
                if (panel != _Panel.search) onPanel(_Panel.search);
              },
            ),
            const SizedBox(width: 8),
          ],
          ChromeButton(
            icon: Icons.bookmark,
            accent: BookTheme.amber,
            tip: 'Bookmarks',
            active: r.bookmarked || panel == _Panel.bookmarks,
            onTap: () => onPanel(_Panel.bookmarks),
          ),
          const SizedBox(width: 8),
          // The Slint reader declares the notes model but never puts a button
          // on it; the port's panel is live, so it keeps one.
          ChromeButton(
            icon: Icons.sticky_note_2_outlined,
            accent: BookTheme.teal,
            tip: 'Notes',
            active: panel == _Panel.notes,
            onTap: () => onPanel(_Panel.notes),
          ),
          const SizedBox(width: 8),
          if (r.imageMode) ...[
            ChromeButton(
              icon: Icons.crop,
              accent: const Color(0xFF118AB2),
              tip: r.trim ? 'Show full page' : 'Trim margins',
              active: r.trim,
              onTap: () => controller.send(BooksCmd.setTrim(on_: !r.trim)),
            ),
            const SizedBox(width: 8),
            ChromeButton(
              icon: Icons.grid_view,
              accent: BookTheme.green,
              tip: 'Pages',
              active: panel == _Panel.thumbs,
              onTap: () => onPanel(_Panel.thumbs),
            ),
            const SizedBox(width: 8),
          ],
          ChromeButton(
            icon: Icons.menu,
            accent: BookTheme.accent,
            tip: 'Table of contents',
            active: panel == _Panel.contents,
            onTap: () => onPanel(_Panel.contents),
          ),
          const SizedBox(width: 8),
          ChromeButton(
            icon: r.single ? Icons.description_outlined : Icons.menu_book,
            accent: BookTheme.pink,
            tip: r.single ? 'Two-page view' : 'Single-page view',
            active: r.single,
            onTap: () => controller.send(BooksCmd.setSingle(on_: !r.single)),
          ),
          const SizedBox(width: 8),
          ChromeButton(
            caption: 'Aa',
            accent: BookTheme.accent,
            tip: 'Reading settings',
            active: typography,
            onTap: onTypography,
          ),
          const SizedBox(width: 8),
          ChromeButton(
            icon: Icons.light_mode_outlined,
            accent: BookTheme.amber,
            tip: 'Cycle brightness',
            onTap: () {
              // 1.0 → 0.8 → 0.6 → 1.0, the three steps the Aa sheet offers.
              final v = r.prefs.brightness;
              final next = v >= 0.95 ? 0.8 : (v > 0.65 ? 0.6 : 1.0);
              controller.send(BooksCmd.setBrightness(value: next));
            },
          ),
          const SizedBox(width: 8),
          ChromeButton(
            icon: Icons.more_horiz,
            accent: BookTheme.pink,
            tip: 'More actions',
            active: more,
            onTap: onMore,
          ),
        ],
      ),
    );
  }
}

class _RoundBack extends StatefulWidget {
  const _RoundBack({required this.onTap});

  final VoidCallback onTap;

  @override
  State<_RoundBack> createState() => _RoundBackState();
}

class _RoundBackState extends State<_RoundBack> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) => MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: Container(
            width: 44,
            height: 44,
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color:
                  BookTheme.accent.withValues(alpha: _hover ? 0.2 : 0.10),
              shape: BoxShape.circle,
              border: Border.all(
                  color: BookTheme.accent.withValues(alpha: 0.4)),
            ),
            child: const Icon(Icons.chevron_left,
                size: 20, color: BookTheme.accent),
          ),
        ),
      );
}

class _TitlePill extends StatelessWidget {
  const _TitlePill({required this.reader});

  final Reader reader;

  @override
  Widget build(BuildContext context) => Container(
        height: 48,
        padding: const EdgeInsets.symmetric(horizontal: 14),
        decoration: BoxDecoration(
          color: BookTheme.accent.withValues(alpha: 0.12),
          borderRadius: BorderRadius.circular(14),
          border:
              Border.all(color: BookTheme.accent.withValues(alpha: 0.4)),
        ),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(reader.title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                    fontSize: 15,
                    height: 1.2,
                    fontWeight: FontWeight.w800,
                    color: BookTheme.accent)),
            const SizedBox(height: 2),
            Text(reader.author,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                  fontSize: 11,
                  height: 1.2,
                  fontWeight: FontWeight.w600,
                  color: BookTheme.blue,
                  decoration: TextDecoration.underline,
                  decorationColor: BookTheme.blue,
                )),
          ],
        ),
      );
}

/// The manga and comic case, where finishing one book almost always means
/// opening the next. Hidden entirely when there is no next entry.
class _NextInSeries extends StatefulWidget {
  const _NextInSeries({required this.title, required this.onTap});

  final String title;
  final VoidCallback onTap;

  @override
  State<_NextInSeries> createState() => _NextInSeriesState();
}

class _NextInSeriesState extends State<_NextInSeries> {
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
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 120),
          height: 48,
          padding: const EdgeInsets.symmetric(horizontal: 12),
          decoration: BoxDecoration(
            color: BookTheme.accent
                .withValues(alpha: _hover ? 0.18 : 0.08),
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
                color: BookTheme.accent.withValues(alpha: 0.35)),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Flexible(
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text('Next in series',
                        style: TextStyle(
                            fontSize: 10,
                            height: 1.2,
                            fontWeight: FontWeight.w700,
                            color: b.inkDim)),
                    const SizedBox(height: 1),
                    Text(widget.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                            fontSize: 13,
                            height: 1.2,
                            fontWeight: FontWeight.w800,
                            color: BookTheme.accent)),
                  ],
                ),
              ),
              const SizedBox(width: 8),
              const Icon(Icons.chevron_right,
                  size: 16, color: BookTheme.accent),
            ],
          ),
        ),
      ),
    );
  }
}

/// The search field lives in the bar rather than behind an icon: typing opens
/// the results panel. Hidden for books with no searchable text.
class _InlineSearch extends StatefulWidget {
  const _InlineSearch({
    required this.controller,
    required this.reader,
    required this.open,
    required this.onFocus,
  });

  final BooksController controller;
  final Reader reader;
  final bool open;
  final VoidCallback onFocus;

  @override
  State<_InlineSearch> createState() => _InlineSearchState();
}

class _InlineSearchState extends State<_InlineSearch> {
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
    return SizedBox(
      width: 200,
      height: 44,
      child: Container(
        padding: const EdgeInsets.fromLTRB(10, 0, 8, 0),
        decoration: BoxDecoration(
          color: BookTheme.blue.withValues(alpha: 0.10),
          borderRadius: BorderRadius.circular(12),
          border: Border.all(
              color: BookTheme.blue
                  .withValues(alpha: widget.open ? 0.8 : 0.4)),
        ),
        child: Row(
          children: [
            const Icon(Icons.search, size: 15, color: BookTheme.blue),
            const SizedBox(width: 6),
            Expanded(
              child: TextField(
                controller: _q,
                style: TextStyle(fontSize: 13, color: b.ink),
                decoration: InputDecoration(
                  border: InputBorder.none,
                  isCollapsed: true,
                  hintText: 'Search in book',
                  hintStyle: TextStyle(fontSize: 13, color: b.inkDim),
                ),
                // One search is a scan of every page, so it runs on Enter
                // rather than per keystroke.
                onSubmitted: (v) {
                  widget.onFocus();
                  widget.controller
                      .send(BooksCmd.readerSearch(query: v.trim()));
                },
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ── the stage ───────────────────────────────────────────────────────────────

class _Stage extends StatelessWidget {
  const _Stage({
    required this.controller,
    required this.reader,
    required this.palette,
    required this.zoom,
    required this.fullscreen,
    required this.onZoom,
    required this.onFullscreen,
    required this.onEdgeHover,
  });

  final BooksController controller;
  final Reader reader;
  final ReaderPalette palette;
  final double zoom;
  final bool fullscreen;
  final ValueChanged<double> onZoom;
  final VoidCallback onFullscreen;
  final void Function(bool top, bool bottom) onEdgeHover;

  @override
  Widget build(BuildContext context) {
    final r = reader;
    final b = context.book;

    return ClipRect(
      child: MouseRegion(
        onHover: (e) {
          if (!fullscreen) return;
          final h = context.size?.height ?? 0;
          onEdgeHover(e.localPosition.dy < 90, e.localPosition.dy > h - 90);
        },
        child: Stack(
          children: [
            Positioned.fill(child: ColoredBox(color: b.canvas)),
            if (r.pageCount == 0)
              Center(
                child: Text('This book has no readable pages.',
                    style: TextStyle(color: palette.ink)),
              )
            else
              Positioned.fill(child: _pages(context)),
            // The dimmer sits over the paper and under the chrome: it has to
            // work over rendered PDF pixels we do not own.
            if (r.prefs.brightness < 0.999)
              Positioned.fill(
                child: IgnorePointer(
                  child: ColoredBox(
                    color: Colors.black
                        .withValues(alpha: 1.0 - r.prefs.brightness),
                  ),
                ),
              ),
            if (r.pageCount > 0) ...[
              Positioned(
                left: 16,
                top: 0,
                bottom: 0,
                child: Center(
                  child: EdgeNav(
                    icon: Icons.chevron_left,
                    tip: r.single ? 'Previous page' : 'Previous spread',
                    onTap: () =>
                        controller.send(const BooksCmd.readerPrev()),
                  ),
                ),
              ),
              Positioned(
                right: 16,
                top: 0,
                bottom: 0,
                child: Center(
                  child: EdgeNav(
                    icon: Icons.chevron_right,
                    tip: r.single ? 'Next page' : 'Next spread',
                    onTap: () =>
                        controller.send(const BooksCmd.readerNext()),
                  ),
                ),
              ),
              Positioned(
                right: 12,
                bottom: 16,
                width: 36,
                height: 332,
                child: _ZoomColumn(
                  controller: controller,
                  reader: r,
                  zoom: zoom,
                  fullscreen: fullscreen,
                  onZoom: onZoom,
                  onFullscreen: onFullscreen,
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }

  Widget _pages(BuildContext context) {
    final r = reader;
    // A PDF the bridge did not rasterise is read by pdfium in this process, in
    // the library's own viewer: continuous scroll, pinch zoom, selectable text,
    // links. The archive formats still arrive as one extracted image per leaf.
    if (r.imageMode &&
        r.filePath.isNotEmpty &&
        r.leftImage.isEmpty &&
        r.rightImage.isEmpty) {
      return _PdfDocStage(
        controller: controller,
        reader: r,
        palette: palette,
      );
    }
    if (r.imageMode) {
      return _PdfStage(
        controller: controller,
        reader: r,
        palette: palette,
        zoom: zoom,
      );
    }
    return r.single
        ? _SingleSheet(
            controller: controller, reader: r, palette: palette)
        : _Spread(controller: controller, reader: r, palette: palette);
  }
}

/// The two-page spread: the open-book mockup with two identical text columns
/// centred on its page surfaces.
///
/// The fractions are the ones measured off the 1511 × 1041 artwork — left page
/// centred at 0.286 of the mockup's width, right at 0.714, both at 0.49 of its
/// height, each column 0.34 × 0.87 of it.
class _Spread extends StatelessWidget {
  const _Spread({
    required this.controller,
    required this.reader,
    required this.palette,
  });

  final BooksController controller;
  final Reader reader;
  final ReaderPalette palette;

  static const double _ar = 1511 / 1041;

  @override
  Widget build(BuildContext context) {
    final r = reader;
    return LayoutBuilder(
      builder: (context, box) {
        final w = [
          1360.0,
          box.maxWidth - 140,
          box.maxHeight * _ar,
        ].reduce((a, c) => a < c ? a : c) *
            0.95;
        final bgW = w.clamp(1.0, double.infinity);
        final bgH = bgW / _ar;
        final pageW = bgW * 0.34;
        final pageH = bgH * 0.87;

        Widget page(String body, String heading, int folio, double cx) =>
            Positioned(
              left: bgW * cx - pageW / 2,
              top: bgH * 0.49 - pageH / 2,
              width: pageW,
              height: pageH,
              child: PaperPage(
                body: body,
                heading: heading,
                folio: folio,
                palette: palette,
                prefs: r.prefs,
                onPrev: () =>
                    controller.send(const BooksCmd.readerPrev()),
                onNext: () =>
                    controller.send(const BooksCmd.readerNext()),
              ),
            );

        return Center(
          child: SizedBox(
            width: bgW,
            height: bgH,
            child: Stack(
              clipBehavior: Clip.none,
              children: [
                // Edge shadows, so the mockup sits on the canvas rather than
                // floating over it.
                Positioned(
                  left: -18,
                  top: 8,
                  width: 18,
                  height: bgH - 8,
                  child: const DecoratedBox(
                    decoration: BoxDecoration(
                      gradient: LinearGradient(
                          colors: [Color(0x00000000), Color(0x30000000)]),
                    ),
                  ),
                ),
                Positioned(
                  right: -18,
                  top: 8,
                  width: 18,
                  height: bgH - 8,
                  child: const DecoratedBox(
                    decoration: BoxDecoration(
                      gradient: LinearGradient(
                          colors: [Color(0x30000000), Color(0x00000000)]),
                    ),
                  ),
                ),
                Positioned(
                  left: -8,
                  top: bgH,
                  width: bgW + 16,
                  height: 22,
                  child: const DecoratedBox(
                    decoration: BoxDecoration(
                      gradient: LinearGradient(
                        begin: Alignment.topCenter,
                        end: Alignment.bottomCenter,
                        colors: [Color(0x38000000), Color(0x00000000)],
                      ),
                    ),
                  ),
                ),
                Positioned.fill(
                  child: Image.asset(
                    palette.spreadAsset,
                    fit: BoxFit.fill,
                    errorBuilder: (_, __, ___) => DecoratedBox(
                      decoration: BoxDecoration(
                        color: palette.page,
                        borderRadius: BorderRadius.circular(6),
                      ),
                    ),
                  ),
                ),
                page(r.leftText, r.leftHeading, r.leftFolio.toInt(), 0.286),
                page(r.rightText, r.rightHeading, r.rightFolio.toInt(),
                    0.714),
              ],
            ),
          ),
        );
      },
    );
  }
}

/// One page on its own sheet. Slint scrolls a whole chapter here instead, from
/// a `full-text` the bridge does not send — so this is the same paginated page,
/// alone and centred.
class _SingleSheet extends StatelessWidget {
  const _SingleSheet({
    required this.controller,
    required this.reader,
    required this.palette,
  });

  final BooksController controller;
  final Reader reader;
  final ReaderPalette palette;

  @override
  Widget build(BuildContext context) => Center(
        child: Padding(
          padding: const EdgeInsets.symmetric(vertical: 22),
          child: AspectRatio(
            aspectRatio: 0.72,
            child: Container(
              decoration: BoxDecoration(
                color: palette.page,
                borderRadius: BorderRadius.circular(6),
                boxShadow: const [
                  BoxShadow(
                      color: Color(0x33000000),
                      blurRadius: 24,
                      offset: Offset(0, 6)),
                ],
              ),
              child: PaperPage(
                body: reader.leftText,
                heading: reader.leftHeading,
                folio: reader.leftFolio.toInt(),
                palette: palette,
                prefs: reader.prefs,
                onPrev: () => controller.send(const BooksCmd.readerPrev()),
                onNext: () => controller.send(const BooksCmd.readerNext()),
              ),
            ),
          ),
        ),
      );
}

/// The flat stage a fixed-page book gets: one leaf or two side by side, zoom
/// and pan, no book mockup. Tapping the outer thirds turns the page.
class _PdfStage extends StatelessWidget {
  const _PdfStage({
    required this.controller,
    required this.reader,
    required this.palette,
    required this.zoom,
  });

  final BooksController controller;
  final Reader reader;
  final ReaderPalette palette;
  final double zoom;

  @override
  Widget build(BuildContext context) {
    final r = reader;
    final dual = !r.single && r.rightFolio > 0;

    Widget leaf(String image, int folio) => _Leaf(
          reader: r,
          palette: palette,
          image: image,
          folio: folio,
        );

    // Right-to-left books put the later page on the left.
    final left = leaf(r.leftImage, r.leftFolio.toInt());
    final right = leaf(r.rightImage, r.rightFolio.toInt());

    return LayoutBuilder(
      builder: (context, box) => GestureDetector(
        onTapUp: (d) {
          final x = d.localPosition.dx;
          if (x < box.maxWidth / 3) {
            controller.send(const BooksCmd.readerPrev());
          } else if (x > box.maxWidth * 2 / 3) {
            controller.send(const BooksCmd.readerNext());
          }
        },
        child: Transform.scale(
          scale: zoom,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(56, 22, 56, 22),
            child: dual
                ? Row(children: [
                    Expanded(child: left),
                    const SizedBox(width: 12),
                    Expanded(child: right),
                  ])
                : Center(child: left),
          ),
        ),
      ),
    );
  }
}

/// One rendered leaf — an archive page the bridge extracted, or a PDF page
/// pdfium draws here.
class _Leaf extends StatelessWidget {
  const _Leaf({
    required this.reader,
    required this.palette,
    required this.image,
    required this.folio,
  });

  final Reader reader;
  final ReaderPalette palette;
  final String image;
  final int folio;

  @override
  Widget build(BuildContext context) {
    if (image.isEmpty) return const SizedBox.shrink();
    return Column(
      children: [
        Expanded(
          child: Image.file(
            File(image),
            fit: BoxFit.contain,
            errorBuilder: (_, __, ___) => Center(
              child: Text('Page $folio would not render',
                  style: TextStyle(color: palette.faint, fontSize: 12)),
            ),
          ),
        ),
        const SizedBox(height: 6),
        Text('$folio',
            style: TextStyle(fontSize: 11, color: context.book.inkDim)),
      ],
    );
  }
}

/// A PDF, read in pdfrx's own viewer.
///
/// The archive formats are a bitmap per leaf, so the flat `_PdfStage` is right
/// for them. A PDF is not: pdfium can lay it out, scale it and give the text
/// to the selection layer, and reducing that to one still picture per page
/// throws away everything the format is good at. So this hands the file to the
/// viewer and keeps the reader's own chrome in step with it — the scrub bar,
/// the thumbnails and the contents panel all still drive the page, and a page
/// the reader scrolls to reports itself back.
class _PdfDocStage extends StatefulWidget {
  const _PdfDocStage({
    required this.controller,
    required this.reader,
    required this.palette,
  });

  final BooksController controller;
  final Reader reader;
  final ReaderPalette palette;

  @override
  State<_PdfDocStage> createState() => _PdfDocStageState();
}

class _PdfDocStageState extends State<_PdfDocStage> {
  final PdfViewerController _pdf = PdfViewerController();

  /// The page this widget last asked for, so the viewer's own report of
  /// arriving there is not sent straight back to the bridge as a jump.
  int _wanted = 0;

  /// The reader's dark theme, applied to a page that is black on white.
  /// Rasterising an inverted PNG is what the Rust path did; a colour filter
  /// does it on the GPU and survives a zoom.
  static const ColorFilter _invert = ColorFilter.matrix(<double>[
    -1, 0, 0, 0, 255, //
    0, -1, 0, 0, 255, //
    0, 0, -1, 0, 255, //
    0, 0, 0, 1, 0, //
  ]);

  int get _perScreen => widget.reader.single ? 1 : 2;

  int get _folio => widget.reader.leftFolio.toInt().clamp(1, 1 << 30);

  @override
  void didUpdateWidget(_PdfDocStage old) {
    super.didUpdateWidget(old);
    if (old.reader.leftFolio != widget.reader.leftFolio) _sync();
  }

  /// Move the viewer to whatever page the reader is now on. Deferred, because
  /// this runs during a build and the viewer animates.
  void _sync() {
    final want = _folio;
    if (_wanted == want) return;
    _wanted = want;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted && _pdf.isReady && _pdf.pageNumber != want) {
        _pdf.goToPage(pageNumber: want);
      }
    });
  }

  /// The reverse: the reader scrolled, so tell the bridge which screen page
  /// that is. Writing it back is what keeps progress, bookmarks and the resume
  /// position honest.
  void _report(int? page) {
    if (page == null || page == _wanted) return;
    _wanted = page;
    widget.controller
        .send(BooksCmd.readerJump(page: (page - 1) ~/ _perScreen + 1));
  }

  @override
  Widget build(BuildContext context) {
    _sync();
    final night = widget.reader.prefs.theme == 'dark';
    final view = PdfViewer.file(
      widget.reader.filePath,
      controller: _pdf,
      initialPageNumber: _folio,
      params: PdfViewerParams(
        margin: 12,
        backgroundColor: widget.palette.page.withValues(alpha: 0.35),
        onPageChanged: _report,
        // Two up unless the reader is in single-page mode, which is the same
        // choice the spread makes for every other format.
        layoutPages: _perScreen == 1 ? null : _twoUp,
        loadingBannerBuilder: (context, bytes, total) => Center(
          child: SizedBox(
            width: 18,
            height: 18,
            child: CircularProgressIndicator(
                strokeWidth: 2, color: widget.palette.faint),
          ),
        ),
      ),
    );
    return night ? ColorFiltered(colorFilter: _invert, child: view) : view;
  }

  /// Facing pages, page 1 alone — a book's first leaf is a right-hand page.
  static PdfPageLayout _twoUp(List<PdfPage> pages, PdfViewerParams params) {
    final gap = params.margin;
    // Rows first: [0], then [1,2], [3,4] …
    final rows = <List<PdfPage>>[];
    for (var i = 0; i < pages.length;) {
      final n = i == 0 ? 1 : math.min(2, pages.length - i);
      rows.add(pages.sublist(i, i + n));
      i += n;
    }
    double rowWidth(List<PdfPage> r) =>
        r.fold(0.0, (w, p) => w + p.width) + (r.length - 1) * gap;
    final width = rows.fold(0.0, (w, r) => math.max(w, rowWidth(r))) + gap * 2;

    final out = <Rect>[];
    var y = gap;
    for (final r in rows) {
      final h = r.fold(0.0, (h, p) => math.max(h, p.height));
      var x = (width - rowWidth(r)) / 2;
      for (final p in r) {
        out.add(Rect.fromLTWH(x, y + (h - p.height) / 2, p.width, p.height));
        x += p.width + gap;
      }
      y += h + gap;
    }
    return PdfPageLayout(pageLayouts: out, documentSize: Size(width, y));
  }
}

/// The zoom column pinned to the stage's right-bottom: slider, percentage, a
/// ratio preset and full screen. A PDF zooms its raster; an ebook scales its
/// type, so the same control moves the font size and the engine repaginates.
class _ZoomColumn extends StatelessWidget {
  const _ZoomColumn({
    required this.controller,
    required this.reader,
    required this.zoom,
    required this.fullscreen,
    required this.onZoom,
    required this.onFullscreen,
  });

  final BooksController controller;
  final Reader reader;
  final double zoom;
  final bool fullscreen;
  final ValueChanged<double> onZoom;
  final VoidCallback onFullscreen;

  /// The ebook slider runs over the font-size range the engine accepts, so a
  /// wider range would just be dead travel.
  static const double _minPx = 13;
  static const double _maxPx = 26;

  double get _value => reader.imageMode
      ? zoom
      : (reader.prefs.fontPx - _minPx) / (_maxPx - _minPx) * 0.75 + 0.75;

  void _set(double v) {
    if (reader.imageMode) {
      onZoom(v.clamp(0.5, 4.0));
      return;
    }
    final px = _minPx + ((v - 0.75) / 0.75).clamp(0.0, 1.0) * (_maxPx - _minPx);
    controller.send(BooksCmd.setFontPx(px: px.roundToDouble()));
  }

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final image = reader.imageMode;
    final shown = image ? zoom : reader.prefs.fontPx / 17.0;
    return Column(
      children: [
        Expanded(
          child: VFloatSlider(
            value: _value,
            minimum: image ? 0.5 : 0.75,
            maximum: image ? 4.0 : 1.5,
            onChanged: _set,
          ),
        ),
        const SizedBox(height: 8),
        Text(image ? '${(zoom * 100).round()}%' : '${reader.prefs.fontPx.round()} px',
            style: TextStyle(
                fontSize: 11, fontWeight: FontWeight.w700, color: b.inkDim)),
        const SizedBox(height: 8),
        SizedBox(
          width: 36,
          height: 32,
          child: ChromeButton(
            side: 32,
            caption: image
                ? (zoom == 0.7 ? '70' : (zoom == 0.5 ? '50' : '1:1'))
                : (shown < 0.95 ? '75' : '1:1'),
            tip: image
                ? 'Zoom preset: 100% → 70% → 50%'
                : 'Text preset: 17px ↔ 13px',
            onTap: () {
              if (image) {
                onZoom(zoom == 1.0 ? 0.7 : (zoom == 0.7 ? 0.5 : 1.0));
              } else {
                controller.send(BooksCmd.setFontPx(
                    px: reader.prefs.fontPx > 15 ? 13 : 17));
              }
            },
          ),
        ),
        const SizedBox(height: 8),
        SizedBox(
          width: 36,
          height: 32,
          child: ChromeButton(
            side: 32,
            icon: Icons.fullscreen,
            tip: fullscreen ? 'Exit full screen (Esc)' : 'Full screen',
            active: fullscreen,
            onTap: onFullscreen,
          ),
        ),
      ],
    );
  }
}

// ── bottom bar ──────────────────────────────────────────────────────────────

class _BottomBar extends StatelessWidget {
  const _BottomBar({required this.controller, required this.reader});

  final BooksController controller;
  final Reader reader;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final r = reader;
    final chapters = r.toc.isNotEmpty;

    return Container(
      height: 72,
      padding: const EdgeInsets.symmetric(horizontal: 20),
      decoration: BoxDecoration(
        color: b.card,
        border: Border(top: BorderSide(color: b.hairline)),
      ),
      child: Row(
        children: [
          if (chapters) ...[
            _ChapterPill(
              label: 'Previous',
              icon: Icons.chevron_left,
              filled: false,
              width: 108,
              tip: 'Previous chapter',
              onTap: () =>
                  controller.send(const BooksCmd.readerPrevChapter()),
            ),
            const SizedBox(width: 16),
          ],
          Expanded(
            child: Padding(
              // Without chapters the pills are gone; inset the bar so it still
              // reads as centred.
              padding: EdgeInsets.symmetric(
                  horizontal: chapters ? 0 : 80),
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: BookSlider(
                          value: r.page.toInt(),
                          maximum: r.pageCount.toInt().clamp(1, 1 << 30),
                          step: r.single ? 1 : 2,
                          onChanged: (p) =>
                              controller.send(BooksCmd.readerJump(page: p)),
                        ),
                      ),
                      const SizedBox(width: 12),
                      Container(
                        width: 74,
                        height: 26,
                        alignment: Alignment.center,
                        decoration: BoxDecoration(
                          color: b.card,
                          borderRadius: BorderRadius.circular(13),
                          border: Border.all(color: b.hairline),
                        ),
                        child: Text('${r.page} / ${r.pageCount}',
                            style: TextStyle(
                                fontSize: 13,
                                fontWeight: FontWeight.w700,
                                color: b.ink)),
                      ),
                      const SizedBox(width: 12),
                      Text('${(r.percent * 100).round()}%',
                          style: TextStyle(fontSize: 12, color: b.inkDim)),
                    ],
                  ),
                  if (!r.imageMode || chapters) ...[
                    const SizedBox(height: 2),
                    Row(
                      children: [
                        Expanded(
                          child: Text(r.chapterName,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                  fontSize: 12,
                                  fontWeight: FontWeight.w700,
                                  color: b.inkDim)),
                        ),
                        Expanded(
                          child: Text(r.nextChapterName,
                              maxLines: 1,
                              textAlign: TextAlign.right,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                  fontSize: 12,
                                  fontWeight: FontWeight.w700,
                                  color: b.inkDim)),
                        ),
                      ],
                    ),
                  ],
                ],
              ),
            ),
          ),
          if (chapters) ...[
            const SizedBox(width: 16),
            _ChapterPill(
              label: 'Next',
              icon: Icons.chevron_right,
              filled: true,
              width: 92,
              tip: 'Next chapter',
              onTap: () =>
                  controller.send(const BooksCmd.readerNextChapter()),
            ),
          ],
        ],
      ),
    );
  }
}

class _ChapterPill extends StatefulWidget {
  const _ChapterPill({
    required this.label,
    required this.icon,
    required this.filled,
    required this.width,
    required this.tip,
    required this.onTap,
  });

  final String label;
  final IconData icon;
  final bool filled;
  final double width;
  final String tip;
  final VoidCallback onTap;

  @override
  State<_ChapterPill> createState() => _ChapterPillState();
}

class _ChapterPillState extends State<_ChapterPill> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final filled = widget.filled;
    final hue = filled ? BookTheme.accent : BookTheme.blue;
    return Tooltip(
      message: widget.tip,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: GestureDetector(
          onTap: widget.onTap,
          child: Container(
            height: 40,
            alignment: Alignment.center,
            constraints: BoxConstraints(minWidth: widget.width),
            padding: const EdgeInsets.symmetric(horizontal: 12),
            decoration: BoxDecoration(
              color: filled
                  ? hue.withValues(alpha: _hover ? 0.9 : 1.0)
                  : hue.withValues(alpha: _hover ? 0.24 : 0.10),
              borderRadius: BorderRadius.circular(20),
              border: filled
                  ? null
                  : Border.all(color: hue.withValues(alpha: 0.45)),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (!filled) ...[
                  Icon(widget.icon, size: 14, color: hue),
                  const SizedBox(width: 4),
                ],
                Text(widget.label,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight:
                            filled ? FontWeight.w700 : FontWeight.w600,
                        color: filled ? Colors.white : hue)),
                if (filled) ...[
                  const SizedBox(width: 4),
                  Icon(widget.icon, size: 14, color: Colors.white),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

// ── the Aa sheet, the ••• popover and the close dialog ──────────────────────

class _TypographySheet extends StatelessWidget {
  const _TypographySheet({
    required this.controller,
    required this.reader,
    required this.zoom,
    required this.fullscreen,
    required this.onZoom,
    required this.onFullscreen,
  });

  final BooksController controller;
  final Reader reader;
  final double zoom;
  final bool fullscreen;
  final ValueChanged<double> onZoom;
  final VoidCallback onFullscreen;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final r = reader;
    final p = r.prefs;
    final image = r.imageMode;

    Widget head(String s) => Padding(
          padding: const EdgeInsets.only(bottom: 7),
          child: Text(s,
              style: TextStyle(
                  fontSize: 11,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 0.5,
                  color: b.inkDim)),
        );

    Widget seg(List<String> labels, int active, ValueChanged<int> pick,
            {List<String?>? fonts}) =>
        Row(
          children: [
            for (var i = 0; i < labels.length; i++) ...[
              if (i > 0) const SizedBox(width: 8),
              Expanded(
                child: SegItem(
                  label: labels[i],
                  active: active == i,
                  previewFont: fonts?[i],
                  onTap: () => pick(i),
                ),
              ),
            ],
          ],
        );

    return Container(
      width: 320,
      height: image ? 352 : 566,
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: b.card,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: b.hairline),
        boxShadow: const [
          BoxShadow(
              color: Color(0x26000000),
              blurRadius: 28,
              offset: Offset(0, 6)),
        ],
      ),
      child: ListView(
        padding: EdgeInsets.zero,
        children: [
          if (image) ...[
            // Fixed-page books: layout, zoom and full screen. The typography
            // below is meaningless for a page that is a picture.
            head('Page layout'),
            seg(
              const ['Single', 'Spread'],
              r.single ? 0 : 1,
              (i) => controller.send(BooksCmd.setSingle(on_: i == 0)),
            ),
            const SizedBox(height: 14),
            head('Zoom'),
            Row(
              children: [
                ChromeButton(
                  side: 36,
                  caption: '−',
                  onTap: () => onZoom((zoom - 0.25).clamp(0.5, 4.0)),
                ),
                Expanded(
                  child: Text('${(zoom * 100).round()}%',
                      textAlign: TextAlign.center,
                      style: TextStyle(
                          fontSize: 14,
                          fontWeight: FontWeight.w700,
                          color: b.ink)),
                ),
                ChromeButton(
                  side: 36,
                  caption: '+',
                  onTap: () => onZoom((zoom + 0.25).clamp(0.5, 4.0)),
                ),
              ],
            ),
            const SizedBox(height: 8),
            SegItem(
                label: 'Fit page (100%)',
                active: zoom == 1.0,
                onTap: () => onZoom(1.0)),
            const SizedBox(height: 14),
            SegItem(
              label: fullscreen
                  ? 'Full screen: On  (Esc exits)'
                  : 'Full screen: Off',
              active: fullscreen,
              onTap: onFullscreen,
            ),
            const SizedBox(height: 14),
          ] else ...[
            head('Text size'),
            Row(
              children: [
                ChromeButton(
                  side: 36,
                  caption: 'A−',
                  onTap: () => controller.send(
                      BooksCmd.setFontPx(px: (p.fontPx - 1).clamp(13, 26))),
                ),
                Expanded(
                  child: Text('${p.fontPx.round()} px',
                      textAlign: TextAlign.center,
                      style: TextStyle(
                          fontSize: 14,
                          fontWeight: FontWeight.w700,
                          color: b.ink)),
                ),
                ChromeButton(
                  side: 36,
                  caption: 'A+',
                  onTap: () => controller.send(
                      BooksCmd.setFontPx(px: (p.fontPx + 1).clamp(13, 26))),
                ),
              ],
            ),
            const SizedBox(height: 14),
            head('Line spacing'),
            seg(
                const ['Compact', 'Normal', 'Relaxed'],
                p.lineIndex.toInt(),
                (i) => controller.send(BooksCmd.setLineSpacing(index: i))),
            const SizedBox(height: 14),
            head('Margins'),
            seg(const ['Narrow', 'Normal', 'Wide'], p.marginIndex.toInt(),
                (i) => controller.send(BooksCmd.setMargin(index: i))),
            const SizedBox(height: 14),
            head('Typeface'),
            seg(const ['Serif', 'Sans', 'Mono'], p.typeface.toInt(),
                (i) => controller.send(BooksCmd.setTypeface(index: i)),
                fonts: const ['serif', null, 'monospace']),
            const SizedBox(height: 14),
            head('Alignment'),
            seg(const ['Left', 'Center', 'Right'], p.align.toInt(),
                (i) => controller.send(BooksCmd.setAlign(index: i))),
            const SizedBox(height: 14),
            head('Theme'),
            Row(
              children: [
                // Slint offers a fourth, night; the bridge folds anything it
                // does not know back to light, so only the three it stores are
                // offered here.
                for (final t in const [
                  (key: 'light', col: Color(0xFFFDFCF8)),
                  (key: 'sepia', col: Color(0xFFF4ECD8)),
                  (key: 'dark', col: Color(0xFF1C1C20)),
                ]) ...[
                  if (t.key != 'light') const SizedBox(width: 10),
                  Expanded(
                    child: GestureDetector(
                      onTap: () =>
                          controller.send(BooksCmd.setTheme(theme: t.key)),
                      child: MouseRegion(
                        cursor: SystemMouseCursors.click,
                        child: Container(
                          height: 36,
                          decoration: BoxDecoration(
                            color: t.col,
                            borderRadius: BorderRadius.circular(10),
                            border: Border.all(
                              color: p.theme == t.key
                                  ? BookTheme.accent
                                  : b.hairline,
                              width: p.theme == t.key ? 2.5 : 1,
                            ),
                          ),
                        ),
                      ),
                    ),
                  ),
                ],
              ],
            ),
            const SizedBox(height: 14),
          ],
          head('Brightness'),
          seg(
            const ['Dim', 'Medium', 'Full'],
            p.brightness <= 0.65 ? 0 : (p.brightness < 0.95 ? 1 : 2),
            (i) => controller.send(BooksCmd.setBrightness(
                value: switch (i) { 0 => 0.6, 1 => 0.8, _ => 1.0 })),
          ),
          if (!image) ...[
            const SizedBox(height: 14),
            SegItem(
              label: p.bold ? 'Bold: On' : 'Bold: Off',
              active: p.bold,
              onTap: () => controller.send(const BooksCmd.toggleBold()),
            ),
          ],
        ],
      ),
    );
  }
}

class _MorePopover extends StatelessWidget {
  const _MorePopover({
    required this.controller,
    required this.reader,
    required this.onDone,
  });

  final BooksController controller;
  final Reader reader;
  final VoidCallback onDone;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final rows = <({String label, IconData icon, bool danger, VoidCallback go})>[
      (
        label: 'Mark as finished',
        icon: Icons.check,
        danger: false,
        go: () => controller.send(const BooksCmd.markFinished()),
      ),
      if (reader.imageMode)
        (
          label: reader.rtl ? 'Left-to-right pages' : 'Right-to-left pages',
          icon: Icons.swap_horiz,
          danger: false,
          go: () => controller.send(BooksCmd.toggleRtl(id: reader.id)),
        ),
      (
        label: 'Remove from library',
        icon: Icons.delete_outline,
        danger: true,
        go: () async {
          await controller.send(BooksCmd.trash(id: reader.id));
          await controller.send(const BooksCmd.closeReader());
        },
      ),
    ];

    return Container(
      width: 220,
      padding: const EdgeInsets.all(8),
      decoration: BoxDecoration(
        color: b.card,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: b.hairline),
        boxShadow: const [
          BoxShadow(
              color: Color(0x22000000),
              blurRadius: 24,
              offset: Offset(0, 4)),
        ],
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (final row in rows)
            _MoreRow(
              label: row.label,
              icon: row.icon,
              danger: row.danger,
              onTap: () {
                onDone();
                row.go();
              },
            ),
        ],
      ),
    );
  }
}

class _MoreRow extends StatefulWidget {
  const _MoreRow({
    required this.label,
    required this.icon,
    required this.danger,
    required this.onTap,
  });

  final String label;
  final IconData icon;
  final bool danger;
  final VoidCallback onTap;

  @override
  State<_MoreRow> createState() => _MoreRowState();
}

class _MoreRowState extends State<_MoreRow> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    final ink = widget.danger ? BookTheme.danger : b.ink;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: Container(
          height: 38,
          padding: const EdgeInsets.symmetric(horizontal: 10),
          decoration: BoxDecoration(
            color: _hover ? b.pillBg : Colors.transparent,
            borderRadius: BorderRadius.circular(8),
          ),
          child: Row(
            children: [
              Icon(widget.icon,
                  size: 16,
                  color: widget.danger ? BookTheme.danger : b.inkDim),
              const SizedBox(width: 10),
              Expanded(
                child: Text(widget.label,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w600,
                        color: ink)),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Escape asks before it throws away where you are. Enter confirms.
class _CloseConfirm extends StatelessWidget {
  const _CloseConfirm({required this.onCancel, required this.onClose});

  final VoidCallback onCancel;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return Stack(
      children: [
        Positioned.fill(
          child: GestureDetector(
            onTap: onCancel,
            child: const ColoredBox(color: Color(0x66000000)),
          ),
        ),
        Center(
          child: CallbackShortcuts(
            bindings: <ShortcutActivator, VoidCallback>{
              const SingleActivator(LogicalKeyboardKey.enter): onClose,
              const SingleActivator(LogicalKeyboardKey.numpadEnter): onClose,
              const SingleActivator(LogicalKeyboardKey.escape): onCancel,
            },
            child: Focus(
              autofocus: true,
              child: Container(
                width: 320,
                height: 150,
                padding: const EdgeInsets.all(20),
                decoration: BoxDecoration(
                  color: b.card,
                  borderRadius: BorderRadius.circular(16),
                  border: Border.all(
                      color: BookTheme.accent.withValues(alpha: 0.5),
                      width: 1.5),
                  boxShadow: const [
                    BoxShadow(color: Color(0x44000000), blurRadius: 32),
                  ],
                ),
                child: Column(
                  children: [
                    Text('Close this book?',
                        style: TextStyle(
                            fontSize: 16,
                            fontWeight: FontWeight.w800,
                            color: b.ink)),
                    const SizedBox(height: 14),
                    Text('Your reading position is saved.',
                        style: TextStyle(fontSize: 12, color: b.inkDim)),
                    const Spacer(),
                    Row(
                      mainAxisAlignment: MainAxisAlignment.center,
                      children: [
                        _DialogButton(
                          label: 'Cancel  (Esc)',
                          filled: false,
                          onTap: onCancel,
                        ),
                        const SizedBox(width: 10),
                        _DialogButton(
                          label: 'Close  (↵)',
                          filled: true,
                          onTap: onClose,
                        ),
                      ],
                    ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

class _DialogButton extends StatelessWidget {
  const _DialogButton({
    required this.label,
    required this.filled,
    required this.onTap,
  });

  final String label;
  final bool filled;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final b = context.book;
    return GestureDetector(
      onTap: onTap,
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        child: Container(
          width: 110,
          height: 38,
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: filled ? BookTheme.accent : b.pillBg,
            borderRadius: BorderRadius.circular(19),
            border: filled ? null : Border.all(color: b.hairline),
          ),
          child: Text(label,
              style: TextStyle(
                  fontSize: 12,
                  fontWeight: filled ? FontWeight.w700 : FontWeight.w600,
                  color: filled ? Colors.white : b.ink)),
        ),
      ),
    );
  }
}
