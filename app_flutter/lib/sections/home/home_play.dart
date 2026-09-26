// Home · Play — for the Play preset (Videos, Music, Arcade, Books).
//
// docs/mockups/NewSections/home-media-play-deck.html. A couch dashboard: one
// row of big tiles — the film to resume, the game to go back to, what is
// playing, the book you are in — moved along with the arrow keys, the backdrop
// taking the picture of whichever is focused. Rails of games, films and songs
// sit under it. Always dark: it is a screen for the evening.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/arcade.dart';
import '../../src/rust/api/home.dart';
import '../../src/rust/api/music.dart';
import '../music/music_controller.dart';
import '../music/music_widgets.dart';
import 'home_controller.dart';
import 'home_modern.dart';
import 'home_shared.dart';

/// One tile on the focus row.
class _Pick {
  const _Pick({
    required this.kind,
    required this.label,
    required this.title,
    required this.blurb,
    required this.color,
    required this.icon,
    required this.action,
    required this.art,
    required this.go,
    this.frac = -1,
  });

  /// "FILM · RESUME", shown over the title.
  final String kind;

  /// The short word on the tile itself.
  final String label;
  final String title;
  final String blurb;
  final Color color;
  final IconData icon;
  final String action;
  final Widget art;
  final VoidCallback go;
  final double frac;
}

class PlayHome extends StatefulWidget {
  const PlayHome({super.key, required this.controller, required this.state});

  final HomeController controller;
  final HomeState state;

  @override
  State<PlayHome> createState() => _PlayHomeState();
}

class _PlayHomeState extends State<PlayHome> {
  List<GameTile> _games = const [];
  int _sel = 0;
  final FocusNode _focus = FocusNode(debugLabel: 'play-home');
  final ScrollController _row = ScrollController();

  @override
  void initState() {
    super.initState();
    _loadGames();
    MusicController.instance.addListener(_music);
  }

  @override
  void didUpdateWidget(PlayHome old) {
    super.didUpdateWidget(old);
    // Home refreshes on every visit; the games may have moved too.
    if (!identical(old.state, widget.state)) _loadGames();
  }

  @override
  void dispose() {
    MusicController.instance.removeListener(_music);
    _focus.dispose();
    _row.dispose();
    super.dispose();
  }

  String _track = '';

  /// Only a track starting, stopping or changing moves the row; a position
  /// tick does not, and rebuilding the whole page on every tick is what that
  /// would cost.
  void _music() {
    final now = MusicController.instance.now;
    final track = now == null || !now.loaded
        ? ''
        : '${now.itemId}/${now.title}/${now.streamTitle}';
    if (track != _track && mounted) setState(() => _track = track);
  }

  Future<void> _loadGames() async {
    if (!sectionOn(Section.arcade)) {
      if (_games.isNotEmpty) setState(() => _games = const []);
      return;
    }
    try {
      final g = await arcadeRecent(limit: 12);
      if (mounted) setState(() => _games = g);
    } catch (_) {
      // No games is an empty rail, not a banner over a landing page.
    }
  }

