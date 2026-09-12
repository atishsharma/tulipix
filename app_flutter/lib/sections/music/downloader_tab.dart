// Downloader — My Music's eighth sub-tab.
//
// Two blocks side by side, which is Slint's layout: the work on the left — a
// header strip carrying the run's counters and its buttons, and under it the
// entries table filling the rest of the height — and a fixed rail on the right
// holding the three forms (where the music comes from, how it lands on disk,
// and the activity log). The Slint page paged the queue at 25 rows because a
// Slint model of 500 is expensive; a `ListView.builder` is not, so the whole
// queue is one list here and the pager is gone with it.

import 'dart:math' as math;

import 'package:flutter/material.dart';
// frb's Int64List, not dart:typed_data's — they are different types and
// `playList` takes the former.
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart' show Int64List;

import '../../design/pick.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/mdl.dart';
import '../../src/rust/api/music.dart';
import 'mdl_controller.dart';
import 'music_controller.dart';
import 'music_widgets.dart';

const Color kMdlAccent = Color(0xFFEC4899);
const Color kMdlAccent2 = Color(0xFF8B5CF6);

class DownloaderTab extends StatefulWidget {
  const DownloaderTab({super.key});

  @override
  State<DownloaderTab> createState() => _DownloaderTabState();
}

class _DownloaderTabState extends State<DownloaderTab> {
  final MdlController _c = MdlController();
  final TextEditingController _url = TextEditingController();

  @override
  void initState() {
    super.initState();
    // Not straight: `initState` runs inside a build and `send` notifies before
    // it awaits, which would mark an already-built listener dirty mid-build.
    WidgetsBinding.instance.addPostFrameCallback((_) => _c.refresh());
    _c.addListener(_adopt);
  }

  /// What the field has sent Rust and not yet seen come back.
  final List<String> _echo = [];

  /// The url of the snapshot `_adopt` last looked at.
  String? _seen;

  /// A keystroke: the field is already right, Rust is told.
  void _type(String v) {
    _echo.add(v);
    if (_echo.length > 64) _echo.removeAt(0);
    _c.send(MdlCmd.setUrl(url: v));
  }

  /// Keep the link field in step with Rust without fighting the cursor.
  ///
  /// It used to copy `st.url` in whenever the two differed. But `send`
  /// notifies before its round trip, with the old snapshot still held, so
  /// every keystroke was written back over by the text from before it — and
  /// `text =` puts the cursor at -1, where nothing typed lands. One character,
  /// then a dead field. Now only a snapshot whose url actually moved, and
  /// that is not our own keystroke coming back, is Rust's news: a history
  /// replay, a mode switch, a clear.
  void _adopt() {
    final url = _c.state?.url;
    if (url == null || url == _seen) return;
    _seen = url;
    final i = _echo.indexOf(url);
    if (i >= 0) {
      _echo.removeRange(0, i + 1);
      return;
    }
    if (_url.text != url) {
      _url.value = TextEditingValue(
        text: url,
        selection: TextSelection.collapsed(offset: url.length),
      );
    }
  }

  @override
  void dispose() {
    _c.removeListener(_adopt);
    _c.dispose();
    _url.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        final st = _c.state;
        if (st == null) {
          return const Center(child: CircularProgressIndicator());
        }
        // Two blocks, as in Slint: the work on the left, the settings on the
        // right. The port stacked all five cards in one scrolling column, so
        // the queue — the thing you are actually reading — started below the
        // fold behind two forms you had already filled in, and the table never
        // got more than the height of one screenful of rows.
        //
        // The rail is always 377 wide. Opening the app sidebar takes its width
        // from the queue, which can spare it, never from the forms, whose rows
        // are laid out for exactly this width. It used to be capped at 44.5% of
        // the window too, which is how the Source row ran out of room.
        return Padding(
          padding: const EdgeInsets.all(16),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    _Head(controller: _c, state: st),
                    const SizedBox(height: 14),
                    if (_c.error != null) ...[
                      _Bar(
                        text: '${_c.error}',
                        tint: Tokens.error,
                        onClose: _c.clearError,
                      ),
                      const SizedBox(height: 12),
                    ],
                    if (st.error.isNotEmpty) ...[
                      _Bar(text: st.error, tint: Tokens.error, onClose: null),
                      const SizedBox(height: 12),
                    ],
                    if (st.ytWarn) ...[
                      const _Bar(
                        text: 'That YouTube link carries no album tag, so '
                            'it may be a video rather than a track. It '
                            'will still download — the cover and album '
                            'will just be whatever YouTube had.',
                        tint: Color(0xFFF59E0B),
                        onClose: null,
                      ),
                      const SizedBox(height: 12),
                    ],
                    Expanded(child: _Tables(controller: _c, state: st)),
                  ],
                ),
              ),
              const SizedBox(width: 14),
              SizedBox(
                width: kMdlRailWidth,
                // Source at its own height, and Output taking the rest, so the
                // rail ends exactly where the queue block does. On a window
                // too short for Output's rows they scroll inside its card.
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    _Source(
                        controller: _c, state: st, url: _url, onType: _type),
                    const SizedBox(height: 12),
                    Expanded(child: _Output(controller: _c, state: st)),
                  ],
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}

// ── header ──────────────────────────────────────────────────────────────────

class _Head extends StatelessWidget {
  const _Head({required this.controller, required this.state});

