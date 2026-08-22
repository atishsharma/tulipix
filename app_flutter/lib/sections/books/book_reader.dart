// The reader.
//
// Two kinds of book, one set of controls. An EPUB is text the bridge has
// already laid out into pages against the current typography, so a page here is
// a string and changing the font size repaginates on the Rust side. A PDF or a
// comic is a rendered image per leaf, so a page here is a file path and the
// typography controls do not apply.
//
// The spread is the unit of navigation in both: two leaves side by side, or one
// in single mode, and right-to-left books swap which leaf goes where. That
// belongs to the bridge — it is where the page and the progress record agree.

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/books.dart';
import 'book_detail.dart';
import 'books_controller.dart';

/// The three page tints. Sepia and dark are the reader's own, deliberately
/// independent of the app theme: people read at night in a light app.
({Color page, Color ink, Color faint}) _palette(String theme) =>
    switch (theme) {
      'sepia' => (
          page: const Color(0xFFF4ECD8),
          ink: const Color(0xFF3B2F1E),
          faint: const Color(0xFF8A7A5E),
        ),
      'dark' => (
          page: const Color(0xFF16171C),
          ink: const Color(0xFFD8D5CE),
          faint: const Color(0xFF6B6B72),
        ),
      _ => (
          page: const Color(0xFFFDFCF9),
          ink: const Color(0xFF1E1B16),
          faint: const Color(0xFF8C8778),
        ),
    };

const List<String> _typefaces = ['Serif', 'Sans', 'Mono'];

class BookReader extends StatelessWidget {
  const BookReader({super.key, required this.controller});

  final BooksController controller;

