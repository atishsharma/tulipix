// The tray panel: click the status-bar icon, get the deck.
//
// docs/tray-player-deck.html, the "Ours" view. The Slint build draws this as
// `TrayPopup` in ui/mini_widget.slint, a second frameless always-on-top
// toplevel. Here it is a second toplevel too -- but a second VIEW on the one
// engine, not a second engine: `panel` in linux/runner/my_application.cc makes
// the window with `fl_view_new_for_engine`, and this isolate draws into it. So
// the panel reads `MusicController.instance` directly, the same object the
// player bar and the widget read, and every button on it is the `MusicCmd`
// those already send.
//
// The one ordering rule: the `View` leaves the tree BEFORE the window goes.
// The runner never closes the panel on its own; a click past it or Escape
// comes back as `onPanelDismiss`, [TrayPanel.close] drops the view, and only
// after that frame does it ask for the window to be destroyed.
//
// A panel that is shut costs nothing: no window, no view, no ticker.

import 'dart:io';
import 'dart:ui' show FlutterView, FontFeature;

import 'package:flutter/material.dart';

import '../design/app_theme.dart';
import '../design/skin.dart';
import '../design/tokens.dart';
import '../sections/music/music_controller.dart';
import '../sections/music/player_widgets.dart' show TrackBar;
import '../src/rust/api/music.dart';
import 'shell_controller.dart';
import 'window.dart';

/// Whether the panel is up. One flag, and the order its window comes and goes
/// in.
class TrayPanel extends ChangeNotifier {
  static final TrayPanel instance = TrayPanel._();

  TrayPanel._();

  bool open = false;
  DateTime _shut = DateTime(0);

  /// A left click on the icon. A second one closes it, as a second click on a
  /// tray icon closes its menu.
  void toggle() {
    if (open) return close();
    // That second click took focus off the panel on its way to the icon, so
    // the runner has already dismissed it by the time the click lands here --
    // and without this the click would open it straight back up.
    if (DateTime.now().difference(_shut) < const Duration(milliseconds: 400)) {
      return;
    }
    open = true;
    notifyListeners();
    setPanelWindow(true);
  }

  void close() {
    if (!open) return;
    open = false;
    _shut = DateTime.now();
    notifyListeners();
    WidgetsBinding.instance.endOfFrame.then((_) {
      if (!open) setPanelWindow(false);
    });
  }
}

/// The root: the app in the implicit view, and the panel in the other one
/// while it is open.
class AppViews extends StatefulWidget {
  const AppViews({super.key, required this.app});

  final Widget app;

  @override
  State<AppViews> createState() => _AppViewsState();
}

class _AppViewsState extends State<AppViews> with WidgetsBindingObserver {
  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addObserver(this);
    TrayPanel.instance.addListener(_changed);
  }

  @override
  void dispose() {
    WidgetsBinding.instance.removeObserver(this);
    TrayPanel.instance.removeListener(_changed);
    super.dispose();
  }

  void _changed() => setState(() {});

  /// A view added or removed arrives as a metrics change.
  @override
  void didChangeMetrics() => _changed();

  @override
  Widget build(BuildContext context) {
    final dispatcher = WidgetsBinding.instance.platformDispatcher;
    final main = dispatcher.implicitView!;
    FlutterView? panel;
    if (TrayPanel.instance.open) {
      for (final v in dispatcher.views) {
        if (v.viewId != main.viewId) panel = v;
      }
    }
    return ViewCollection(views: [
      View(view: main, child: widget.app),
      if (panel != null)
        View(view: panel, child: const _PanelTheme(child: _Panel())),
    ]);
  }
}