  final MdlController controller;
  final MdlState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final running = state.status == 'downloading';
    final denom = state.selected > 0 ? state.selected : state.rows.length;
    final moved = controller.done + controller.skipped;
    return Container(
      height: 76,
      padding: const EdgeInsets.symmetric(horizontal: 16),
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(
          color: running ? const Color(0xFF22C55E) : t.outline,
          width: 1.5,
        ),
      ),
      child: Row(
        children: [
          Container(
            width: 42,
            height: 42,
            decoration: BoxDecoration(
              gradient: const LinearGradient(colors: [kMdlAccent, kMdlAccent2]),
              borderRadius: BorderRadius.circular(13),
            ),
            child: const Icon(Icons.download, color: Colors.white, size: 21),
          ),
          const SizedBox(width: 12),
          // Expanded, with no Spacer after it: the title takes all the free
          // space, so the controls sit on the right edge. A Flexible title
          // beside a Spacer split that space in two and left the buttons
          // stranded short of the edge.
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Text('Music Downloader',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontFamily: Tokens.fontFamily,
                        fontSize: 18,
                        fontWeight: FontWeight.w800,
                        color: t.text)),
                const SizedBox(height: 2),
                Text(
                  state.status == 'resolving'
                      ? 'Resolving…'
                      : state.rows.isEmpty
                          ? 'Paste a link or search — tracks land in your '
                              'library with full metadata'
                          : '${state.title.isEmpty ? "" : "${state.title}  ·  "}'
                              '${state.rows.length} tracks resolved',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11.5, color: t.textDim),
                ),
              ],
            ),
          ),
          const SizedBox(width: 12),
          _Counters(state: state, controller: controller),
          const SizedBox(width: 10),
          // The run's progress, over what is *selected* rather than over the
          // whole queue — the selection is what will actually download.
          _DlPill(
            active: running,
            frac: denom > 0 ? (moved / denom).clamp(0.0, 1.0) : 0,
            fill: denom > 0 && moved >= denom
                ? const Color(0xFF22C55E)
                : kMdlAccent2,
            label: '$moved / $denom '
                '${running ? "downloading" : "downloaded"}',
          ),
          const SizedBox(width: 10),
          FilledButton.icon(
            style: FilledButton.styleFrom(
              backgroundColor: const Color(0xFF22C55E),
              foregroundColor: const Color(0xFF07120B),
            ),
            onPressed: state.rows.isEmpty || running
                ? null
                : () {
                    controller.setPane(MdlPane.queue);
                    controller.send(const MdlCmd.download());
                  },
            icon: const Icon(Icons.download, size: 17),
            label: Text(
                state.selected > 0 ? 'Download ${state.selected}' : 'Download'),
          ),
          const SizedBox(width: 8),
          IconButton(
            tooltip: 'Cancel',
            onPressed: running
                ? () => controller.send(const MdlCmd.cancel())
                : () => controller.send(const MdlCmd.cancelResolve()),
            icon: const Icon(Icons.close, size: 18),
            color: Tokens.error,
          ),
          // Cancel stops what is running and leaves the list, so a stopped
          // download can be picked over and started again. This empties it —
          // stopping first when something is still downloading, since rows
          // cannot go out from under a running job.
          TextButton.icon(
            onPressed: state.rows.isEmpty
                ? null
                : () async {
                    if (running) await controller.send(const MdlCmd.cancel());
                    await controller.send(const MdlCmd.clearAll());
                  },
            icon: const Icon(Icons.clear_all, size: 18),
            label: const Text('Clear queue'),
          ),
          IconButton(
            tooltip: 'Rescan the library',
            // The downloader adds finished tracks itself, so this is for files
            // that arrived some other way — a copy into the watched folder.
            onPressed: () =>
                MusicController.instance.send(const MusicCmd.scan()),
            icon: const Icon(Icons.refresh, size: 18),
            color: t.textDim,
          ),
        ],
      ),
    );
  }
}

/// Four numbers with their own icon, in one pill. Queued, done, skipped,
/// failed — Slint's counter chip, which has height for one line only.
class _Counters extends StatelessWidget {
  const _Counters({required this.state, required this.controller});

  final MdlState state;
  final MdlController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget one(IconData icon, Color tint, int n) => Padding(
          padding: const EdgeInsets.only(right: 12),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(icon, size: 12, color: tint),
              const SizedBox(width: 5),
              Text('$n',
                  style: TextStyle(
                      fontFamily: Tokens.fontFamily,
                      fontSize: 12,
                      fontWeight: FontWeight.w800,
                      color: t.text)),
            ],
          ),
        );
    return Container(
      height: 34,
      padding: const EdgeInsets.only(left: 13, right: 1),
      decoration: BoxDecoration(
        color: t.panel2,
        borderRadius: BorderRadius.circular(17),
        border: Border.all(color: t.outline),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          one(Icons.queue_music, kMdlAccent, state.selected),
          one(Icons.check, const Color(0xFF22C55E), controller.done),
          one(Icons.skip_next, const Color(0xFF64748B), controller.skipped),
          one(Icons.close, const Color(0xFFEF4444), controller.failed),
        ],
      ),
    );
  }
}

/// Slint's `DlPill` — a rounded strip that is its own progress bar, so the
/// number and the fill are the same control rather than a label above a track.
class _DlPill extends StatelessWidget {
  const _DlPill({
    required this.active,
    required this.frac,
    required this.fill,
    required this.label,
  });

  final bool active;
  final double frac;
  final Color fill;
  final String label;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final skin = context.skin;
    final r = BorderRadius.circular(17);
    // Standard draws it plainly. Any other language draws its own resting
    // control as the track and its own latched one, tinted, as the fill —
    // the way its stat cards and tabs are drawn. It used to paint Standard's
    // flat panel and a half-alpha wash in every language, which under
    // Neumorphism or Glass read as a broken control beside real ones.
    final track = skin.isStandard ? null : skin.control(active: false, radius: 17);
    final latched = skin.isStandard
        ? null
        : skin.control(active: true, tint: fill, radius: 17);
    return SizedBox(
      width: 208,
      height: 34,
      child: Stack(
        fit: StackFit.expand,
        children: [
          DecoratedBox(
            decoration: track ??
                BoxDecoration(
                  color: t.panel2,
                  borderRadius: r,
                  border: Border.all(color: active ? fill : t.outline),
                ),
          ),
          // Only the fill is clipped: a language's own decoration may not
          // offer a clip path, and clipping the track would cut its shadows.
          ClipRRect(
            borderRadius: r,
            child: Align(
              alignment: Alignment.centerLeft,
              child: FractionallySizedBox(
                widthFactor: frac.clamp(0.0, 1.0),
                child: latched == null
                    ? ColoredBox(color: fill.withValues(alpha: 0.5))
                    : DecoratedBox(decoration: latched),
              ),
            ),
          ),
          Center(
            child: Text(
              label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontFamily: skin.fontFamily ?? Tokens.fontFamily,
                fontSize: 12,
                fontWeight: FontWeight.w700,
                color: skin.ink ?? t.text,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _Bar extends StatelessWidget {
  const _Bar({required this.text, required this.tint, required this.onClose});

  final String text;
  final Color tint;
  final VoidCallback? onClose;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 10, 6, 10),
      decoration: BoxDecoration(
        color: tint.withValues(alpha: 0.1),
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: tint.withValues(alpha: 0.35)),
      ),
      child: Row(
        children: [
          Icon(Icons.info_outline, size: 16, color: tint),
          const SizedBox(width: 9),
          Expanded(
            child: Text(text,
                style: TextStyle(fontSize: 12.5, color: t.text, height: 1.45)),
          ),
          if (onClose != null)
            IconButton(
              tooltip: 'Dismiss',
              icon: const Icon(Icons.close, size: 16),
              onPressed: onClose,
              visualDensity: VisualDensity.compact,
            ),
        ],
      ),
    );
  }
}

// ── cards ───────────────────────────────────────────────────────────────────

// ── the rail ────────────────────────────────────────────────────────────────
//
// One column of full-width rows. No row sets a width of its own, so nothing in
// it can be pushed past the edge: switches split the width into equal slots,
// chip lines are justified to both edges, and each card ends in one button.

/// The downloader's settings rail, whatever the window does.
const double kMdlRailWidth = 377;

/// A card of the rail: an icon tile, a title and a line under it, then rows.
class _RailCard extends StatelessWidget {
  const _RailCard({
    required this.icon,
    required this.tint,
    required this.title,
    required this.sub,
    required this.children,
    this.fill = false,
  });

  final IconData icon;
  final Color tint;
  final String title;
  final String sub;
  final List<Widget> children;

  /// Take the height offered rather than the content's, scrolling the rows
  /// inside the card when they do not fit.
  final bool fill;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: t.outline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: fill ? MainAxisSize.max : MainAxisSize.min,
        children: [
          Row(
            children: [
              Container(
                width: 30,
                height: 30,
                decoration: BoxDecoration(
                  color: tint.withValues(alpha: 0.16),
                  borderRadius: BorderRadius.circular(9),
                ),
                child: Icon(icon, size: 16, color: tint),
              ),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(title,
                        style: TextStyle(
                            fontSize: 14,
                            fontWeight: FontWeight.w700,
                            color: t.text)),
                    Text(sub,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11.5, color: t.textDim)),
                  ],
                ),
              ),
            ],
          ),
          if (fill)
            Expanded(
              child: SingleChildScrollView(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    for (final c in children) ...[
                      const SizedBox(height: 12),
                      c,
                    ],
                  ],
                ),
              ),
            )
          else
            for (final c in children) ...[const SizedBox(height: 12), c],
        ],
      ),
    );
  }
}

