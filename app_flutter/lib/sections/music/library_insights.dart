// What the library knows about itself: where the listening went, and which
// recordings it holds twice. Both open over three quarters of the window.
//
// Stats Center shows everything at once, with no scrolling: each list takes as
// many rows as its panel has room for. Duplicates has a Run button that
// fingerprints what the analysis pass has not reached, compares the whole
// library, and then lets you review one group at a time, copies side by side.
//
// The Run is not new bridge work. It is `musicAnalyseAll` — whose progress
// already rides the section's scan events — followed by `musicDuplicates`.
// Neither popup asks anything of the network.

import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
// frb's Int64List, not dart:typed_data's — the command constructors take it.
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart' show Int64List;

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import '../../src/rust/api/scrobble.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';
import 'music_widgets.dart';

// ------------------------------------------------------------------ shared --

const Color _pink = Tokens.secMusic;
const Color _violet = Color(0xFF8B5CF6);
const Color _indigo = Color(0xFF6366F1);
const Color _cyan = Color(0xFF22D3EE);
const Color _amber = Color(0xFFFBBF24);
const Color _good = Color(0xFF34D399);
const Color _bad = Color(0xFFF87171);
const Color _other = Color(0xFF5B6068);

/// A popup three quarters of the window each way, centred, so the app still
/// shows around it.
Future<void> _openFull(BuildContext context, Widget child) {
  final t = context.tokens;
  return showDialog<void>(
    context: context,
    builder: (ctx) {
      final size = MediaQuery.sizeOf(ctx);
      return Dialog(
        insetPadding: EdgeInsets.zero,
        backgroundColor: t.nCanvas,
        clipBehavior: Clip.antiAlias,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(20),
          side: BorderSide(color: t.outlineStrong),
        ),
        child: SizedBox(
          width: size.width * 0.75,
          height: size.height * 0.75,
          child: child,
        ),
      );
    },
  );
}

TextStyle _mono(double size, Color color, [FontWeight w = FontWeight.w500]) =>
    TextStyle(
      fontFamily: 'monospace',
      fontSize: size,
      fontWeight: w,
      color: color,
      fontFeatures: const [FontFeature.tabularFigures()],
    );

TextStyle _caps(BuildContext context) => TextStyle(
      fontFamily: Tokens.fontFamily,
      fontSize: 10,
      fontWeight: FontWeight.w600,
      letterSpacing: 0.7,
      color: context.tokens.nInk2,
    );

/// 2184 → "2,184".
String _n(int v) {
  final s = v.abs().toString();
  final b = StringBuffer(v < 0 ? '-' : '');
  for (var i = 0; i < s.length; i++) {
    if (i > 0 && (s.length - i) % 3 == 0) b.write(',');
    b.write(s[i]);
  }
  return b.toString();
}

/// 1,992,294,400 → "1.9 GB".
String _size(int bytes) {
  if (bytes <= 0) return '0 MB';
  const units = ['KB', 'MB', 'GB', 'TB'];
  var v = bytes / 1024;
  var i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return '${v >= 100 ? v.toStringAsFixed(0) : v.toStringAsFixed(1)} ${units[i]}';
}

String _dur(double secs) {
  final s = secs.round();
  return '${s ~/ 60}:${(s % 60).toString().padLeft(2, '0')}';
}

const List<String> _months = [
  'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun',
  'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
];

class _Head extends StatelessWidget {
  const _Head({required this.title, required this.sub, this.actions = const []});

  final String title;
  final String sub;
  final List<Widget> actions;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 62,
      padding: const EdgeInsets.only(left: 22, right: 14),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          Text(
            title,
            style: TextStyle(
              fontFamily: Tokens.fontFamily,
              fontSize: 19,
              fontWeight: FontWeight.w800,
              color: t.nInk,
            ),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              sub,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(fontSize: 12.5, color: t.nInk2),
            ),
          ),
          for (final a in actions) ...[const SizedBox(width: 12), a],
          const SizedBox(width: 12),
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 2),
            decoration: BoxDecoration(
              border: Border.all(color: t.outlineStrong),
              borderRadius: BorderRadius.circular(5),
            ),
            child: Text('Esc', style: _mono(10.5, t.nInk2)),
          ),
          const SizedBox(width: 4),
          IconButton(
            tooltip: 'Close',
            onPressed: () => Navigator.of(context).pop(),
            icon: const Icon(Icons.close, size: 18),
            color: t.nInk3,
          ),
        ],
      ),
    );
  }
}

class _Foot extends StatelessWidget {
  const _Foot({required this.children});

  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 46,
      padding: const EdgeInsets.only(left: 22, right: 14),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: t.nHair)),
      ),
      child: DefaultTextStyle.merge(
        style: TextStyle(fontSize: 12.5, color: t.nInk2),
        child: Row(children: children),
      ),
    );
  }
}

enum _Kind { plain, primary, ghost, danger }

class _Btn extends StatelessWidget {
  const _Btn(
    this.label, {
    this.icon,
    this.onTap,
    this.kind = _Kind.plain,
    this.small = false,
  });