/// The app's theme, in a window the app's `MaterialApp` never reaches.
///
/// A second view is a second tree: nothing the main window's `Theme` says
/// arrives here. So the panel builds the same one, from the same two things
/// the shell reads -- the stored theme and the design language -- through the
/// same cached [themeFor] zen uses.
class _PanelTheme extends StatelessWidget {
  const _PanelTheme({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
        listenable: ShellController.instance,
        builder: (context, child) {
          final shell = ShellController.instance;
          final base = shell.theme == 'light'
              ? Tokens.light()
              : Tokens.dark(oled: shell.theme == 'extra-dark');
          return Theme(
              data: themeFor(shell.designLanguage, base), child: child!);
        },
        child: child,
      );
}

/// The panel's colours. Standard is the deck's own dark card, `--pop`: it
/// floats over the wallpaper, not over the app, and that was the design. Every
/// other language brings its own ink and its own slab.
class _Look {
  _Look.of(BuildContext context)
      : skin = context.skin,
        std = context.skin.isStandard,
        ink = context.skin.isStandard
            ? const Color(0xFFF2F1FA)
            : (context.skin.ink ?? context.tokens.nInk),
        ink2 = context.skin.isStandard
            ? const Color(0xFFB9B5D0)
            : (context.skin.inkDim ?? context.tokens.nInk2),
        ink3 = context.skin.isStandard
            ? const Color(0xFF8A86A4)
            : context.tokens.nInk3,
        rule = context.skin.isStandard
            ? const Color(0x12FFFFFF)
            : context.tokens.nHair,
        accent = context.skin.accent ?? Tokens.secMusic,
        canvas = context.skin.canvas ?? context.tokens.nCanvas;

  final AppSkin skin;
  final bool std;
  final Color ink;
  final Color ink2;
  final Color ink3;
  final Color rule;
  final Color accent;
  final Color canvas;

  Color get heart => const Color(0xFFF472B6);

  TextStyle get times => TextStyle(
        fontSize: 11,
        color: ink2,
        fontFeatures: const [FontFeature.tabularFigures()],
      );
}

class _Panel extends StatefulWidget {
  const _Panel();

  @override
  State<_Panel> createState() => _PanelState();
}

class _PanelState extends State<_Panel> {
  final _box = GlobalKey();
  int _height = 0;

  /// Asked once per open: the panel's State lives exactly as long as it does.
  final Future<Listening> _listening = musicListening(days: 0);

