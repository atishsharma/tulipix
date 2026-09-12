// The Music section shell: a header with the five category tabs and one
// search box, whichever tab is open below it, and the player bar across the
// bottom of all of them.
//
// The bar is outside the tab switch on purpose. It is the thing that makes
// these five pages one section rather than five: an album started in My Music
// keeps playing while you browse podcasts, and the transport at the bottom is
// still driving it.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/design_language.dart';
import '../../design/first_load.dart';
import '../../design/pick.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/music.dart';
import 'audiobooks_tab.dart';
import 'music_controller.dart';
import 'music_widgets.dart';
import 'my_music_tab.dart';
import 'player_bar.dart';
import 'podcasts_tab.dart';
import 'radio_tab.dart';
import 'side_panel.dart';
import 'youtube_tab.dart';


/// Whether the keyboard is currently going into a text field.
///
/// The section's key bindings sit above every field in it and see a key before
/// a focused `TextField` does -- so without this, typing "search" into the
/// search box paused the deck on the space, toggled shuffle on the s, and loved
/// the track on the l. `_Keys` asks it before claiming a key. The focus node a
/// TextField installs lives inside `EditableText`, which is what this looks for.
bool _typing() {
  final ctx = FocusManager.instance.primaryFocus?.context;
  if (ctx == null) return false;
  return ctx.widget is EditableText ||
      ctx.findAncestorWidgetOfExactType<EditableText>() != null;
}

class MusicPage extends StatefulWidget {
  const MusicPage({super.key});

  @override
  State<MusicPage> createState() => _MusicPageState();
}

class _MusicPageState extends State<MusicPage> {
  // The app's one controller, not this page's: the floating mini and the zen
  // player are hosted above every section and read the same state.
  final MusicController _c = MusicController.instance;
  final TextEditingController _search = TextEditingController();

