// The lock screen, and the overlay that decides when it shows
// (docs/mockups/lock-screen.html).
//
// An ambient display first: a clock, and a room lit and moved by what is
// playing — music, a paused film, or nothing (the wallpapers folder). A key or
// a click turns it into the sign-in: a PIN pad when a PIN is set, a button
// when not. The PIN is the core's (`account::verify_pin`), the same one the
// Slint build's lock screen checks.
//
// It is on screen for exactly the hours nobody is looking, so it is built to
// cost little:
//   * One motion clock — a 20 fps Timer, running only while motion is on and
//     the window is visible. The smoke and the wallpaper zoom read it; nothing
//     else on the screen animates between states.
//   * The smoke is shaded at a third of the window's size (smoke_layer.dart).
//   * The cover is blurred once per track into a 64 px image and scaled up.
//     No blur runs per frame, and nothing here is a BackdropFilter.
//   * The app under it stops its tickers at once and stops being painted once
//     the lock has faded in.
//   * The clock changes once a minute; the music face follows the player's
//     once-a-second tick.

import 'dart:async';
import 'dart:io';
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/app_mark.dart';
import '../../design/tokens.dart';
import '../../playback/audio_deck.dart';
import '../../playback/video_layer.dart';
import '../../sections/music/music_controller.dart';
import '../../src/rust/api/home.dart';
import '../../src/rust/api/lock.dart';
import '../../src/rust/api/music.dart';
import '../shell_controller.dart';
import 'lock_controller.dart';
import 'smoke_layer.dart';

/// Wraps the app: tracks input for the idle clock, dims before locking, and
/// puts the lock screen over everything — the video and the chat included.
class LockOverlay extends StatefulWidget {
  const LockOverlay({super.key, required this.child});

  final Widget child;

  @override
  State<LockOverlay> createState() => _LockOverlayState();
}

class _LockOverlayState extends State<LockOverlay> {
  final LockController _lock = LockController.instance;

  /// The lock screen is in the tree: locked, or fading out.
  bool _present = false;

  /// It has been drawn once at zero, so the fade in has somewhere to start.
  bool _shown = false;

  /// The app under it is not painted: set once the lock has faded in.
  bool _hideApp = false;
  Timer? _hideT;

  /// Where focus was, to hand it back on unlock.
  FocusNode? _before;

  @override
  void initState() {
    super.initState();
    _lock.addListener(_onLock);
    HardwareKeyboard.instance.addHandler(_onKey);
  }

  @override
  void dispose() {
    _lock.removeListener(_onLock);
    HardwareKeyboard.instance.removeHandler(_onKey);
    _hideT?.cancel();
    super.dispose();
  }

  bool _onKey(KeyEvent e) {
    _lock.markActive();
    return false;
  }

  void _mark(PointerEvent _) => _lock.markActive();

  void _onLock() {
    if (!mounted) return;
    if (_lock.locked && !_present) {
      _before = FocusManager.instance.primaryFocus;
      setState(() {
        _present = true;
        _shown = false;
      });
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) setState(() => _shown = true);
      });
      _hideT?.cancel();
      _hideT = Timer(const Duration(milliseconds: 900), () {
        if (mounted && _lock.locked) setState(() => _hideApp = true);
      });
    } else if (!_lock.locked && _present) {
      _hideT?.cancel();
      setState(() {
        _hideApp = false;
        _shown = false;
      });
      final before = _before;
      _before = null;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (before != null && before.context != null) before.requestFocus();
      });
    } else {
      setState(() {});
    }
  }

  @override
  Widget build(BuildContext context) {
    final locked = _lock.locked;
    final warn = _lock.warning;
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.keyL, control: true):
            _lock.lock,
      },
      child: Listener(
        behavior: HitTestBehavior.translucent,
        onPointerDown: _mark,
        onPointerMove: _mark,
        onPointerHover: _mark,
        onPointerSignal: _mark,
        child: Stack(
          fit: StackFit.expand,
          children: [
            ExcludeFocus(
              excluding: locked,
              child: TickerMode(
                enabled: !locked,
                child: Offstage(offstage: _hideApp, child: widget.child),
              ),
            ),
            IgnorePointer(
              child: AnimatedOpacity(
                opacity: warn > 0 ? .5 : 0,
                duration: Duration(milliseconds: warn > 0 ? 1500 : 250),
                child: const ColoredBox(color: Colors.black),
              ),
            ),
            if (warn > 0)
              Positioned(
                left: 0,
                right: 0,
                bottom: 40,
                child: Center(child: _WarnPill(seconds: warn)),
              ),
            if (_present)
              AnimatedOpacity(
                opacity: _shown && locked ? 1 : 0,
                duration: const Duration(milliseconds: 600),
                onEnd: () {
                  if (mounted && !_lock.locked) {
                    setState(() => _present = false);
                  }
                },
                child: const LockScreen(),
              ),
          ],
        ),
      ),
    );
  }
}

class _WarnPill extends StatelessWidget {
  const _WarnPill({required this.seconds});

  final int seconds;

  @override
  Widget build(BuildContext context) => Container(
        padding: const EdgeInsets.symmetric(horizontal: 18, vertical: 11),
        decoration: BoxDecoration(
          color: const Color(0xEB141518),
          borderRadius: BorderRadius.circular(999),
          border: Border.all(color: const Color(0x33FFFFFF)),
        ),
        child: Text(
          'Locking in $seconds s · move the pointer to stay',
          style: const TextStyle(
              fontSize: 14, fontWeight: FontWeight.w600, color: Colors.white),
        ),
      );
}

// ── the lock screen ─────────────────────────────────────────────────────────

class LockScreen extends StatefulWidget {
  const LockScreen({super.key});

  @override
  State<LockScreen> createState() => _LockScreenState();
}

class _LockScreenState extends State<LockScreen> {
  final ValueNotifier<double> _clock = ValueNotifier(0);
  final ValueNotifier<Offset> _ptr = ValueNotifier(const Offset(.5, .3));
  final FocusNode _focus = FocusNode(debugLabel: 'lock screen');
  final Stopwatch _watch = Stopwatch();
  Timer? _motion;
  AppLifecycleListener? _life;
  bool _visible = true;
  Size _size = Size.zero;

  late final Listenable _deps = Listenable.merge([
    MusicController.instance,
    videoRequest,
    videoPlaying,
    LockController.instance,
    ShellController.instance,
  ]);

  // sign-in
  bool _signin = false;
  String _entry = '';
  String _msg = '';
  bool _msgInfo = false;
  int _tries = 0;
  int _shake = 0;
  bool _checking = false;
  DateTime? _waitUntil;
  Timer? _back;
  Timer? _waitT;
  bool _resume = false;

  /// The PIN's digits are shown rather than dots.
  bool _reveal = false;

  /// Stop was pressed: the music face gives way to the clock for the rest of
  /// this lock, whatever the player reports on its way to idle.
  bool _musicHidden = false;

  // what is behind
  ui.Image? _artBlur;
  String _artFor = '';
  Uint8List? _frame;
  List<File> _walls = const [];
  int _wall = 0;
  Timer? _wallT;
  HomeContinue? _continue;

  LockConfig? get _cfg => LockController.instance.config;
  bool get _hasPin => _cfg?.hasPin ?? false;

  /// A fingerprint read or a key touch is outstanding. The button says so and
  /// a second press does nothing: fprintd allows one verify at a time.
  bool _passkeyBusy = false;
  bool get _moving =>
      (_cfg?.motion ?? true) &&
      !(ShellController.instance.state?.reduceMotion ?? false);