  MusicController get _c => MusicController.instance;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) => _report());
  }

  /// Tell the runner how tall this came out, whenever that changes: the
  /// artists arrive a beat after the rest, and the queue can be empty.
  void _report() {
    final h = _box.currentContext?.size?.height.ceil() ?? 0;
    if (h > 0 && h != _height) {
      _height = h;
      setPanelHeight(h);
    }
  }

  /// Every door out of the panel: do the thing, then go.
  void _leave(VoidCallback then) {
    then();
    TrayPanel.instance.close();
  }

  void _openApp() => _leave(presentWindow);

  void _openMusic(Future<void> Function() then) => _leave(() {
        presentWindow();
        if (_c.zenOpen) _c.closeZen();
        ShellController.instance.go(Section.music);
        then();
      });

  @override
  Widget build(BuildContext context) {
    final l = _Look.of(context);
    final theme = Theme.of(context);
    return Directionality(
      textDirection: TextDirection.ltr,
      child: DefaultTextStyle(
        style: TextStyle(
          color: l.ink,
          fontSize: 12,
          height: 1.3,
          fontFamily:
              l.skin.fontFamily ?? theme.textTheme.bodyMedium?.fontFamily,
        ),
        // Standard's card is dark whatever the app is, so its InkWells get a
        // white hover -- the theme's would be black on black. The other
        // languages' slabs are the app's own, and so is their hover.
        child: Theme(
          data: l.std
              ? theme.copyWith(
                  hoverColor: const Color(0x14FFFFFF),
                  splashColor: const Color(0x1FFFFFFF),
                  highlightColor: Colors.transparent,
                )
              : theme,
          child: Material(
            type: MaterialType.transparency,
            // The window is whatever height it was last cut to; the content is
            // laid out at its own and the window follows it.
            child: OverflowBox(
              alignment: Alignment.topCenter,
              minHeight: 0,
              maxHeight: double.infinity,
              child: NotificationListener<SizeChangedLayoutNotification>(
                onNotification: (_) {
                  WidgetsBinding.instance
                      .addPostFrameCallback((_) => _report());
                  return true;
                },
                child: SizeChangedLayoutNotifier(
                  child: Container(
                    key: _box,
                    // The page colour under the language's slab: the window
                    // paints on nothing, and a glass pane over nothing is a
                    // pane you read the desktop through.
                    decoration: l.std
                        ? BoxDecoration(
                            color: const Color(0xED141624),
                            borderRadius: BorderRadius.circular(15),
                            border: Border.all(color: const Color(0x1FFFFFFF)),
                          )
                        : BoxDecoration(
                            color: l.canvas,
                            borderRadius: BorderRadius.circular(15),
                          ),
                    clipBehavior: Clip.antiAlias,
                    child: l.std
                        ? _content()
                        : DecoratedBox(
                            decoration:
                                l.skin.surface(SurfaceRole.card, radius: 15) ??
                                    const BoxDecoration(),
                            child: _content(),
                          ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _content() => ListenableBuilder(
        listenable: _c.live,
        builder: (context, _) => Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _nowPlaying(),
            _artists(),
            _next(),
            _foot(),
          ],
        ),
      );

  Widget _nowPlaying() {
    final c = _c;
    final now = c.now;
    final has = now != null && (now.loaded || now.title.isNotEmpty);
    final library = now?.mode == 'music';
    final dur = c.tickDur;
    final repeat = c.state?.repeat ?? 'off';
    final shuffle = c.state?.shuffle ?? false;
    final l = _Look.of(context);
    return Container(
      padding: const EdgeInsets.fromLTRB(13, 13, 13, 0),
      decoration: BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [
            l.accent.withValues(alpha: 0.2),
            l.accent.withValues(alpha: 0),
          ],
        ),
      ),
      child: Column(crossAxisAlignment: CrossAxisAlignment.stretch, children: [
        Row(children: [
          _Art(
            path: now == null
                ? null
                : (now.art.isNotEmpty
                    ? now.art
                    : c.artFor('track', '${now.itemId}')),
            label: has ? now.artist : '',
            size: 58,
            radius: 10,
          ),
          const SizedBox(width: 11),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                _line1(
                    has ? now.title : 'Nothing playing',
                    const TextStyle(
                        fontSize: 13.5, fontWeight: FontWeight.w700)),
                _line1(has ? now.artist : 'Pick something in Tulipix',
                    TextStyle(fontSize: 11.5, color: l.ink2)),
              ],
            ),
          ),
          if (library && has)
            _Btn(
              icon: now.loved ? Icons.favorite : Icons.favorite_border,
              label: 'Favourite',
              color: now.loved ? l.heart : l.ink3,
              onTap: () => c.send(MusicCmd.love(itemId: now.itemId)),
            ),
        ]),
        const SizedBox(height: 8),
        _Bar(
          frac: dur > 0 ? (c.tickPos / dur).clamp(0.0, 1.0) : 0,
          fill: l.std ? Colors.white : l.accent,
          knob: true,
          onSet: dur > 0 ? (f) => c.send(MusicCmd.seek(secs: f * dur)) : null,
        ),
        Row(children: [
          Text(_clock(c.tickPos), style: l.times),
          const Spacer(),
          Text(_clock(dur), style: l.times),
        ]),
        Row(
          mainAxisAlignment: MainAxisAlignment.spaceBetween,
          children: [
            _Btn(
              icon: Icons.shuffle,
              label: 'Shuffle',
              on: shuffle,
              onTap: () => c.send(const MusicCmd.toggleShuffle()),
            ),
            _Btn(
              icon: Icons.skip_previous,
              label: 'Previous',
              onTap: () => c.send(const MusicCmd.prev()),
            ),
            _Btn(
              icon: c.tickPlaying ? Icons.pause : Icons.play_arrow,
              label: c.tickPlaying ? 'Pause' : 'Play',
              big: true,
              onTap: () => c.send(const MusicCmd.playPause()),
            ),
            _Btn(
              icon: Icons.skip_next,
              label: 'Next',
              onTap: () => c.send(const MusicCmd.next()),
            ),
            _Btn(
              icon: repeat == 'one' ? Icons.repeat_one : Icons.repeat,
              label: 'Repeat',
              on: repeat != 'off',
              onTap: () => c.send(const MusicCmd.cycleRepeat()),
            ),
          ],
        ),
        Padding(
          padding: const EdgeInsets.only(bottom: 9),
          child: Row(children: [
            _Btn(
              icon: c.muted ? Icons.volume_off : Icons.volume_up,
              label: c.muted ? 'Unmute' : 'Mute',
              small: true,
              onTap: () => c.send(const MusicCmd.toggleMute()),
            ),
            const SizedBox(width: 6),
            Expanded(
              child: _Bar(
                // The scale is mpv's 0..130; the bar is the 0..100 most
                // people mean, with the boost past its end.
                frac: c.muted ? 0 : (c.volume / 100).clamp(0.0, 1.0),
                fill: l.std ? const Color(0xBFFFFFFF) : l.accent,
                onSet: (f) => c.setVolume(f * 100),
              ),
            ),
            SizedBox(
              width: 34,
              child: Text(
                c.muted ? '—' : '${c.volume.round()}',
                textAlign: TextAlign.right,
                style: l.times.copyWith(fontWeight: FontWeight.w600),
              ),
            ),
          ]),
        ),
      ]),
    );
  }

  /// Four, not a scrolling row: a strip you have to drag inside a popup that
  /// closes when you click past it is a control that fights its container.
  Widget _artists() => FutureBuilder<Listening>(
        future: _listening,
        builder: (context, snap) {
          final rows = (snap.data?.artists ?? const <Tally>[]).take(4).toList();
          if (rows.isEmpty) return const SizedBox.shrink();
          return _Section(
            title: 'Favourite artists',
            note: 'your ${rows.length} most played',
            child: Row(
              mainAxisAlignment: MainAxisAlignment.spaceBetween,
              children: [
                for (final a in rows)
                  InkWell(
                    borderRadius: BorderRadius.circular(10),
                    onTap: a.key > 0
                        ? () => _openMusic(() async {
                              if (_c.view != 'mymusic') {
                                await _c.send(
                                    const MusicCmd.setView(name: 'mymusic'));
                              }
                              await _c
                                  .send(MusicCmd.openArtist(artistId: a.key));
                            })
                        : null,
                    child: SizedBox(
                      width: 68,
                      child: Column(children: [
                        // ponytail: the library keeps no artist pictures, so a
                        // circle is the artist's initial. Their most-played
                        // cover would be one more query; add it if the row
                        // reads as too plain.
                        _Art(label: a.label, size: 60, radius: 30),
                        const SizedBox(height: 5),
                        _line1(
                            a.label,
                            TextStyle(
                                fontSize: 10.5, color: _Look.of(context).ink2),
                            center: true),
                      ]),
                    ),
                  ),
              ],
            ),
          );
        },
      );

  Widget _next() {
    final queue = (_c.state?.queue ?? const <Track>[]).take(4).toList();
    if (queue.isEmpty) return const SizedBox.shrink();
    Widget cell(int i) {
      final t = queue[i];
      return InkWell(
        borderRadius: BorderRadius.circular(9),
        onTap: () => _c.send(MusicCmd.queuePlayAt(index: i)),
        child: Padding(
          padding: const EdgeInsets.all(5),
          child: Row(children: [
            _Art(
              path:
                  t.art.isNotEmpty ? t.art : _c.artFor('track', '${t.itemId}'),
              label: t.artist,
              size: 38,
              radius: 7,
            ),
            const SizedBox(width: 8),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _line1(
                      t.title,
                      const TextStyle(
                          fontSize: 11.5, fontWeight: FontWeight.w600)),
                  _line1(t.artist,
                      TextStyle(fontSize: 10.5, color: _Look.of(context).ink3)),
                ],
              ),
            ),
          ]),
        ),
      );
    }

    return _Section(
      title: 'Playing next',
      note: 'the next ${queue.length}',
      child: Column(children: [
        for (var r = 0; r < queue.length; r += 2)
          Row(children: [
            Expanded(child: cell(r)),
            const SizedBox(width: 8),
            Expanded(
                child: r + 1 < queue.length ? cell(r + 1) : const SizedBox()),
          ]),
      ]),
    );
  }

  Widget _foot() => Container(
        padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 6),
        decoration: BoxDecoration(
          color: _Look.of(context).std
              ? const Color(0x2E000000)
              : Colors.transparent,
          border: Border(top: BorderSide(color: _Look.of(context).rule)),
        ),
        child: Row(children: [
          _FootBtn(
              icon: Icons.open_in_new, label: 'Open Tulipix', onTap: _openApp),
          _FootBtn(
            icon: Icons.picture_in_picture_alt,
            label: 'Mini widget',
            // What the tray menu's Mini item does: the app becomes the widget.
            onTap: () => _leave(() {
              presentWindow();
              if (!_c.widgetOpen) _c.toggleWidget();
            }),
          ),
          const Spacer(),
          _FootBtn(
            icon: Icons.queue_music,
            tip: 'Queue',
            onTap: () => _openMusic(() async {
              if (_c.panel != 'queue') _c.setPanel('queue');
            }),
          ),
          const _FootBtn(
              icon: Icons.power_settings_new, tip: 'Quit', onTap: closeWindow),
        ]),
      );
}

