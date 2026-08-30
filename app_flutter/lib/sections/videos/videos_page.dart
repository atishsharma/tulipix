// The Videos shell: the header with the seven kind tabs, the library sub-tabs,
// and whichever tab body the snapshot says is showing.
//
// Seven tabs over one bridge session. Which one is open lives in Rust, because
// entering a tab is also what loads it — `SetKind` is both the switch and the
// fetch, exactly as `set-kind` is on the Slint side.

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/videos.dart';
import 'videos_controller.dart';
import 'videos_discover.dart';
import 'videos_library.dart';
import 'videos_livetv.dart';
import 'videos_splus.dart';
import 'videos_stream.dart';
import 'videos_stream_pages.dart';
import 'videos_widgets.dart';

class VideosPage extends StatefulWidget {
  const VideosPage({super.key});

  @override
  State<VideosPage> createState() => _VideosPageState();
}

class _VideosPageState extends State<VideosPage> {
  final VideosController _c = VideosController();
  final TextEditingController _search = TextEditingController();
  final TextEditingController _streamQuery = TextEditingController();

  /// Stream's sub-view: search | downloads | history | bookmarks. It lives here
  /// rather than in Rust because the destinations that switch it are drawn in
  /// this header, and nothing in the session depends on which one is open.
  String _streamView = 'search';

  @override
  void initState() {
    super.initState();
    _c.refresh();
    // Home's Stream and Live TV launchers land on a tab, not on the section's
    // front page. `SetKind` is both the switch and the fetch here.
    ShellController.instance.onOpen(Section.videos, (tab) {
      _c.send(VideosCmd.setKind(kind: tab));
    });
  }

  @override
  void dispose() {
    _search.dispose();
    _streamQuery.dispose();
    _c.dispose();
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
              _Header(
                controller: _c,
                state: st,
                search: _search,
                streamActions: st?.kind == 'stream'
                    ? _streamActions(st!.stream)
                    : const [],
              ),
              if (st != null && !_isRemote(st.kind))
                _CategoryBar(controller: _c, state: st),
              if (st != null && st.kind == 'tv' && st.showOpen)
                _ShowBackBar(controller: _c, state: st),
              if (_c.scanRoot != null) _ScanBar(root: _c.scanRoot!),
              if (_c.scanResult != null)
                _Banner(
                  message: _c.scanResult!,
                  onDismiss: _c.clearScanResult,
                  hue: cPlay,
                ),
              if (_c.error != null)
                _Banner(
                  message: '${_c.error}',
                  onDismiss: _c.clearError,
                  hue: cErr,
                ),
              Expanded(
                child: st == null
                    ? FirstLoad(error: _c.error, onRetry: _c.refresh)
                    : _body(st),
              ),
            ],
          ),
        );
      },
    );
  }

  bool _isRemote(String kind) =>
      kind == 'discover' ||
      kind == 'stream' ||
      kind == 'livetv' ||
      kind == 'splus';

  List<Widget> _streamActions(StreamView s) => [
        VideoTab(
          hue: cDl,
          icon: Icons.download,
          label: s.dlPending > 0 ? 'Downloads (${s.dlPending})' : 'Downloads',
          active: _streamView == 'downloads',
          onTap: () => _toStream('downloads',
              () => _c.send(const VideosCmd.streamDownloadsLoad())),
        ),
        const SizedBox(width: 8),
        VideoTab(
          hue: cSave,
          icon: Icons.bookmark_outline,
          label: 'Bookmarks',
          active: _streamView == 'bookmarks',
          onTap: () => _toStream('bookmarks',
              () => _c.send(const VideosCmd.streamBookmarksLoad())),
        ),
        const SizedBox(width: 8),
        VideoTab(
          hue: cInfo,
          icon: Icons.history,
          label: 'History',
          active: _streamView == 'history',
          onTap: () => _toStream('history',
              () => _c.send(const VideosCmd.streamHistoryLoad(page: 0))),
        ),
        const SizedBox(width: 8),
        VideoTab(
          hue: cInfo,
          icon: Icons.settings,
          label: 'Stream Setting',
          onTap: () => openStreamSettings(context, _c),
        ),
      ];

  void _toStream(String view, VoidCallback load) {
    if (_streamView == view) {
      setState(() => _streamView = 'search');
      return;
    }
    setState(() => _streamView = view);
    load();
  }

  Widget _body(VideosState st) => switch (st.kind) {
        'discover' => VideosDiscover(controller: _c, state: st),
        'livetv' => VideosLiveTv(controller: _c, live: st.live),
        'splus' => VideosSplus(controller: _c, splus: st.splus),
        'stream' => VideosStream(
            controller: _c,
            stream: st.stream,
            query: _streamQuery,
            view: _streamView,
            onView: (v) => setState(() => _streamView = v),
          ),
        _ => VideosLibrary(controller: _c, state: st),
      };
}