  @override
  void initState() {
    super.initState();
    _life = AppLifecycleListener(onStateChange: (s) {
      final visible = s != AppLifecycleState.hidden &&
          s != AppLifecycleState.paused &&
          s != AppLifecycleState.detached;
      if (visible == _visible) return;
      setState(() => _visible = visible);
      _syncMotion();
    });
    _syncMotion();
    MusicController.instance.addListener(_onMusic);
    _onMusic();
    if (_face == 'video') {
      videoFrame?.call().then((b) {
        if (mounted && b != null) setState(() => _frame = b);
      }).catchError((Object _) {});
    }
    _loadWalls();
    if (_cfg?.showGlance ?? true) {
      homeDispatch(cmd: const HomeCmd.refresh()).then((h) {
        if (mounted && h.continueRows.isNotEmpty) {
          setState(() => _continue = h.continueRows.first);
        }
      }).catchError((Object _) {});
    }
  }

  @override
  void dispose() {
    _motion?.cancel();
    _back?.cancel();
    _waitT?.cancel();
    _wallT?.cancel();
    _life?.dispose();
    MusicController.instance.removeListener(_onMusic);
    _clock.dispose();
    _ptr.dispose();
    _focus.dispose();
    _artBlur?.dispose();
    super.dispose();
  }

  /// The one clock anything here moves by, at 20 fps, and only while it can
  /// be seen and motion is allowed.
  void _syncMotion() {
    final on = _moving && _visible;
    if (on && _motion == null) {
      _watch.start();
      _motion = Timer.periodic(const Duration(milliseconds: 50), (_) {
        _clock.value = _watch.elapsedMilliseconds / 1000.0;
      });
    } else if (!on && _motion != null) {
      _motion!.cancel();
      _motion = null;
      _watch.stop();
    }
  }

  // ── what shows ────────────────────────────────────────────────────────────

  /// video | music | still. A paused film wins, then music that is loaded
  /// (playing or paused), then the wallpapers.
  String get _face {
    final cfg = _cfg;
    if ((cfg?.showVideo ?? true) &&
        videoRequest.value != null &&
        !videoPlaying.value) {
      return 'video';
    }
    final now = MusicController.instance.now;
    if ((cfg?.showMusic ?? true) &&
        !_musicHidden &&
        now != null &&
        now.loaded &&
        now.mode != 'idle') {
      return 'music';
    }
    return 'still';
  }

  List<Color> _colors(String face) {
    switch (face) {
      case 'music':
        final m = MusicController.instance;
        final h = HSLColor.fromColor(m.accent);
        return [
          m.accent,
          m.accentAlt,
          h.withHue((h.hue + 150) % 360).toColor(),
        ];
      case 'video':
        return const [Color(0xFF34D399), Color(0xFF22D3EE), Color(0xFF8B5CF6)];
      default:
        return const [Tokens.brand, Tokens.brand2, Color(0xFF0EA5E9)];
    }
  }

  /// The smoke's loudness: the track's stored waveform at the playhead, not a
  /// live tap on the audio, with a slow swell so a flat stretch still breathes.
  double _energy(String face) {
    if (face != 'music') return .15;
    final m = MusicController.instance;
    if (!m.tickPlaying) return .08;
    final now = m.now;
    var loud = .5;
    final wave = now == null ? null : m.waveFor(now.itemId);
    if (wave != null && wave.isNotEmpty && m.tickDur > 0) {
      final i = (m.tickPos / m.tickDur * wave.length)
          .floor()
          .clamp(0, wave.length - 1);
      loud = wave[i] / 255.0;
    }
    return (.15 + .6 * loud + .12 * math.sin(_clock.value * 1.3))
        .clamp(0.0, 1.0);
  }

  void _onMusic() {
    final art = MusicController.instance.now?.art ?? '';
    if (art != _artFor) _blurArt(art);
  }

  /// The cover, blurred once into 64 px: scaled up to the window it reads as
  /// light rather than as a picture, and costs nothing per frame.
  Future<void> _blurArt(String path) async {
    _artFor = path;
    ui.Image? out;
    if (path.isNotEmpty) {
      try {
        final codec = await ui.instantiateImageCodec(
            await File(path).readAsBytes(),
            targetWidth: 96);
        final src = (await codec.getNextFrame()).image;
        codec.dispose();
        final rec = ui.PictureRecorder();
        Canvas(rec).drawImageRect(
          src,
          Rect.fromLTWH(0, 0, src.width.toDouble(), src.height.toDouble()),
          const Rect.fromLTWH(0, 0, 64, 64),
          Paint()
            ..filterQuality = FilterQuality.medium
            ..imageFilter = ui.ImageFilter.blur(
                sigmaX: 5, sigmaY: 5, tileMode: TileMode.clamp),
        );
        final pic = rec.endRecording();
        out = await pic.toImage(64, 64);
        pic.dispose();
        src.dispose();
      } catch (_) {
        out = null;
      }
    }
    if (!mounted || _artFor != path) {
      out?.dispose();
      return;
    }
    setState(() {
      _artBlur?.dispose();
      _artBlur = out;
    });
  }

  Future<void> _loadWalls() async {
    final dir = _cfg?.wallpapers.trim() ?? '';
    if (dir.isEmpty) return;
    try {
      final files = await Directory(dir)
          .list()
          .where((e) => e is File && _isImage(e.path))
          .cast<File>()
          .toList();
      files.sort((a, b) => a.path.compareTo(b.path));
      if (!mounted || files.isEmpty) return;
      setState(() => _walls = files.take(10).toList());
      if (_walls.length > 1) {
        _wallT = Timer.periodic(const Duration(seconds: 12), (_) {
          if (mounted && _face == 'still') {
            setState(() => _wall = (_wall + 1) % _walls.length);
          }
        });
      }
    } catch (_) {
      // A folder that went away is the gradient, not an error on a lock screen.
    }
  }

  static bool _isImage(String p) {
    final l = p.toLowerCase();
    return l.endsWith('.jpg') ||
        l.endsWith('.jpeg') ||
        l.endsWith('.png') ||
        l.endsWith('.webp');
  }

  // ── input ─────────────────────────────────────────────────────────────────

  void _bump() {
    _back?.cancel();
    if (_signin) {
      _back = Timer(const Duration(seconds: 20), () {
        if (mounted) _closeSignin();
      });
    }
  }

  void _openSignin({String info = ''}) {
    setState(() {
      _signin = true;
      if (_waitUntil == null) {
        _msg = info;
        _msgInfo = info.isNotEmpty;
      }
    });
    _bump();
  }

  void _closeSignin() {
    _back?.cancel();
    setState(() {
      _signin = false;
      _entry = '';
      _reveal = false;
      _resume = false;
      if (_waitUntil == null) _msg = '';
    });
  }

  bool get _waiting =>
      _waitUntil != null && DateTime.now().isBefore(_waitUntil!);

  void _type(String d) {
    if (_waiting || _entry.length >= 8) return;
    setState(() {
      _entry += d;
      if (!_msgInfo) _msg = '';
    });
    // With the length known, the last digit is the Enter. A wrong one counts
    // as a try, exactly as Enter would.
    final len = _cfg?.pinLen ?? 0;
    if (len > 0 && _entry.length == len) _submit();
  }

  void _backspace() {
    if (_entry.isEmpty) return;
    setState(() => _entry = _entry.substring(0, _entry.length - 1));
  }