  @override
  Widget build(BuildContext context) {
    final r = controller.reader!;
    final pal = _palette(r.prefs.theme);

    if (r.error.isNotEmpty) {
      return _Broken(controller: controller, reader: r);
    }

    return CallbackShortcuts(
      bindings: <ShortcutActivator, VoidCallback>{
        const SingleActivator(LogicalKeyboardKey.arrowRight): () =>
            controller.send(const BooksCmd.readerNext()),
        const SingleActivator(LogicalKeyboardKey.arrowLeft): () =>
            controller.send(const BooksCmd.readerPrev()),
        const SingleActivator(LogicalKeyboardKey.space): () =>
            controller.send(const BooksCmd.readerNext()),
        const SingleActivator(LogicalKeyboardKey.escape): () =>
            controller.send(const BooksCmd.closeReader()),
        const SingleActivator(LogicalKeyboardKey.keyB): () =>
            controller.send(const BooksCmd.toggleBookmark()),
      },
      child: Focus(
        autofocus: true,
        child: ColoredBox(
          color: pal.page,
          child: Column(
            children: [
              _Chrome(controller: controller, reader: r, palette: pal),
              Expanded(
                child: Row(
                  children: [
                    _SidePanel(controller: controller, reader: r),
                    Expanded(
                      child: Stack(
                        children: [
                          // Brightness is a dim over the page, not a change to
                          // the page colour: it has to work over rendered PDF
                          // images too, and those are pixels we do not own.
                          Positioned.fill(
                            child: _Spread(
                                controller: controller,
                                reader: r,
                                palette: pal),
                          ),
                          if (r.prefs.brightness < 0.999)
                            Positioned.fill(
                              child: IgnorePointer(
                                child: ColoredBox(
                                  color: Colors.black.withValues(
                                      alpha: 1.0 - r.prefs.brightness),
                                ),
                              ),
                            ),
                          Positioned(
                            left: 0,
                            top: 0,
                            bottom: 0,
                            width: 64,
                            child: _EdgeNav(
                              icon: Icons.chevron_left,
                              enabled: r.page > 1,
                              onTap: () =>
                                  controller.send(const BooksCmd.readerPrev()),
                            ),
                          ),
                          Positioned(
                            right: 0,
                            top: 0,
                            bottom: 0,
                            width: 64,
                            child: _EdgeNav(
                              icon: Icons.chevron_right,
                              enabled: r.page < r.pageCount,
                              onTap: () =>
                                  controller.send(const BooksCmd.readerNext()),
                            ),
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
              _Footer(controller: controller, reader: r, palette: pal),
            ],
          ),
        ),
      ),
    );
  }
}

class _Broken extends StatelessWidget {
  const _Broken({required this.controller, required this.reader});

  final BooksController controller;
  final Reader reader;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ColoredBox(
      color: t.nCanvas,
      child: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(Icons.link_off, size: 44, color: Tokens.error),
            const SizedBox(height: 14),
            Text(reader.title,
                style: TextStyle(
                    fontSize: 18, fontWeight: FontWeight.w700, color: t.nInk)),
            const SizedBox(height: 6),
            Text(reader.error, style: TextStyle(fontSize: 13, color: t.nInk2)),
            const SizedBox(height: 18),
            FilledButton(
              onPressed: () => controller.send(const BooksCmd.closeReader()),
              child: const Text('Back to the shelf'),
            ),
          ],
        ),
      ),
    );
  }
}

/// The top bar: out, what this is, and the panels.
class _Chrome extends StatelessWidget {
  const _Chrome({
    required this.controller,
    required this.reader,
    required this.palette,
  });

  final BooksController controller;
  final Reader reader;
  final ({Color page, Color ink, Color faint}) palette;

  @override
  Widget build(BuildContext context) {
    final r = reader;
    return Container(
      height: 52,
      padding: const EdgeInsets.symmetric(horizontal: 10),
      decoration: BoxDecoration(
        color: palette.page,
        border: Border(
            bottom: BorderSide(color: palette.faint.withValues(alpha: 0.25))),
      ),
      child: Row(
        children: [
          IconButton(
            tooltip: 'Back to the shelf',
            icon: Icon(Icons.arrow_back, color: palette.ink),
            onPressed: () => controller.send(const BooksCmd.closeReader()),
          ),
          const SizedBox(width: 6),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Text(r.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 14,
                        fontWeight: FontWeight.w700,
                        color: palette.ink)),
                Text(
                  [
                    r.author,
                    if (r.chapterName.isNotEmpty) r.chapterName,
                  ].where((s) => s.isNotEmpty).join('  ·  '),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11, color: palette.faint),
                ),
              ],
            ),
          ),
          _ChromeButton(
            icon: Icons.list,
            tip: 'Contents',
            palette: palette,
            active: _panel == 'contents',
            onTap: () => _setPanel('contents'),
          ),
          if (r.searchEnabled)
            _ChromeButton(
              icon: Icons.search,
              tip: 'Search in this book',
              palette: palette,
              active: _panel == 'search',
              onTap: () => _setPanel('search'),
            ),
          _ChromeButton(
            icon: r.bookmarked ? Icons.bookmark : Icons.bookmark_border,
            tip: r.bookmarked ? 'Remove bookmark' : 'Bookmark this page',
            palette: palette,
            active: r.bookmarked,
            onTap: () => controller.send(const BooksCmd.toggleBookmark()),
          ),
          _ChromeButton(
            icon: Icons.bookmarks_outlined,
            tip: 'Bookmarks',
            palette: palette,
            active: _panel == 'bookmarks',
            onTap: () => _setPanel('bookmarks'),
          ),
          _ChromeButton(
            icon: Icons.sticky_note_2_outlined,
            tip: 'Notes',
            palette: palette,
            active: _panel == 'notes',
            onTap: () => _setPanel('notes'),
          ),
          if (r.imageMode)
            _ChromeButton(
              icon: Icons.grid_view,
              tip: 'Pages',
              palette: palette,
              active: _panel == 'thumbs',
              onTap: () {
                _setPanel('thumbs');
                if (_panel == 'thumbs' && r.thumbs.isEmpty) {
                  controller.send(const BooksCmd.loadThumbs());
                }
              },
            ),
          _ChromeButton(
            icon: Icons.text_fields,
            tip: 'Typography',
            palette: palette,
            active: _panel == 'type',
            onTap: () => _setPanel('type'),
          ),
          _ReaderMenu(controller: controller, reader: r, palette: palette),
        ],
      ),
    );
  }

  // Which side panel is open. Reader panels are a view preference of this
  // screen, not of the book, so they live here rather than in the snapshot —
  // opening the contents should not be a round trip to SQLite.
  static String _panel = '';
  static final ValueNotifier<int> _panelTick = ValueNotifier(0);

  void _setPanel(String which) {
    _panel = _panel == which ? '' : which;
    _panelTick.value++;
  }
}