  @override
  void initState() {
    super.initState();
    // Not `_c.refresh()` straight: `initState` runs inside a build, `send`
    // notifies before it awaits, and the mini player wrapping the whole app is
    // already listening — marking it dirty mid-build is an assertion. Ask for
    // the first snapshot once this frame is out.
    WidgetsBinding.instance.addPostFrameCallback((_) => _c.refresh());
    // Home's YouTube and Music D/L launchers land on a tab, not on the
    // section's front page.
    ShellController.instance.onOpen(Section.music, (tab) {
      if (tab == 'downloader') {
        _c.send(const MusicCmd.setView(name: 'mymusic'));
        _c.send(const MusicCmd.setLibTab(name: 'downloader'));
      } else {
        _c.send(MusicCmd.setView(name: tab));
      }
    });
  }

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // A skin's face reaches every Music text that does not name one. Null
    // under Standard, which merges to nothing — the ambient Sora stands.
    final skin = context.skin;
    return DefaultTextStyle.merge(
      style: TextStyle(fontFamily: skin.fontFamily),
      child: AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        final st = _c.state;
        // Two gates on every transport key.
        //
        // The player has to be up. These keys move a deck; with nothing loaded
        // there is no deck, and Space silently doing nothing on a library page
        // is worse than Space doing what the platform would have done with it.
        // The condition is exactly the one `PlayerBar` renders itself under, so
        // "the keys work" and "the bar is on screen" are the same fact.
        final now = st?.now;
        final deckUp =
            now != null && (now.loaded || now.title.isNotEmpty);

        // And nothing may be being typed into -- which `_Keys` sees to, not
        // this: see there for why a check in the callback was not enough.
        VoidCallback guard(VoidCallback run) => () {
              if (!deckUp) return;
              run();
            };

        return _Keys(
          typing: _typing,
          // Escape closes whatever the player has open — the docked panel or
          // the equalizer — before anything outside the section sees the key.
          // Slint's is a focus scope that exists only while one is open; this
          // is the same rule, bound where the panels live.
          bindings: <ShortcutActivator, VoidCallback>{
            if (_c.panel.isNotEmpty)
              const SingleActivator(LogicalKeyboardKey.escape): () =>
                  _c.setPanel(_c.panel),
            // Transport.
            const SingleActivator(LogicalKeyboardKey.space):
                guard(_c.keyPlayPause),
            const SingleActivator(LogicalKeyboardKey.arrowLeft):
                guard(() => _c.nudge(-10)),
            const SingleActivator(LogicalKeyboardKey.arrowRight):
                guard(() => _c.nudge(10)),
            // Whole tracks, because holding the arrow to cross a nine-minute
            // side is not seeking, it is waiting.
            const SingleActivator(LogicalKeyboardKey.arrowLeft, shift: true):
                guard(_c.keyPrev),
            const SingleActivator(LogicalKeyboardKey.arrowRight, shift: true):
                guard(_c.keyNext),
            // Marks and modes.
            const SingleActivator(LogicalKeyboardKey.keyL): guard(_c.keyLove),
            const SingleActivator(LogicalKeyboardKey.keyS): guard(_c.keyShuffle),
            const SingleActivator(LogicalKeyboardKey.keyR): guard(_c.keyRepeat),
            const SingleActivator(LogicalKeyboardKey.keyQ):
                guard(() => _c.setPanel('queue')),
            // Back out of a detail page the way the browser key does, since
            // the trail is a history now. Not gated on the deck -- this is
            // navigation, not transport -- but still not while typing, or
            // Alt+Left in the search box leaves the page.
            if (_c.canGoBack)
              const SingleActivator(LogicalKeyboardKey.arrowLeft, alt: true):
                  () {
                if (_typing()) return;
                _c.goBack();
              },
          },
          // Under a language with a backdrop the page is the shell's card over
          // that backdrop, as every other section is, and the shell lights it
          // with this record while Music is up. Painting a canvas and a second
          // backdrop here drew the window twice: four window-sized gradients a
          // frame under Glass.
          child: _canvas(
            skin.pageBackdrop(accent: _c.accent) == null
                ? skin.canvas ?? t.nCanvas
                : null,
            Column(
              children: [
                _Header(controller: _c, search: _search),
                if (_c.progress != null) _ProgressBar(controller: _c),
                // One row, not two. A failed command sets `status` on the
                // snapshot AND emits `Failed`, which becomes `error` here, so
                // the same sentence arrived twice — once with a close button
                // and once without. The dismissible one wins; the status line
                // is for the notes no command failed over ("Queued 25 similar
                // tracks").
                if (_c.error != null)
                  _ErrorBanner(controller: _c)
                else if (st != null && st.status.isNotEmpty)
                  // Keyed, so the progress bar coming and going above it does
                  // not rebuild it into a fresh five seconds.
                  _StatusBanner(
                      key: const ValueKey('status'), message: st.status),
                Expanded(
                  child: st == null
                      ? FirstLoad(error: _c.error, onRetry: _c.refresh)
                      : Stack(
                          children: [
                            // IndexedStack, not a switch: each tab holds scroll
                            // positions and text fields, and rebuilding the whole
                            // subtree on every category change would throw both
                            // away.
                            IndexedStack(
                              index: musicViews
                                  .indexWhere((v) => v.id == st.view)
                                  .clamp(0, musicViews.length - 1),
                              children: [
                                MyMusicTab(controller: _c),
                                PodcastsTab(controller: _c),
                                AudiobooksTab(controller: _c),
                                RadioTab(controller: _c),
                                YoutubeTab(controller: _c),
                              ],
                            ),
                            // Queue and Lyrics dock here — under BOTH headers
                            // and above the player, which is the band Slint
                            // gives them. It used to start at the top of this
                            // Stack, which put it over the tab row belonging to
                            // the page behind it: the panel covered the way out
                            // of itself.
                            if (_c.panel == 'queue' || _c.panel == 'lyrics')
                              Positioned(
                                top: st.view == 'mymusic' ? 57 : 52,
                                left: 0,
                                right: 0,
                                bottom: 0,
                                // The lyric line follows the position.
                                child: ListenableBuilder(
                                  listenable: _c.ticks,
                                  builder: (_, __) => SidePanel(
                                      key: sidePanelKey, controller: _c),
                                ),
                              ),
                            // The equalizer opens over the page from the bar's
                            // top edge, rather than inside the bar, where it
                            // pushed everything above up by its own height.
                            if (_c.panel == 'eq')
                              Positioned(
                                left: 0,
                                right: 0,
                                bottom: 0,
                                height: 260,
                                child: PlayerEqPanel(controller: _c),
                              ),
                          ],
                        ),
                ),
                // A position tick rebuilds only the bar's lyric and seek rows,
                // inside it; this builder listens to the controller, which a
                // tick that moves nothing but the position does not fire.
                PlayerBar(controller: _c),
              ],
            ),
          ),
        );
      },
      ),
    );
  }
}