  Future<void> _submit() async {
    if (_entry.isEmpty || _checking || _waiting) return;
    _checking = true;
    final ok = await LockController.instance.tryPin(_entry);
    _checking = false;
    if (!mounted) return;
    if (ok) {
      _afterUnlock();
      return;
    }
    setState(() {
      _entry = '';
      _shake++;
      _tries++;
      _msgInfo = false;
      if (_tries >= 5) {
        _tries = 0;
        _waitUntil = DateTime.now().add(const Duration(seconds: 30));
        _msg = 'Too many tries. Wait 30 s.';
        _waitT?.cancel();
        _waitT = Timer.periodic(const Duration(seconds: 1), (_) {
          if (!mounted) return;
          final left = _waitUntil!.difference(DateTime.now()).inSeconds + 1;
          setState(() {
            if (left <= 0 || !_waiting) {
              _waitT?.cancel();
              _waitUntil = null;
              _msg = '';
            } else {
              _msg = 'Too many tries. Wait $left s.';
            }
          });
        });
      } else {
        _msg = 'That PIN isn’t right. Try again.';
      }
    });
  }

  void _unlock() {
    LockController.instance.unlock();
    _afterUnlock();
  }

  void _afterUnlock() {
    if (_resume) videoResume?.call();
  }

  /// Stop the music and put the clock back.
  void _stopMusic() {
    setState(() => _musicHidden = true);
    MusicController.instance.send(const MusicCmd.stop());
  }

  void _resumeVideo() {
    _resume = true;
    if (_hasPin) {
      _openSignin(info: 'Unlock to resume');
    } else {
      _unlock();
    }
  }

  static bool _isModifier(LogicalKeyboardKey k) =>
      k == LogicalKeyboardKey.controlLeft ||
      k == LogicalKeyboardKey.controlRight ||
      k == LogicalKeyboardKey.shiftLeft ||
      k == LogicalKeyboardKey.shiftRight ||
      k == LogicalKeyboardKey.altLeft ||
      k == LogicalKeyboardKey.altRight ||
      k == LogicalKeyboardKey.metaLeft ||
      k == LogicalKeyboardKey.metaRight ||
      k == LogicalKeyboardKey.tab;

  static String? _digit(KeyEvent e) {
    final c = e.character;
    return c != null && c.length == 1 && '0123456789'.contains(c) ? c : null;
  }

  /// Every key stops here while locked: the app's own shortcuts are under the
  /// lock and must not answer.
  KeyEventResult _onKey(FocusNode node, KeyEvent e) {
    if (_isModifier(e.logicalKey)) return KeyEventResult.ignored;
    if (e is! KeyDownEvent && e is! KeyRepeatEvent) {
      return KeyEventResult.handled;
    }
    final key = e.logicalKey;
    if (!_signin) {
      _openSignin();
      final d = _digit(e);
      if (d != null && _hasPin) _type(d);
      return KeyEventResult.handled;
    }
    _bump();
    final enter = key == LogicalKeyboardKey.enter ||
        key == LogicalKeyboardKey.numpadEnter;
    if (key == LogicalKeyboardKey.escape) {
      _closeSignin();
    } else if (_hasPin) {
      final d = _digit(e);
      if (d != null) {
        _type(d);
      } else if (key == LogicalKeyboardKey.backspace) {
        _backspace();
      } else if (enter) {
        _submit();
      }
    } else if (enter || key == LogicalKeyboardKey.space) {
      _unlock();
    }
    return KeyEventResult.handled;
  }

  // ── build ─────────────────────────────────────────────────────────────────