class _ChromeButton extends StatelessWidget {
  const _ChromeButton({
    required this.icon,
    required this.tip,
    required this.palette,
    required this.active,
    required this.onTap,
  });

  final IconData icon;
  final String tip;
  final ({Color page, Color ink, Color faint}) palette;
  final bool active;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => IconButton(
        tooltip: tip,
        iconSize: 19,
        icon: Icon(icon, color: active ? Tokens.secBooks : palette.ink),
        onPressed: onTap,
      );
}

class _ReaderMenu extends StatelessWidget {
  const _ReaderMenu({
    required this.controller,
    required this.reader,
    required this.palette,
  });

  final BooksController controller;
  final Reader reader;
  final ({Color page, Color ink, Color faint}) palette;

  @override
  Widget build(BuildContext context) {
    final r = reader;
    return PopupMenuButton<String>(
      tooltip: 'More',
      icon: Icon(Icons.more_vert, color: palette.ink),
      onSelected: (v) {
        switch (v) {
          case 'single':
            controller.send(BooksCmd.setSingle(on_: !r.single));
          case 'trim':
            controller.send(BooksCmd.setTrim(on_: !r.trim));
          case 'rtl':
            controller.send(BooksCmd.toggleRtl(id: r.id));
          case 'finished':
            controller.send(const BooksCmd.markFinished());
          case 'next':
            controller.send(const BooksCmd.readerOpenNext());
        }
      },
      itemBuilder: (_) => [
        PopupMenuItem(
          value: 'single',
          child: Text(r.single ? 'Two-page spread' : 'Single page'),
        ),
        if (r.imageMode)
          PopupMenuItem(
            value: 'trim',
            child: Text(r.trim ? 'Keep the margins' : 'Trim the margins'),
          ),
        if (r.imageMode)
          PopupMenuItem(
            value: 'rtl',
            child: Text(r.rtl ? 'Left-to-right pages' : 'Right-to-left pages'),
          ),
        const PopupMenuDivider(),
        const PopupMenuItem(value: 'finished', child: Text('Mark as finished')),
        if (r.nextId != 0)
          PopupMenuItem(
            value: 'next',
            child: Text('Next: ${r.nextTitle}'),
          ),
      ],
    );
  }
}

/// The page, or the two pages.
class _Spread extends StatelessWidget {
  const _Spread({
    required this.controller,
    required this.reader,
    required this.palette,
  });

  final BooksController controller;
  final Reader reader;
  final ({Color page, Color ink, Color faint}) palette;

  @override
  Widget build(BuildContext context) {
    final r = reader;
    if (r.pageCount == 0) {
      return Center(
        child: Text('This book has no readable pages.',
            style: TextStyle(color: palette.faint)),
      );
    }
    // The last spread of the last book in a series is the one place an
    // invitation belongs.
    final atEnd = r.page >= r.pageCount;

    final left = _Leaf(
      reader: r,
      palette: palette,
      text: r.leftText,
      heading: r.leftHeading,
      image: r.leftImage,
      folio: r.leftFolio,
    );
    final right = _Leaf(
      reader: r,
      palette: palette,
      text: r.rightText,
      heading: r.rightHeading,
      image: r.rightImage,
      folio: r.rightFolio,
    );

    return Column(
      children: [
        Expanded(
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 56, vertical: 22),
            child: r.single
                ? Center(child: SizedBox(width: 620, child: left))
                : Row(
                    children: [
                      Expanded(child: left),
                      Container(
                        width: 1,
                        margin: const EdgeInsets.symmetric(vertical: 28),
                        color: palette.faint.withValues(alpha: 0.22),
                      ),
                      Expanded(child: right),
                    ],
                  ),
          ),
        ),
        if (atEnd && r.nextId != 0)
          Padding(
            padding: const EdgeInsets.only(bottom: 16),
            child: FilledButton.icon(
              style: FilledButton.styleFrom(backgroundColor: Tokens.secBooks),
              icon: const Icon(Icons.east, size: 18),
              label: Text('Next in the series: ${r.nextTitle}'),
              onPressed: () => controller.send(const BooksCmd.readerOpenNext()),
            ),
          ),
      ],
    );
  }
}