String _clock(double s) {
  final t = s.isFinite && s > 0 ? s.floor() : 0;
  return '${t ~/ 60}:${(t % 60).toString().padLeft(2, '0')}';
}

Widget _line1(String text, TextStyle style, {bool center = false}) => Text(
      text,
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      softWrap: false,
      textAlign: center ? TextAlign.center : TextAlign.start,
      style: style,
    );

class _Section extends StatelessWidget {
  const _Section(
      {required this.title, required this.note, required this.child});

  final String title;
  final String note;
  final Widget child;

  @override
  Widget build(BuildContext context) => Container(
        padding: const EdgeInsets.fromLTRB(13, 11, 13, 11),
        decoration: BoxDecoration(
          border: Border(top: BorderSide(color: _Look.of(context).rule)),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text.rich(TextSpan(children: [
              TextSpan(
                text: '$title  ',
                style: const TextStyle(
                    fontSize: 12.5, fontWeight: FontWeight.w700),
              ),
              TextSpan(
                  text: note,
                  style:
                      TextStyle(fontSize: 11, color: _Look.of(context).ink3)),
            ])),
            const SizedBox(height: 9),
            child,
          ],
        ),
      );
}

/// A cover, or a gradient and an initial where there is none.
class _Art extends StatelessWidget {
  const _Art(
      {this.path,
      required this.label,
      required this.size,
      required this.radius});