  @override
  Widget build(BuildContext context) {
    return Material(
      type: MaterialType.transparency,
      child: Focus(
        focusNode: _focus,
        autofocus: true,
        onKeyEvent: _onKey,
        child: MouseRegion(
          onHover: (e) {
            if (_size.isEmpty) return;
            _ptr.value = Offset(e.localPosition.dx / _size.width,
                1 - e.localPosition.dy / _size.height);
          },
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: () => _signin ? _bump() : _openSignin(),
            child: LayoutBuilder(
              builder: (context, box) {
                _size = box.biggest;
                return ListenableBuilder(
                  listenable: _deps,
                  builder: (context, _) => _scene(box.biggest),
                );
              },
            ),
          ),
        ),
      ),
    );
  }

  Widget _scene(Size size) {
    final face = _face;
    final k = (size.height / 800).clamp(0.7, 1.35).toDouble();
    return Stack(
      fit: StackFit.expand,
      children: [
        _backdrop(face),
        RepaintBoundary(
          child: SmokeLayer(
            colors: _colors(face),
            calm: (face == 'music' ? .95 : .55) * (_signin ? .6 : 1),
            energy: () => _energy(face),
            clock: _clock,
            pointer: _ptr,
            moving: _moving && _visible,
          ),
        ),
        const IgnorePointer(child: _Vignette()),
        RepaintBoundary(
          child: IgnorePointer(
            ignoring: _signin,
            child: AnimatedOpacity(
              opacity: _signin ? 0 : 1,
              duration: const Duration(milliseconds: 350),
              child: AnimatedSlide(
                offset: _signin ? const Offset(0, -.04) : Offset.zero,
                duration: const Duration(milliseconds: 500),
                curve: Curves.easeOutCubic,
                child: _ambient(face, k),
              ),
            ),
          ),
        ),
        IgnorePointer(
          ignoring: !_signin,
          child: AnimatedOpacity(
            opacity: _signin ? 1 : 0,
            duration: const Duration(milliseconds: 400),
            child: AnimatedSlide(
              offset: _signin ? Offset.zero : const Offset(0, .03),
              duration: const Duration(milliseconds: 500),
              curve: Curves.easeOutCubic,
              child: _signinView(face),
            ),
          ),
        ),
        const Positioned(left: 28, bottom: 22, child: _Locked()),
        const Positioned(right: 28, bottom: 22, child: _Health()),
      ],
    );
  }

  Widget _backdrop(String face) {
    final dim = _signin ? const Color(0x99000000) : const Color(0x59000000);
    switch (face) {
      case 'music':
        final m = MusicController.instance;
        return Stack(fit: StackFit.expand, children: [
          if (_artBlur != null)
            RawImage(
              image: _artBlur,
              fit: BoxFit.cover,
              filterQuality: FilterQuality.medium,
            )
          else
            DecoratedBox(
              decoration: BoxDecoration(
                gradient: LinearGradient(
                  begin: Alignment.topLeft,
                  end: Alignment.bottomRight,
                  colors: [
                    Color.lerp(m.accent, Colors.black, .6)!,
                    Color.lerp(m.accentAlt, Colors.black, .75)!,
                  ],
                ),
              ),
            ),
          ColoredBox(color: dim),
        ]);
      case 'video':
        return Stack(fit: StackFit.expand, children: [
          if (_frame != null)
            Image.memory(_frame!, fit: BoxFit.cover, gaplessPlayback: true)
          else
            const ColoredBox(color: Color(0xFF05060A)),
          ColoredBox(color: _signin ? const Color(0xB3000000) : dim),
        ]);
      default:
        return Stack(fit: StackFit.expand, children: [
          if (_walls.isEmpty)
            const DecoratedBox(
              decoration: BoxDecoration(
                gradient: LinearGradient(
                  begin: Alignment.topCenter,
                  end: Alignment.bottomCenter,
                  colors: [Color(0xFF0B0B1A), Color(0xFF1B1035)],
                ),
              ),
            )
          else
            AnimatedSwitcher(
              duration: const Duration(seconds: 2),
              child: KeyedSubtree(
                key: ValueKey(_wall),
                child: _kenBurns(_walls[_wall]),
              ),
            ),
          ColoredBox(color: dim),
        ]);
    }
  }

  /// A wallpaper, drifting in over 26 s and back out, on the motion clock.
  Widget _kenBurns(File f) {
    final px = (_size.width * MediaQuery.devicePixelRatioOf(context)).round();
    final img = Image.file(
      f,
      fit: BoxFit.cover,
      cacheWidth: px > 0 ? px : null,
      gaplessPlayback: true,
      errorBuilder: (context, error, stack) => const SizedBox.shrink(),
    );
    if (!_moving) return img;
    return ValueListenableBuilder<double>(
      valueListenable: _clock,
      child: img,
      builder: (context, t, child) => Transform.scale(
        scale: 1.04 + .1 * (.5 - .5 * math.cos(t / 26 * math.pi)),
        child: child,
      ),
    );
  }

  // ── the clock and the faces ───────────────────────────────────────────────

  Widget _ambient(String face, double k) {
    final glance = face == 'still' && (_cfg?.showGlance ?? true);
    return Stack(
      children: [
        if (face == 'still')
          Positioned(
            left: 0,
            right: 0,
            top: 110 * k,
            child: _LockClock(size: 150 * k, center: true),
          )
        else
          Positioned(
            left: 56 * k,
            top: 46 * k,
            child: _LockClock(size: 84 * k),
          ),
        // The mark and the name, bottom centre, on the row with Locked and
        // the health line. Part of the ambient face, so the sign-in's own
        // bottom line never lands on it.
        Positioned(
          left: 0,
          right: 0,
          bottom: 16,
          child: Center(child: _LockBrand(size: 30 * k)),
        ),
        if (face == 'music')
          Positioned(
            left: 96 * k,
            right: 96 * k,
            top: 216 * k,
            child: _musicFace(k),
          ),
        if (face == 'video')
          Positioned(left: 56 * k, bottom: 72 * k, child: _videoFace(k)),
        if (glance)
          Positioned(left: 24, right: 24, bottom: 88 * k, child: _glance(k)),
      ],
    );
  }

  Widget _musicFace(double k) {
    final m = MusicController.instance;
    return ListenableBuilder(
      listenable: m.live,
      builder: (context, _) {
        final now = m.now;
        if (now == null) return const SizedBox.shrink();
        final cfg = _cfg;
        final lines = m.state?.lyrics ?? const <LyricLine>[];
        final lyrics = (cfg?.showLyrics ?? true) && lines.isNotEmpty;
        final side = 320 * k;
        final px = (side * MediaQuery.devicePixelRatioOf(context)).round();
        return Row(
          children: [
            Container(
              width: side,
              height: side,
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(22 * k),
                boxShadow: [
                  const BoxShadow(
                      color: Color(0x8C000000),
                      blurRadius: 60,
                      offset: Offset(0, 24)),
                  BoxShadow(
                      color: m.accent.withValues(alpha: .4),
                      blurRadius: 90,
                      spreadRadius: 4),
                ],
              ),
              child: ClipRRect(
                borderRadius: BorderRadius.circular(22 * k),
                child: now.art.isEmpty
                    ? ColoredBox(
                        color: m.accent,
                        child: const Icon(Icons.music_note,
                            size: 64, color: Colors.white70),
                      )
                    : Image.file(
                        File(now.art),
                        fit: BoxFit.cover,
                        cacheWidth: px,
                        gaplessPlayback: true,
                        errorBuilder: (context, error, stack) =>
                            ColoredBox(color: m.accent),
                      ),
              ),
            ),
            SizedBox(width: 56 * k),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  _eyebrow(m.tickPlaying ? 'Now playing' : 'Paused'),
                  SizedBox(height: 12 * k),
                  Text(now.title,
                      maxLines: 2,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 46 * k,
                          fontWeight: FontWeight.w700,
                          height: 1.08,
                          letterSpacing: -.5,
                          color: Colors.white)),
                  SizedBox(height: 8 * k),
                  Text(
                      [now.artist, now.album]
                          .where((s) => s.isNotEmpty)
                          .join(' · '),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 19 * k, color: const Color(0xD9FFFFFF))),
                  if (lyrics) ...[
                    SizedBox(height: 28 * k),
                    _lyrics(lines, m.activeLyric, k),
                  ],
                  SizedBox(height: 24 * k),
                  _LockSeek(
                    pos: m.tickPos,
                    dur: m.tickDur,
                    tint: m.accent,
                    k: k,
                    // Without media controls the bar is a reading, not a
                    // control.
                    onSeek: (cfg?.mediaControls ?? true)
                        ? (s) => m.send(MusicCmd.seek(secs: s))
                        : null,
                  ),
                  if (cfg?.mediaControls ?? true) ...[
                    SizedBox(height: 20 * k),
                    // A Wrap, not a Row: with the volume and Stop the row is
                    // wider than a small window leaves beside the cover.
                    Wrap(
                      spacing: 12,
                      runSpacing: 12,
                      crossAxisAlignment: WrapCrossAlignment.center,
                      children: [
                        _RoundBtn(
                          icon: Icons.skip_previous_rounded,
                          tip: 'Previous',
                          onTap: () => m.send(const MusicCmd.prev()),
                        ),
                        _RoundBtn(
                          icon: m.tickPlaying
                              ? Icons.pause_rounded
                              : Icons.play_arrow_rounded,
                          tip: m.tickPlaying ? 'Pause' : 'Play',
                          big: true,
                          onTap: () => m.send(const MusicCmd.playPause()),
                        ),
                        _RoundBtn(
                          icon: Icons.skip_next_rounded,
                          tip: 'Next',
                          onTap: () => m.send(const MusicCmd.next()),
                        ),
                        _RoundBtn(
                          icon: now.loved
                              ? Icons.favorite_rounded
                              : Icons.favorite_border_rounded,
                          tip: now.loved ? 'Loved' : 'Love',
                          tint: now.loved ? const Color(0xFFF472B6) : null,
                          onTap: m.keyLove,
                        ),
                        _LockVolume(barWidth: 130 * k),
                        _RoundBtn(
                          icon: Icons.stop_rounded,
                          tip: 'Stop, and back to the clock',
                          onTap: _stopMusic,
                        ),
                      ],
                    ),
                  ],
                ],
              ),
            ),
          ],
        );
      },
    );
  }

  Widget _lyrics(List<LyricLine> lines, int at, double k) {
    String line(int i) => i >= 0 && i < lines.length ? lines[i].text : '';
    final dim = TextStyle(
        fontSize: 18 * k, color: const Color(0x61FFFFFF), height: 1.35);
    return AnimatedSwitcher(
      duration: const Duration(milliseconds: 400),
      child: Column(
        key: ValueKey(at),
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(line(at - 1),
              maxLines: 1, overflow: TextOverflow.ellipsis, style: dim),
          SizedBox(height: 6 * k),
          Text(line(at),
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 28 * k,
                  fontWeight: FontWeight.w600,
                  height: 1.25,
                  color: Colors.white)),
          SizedBox(height: 6 * k),
          Text(line(at + 1),
              maxLines: 1, overflow: TextOverflow.ellipsis, style: dim),
        ],
      ),
    );
  }

  static String _fmt(double s) {
    final t = s.isFinite && s > 0 ? s.floor() : 0;
    return '${t ~/ 60}:${(t % 60).toString().padLeft(2, '0')}';
  }

  static String _videoTitle(VideoRequest? r) {
    if (r == null) return '';
    final src = r.src;
    final slash = math.max(src.lastIndexOf('/'), src.lastIndexOf('\\'));
    final base = src.substring(slash + 1);
    final dot = base.lastIndexOf('.');
    final stem = dot > 0 ? base.substring(0, dot) : base;
    return stem.replaceAll('.', ' ').replaceAll('_', ' ').trim();
  }

  Widget _videoFace(double k) => Container(
        width: 600 * k,
        padding: EdgeInsets.all(24 * k),
        decoration: _glass(22 * k),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            _eyebrow('Paused'),
            SizedBox(height: 14 * k),
            Text(_videoTitle(videoRequest.value),
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 38 * k,
                    fontWeight: FontWeight.w700,
                    letterSpacing: -.5,
                    color: Colors.white)),
            SizedBox(height: 20 * k),
            Row(children: [
              _Pill(
                  label: 'Resume',
                  icon: Icons.play_arrow_rounded,
                  filled: true,
                  onTap: _resumeVideo),
              const SizedBox(width: 14),
              Flexible(
                child: Text(
                  _hasPin
                      ? 'Unlock to carry on where it stopped'
                      : 'Carries on where it stopped',
                  style:
                      const TextStyle(fontSize: 13, color: Color(0xBFFFFFFF)),
                ),
              ),
            ]),
          ],
        ),
      );

  Widget _glance(double k) {
    final shell = ShellController.instance.state;
    final bills = shell?.financesBadge ?? 0;
    final c = _continue;
    final cards = <Widget>[
      if (c != null)
        _GlanceCard(
          icon: Icons.play_circle_outline,
          label: 'Continue',
          title: c.title,
          sub: c.sub,
          frac: c.frac,
        ),
      if (bills > 0)
        _GlanceCard(
          icon: Icons.receipt_long_outlined,
          label: 'Due this week',
          title: '$bills ${bills == 1 ? 'bill' : 'bills'}',
          sub: shell!.financesOverdue ? 'Something is overdue' : 'Nothing late',
        ),
      if (shell != null && shell.user.secondary.isNotEmpty)
        _GlanceCard(
          icon: Icons.inventory_2_outlined,
          label: 'Library',
          title: shell.user.secondary,
          sub: 'On this computer',
        ),
    ];
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        for (var i = 0; i < cards.length; i++) ...[
          if (i > 0) const SizedBox(width: 14),
          SizedBox(width: 240 * k, child: cards[i]),
        ],
      ],
    );
  }

  // ── sign-in ───────────────────────────────────────────────────────────────

  Widget _signinView(String face) {
    final user = ShellController.instance.state?.user;
    return Stack(children: [
      Positioned.fill(
        bottom: 70,
        child: Center(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              _Avatar(
                size: 116,
                path: user?.avatarPath ?? '',
                emoji: user?.avatarEmoji ?? '',
              ),
              const SizedBox(height: 14),
              Text(user?.displayName ?? 'Local user',
                  style: const TextStyle(
                      fontSize: 30,
                      fontWeight: FontWeight.w600,
                      color: Colors.white)),
              const SizedBox(height: 16),
              if (_hasPin) ..._pinPad() else ..._noPin(),
              ..._passkey(),
              ..._profilePicker(),
            ],
          ),
        ),
      ),
      if (face == 'music' && (_cfg?.mediaControls ?? true))
        Positioned(
          left: 0,
          right: 0,
          bottom: 58,
          child: Center(child: _miniPlayer()),
        ),
      const Positioned(
        left: 0,
        right: 0,
        bottom: 22,
        child: Center(
          child: Text('Esc · back to the clock',
              style: TextStyle(fontSize: 11.5, color: Color(0xB3FFFFFF))),
        ),
      ),
    ]);
  }

  List<Widget> _pinPad() {
    // As many slots as the PIN has digits, when that is known.
    final slots = math.max(_cfg?.pinLen ?? 0, math.max(4, _entry.length));
    final gap = slots > 6 ? 10.0 : 16.0;
    Widget slot(int i) {
      if (_reveal && i < _entry.length) {
        return Text(_entry[i],
            textAlign: TextAlign.center,
            style: const TextStyle(
                fontSize: 19,
                fontWeight: FontWeight.w500,
                color: Colors.white,
                fontFeatures: [FontFeature.tabularFigures()]));
      }
      return Center(
        child: Container(
          width: 12,
          height: 12,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: i < _entry.length ? Colors.white : null,
            border: Border.all(color: const Color(0xBFFFFFFF), width: 1.5),
          ),
        ),
      );
    }

    final dots = Container(
      width: 290,
      height: 50,
      // The left pad matches the eye on the right, so the slots stay centred.
      padding: const EdgeInsets.only(left: 44, right: 6),
      decoration: _glass(12),
      child: Row(
        children: [
          Expanded(
            child: Row(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                for (var i = 0; i < slots; i++) ...[
                  if (i > 0) SizedBox(width: gap),
                  SizedBox(width: 14, child: slot(i)),
                ],
              ],
            ),
          ),
          // A plain gesture rather than a button: nothing here may take the
          // keyboard focus off the pad.
          Tooltip(
            message: _reveal ? 'Hide the digits' : 'Show the digits',
            child: MouseRegion(
              cursor: SystemMouseCursors.click,
              child: GestureDetector(
                behavior: HitTestBehavior.opaque,
                onTap: () {
                  _bump();
                  setState(() => _reveal = !_reveal);
                },
                child: SizedBox(
                  width: 38,
                  height: 38,
                  child: Icon(
                    _reveal
                        ? Icons.visibility_off_outlined
                        : Icons.visibility_outlined,
                    size: 19,
                    color: const Color(0xD9FFFFFF),
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
    Widget key(Widget child, VoidCallback onTap, String tip) => _PadKey(
          tip: tip,
          onTap: () {
            _bump();
            onTap();
          },
          child: child,
        );
    Widget digit(String d) => key(
        Text(d,
            style: const TextStyle(
                fontSize: 22, fontWeight: FontWeight.w300, color: Colors.white)),
        () => _type(d),
        d);
    Widget row(List<Widget> keys) => Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            for (var i = 0; i < keys.length; i++) ...[
              if (i > 0) const SizedBox(width: 10),
              keys[i],
            ],
          ],
        );
    return [
      TweenAnimationBuilder<double>(
        key: ValueKey(_shake),
        tween: Tween(begin: 0, end: 1),
        duration: const Duration(milliseconds: 420),
        builder: (context, v, child) => Transform.translate(
          offset: Offset(
              _shake == 0 ? 0 : math.sin(v * math.pi * 6) * 10 * (1 - v), 0),
          child: child,
        ),
        child: dots,
      ),
      const SizedBox(height: 10),
      SizedBox(
        height: 20,
        child: Text(_msg,
            style: TextStyle(
                fontSize: 13.5,
                color: _msgInfo
                    ? const Color(0xD9FFFFFF)
                    : const Color(0xFFFCA5A5))),
      ),
      const SizedBox(height: 10),
      IgnorePointer(
        ignoring: _waiting,
        child: Opacity(
          opacity: _waiting ? .35 : 1,
          child: Column(children: [
            row([digit('1'), digit('2'), digit('3')]),
            const SizedBox(height: 10),
            row([digit('4'), digit('5'), digit('6')]),
            const SizedBox(height: 10),
            row([digit('7'), digit('8'), digit('9')]),
            const SizedBox(height: 10),
            row([
              key(const Icon(Icons.backspace_outlined,
                  size: 20, color: Colors.white), _backspace, 'Delete a digit'),
              digit('0'),
              key(const Icon(Icons.arrow_forward_rounded,
                  size: 22, color: Colors.white), _submit, 'Unlock'),
            ]),
          ]),
        ),
      ),
    ];
  }

  /// Whose library to open, when more than one person uses this computer
  /// account. Picking another restarts Tulipix as that profile — every path
  /// in the app is fixed when the process starts.
  List<Widget> _profilePicker() {
    final profiles = _cfg?.profiles ?? const [];
    if (profiles.length < 2) return const [];
    return [
      const SizedBox(height: 22),
      const Text('Whose library?',
          style: TextStyle(fontSize: 12.5, color: Color(0xBFFFFFFF))),
      const SizedBox(height: 8),
      Wrap(
        spacing: 8,
        runSpacing: 8,
        alignment: WrapAlignment.center,
        children: [
          for (final p in profiles)
            _Pill(
              label: p.name,
              icon: p.active ? Icons.person : Icons.person_outline,
              filled: p.active,
              onTap: () {
                if (!p.active) _switchProfile(p.slug);
              },
            ),
        ],
      ),
    ];
  }

  Future<void> _switchProfile(String slug) async {
    setState(() {
      _msgInfo = true;
      _msg = 'Opening that library — Tulipix is starting again…';
    });
    await lockSwitchProfile(slug: slug);
  }

  /// The fingerprint / security-key button, under whichever way in is drawn.
  /// Only when Rust says there is something enrolled to use.
  List<Widget> _passkey() {
    final label = _cfg?.passkeyLabel ?? '';
    if (!(_cfg?.passkey ?? false) || label.isEmpty) return const [];
    return [
      const SizedBox(height: 14),
      _Pill(
        label: _passkeyBusy ? 'Waiting…' : label,
        icon: Icons.fingerprint,
        // _Pill takes a callback, not a nullable one; _tryPasskey is
        // already a no-op while a read is outstanding.
        onTap: _tryPasskey,
      ),
    ];
  }

  Future<void> _tryPasskey() async {
    if (_passkeyBusy || _waiting) return;
    setState(() {
      _passkeyBusy = true;
      _msgInfo = true;
      _msg = 'Waiting for your fingerprint or key…';
    });
    final ok = await LockController.instance.tryPasskey();
    if (!mounted) return;
    if (ok) {
      setState(() => _passkeyBusy = false);
      _afterUnlock();
      return;
    }
    setState(() {
      _passkeyBusy = false;
      _shake++;
      _msgInfo = false;
      _msg = 'That didn\u2019t open it. Try again, or use the PIN.';
    });
  }

  List<Widget> _noPin() => [
        _Pill(
          label: 'Unlock',
          icon: Icons.arrow_forward_rounded,
          onTap: _unlock,
          trailingIcon: true,
        ),
        const SizedBox(height: 10),
        const Text('or press Enter',
            style: TextStyle(fontSize: 13, color: Color(0xBFFFFFFF))),
      ];

  Widget _miniPlayer() {
    final m = MusicController.instance;
    return ListenableBuilder(
      listenable: m,
      builder: (context, _) {
        final now = m.now;
        if (now == null) return const SizedBox.shrink();
        return Container(
          width: 360,
          padding: const EdgeInsets.fromLTRB(8, 8, 10, 8),
          decoration: _glass(16),
          child: Row(children: [
            ClipRRect(
              borderRadius: BorderRadius.circular(9),
              child: SizedBox(
                width: 40,
                height: 40,
                child: now.art.isEmpty
                    ? ColoredBox(color: m.accent)
                    : Image.file(File(now.art),
                        fit: BoxFit.cover,
                        cacheWidth: 120,
                        errorBuilder: (context, error, stack) =>
                            ColoredBox(color: m.accent)),
              ),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(now.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                          fontSize: 13,
                          fontWeight: FontWeight.w600,
                          color: Colors.white)),
                  Text(now.artist,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(
                          fontSize: 12, color: Color(0xBFFFFFFF))),
                ],
              ),
            ),
            _RoundBtn(
              icon: m.tickPlaying
                  ? Icons.pause_rounded
                  : Icons.play_arrow_rounded,
              tip: m.tickPlaying ? 'Pause' : 'Play',
              big: true,
              small: true,
              onTap: () {
                _bump();
                m.send(const MusicCmd.playPause());
              },
            ),
            const SizedBox(width: 8),
            _RoundBtn(
              icon: Icons.skip_next_rounded,
              tip: 'Next',
              small: true,
              onTap: () {
                _bump();
                m.send(const MusicCmd.next());
              },
            ),
          ]),
        );
      },
    );
  }
}