class _Leaf extends StatelessWidget {
  const _Leaf({
    required this.reader,
    required this.palette,
    required this.text,
    required this.heading,
    required this.image,
    required this.folio,
  });

  final Reader reader;
  final ({Color page, Color ink, Color faint}) palette;
  final String text;
  final String heading;
  final String image;
  final int folio;

  @override
  Widget build(BuildContext context) {
    if (reader.imageMode) {
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
          Text('$folio', style: TextStyle(fontSize: 11, color: palette.faint)),
        ],
      );
    }

    if (text.isEmpty) return const SizedBox.shrink();
    final p = reader.prefs;
    final family = switch (p.typeface) {
      1 => null, // the platform sans, which is the default
      2 => 'monospace',
      _ => 'serif',
    };
    final align = switch (p.align) {
      0 => TextAlign.left,
      2 => TextAlign.right,
      _ => TextAlign.justify,
    };
    final lineHeight = switch (p.lineIndex) {
      0 => 1.35,
      2 => 1.85,
      _ => 1.55,
    };
    final margin = switch (p.marginIndex) {
      0 => 24.0,
      2 => 72.0,
      _ => 44.0,
    };

    return Padding(
      padding: EdgeInsets.symmetric(horizontal: margin),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (heading.isNotEmpty) ...[
            const SizedBox(height: 18),
            Text(
              heading,
              textAlign: TextAlign.center,
              style: TextStyle(
                fontSize: p.fontPx * 1.25,
                fontWeight: FontWeight.w700,
                fontFamily: family,
                color: palette.ink,
              ),
            ),
            const SizedBox(height: 10),
            Center(
              child: Container(
                width: 60,
                height: 1,
                color: palette.faint.withValues(alpha: 0.5),
              ),
            ),
            const SizedBox(height: 18),
          ],
          Expanded(
            child: Text(
              text,
              textAlign: align,
              style: TextStyle(
                fontSize: p.fontPx,
                height: lineHeight,
                fontFamily: family,
                fontWeight: p.bold ? FontWeight.w600 : FontWeight.w400,
                color: palette.ink,
              ),
            ),
          ),
          const SizedBox(height: 8),
          Text('$folio',
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 11, color: palette.faint)),
        ],
      ),
    );
  }
}

/// A wide, quiet click target at each edge — the way you turn a page without
/// aiming at a button.
class _EdgeNav extends StatefulWidget {
  const _EdgeNav({
    required this.icon,
    required this.enabled,
    required this.onTap,
  });

  final IconData icon;
  final bool enabled;
  final VoidCallback onTap;

  @override
  State<_EdgeNav> createState() => _EdgeNavState();
}

class _EdgeNavState extends State<_EdgeNav> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    if (!widget.enabled) return const SizedBox.shrink();
    return MouseRegion(
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedOpacity(
          opacity: _hover ? 1 : 0,
          duration: const Duration(milliseconds: 160),
          child: Center(
            child: Container(
              width: 38,
              height: 38,
              decoration: const BoxDecoration(
                color: Color(0x33000000),
                shape: BoxShape.circle,
              ),
              child: Icon(widget.icon, color: Colors.white, size: 22),
            ),
          ),
        ),
      ),
    );
  }
}

/// Scrubber, chapter steps, and where you are.
class _Footer extends StatefulWidget {
  const _Footer({
    required this.controller,
    required this.reader,
    required this.palette,
  });

  final BooksController controller;
  final Reader reader;
  final ({Color page, Color ink, Color faint}) palette;

  @override
  State<_Footer> createState() => _FooterState();
}

class _FooterState extends State<_Footer> {
  double? _dragging;