class _Header extends StatelessWidget {
  const _Header({
    required this.controller,
    required this.state,
    required this.search,
    required this.streamActions,
  });

  final VideosController controller;
  final VideosState? state;
  final TextEditingController search;
  final List<Widget> streamActions;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = state;
    final kind = st?.kind ?? 'local';
    final stream = kind == 'stream';
    return Container(
      height: 72,
      color: t.panel2,
      padding: const EdgeInsets.symmetric(horizontal: 28),
      child: Row(
        children: [
          Container(
            width: 34,
            height: 34,
            decoration: BoxDecoration(
              color: Tokens.secVideos.withValues(alpha: 0.18),
              borderRadius: BorderRadius.circular(10),
            ),
            child: const Icon(Icons.movie, size: 18, color: Tokens.secVideos),
          ),
          const SizedBox(width: 10),
          Text('Videos',
              style: TextStyle(
                  fontSize: 22, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(width: 12),
          for (final k in kVideoKinds) ...[
            VideoTab(
              hue: k.hue,
              label: k.label,
              active: kind == k.id,
              onTap: () => controller.send(VideosCmd.setKind(kind: k.id)),
            ),
            const SizedBox(width: 8),
          ],
          const Spacer(),
          // Stream searches the remote catalogue from its own field; Live TV
          // and Stream Plus have their own too.
          if (!stream && kind != 'livetv' && kind != 'splus') ...[
            VideoSearchField(
              controller: search,
              hint: 'Search videos',
              onChanged: (q) => controller.send(VideosCmd.search(query: q)),
            ),
            const SizedBox(width: 12),
            VideoTab(
              hue: cPlay,
              label: '+ Add folder',
              active: true,
              onTap: () => addVideoFolder(context, controller),
            ),
            const SizedBox(width: 12),
            Container(
              height: 44,
              padding: const EdgeInsets.symmetric(horizontal: 16),
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: Tokens.secVideos,
                borderRadius: BorderRadius.circular(22),
              ),
              child: Text('${st?.itemCount ?? 0} videos',
                  style: const TextStyle(
                      fontSize: 12,
                      fontWeight: FontWeight.w800,
                      color: Colors.white)),
            ),
          ],
          if (kind == 'livetv') ...[
            VideoSearchField(
              controller: search,
              width: 300,
              hint: 'Search channels',
              onChanged: (q) => controller.send(VideosCmd.liveSearch(query: q)),
            ),
            const SizedBox(width: 12),
            VideoTab(
              hue: cSave,
              icon: Icons.list,
              label: (st?.live.pickedCount ?? 0) == 0
                  ? 'Playlists'
                  : 'Playlists (${st!.live.pickedCount})',
              onTap: () => openPlaylistPicker(context, controller, st!.live),
            ),
            const SizedBox(width: 8),
            VideoTab(
              hue: cBack,
              icon: Icons.refresh,
              label: (st?.live.busy ?? false) ? 'Loading…' : 'Refresh',
              onTap: () {
                if (!(st?.live.busy ?? false)) {
                  controller.send(const VideosCmd.liveRefresh());
                }
              },
            ),
            if ((st?.live.nowName ?? '').isNotEmpty) ...[
              const SizedBox(width: 8),
              _NowPlayingPill(controller: controller, live: st!.live),
            ],
          ],
          ...streamActions,
        ],
      ),
    );
  }
}

/// The on-air pill. Tapping it opens the sheet with the channel's own details
/// and what mpv is being fed.
class _NowPlayingPill extends StatelessWidget {
  const _NowPlayingPill({required this.controller, required this.live});