// ── pieces ──────────────────────────────────────────────────────────────────

/// Dark and see-through, and deliberately not a BackdropFilter: a blur behind
/// every card would re-run on every frame of the smoke.
BoxDecoration _glass(double radius) => BoxDecoration(
      color: const Color(0x73101016),
      borderRadius: BorderRadius.circular(radius),
      border: Border.all(color: const Color(0x21FFFFFF)),
    );

Widget _eyebrow(String text) => Text(
      text.toUpperCase(),
      style: const TextStyle(
        fontSize: 11.5,
        fontWeight: FontWeight.w600,
        letterSpacing: 1.1,
        color: Color(0xBFFFFFFF),
      ),
    );

class _Vignette extends StatelessWidget {
  const _Vignette();

  @override
  Widget build(BuildContext context) => const Stack(
        fit: StackFit.expand,
        children: [
          DecoratedBox(
            decoration: BoxDecoration(
              gradient: RadialGradient(
                center: Alignment(0, -.15),
                radius: 1.1,
                colors: [Color(0x00000000), Color(0x8C000000)],
                stops: [.55, 1],
              ),
            ),
          ),
          DecoratedBox(
            decoration: BoxDecoration(
              gradient: LinearGradient(
                begin: Alignment.topCenter,
                end: Alignment.bottomCenter,
                colors: [Color(0x00000000), Color(0x73000000)],
                stops: [.6, 1],
              ),
            ),
          ),
        ],
      );
}