  final String? path;
  final String label;
  final double size;
  final double radius;

  @override
  Widget build(BuildContext context) {
    final initial = label.trim().isEmpty ? '♪' : label.trim()[0].toUpperCase();
    // One hue per name, so an artist keeps their colour from open to open.
    final hue = (label.hashCode % 360).abs().toDouble();
    final plain = DecoratedBox(
      decoration: BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [
            HSLColor.fromAHSL(1, hue, .65, .55).toColor(),
            HSLColor.fromAHSL(1, hue, .6, .22).toColor(),
          ],
        ),
      ),
      child: Center(
        child: Text(initial,
            style: TextStyle(
                fontSize: size * .32,
                fontWeight: FontWeight.w800,
                color: Colors.white)),
      ),
    );
    final p = path;
    return ClipRRect(
      borderRadius: BorderRadius.circular(radius),
      child: SizedBox.square(
        dimension: size,
        child: p == null || p.isEmpty
            ? plain
            : Image.file(File(p),
                fit: BoxFit.cover, errorBuilder: (_, __, ___) => plain),
      ),
    );
  }
}

class _Btn extends StatelessWidget {
  const _Btn({
    required this.icon,
    required this.label,
    required this.onTap,
    this.on = false,
    this.big = false,
    this.small = false,
    this.color,
  });

  final IconData icon;
  final String label;
  final VoidCallback onTap;
  final bool on;
  final bool big;
  final bool small;
  final Color? color;