  final VideosController controller;
  final LiveView live;

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: () => openNowPlaying(context, controller, live),
      child: MouseRegion(
        cursor: SystemMouseCursors.click,
        child: Container(
          height: 44,
          padding: const EdgeInsets.only(left: 6, right: 16),
          decoration: BoxDecoration(
            color: Tokens.secVideos,
            borderRadius: BorderRadius.circular(22),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Container(
                width: 26,
                height: 26,
                decoration: const BoxDecoration(
                    color: Color(0xFF3A3F48), shape: BoxShape.circle),
                clipBehavior: Clip.antiAlias,
                child: live.nowLogo.isEmpty
                    ? const Icon(Icons.radio, size: 13, color: Colors.white70)
                    : Artwork(
                        path: live.nowLogo,
                        width: 26,
                        height: 26,
                        radius: 13,
                        fallback: Icons.radio),
              ),
              const SizedBox(width: 8),
              Text(live.nowLabel,
                  style: const TextStyle(
                      fontSize: 12,
                      fontWeight: FontWeight.w800,
                      color: Colors.white)),
            ],
          ),
        ),
      ),
    );
  }
}

class _CategoryBar extends StatelessWidget {
  const _CategoryBar({required this.controller, required this.state});

  final VideosController controller;
  final VideosState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 54,
      color: t.panel,
      padding: const EdgeInsets.symmetric(horizontal: 28),
      child: Row(
        children: [
          for (final c in kVideoCategories) ...[
            VideoTab(
              hue: c.hue,
              label: c.label,
              active: state.category == c.id,
              onTap: () => controller.send(VideosCmd.setCategory(name: c.id)),
            ),
            const SizedBox(width: 8),
          ],
          const Spacer(),
          VideoTab(
            hue: cBack,
            icon: Icons.refresh,
            label: 'Clear thumbs',
            onTap: () => controller.send(const VideosCmd.clearThumbs()),
          ),
          const SizedBox(width: 8),
          VideoTab(
            hue: cDl,
            icon: Icons.sync,
            label: 'Rescan',
            onTap: () => controller.send(const VideosCmd.scan()),
          ),
        ],
      ),
    );
  }
}

class _ShowBackBar extends StatelessWidget {
  const _ShowBackBar({required this.controller, required this.state});

  final VideosController controller;
  final VideosState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 44,
      color: t.panel,
      padding: const EdgeInsets.symmetric(horizontal: 28),
      child: Row(
        children: [
          VideoTab(
            icon: Icons.chevron_left,
            label: 'Shows',
            onTap: () => controller.send(const VideosCmd.showBack()),
          ),
          const SizedBox(width: 10),
          Text(state.showTitle,
              style: TextStyle(
                  fontSize: 15, fontWeight: FontWeight.w700, color: t.nInk)),
        ],
      ),
    );
  }
}

class _ScanBar extends StatelessWidget {
  const _ScanBar({required this.root});

  final String root;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      color: t.panel2,
      padding: const EdgeInsets.fromLTRB(28, 8, 28, 8),
      child: Row(
        children: [
          const SizedBox(
              width: 14,
              height: 14,
              child: CircularProgressIndicator(strokeWidth: 2)),
          const SizedBox(width: 12),
          Expanded(
            child: Text('Scanning $root…',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12, color: t.nInk2)),
          ),
        ],
      ),
    );
  }
}

class _Banner extends StatelessWidget {
  const _Banner({
    required this.message,
    required this.onDismiss,
    required this.hue,
  });

  final String message;
  final VoidCallback onDismiss;
  final Color hue;

  @override
  Widget build(BuildContext context) {
    return Container(
      color: hue.withValues(alpha: 0.14),
      padding: const EdgeInsets.fromLTRB(28, 8, 12, 8),
      child: Row(
        children: [
          Icon(Icons.info_outline, size: 15, color: hue),
          const SizedBox(width: 10),
          Expanded(
            child: Text(message,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12, color: context.tokens.nInk)),
          ),
          IconButton(
              iconSize: 15,
              onPressed: onDismiss,
              icon: const Icon(Icons.close)),
        ],
      ),
    );
  }
}