  List<_Pick> _picks() {
    final st = widget.state;
    final rows = cardOn(st, 'continue') ? st.continueRows : <HomeContinue>[];
    final out = <_Pick>[];
    HomeContinue? first(String kind) =>
        rows.where((r) => r.kind == kind).firstOrNull;

    _Pick row(HomeContinue r, String kind, String action) => _Pick(
          kind: '$kind · ${r.frac >= 0 ? 'Resume' : 'Continue'}',
          label: kind,
          title: r.title,
          blurb: [r.author, r.sub].where((s) => s.isNotEmpty).join(' · '),
          color: kindColor(r.kind),
          icon: kindIcon(r.kind),
          action: action,
          art: RowCover(row: r, iconSize: 40),
          go: () => resumeRow(r),
          frac: r.frac,
        );

    final video = first('video');
    if (video != null) out.add(row(video, 'Film', 'Resume'));

    if (_games.isNotEmpty) {
      final g = _games.first;
      out.add(_Pick(
        kind: 'Game · Continue',
        label: 'Game',
        title: g.title,
        blurb: [
          if (g.last.isNotEmpty) 'Last played ${g.last.toLowerCase()}',
          if (g.played.isNotEmpty) '${g.played} in total',
        ].join(' · '),
        color: Tokens.secArcade,
        icon: Icons.sports_esports_outlined,
        action: 'Play',
        art: GameCover(game: g, big: true),
        go: () => playGame(g),
      ));
    }

    final now = MusicController.instance.now;
    if (cardOn(st, 'player') && now != null && now.loaded) {
      out.add(_Pick(
        kind: 'Music · Now playing',
        label: 'Music',
        title: now.streamTitle.isNotEmpty ? now.streamTitle : now.title,
        blurb: [now.artist, now.album].where((s) => s.isNotEmpty).join(' · '),
        color: Tokens.secMusic,
        icon: Icons.music_note,
        action: 'Play / pause',
        art: MusicArt(
          controller: MusicController.instance,
          kind: 'track',
          artKey: '${now.itemId}',
          direct: now.art,
          size: 260,
          radius: 0,
        ),
        go: () => MusicController.instance.send(const MusicCmd.playPause()),
      ));
    }

    final book = first('book');
    if (book != null) out.add(row(book, 'Book', 'Read'));
    for (final r in rows) {
      if (r == video || r == book) continue;
      if (out.length >= 7) break;
      out.add(row(
          r,
          switch (r.kind) {
            'video' => 'Film',
            'book' => 'Book',
            'podcast' => 'Podcast',
            _ => 'Audiobook',
          },
          'Resume'));
    }
    for (final g in _games.skip(1)) {
      if (out.length >= 8) break;
      out.add(_Pick(
        kind: 'Game · Arcade',
        label: 'Game',
        title: g.title,
        blurb: g.played.isEmpty ? g.system : '${g.played} played',
        color: Tokens.secArcade,
        icon: Icons.sports_esports_outlined,
        action: 'Play',
        art: GameCover(game: g, big: true),
        go: () => playGame(g),
      ));
    }
    return out;
  }