  final String label;
  final IconData? icon;
  final VoidCallback? onTap;
  final _Kind kind;
  final bool small;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final (bg, border, fg) = switch (kind) {
      _Kind.primary => (_pink, _pink, Colors.white),
      _Kind.ghost => (Colors.transparent, Colors.transparent, t.nInk3),
      _Kind.danger => (
          Colors.transparent,
          _bad.withValues(alpha: 0.35),
          _bad,
        ),
      _Kind.plain => (t.nCard, t.outlineStrong, t.nInk),
    };
    final radius = BorderRadius.circular(small ? 8 : 10);
    return Opacity(
      opacity: onTap == null ? 0.5 : 1,
      child: Material(
        color: bg,
        shape: RoundedRectangleBorder(
          borderRadius: radius,
          side: BorderSide(color: border),
        ),
        child: InkWell(
          borderRadius: radius,
          onTap: onTap,
          child: Container(
            height: small ? 28 : 34,
            padding: EdgeInsets.symmetric(horizontal: small ? 10 : 14),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (icon != null) ...[
                  Icon(icon, size: small ? 14 : 16, color: fg),
                  if (label.isNotEmpty) const SizedBox(width: 7),
                ],
                if (label.isNotEmpty)
                  Text(
                    label,
                    style: TextStyle(
                      fontSize: small ? 12 : 13,
                      fontWeight: FontWeight.w600,
                      color: fg,
                    ),
                  ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// A slim, single-line figure: the same card as the Home stat row.
class _Kpi extends StatelessWidget {
  const _Kpi(this.label, this.value, this.tint, {this.delta = ''});

  final String label;
  final String value;
  final Color tint;
  final String delta;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final down = delta.startsWith('−');
    final dc = down ? _bad : _good;
    return Container(
      height: 46,
      padding: const EdgeInsets.symmetric(horizontal: 14),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: t.nHair),
      ),
      child: Row(
        children: [
          Container(
            width: 7,
            height: 7,
            decoration: BoxDecoration(color: tint, shape: BoxShape.circle),
          ),
          const SizedBox(width: 10),
          Text(label.toUpperCase(), style: _caps(context)),
          const SizedBox(width: 10),
          Expanded(
            child: LayoutBuilder(
              builder: (context, box) => Row(
                mainAxisAlignment: MainAxisAlignment.end,
                children: [
                  // The change is the first thing to go on a narrow card;
                  // the figure is what the card is for.
                  if (delta.isNotEmpty && box.maxWidth >= 120) ...[
                    Container(
                      padding: const EdgeInsets.symmetric(
                          horizontal: 6, vertical: 2),
                      decoration: BoxDecoration(
                        color: dc.withValues(alpha: 0.13),
                        borderRadius: BorderRadius.circular(6),
                      ),
                      child:
                          Text(delta, style: _mono(10.5, dc, FontWeight.w600)),
                    ),
                    const SizedBox(width: 8),
                  ],
                  Flexible(
                    child: Text(
                      value.isEmpty ? '—' : value,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontFamily: Tokens.fontFamily,
                        fontSize: 15.5,
                        fontWeight: FontWeight.w800,
                        color: t.nInk,
                        fontFeatures: const [FontFeature.tabularFigures()],
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// One card of the grid: a title line, and the rest of its height for [child].
class _Panel extends StatelessWidget {
  const _Panel({required this.title, required this.child, this.meta});

  final String title;
  final String? meta;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.fromLTRB(16, 13, 16, 14),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: t.nHair),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SizedBox(
            height: 20,
            child: Row(
              children: [
                Text(
                  title,
                  style: TextStyle(
                    fontSize: 13,
                    fontWeight: FontWeight.w700,
                    color: t.nInk,
                  ),
                ),
                const SizedBox(width: 8),
                if (meta != null)
                  Expanded(
                    child: Text(
                      meta!,
                      textAlign: TextAlign.right,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: _mono(11, t.nInk2, FontWeight.w400),
                    ),
                  ),
              ],
            ),
          ),
          const SizedBox(height: 10),
          Expanded(child: child),
        ],
      ),
    );
  }
}

class _Seg<T> extends StatelessWidget {
  const _Seg({required this.options, required this.value, required this.onPick});

  final List<(String, T)> options;
  final T value;
  final ValueChanged<T> onPick;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(3),
      decoration: BoxDecoration(
        color: t.nCanvas,
        borderRadius: BorderRadius.circular(11),
        border: Border.all(color: t.nHair),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (final (label, v) in options)
            InkWell(
              onTap: () => onPick(v),
              borderRadius: BorderRadius.circular(8),
              child: Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
                decoration: v == value
                    ? BoxDecoration(
                        color: t.nChip,
                        borderRadius: BorderRadius.circular(8),
                        border: Border.all(color: t.outlineStrong),
                      )
                    : null,
                child: Text(
                  label,
                  style: TextStyle(
                    fontSize: 12.5,
                    fontWeight: FontWeight.w600,
                    color: v == value ? t.nInk : t.nInk2,
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

// ------------------------------------------------------------ stats center --

/// Where the listening went, and what the library is made of.
Future<void> listeningSummary(BuildContext context, MusicController c) =>
    _openFull(context, _StatsCenter(controller: c, opener: context));

class _StatsCenter extends StatefulWidget {
  const _StatsCenter({required this.controller, required this.opener});

  final MusicController controller;

  /// The page that opened this, which outlives it: "Find duplicates" closes
  /// this popup and opens the next one from there.
  final BuildContext opener;

  @override
  State<_StatsCenter> createState() => _StatsCenterState();
}

class _StatsCenterState extends State<_StatsCenter> {
  static const List<(String, int)> _ranges = [
    ('7 days', 7),
    ('30 days', 30),
    ('12 months', 365),
    ('All time', 0),
  ];

  int _days = 30;
  StatsCenter? _data;
  Object? _error;

  /// Which request is current, so a slow answer for the range you just left
  /// cannot land over the one you picked.
  int _ask = 0;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    final ask = ++_ask;
    try {
      final d = await musicStatsCenter(days: _days);
      if (mounted && ask == _ask) {
        setState(() {
          _data = d;
          _error = null;
        });
      }
    } catch (e) {
      if (mounted && ask == _ask) setState(() => _error = e);
    }
  }

  String get _sub {
    final d = _data;
    if (d == null) return '';
    if (_days == 0) return d.listening.range;
    final now = DateTime.now();
    final from = now.subtract(Duration(days: _days));
    final span = _days > 60
        ? '${_months[from.month - 1]} ${from.year} – '
            '${_months[now.month - 1]} ${now.year}'
        : '${from.day} ${_months[from.month - 1]} – '
            '${now.day} ${_months[now.month - 1]}';
    return '${d.listening.range} · $span';
  }

  @override
  Widget build(BuildContext context) {
    final d = _data;
    return Column(
      children: [
        _Head(
          title: 'Stats',
          sub: _sub,
          actions: [
            _Seg<int>(
              options: _ranges,
              value: _days,
              onPick: (v) {
                if (v == _days) return;
                setState(() => _days = v);
                _load();
              },
            ),
          ],
        ),
        Expanded(
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 14),
            child: _error != null
                ? Center(
                    child: Text('$_error',
                        style: TextStyle(color: context.tokens.nInk2)))
                : d == null
                    ? const Center(child: CircularProgressIndicator())
                    : _StatsGrid(data: d, controller: widget.controller),
          ),
        ),
        _Foot(
          children: [
            // Where the listening is being sent, when anything is queued.
            const ScrobbleRow(),
            // All the free space, so the button sits on the right edge. A
            // Spacer beside a Flexible split it in two and left a gap after.
            const Expanded(
              child: Text(
                'Play history stays on this computer',
                textAlign: TextAlign.right,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
            ),
            const SizedBox(width: 14),
            _Btn(
              'Find duplicates',
              icon: Icons.content_copy_outlined,
              small: true,
              onTap: () {
                Navigator.of(context).pop();
                duplicateFinder(widget.opener, widget.controller);
              },
            ),
          ],
        ),
      ],
    );
  }
}

/// Six figures on top, then two rows of three panels. Every panel is a share
/// of the popup's height rather than a height of its own, so the grid fits the
/// window it is in and nothing scrolls.
class _StatsGrid extends StatelessWidget {
  const _StatsGrid({required this.data, required this.controller});

  final StatsCenter data;
  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final l = data.listening;
    const gap = SizedBox(width: 12);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        SizedBox(
          height: 46,
          child: Row(
            children: [
              Expanded(
                  child: _Kpi('Listening', l.total, _pink, delta: data.delta)),
              gap,
              Expanded(child: _Kpi('Plays', _n(data.plays), _violet)),
              gap,
              Expanded(child: _Kpi('Streak', data.streak, _amber)),
              gap,
              Expanded(child: _Kpi('New artists', _n(data.newArtists), _cyan)),
              gap,
              Expanded(child: _Kpi('Finished', data.finished, _good)),
              gap,
              Expanded(
                child: _Kpi(
                  'Top genre',
                  l.genres.isEmpty ? '—' : l.genres.first.label,
                  _indigo,
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: 12),
        Expanded(
          flex: 118,
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(
                flex: 5,
                child: _Panel(
                  title: 'When you listen',
                  child: _When(data: data),
                ),
              ),
              gap,
              Expanded(
                flex: 4,
                child: _Panel(
                  title: 'Top artists',
                  meta: 'listening time',
                  child: _Ranked(
                    rows: l.artists,
                    leading: (r) => _Initial(r.label),
                    trailing: (r) => r.value,
                    onTap: (r) {
                      if (r.key <= 0) return;
                      Navigator.of(context).pop();
                      controller.send(MusicCmd.openArtist(artistId: r.key));
                    },
                  ),
                ),
              ),
              gap,
              Expanded(
                flex: 3,
                child: _Panel(title: 'Genres', child: _Genres(rows: l.genres)),
              ),
            ],
          ),
        ),
        const SizedBox(height: 12),
        Expanded(
          flex: 100,
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(
                flex: 4,
                child: _Panel(
                  title: 'On repeat',
                  meta: 'plays',
                  child: _Ranked(
                    rows: l.tracks,
                    leading: (r) => MusicArt(
                      controller: controller,
                      kind: 'track',
                      artKey: '${r.key}',
                      size: 26,
                      radius: 6,
                    ),
                    sub: (r) => r.value,
                    trailing: (r) => _n(r.plays),
                    onTap: (r) => controller.send(MusicCmd.playList(
                      itemIds: Int64List.fromList([r.key]),
                      index: 0,
                      source: 'stats',
                    )),
                  ),
                ),
              ),
              gap,
              Expanded(
                flex: 5,
                child: _Panel(
                  title: 'Your library',
                  meta: '${data.length} of music',
                  child: _Library(data: data),
                ),
              ),
              gap,
              Expanded(
                flex: 3,
                child: _Panel(
                  title: 'Never finished',
                  meta: 'times dropped',
                  child: _NeverFinished(rows: l.abandoned),
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// An artist's initial on a colour of its own. There is no artist picture
/// to hand here, and a grey circle on every row reads as a loading state.
class _Initial extends StatelessWidget {
  const _Initial(this.name);

  final String name;

  @override
  Widget build(BuildContext context) {
    final hue = (name.hashCode % 360).abs().toDouble();
    return Container(
      width: 26,
      height: 26,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [
            HSLColor.fromAHSL(1, hue, 0.7, 0.72).toColor(),
            HSLColor.fromAHSL(1, (hue + 40) % 360, 0.6, 0.32).toColor(),
          ],
        ),
      ),
      child: Text(
        name.isEmpty ? '?' : name.characters.first.toUpperCase(),
        style: const TextStyle(
          fontSize: 11,
          fontWeight: FontWeight.w800,
          color: Colors.white,
        ),
      ),
    );
  }
}

/// A ranked list that shows as many rows as fit, never a scrollbar.
class _Ranked extends StatelessWidget {
  const _Ranked({
    required this.rows,
    required this.leading,
    required this.trailing,
    this.sub,
    this.onTap,
  });

  final List<Tally> rows;
  final Widget Function(Tally) leading;
  final String Function(Tally) trailing;

  /// A second line of text; null draws the row's share as a bar instead.
  final String Function(Tally)? sub;
  final void Function(Tally)? onTap;

  static const double _rowH = 38;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (rows.isEmpty) {
      return Align(
        alignment: Alignment.topLeft,
        child: Text('Nothing played in this window.',
            style: TextStyle(fontSize: 12, color: t.nInk2)),
      );
    }
    return LayoutBuilder(
      builder: (context, box) {
        final n =
            math.min(rows.length, math.max(1, (box.maxHeight / _rowH).floor()));
        return Column(
          children: [
            for (var i = 0; i < n; i++)
              SizedBox(
                height: _rowH,
                child: InkWell(
                  borderRadius: BorderRadius.circular(8),
                  onTap: onTap == null ? null : () => onTap!(rows[i]),
                  child: Row(
                    children: [
                      SizedBox(
                        width: 16,
                        child: Text('${i + 1}',
                            textAlign: TextAlign.right,
                            style: _mono(10.5, t.nInk2)),
                      ),
                      const SizedBox(width: 10),
                      leading(rows[i]),
                      const SizedBox(width: 10),
                      Expanded(
                        child: Column(
                          mainAxisAlignment: MainAxisAlignment.center,
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              rows[i].label,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                fontSize: 12.5,
                                fontWeight: FontWeight.w600,
                                color: t.nInk,
                              ),
                            ),
                            const SizedBox(height: 4),
                            if (sub == null)
                              _Share(frac: rows[i].frac)
                            else
                              Text(
                                sub!(rows[i]),
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(fontSize: 11, color: t.nInk2),
                              ),
                          ],
                        ),
                      ),
                      const SizedBox(width: 10),
                      Text(trailing(rows[i]), style: _mono(11.5, t.nInk3)),
                    ],
                  ),
                ),
              ),
          ],
        );
      },
    );
  }
}

class _Share extends StatelessWidget {
  const _Share({required this.frac});

  final double frac;

  @override
  Widget build(BuildContext context) {
    return ClipRRect(
      borderRadius: BorderRadius.circular(2),
      child: SizedBox(
        height: 3,
        child: Stack(
          fit: StackFit.expand,
          children: [
            ColoredBox(color: context.tokens.nChip),
            FractionallySizedBox(
              alignment: Alignment.centerLeft,
              widthFactor: frac.clamp(0.0, 1.0),
              child: const ColoredBox(color: _pink),
            ),
          ],
        ),
      ),
    );
  }
}

/// The 22-week calendar and the 24-hour chart.
class _When extends StatelessWidget {
  const _When({required this.data});

  final StatsCenter data;

  static const double _step = 14;
  static const double _labelW = 26;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final l = data.listening;
    return LayoutBuilder(
      builder: (context, box) {
        // The calendar is the first thing to go on a short window; the hour
        // chart only needs a few pixels to still say something.
        final calendar = box.maxHeight >= 175 && data.days.isNotEmpty;
        final side = box.maxWidth >= 430;
        final weeks = ((box.maxWidth - _labelW - (side ? 140 : 0)) / _step)
            .floor()
            .clamp(6, 22);
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (calendar) ...[
              SizedBox(
                height: 7 * _step + 15,
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    SizedBox(
                      width: _labelW + weeks * _step,
                      child: CustomPaint(
                        painter: _Calendar(
                          days: data.days,
                          weeks: weeks,
                          ink: t.nInk2,
                          empty: t.nChip,
                          today: t.nInk,
                        ),
                      ),
                    ),
                    if (side) ...[
                      const SizedBox(width: 16),
                      Expanded(child: _CalendarSide(days: data.days, weeks: weeks)),
                    ],
                  ],
                ),
              ),
              const SizedBox(height: 12),
            ],
            SizedBox(
              height: 20,
              child: Row(
                children: [
                  Text('By hour',
                      style: TextStyle(
                          fontSize: 13,
                          fontWeight: FontWeight.w700,
                          color: t.nInk)),
                  const Spacer(),
                  Text(
                    l.peakHour.isEmpty ? 'nothing yet' : 'busiest ${l.peakHour}',
                    style: _mono(11, t.nInk2, FontWeight.w400),
                  ),
                ],
              ),
            ),
            const SizedBox(height: 8),
            Expanded(child: _Hours(hours: l.hours)),
            const SizedBox(height: 4),
            Row(
              mainAxisAlignment: MainAxisAlignment.spaceBetween,
              children: [
                for (final h in const ['00', '06', '12', '18', '23'])
                  Text(h, style: _mono(9.5, t.nInk2, FontWeight.w400)),
              ],
            ),
          ],
        );
      },
    );
  }
}

/// Where day `ago` (0 = today) sits on a Monday-first calendar whose last
/// column is this week. Null once it falls off the left edge.
(int col, int row)? _cell(int ago, int weeks, int dow) {
  if (ago <= dow) return (weeks - 1, dow - ago);
  final k = ago - dow - 1; // 0 = last Sunday
  final col = weeks - 2 - k ~/ 7;
  return col < 0 ? null : (col, 6 - k % 7);
}

double _level(double v) => v <= 0.25
    ? 0.28
    : v <= 0.5
        ? 0.52
        : v <= 0.75
            ? 0.76
            : 1.0;

class _Calendar extends CustomPainter {
  _Calendar({
    required this.days,
    required this.weeks,
    required this.ink,
    required this.empty,
    required this.today,
  });

  final List<double> days;
  final int weeks;
  final Color ink;
  final Color empty;
  final Color today;

  static const double _cellSize = 11;

  void _text(Canvas canvas, String s, Offset at, double size) {
    (TextPainter(
      text: TextSpan(
        text: s,
        style: TextStyle(fontFamily: 'monospace', fontSize: size, color: ink),
      ),
      textDirection: TextDirection.ltr,
    )..layout())
        .paint(canvas, at);
  }

  @override
  void paint(Canvas canvas, Size size) {
    const step = _When._step;
    const left = _When._labelW;
    const top = 15.0;
    final now = DateTime.now();
    final dow = now.weekday - 1; // Monday 0 … Sunday 6

    for (final (row, label) in const [(1, 'Mon'), (3, 'Wed'), (5, 'Fri')]) {
      _text(canvas, label, Offset(0, top + row * step - 1), 9);
    }

    final fill = Paint();
    for (var ago = 0; ago < days.length; ago++) {
      final at = _cell(ago, weeks, dow);
      if (at == null) break;
      final v = days[days.length - 1 - ago];
      fill.color = v <= 0 ? empty : Color.lerp(empty, _pink, _level(v))!;
      final r = RRect.fromRectAndRadius(
        Rect.fromLTWH(left + at.$1 * step, top + at.$2 * step, _cellSize,
            _cellSize),
        const Radius.circular(3),
      );
      canvas.drawRRect(r, fill);
      if (ago == 0) {
        canvas.drawRRect(
          r.inflate(1.5),
          Paint()
            ..style = PaintingStyle.stroke
            ..strokeWidth = 1.5
            ..color = today,
        );
      }
    }

    // A month's name over the first week that starts in it, and never closer
    // than three columns to the last one: "Apr" and "May" would overprint.
    var lastMonth = -1;
    var lastCol = -9;
    for (var col = 0; col < weeks; col++) {
      final monday = DateTime(
          now.year, now.month, now.day - dow - (weeks - 1 - col) * 7);
      if (monday.month != lastMonth && col - lastCol >= 3 && col < weeks - 1) {
        _text(canvas, _months[monday.month - 1], Offset(left + col * step, 0),
            9.5);
        lastCol = col;
      }
      lastMonth = monday.month;
    }
  }

  @override
  bool shouldRepaint(_Calendar old) =>
      old.days != days ||
      old.weeks != weeks ||
      old.ink != ink ||
      old.empty != empty;
}

class _CalendarSide extends StatelessWidget {
  const _CalendarSide({required this.days, required this.weeks});

  final List<double> days;
  final int weeks;

  static const List<String> _dayNames = [
    'Mondays', 'Tuesdays', 'Wednesdays', 'Thursdays',
    'Fridays', 'Saturdays', 'Sundays',
  ];

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final dow = DateTime.now().weekday - 1;
    var played = 0;
    final byDay = List<double>.filled(7, 0);
    for (var ago = 0; ago < days.length; ago++) {
      final at = _cell(ago, weeks, dow);
      if (at == null) break;
      final v = days[days.length - 1 - ago];
      if (v > 0) played++;
      byDay[at.$2] += v;
    }
    final best = byDay.indexOf(byDay.reduce(math.max));
    TextStyle big() => TextStyle(
        fontSize: 17, fontWeight: FontWeight.w800, color: t.nInk, height: 1.2);
    final small = TextStyle(fontSize: 12, color: t.nInk2);
    return Padding(
      padding: const EdgeInsets.only(top: 14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(_n(played), style: big()),
          Text('days with a play', style: small),
          const SizedBox(height: 8),
          Text(played == 0 ? '—' : _dayNames[best], style: big()),
          Text('your busiest day', style: small),
          const Spacer(),
          Row(
            children: [
              Text('less ', style: _mono(9.5, t.nInk2, FontWeight.w400)),
              for (final a in const [0.0, 0.28, 0.52, 0.76, 1.0])
                Container(
                  width: 10,
                  height: 10,
                  margin: const EdgeInsets.only(right: 3),
                  decoration: BoxDecoration(
                    color: Color.lerp(t.nChip, _pink, a),
                    borderRadius: BorderRadius.circular(2),
                  ),
                ),
              Text(' more', style: _mono(9.5, t.nInk2, FontWeight.w400)),
            ],
          ),
        ],
      ),
    );
  }
}

class _Hours extends StatelessWidget {
  const _Hours({required this.hours});

  final List<double> hours;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return LayoutBuilder(
      builder: (context, box) {
        final h = box.maxHeight;
        return Row(
          crossAxisAlignment: CrossAxisAlignment.end,
          children: [
            for (var i = 0; i < hours.length; i++)
              Expanded(
                child: Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 1.5),
                  child: Tooltip(
                    message: '${i.toString().padLeft(2, '0')}:00',
                    child: Container(
                      // A floor of three pixels: a quiet hour is still an
                      // hour, and a gap reads as a hole in the axis.
                      height:
                          math.max(0.0, math.min(h, 3 + hours[i] * (h - 3))),
                      decoration: BoxDecoration(
                        color: hours[i] >= 1
                            ? _pink
                            : Color.lerp(t.nChip, _pink, hours[i]),
                        borderRadius: const BorderRadius.vertical(
                            top: Radius.circular(3)),
                      ),
                    ),
                  ),
                ),
              ),
          ],
        );
      },
    );
  }
}

/// Listening time by genre as a ring: the top five and everything else.
class _Genres extends StatelessWidget {
  const _Genres({required this.rows});

  final List<Tally> rows;

  static const List<Color> _colors = [
    _pink, _violet, _indigo, _cyan, _amber, _other,
  ];

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (rows.isEmpty) {
      return Align(
        alignment: Alignment.topLeft,
        child: Text('No genre tags on what you played.',
            style: TextStyle(fontSize: 12, color: t.nInk2)),
      );
    }
    // `frac` is against the biggest row, which is proportional to listening
    // time, so shares of the sum are shares of the time.
    final total = rows.fold<double>(0, (s, r) => s + r.frac);
    // Plays logged with no time on them add up to nothing; no ring beats NaN.
    final sum = total > 0 ? total : 1.0;
    final entries = <(String, double)>[
      for (final r in rows.take(5)) (r.label, r.frac / sum),
      if (rows.length > 5)
        ('Other', rows.skip(5).fold<double>(0, (s, r) => s + r.frac) / sum),
    ];
    return LayoutBuilder(
      builder: (context, box) {
        final ring = math.min(118.0, math.min(box.maxHeight, box.maxWidth * 0.42));
        final fit = math.max(1, (box.maxHeight / 19).floor());
        return Row(
          children: [
            SizedBox(
              width: ring,
              height: ring,
              child: CustomPaint(
                painter: _Ring(
                  shares: [for (final e in entries) e.$2],
                  colors: _colors,
                ),
                child: Center(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(
                        rows.length >= 12 ? '12+' : '${rows.length}',
                        style: TextStyle(
                            fontSize: 15,
                            fontWeight: FontWeight.w800,
                            color: t.nInk),
                      ),
                      Text('genres',
                          style: TextStyle(fontSize: 10, color: t.nInk2)),
                    ],
                  ),
                ),
              ),
            ),
            const SizedBox(width: 14),
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  for (var i = 0; i < math.min(entries.length, fit); i++)
                    SizedBox(
                      height: 19,
                      child: Row(
                        children: [
                          Container(
                            width: 8,
                            height: 8,
                            decoration: BoxDecoration(
                              color: _colors[i],
                              borderRadius: BorderRadius.circular(2),
                            ),
                          ),
                          const SizedBox(width: 8),
                          Expanded(
                            child: Text(
                              entries[i].$1,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(fontSize: 12, color: t.nInk3),
                            ),
                          ),
                          Text('${(entries[i].$2 * 100).round()}%',
                              style: _mono(11, t.nInk2)),
                        ],
                      ),
                    ),
                ],
              ),
            ),
          ],
        );
      },
    );
  }
}

