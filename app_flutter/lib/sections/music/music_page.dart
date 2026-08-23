// The Music section shell: a header with the five category tabs and one
// search box, whichever tab is open below it, and the player bar across the
// bottom of all of them.
//
// The bar is outside the tab switch on purpose. It is the thing that makes
// these five pages one section rather than five: an album started in My Music
// keeps playing while you browse podcasts, and the transport at the bottom is
// still driving it.

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'audiobooks_tab.dart';
import 'music_controller.dart';
import 'music_widgets.dart';
import 'my_music_tab.dart';
import 'player_bar.dart';
import 'podcasts_tab.dart';
import 'radio_tab.dart';
import 'youtube_tab.dart';

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
  }

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        final st = _c.state;
        return ColoredBox(
          color: t.nCanvas,
          child: Column(
            children: [
              _Header(controller: _c, search: _search),
              if (_c.progress != null) _ProgressBar(controller: _c),
              if (_c.error != null) _ErrorBanner(controller: _c),
              if (st != null && st.status.isNotEmpty)
                _StatusBanner(message: st.status),
              Expanded(
                child: st == null
                    ? FirstLoad(error: _c.error, onRetry: _c.refresh)
                    // IndexedStack, not a switch: each tab holds scroll
                    // positions and text fields, and rebuilding the whole
                    // subtree on every category change would throw both away.
                    : IndexedStack(
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
              ),
              PlayerBar(controller: _c),
            ],
          ),
        );
      },
    );
  }
}

class _Header extends StatelessWidget {
  const _Header({required this.controller, required this.search});

  final MusicController controller;
  final TextEditingController search;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final active = controller.view;
    return DecoratedBox(
      // The 2px gradient underline the Slint header draws beneath its tab row.
      decoration: const BoxDecoration(
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
          color: t.nCanvas,
          child: SizedBox(
            height: 58,
            child: Row(
              children: [
                const SizedBox(width: 20),
                for (final v in musicViews)
                  Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 4),
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
                const Spacer(),
                // One search box for the section. What it filters depends on
                // the open tab, which is how the Slint header behaves too.
                SizedBox(
                  width: 280,
                  child: TextField(
                    controller: search,
                    decoration: InputDecoration(
                      isDense: true,
                      prefixIcon: const Icon(Icons.search, size: 18),
                      hintText: 'Search the library',
                      border: const OutlineInputBorder(),
                      suffixIcon: search.text.isEmpty
                          ? null
                          : IconButton(
                              icon: const Icon(Icons.close, size: 16),
                              onPressed: () {
                                search.clear();
                                controller
                                    .send(const MusicCmd.search(query: ''));
                              },
                            ),
                    ),
                    onSubmitted: (q) =>
                        controller.send(MusicCmd.search(query: q)),
                  ),
                ),
                const SizedBox(width: 20),
              ],
            ),
          ),
        ),
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

class _StatusBanner extends StatelessWidget {
  const _StatusBanner({required this.message});

  final String message;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      width: double.infinity,
      color: Tokens.warn.withValues(alpha: 0.12),
      padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 8),
      child: Row(
        children: [
          const Icon(Icons.info_outline, size: 16, color: Tokens.warn),
          const SizedBox(width: 8),
          Expanded(
            child: Text(message, style: TextStyle(fontSize: 12, color: t.nInk)),
          ),
        ],
      ),
    );
  }
}