/// A small caps label over a row, with an optional note on its right.
class _Lbl extends StatelessWidget {
  const _Lbl(this.text, {this.trailing});

  final String text;
  final String? trailing;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(bottom: 7),
      child: Row(
        children: [
          Text(
            text.toUpperCase(),
            style: TextStyle(
              fontSize: 10,
              fontWeight: FontWeight.w600,
              letterSpacing: 0.7,
              color: t.textDim,
            ),
          ),
          if (trailing != null) ...[
            const Spacer(),
            Text(trailing!, style: TextStyle(fontSize: 11.5, color: t.nInk3)),
          ],
        ],
      ),
    );
  }
}

/// A switch whose options split the full width equally, so it can never
/// outgrow the card and picking one never moves anything else.
class _FullSeg extends StatelessWidget {
  const _FullSeg({
    required this.options,
    required this.active,
    required this.onPick,
    this.labels,
    this.icons,
    this.accent = false,
    this.mono = false,
  });

  final List<String> options;
  final List<String>? labels;
  final List<IconData>? icons;
  final String active;
  final ValueChanged<String> onPick;

  /// The picked slot in the section's pink rather than a raised chip.
  final bool accent;
  final bool mono;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(3),
      decoration: BoxDecoration(
        color: t.nCanvas,
        borderRadius: BorderRadius.circular(11),
        border: Border.all(color: t.outline),
      ),
      child: Row(
        children: [
          for (var i = 0; i < options.length; i++)
            Expanded(
              child: Padding(
                padding: const EdgeInsets.symmetric(horizontal: 1),
                child: _slot(t, i),
              ),
            ),
        ],
      ),
    );
  }

  Widget _slot(Tokens t, int i) {
    final on = options[i] == active;
    final fg = on ? (accent ? Colors.white : t.text) : t.textDim;
    return InkWell(
      onTap: () => onPick(options[i]),
      borderRadius: BorderRadius.circular(8),
      child: Container(
        height: 30,
        alignment: Alignment.center,
        padding: const EdgeInsets.symmetric(horizontal: 4),
        decoration: on
            ? BoxDecoration(
                color: accent ? kMdlAccent : t.nChip,
                borderRadius: BorderRadius.circular(8),
                border: accent ? null : Border.all(color: t.outlineStrong),
              )
            : null,
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (icons != null) ...[
              Icon(icons![i], size: 14, color: fg),
              const SizedBox(width: 6),
            ],
            Flexible(
              child: Text(
                labels?[i] ?? options[i],
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: mono
                    ? TextStyle(
                        fontFamily: 'monospace',
                        fontSize: 11.5,
                        fontWeight: FontWeight.w600,
                        color: fg)
                    : TextStyle(
                        fontSize: 12.5, fontWeight: FontWeight.w600, color: fg),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

double _textWidth(String s, TextStyle style, TextScaler scaler) {
  final p = TextPainter(
    text: TextSpan(text: s, style: style),
    textDirection: TextDirection.ltr,
    textScaler: scaler,
    maxLines: 1,
  )..layout();
  final w = p.width;
  p.dispose();
  return w;
}

/// Chips that fill every line they are on. Each line takes as many as fit at
/// their natural width, and the room left over is shared out equally — what
/// CSS does for `flex: 1 1 auto` in a wrapping row. `Wrap` cannot stretch its
/// children, so this measures the labels itself.
class _Justified extends StatelessWidget {
  const _Justified({
    required this.labels,
    required this.chip,
    required this.style,
    required this.extra,
    this.gap = 6,
  });

  final List<String> labels;
  final Widget Function(String label) chip;

  /// The label's style, for measuring. Merged over the inherited style the
  /// same way the chip's own `Text` is, or the widths are for another font.
  final TextStyle style;

  /// What a chip adds around its label: padding, border and dot.
  final double extra;
  final double gap;

  @override
  Widget build(BuildContext context) {
    final base = DefaultTextStyle.of(context).style.merge(style);
    final scaler = MediaQuery.textScalerOf(context);
    final natural = [
      for (final l in labels) _textWidth(l, base, scaler) + extra,
    ];
    return LayoutBuilder(
      builder: (context, box) {
        final lines = <List<int>>[];
        var line = <int>[];
        var used = 0.0;
        for (var i = 0; i < natural.length; i++) {
          if (line.isNotEmpty && used + gap + natural[i] > box.maxWidth) {
            lines.add(line);
            line = [];
            used = 0;
          }
          used += (line.isEmpty ? 0 : gap) + natural[i];
          line.add(i);
        }
        if (line.isNotEmpty) lines.add(line);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            for (var li = 0; li < lines.length; li++)
              Padding(
                padding: EdgeInsets.only(top: li == 0 ? 0 : gap),
                child: Row(children: _line(lines[li], natural, box.maxWidth)),
              ),
          ],
        );
      },
    );
  }

  List<Widget> _line(List<int> ids, List<double> natural, double width) {
    final taken =
        ids.fold<double>(0, (s, i) => s + natural[i]) + gap * (ids.length - 1);
    final share = math.max(0.0, (width - taken) / ids.length);
    return [
      for (var j = 0; j < ids.length; j++) ...[
        if (j > 0) SizedBox(width: gap),
        // The last chip takes whatever the rounding left, so every line ends
        // exactly on the card's edge.
        if (j == ids.length - 1)
          Expanded(child: chip(labels[ids[j]]))
        else
          SizedBox(
            width: (natural[ids[j]] + share).floorToDouble(),
            child: chip(labels[ids[j]]),
          ),
      ],
    ];
  }
}

/// A provider a search can run on.
class _ProviderChip extends StatelessWidget {
  const _ProviderChip({
    required this.name,
    required this.on,
    required this.onTap,
  });

  final String name;
  final bool on;
  final VoidCallback onTap;

  /// Horizontal padding, border and dot, for [_Justified.extra].
  static const double extra = 40;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = providerColor(name);
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(999),
      child: Container(
        height: 30,
        alignment: Alignment.center,
        padding: const EdgeInsets.symmetric(horizontal: 11),
        decoration: BoxDecoration(
          color: on ? tint.withValues(alpha: 0.14) : null,
          borderRadius: BorderRadius.circular(999),
          border: Border.all(
            color: on ? tint.withValues(alpha: 0.7) : t.outlineStrong,
          ),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              width: 7,
              height: 7,
              decoration: BoxDecoration(color: tint, shape: BoxShape.circle),
            ),
            const SizedBox(width: 7),
            Flexible(
              child: Text(
                name,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                  color: on ? t.text : t.nInk3,
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

/// A provider a link can come from, lit when the pasted link is one of its.
class _WorksWith extends StatelessWidget {
  const _WorksWith({required this.name, required this.lit});

  final String name;
  final bool lit;

  static const double extra = 30;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = providerColor(name);
    return Container(
      height: 24,
      alignment: Alignment.center,
      padding: const EdgeInsets.symmetric(horizontal: 8),
      decoration: BoxDecoration(
        color: lit ? tint.withValues(alpha: 0.12) : t.nCanvas,
        borderRadius: BorderRadius.circular(999),
        border: Border.all(
          color: lit ? tint.withValues(alpha: 0.6) : t.outline,
        ),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 5,
            height: 5,
            decoration: BoxDecoration(color: tint, shape: BoxShape.circle),
          ),
          const SizedBox(width: 5),
          Flexible(
            child: Text(
              name,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(fontSize: 11, color: lit ? t.text : t.textDim),
            ),
          ),
        ],
      ),
    );
  }
}

/// The card's one action. While a link resolves it becomes its own progress
/// bar, with the Stop button beside it rather than a second row.
class _GoButton extends StatelessWidget {
  const _GoButton({
    required this.busy,
    required this.icon,
    required this.label,
    required this.onTap,
  });

  final bool busy;
  final IconData icon;
  final String label;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final fg = busy ? t.text : Colors.white;
    return Material(
      color: busy ? t.panel2 : kMdlAccent,
      clipBehavior: Clip.antiAlias,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(12),
        side: BorderSide(color: busy ? t.outlineStrong : kMdlAccent),
      ),
      child: InkWell(
        onTap: onTap,
        child: SizedBox(
          height: 42,
          child: Stack(
            children: [
              Center(
                child: Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 12),
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Icon(icon, size: 17, color: fg),
                      const SizedBox(width: 8),
                      Flexible(
                        child: Text(
                          label,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            fontSize: 13.5,
                            fontWeight: FontWeight.w700,
                            color: fg,
                          ),
                        ),
                      ),
                      if (!busy) ...[
                        const SizedBox(width: 8),
                        Container(
                          padding: const EdgeInsets.symmetric(
                              horizontal: 5, vertical: 1),
                          decoration: BoxDecoration(
                            borderRadius: BorderRadius.circular(5),
                            border: Border.all(
                                color: Colors.white.withValues(alpha: 0.4)),
                          ),
                          child: Text(
                            'Enter',
                            style: TextStyle(
                              fontFamily: 'monospace',
                              fontSize: 10.5,
                              color: Colors.white.withValues(alpha: 0.75),
                            ),
                          ),
                        ),
                      ],
                    ],
                  ),
                ),
              ),
              if (busy)
                const Positioned(
                  left: 0,
                  right: 0,
                  bottom: 0,
                  child: LinearProgressIndicator(
                    minHeight: 3,
                    color: kMdlAccent,
                    backgroundColor: Colors.transparent,
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Where the music comes from: a link to resolve, or a search to run.
///
/// Top to bottom: the Link/Search switch, the field, the providers, one
/// button. It used to put the field, a five-provider switch and the button on
/// one row, which needed more than the rail's width and pushed the button
/// past its edge.
class _Source extends StatefulWidget {
  const _Source({
    required this.controller,
    required this.state,
    required this.url,
    required this.onType,
  });

  final MdlController controller;
  final MdlState state;
  final TextEditingController url;

  /// Every change to the field goes through here, so the tab can tell its
  /// own keystrokes apart from Rust moving the url.
  final ValueChanged<String> onType;

  @override
  State<_Source> createState() => _SourceState();
}

class _SourceState extends State<_Source> {
  void _go() {
    final c = widget.controller;
    // What comes back lands in the queue, so that is the list to be looking
    // at, whichever tab was open.
    c.setPane(MdlPane.queue);
    widget.onType(widget.url.text);
    c.send(widget.state.mode == 'search'
        ? const MdlCmd.search()
        : const MdlCmd.resolve());
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.state;
    final c = widget.controller;
    final searching = st.mode == 'search';
    final running = c.status == 'resolving';
    final border = OutlineInputBorder(
      borderRadius: BorderRadius.circular(12),
      borderSide: BorderSide(color: t.outlineStrong),
    );
    return _RailCard(
      icon: Icons.link,
      tint: kMdlAccent,
      title: 'Source',
      sub: searching
          ? 'Find an artist, song or album'
          : 'Paste a link to an album, playlist or track',
      children: [
        _FullSeg(
          options: const ['url', 'search'],
          labels: const ['Link', 'Search'],
          icons: const [Icons.link, Icons.search],
          active: st.mode,
          accent: true,
          onPick: (v) => c.send(MdlCmd.setMode(mode: v)),
        ),
        TextField(
          controller: widget.url,
          onChanged: widget.onType,
          onSubmitted: (_) => _go(),
          style: TextStyle(fontSize: 13, color: t.text),
          decoration: InputDecoration(
            isDense: true,
            filled: true,
            fillColor: t.nCanvas,
            hintText: searching
                ? 'Artist, song or album'
                : 'https://open.spotify.com/album/…',
            prefixIcon: Icon(searching ? Icons.search : Icons.link, size: 18),
            // No provider badge in here: the provider is lit in "Works with"
            // underneath, and one place saying it is enough.
            suffixIcon: widget.url.text.isEmpty
                ? null
                : IconButton(
                    tooltip: 'Clear',
                    icon: const Icon(Icons.close, size: 16),
                    onPressed: () {
                      widget.url.clear();
                      widget.onType('');
                    },
                  ),
            contentPadding: const EdgeInsets.symmetric(vertical: 14),
            border: border,
            enabledBorder: border,
            focusedBorder: border.copyWith(
              borderSide: const BorderSide(color: kMdlAccent, width: 1.5),
            ),
          ),
        ),
        Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _Lbl(searching ? 'Search on' : 'Works with'),
            if (searching)
              // From the bridge, not from here. The list used to be typed
              // out on both sides, which is how it stayed at two after a
              // third provider learned to search.
              _Justified(
                labels: mdlSearchProviders(),
                style:
                    const TextStyle(fontSize: 12, fontWeight: FontWeight.w600),
                extra: _ProviderChip.extra,
                chip: (name) => _ProviderChip(
                  name: name,
                  on: st.searchProvider == name,
                  onTap: () => c.send(MdlCmd.setSearchProvider(name: name)),
                ),
              )
            else
              _Justified(
                labels: mdlProviders(),
                style: const TextStyle(fontSize: 11),
                extra: _WorksWith.extra,
                gap: 5,
                chip: (name) =>
                    _WorksWith(name: name, lit: st.providerBadge == name),
              ),
          ],
        ),
        Row(
          children: [
            Expanded(
              child: _GoButton(
                busy: running,
                icon: searching ? Icons.search : Icons.playlist_add_check,
                label: running
                    ? st.rows.isEmpty
                        ? 'Resolving…'
                        : 'Resolving · ${st.rows.length} tracks'
                    : searching
                        ? 'Search ${st.searchProvider}'
                        : 'Resolve',
                onTap: running ? null : _go,
              ),
            ),
            if (running) ...[
              const SizedBox(width: 8),
              Tooltip(
                message: 'Stop resolving',
                child: Material(
                  color: Colors.transparent,
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(12),
                    side: BorderSide(
                        color: Tokens.error.withValues(alpha: 0.4)),
                  ),
                  child: InkWell(
                    borderRadius: BorderRadius.circular(12),
                    onTap: () => c.send(const MdlCmd.cancelResolve()),
                    child: const SizedBox(
                      width: 42,
                      height: 42,
                      child: Icon(Icons.stop_rounded,
                          size: 18, color: Tokens.error),
                    ),
                  ),
                ),
              ),
            ],
          ],
        ),
      ],
    );
  }
}

/// How the files land: folder, codec, bitrate, naming, concurrency.
///
/// Every row is full width. They used to share one `Wrap` of controls of
/// different widths, so each format change moved the others around, and the
/// lossless note brought a fixed 260px of its own.
class _Output extends StatelessWidget {
  const _Output({required this.controller, required this.state});

  final MdlController controller;
  final MdlState state;

  /// Bitrate is meaningless for the lossless codecs, so the row goes away
  /// rather than sitting there being ignored.
  static const Set<String> _lossy = {'opus', 'm4a', 'mp3'};

  /// What the chosen naming makes of the first queued track, or just the
  /// extension while the queue is empty.
  static String _example(MdlState st) {
    if (st.rows.isEmpty) return '.${st.format}';
    final r = st.rows.first;
    final artist =
        r.mainArtist.isNotEmpty ? r.mainArtist : r.artists.join(', ');
    final stem = switch (st.nameMethod) {
      'Artists - Song' => '${r.artists.join(', ')} - ${r.title}',
      'Album - Song' => '${r.album} - ${r.title}',
      '## - Artist - Song' => '01 - $artist - ${r.title}',
      'Song' => r.title,
      _ => '$artist - ${r.title}',
    };
    return '$stem.${st.format}';
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = state;
    final c = controller;
    final lossy = _lossy.contains(st.format);
    final folder = st.dest.split('/').where((s) => s.isNotEmpty).lastOrNull;
    return _RailCard(
      fill: true,
      icon: Icons.folder_outlined,
      tint: kMdlAccent2,
      title: 'Output',
      sub: 'How the files land on disk',
      children: [
        Tooltip(
          message: 'Also added to the watched folders, so it survives a rescan',
          child: Container(
            padding: const EdgeInsets.fromLTRB(12, 8, 8, 8),
            decoration: BoxDecoration(
              color: t.nCanvas,
              borderRadius: BorderRadius.circular(12),
              border: Border.all(color: t.outlineStrong),
            ),
            child: Row(
              children: [
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        folder ?? 'No folder chosen',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 13,
                            fontWeight: FontWeight.w600,
                            color: t.text),
                      ),
                      if (st.dest.isNotEmpty)
                        Text(
                          st.dest,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontFamily: 'monospace',
                              fontSize: 11,
                              color: t.textDim),
                        ),
                    ],
                  ),
                ),
                const SizedBox(width: 10),
                Material(
                  color: t.panel2,
                  shape: RoundedRectangleBorder(
                    borderRadius: BorderRadius.circular(9),
                    side: BorderSide(color: t.outlineStrong),
                  ),
                  child: InkWell(
                    borderRadius: BorderRadius.circular(9),
                    onTap: () async {
                      final path = await pickDirectory(initial: st.dest);
                      if (path == null) return;
                      await c.send(MdlCmd.setDest(path: path));
                    },
                    child: Container(
                      height: 30,
                      alignment: Alignment.center,
                      padding: const EdgeInsets.symmetric(horizontal: 11),
                      child: Text('Change',
                          style: TextStyle(
                              fontSize: 12,
                              fontWeight: FontWeight.w600,
                              color: t.text)),
                    ),
                  ),
                ),
              ],
            ),
          ),
        ),
        Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            const _Lbl('Format'),
            _FullSeg(
              options: mdlFormats(),
              labels: [for (final f in mdlFormats()) f.toUpperCase()],
              active: st.format,
              mono: true,
              onPick: (v) => c.send(MdlCmd.setFormat(name: v)),
            ),
          ],
        ),
        if (lossy)
          Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              const _Lbl('Bitrate', trailing: 'kbps'),
              _FullSeg(
                options: const ['96', '128', '192', '256', '320'],
                active: '${st.bitrate}',
                mono: true,
                onPick: (v) =>
                    c.send(MdlCmd.setBitrate(kbps: int.tryParse(v) ?? 128)),
              ),
            ],
          )
        else
          // Say what a lossless container does and does not buy here. The
          // audio is always yt-dlp's bestaudio — Opus or AAC — so FLAC wraps
          // a lossy master in a lossless box: five to ten times the bytes,
          // not one bit more music.
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
            decoration: BoxDecoration(
              color: Tokens.warn.withValues(alpha: 0.09),
              borderRadius: BorderRadius.circular(10),
              border: Border.all(color: Tokens.warn.withValues(alpha: 0.3)),
            ),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Padding(
                  padding: EdgeInsets.only(top: 1),
                  child:
                      Icon(Icons.info_outline, size: 14, color: Tokens.warn),
                ),
                const SizedBox(width: 9),
                Expanded(
                  child: Text.rich(
                    TextSpan(
                      children: [
                        TextSpan(
                          text: st.format.toUpperCase(),
                          style: TextStyle(
                              color: t.text, fontWeight: FontWeight.w700),
                        ),
                        const TextSpan(
                          text: ' is lossless, but YouTube audio is Opus or '
                              'AAC. Converting it takes 5–10 times the space '
                              'and adds no quality.',
                        ),
                      ],
                    ),
                    style: TextStyle(
                        fontSize: 11.5, color: t.nInk3, height: 1.45),
                  ),
                ),
              ],
            ),
          ),
        Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            const _Lbl('File name'),
            PopupMenuButton<String>(
              tooltip: 'How downloaded files are named',
              initialValue: st.nameMethod,
              onSelected: (v) => c.send(MdlCmd.setNameMethod(label: v)),
              itemBuilder: (context) => [
                for (final m in mdlNameMethods())
                  PopupMenuItem(value: m, child: Text(m)),
              ],
              // Two rows: the choice, and under it what the choice makes of
              // the first queued track, so picking another shows the change.
              child: Container(
                padding: const EdgeInsets.fromLTRB(12, 8, 10, 8),
                decoration: BoxDecoration(
                  color: t.nCanvas,
                  borderRadius: BorderRadius.circular(10),
                  border: Border.all(color: t.outlineStrong),
                ),
                child: Row(
                  children: [
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            st.nameMethod,
                            style: TextStyle(
                                fontSize: 13,
                                fontWeight: FontWeight.w600,
                                color: t.text),
                          ),
                          const SizedBox(height: 3),
                          Text(
                            _example(st),
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                                fontFamily: 'monospace',
                                fontSize: 11,
                                color: t.textDim),
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(width: 8),
                    Icon(Icons.expand_more, size: 18, color: t.textDim),
                  ],
                ),
              ),
            ),
          ],
        ),
        Row(
          children: [
            Expanded(
              child: _Stepper(
                label: 'Parallel',
                value: st.parallel,
                min: 1,
                max: 4,
                onSet: (v) => c.send(MdlCmd.setParallel(n: v)),
              ),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: _Stepper(
                label: 'Threads',
                value: st.threads,
                min: 1,
                max: 8,
                onSet: (v) => c.send(MdlCmd.setThreads(n: v)),
              ),
            ),
          ],
        ),
      ],
    );
  }
}