class _Ring extends CustomPainter {
  _Ring({required this.shares, required this.colors});

  final List<double> shares;
  final List<Color> colors;

  @override
  void paint(Canvas canvas, Size size) {
    final stroke = size.shortestSide * 0.15;
    final rect = (Offset.zero & size).deflate(stroke / 2);
    var start = -math.pi / 2;
    for (var i = 0; i < shares.length; i++) {
      final sweep = shares[i] * 2 * math.pi;
      canvas.drawArc(
        rect,
        start,
        sweep,
        false,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = stroke
          ..color = colors[i % colors.length],
      );
      start += sweep;
    }
  }

  @override
  bool shouldRepaint(_Ring old) => old.shares != shares;
}

class _Library extends StatefulWidget {
  const _Library({required this.data});

  final StatsCenter data;

  @override
  State<_Library> createState() => _LibraryState();
}

class _LibraryState extends State<_Library> {
  bool _started = false;

  static const List<Color> _colors = [_good, _indigo, _cyan, _amber, _other];

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final d = widget.data;
    final analysed = math.max(0, d.tracks - d.pending);
    Widget fig(String label, String value) => Expanded(
          child: Container(
            color: t.nCanvas,
            padding: const EdgeInsets.symmetric(horizontal: 11, vertical: 7),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(label.toUpperCase(), style: _caps(context)),
                const SizedBox(height: 2),
                Text(
                  value,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: 16,
                    fontWeight: FontWeight.w800,
                    color: t.nInk,
                    fontFeatures: const [FontFeature.tabularFigures()],
                  ),
                ),
              ],
            ),
          ),
        );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ClipRRect(
          borderRadius: BorderRadius.circular(10),
          child: ColoredBox(
            color: t.nHair,
            child: Row(
              children: [
                fig('Tracks', _n(d.tracks)),
                const SizedBox(width: 1),
                fig('Albums', _n(d.albums)),
                const SizedBox(width: 1),
                fig('Artists', _n(d.artists)),
                const SizedBox(width: 1),
                fig('On disk', _size(d.bytes)),
              ],
            ),
          ),
        ),
        const SizedBox(height: 10),
        // The file types and the analysis side by side, so the chart keeps
        // its rows on a short window instead of being the first thing cut.
        Expanded(
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Expanded(
                flex: 3,
                child: _Formats(formats: d.formats, colors: _colors),
              ),
              const SizedBox(width: 12),
              Expanded(
                flex: 2,
                child: Container(
                  padding: const EdgeInsets.all(10),
                  decoration: BoxDecoration(
                    color: t.nCanvas,
                    borderRadius: BorderRadius.circular(10),
                    border: Border.all(color: t.nHair),
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Row(
                        children: [
                          SizedBox(
                            width: 28,
                            height: 28,
                            child: CircularProgressIndicator(
                              value: d.tracks == 0 ? 0 : analysed / d.tracks,
                              strokeWidth: 4,
                              color: _good,
                              backgroundColor: t.nChip,
                            ),
                          ),
                          const SizedBox(width: 10),
                          Expanded(
                            child: Text.rich(
                              TextSpan(
                                children: [
                                  TextSpan(
                                    text: '${_n(analysed)} of ${_n(d.tracks)}\n',
                                    style: TextStyle(
                                        color: t.nInk,
                                        fontWeight: FontWeight.w700),
                                  ),
                                  const TextSpan(text: 'analysed'),
                                ],
                              ),
                              maxLines: 2,
                              overflow: TextOverflow.ellipsis,
                              style: TextStyle(
                                  fontSize: 12, color: t.nInk2, height: 1.3),
                            ),
                          ),
                        ],
                      ),
                      const Spacer(),
                      if (d.pending > 0)
                        _Btn(
                          _started ? 'Analysing…' : 'Analyse ${_n(d.pending)}',
                          small: true,
                          // Its progress is the section's scan bar, like the
                          // Settings button that starts the same pass.
                          onTap: _started
                              ? null
                              : () {
                                  setState(() => _started = true);
                                  musicAnalyseAll();
                                },
                        )
                      else
                        Text(
                          'Tempo, key and duplicates cover everything.',
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 11, color: t.nInk2),
                        ),
                    ],
                  ),
                ),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// The file types, as a bar chart: one row per codec, as many rows as the