/// The time and the date. Rebuilds once a minute, on the minute.
class _LockClock extends StatefulWidget {
  const _LockClock({required this.size, this.center = false});

  final double size;
  final bool center;

  @override
  State<_LockClock> createState() => _LockClockState();
}

class _LockClockState extends State<_LockClock> {
  DateTime _now = DateTime.now();
  Timer? _t;

  static const _days = [
    'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'
  ];
  static const _months = [
    'January', 'February', 'March', 'April', 'May', 'June', 'July', 'August',
    'September', 'October', 'November', 'December'
  ];

  @override
  void initState() {
    super.initState();
    _schedule();
  }

  void _schedule() {
    final now = DateTime.now();
    _t = Timer(
      Duration(seconds: 60 - now.second, milliseconds: 20 - now.millisecond),
      () {
        if (!mounted) return;
        setState(() => _now = DateTime.now());
        _schedule();
      },
    );
  }

  @override
  void dispose() {
    _t?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final s = widget.size;
    final h24 = MediaQuery.alwaysUse24HourFormatOf(context);
    final h12 = _now.hour % 12 == 0 ? 12 : _now.hour % 12;
    final hm = '${h24 ? _now.hour.toString().padLeft(2, '0') : h12}:'
        '${_now.minute.toString().padLeft(2, '0')}';
    final date =
        '${_days[_now.weekday - 1]}, ${_now.day} ${_months[_now.month - 1]}';
    const shadow = [Shadow(color: Color(0x59000000), blurRadius: 30)];
    return Column(
      crossAxisAlignment:
          widget.center ? CrossAxisAlignment.center : CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Row(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.baseline,
          textBaseline: TextBaseline.alphabetic,
          children: [
            Text(hm,
                style: TextStyle(
                    fontSize: s,
                    fontWeight: FontWeight.w200,
                    height: 1,
                    letterSpacing: -s * .03,
                    color: Colors.white,
                    fontFeatures: const [FontFeature.tabularFigures()],
                    shadows: shadow)),
            if (!h24) ...[
              SizedBox(width: s * .08),
              Text(_now.hour < 12 ? 'AM' : 'PM',
                  style: TextStyle(
                      fontSize: s * .24,
                      color: const Color(0xCCFFFFFF),
                      shadows: shadow)),
            ],
          ],
        ),
        SizedBox(height: s * .14),
        Text(date,
            style: TextStyle(
                fontSize: widget.center ? s * .17 : s * .22,
                color: const Color(0xEBFFFFFF),
                shadows: shadow)),
      ],
    );
  }
}