/// − value +, as wide as its column.
class _Stepper extends StatelessWidget {
  const _Stepper({
    required this.label,
    required this.value,
    required this.min,
    required this.max,
    required this.onSet,
  });

  final String label;
  final int value;
  final int min;
  final int max;
  final ValueChanged<int> onSet;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget key(IconData icon, bool enabled, VoidCallback onTap) => InkWell(
          onTap: enabled ? onTap : null,
          borderRadius: BorderRadius.circular(9),
          child: SizedBox(
            width: 34,
            height: 38,
            child:
                Icon(icon, size: 15, color: enabled ? kMdlAccent : t.outline),
          ),
        );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _Lbl(label),
        Container(
          height: 38,
          decoration: BoxDecoration(
            color: t.nCanvas,
            borderRadius: BorderRadius.circular(10),
            border: Border.all(color: t.outlineStrong),
          ),
          child: Row(
            children: [
              key(Icons.remove, value > min, () => onSet(value - 1)),
              Expanded(
                child: Text(
                  '$value',
                  textAlign: TextAlign.center,
                  style: TextStyle(
                    fontSize: 13.5,
                    fontWeight: FontWeight.w700,
                    color: t.text,
                    fontFeatures: const [FontFeature.tabularFigures()],
                  ),
                ),
              ),
              key(Icons.add, value < max, () => onSet(value + 1)),
            ],
          ),
        ),
      ],
    );
  }
}