/// panel has height for, the longest bar the most common type.
class _Formats extends StatelessWidget {
  const _Formats({required this.formats, required this.colors});

  final List<Tally> formats;
  final List<Color> colors;

  static const double _rowH = 20;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text('FILE TYPES', style: _caps(context)),
        const SizedBox(height: 6),
        Expanded(
          child: formats.isEmpty
              ? Align(
                  alignment: Alignment.topLeft,
                  child: Text('No file types read yet.',
                      style: TextStyle(fontSize: 12, color: t.nInk2)),
                )
              : LayoutBuilder(
                  builder: (context, box) {
                    final n = math.min(formats.length,
                        math.max(1, (box.maxHeight / _rowH).floor()));
                    final top = formats.map((f) => f.frac).reduce(math.max);
                    return Column(
                      children: [
                        for (var i = 0; i < n; i++)
                          Tooltip(
                            message: '${_n(formats[i].plays)} tracks',
                            child: SizedBox(
                              height: _rowH,
                              child: Row(
                                children: [
                                  SizedBox(
                                    width: 46,
                                    child: Text(
                                      formats[i].label,
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                      style:
                                          _mono(11, t.nInk, FontWeight.w600),
                                    ),
                                  ),
                                  const SizedBox(width: 8),
                                  Expanded(
                                    child: ClipRRect(
                                      borderRadius: BorderRadius.circular(3),
                                      child: SizedBox(
                                        height: 8,
                                        child: Stack(
                                          fit: StackFit.expand,
                                          children: [
                                            ColoredBox(color: t.nChip),
                                            FractionallySizedBox(
                                              alignment: Alignment.centerLeft,
                                              widthFactor: top > 0
                                                  ? (formats[i].frac / top)
                                                      .clamp(0.0, 1.0)
                                                  : 0,
                                              child: ColoredBox(
                                                  color: colors[
                                                      i % colors.length]),
                                            ),
                                          ],
                                        ),
                                      ),
                                    ),
                                  ),
                                  const SizedBox(width: 8),
                                  SizedBox(
                                    width: 38,
                                    child: Text(
                                      formats[i].value,
                                      textAlign: TextAlign.right,
                                      style: _mono(11, t.nInk2),
                                    ),
                                  ),
                                ],
                              ),
                            ),
                          ),
                      ],
                    );
                  },
                ),
        ),
      ],
    );
  }
}

class _NeverFinished extends StatelessWidget {
  const _NeverFinished({required this.rows});

  final List<Tally> rows;