class _RoundBtn extends StatelessWidget {
  const _RoundBtn({
    required this.icon,
    required this.tip,
    required this.onTap,
    this.big = false,
    this.small = false,
    this.tint,
  });

  final IconData icon;
  final String tip;
  final VoidCallback onTap;
  final bool big;
  final bool small;
  final Color? tint;

  @override
  Widget build(BuildContext context) {
    final size = small ? 38.0 : (big ? 64.0 : 48.0);
    return Tooltip(
      message: tip,
      child: Material(
        color: big ? Colors.white : const Color(0x14FFFFFF),
        shape: CircleBorder(
            side: BorderSide(
                color: big ? Colors.white : const Color(0x29FFFFFF))),
        child: InkWell(
          customBorder: const CircleBorder(),
          onTap: onTap,
          child: SizedBox(
            width: size,
            height: size,
            child: Icon(icon,
                size: size * .46,
                color: big ? const Color(0xFF111111) : (tint ?? Colors.white)),
          ),
        ),
      ),
    );
  }
}

class _Pill extends StatelessWidget {
  const _Pill({
    required this.label,
    required this.icon,
    required this.onTap,
    this.filled = false,
    this.trailingIcon = false,
  });

  final String label;
  final IconData icon;
  final VoidCallback onTap;
  final bool filled;
  final bool trailingIcon;

  @override
  Widget build(BuildContext context) {
    final fg = filled ? const Color(0xFF111111) : Colors.white;
    final ic = Icon(icon, size: 20, color: fg);
    return Material(
      color: filled ? Colors.white : const Color(0x24FFFFFF),
      shape: StadiumBorder(
          side: BorderSide(
              color: filled ? Colors.white : const Color(0x47FFFFFF))),
      child: InkWell(
        customBorder: const StadiumBorder(),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 22, vertical: 13),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (!trailingIcon) ...[ic, const SizedBox(width: 8)],
              Text(label,
                  style: TextStyle(
                      fontSize: 15, fontWeight: FontWeight.w600, color: fg)),
              if (trailingIcon) ...[const SizedBox(width: 10), ic],
            ],
          ),
        ),
      ),
    );
  }
}

class _PadKey extends StatelessWidget {
  const _PadKey({required this.child, required this.onTap, required this.tip});

  final Widget child;
  final VoidCallback onTap;
  final String tip;

  @override
  Widget build(BuildContext context) => Semantics(
        button: true,
        label: tip,
        child: Material(
          color: const Color(0x14FFFFFF),
          shape: const CircleBorder(side: BorderSide(color: Color(0x24FFFFFF))),
          child: InkWell(
            customBorder: const CircleBorder(),
            onTap: onTap,
            child: SizedBox(width: 58, height: 58, child: Center(child: child)),
          ),
        ),
      );
}

class _GlanceCard extends StatelessWidget {
  const _GlanceCard({
    required this.icon,
    required this.label,
    required this.title,
    required this.sub,
    this.frac = -1,
  });

  final IconData icon;
  final String label;
  final String title;
  final String sub;
  final double frac;

  @override
  Widget build(BuildContext context) => Container(
        padding: const EdgeInsets.fromLTRB(16, 14, 16, 14),
        decoration: _glass(18),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisSize: MainAxisSize.min,
          children: [
            Row(children: [
              Icon(icon, size: 14, color: const Color(0xC7FFFFFF)),
              const SizedBox(width: 8),
              _eyebrow(label),
            ]),
            const SizedBox(height: 8),
            Text(title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                    fontSize: 16,
                    fontWeight: FontWeight.w600,
                    color: Colors.white)),
            const SizedBox(height: 2),
            Text(sub,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(fontSize: 13, color: Color(0xC7FFFFFF))),
            if (frac >= 0) ...[
              const SizedBox(height: 10),
              Container(
                height: 4,
                decoration: BoxDecoration(
                  color: const Color(0x33FFFFFF),
                  borderRadius: BorderRadius.circular(2),
                ),
                alignment: Alignment.centerLeft,
                child: FractionallySizedBox(
                  widthFactor: frac.clamp(0.0, 1.0),
                  child: Container(
                    decoration: BoxDecoration(
                      color: Colors.white,
                      borderRadius: BorderRadius.circular(2),
                    ),
                  ),
                ),
              ),
            ],
          ],
        ),
      );
}

class _Avatar extends StatelessWidget {
  const _Avatar({required this.size, required this.path, required this.emoji});

  final double size;
  final String path;
  final String emoji;

  @override
  Widget build(BuildContext context) {
    final fallback = Container(
      width: size,
      height: size,
      alignment: Alignment.center,
      decoration: const BoxDecoration(
        shape: BoxShape.circle,
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [Color(0xFF312E81), Tokens.brand],
        ),
      ),
      child: emoji.trim().isEmpty
          ? Icon(Icons.person, size: size * .42, color: Colors.white)
          : Text(emoji, style: TextStyle(fontSize: size * .5)),
    );
    return Container(
      decoration: const BoxDecoration(
        shape: BoxShape.circle,
        boxShadow: [
          BoxShadow(color: Color(0x33FFFFFF), spreadRadius: 3),
          BoxShadow(
              color: Color(0x73000000), blurRadius: 60, offset: Offset(0, 20)),
        ],
      ),
      child: path.isEmpty
          ? fallback
          : ClipOval(
              child: Image.file(
                File(path),
                key: ValueKey(ShellController.instance.pictureEpoch),
                width: size,
                height: size,
                fit: BoxFit.cover,
                cacheWidth: (size * 2).round(),
                errorBuilder: (context, error, stack) => fallback,
              ),
            ),
    );
  }
}

class _Locked extends StatelessWidget {
  const _Locked();

  @override
  Widget build(BuildContext context) => const Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.lock_outline, size: 15, color: Color(0xD9FFFFFF)),
          SizedBox(width: 8),
          Text('Locked',
              style: TextStyle(fontSize: 13, color: Color(0xD9FFFFFF))),
        ],
      );
}

class _Health extends StatelessWidget {
  const _Health();

  @override
  Widget build(BuildContext context) {
    final shell = ShellController.instance;
    final tint = switch (shell.statusLevel) {
      'ok' => const Color(0xFF34D399),
      'warn' => Tokens.warn,
      'error' || 'bad' || 'problem' => Tokens.error,
      _ => const Color(0xB3FFFFFF),
    };
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Container(
          width: 8,
          height: 8,
          decoration: BoxDecoration(color: tint, shape: BoxShape.circle),
        ),
        const SizedBox(width: 8),
        ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 280),
          child: Text(shell.statusNote,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(fontSize: 13, color: Colors.white)),
        ),
      ],
    );
  }
}