  @override
  Widget build(BuildContext context) {
    final l = _Look.of(context);
    if (!l.std) {
      // The language's round control, as the player bar's transport draws
      // it: play is the prominent one, shuffle and repeat latch.
      final d = big ? 40.0 : (small ? 28.0 : 34.0);
      return Semantics(
        button: true,
        label: label,
        child: SkinButton(
          width: d,
          height: d,
          radius: d / 2,
          active: on,
          prominent: big,
          onTap: onTap,
          child: Icon(
            l.skin.icon(icon),
            size: big ? 20 : (small ? 14 : 16),
            color: color ??
                (big
                    ? (l.skin.onProminent ?? l.accent)
                    : (on ? l.accent : l.ink2)),
            fill: on ? 1 : null,
          ),
        ),
      );
    }
    return Semantics(
      button: true,
      label: label,
      child: InkWell(
        onTap: onTap,
        borderRadius: BorderRadius.circular(9),
        child: Container(
          width: big ? 46 : (small ? 28 : 38),
          height: big ? 40 : (small ? 28 : 34),
          decoration: on
              ? BoxDecoration(
                  color: const Color(0x294ADE80),
                  borderRadius: BorderRadius.circular(9),
                )
              : null,
          child: Icon(
            icon,
            size: big ? 26 : (small ? 16 : 18),
            color: color ??
                (on ? const Color(0xFF4ADE80) : (big ? l.ink : l.ink2)),
          ),
        ),
      ),
    );
  }
}

class _FootBtn extends StatelessWidget {
  const _FootBtn(
      {required this.icon, this.label, this.tip, required this.onTap});

  final IconData icon;
  final String? label;
  final String? tip;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => Semantics(
        button: true,
        label: label ?? tip,
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(8),
          child: SizedBox(
            height: 28,
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 9),
              child: Row(mainAxisSize: MainAxisSize.min, children: [
                Icon(_Look.of(context).skin.icon(icon),
                    size: 15, color: _Look.of(context).ink2),
                if (label != null) ...[
                  const SizedBox(width: 6),
                  Text(label!,
                      style: TextStyle(
                          fontSize: 11.5, color: _Look.of(context).ink2)),
                ],
              ]),
            ),
          ),
        ),
      );
}

/// A 4px bar you can press anywhere on and drag: the seek line and the volume.
class _Bar extends StatefulWidget {
  const _Bar(
      {required this.frac, required this.fill, this.onSet, this.knob = false});

  final double frac;
  final Color fill;
  final ValueChanged<double>? onSet;
  final bool knob;

  @override
  State<_Bar> createState() => _BarState();
}

class _BarState extends State<_Bar> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) => LayoutBuilder(builder: (context, box) {
        final w = box.maxWidth;
        void at(Offset p) => widget.onSet?.call((p.dx / w).clamp(0.0, 1.0));
        return MouseRegion(
          cursor: widget.onSet == null
              ? MouseCursor.defer
              : SystemMouseCursors.click,
          onEnter: (_) => setState(() => _hover = true),
          onExit: (_) => setState(() => _hover = false),
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTapDown: (d) => at(d.localPosition),
            onHorizontalDragUpdate: (d) => at(d.localPosition),
            child: SizedBox(
              height: 14,
              // The player bar's groove in every other language.
              child: !_Look.of(context).std
                  ? Center(
                      child: TrackBar(
                        frac: widget.frac,
                        accent: widget.fill,
                        thickness: 4,
                      ),
                    )
                  : Stack(
                      alignment: Alignment.centerLeft,
                      clipBehavior: Clip.none,
                      children: [
                          Container(
                            height: 4,
                            decoration: BoxDecoration(
                              color: const Color(0x2EFFFFFF),
                              borderRadius: BorderRadius.circular(2),
                            ),
                          ),
                          Container(
                            width: w * widget.frac,
                            height: 4,
                            decoration: BoxDecoration(
                              color: widget.fill,
                              borderRadius: BorderRadius.circular(2),
                            ),
                          ),
                          if (widget.knob && _hover)
                            Positioned(
                              left: w * widget.frac - 5.5,
                              child: Container(
                                width: 11,
                                height: 11,
                                decoration: const BoxDecoration(
                                  color: Colors.white,
                                  shape: BoxShape.circle,
                                  boxShadow: [
                                    BoxShadow(
                                        color: Color(0x80000000), blurRadius: 4)
                                  ],
                                ),
                              ),
                            ),
                        ]),
            ),
          ),
        );
      });
}