  static const double _rowH = 28;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (rows.isEmpty) {
      return Align(
        alignment: Alignment.topLeft,
        child: Text('Nothing you keep starting and dropping.',
            style: TextStyle(fontSize: 12, color: t.nInk2)),
      );
    }
    return LayoutBuilder(
      builder: (context, box) {
        final n =
            math.min(rows.length, math.max(1, (box.maxHeight / _rowH).floor()));
        return Column(
          children: [
            for (var i = 0; i < n; i++)
              SizedBox(
                height: _rowH,
                child: Row(
                  children: [
                    Expanded(
                      child: Text(
                        rows[i].label,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 12.5, color: t.nInk),
                      ),
                    ),
                    const SizedBox(width: 8),
                    Container(
                      padding: const EdgeInsets.symmetric(
                          horizontal: 6, vertical: 2),
                      decoration: BoxDecoration(
                        color: _amber.withValues(alpha: 0.12),
                        borderRadius: BorderRadius.circular(6),
                      ),
                      child: Text('${rows[i].plays}×', style: _mono(11, _amber)),
                    ),
                  ],
                ),
              ),
          ],
        );
      },
    );
  }
}

// -------------------------------------------------------------- duplicates --

/// The same recording, more than once.
Future<void> duplicateFinder(BuildContext context, MusicController c) =>
    _openFull(context, _Duplicates(controller: c, opener: context));

/// The last run, kept for the session: reopening shows it at once instead of
/// asking for another run. What was reviewed in it is kept beside it.
List<DupeGroup>? _lastGroups;
DateTime? _lastRun;
final Map<DupeGroup, _Mark> _marks = Map.identity();

/// What has been done to one group this session.
class _Mark {
  /// The copies ticked for deletion. Null until the group is touched, which
  /// means every copy but the bridge's pick.
  Set<int>? drop;

  /// Copies deleted since the run.
  final Set<int> gone = {};

  /// Merged down to one copy, or dismissed as not duplicates.
  bool done = false;
}

enum _Phase { ready, fingerprint, compare, results, clean }

const Set<String> _losslessCodecs = {
  'FLAC', 'ALAC', 'WAV', 'AIFF', 'AIF', 'APE', 'WV',
};

String _codec(DupeCopy c) => c.quality.split('·').first.trim().toUpperCase();

bool _isLossless(DupeCopy c) {
  final k = _codec(c);
  return _losslessCodecs.contains(k) || k.startsWith('PCM');
}

bool _same(String a, String b) => a.trim().toLowerCase() == b.trim().toLowerCase();

String _base(String path) {
  final name = path.split('/').last;
  final dot = name.lastIndexOf('.');
  return dot > 0 ? name.substring(0, dot) : name;
}

String _shortPath(String path) {
  final parts = path.split('/').where((p) => p.isNotEmpty).toList();
  return parts.length <= 3
      ? path
      : '…/${parts.sublist(parts.length - 3).join('/')}';
}

String _titleOf(DupeCopy c) =>
    c.track.title.isEmpty ? _base(c.track.path) : c.track.title;

class _Duplicates extends StatefulWidget {
  const _Duplicates({required this.controller, required this.opener});

  final MusicController controller;
  final BuildContext opener;

  @override
  State<_Duplicates> createState() => _DuplicatesState();
}

class _DuplicatesState extends State<_Duplicates> {
  late _Phase _phase = _lastGroups == null
      ? _Phase.ready
      : _lastGroups!.isEmpty
          ? _Phase.clean
          : _Phase.results;
  int? _pending;
  bool _fingerprintFirst = true;
  bool _stopped = false;

  /// Whether this run has seen the fingerprinting pass report yet. Until it
  /// has, a controller with no progress is a pass not started, not one done.
  bool _seen = false;
  bool _following = false;
  int _sel = 0;
  int _filter = 0;
  String _note = '';

  MusicController get _c => widget.controller;

  bool get _analysing =>
      _c.progress?.label.startsWith('Analysing') ?? false;

  @override
  void initState() {
    super.initState();
    _countPending();
    // A pass still running from an earlier open: follow it rather than offer
    // to start a second one.
    if (_analysing) {
      _phase = _Phase.fingerprint;
      _follow();
    }
  }

  @override
  void dispose() {
    _c.removeListener(_onProgress);
    super.dispose();
  }

  Future<void> _countPending() async {
    try {
      final p = await musicAnalysePending();
      if (mounted) setState(() => _pending = p.toInt());
    } catch (_) {}
  }

  Future<void> _run() async {
    setState(() {
      _phase = _Phase.fingerprint;
      _stopped = false;
      _note = '';
    });
    final n = _fingerprintFirst && (_pending ?? 0) > 0
        ? (await musicAnalyseAll()).toInt()
        : 0;
    if (!mounted) return;
    if (n > 0 || _analysing) {
      _follow();
    } else {
      await _compare();
    }
  }

  void _follow() {
    _seen = _analysing;
    if (!_following) {
      _following = true;
      _c.addListener(_onProgress);
    }
    // The pass reports before its first track, so two seconds with no report
    // means it finished before this was listening — go straight to the
    // comparison rather than wait for an event that has already gone by.
    Future.delayed(const Duration(seconds: 2), () {
      if (mounted && _following && !_seen && _c.progress == null) _finished();
    });
  }

  void _onProgress() {
    if (!mounted) return;
    if (_c.progress != null) {
      _seen = true;
      setState(() {});
    } else if (_seen) {
      _finished();
    }
  }

  void _finished() {
    _following = false;
    _c.removeListener(_onProgress);
    _countPending();
    if (_stopped) {
      setState(() => _phase = _Phase.ready);
    } else {
      _compare();
    }
  }

  Future<void> _compare() async {
    setState(() => _phase = _Phase.compare);
    try {
      final groups = await musicDuplicates();
      _lastGroups = groups;
      _lastRun = DateTime.now();
      _marks.clear();
      if (!mounted) return;
      setState(() {
        _phase = groups.isEmpty ? _Phase.clean : _Phase.results;
        _sel = 0;
        _filter = 0;
      });
    } catch (e) {
      if (mounted) {
        setState(() {
          _phase = _Phase.ready;
          _note = '$e';
        });
      }
    }
  }

  Future<void> _stop() async {
    _stopped = true;
    await musicAnalyseStop();
  }

  // --- one group ---

  _Mark _mark(DupeGroup g) => _marks.putIfAbsent(g, _Mark.new);

  List<DupeCopy> _live(DupeGroup g) {
    final gone = _mark(g).gone;
    return [for (final c in g.copies) if (!gone.contains(c.track.itemId)) c];
  }

  /// The bridge's pick first — the best copy on quality — then the closest
  /// match.
  List<DupeCopy> _ordered(DupeGroup g) {
    final live = _live(g);
    final rest = live.where((c) => !c.keep).toList()
      ..sort((a, b) => b.confidence.compareTo(a.confidence));
    return [...live.where((c) => c.keep), ...rest];
  }

  /// Ticked for deletion. Any copy can be, the best one included: the one
  /// worth keeping is sometimes the one with the name you want rather than
  /// the one with the most bits.
  Set<int> _drop(DupeGroup g) => _mark(g).drop ??= {
        for (final c in _ordered(g).skip(1)) c.track.itemId,
      };

  /// The copy that stays and takes the others' plays and playlist places: the
  /// first unticked one. Null when every copy is ticked.
  DupeCopy? _keeper(DupeGroup g) {
    final drop = _drop(g);
    for (final c in _ordered(g)) {
      if (!drop.contains(c.track.itemId)) return c;
    }
    return null;
  }

  /// What the group is shown as, and what its other copies are compared to.
  DupeCopy _lead(DupeGroup g) => _keeper(g) ?? _ordered(g).first;

  bool _tagsDiffer(DupeGroup g) {
    final live = _live(g);
    if (live.length < 2) return false;
    final k = _ordered(g).first;
    return live.any((c) =>
        c != k &&
        (!_same(c.track.title, k.track.title) ||
            !_same(c.track.album, k.track.album)));
  }

  bool _mixed(DupeGroup g) {
    final live = _live(g);
    return live.any(_isLossless) && live.any((c) => !_isLossless(c));
  }

  /// What deleting the ticked copies frees.
  int _extraBytes(DupeGroup g) {
    final drop = _drop(g);
    return _live(g)
        .where((c) => drop.contains(c.track.itemId))
        .fold(0, (s, c) => s + c.bytes);
  }

  List<DupeGroup> get _shown {
    final all = _lastGroups ?? const <DupeGroup>[];
    return switch (_filter) {
      1 => all.where(_mixed).toList(),
      2 => all.where(_tagsDiffer).toList(),
      _ => all,
    };
  }

  void _move(int by) {
    final n = _shown.length;
    if (n == 0) return;
    setState(() => _sel = (_sel + by).clamp(0, n - 1));
  }

  /// The next group still to review, after this one.
  void _next() {
    final shown = _shown;
    for (var i = 1; i <= shown.length; i++) {
      final j = (_sel + i) % shown.length;
      if (!_mark(shown[j]).done) {
        setState(() => _sel = j);
        return;
      }
    }
  }