class _Segmented extends StatelessWidget {
  const _Segmented({
    required this.options,
    required this.labels,
    required this.active,
    required this.onPick,
  });

  final List<String> options;
  final List<String> labels;
  final String active;
  final ValueChanged<String> onPick;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(3),
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(9),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (var i = 0; i < options.length; i++)
            InkWell(
              onTap: () => onPick(options[i]),
              borderRadius: BorderRadius.circular(7),
              child: Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 12, vertical: 5),
                decoration: BoxDecoration(
                  color: active == options[i] ? kMdlAccent : null,
                  borderRadius: BorderRadius.circular(7),
                ),
                child: Text(
                  labels[i],
                  style: TextStyle(
                    fontSize: 11.5,
                    fontWeight: FontWeight.w700,
                    color: active == options[i] ? Colors.white : t.textDim,
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

// ── the three tables ────────────────────────────────────────────────────────

/// Queue · Downloaded · Searches. One card, three panes — the same arrangement
/// the Slint page uses, so nothing about the downloader covers the downloader.
///
/// One row across its top: what you do to the list on the left — select,
/// sort — and the three tabs on the right edge. No title: the lit tab
/// already says which list this is. The activity log runs along its foot.
class _Tables extends StatefulWidget {
  const _Tables({required this.controller, required this.state});

  final MdlController controller;
  final MdlState state;

  @override
  State<_Tables> createState() => _TablesState();
}

class _TablesState extends State<_Tables> {
  bool _log = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = widget.controller;
    final st = widget.state;
    return Container(
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: t.outline),
      ),
      clipBehavior: Clip.antiAlias,
      child: LayoutBuilder(
        builder: (context, box) => Stack(
          children: [
            Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 12, 16, 10),
                  child: Row(
                    children: [
                      Expanded(
                        child: c.pane == MdlPane.queue && st.rows.isNotEmpty
                            ? _QueueBar(controller: c, state: st)
                            : const SizedBox.shrink(),
                      ),
                      const SizedBox(width: 12),
                      _Segmented(
                        options: const ['queue', 'downloaded', 'searches'],
                        labels: [
                          'Queue${st.rows.isEmpty ? "" : " ${st.rows.length}"}',
                          'Downloaded',
                          'Searches',
                        ],
                        active: c.pane.name,
                        onPick: (v) => c.setPane(MdlPane.values.byName(v)),
                      ),
                    ],
                  ),
                ),
                Expanded(
                  child: Padding(
                    padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
                    child: switch (c.pane) {
                      MdlPane.queue => _Queue(controller: c, state: st),
                      MdlPane.downloaded => _History(controller: c, state: st),
                      MdlPane.searches => _Searches(controller: c, state: st),
                    },
                  ),
                ),
                _ActivityRow(
                  controller: c,
                  open: _log,
                  onTap: () => setState(() => _log = !_log),
                ),
              ],
            ),
            // Over the list, not in place of it, and never more than 15% of
            // the block: the queue it is reporting on stays in view.
            if (_log)
              Positioned(
                left: 0,
                right: 0,
                bottom: _ActivityRow.height,
                height: box.maxHeight * 0.15,
                child: _ActivityLog(controller: c),
              ),
          ],
        ),
      ),
    );
  }
}