/// The mark and the name, as the sidebar draws them, sized to be read from
/// across the room.
class _LockBrand extends StatelessWidget {
  const _LockBrand({required this.size});

  final double size;

  @override
  Widget build(BuildContext context) {
    const shadow = [Shadow(color: Color(0x59000000), blurRadius: 30)];
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        DecoratedBox(
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(size * .25),
            boxShadow: const [
              BoxShadow(
                  color: Color(0x59000000),
                  blurRadius: 30,
                  offset: Offset(0, 10)),
            ],
          ),
          child: AppMark(
            size: size,
            radius: size * .25,
            choice: ShellController.instance.state?.logoChoice ?? 0,
          ),
        ),
        SizedBox(width: size * .3),
        Text('Tulipix',
            style: TextStyle(
                fontSize: size * .5,
                fontWeight: FontWeight.w700,
                letterSpacing: -.1,
                color: Colors.white,
                shadows: shadow)),
      ],
    );
  }
}

/// The seek bar, as the players have it: click or drag, and the jump happens
/// on release — seeking on every drag frame makes mpv stutter. The clock
/// follows the pointer while it moves.
class _LockSeek extends StatefulWidget {
  const _LockSeek({
    required this.pos,
    required this.dur,
    required this.tint,
    required this.k,
    required this.onSeek,
  });

  final double pos;
  final double dur;
  final Color tint;
  final double k;

  /// Null: shown, not draggable.
  final ValueChanged<double>? onSeek;

  @override
  State<_LockSeek> createState() => _LockSeekState();
}

class _LockSeekState extends State<_LockSeek> {
  /// Where the pointer has the playhead, 0..1, while dragging — and after
  /// release, until the player reports the new position, so the bar does not
  /// jump back for the second in between.
  double? _drag;
  bool _sent = false;
  bool _hover = false;

  @override
  void didUpdateWidget(_LockSeek old) {
    super.didUpdateWidget(old);
    if (_sent && widget.pos != old.pos) {
      _sent = false;
      _drag = null;
    }
  }

  void _commit(double f) {
    widget.onSeek?.call(f * widget.dur);
    setState(() {
      _drag = f;
      _sent = true;
    });
  }

  @override
  Widget build(BuildContext context) {
    final k = widget.k;
    final dur = widget.dur;
    final live = widget.onSeek != null && dur > 0;
    final frac =
        _drag ?? (dur > 0 ? (widget.pos / dur).clamp(0.0, 1.0) : 0.0);
    final style = TextStyle(
        fontSize: 13 * k,
        color: Colors.white,
        fontFeatures: const [FontFeature.tabularFigures()]);
    final big = live && (_hover || (_drag != null && !_sent));
    return Row(children: [
      Text(_LockScreenState._fmt(_drag == null ? widget.pos : _drag! * dur),
          style: style),
      const SizedBox(width: 14),
      Expanded(
        child: LayoutBuilder(
          builder: (context, box) {
            final w = box.maxWidth;
            double at(double dx) => (dx / w).clamp(0.0, 1.0);
            final bar = big ? 7.0 : 5.0;
            return MouseRegion(
              cursor: live ? SystemMouseCursors.click : MouseCursor.defer,
              onEnter: (_) => setState(() => _hover = true),
              onExit: (_) => setState(() => _hover = false),
              child: GestureDetector(
                behavior: HitTestBehavior.opaque,
                onTapUp: live ? (d) => _commit(at(d.localPosition.dx)) : null,
                onHorizontalDragStart: live
                    ? (d) => setState(() {
                          _sent = false;
                          _drag = at(d.localPosition.dx);
                        })
                    : null,
                onHorizontalDragUpdate: live
                    ? (d) => setState(() => _drag = at(d.localPosition.dx))
                    : null,
                onHorizontalDragEnd: live
                    ? (_) {
                        final f = _drag;
                        if (f != null) _commit(f);
                      }
                    : null,
                onHorizontalDragCancel: () => setState(() => _drag = null),
                child: SizedBox(
                  height: 24,
                  child: Stack(
                    clipBehavior: Clip.none,
                    alignment: Alignment.centerLeft,
                    children: [
                      Container(
                        height: bar,
                        decoration: BoxDecoration(
                          color: const Color(0x33FFFFFF),
                          borderRadius: BorderRadius.circular(bar / 2),
                        ),
                      ),
                      Container(
                        width: w * frac,
                        height: bar,
                        decoration: BoxDecoration(
                          color: widget.tint,
                          borderRadius: BorderRadius.circular(bar / 2),
                        ),
                      ),
                      if (big)
                        Positioned(
                          left: w * frac - 7,
                          child: Container(
                            width: 14,
                            height: 14,
                            decoration: const BoxDecoration(
                              color: Colors.white,
                              shape: BoxShape.circle,
                              boxShadow: [
                                BoxShadow(
                                    color: Color(0x66000000), blurRadius: 6),
                              ],
                            ),
                          ),
                        ),
                    ],
                  ),
                ),
              ),
            );
          },
        ),
      ),
      const SizedBox(width: 14),
      Text(_LockScreenState._fmt(dur), style: style),
    ]);
  }
}

/// Mute, and the volume on the same kind of bar. It moves the deck at once;
/// the controller tells Rust once the pointer rests. 130 fills it, as on the
/// players — mpv's softvol goes past unity.
class _LockVolume extends StatelessWidget {
  const _LockVolume({required this.barWidth});

  final double barWidth;

  @override
  Widget build(BuildContext context) {
    final m = MusicController.instance;
    return ListenableBuilder(
      listenable: Listenable.merge([audioLevel, m]),
      builder: (context, _) {
        final v = m.volume;
        final muted = m.muted;
        final frac = muted ? 0.0 : (v.clamp(0, 130) / 130).toDouble();
        void to(double dx) =>
            m.setVolume((dx / barWidth).clamp(0.0, 1.0) * 130);
        return Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            _RoundBtn(
              icon: muted
                  ? Icons.volume_off_rounded
                  : v > 66
                      ? Icons.volume_up_rounded
                      : v > 0
                          ? Icons.volume_down_rounded
                          : Icons.volume_mute_rounded,
              tip: muted ? 'Unmute' : 'Mute',
              onTap: () => m.send(const MusicCmd.toggleMute()),
            ),
            const SizedBox(width: 10),
            MouseRegion(
              cursor: SystemMouseCursors.click,
              child: GestureDetector(
                behavior: HitTestBehavior.opaque,
                onTapDown: (d) => to(d.localPosition.dx),
                onHorizontalDragStart: (d) => to(d.localPosition.dx),
                onHorizontalDragUpdate: (d) => to(d.localPosition.dx),
                child: SizedBox(
                  width: barWidth,
                  height: 24,
                  child: Stack(
                    alignment: Alignment.centerLeft,
                    children: [
                      Container(
                        height: 5,
                        decoration: BoxDecoration(
                          color: const Color(0x33FFFFFF),
                          borderRadius: BorderRadius.circular(3),
                        ),
                      ),
                      Container(
                        width: barWidth * frac,
                        height: 5,
                        decoration: BoxDecoration(
                          color: Colors.white,
                          borderRadius: BorderRadius.circular(3),
                        ),
                      ),
                    ],
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