  Future<void> _delete(DupeGroup g, List<DupeCopy> drop) async {
    final keep = _keeper(g);
    if (drop.isEmpty || keep == null) return;
    final ok = await confirm(
      context,
      title: drop.length == 1 ? 'Delete this copy?' : 'Delete ${drop.length} copies?',
      body: 'These go to the recycle bin:\n\n'
          '${drop.map((c) => c.track.path).join('\n')}\n\n'
          'Their plays and playlist places move to the copy you keep:\n'
          '${keep.track.path}',
      action: drop.length == 1 ? 'Delete copy' : 'Delete ${drop.length} copies',
    );
    if (!ok) return;
    await _c.send(MusicCmd.dupeMerge(
      keepId: keep.track.itemId,
      dropIds: Int64List.fromList([for (final c in drop) c.track.itemId]),
    ));
    if (!mounted) return;
    setState(() {
      if (_c.error != null) {
        _note = '${_c.error}';
        return;
      }
      _note = '';
      final m = _mark(g);
      m.gone.addAll(drop.map((c) => c.track.itemId));
      m.drop = null;
      if (_live(g).length < 2) m.done = true;
    });
  }

  Future<void> _dismiss(DupeGroup g) async {
    await _c.send(MusicCmd.dupeDismiss(
      itemIds: Int64List.fromList([for (final c in g.copies) c.track.itemId]),
    ));
    if (!mounted) return;
    setState(() {
      if (_c.error != null) {
        _note = '${_c.error}';
      } else {
        _note = '';
        _mark(g).done = true;
      }
    });
  }