class _Queue extends StatelessWidget {
  const _Queue({required this.controller, required this.state});

  final MdlController controller;
  final MdlState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = state;
    if (st.rows.isEmpty) {
      return const MusicEmpty(
        icon: Icons.download_outlined,
        title: 'Nothing queued',
        body: 'Paste an album, playlist or track link above — or switch to '
            'Search and type what you want. Everything that comes back lands '
            'here for you to pick over before anything is downloaded.',
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Select and sort are in the block's top row, beside the tabs.
        // The card is as tall as the column now, so the list takes what is
        // left of it. It used to be capped at 520 and shrink-wrapped, because
        // it sat in a page-length scroll view where an unbounded list would
        // have made the page taller than the window.
        Expanded(
          child: ListView.separated(
            itemCount: st.rows.length,
            separatorBuilder: (_, __) => Divider(height: 1, color: t.nHair),
            itemBuilder: (context, i) => _Row(
              controller: controller,
              row: st.rows[i],
              index: i,
              allArtists: st.allArtists,
            ),
          ),
        ),
      ],
    );
  }
}

/// Select-all, the bulk primary-artist menu, the sort keys, and Download.
class _QueueBar extends StatelessWidget {
  const _QueueBar({required this.controller, required this.state});

  final MdlController controller;
  final MdlState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = state;
    final all = st.selected == st.rows.length;
    return Wrap(
      spacing: 10,
      runSpacing: 8,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        TextButton.icon(
          onPressed: () => controller.send(MdlCmd.selectAll(all: !all)),
          icon: Icon(all ? Icons.deselect : Icons.select_all, size: 16),
          label: Text(all ? 'None' : 'All'),
        ),
        Text('${st.selected} of ${st.rows.length} selected',
            style: TextStyle(fontSize: 12, color: t.textDim)),
        for (final key in const ['name', 'length', 'artist'])
          MusicChip(
            label: switch (key) {
              'name' => 'Title',
              'length' => 'Length',
              _ => 'Artist',
            },
            active: st.sort == key,
            // The direction is only meaningful on the active key, so it only
            // shows there.
            icon: st.sort != key
                ? null
                : st.sortDir == 1
                    ? Icons.arrow_upward
                    : Icons.arrow_downward,
            onTap: () => controller.send(MdlCmd.sortBy(key: key)),
          ),
        // Download and Cancel are in the header strip, with the counters and
        // the progress they belong to. This row is what you do *to the list*.
      ],
    );
  }
}