  KeyEventResult _key(FocusNode _, KeyEvent e, int n) {
    if (e is! KeyDownEvent && e is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    if (n == 0) return KeyEventResult.ignored;
    final k = e.logicalKey;
    if (k == LogicalKeyboardKey.arrowRight ||
        k == LogicalKeyboardKey.arrowLeft) {
      final d = k == LogicalKeyboardKey.arrowRight ? 1 : -1;
      setState(() => _sel = (_sel + d + n) % n);
      _scrollTo(_sel);
      return KeyEventResult.handled;
    }
    if (k == LogicalKeyboardKey.enter || k == LogicalKeyboardKey.space) {
      _picks()[_sel.clamp(0, n - 1)].go();
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  void _scrollTo(int i) {
    if (!_row.hasClients) return;
    const tile = 190.0 + 16;
    final target = (i * tile - 40).clamp(0.0, _row.position.maxScrollExtent);
    _row.animateTo(target,
        duration: const Duration(milliseconds: 220), curve: Curves.easeOut);
  }

  @override
  Widget build(BuildContext context) {
    final st = widget.state;
    final picks = _picks();
    final sel = picks.isEmpty ? 0 : _sel.clamp(0, picks.length - 1);
    final p = picks.isEmpty ? null : picks[sel];
    final name = firstName(st);
    final motion = !context.tokens.reduceMotion;

    return ColoredBox(
      color: const Color(0xFF06070F),
      child: Focus(
        focusNode: _focus,
        autofocus: true,
        onKeyEvent: (n, e) => _key(n, e, picks.length),
        child: Stack(
          children: [
            // The focused tile's picture, washed out behind everything.
            Positioned.fill(
              child: AnimatedSwitcher(
                duration: Duration(milliseconds: motion ? 450 : 0),
                child: p == null
                    ? const SizedBox.shrink()
                    : Opacity(
                        key: ValueKey('${p.kind}/${p.title}'),
                        opacity: 0.45,
                        child: SizedBox.expand(child: p.art),
                      ),
              ),
            ),
            Positioned.fill(
              child: AnimatedContainer(
                duration: Duration(milliseconds: motion ? 450 : 0),
                decoration: BoxDecoration(
                  gradient: RadialGradient(
                    center: const Alignment(0.7, -0.6),
                    radius: 1.2,
                    colors: [
                      (p?.color ?? Tokens.secVideos).withValues(alpha: 0.35),
                      const Color(0x0006070F),
                    ],
                  ),
                ),
              ),
            ),
            const Positioned.fill(
              child: DecoratedBox(
                decoration: BoxDecoration(
                  gradient: LinearGradient(
                    colors: [
                      Color(0xF006070F),
                      Color(0x8C06070F),
                      Color(0x2606070F)
                    ],
                    stops: [0, 0.45, 1],
                  ),
                ),
              ),
            ),
            const Positioned.fill(
              child: DecoratedBox(
                decoration: BoxDecoration(
                  gradient: LinearGradient(
                    begin: Alignment.bottomCenter,
                    end: Alignment.topCenter,
                    colors: [Color(0xFF06070F), Color(0x0006070F)],
                    stops: [0, 0.5],
                  ),
                ),
              ),
            ),
            SingleChildScrollView(
              padding: const EdgeInsets.fromLTRB(34, 22, 34, 30),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _TopBar(name: name),
                  const SizedBox(height: 48),
                  _Info(pick: p),
                  const SizedBox(height: 22),
                  SizedBox(
                    height: 180,
                    child: ListView.separated(
                      controller: _row,
                      scrollDirection: Axis.horizontal,
                      padding: const EdgeInsets.symmetric(
                          horizontal: 4, vertical: 10),
                      itemCount: picks.length,
                      separatorBuilder: (_, __) => const SizedBox(width: 16),
                      itemBuilder: (_, i) => _FocusTile(
                        pick: picks[i],
                        on: i == sel,
                        motion: motion,
                        onHover: () {
                          if (_sel != i) setState(() => _sel = i);
                        },
                        onTap: () {
                          if (_sel == i) {
                            picks[i].go();
                          } else {
                            setState(() => _sel = i);
                          }
                          _focus.requestFocus();
                        },
                      ),
                    ),
                  ),
                  if (_games.isNotEmpty) ...[
                    const SizedBox(height: 18),
                    _RailHead('Your games', Tokens.secArcade,
                        onTap: () =>
                            ShellController.instance.go(Section.arcade)),
                    ModernRail(
                      height: 200,
                      children: [
                        for (final g in _games)
                          _Shelf(
                            width: 120,
                            height: 160,
                            title: g.title,
                            sub: [g.system, g.played]
                                .where((s) => s.isNotEmpty)
                                .join(' · '),
                            art: GameCover(game: g),
                            onTap: () => playGame(g),
                          ),
                      ],
                    ),
                  ],
                  if (sectionOn(Section.videos) &&
                      st.recentVideos.isNotEmpty) ...[
                    const SizedBox(height: 18),
                    _RailHead('New to watch', Tokens.secVideos,
                        onTap: () =>
                            ShellController.instance.go(Section.videos)),
                    ModernRail(
                      height: 222,
                      children: [
                        for (final v in st.recentVideos)
                          _Shelf(
                            width: 124,
                            height: 186,
                            title: v.label,
                            sub: v.sub,
                            art: LazyCover(
                              section: Section.videos,
                              id: v.id,
                              tint: Tokens.secVideos,
                              icon: Icons.movie_outlined,
                            ),
                            onTap: () => openVideo(v.id),
                          ),
                      ],
                    ),
                  ],
                  if (sectionOn(Section.music) &&
                      st.recentSongs.isNotEmpty) ...[
                    const SizedBox(height: 18),
                    _RailHead('Recent songs', Tokens.secMusic,
                        onTap: () =>
                            ShellController.instance.go(Section.music)),
                    ModernRail(
                      height: 150,
                      children: [
                        for (final s in st.recentSongs)
                          _Shelf(
                            width: 112,
                            height: 112,
                            round: true,
                            title: s.label,
                            sub: '',
                            art: LazyCover(
                              section: Section.music,
                              id: s.id,
                              tint: Tokens.secMusic,
                              icon: Icons.music_note,
                            ),
                            onTap: () =>
                                ShellController.instance.go(Section.music),
                          ),
                      ],
                    ),
                  ],
                  const SizedBox(height: 24),
                  const _Hints(),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ── the top bar ─────────────────────────────────────────────────────────────

class _TopBar extends StatelessWidget {
  const _TopBar({required this.name});

  final String name;

  @override
  Widget build(BuildContext context) {
    final part = dayPartName(dayPart());
    final doors = [
      if (sectionOn(Section.videos)) ('Watch', Section.videos),
      if (sectionOn(Section.arcade)) ('Play', Section.arcade),
      if (sectionOn(Section.music)) ('Listen', Section.music),
      if (sectionOn(Section.books)) ('Read', Section.books),
    ];
    return Wrap(
      spacing: 6,
      runSpacing: 6,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        Padding(
          padding: const EdgeInsets.only(right: 8),
          child: Text(name.isEmpty ? part : '$part, $name',
              style: const TextStyle(
                  fontSize: 14,
                  fontWeight: FontWeight.w700,
                  color: Colors.white)),
        ),
        for (final (label, s) in doors)
          Tap(
            onTap: () => ShellController.instance.go(s),
            child: Container(
              height: 32,
              padding: const EdgeInsets.symmetric(horizontal: 14),
              decoration: BoxDecoration(
                color: Colors.white.withValues(alpha: 0.08),
                borderRadius: BorderRadius.circular(999),
              ),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Container(
                    width: 8,
                    height: 8,
                    decoration: BoxDecoration(
                        color: Tokens.accentOf(s), shape: BoxShape.circle),
                  ),
                  const SizedBox(width: 7),
                  Text(label,
                      style: const TextStyle(
                          fontSize: 12.5,
                          fontWeight: FontWeight.w700,
                          color: Color(0xCCFFFFFF))),
                ],
              ),
            ),
          ),
      ],
    );
  }
}

// ── the focused tile, spelled out ───────────────────────────────────────────

class _Info extends StatelessWidget {
  const _Info({required this.pick});

  final _Pick? pick;

  @override
  Widget build(BuildContext context) {
    final p = pick;
    if (p == null) {
      return const SizedBox(
        height: 200,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          mainAxisAlignment: MainAxisAlignment.end,
          children: [
            Text('Nothing to go back to yet',
                style: TextStyle(
                    fontSize: 40,
                    height: 1.05,
                    fontWeight: FontWeight.w800,
                    color: Colors.white)),
            SizedBox(height: 10),
            Text(
                'Start a film, a game or a book and it will wait for you here.',
                style: TextStyle(fontSize: 13.5, color: Color(0xB8FFFFFF))),
          ],
        ),
      );
    }
    return ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: 560, minHeight: 200),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(p.kind.toUpperCase(),
              style: TextStyle(
                  fontSize: 11,
                  fontWeight: FontWeight.w800,
                  letterSpacing: 1.6,
                  color: brighter(p.color, 0.25))),
          const SizedBox(height: 10),
          Text(p.title,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(
                  fontSize: 46,
                  height: 1.02,
                  fontWeight: FontWeight.w800,
                  letterSpacing: -1.2,
                  color: Colors.white)),
          if (p.blurb.isNotEmpty) ...[
            const SizedBox(height: 8),
            Text(p.blurb,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style:
                    const TextStyle(fontSize: 13.5, color: Color(0xB8FFFFFF))),
          ],
          if (p.frac >= 0) ...[
            const SizedBox(height: 16),
            SizedBox(
              width: 280,
              child: ThinBar(
                  frac: p.frac,
                  color: brighter(p.color, 0.2),
                  track: const Color(0x33FFFFFF)),
            ),
          ],
          const SizedBox(height: 18),
          PillBtn(label: p.action, icon: Icons.play_arrow_rounded, onTap: p.go),
        ],
      ),
    );
  }
}

class _FocusTile extends StatelessWidget {
  const _FocusTile({
    required this.pick,
    required this.on,
    required this.motion,
    required this.onHover,
    required this.onTap,
  });

  final _Pick pick;
  final bool on;
  final bool motion;
  final VoidCallback onHover;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Align(
      alignment: Alignment.bottomCenter,
      child: MouseRegion(
        onEnter: (_) => onHover(),
        cursor: SystemMouseCursors.click,
        child: GestureDetector(
          onTap: onTap,
          child: AnimatedContainer(
            duration: Duration(milliseconds: motion ? 220 : 0),
            curve: Curves.easeOut,
            width: on ? 250 : 190,
            height: on ? 156 : 120,
            clipBehavior: Clip.antiAlias,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(16),
              border: Border.all(
                  color: on ? Colors.white : Colors.transparent, width: 3),
              boxShadow: const [
                BoxShadow(
                    color: Color(0xE6000000),
                    blurRadius: 26,
                    offset: Offset(0, 10),
                    spreadRadius: -14),
              ],
            ),
            child: Stack(
              fit: StackFit.expand,
              children: [
                pick.art,
                const DecoratedBox(
                  decoration: BoxDecoration(
                    gradient: LinearGradient(
                      begin: Alignment.bottomCenter,
                      end: Alignment.topCenter,
                      colors: [Color(0xB3000000), Color(0x00000000)],
                      stops: [0, 0.6],
                    ),
                  ),
                ),
                Positioned(
                  top: 10,
                  left: 10,
                  child: Container(
                    width: 26,
                    height: 26,
                    decoration: BoxDecoration(
                      color: const Color(0x73000000),
                      borderRadius: BorderRadius.circular(8),
                    ),
                    child: Icon(pick.icon, size: 14, color: Colors.white),
                  ),
                ),
                Positioned(
                  left: 12,
                  right: 12,
                  bottom: 10,
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(pick.label.toUpperCase(),
                          style: const TextStyle(
                              fontSize: 10,
                              fontWeight: FontWeight.w800,
                              letterSpacing: 1.2,
                              color: Color(0xCCFFFFFF))),
                      Text(pick.title,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(
                              fontSize: 12.5,
                              fontWeight: FontWeight.w700,
                              color: Colors.white)),
                    ],
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

// ── the rails ───────────────────────────────────────────────────────────────

class _RailHead extends StatelessWidget {
  const _RailHead(this.title, this.color, {required this.onTap});

  final String title;
  final Color color;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.only(bottom: 10),
        child: Tap(
          onTap: onTap,
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Container(
                width: 8,
                height: 8,
                decoration: BoxDecoration(color: color, shape: BoxShape.circle),
              ),
              const SizedBox(width: 8),
              Text(title,
                  style: const TextStyle(
                      fontSize: 14,
                      fontWeight: FontWeight.w700,
                      color: Color(0xE6FFFFFF))),
              const SizedBox(width: 4),
              const Icon(Icons.chevron_right,
                  size: 16, color: Color(0x80FFFFFF)),
            ],
          ),
        ),
      );
}

class _Shelf extends StatelessWidget {
  const _Shelf({
    required this.width,
    required this.height,
    required this.title,
    required this.sub,
    required this.art,
    required this.onTap,
    this.round = false,
  });

  final double width;
  final double height;
  final String title;
  final String sub;
  final Widget art;
  final VoidCallback onTap;
  final bool round;

  @override
  Widget build(BuildContext context) => SizedBox(
        width: width,
        child: Tap(
          onTap: onTap,
          child: Column(
            crossAxisAlignment:
                round ? CrossAxisAlignment.center : CrossAxisAlignment.start,
            children: [
              ClipRRect(
                borderRadius: BorderRadius.circular(round ? width : 12),
                child: SizedBox(width: width, height: height, child: art),
              ),
              const SizedBox(height: 6),
              Text(title,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                      fontSize: 11.5, color: Color(0xBFFFFFFF))),
              if (sub.isNotEmpty)
                Text(sub,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: const TextStyle(
                        fontSize: 10, color: Color(0x80FFFFFF))),
            ],
          ),
        ),
      );
}

class _Hints extends StatelessWidget {
  const _Hints();

  @override
  Widget build(BuildContext context) {
    Widget hint(String key, String what) => Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 4),
              decoration: BoxDecoration(
                color: const Color(0x1AFFFFFF),
                borderRadius: BorderRadius.circular(6),
              ),
              child: Text(key,
                  style: const TextStyle(
                      fontSize: 10.5,
                      fontWeight: FontWeight.w700,
                      color: Colors.white)),
            ),
            const SizedBox(width: 6),
            Text(what,
                style:
                    const TextStyle(fontSize: 11.5, color: Color(0x8CFFFFFF))),
          ],
        );
    return Wrap(
      spacing: 16,
      runSpacing: 8,
      children: [
        hint('← →', 'move'),
        hint('Enter', 'play'),
        hint('Click', 'focus, again to play'),
      ],
    );
  }
}