/// [child] on [colour], or on nothing when the shell's backdrop shows through.
Widget _canvas(Color? colour, Widget child) =>
    colour == null ? child : ColoredBox(color: colour, child: child);

class _Header extends StatelessWidget {
  const _Header({required this.controller, required this.search});

  final MusicController controller;
  final TextEditingController search;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final active = controller.view;
    final view = musicViews.firstWhere((v) => v.id == active,
        orElse: () => musicViews.first);
    final st = controller.state;
    final accent = controller.accent;
    final skin = context.skin;
    return DecoratedBox(
      // The 2px gradient underline the Slint header draws beneath its tab row.
      // A Standard trait: a skin keeps its one material edge to edge.
      decoration: !skin.isStandard
          ? const BoxDecoration()
          : const BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.bottomLeft,
          end: Alignment.bottomRight,
          stops: [0.0, 0.22, 0.78, 1.0],
          colors: [
            Color(0x00EC4899),
            Color(0xFFEC4899),
            Color(0xFF8B5CF6),
            Color(0x008B5CF6),
          ],
        ),
      ),
      child: Padding(
        padding: const EdgeInsets.only(bottom: 2),
        child: ColoredBox(
          // Clear under a skin: the page already paints its canvas below, and
          // a backdrop — an aura, a metal plate — has to show through.
          color: skin.isStandard ? t.nCanvas : Colors.transparent,
          // The album-art wash, which is the thing the header was missing: the
          // player bar at the foot of the page is painted in the cover's
          // colour and the header at the top of it was not, so the section
          // read as two unrelated bars around a white page. Slint's is
          // `@linear-gradient(90deg, np-accent.with-alpha(0.22), transparent
          // 55%)` over `surf-header` — left-anchored, gone by just past
          // halfway, so it never fights the search pill or the count.
          //
          // A BoxDecoration cannot carry both: a gradient replaces the colour
          // outright, so the canvas is the box under this one.
          child: DecoratedBox(
            // The cover wash is Standard's too; a skin lets its accent carry
            // the state instead.
            decoration: !skin.isStandard
                ? const BoxDecoration()
                : BoxDecoration(
              gradient: LinearGradient(
                begin: Alignment.centerLeft,
                end: Alignment.centerRight,
                stops: const [0.0, 0.55],
                colors: [
                  accent.withValues(alpha: 0.22),
                  accent.withValues(alpha: 0.0),
                ],
              ),
            ),
            child: SizedBox(
              // 72, not 64: the Slint row is 72 and the eight-pixel difference
              // is what was making the two-tier header look squashed against
              // the sub-tabs.
              height: 72,
              child: Row(
                children: [
                  const SizedBox(width: 28),
                  _Wordmark(accent: accent),
                  const SizedBox(width: 18),
                  // The five fill whatever is left between the title and the
                  // search pill, equally — `horizontal-stretch: 1` on each.
                  Expanded(
                    child: Row(
                      children: [
                        for (final v in musicViews) ...[
                          if (v != musicViews.first) const SizedBox(width: 8),
                          Expanded(
                            child: MusicChip(
                              label: v.label,
                              icon: v.icon,
                              active: active == v.id,
                              tint: v.tint,
                              tint2: v.tint2,
                              onTap: () =>
                                  controller.send(MusicCmd.setView(name: v.id)),
                            ),
                          ),
                        ],
                      ],
                    ),
                  ),
                  const SizedBox(width: 14),
                  _SearchPill(controller: controller, search: search),
                  const SizedBox(width: 10),
                  // Add follows the open section's colour. Always drawn as an
                  // active chip — it is an action, not a tab.
                  // What "add" means is different in each of the five, the
                  // way it is in Slint: a folder here, a feed URL there, a
                  // stream, a subscriptions export. One chip, five doors.
                  MusicChip(
                    label: '+ Add',
                    active: true,
                    tint: view.tint,
                    tint2: view.tint2,
                    minWidth: 86,
                    onTap: () => musicAdd(context, controller),
                  ),
                  const SizedBox(width: 10),
                  _countPill(st, active),
                  const SizedBox(width: 28),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// The header's `+ Add`, which means something different in each of the five.
///
/// Slint gives every view its own add control; the port had one that called
/// `addFolder` from all five, so "add" on the Radio tab was offering to scan a
/// directory of MP3s.
Future<void> musicAdd(BuildContext context, MusicController c) async {
  switch (c.view) {
    case 'podcasts':
      await addFeed(context, c);
    case 'radio':
      await addStation(context, c);
    case 'youtube':
      // There is nothing to type: a channel is subscribed to from its own page,
      // and the only thing you can hand YouTube from outside is the
      // subscriptions export Google gives you.
      final path = await pickFile(
        label: 'Subscriptions',
        extensions: const ['csv', 'json', 'opml', 'xml'],
      );
      if (path == null) return;
      await c.send(MusicCmd.ytImportSubs(path: path));
    // Audiobooks and My Music are both folders of files, so both get the same
    // chooser -- but the folder lands in whichever of the two is OPEN. The
    // bridge reads the current view when it takes the path, which is what makes
    // a folder picked here a shelf rather than four hundred songs in My Music.
    default:
      await c.addFolder();
  }
}

/// How much of what is open there is.
///
/// Per sub-tab inside My Music, not per section: `FancyCount` in Slint reads
/// `item-count`, which is whatever the open tab is counting, and a pill saying
/// "385 Tracks" while you look at a wall of twenty-two albums is answering a
/// question nobody asked. The numbers are already on the snapshot —
/// `songTotal` for the paged song lists, `cardTotal` for the browse grids — so
/// this is a switch, not a query.
Widget _countPill(MusicState? st, String view) {
  if (st == null) return const _CountPill(count: 0, label: 'Tracks');
  if (view != 'mymusic') {
    return _CountPill(
      // Radio's number is not the library's: it is how many stations are in
      // the local cache, which is what Refresh all changes. The pill said
      // "0 Stations" on a machine with thirty thousand of them.
      count: view == 'radio' ? st.radioTotal : st.trackCount,
      label: switch (view) {
        'podcasts' => 'Podcasts',
        'audiobooks' => 'Audiobooks',
        'radio' => 'Stations',
        _ => 'Channels',
      },
    );
  }
  final (count, label) = switch (st.libTab) {
    'songs' => (st.songTotal, 'Songs'),
    'albums' => (st.cardTotal, 'Albums'),
    'artists' => (st.cardTotal, 'Artists'),
    'genres' => (st.cardTotal, 'Genres'),
    'playlists' => (st.cardTotal, 'Playlists'),
    'folders' => (st.cardTotal, 'Folders'),
    'favorites' => (st.songTotal, 'Loved'),
    'history' => (st.songTotal, 'Played'),
    // Home and the downloader count nothing of their own, so they fall back to
    // the library.
    _ => (st.trackCount, 'Tracks'),
  };
  return _CountPill(count: count, label: label);
}

/// The note and the wordmark, both carrying the cover's colour.
///
/// The title is drawn twice: an accent-tinted copy offset a pixel and a half
/// right and two and a half down, then the real one over it. That is not a
/// drop shadow — it is Slint's, and the reason is the wash behind it. A single
/// flat ink title over a gradient that changes with every track loses its edge
/// on light covers; the offset copy gives it one in the cover's own colour
/// rather than in grey.
///
/// It stands still. Slint bounces the glyph and the wordmark on the bass
/// (`- 5px * vis-bars[0]`); that was dropped on purpose, as a frame source
/// that told the listener nothing the visualizer beside it does not.
class _Wordmark extends StatelessWidget {
  const _Wordmark({required this.accent});

  final Color accent;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final skin = context.skin;
    final style = TextStyle(
      fontFamily: skin.fontFamily ?? Tokens.fontFamily,
      fontSize: 22,
      fontWeight: FontWeight.w700,
    );
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(
          // A note, not a record crate. `Icons.music` in Slint, tinted
          // `np-accent.mix(fg, 0.45)` — the cover's colour pulled most of the
          // way to the page's ink, so it reads as text and not as a badge.
          skin.icon(Icons.music_note),
          size: 22,
          color: Color.lerp(t.nInk, accent, 0.45),
        ),
        const SizedBox(width: 8),
        Stack(
          children: [
            Padding(
              padding: const EdgeInsets.only(left: 1.5, top: 2.5),
              child: Text(
                'Music',
                style: style.copyWith(
                  color: accent.withValues(alpha: 0.55),
                ),
              ),
            ),
            Text('Music', style: style.copyWith(color: t.nInk)),
          ],
        ),
      ],
    );
  }
}

/// The one search box for the section, as a 44px pill inside a 1.5px gradient
/// ring over a white interior — both themes, which is deliberate in Slint: the
/// ring is the section's identity and the field under it has to stay a field.
///
/// The whole pill is the field: a click anywhere on it puts the cursor in the
/// text, and while the cursor is there the pill itself lights — the skin's
/// active control, or Standard's ring glowing — rather than a box being drawn
/// inside it.
class _SearchPill extends StatefulWidget {
  const _SearchPill({required this.controller, required this.search});

  final MusicController controller;
  final TextEditingController search;

  @override
  State<_SearchPill> createState() => _SearchPillState();
}

class _SearchPillState extends State<_SearchPill> {
  final FocusNode _focus = FocusNode();

  @override
  void initState() {
    super.initState();
    _focus.addListener(_onFocus);
  }

  void _onFocus() => setState(() {});

  @override
  void dispose() {
    _focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final controller = widget.controller;
    final search = widget.search;
    final radio = controller.view == 'radio';
    final skin = context.skin;
    final focused = _focus.hasFocus;
    // Search holds a value, so a skin sinks it into the page: its own well in
    // place of the gradient ring and the white field, and its lit control
    // while the cursor is in it.
    final well = focused
        ? skin.control(active: true, radius: 22)
        : skin.surface(SurfaceRole.well, radius: 22);
    return GestureDetector(
      onTap: _focus.requestFocus,
      child: Container(
      width: 340,
      height: 44,
      decoration: well ?? BoxDecoration(
        borderRadius: BorderRadius.circular(22),
        gradient: const LinearGradient(
          begin: Alignment(-1, -0.58),
          end: Alignment(1, 0.58),
          stops: [0.0, 0.5, 1.0],
          colors: [Color(0xFFEC4899), Color(0xFF8B5CF6), Color(0xFF06B6D4)],
        ),
        boxShadow: focused
            ? const [BoxShadow(color: Color(0x66EC4899), blurRadius: 14)]
            : null,
      ),
      padding: const EdgeInsets.all(1.5),
      child: Container(
        decoration: well != null
            ? null
            : BoxDecoration(
                color: Colors.white,
                borderRadius: BorderRadius.circular(20.5),
              ),
        padding: const EdgeInsets.only(left: 14, right: 8),
        child: Row(
          children: [
            Icon(skin.icon(Icons.search),
                size: 15,
                color: skin.inkDim ??
                    const Color(0xFF6B6B74)),
            const SizedBox(width: 8),
            Expanded(
              child: TextField(
                controller: search,
                focusNode: _focus,
                style: TextStyle(
                  fontFamily: skin.fontFamily ?? Tokens.fontFamily,
                  fontSize: 14,
                  color:
                      skin.ink ?? const Color(0xFF16161B),
                ),
                cursorColor: skin.accent ?? const Color(0xFFEC4899),
                // Radio is not in the library, so the library filter cannot
                // reach it: its stations live in radio.db and are found by
                // asking radio-browser. One box, two questions — which is what
                // Slint does, and is why the Radio tab had a second search
                // field of its own.
                textInputAction: radio
                    ? TextInputAction.search
                    : TextInputAction.unspecified,
                onSubmitted: radio
                    ? (q) => controller.send(MusicCmd.radioSearch(query: q))
                    : null,
                onChanged: radio
                    ? null
                    : (q) => controller.send(MusicCmd.search(query: q)),
                decoration: InputDecoration(
                  isCollapsed: true,
                  border: InputBorder.none,
                  hintText:
                      radio ? 'Search stations — press ↵' : 'Search music',
                  hintStyle: TextStyle(
                    fontFamily: skin.fontFamily ?? Tokens.fontFamily,
                    fontSize: 14,
                    color: skin.inkDim ??
                        const Color(0xFF8A8A92),
                  ),
                ),
              ),
            ),
            if (search.text.isNotEmpty)
              _Round(
                icon: Icons.close,
                onTap: () {
                  search.clear();
                  // An empty station search is a no-op in the bridge -- there
                  // is nothing to look for -- so clearing the box on Radio has
                  // to mean "back to the categories".
                  controller.send(radio
                      ? const MusicCmd.radioBack()
                      : const MusicCmd.search(query: ''));
                },
              ),
            const _Round(icon: Icons.mic_none, onTap: null),
          ],
        ),
      ),
      ),
    );
  }
}

/// The two 26px pink discs that live at the right end of the search pill.
class _Round extends StatelessWidget {
  const _Round({required this.icon, required this.onTap});

  final IconData icon;
  final VoidCallback? onTap;

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.only(left: 4),
        child: !context.skin.isStandard
            ? SkinButton(
                width: 26,
                height: 26,
                radius: 13,
                onTap: onTap,
                child: Icon(context.skin.icon(icon),
                    size: 13, color: context.skin.accent),
              )
            : SizedBox(
          width: 26,
          height: 26,
          child: Material(
            color: const Color(0x33EC4899),
            shape: const CircleBorder(),
            clipBehavior: Clip.antiAlias,
            child: InkWell(
              onTap: onTap,
              hoverColor: const Color(0xFFEC4899),
              child: Icon(icon, size: 13, color: const Color(0xFFEC4899)),
            ),
          ),
        ),
      );
}

/// `FancyCount` — how much of the open section there is, in a gradient pill.
class _CountPill extends StatelessWidget {
  const _CountPill({required this.count, required this.label});

  final int count;
  final String label;

  @override
  Widget build(BuildContext context) {
    final skin = context.skin;
    // The gradient pill with white type, under Standard and under Glass. Glass
    // draws a control at rest as bare glyphs on the pane, which is right for a
    // button and left a figure in no pill at all. The other skins raise the
    // count out of their material and ink it in the accent.
    final pill =
        skin.isStandard || skin.language == DesignLanguage.glassmorphism;
    return Container(
      height: 44,
      constraints: const BoxConstraints(minWidth: 150),
      padding: const EdgeInsets.only(left: 20, right: 22),
      decoration: pill
          ? const BoxDecoration(
              borderRadius: BorderRadius.all(Radius.circular(22)),
              gradient: LinearGradient(
                begin: Alignment(-1, -0.58),
                end: Alignment(1, 0.58),
                colors: [Color(0xFFEC4899), Color(0xFF8B5CF6)],
              ),
              boxShadow: [
                BoxShadow(color: Color(0x55EC4899), blurRadius: 16),
              ],
            )
          : skin.control(active: false, radius: 22),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          Text(
            '$count',
            style: TextStyle(
              fontFamily: skin.fontFamily ?? Tokens.fontFamily,
              fontSize: 23,
              fontWeight: FontWeight.w800,
              color: pill ? Colors.white : skin.accent,
            ),
          ),
          const SizedBox(width: 9),
          Text(
            label,
            style: TextStyle(
              fontFamily: skin.fontFamily ?? Tokens.fontFamily,
              fontSize: 12,
              fontWeight: FontWeight.w700,
              letterSpacing: 0.5,
              color: pill ? const Color(0xDDFFFFFF) : skin.inkDim,
            ),
          ),
        ],
      ),
    );
  }
}