  @override
  Widget build(BuildContext context) {
    final r = widget.reader;
    final pal = widget.palette;
    final max = r.pageCount.clamp(1, 1 << 30).toDouble();
    final at = (_dragging ?? r.page.toDouble()).clamp(1, max);

    return Container(
      height: 54,
      padding: const EdgeInsets.symmetric(horizontal: 18),
      decoration: BoxDecoration(
        color: pal.page,
        border:
            Border(top: BorderSide(color: pal.faint.withValues(alpha: 0.25))),
      ),
      child: Row(
        children: [
          if (!r.imageMode) ...[
            IconButton(
              tooltip: 'Previous chapter',
              iconSize: 18,
              icon: Icon(Icons.first_page, color: pal.ink),
              onPressed: () =>
                  widget.controller.send(const BooksCmd.readerPrevChapter()),
            ),
          ],
          Text('${at.round()}',
              style: TextStyle(fontSize: 12, color: pal.faint)),
          Expanded(
            child: SliderTheme(
              data: SliderTheme.of(context).copyWith(
                trackHeight: 3,
                thumbShape: const RoundSliderThumbShape(enabledThumbRadius: 6),
                activeTrackColor: Tokens.secBooks,
                inactiveTrackColor: pal.faint.withValues(alpha: 0.3),
                thumbColor: Tokens.secBooks,
              ),
              // Commits on release: jumping a PDF sixty times a second would
              // render sixty pages that nobody sees.
              child: Slider(
                value: at.toDouble(),
                min: 1,
                max: max,
                onChanged: (v) => setState(() => _dragging = v),
                onChangeEnd: (v) {
                  setState(() => _dragging = null);
                  widget.controller.send(BooksCmd.readerJump(page: v.round()));
                },
              ),
            ),
          ),
          Text('${r.pageCount}',
              style: TextStyle(fontSize: 12, color: pal.faint)),
          const SizedBox(width: 12),
          Text('${(r.percent * 100).round()}%',
              style: const TextStyle(
                  fontSize: 12,
                  fontWeight: FontWeight.w700,
                  color: Tokens.secBooks)),
          if (!r.imageMode)
            IconButton(
              tooltip: r.nextChapterName.isEmpty
                  ? 'Next chapter'
                  : 'Next: ${r.nextChapterName}',
              iconSize: 18,
              icon: Icon(Icons.last_page, color: pal.ink),
              onPressed: () =>
                  widget.controller.send(const BooksCmd.readerNextChapter()),
            ),
        ],
      ),
    );
  }
}

/// Whichever panel the chrome has open, docked to the left.
class _SidePanel extends StatelessWidget {
  const _SidePanel({required this.controller, required this.reader});

  final BooksController controller;
  final Reader reader;

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder<int>(
      valueListenable: _Chrome._panelTick,
      builder: (context, _, __) {
        final which = _Chrome._panel;
        if (which.isEmpty) return const SizedBox.shrink();
        final t = context.tokens;
        return Container(
          width: 320,
          decoration: BoxDecoration(
            color: t.nCard,
            border: Border(right: BorderSide(color: t.nHair)),
          ),
          child: switch (which) {
            'contents' => _Contents(controller: controller, reader: reader),
            'search' => _Search(controller: controller, reader: reader),
            'bookmarks' => _Bookmarks(controller: controller, reader: reader),
            'notes' => _Notes(controller: controller, reader: reader),
            'thumbs' => _Thumbs(controller: controller, reader: reader),
            'type' => _Typography(controller: controller, reader: reader),
            _ => const SizedBox.shrink(),
          },
        );
      },
    );
  }
}

class _PanelHead extends StatelessWidget {
  const _PanelHead({required this.title, this.trailing});

  final String title;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 14, 8, 8),
      child: Row(
        children: [
          Text(title,
              style: TextStyle(
                  fontSize: 14, fontWeight: FontWeight.w700, color: t.nInk)),
          const Spacer(),
          if (trailing != null) trailing!,
        ],
      ),
    );
  }
}

class _Contents extends StatelessWidget {
  const _Contents({required this.controller, required this.reader});