class _Row extends StatelessWidget {
  const _Row({
    required this.controller,
    required this.row,
    required this.index,
    required this.allArtists,
  });

  final MdlController controller;
  final MdlRow row;
  final int index;
  final List<String> allArtists;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = stageColor(row.stage);
    final active = row.percent > 0 && row.percent < 100;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 7),
      child: Row(
        children: [
          Checkbox(
            value: row.selected,
            activeColor: kMdlAccent,
            visualDensity: VisualDensity.compact,
            onChanged: (_) => controller.send(MdlCmd.toggleRow(index: index)),
          ),
          _Art(url: row.artUrl),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(row.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w600,
                        color: t.text)),
                const SizedBox(height: 2),
                Text(
                  [
                    if (row.mainArtist.isNotEmpty) row.mainArtist,
                    if (row.album.isNotEmpty) row.album,
                    if (row.length.isNotEmpty) row.length,
                  ].join(' · '),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11.5, color: t.textDim),
                ),
                if (row.file.isNotEmpty) ...[
                  const SizedBox(height: 2),
                  Text(row.file,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 11,
                          color: row.stage == 'failed' ? Tokens.error : tint)),
                ],
              ],
            ),
          ),
          const SizedBox(width: 10),
          _ArtistMenu(controller: controller, row: row, index: index),
          const SizedBox(width: 10),
          SizedBox(
            width: 96,
            child: active
                ? LinearProgressIndicator(
                    value: row.percent / 100,
                    color: tint,
                    backgroundColor: tint.withValues(alpha: 0.18),
                  )
                : _StagePill(stage: row.stage, tint: tint),
          ),
          if (row.stage == 'failed')
            IconButton(
              tooltip: 'Try again',
              icon: const Icon(Icons.refresh, size: 17),
              onPressed: () => controller.send(MdlCmd.retry(index: index)),
            ),
        ],
      ),
    );
  }
}

class _StagePill extends StatelessWidget {
  const _StagePill({required this.stage, required this.tint});

  final String stage;
  final Color tint;

  @override
  Widget build(BuildContext context) => Container(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
        decoration: BoxDecoration(
          color: tint.withValues(alpha: 0.13),
          borderRadius: BorderRadius.circular(999),
          border: Border.all(color: tint.withValues(alpha: 0.4)),
        ),
        child: Text(stage,
            textAlign: TextAlign.center,
            style: TextStyle(
                fontSize: 10.5, fontWeight: FontWeight.w700, color: tint)),
      );
}

/// Which of a track's credited artists the library should file it under. The
/// full credit list still goes into the file's own tags.
class _ArtistMenu extends StatelessWidget {
  const _ArtistMenu({
    required this.controller,
    required this.row,
    required this.index,
  });

  final MdlController controller;
  final MdlRow row;
  final int index;

  /// A box beside the stage, as in Slint's queue row: 156 wide, a menu of
  /// the credited artists when there is more than one, and the name alone
  /// when there is not — so every row shows who it will be filed under.
  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final many = row.artists.length > 1;
    final box = Container(
      width: 156,
      height: 32,
      padding: const EdgeInsets.symmetric(horizontal: 10),
      decoration: BoxDecoration(
        color: t.nCanvas,
        borderRadius: BorderRadius.circular(9),
        border: Border.all(color: t.outline),
      ),
      // Centred either way, so a column of these reads as one column. The
      // arrow's width is mirrored on the left, or a menu's name would sit
      // eight pixels left of a plain one's.
      child: Row(
        children: [
          if (many) const SizedBox(width: 16),
          Expanded(
            child: Text(
              row.mainArtist.isEmpty ? '—' : row.mainArtist,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 12, color: t.text),
            ),
          ),
          if (many) Icon(Icons.expand_more, size: 16, color: t.textDim),
        ],
      ),
    );
    if (!many) return box;
    return PopupMenuButton<String>(
      tooltip: 'File under',
      initialValue: row.mainArtist,
      onSelected: (v) =>
          controller.send(MdlCmd.setMainArtist(index: index, artist: v)),
      itemBuilder: (context) => [
        for (final a in row.artists) PopupMenuItem(value: a, child: Text(a)),
      ],
      child: box,
    );
  }
}

/// Cover art, straight from the provider's URL. Flutter caches it, so the
/// bytes never cross the bridge.
class _Art extends StatelessWidget {
  const _Art({required this.url});

  final String url;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final plate = Container(
      width: 34,
      height: 34,
      decoration: BoxDecoration(
        color: t.nTile,
        borderRadius: BorderRadius.circular(6),
      ),
      child: Icon(Icons.music_note, size: 16, color: t.textDim),
    );
    if (url.isEmpty) return plate;
    return ClipRRect(
      borderRadius: BorderRadius.circular(6),
      child: Image.network(
        url,
        width: 34,
        height: 34,
        fit: BoxFit.cover,
        errorBuilder: (_, __, ___) => plate,
      ),
    );
  }
}

class _History extends StatelessWidget {
  const _History({required this.controller, required this.state});