class _ProgressBar extends StatelessWidget {
  const _ProgressBar({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final p = controller.progress!;
    final frac = p.total > 0 ? p.done / p.total : null;
    return Container(
      color: t.nCard,
      padding: const EdgeInsets.fromLTRB(24, 8, 24, 8),
      child: Row(
        children: [
          SizedBox(
            width: 160,
            child: LinearProgressIndicator(
              value: frac,
              minHeight: 4,
              backgroundColor: t.nHair,
              valueColor: const AlwaysStoppedAnimation(Tokens.secMusic),
            ),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              p.total > 0 ? '${p.label} · ${p.done} of ${p.total}' : p.label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(fontSize: 12, color: t.nInk2),
            ),
          ),
        ],
      ),
    );
  }
}

class _ErrorBanner extends StatelessWidget {
  const _ErrorBanner({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) => MaterialBanner(
        backgroundColor: Tokens.error.withValues(alpha: 0.12),
        content: Text('${controller.error}'),
        leading: const Icon(Icons.error_outline, color: Tokens.error),
        actions: [
          TextButton(
            onPressed: controller.clearError,
            child: const Text('Dismiss'),
          ),
        ],
      );
}

/// The note from the last command, under the second header. It closes itself
/// after five seconds, or on its close button; a different note brings it
/// back. It used to stay up until the next command replaced it, which on a
/// quiet page meant for good.
class _StatusBanner extends StatefulWidget {
  const _StatusBanner({super.key, required this.message});