  // --- build ---

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final run = _lastRun;
    return Column(
      children: [
        _Head(
          title: 'Duplicates',
          sub: 'Recordings you have more than once, matched by sound, not tags',
          actions: [
            if (run != null &&
                (_phase == _Phase.results || _phase == _Phase.clean))
              Text(
                'Last run ${run.hour.toString().padLeft(2, '0')}:'
                '${run.minute.toString().padLeft(2, '0')}',
                style: _mono(11, t.nInk2, FontWeight.w400),
              ),
            switch (_phase) {
              _Phase.ready => _Btn('Run on library',
                  icon: Icons.play_arrow_rounded,
                  kind: _Kind.primary,
                  onTap: _run),
              _Phase.fingerprint => _Btn('Stop',
                  icon: Icons.stop_rounded, kind: _Kind.danger, onTap: _stop),
              _Phase.compare => const SizedBox.shrink(),
              _ => _Btn('Run again', icon: Icons.refresh, onTap: _run),
            },
          ],
        ),
        Expanded(
          child: Padding(
            padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 14),
            child: switch (_phase) {
              _Phase.ready => _center(_readyCard(context)),
              _Phase.fingerprint || _Phase.compare =>
                _center(_runningCard(context)),
              _Phase.clean => _center(_cleanCard(context)),
              _Phase.results => _results(context),
            },
          ),
        ),
        _Foot(
          children: [
            Expanded(
              child: Text(
                _note.isNotEmpty
                    ? _note
                    : switch (_phase) {
                        _Phase.results => '↑ ↓ move between groups · a '
                            'confirmation lists every file before anything '
                            'is deleted',
                        _Phase.fingerprint || _Phase.compare =>
                          'You can close this; fingerprinting keeps running',
                        _ => 'Matches on sound only. Songs that just share a '
                            'title are not duplicates.',
                      },
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: _note.isNotEmpty ? const TextStyle(color: _bad) : null,
              ),
            ),
            const SizedBox(width: 14),
            _Btn(
              'Stats',
              icon: Icons.arrow_back,
              small: true,
              onTap: () {
                Navigator.of(context).pop();
                listeningSummary(widget.opener, _c);
              },
            ),
          ],
        ),
      ],
    );
  }

  Widget _center(Widget card) => Center(
        child: SingleChildScrollView(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 640),
            child: card,
          ),
        ),
      );

  Widget _card(BuildContext context, List<Widget> children,
      {bool centred = false}) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.fromLTRB(30, 28, 30, 28),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: t.nHair),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment:
            centred ? CrossAxisAlignment.center : CrossAxisAlignment.stretch,
        children: [
          for (var i = 0; i < children.length; i++) ...[
            if (i > 0) const SizedBox(height: 18),
            children[i],
          ],
        ],
      ),
    );
  }

  TextStyle _h4(BuildContext context) => TextStyle(
        fontFamily: Tokens.fontFamily,
        fontSize: 22,
        fontWeight: FontWeight.w800,
        color: context.tokens.nInk,
      );

  TextStyle _para(BuildContext context) =>
      TextStyle(fontSize: 13.5, color: context.tokens.nInk3, height: 1.6);

  Widget _steps(List<Widget> steps) => IntrinsicHeight(
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            for (var i = 0; i < steps.length; i++) ...[
              if (i > 0) const SizedBox(width: 10),
              Expanded(child: steps[i]),
            ],
          ],
        ),
      );

  Widget _readyCard(BuildContext context) {
    final t = context.tokens;
    final p = _pending;
    return _card(context, [
      Text('Find every recording you have twice', style: _h4(context)),
      Text(
        'Run compares every track by length and by sound. A 128 kbps rip is '
        'matched to its FLAC even when the title, album and file name are all '
        'different. Nothing is deleted without you choosing it.',
        style: _para(context),
      ),
      _steps([
        _Step(
          label: '1 · Fingerprint',
          title: p == null
              ? '…'
              : p == 0
                  ? 'Every track is done'
                  : '${_n(p)} tracks to analyse',
          body: p == 0
              ? 'Nothing to wait for.'
              : 'The rest are already done. The pass runs in the background.',
        ),
        const _Step(
          label: '2 · Compare',
          title: 'Whole library',
          body: 'Length within 2 s, sound 95% alike.',
        ),
        const _Step(
          label: '3 · Review',
          title: 'Keep the best copy',
          body: 'Lossless beats lossy, then higher bitrate.',
        ),
      ]),
      if ((p ?? 0) > 0)
        InkWell(
          onTap: () => setState(() => _fingerprintFirst = !_fingerprintFirst),
          child: Row(
            children: [
              Checkbox(
                value: _fingerprintFirst,
                activeColor: _pink,
                visualDensity: VisualDensity.compact,
                onChanged: (v) => setState(() => _fingerprintFirst = v ?? true),
              ),
              const SizedBox(width: 4),
              Text('Fingerprint the ${_n(p!)} unanalysed tracks first',
                  style: TextStyle(fontSize: 13, color: t.nInk3)),
            ],
          ),
        ),
      Row(
        children: [
          _Btn('Run on library',
              icon: Icons.play_arrow_rounded,
              kind: _Kind.primary,
              onTap: _run),
          const Spacer(),
          Text('You can keep listening while it runs',
              style: _mono(12, t.nInk2, FontWeight.w400)),
        ],
      ),
    ]);
  }

  Widget _runningCard(BuildContext context) {
    final t = context.tokens;
    final p = _c.progress;
    final comparing = _phase == _Phase.compare;
    final total = p?.total ?? 0;
    final done = p?.done ?? 0;
    final frac = total > 0 ? done / total : null;
    return _card(context, [
      Text(
        comparing
            ? 'Comparing the library…'
            : total > 0
                ? 'Fingerprinting · ${_n(done)} of ${_n(total)}'
                : 'Starting…',
        style: _h4(context),
      ),
      _steps([
        _Step(
          label: '1 · Fingerprint',
          title: comparing ? 'Done' : total > 0 ? '$done / $total' : '…',
          body: comparing ? 'Every track has a fingerprint' : 'Running',
          state: comparing ? 2 : 1,
        ),
        _Step(
          label: '2 · Compare',
          title: comparing ? 'Comparing' : 'Waiting',
          body: 'A few seconds, once step 1 is done',
          state: comparing ? 1 : 0,
        ),
        const _Step(
          label: '3 · Review',
          title: '—',
          body: 'Groups appear here',
        ),
      ]),
      ClipRRect(
        borderRadius: BorderRadius.circular(4),
        child: LinearProgressIndicator(
          value: comparing ? null : frac,
          minHeight: 8,
          color: _pink,
          backgroundColor: t.nChip,
        ),
      ),
      Row(
        children: [
          Expanded(
            child: Text(
              comparing
                  ? 'Length within 2 s, sound 95% alike'
                  : p?.label ?? '',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: _mono(11.5, t.nInk2, FontWeight.w400),
            ),
          ),
          if (!comparing && frac != null)
            Text('${(frac * 100).round()}%',
                style: _mono(11.5, t.nInk2, FontWeight.w400)),
        ],
      ),
      Text(
        "Closing this doesn't stop fingerprinting. Its progress stays in the "
        "section's progress bar, and opening Duplicates again picks it up.",
        style: _para(context),
      ),
    ]);
  }

  Widget _cleanCard(BuildContext context) {
    final p = _pending ?? 0;
    return _card(
      context,
      [
        Container(
          width: 56,
          height: 56,
          decoration: BoxDecoration(
            color: _good.withValues(alpha: 0.14),
            shape: BoxShape.circle,
          ),
          child: const Icon(Icons.check_rounded, color: _good, size: 28),
        ),
        Text('No duplicates found',
            style: _h4(context), textAlign: TextAlign.center),
        Text(
          p == 0
              ? 'Every track has been fingerprinted, so the whole library was '
                  'checked. Run again after you add music.'
              : '${_n(p)} tracks have no fingerprint yet, so they were not '
                  'compared. Run again with fingerprinting on to include them.',
          style: _para(context),
          textAlign: TextAlign.center,
        ),
      ],
      centred: true,
    );
  }

  Widget _results(BuildContext context) {
    final all = _lastGroups!;
    final open = all.where((g) => !_mark(g).done).toList();
    final extra = open.fold<int>(0, (s, g) => s + _live(g).length - 1);
    final space = open.fold<int>(0, (s, g) => s + _extraBytes(g));
    final shown = _shown;
    final sel = shown.isEmpty ? null : shown[_sel.clamp(0, shown.length - 1)];
    const gap = SizedBox(width: 12);
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.arrowDown): () => _move(1),
        const SingleActivator(LogicalKeyboardKey.arrowUp): () => _move(-1),
      },
      child: Focus(
        autofocus: true,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            SizedBox(
              height: 46,
              child: Row(
                children: [
                  Expanded(child: _Kpi('Groups', _n(all.length), _pink)),
                  gap,
                  Expanded(child: _Kpi('Extra copies', _n(extra), _violet)),
                  gap,
                  Expanded(child: _Kpi('Space to free', _size(space), _good)),
                  gap,
                  Expanded(
                    child: _Kpi(
                        'Tags disagree', _n(all.where(_tagsDiffer).length), _amber),
                  ),
                  gap,
                  Expanded(
                    child: _Kpi('Reviewed',
                        '${all.length - open.length} / ${all.length}', _cyan),
                  ),
                ],
              ),
            ),
            const SizedBox(height: 12),
            Expanded(
              child: LayoutBuilder(
                builder: (context, box) => Row(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    SizedBox(
                      width: (box.maxWidth * 0.3).clamp(240.0, 330.0),
                      child: _groupList(context, all, shown, sel),
                    ),
                    gap,
                    Expanded(
                      child: sel == null
                          ? _Panel(
                              title: 'Nothing here',
                              child: Text('No group matches this filter.',
                                  style: TextStyle(
                                      fontSize: 12.5,
                                      color: context.tokens.nInk2)),
                            )
                          : _comparePane(context, sel),
                    ),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _groupList(BuildContext context, List<DupeGroup> all,
      List<DupeGroup> shown, DupeGroup? sel) {
    final t = context.tokens;
    Widget chip(String label, int value) {
      final on = _filter == value;
      return InkWell(
        borderRadius: BorderRadius.circular(999),
        onTap: () => setState(() {
          _filter = value;
          _sel = 0;
        }),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
          decoration: BoxDecoration(
            color: on ? t.nInk : Colors.transparent,
            borderRadius: BorderRadius.circular(999),
            border: Border.all(color: on ? t.nInk : t.outlineStrong),
          ),
          child: Text(
            label,
            style: TextStyle(
              fontSize: 11.5,
              fontWeight: FontWeight.w600,
              color: on ? t.nCanvas : t.nInk2,
            ),
          ),
        ),
      );
    }

    return Container(
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: t.nHair),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 13, 14, 10),
            child: Row(
              children: [
                Text('Groups',
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        color: t.nInk)),
                const Spacer(),
                Text('biggest first', style: _mono(11, t.nInk2, FontWeight.w400)),
              ],
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(14, 0, 14, 10),
            child: Wrap(
              spacing: 6,
              runSpacing: 6,
              children: [
                chip('All ${all.length}', 0),
                chip('Lossy + lossless ${all.where(_mixed).length}', 1),
                chip('Tags disagree ${all.where(_tagsDiffer).length}', 2),
              ],
            ),
          ),
          Expanded(
            child: ListView.builder(
              padding: const EdgeInsets.fromLTRB(8, 0, 8, 8),
              itemCount: shown.length,
              itemBuilder: (context, i) {
                final g = shown[i];
                final k = _lead(g);
                final done = _mark(g).done;
                final on = g == sel;
                return Opacity(
                  opacity: done ? 0.45 : 1,
                  child: InkWell(
                    borderRadius: BorderRadius.circular(10),
                    onTap: () => setState(() => _sel = i),
                    child: Container(
                      padding: const EdgeInsets.all(8),
                      margin: const EdgeInsets.only(bottom: 2),
                      decoration: BoxDecoration(
                        color: on ? _pink.withValues(alpha: 0.10) : null,
                        borderRadius: BorderRadius.circular(10),
                        border: Border.all(
                          color: on
                              ? _pink.withValues(alpha: 0.4)
                              : Colors.transparent,
                        ),
                      ),
                      child: Row(
                        children: [
                          MusicArt(
                            controller: _c,
                            kind: 'track',
                            artKey: '${k.track.itemId}',
                            direct: k.track.art,
                            size: 40,
                            radius: 7,
                          ),
                          const SizedBox(width: 11),
                          Expanded(
                            child: Column(
                              crossAxisAlignment: CrossAxisAlignment.start,
                              children: [
                                Text(
                                  _titleOf(k),
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: TextStyle(
                                    fontSize: 13,
                                    fontWeight: FontWeight.w700,
                                    color: t.nInk,
                                  ),
                                ),
                                Text(
                                  k.track.artist,
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style:
                                      TextStyle(fontSize: 11.5, color: t.nInk2),
                                ),
                              ],
                            ),
                          ),
                          const SizedBox(width: 8),
                          Column(
                            mainAxisAlignment: MainAxisAlignment.center,
                            crossAxisAlignment: CrossAxisAlignment.end,
                            children: [
                              Container(
                                padding: const EdgeInsets.symmetric(
                                    horizontal: 6, vertical: 2),
                                decoration: BoxDecoration(
                                  color: t.nChip,
                                  borderRadius: BorderRadius.circular(6),
                                ),
                                child: Text(
                                  done ? '✓ done' : '${_live(g).length} copies',
                                  style: _mono(10.5, done ? _good : t.nInk3,
                                      FontWeight.w600),
                                ),
                              ),
                              const SizedBox(height: 3),
                              if (!done)
                                Text(_size(_extraBytes(g)),
                                    style: _mono(10.5, t.nInk2, FontWeight.w400)),
                            ],
                          ),
                        ],
                      ),
                    ),
                  ),
                );
              },
            ),
          ),
        ],
      ),
    );
  }

  Widget _comparePane(BuildContext context, DupeGroup g) {
    final t = context.tokens;
    final copies = _ordered(g);
    final keep = _keeper(g);
    final k = _lead(g);
    final drop = _drop(g);
    final others = copies.where((c) => c != k).toList();
    final ticked =
        copies.where((c) => drop.contains(c.track.itemId)).toList();
    final done = _mark(g).done;
    final drift = others.fold<double>(
        0, (m, c) => math.max(m, (c.track.durationS - k.track.durationS).abs()));
    final differ = [
      if (others.any((c) => !_same(c.track.title, k.track.title))) 'title',
      if (others.any((c) => !_same(c.track.album, k.track.album))) 'album',
      if (others.any((c) => !_same(_base(c.track.path), _base(k.track.path))))
        'file name',
    ];
    final alike = others.isEmpty
        ? 100
        : others.map((c) => c.confidence).reduce(math.min);
    Widget pill(String text, {bool warn = false}) => Container(
          padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 3),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(999),
            border: Border.all(
                color: warn ? _amber.withValues(alpha: 0.4) : t.outlineStrong),
          ),
          child: Text(text, style: _mono(11, warn ? _amber : t.nInk3)),
        );
    return Container(
      padding: const EdgeInsets.fromLTRB(18, 16, 18, 16),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: t.nHair),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              MusicArt(
                controller: _c,
                kind: 'track',
                artKey: '${k.track.itemId}',
                direct: k.track.art,
                size: 54,
                radius: 9,
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      _titleOf(k),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontFamily: Tokens.fontFamily,
                        fontSize: 18,
                        fontWeight: FontWeight.w800,
                        color: t.nInk,
                      ),
                    ),
                    Text(
                      [
                        if (k.track.artist.isNotEmpty) k.track.artist,
                        '${copies.length} ${copies.length == 1 ? "copy" : "copies"}',
                        _dur(k.track.durationS),
                        '$alike% alike',
                      ].join(' · '),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 12.5, color: t.nInk2),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 10),
              Flexible(
                child: Wrap(
                  alignment: WrapAlignment.end,
                  spacing: 6,
                  runSpacing: 6,
                  children: [
                    if (others.isNotEmpty)
                      pill('length ±${drift.toStringAsFixed(1)} s'),
                    if (differ.isNotEmpty)
                      pill(
                        '${differ.length == 1 ? differ.first : '${differ.sublist(0, differ.length - 1).join(', ')} and ${differ.last}'} '
                        '${differ.length == 1 ? "differs" : "differ"}',
                        warn: true,
                      ),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 12),
          Expanded(
            child: done
                ? Center(
                    child: Text(
                      copies.length == 1
                          ? '✓ Reviewed. One copy left, with every play and '
                              'playlist place.'
                          : '✓ Marked as not duplicates. This group will not '
                              'come back unless its copies change.',
                      textAlign: TextAlign.center,
                      style: TextStyle(fontSize: 13, color: t.nInk3),
                    ),
                  )
                // Side by side while each copy gets 250px; past that they
                // scroll sideways, rather than squeezing a card's buttons
                // off its edge.
                : LayoutBuilder(
                    builder: (context, box) => copies.length * 260 - 10 <=
                            box.maxWidth
                    ? Row(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          for (var i = 0; i < copies.length; i++) ...[
                            if (i > 0) const SizedBox(width: 10),
                            Expanded(child: _copy(context, g, copies[i], k)),
                          ],
                        ],
                      )
                    : ListView.separated(
                        scrollDirection: Axis.horizontal,
                        itemCount: copies.length,
                        separatorBuilder: (_, __) => const SizedBox(width: 10),
                        itemBuilder: (context, i) => SizedBox(
                          width: 250,
                          child: _copy(context, g, copies[i], k),
                        ),
                      ),
                  ),
          ),
          const SizedBox(height: 12),
          Row(
            children: [
              Expanded(
                child: Text(
                  'Tick any copies to delete, whatever their name or quality '
                  '· the copy you keep takes their plays and playlist places '
                  '· amber = differs from it',
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: _mono(11, t.nInk2, FontWeight.w400),
                ),
              ),
              const SizedBox(width: 10),
              if (!done) ...[
                _Btn('Not duplicates',
                    kind: _Kind.ghost, onTap: () => _dismiss(g)),
                const SizedBox(width: 8),
                _Btn(
                  ticked.isEmpty
                      ? 'Tick a copy to delete'
                      : keep == null
                          ? 'Keep at least one copy'
                          : 'Delete ${ticked.length} · '
                              '${_size(_extraBytes(g))}',
                  kind: _Kind.danger,
                  onTap: ticked.isEmpty || keep == null
                      ? null
                      : () => _delete(g, ticked),
                ),
                const SizedBox(width: 8),
              ],
              _Btn('Next group', icon: Icons.arrow_forward, onTap: _next),
            ],
          ),
        ],
      ),
    );
  }

  Widget _copy(BuildContext context, DupeGroup g, DupeCopy c, DupeCopy k) {
    final t = context.tokens;
    final isLead = c == k;
    final ticked = _drop(g).contains(c.track.itemId);
    final keep = isLead && !ticked;
    final lossless = _isLossless(c);
    Widget row(String label, String value,
        {bool diff = false, bool dim = false, bool path = false, String? tip}) {
      final text = Text(
        value.isEmpty ? '—' : value,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: path
            ? _mono(11, diff ? _amber : t.nInk3, FontWeight.w400)
            : TextStyle(
                fontSize: 12,
                color: diff
                    ? _amber
                    : dim
                        ? t.nInk2
                        : t.nInk,
                fontFeatures: const [FontFeature.tabularFigures()],
              ),
      );
      return Padding(
        padding: const EdgeInsets.only(bottom: 7),
        child: Row(
          children: [
            SizedBox(
              width: 78,
              child: Text(label.toUpperCase(), style: _caps(context)),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: tip == null ? text : Tooltip(message: tip, child: text),
            ),
          ],
        ),
      );
    }

    final moves = ticked && c.playlists > 0;
    return Container(
      decoration: BoxDecoration(
        color: t.nCanvas,
        borderRadius: BorderRadius.circular(13),
        border: Border.all(
          color: keep
              ? _good.withValues(alpha: 0.55)
              : ticked
                  ? _bad.withValues(alpha: 0.45)
                  : t.nHair,
        ),
      ),
      clipBehavior: Clip.antiAlias,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 13, vertical: 11),
            decoration: BoxDecoration(
              border: Border(bottom: BorderSide(color: t.nHair)),
            ),
            child: Row(
              children: [
                Tooltip(
                  message: ticked ? 'Keep this copy' : 'Delete this copy',
                  child: SizedBox(
                    width: 20,
                    height: 20,
                    child: Checkbox(
                      value: ticked,
                      activeColor: _bad,
                      materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                      visualDensity: VisualDensity.compact,
                      onChanged: (_) => setState(() {
                        final d = _drop(g);
                        if (!d.remove(c.track.itemId)) d.add(c.track.itemId);
                      }),
                    ),
                  ),
                ),
                const SizedBox(width: 9),
                Container(
                  padding:
                      const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
                  decoration: BoxDecoration(
                    color: lossless ? _good.withValues(alpha: 0.16) : t.nChip,
                    borderRadius: BorderRadius.circular(6),
                  ),
                  child: Text(
                    _codec(c).isEmpty ? '?' : _codec(c),
                    style: _mono(11, lossless ? _good : t.nInk, FontWeight.w700),
                  ),
                ),
                const Spacer(),
                if (keep) ...[
                  const Icon(Icons.check_rounded, size: 14, color: _good),
                  const SizedBox(width: 4),
                  Text('KEEP', style: _mono(10.5, _good, FontWeight.w700)),
                ] else if (ticked)
                  Text('DELETE', style: _mono(10.5, _bad, FontWeight.w700))
                else
                  Text('${c.confidence}% alike',
                      style: _mono(11, t.nInk3, FontWeight.w600)),
              ],
            ),
          ),
          Expanded(
            child: SingleChildScrollView(
              padding: const EdgeInsets.fromLTRB(13, 12, 13, 5),
              child: Column(
                children: [
                  row('Title', c.track.title,
                      diff: !isLead && !_same(c.track.title, k.track.title)),
                  row('Album', c.track.album,
                      diff: !isLead && !_same(c.track.album, k.track.album)),
                  row('Quality', c.quality, dim: !isLead),
                  row('Size', _size(c.bytes)),
                  row('Plays',
                      '${c.track.playCount}${c.track.loved ? ' · loved' : ''}'),
                  row(
                    'Playlists',
                    moves
                        ? '${c.playlists} · moves to the kept copy'
                        : '${c.playlists}',
                    diff: moves,
                  ),
                  row(
                    'File',
                    _shortPath(c.track.path),
                    path: true,
                    diff: !isLead &&
                        !_same(_base(c.track.path), _base(k.track.path)),
                    tip: c.track.path,
                  ),
                ],
              ),
            ),
          ),
          Container(
            padding: const EdgeInsets.fromLTRB(8, 8, 8, 8),
            decoration: BoxDecoration(
              border: Border(top: BorderSide(color: t.nHair)),
            ),
            child: Row(
              children: [
                _Btn(
                  'Play',
                  icon: Icons.play_arrow_rounded,
                  small: true,
                  kind: _Kind.ghost,
                  onTap: () => _c.send(MusicCmd.playList(
                    itemIds: Int64List.fromList([c.track.itemId]),
                    index: 0,
                    source: 'duplicates',
                  )),
                ),
                const Spacer(),
                _Btn(
                  'Show in Files',
                  icon: Icons.folder_open_outlined,
                  small: true,
                  kind: _Kind.ghost,
                  onTap: () =>
                      _c.send(MusicCmd.revealTrack(itemId: c.track.itemId)),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// One of the three stages a run goes through. `state`: 0 waiting, 1 running,
/// 2 done.
class _Step extends StatelessWidget {
  const _Step({
    required this.label,
    required this.title,
    required this.body,
    this.state = 0,
  });

  final String label;
  final String title;
  final String body;
  final int state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = switch (state) {
      1 => _pink,
      2 => _good,
      _ => t.nInk2,
    };
    return Container(
      padding: const EdgeInsets.fromLTRB(13, 12, 13, 12),
      decoration: BoxDecoration(
        color: t.nCanvas,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(
          color: state == 1 ? _pink.withValues(alpha: 0.6) : t.nHair,
        ),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(label.toUpperCase(), style: _caps(context).copyWith(color: tint)),
          const SizedBox(height: 6),
          Text(title,
              style: TextStyle(
                  fontSize: 13, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(height: 6),
          Text(body,
              style: TextStyle(fontSize: 11.5, color: t.nInk2, height: 1.45)),
        ],
      ),
    );
  }
}

/// Listens queued for ListenBrainz, and a way to push them.
///
/// Scrobbling has worked since it was written — a finished play is queued on
/// the track-change path and posted straight away — and nothing in the section
/// has ever said so. Offline, the queue is the point: it fills up and drains
/// when the network comes back, and until now there was no way to know either
/// had happened.
class ScrobbleRow extends StatefulWidget {
  const ScrobbleRow({super.key});

  @override
  State<ScrobbleRow> createState() => _ScrobbleRowState();
}

class _ScrobbleRowState extends State<ScrobbleRow> {
  int? _pending;
  bool _busy = false;

  @override
  void initState() {
    super.initState();
    _count();
  }

  Future<void> _count() async {
    try {
      final n = await scrobblePendingCount();
      if (mounted) setState(() => _pending = n.toInt());
    } catch (_) {
      // The service is off, or unconfigured. Nothing to report and nothing
      // worth an error: this is a footnote on a dialog about something else.
      if (mounted) setState(() => _pending = null);
    }
  }

  Future<void> _flush() async {
    setState(() => _busy = true);
    try {
      await scrobbleFlush();
    } catch (_) {
      // Same reasoning. A scrobble is a nicety; a listener should not be shown
      // an error because a website was down.
    }
    if (mounted) setState(() => _busy = false);
    await _count();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final n = _pending;
    // Nothing queued and nothing configured look the same from here, and both
    // are the state where this has nothing to say.
    if (n == null || n == 0) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.only(right: 8),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.cloud_upload_outlined, size: 15, color: t.nInk3),
          const SizedBox(width: 6),
          Text(
            '$n ${n == 1 ? "listen" : "listens"} waiting for ListenBrainz',
            style: TextStyle(fontSize: 12, color: t.nInk3),
          ),
          const SizedBox(width: 6),
          TextButton(
            onPressed: _busy ? null : _flush,
            child: Text(_busy ? 'Sending…' : 'Send now'),
          ),
        ],
      ),
    );
  }
}