  final MdlController controller;
  final MdlState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = state;
    if (st.history.isEmpty) {
      return const MusicEmpty(
        icon: Icons.history,
        title: 'Nothing downloaded yet',
        body: 'Every track this downloader finishes is logged here with where '
            'it came from and where it landed.',
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Scrolls inside the card. The pane is as tall as the column now, and
        // twenty-five rows plus a pager is taller than that on a short window.
        Expanded(
          child: ListView(
            padding: EdgeInsets.zero,
            children: [
              for (final r in st.history)
                ListTile(
                  dense: true,
                  contentPadding: EdgeInsets.zero,
                  title: Text(r.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 13, color: t.text)),
                  subtitle: Text(
                    [
                      r.artists,
                      if (r.album.isNotEmpty) r.album,
                      if (r.provider.isNotEmpty) r.provider,
                      r.when,
                    ].join(' · '),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11.5, color: t.textDim),
                  ),
                  trailing: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      IconButton(
                        tooltip: 'Play',
                        icon: const Icon(Icons.play_arrow, size: 18),
                        // -1 means the file is no longer in the library — the row
                        // stays, but there is nothing to play.
                        onPressed: r.itemId < 0
                            ? null
                            : () => MusicController.instance.send(
                                  MusicCmd.playList(
                                    itemIds: Int64List.fromList([r.itemId]),
                                    index: 0,
                                    source: 'downloads',
                                  ),
                                ),
                      ),
                      IconButton(
                        tooltip: 'Show the file',
                        icon: const Icon(Icons.folder_open, size: 18),
                        onPressed: () =>
                            controller.send(MdlCmd.revealFile(path: r.path)),
                      ),
                    ],
                  ),
                ),
            ],
          ),
        ),
        const SizedBox(height: 6),
        Row(
          children: [
            Pager(
              page: st.historyPage,
              pages: st.historyPages,
              onGo: (p) => controller.send(MdlCmd.loadHistory(page: p)),
            ),
            const Spacer(),
            TextButton(
              onPressed: () => controller.send(const MdlCmd.clearHistory()),
              child: const Text('Clear log'),
            ),
          ],
        ),
      ],
    );
  }
}

class _Searches extends StatelessWidget {
  const _Searches({required this.controller, required this.state});

  final MdlController controller;
  final MdlState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = state;
    if (st.searches.isEmpty) {
      return const MusicEmpty(
        icon: Icons.manage_search,
        title: 'No searches yet',
        body: 'Every link you resolve is remembered here, so getting back to '
            'an album you looked at last week is one tap.',
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(
          child: ListView(
            padding: EdgeInsets.zero,
            children: [
              for (final r in st.searches)
                ListTile(
                  dense: true,
                  contentPadding: EdgeInsets.zero,
                  onTap: () => controller.send(MdlCmd.useSearch(url: r.url)),
                  title: Text(r.title.isEmpty ? r.url : r.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 13, color: t.text)),
                  subtitle: Text(
                    [
                      r.kind,
                      if (r.provider.isNotEmpty) r.provider,
                      r.when,
                    ].join(' · '),
                    style: TextStyle(fontSize: 11.5, color: t.textDim),
                  ),
                  trailing: const Icon(Icons.north_east, size: 16),
                ),
            ],
          ),
        ),
        const SizedBox(height: 6),
        Row(
          children: [
            Pager(
              page: st.searchPage,
              pages: st.searchPages,
              onGo: (p) => controller.send(MdlCmd.loadSearches(page: p)),
            ),
            const Spacer(),
            TextButton(
              onPressed: () => controller.send(const MdlCmd.clearSearches()),
              child: const Text('Clear log'),
            ),
          ],
        ),
      ],
    );
  }
}

// ── activity log ────────────────────────────────────────────────────────────

/// What it is doing, in the words yt-dlp and the provider used: one row along
/// the foot of the queue block with the last line showing. This is the thing
/// you open when a track failed and the pill only had room for the first
/// eighty characters of why. A green dot says something is running right now.
class _ActivityRow extends StatelessWidget {
  const _ActivityRow({
    required this.controller,
    required this.open,
    required this.onTap,
  });

  final MdlController controller;
  final bool open;
  final VoidCallback onTap;

  static const double height = 40;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = controller;
    final lines = c.cli;
    final live = c.status == 'downloading' || c.status == 'resolving';
    const green = Color(0xFF22C55E);
    return InkWell(
      onTap: onTap,
      child: Container(
        height: height,
        padding: const EdgeInsets.only(left: 16, right: 10),
        decoration: BoxDecoration(
          border: Border(top: BorderSide(color: t.outline)),
        ),
        child: Row(
          children: [
            Container(
              width: 8,
              height: 8,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: live ? green : t.outlineStrong,
                boxShadow: live
                    ? [
                        BoxShadow(
                          color: green.withValues(alpha: 0.5),
                          blurRadius: 6,
                        ),
                      ]
                    : null,
              ),
            ),
            const SizedBox(width: 10),
            Text('Activity',
                style: TextStyle(
                    fontSize: 12.5,
                    fontWeight: FontWeight.w700,
                    color: t.text)),
            const SizedBox(width: 12),
            Expanded(
              child: Text(
                lines.isEmpty ? 'Nothing yet' : lines.last,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontFamily: 'monospace', fontSize: 11, color: t.textDim),
              ),
            ),
            if (open && lines.isNotEmpty)
              TextButton(
                onPressed: c.clearCli,
                style: TextButton.styleFrom(
                  visualDensity: VisualDensity.compact,
                  tapTargetSize: MaterialTapTargetSize.shrinkWrap,
                ),
                child: const Text('Clear'),
              ),
            // It opens upward, over the list.
            Icon(open ? Icons.expand_more : Icons.expand_less,
                size: 18, color: t.textDim),
          ],
        ),
      ),
    );
  }
}

/// The whole log, laid over the bottom of the queue list.
class _ActivityLog extends StatelessWidget {
  const _ActivityLog({required this.controller});

  final MdlController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final lines = controller.cli;
    return Container(
      decoration: BoxDecoration(
        color: t.nCanvas,
        border: Border(top: BorderSide(color: t.outline)),
        boxShadow: const [
          BoxShadow(
              color: Color(0x44000000), blurRadius: 18, offset: Offset(0, -4)),
        ],
      ),
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
      child: lines.isEmpty
          ? Text('Resolve something and the log fills in.',
              style: TextStyle(fontSize: 12, color: t.textDim))
          : ListView.builder(
              reverse: true,
              itemCount: lines.length,
              itemBuilder: (context, i) {
                final line = lines[lines.length - 1 - i];
                return Text(
                  line,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: 11,
                    height: 1.65,
                    fontFamily: 'monospace',
                    color: line.startsWith('✓')
                        ? const Color(0xFF10B981)
                        : t.nInk3,
                  ),
                );
              },
            ),
    );
  }
}