  final String message;

  @override
  State<_StatusBanner> createState() => _StatusBannerState();
}

class _StatusBannerState extends State<_StatusBanner> {
  static const Duration _life = Duration(seconds: 5);

  Timer? _timer;
  bool _shut = false;

  void _arm() {
    _timer?.cancel();
    _shut = false;
    _timer = Timer(_life, _close);
  }

  void _close() {
    _timer?.cancel();
    if (mounted) setState(() => _shut = true);
  }

  @override
  void initState() {
    super.initState();
    _arm();
  }

  @override
  void didUpdateWidget(_StatusBanner old) {
    super.didUpdateWidget(old);
    if (old.message != widget.message) _arm();
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (_shut) return const SizedBox.shrink();
    final t = context.tokens;
    return Container(
      width: double.infinity,
      color: Tokens.warn.withValues(alpha: 0.12),
      padding: const EdgeInsets.only(left: 24, right: 12, top: 4, bottom: 4),
      child: Row(
        children: [
          const Icon(Icons.info_outline, size: 16, color: Tokens.warn),
          const SizedBox(width: 8),
          Expanded(
            child: Text(widget.message,
                style: TextStyle(fontSize: 12, color: t.nInk)),
          ),
          IconButton(
            tooltip: 'Close',
            onPressed: _close,
            icon: const Icon(Icons.close, size: 16),
            color: t.nInk2,
            visualDensity: VisualDensity.compact,
          ),
        ],
      ),
    );
  }
}

/// Key bindings that step aside while something is being typed.
///
/// `CallbackShortcuts` cannot do that: a binding that matches consumes the key
/// whether or not its callback does anything, so a "not while typing" check
/// inside the callback still swallowed Space, the arrows and L, S, R and Q
/// before the search box -- or any field on the page -- saw them. Here a key
/// that finds a text field focused is left unhandled, and the field gets it.
class _Keys extends StatelessWidget {
  const _Keys({
    required this.typing,
    required this.bindings,
    required this.child,
  });

  final bool Function() typing;
  final Map<ShortcutActivator, VoidCallback> bindings;
  final Widget child;

  @override
  Widget build(BuildContext context) => Focus(
        canRequestFocus: false,
        skipTraversal: true,
        onKeyEvent: (node, event) {
          if (event is KeyUpEvent || typing()) return KeyEventResult.ignored;
          for (final binding in bindings.entries) {
            if (binding.key.accepts(event, HardwareKeyboard.instance)) {
              binding.value();
              return KeyEventResult.handled;
            }
          }
          return KeyEventResult.ignored;
        },
        child: child,
      );
}