  final BooksController controller;
  final Reader reader;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const _PanelHead(title: 'Contents'),
        Expanded(
          child: reader.toc.isEmpty
              ? Center(
                  child: Text('This book has no table of contents.',
                      style: TextStyle(fontSize: 12, color: t.nInk2)))
              : ListView.builder(
                  itemCount: reader.toc.length,
                  itemBuilder: (_, i) {
                    final e = reader.toc[i];
                    return InkWell(
                      onTap: e.chapter < 0
                          ? null
                          : () => controller
                              .send(BooksCmd.readerChapter(index: e.chapter)),
                      child: Padding(
                        padding:
                            EdgeInsets.fromLTRB(16 + e.depth * 14.0, 8, 12, 8),
                        child: Text(
                          e.label,
                          style: TextStyle(
                            fontSize: e.depth == 0 ? 13 : 12,
                            fontWeight: e.depth == 0
                                ? FontWeight.w600
                                : FontWeight.w400,
                            color: e.chapter < 0 ? t.nInk3 : t.nInk,
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

class _Search extends StatefulWidget {
  const _Search({required this.controller, required this.reader});

  final BooksController controller;
  final Reader reader;

  @override
  State<_Search> createState() => _SearchState();
}

class _SearchState extends State<_Search> {
  late final TextEditingController _q =
      TextEditingController(text: widget.reader.searchQuery);

  @override
  void dispose() {
    _q.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final hits = widget.reader.searchResults;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        const _PanelHead(title: 'Search'),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16),
          child: TextField(
            controller: _q,
            autofocus: true,
            decoration: const InputDecoration(
              isDense: true,
              hintText: 'Find in this book…',
              border: OutlineInputBorder(),
            ),
            onSubmitted: (v) =>
                widget.controller.send(BooksCmd.readerSearch(query: v)),
          ),
        ),
        const SizedBox(height: 8),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16),
          child: Text(
            hits.isEmpty
                ? (widget.reader.searchQuery.isEmpty ? '' : 'No matches.')
                : '${hits.length} match${hits.length == 1 ? '' : 'es'}',
            style: TextStyle(fontSize: 11, color: t.nInk2),
          ),
        ),
        Expanded(
          child: ListView.builder(
            itemCount: hits.length,
            itemBuilder: (_, i) => InkWell(
              onTap: () => widget.controller
                  .send(BooksCmd.readerJump(page: hits[i].page)),
              child: Padding(
                padding: const EdgeInsets.fromLTRB(16, 8, 12, 8),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text('Page ${hits[i].page}',
                        style: const TextStyle(
                            fontSize: 10,
                            fontWeight: FontWeight.w700,
                            color: Tokens.secBooks)),
                    Text('…${hits[i].snippet}…',
                        maxLines: 3,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 12, height: 1.4, color: t.nInk2)),
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

class _Bookmarks extends StatelessWidget {
  const _Bookmarks({required this.controller, required this.reader});

  final BooksController controller;
  final Reader reader;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final marks = reader.bookmarks;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _PanelHead(
          title: 'Bookmarks',
          trailing: IconButton(
            iconSize: 18,
            tooltip:
                reader.bookmarked ? 'Remove this page' : 'Bookmark this page',
            icon: Icon(
                reader.bookmarked ? Icons.bookmark_remove : Icons.bookmark_add),
            onPressed: () => controller.send(const BooksCmd.toggleBookmark()),
          ),
        ),
        Expanded(
          child: marks.isEmpty
              ? Center(
                  child: Text('No bookmarks yet.',
                      style: TextStyle(fontSize: 12, color: t.nInk2)))
              : ListView.builder(
                  itemCount: marks.length,
                  itemBuilder: (_, i) => ListTile(
                    dense: true,
                    leading: const Icon(Icons.bookmark,
                        size: 16, color: Tokens.secBooks),
                    title: Text('Page ${marks[i].page + 1}',
                        style: TextStyle(fontSize: 13, color: t.nInk)),
                    subtitle: marks[i].note.isEmpty
                        ? null
                        : Text(marks[i].note,
                            maxLines: 2, overflow: TextOverflow.ellipsis),
                    onTap: () => controller
                        .send(BooksCmd.bookmarkJump(page: marks[i].page)),
                    trailing: IconButton(
                      iconSize: 15,
                      icon: const Icon(Icons.close),
                      onPressed: () => controller
                          .send(BooksCmd.bookmarkRemove(id: marks[i].id)),
                    ),
                  ),
                ),
        ),
      ],
    );
  }
}

class _Notes extends StatelessWidget {
  const _Notes({required this.controller, required this.reader});

  final BooksController controller;
  final Reader reader;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final notes = reader.notes;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _PanelHead(
          title: 'Notes',
          trailing: IconButton(
            iconSize: 18,
            tooltip: 'Note on this page',
            icon: const Icon(Icons.add),
            onPressed: () async {
              final note = await promptText(
                context,
                title: 'Note on page ${reader.page}',
                label: 'Your note',
                confirm: 'Save',
              );
              if (note != null) {
                await controller.send(BooksCmd.annotAdd(
                  snippet: reader.leftText.isEmpty
                      ? ''
                      : reader.leftText
                          .substring(0, reader.leftText.length.clamp(0, 120)),
                  note: note,
                  color: 'yellow',
                ));
              }
            },
          ),
        ),
        Expanded(
          child: notes.isEmpty
              ? Center(
                  child: Text('No notes yet.',
                      style: TextStyle(fontSize: 12, color: t.nInk2)))
              : ListView.builder(
                  itemCount: notes.length,
                  itemBuilder: (_, i) {
                    final n = notes[i];
                    return InkWell(
                      onTap: () =>
                          controller.send(BooksCmd.bookmarkJump(page: n.page)),
                      child: Padding(
                        padding: const EdgeInsets.fromLTRB(16, 10, 8, 10),
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Row(
                              children: [
                                Text('Page ${n.page + 1}',
                                    style: const TextStyle(
                                        fontSize: 10,
                                        fontWeight: FontWeight.w700,
                                        color: Tokens.secBooks)),
                                const Spacer(),
                                IconButton(
                                  iconSize: 14,
                                  icon: const Icon(Icons.edit_outlined),
                                  onPressed: () async {
                                    final v = await promptText(
                                      context,
                                      title: 'Edit note',
                                      label: 'Note',
                                      initial: n.note,
                                      confirm: 'Save',
                                    );
                                    if (v != null) {
                                      await controller.send(
                                          BooksCmd.annotSetNote(
                                              id: n.id, note: v));
                                    }
                                  },
                                ),
                                IconButton(
                                  iconSize: 14,
                                  icon: const Icon(Icons.close),
                                  onPressed: () => controller
                                      .send(BooksCmd.annotRemove(id: n.id)),
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
                                      color: t.nInk3)),
                            if (n.note.isNotEmpty)
                              Padding(
                                padding: const EdgeInsets.only(top: 4),
                                child: Text(n.note,
                                    style:
                                        TextStyle(fontSize: 12, color: t.nInk)),
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

class _Thumbs extends StatelessWidget {
  const _Thumbs({required this.controller, required this.reader});

  final BooksController controller;
  final Reader reader;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final thumbs = reader.thumbs;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _PanelHead(
          title: 'Pages',
          trailing: IconButton(
            iconSize: 18,
            tooltip: 'Render thumbnails',
            icon: const Icon(Icons.refresh),
            onPressed: () => controller.send(const BooksCmd.loadThumbs()),
          ),
        ),
        Expanded(
          child: thumbs.isEmpty
              ? Center(
                  child: Text('Tap refresh to render the page strip.',
                      textAlign: TextAlign.center,
                      style: TextStyle(fontSize: 12, color: t.nInk2)))
              : GridView.builder(
                  padding: const EdgeInsets.all(12),
                  gridDelegate: const SliverGridDelegateWithFixedCrossAxisCount(
                    crossAxisCount: 3,
                    childAspectRatio: 0.66,
                    mainAxisSpacing: 8,
                    crossAxisSpacing: 8,
                  ),
                  itemCount: thumbs.length,
                  itemBuilder: (_, i) => InkWell(
                    onTap: () => controller
                        .send(BooksCmd.readerJump(page: thumbs[i].page)),
                    child: Column(
                      children: [
                        Expanded(
                          child: Image.file(File(thumbs[i].path),
                              fit: BoxFit.cover,
                              errorBuilder: (_, __, ___) =>
                                  ColoredBox(color: t.nHover)),
                        ),
                        Text('${thumbs[i].page}',
                            style: TextStyle(fontSize: 9, color: t.nInk3)),
                      ],
                    ),
                  ),
                ),
        ),
      ],
    );
  }
}

/// Type size, face, spacing, margins, alignment, page tint and brightness.
class _Typography extends StatelessWidget {
  const _Typography({required this.controller, required this.reader});

  final BooksController controller;
  final Reader reader;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final p = reader.prefs;
    final textBook = !reader.imageMode;

    Widget label(String s) => Padding(
          padding: const EdgeInsets.fromLTRB(16, 14, 16, 6),
          child: Text(s,
              style: TextStyle(
                  fontSize: 11,
                  letterSpacing: 0.8,
                  fontWeight: FontWeight.w700,
                  color: t.nInk2)),
        );

    Widget seg(List<String> options, int active, ValueChanged<int> onPick) =>
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16),
          child: Row(
            children: [
              for (var i = 0; i < options.length; i++)
                Expanded(
                  child: Padding(
                    padding: const EdgeInsets.only(right: 6),
                    child: OutlinedButton(
                      style: OutlinedButton.styleFrom(
                        padding: const EdgeInsets.symmetric(vertical: 8),
                        backgroundColor: active == i
                            ? Tokens.secBooks.withValues(alpha: 0.16)
                            : null,
                        side: BorderSide(
                            color: active == i ? Tokens.secBooks : t.nHair),
                      ),
                      onPressed: () => onPick(i),
                      child: Text(options[i],
                          style: TextStyle(
                              fontSize: 11,
                              color: active == i ? Tokens.secBooks : t.nInk)),
                    ),
                  ),
                ),
            ],
          ),
        );

    return ListView(
      children: [
        const _PanelHead(title: 'Typography'),
        if (textBook) ...[
          label('TEXT SIZE'),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16),
            child: Row(
              children: [
                IconButton(
                  iconSize: 18,
                  icon: const Icon(Icons.text_decrease),
                  onPressed: () =>
                      controller.send(BooksCmd.setFontPx(px: p.fontPx - 1)),
                ),
                Expanded(
                  child: Slider(
                    value: p.fontPx.clamp(12, 32),
                    min: 12,
                    max: 32,
                    divisions: 20,
                    activeColor: Tokens.secBooks,
                    // Every change repaginates the whole book on the Rust
                    // side, so this commits on release rather than per frame.
                    onChanged: (_) {},
                    onChangeEnd: (v) =>
                        controller.send(BooksCmd.setFontPx(px: v)),
                  ),
                ),
                IconButton(
                  iconSize: 18,
                  icon: const Icon(Icons.text_increase),
                  onPressed: () =>
                      controller.send(BooksCmd.setFontPx(px: p.fontPx + 1)),
                ),
              ],
            ),
          ),
          label('TYPEFACE'),
          seg(_typefaces, p.typeface,
              (i) => controller.send(BooksCmd.setTypeface(index: i))),
          label('LINE SPACING'),
          seg(const ['Compact', 'Normal', 'Relaxed'], p.lineIndex,
              (i) => controller.send(BooksCmd.setLineSpacing(index: i))),
          label('MARGINS'),
          seg(const ['Narrow', 'Normal', 'Wide'], p.marginIndex,
              (i) => controller.send(BooksCmd.setMargin(index: i))),
          label('ALIGNMENT'),
          seg(const ['Left', 'Justify', 'Right'], p.align,
              (i) => controller.send(BooksCmd.setAlign(index: i))),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16),
            child: SwitchListTile(
              contentPadding: EdgeInsets.zero,
              dense: true,
              title: const Text('Bolder text'),
              value: p.bold,
              activeThumbColor: Tokens.secBooks,
              onChanged: (_) => controller.send(const BooksCmd.toggleBold()),
            ),
          ),
        ],
        label('PAGE'),
        seg(
            const ['Light', 'Sepia', 'Dark'],
            switch (p.theme) { 'sepia' => 1, 'dark' => 2, _ => 0 },
            (i) => controller.send(BooksCmd.setTheme(
                    theme: switch (i) {
                  1 => 'sepia',
                  2 => 'dark',
                  _ => 'light'
                }))),
        label('BRIGHTNESS'),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16),
          child: Slider(
            value: p.brightness.clamp(0.4, 1.0),
            min: 0.4,
            max: 1.0,
            activeColor: Tokens.secBooks,
            onChanged: (v) => controller.send(BooksCmd.setBrightness(value: v)),
          ),
        ),
        const SizedBox(height: 24),
      ],
    );
  }
}
