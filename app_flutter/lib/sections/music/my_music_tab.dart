// My Music — the first and largest of the five tabs.
//
// Nine sub-tabs: Home's rails and listening figures, the paged songs list,
// four browse grids (albums, artists, genres, playlists), the watched-folder
// list, favourites and history. Opening any card lands on the detail page,
// which is one file over.

import 'dart:math' as math;

import 'dart:async';

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'detail_page.dart';
import 'downloader_tab.dart';
import 'library_insights.dart';
import 'music_controller.dart';
import 'meta_manager.dart';
import 'music_dialogs.dart';
import 'music_motion.dart';
import 'music_widgets.dart';
import 'song_menu.dart';

class MyMusicTab extends StatelessWidget {
  const MyMusicTab({super.key, required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    if (st == null) return const Center(child: CircularProgressIndicator());

    // The sub-tabs stay up on the detail pages. Slint's row 2 is
    // `if root.view == "mymusic"` and nothing else — there is no
    // detail-is-open branch, so opening an album keeps the ten tabs where they
    // were. The port returned the detail page from above this row, which is
    // why every album, artist, genre and playlist page had no second header at
    // all and no way back to a sibling tab.
    return Column(
      children: [
        _SubTabs(controller: controller),
        Expanded(
          // Keyed on the tab AND on whether a detail is open, so switching
          // tabs and opening a page both dissolve rather than snap. The key
          // has to carry the detail identity too: album → artist is the same
          // tab and the same widget type, and without it the switcher sees no
          // change at all.
          child: CrossFade(
            slotKey: st.detailOpen
                ? 'detail:${st.detailKind}:${st.detailId}:${st.detailKey}'
                : 'tab:${st.libTab}',
            alignment: Alignment.topCenter,
            child: st.detailOpen
                ? DetailPage(controller: controller)
                : _body(context, st),
          ),
        ),
      ],
    );
  }

  Widget _body(BuildContext context, MusicState st) {
    // An empty library is the one state that beats every sub-tab: none of them
    // can show anything, and the fix is the same from all nine.
    if (st.trackCount == 0 && st.roots.isEmpty) {
      return MusicEmpty(
        icon: Icons.library_music_outlined,
        title: 'No music yet',
        body: 'Add a folder of audio files and Tulipix will read their tags, '
            'group them into albums and artists, and keep watching it.',
        action: ('Add a folder', controller.addFolder),
      );
    }
    return switch (st.libTab) {
      'home' => _Home(controller: controller, st: st),
      'downloader' => const DownloaderTab(),
      // Songs is a *grid* in Slint, not a list — the list is what Favourites
      // and History are, and conflating the three is what made all of My Music
      // look like one page with different contents.
      'songs' => _Songs(controller: controller, st: st),
      'favorites' || 'history' => _TrackPage(controller: controller, st: st),
      'folders' => _Folders(controller: controller, st: st),
      'playlists' => _Playlists(controller: controller, st: st),
      _ => _Browse(controller: controller, st: st),
    };
  }
}

class _SubTabs extends StatelessWidget {
  const _SubTabs({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final active = controller.libTab;
    return SizedBox(
      // 57 in Slint — 54 of content over the 3px the gradient underline sits
      // in.
      height: 57,
      child: Row(
        children: [
          Expanded(
            child: ListView(
              scrollDirection: Axis.horizontal,
              padding: const EdgeInsets.symmetric(horizontal: 20),
              children: [
                // Collapsed only on Songs. That tab spends the right-hand end
                // of this row on play, shuffle, tags, sort, rescan and a pager,
                // so ten labelled chips beside it leave nothing readable; every
                // other tab has room and reads better named than guessed at
                // from an icon. Each carries its own hue, so the row is ten
                // places rather than one selected thing and nine greys.
                for (final tab in libTabs)
                  Padding(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 4, vertical: 8),
                    child: MusicChip(
                      label: tab.label,
                      icon: tab.icon,
                      active: active == tab.id,
                      tint: tab.tint,
                      tint2: tab.tint2,
                      collapsible: active == 'songs',
                      // 110, not 92: ten collapsed chips at 20% more each is
                      // the row Slint draws, and at 92 the icon had barely a
                      // disc of its own to sit in. Expanded, it is a floor --
                      // the label takes what it needs above it.
                      minWidth: 110,
                      onTap: () =>
                          controller.send(MusicCmd.setLibTab(name: tab.id)),
                    ),
                  ),
              ],
            ),
          ),
          // What sits at the right end is per-tab, as in Slint: nothing on
          // Home, the sort chips and pagination on Songs. The four discs that
          // used to be here unconditionally (rescan, read tags, audio
          // settings, add folder) each duplicated a control that already
          // exists — Add is a chip in row 1, the other three are on Folders.
          ..._rowTwo(context, controller, active),
          const SizedBox(width: 20),
        ],
      ),
    );
  }

  /// What hangs off the right end of row 2, per tab.
  ///
  /// Slint has no third bar anywhere in My Music: whatever a tab needs — play
  /// it, sort it, page it, rescan it — hangs off row 2, and the fold that a
  /// third row of chips was eating goes back to the tiles.
  List<Widget> _rowTwo(BuildContext context, MusicController c, String tab) =>
      switch (tab) {
        'songs' => _songControls(context, c),
        'albums' || 'artists' || 'genres' => _browseControls(c, tab),
        'folders' => _folderControls(context, c),
        'favorites' || 'history' => _listControls(context, c, tab),
        _ => const [],
      };

  /// Songs: play the page, shuffle it, fix its tags and words, sort it, rescan
  /// the library, and step through it.
  List<Widget> _songControls(BuildContext context, MusicController c) {
    final st = c.state;
    final songs = st?.songs ?? const <Track>[];
    return [
      DetailActionBtn(
        icon: Icons.play_arrow,
        label: 'Play all',
        tint: const Color(0xFF22C55E),
        onTap: () => c.playFrom(songs, 0, 'songs'),
      ),
      const SizedBox(width: 8),
      DetailActionBtn(
        icon: Icons.shuffle,
        label: 'Shuffle',
        tint: const Color(0xFF3B82F6),
        onTap: () => _shuffleAll(c, songs, 'songs'),
      ),
      const SizedBox(width: 8),
      // Tags & Lyrics — Slint's one pill, opening a two-tab manager.
      DetailActionBtn(
        icon: Icons.label_outline,
        label: 'Tags & Lyrics',
        tint: const Color(0xFFEC4899),
        onTap: () => openMetaManager(context, c),
      ),
      const SizedBox(width: 12),
      _SongSort(controller: c),
      const SizedBox(width: 8),
      _RescanBtn(controller: c),
      if ((st?.songPages ?? 1) > 1) ...[
        const SizedBox(width: 12),
        Pager(
          page: st!.songPage,
          pages: st.songPages,
          compact: true,
          onGo: (p) => c.send(MusicCmd.setSongPage(page: p)),
        ),
      ],
    ];
  }

  /// Albums, Artists and Genres: sort, then page. Both were a third row under
  /// the tabs with a count nobody needed — the header's own pill already says
  /// how many.
  List<Widget> _browseControls(MusicController c, String tab) {
    final st = c.state;
    return [
      // Genres have one order and it is alphabetical; there is nothing to
      // sort them by that the name does not already do.
      if (tab != 'genres') _BrowseSort(controller: c),
      if ((st?.browsePages ?? 1) > 1) ...[
        const SizedBox(width: 12),
        Pager(
          page: st!.browsePage,
          pages: st.browsePages,
          compact: true,
          onGo: (p) => c.send(MusicCmd.setBrowsePage(page: p)),
        ),
      ],
    ];
  }

  /// Folders: one job and a pager.
  ///
  /// Add folder was the same control as the header's `+ Add`, and Read tags is
  /// what the rescan does on its way past — two buttons that each duplicated
  /// something, on the one tab with no room for them.
  List<Widget> _folderControls(BuildContext context, MusicController c) {
    final st = c.state;
    return [
      _RescanBtn(controller: c, label: 'Rescan all'),
      if ((st?.browsePages ?? 1) > 1) ...[
        const SizedBox(width: 12),
        Pager(
          page: st!.browsePage,
          pages: st.browsePages,
          compact: true,
          onGo: (p) => c.send(MusicCmd.setBrowsePage(page: p)),
        ),
      ],
    ];
  }

  /// Loved and History: play the lot, shuffle it, clear it, page it.
  List<Widget> _listControls(
      BuildContext context, MusicController c, String tab) {
    final st = c.state;
    final songs = st?.songs ?? const <Track>[];
    return [
      DetailActionBtn(
        icon: Icons.play_arrow,
        label: 'Play all',
        tint: const Color(0xFF22C55E),
        onTap: () => c.playFrom(songs, 0, tab),
      ),
      const SizedBox(width: 8),
      DetailActionBtn(
        icon: Icons.shuffle,
        label: 'Shuffle',
        tint: const Color(0xFF3B82F6),
        onTap: () => _shuffleAll(c, songs, tab),
      ),
      if (tab == 'history') ...[
        const SizedBox(width: 8),
        DetailActionBtn(
          icon: Icons.delete_outline,
          label: 'Clear',
          tint: Tokens.error,
          onTap: () => confirmThen(
            context,
            c,
            title: 'Clear the play history?',
            body: 'Everything you have played is forgotten, and the listening '
                'figures on Home go with it. The files are untouched.',
            action: 'Clear history',
            cmd: const MusicCmd.clearHistory(),
          ),
        ),
      ],
      if ((st?.songPages ?? 1) > 1) ...[
        const SizedBox(width: 12),
        Pager(
          page: st!.songPage,
          pages: st.songPages,
          compact: true,
          onGo: (p) => c.send(MusicCmd.setSongPage(page: p)),
        ),
      ],
    ];
  }
}

/// Re-walk the watched folders. Confirmed, because on a big library it is
/// minutes of disk and the progress strip takes over the top of the section.
class _RescanBtn extends StatelessWidget {
  const _RescanBtn({required this.controller, this.label = 'Rescan'});

  final MusicController controller;
  final String label;

  @override
  Widget build(BuildContext context) => DetailActionBtn(
        icon: Icons.refresh,
        label: label,
        tint: const Color(0xFF8B5CF6),
        onTap: () => confirmThen(
          context,
          controller,
          title: 'Rescan the library?',
          body: 'Every watched folder is walked again. New files are added, '
              'files that have gone are marked missing, and nothing you have '
              'loved or rated is touched.',
          action: 'Rescan',
          cmd: const MusicCmd.scan(),
        ),
      );
}

/// Name / Tracks, as one popup rather than two chips — the same control the
/// Songs tab sorts with, so row 2 reads the same on every tab that has one.
class _BrowseSort extends StatelessWidget {
  const _BrowseSort({required this.controller});

  final MusicController controller;

  static const Map<String, String> _modes = {
    'name': 'Name',
    'count': 'Tracks',
  };

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    final mode = st?.browseSort ?? 'name';
    final dir = st?.browseDir ?? 'asc';
    return SortMenu(
      modes: _modes,
      mode: mode,
      dir: dir,
      onPick: (v) => controller.send(
        v == mode
            ? MusicCmd.setBrowseSort(
                mode: mode, dir: dir == 'desc' ? 'asc' : 'desc')
            : MusicCmd.setBrowseSort(mode: v, dir: dir),
      ),
    );
  }
}

class _SongSort extends StatelessWidget {
  const _SongSort({required this.controller});

  final MusicController controller;

  static const Map<String, String> _modes = {
    'title': 'Title',
    'artist': 'Artist',
    'album': 'Album',
    'rating': 'Rating',
    'added': 'Date added',
    'plays': 'Play count',
    'duration': 'Length',
  };

  @override
  Widget build(BuildContext context) {
    final st = controller.state;
    // Title, ascending, matching the bridge's default: a library opens
    // alphabetically, not newest-first.
    final mode = st?.songSort ?? 'title';
    final dir = st?.songDir ?? 'asc';
    return SortMenu(
      modes: _modes,
      mode: mode,
      dir: dir,
      onPick: (v) => controller.send(
        // Picking the mode that is already active flips the direction, which
        // is what a sort header does everywhere else.
        v == mode
            ? MusicCmd.setSongSort(
                mode: mode, dir: dir == 'desc' ? 'asc' : 'desc')
            : MusicCmd.setSongSort(mode: v, dir: dir),
      ),
    );
  }
}

/// The My Music dashboard, in the order ui/page_music.slint builds it:
/// recently played beside the top artists, then top albums edge to edge, then
/// the listening strip, then most played and recently added.
///
/// The shape matters as much as the contents. Recently played is a *list* —
/// six rows you read down, with a pager — because what you want from it is the
/// name of a track, and a shelf of squares makes you read titles sideways at
/// half the density. The artists next to it are circles because a face is not
/// an album.
class _Home extends StatefulWidget {
  const _Home({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  State<_Home> createState() => _HomeState();
}

/// Six to a page, which over a fourteen-track rail is the 1/3 the Slint build
/// shows. Local state: the rail arrives whole, so paging it is a view concern
/// and not worth a round trip.
const int _recentPerPage = 6;

/// Below this the artist grid stops fitting beside the list and goes under it.
/// 692 for the grid plus 28 of gutter plus something readable for the rows.
const double _twoColumnMin = 1180;

class _HomeState extends State<_Home> {
  int _page = 0;

  @override
  Widget build(BuildContext context) {
    final st = widget.st;
    final c = widget.controller;
    final pages = (st.railRecent.length / _recentPerPage).ceil().clamp(1, 99);
    final page = _page.clamp(0, pages - 1);
    final slice =
        st.railRecent.skip(page * _recentPerPage).take(_recentPerPage).toList();

    return LayoutBuilder(
      builder: (context, box) {
        final wide = box.maxWidth >= _twoColumnMin;
        final recent = _Recent(
          controller: c,
          rows: slice,
          all: st.railRecent,
          offset: page * _recentPerPage,
          page: page,
          pages: pages,
          onGo: (p) => setState(() => _page = p),
        );
        // The artist grid is measured off the list beside it, not off its own
        // cell width. Four columns of `width / 4` gave two rows about fifty
        // pixels taller than six track rows, and the whole difference showed
        // as blank canvas under Recently played. Two rows of exactly half the
        // list's height, and the circles take whatever that leaves.
        // Measured off a full page, not off this one. The last page of a short
        // rail can be a single row, and half of one track row is 27 pixels --
        // less than the label alone, so the circles overflowed their cells on
        // every rail with a remainder. Clamped as well: an empty rail would
        // otherwise ask for a negative height.
        final railRows = math.min(st.railRecent.length, _recentPerPage);
        final artists = _TopArtists(
          controller: c,
          cards: st.railArtists,
          rowHeight: math.max(
            96.0,
            (railRows * 56 + math.max(0, railRows - 1) * 2) / 2,
          ),
        );

        return ListView(
          padding: const EdgeInsets.only(top: 18, bottom: 28),
          children: [
            // The listening strip sits directly under the sub-tabs. It is the
            // page's summary — how much you have played this week, all time,
            // what you play most — and a summary belongs above the thing it
            // summarises, not buried between two shelves of covers.
            _StatsStrip(stats: st.stats, controller: c),
            const SizedBox(height: 22),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 36),
              child: wide
                  ? Row(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Expanded(child: recent),
                        const SizedBox(width: 28),
                        // Fixed, not stretchy: 692 is what buys the fourth
                        // column at the same cell size the three-column
                        // version had, so nothing reflows when the sidebar
                        // moves. The list beside it gives up the difference.
                        SizedBox(width: 692, child: artists),
                      ],
                    )
                  : Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [recent, const SizedBox(height: 28), artists],
                    ),
            ),
            const SizedBox(height: 28),
            // Records you started and walked away from, directly above the
            // shelf of things you have not started at all — which is the row
            // it is answering.
            _ResumeRail(controller: c, revision: st.stats.total),
            _CardRow(
              title: 'Top albums',
              cards: st.railAlbums,
              controller: c,
              onTap: (a) => c.send(MusicCmd.openAlbum(albumId: a.id)),
            ),
            if (st.railMost.isNotEmpty) ...[
              const SizedBox(height: 28),
              _TrackRow2(
                title: 'Most played',
                tracks: st.railMost,
                controller: c,
                source: 'most',
              ),
            ],
            if (st.railLoved.isNotEmpty) ...[
              const SizedBox(height: 28),
              _TrackRow2(
                title: 'Loved',
                tracks: st.railLoved,
                controller: c,
                source: 'loved',
              ),
            ],
            if (st.railFresh.isNotEmpty) ...[
              const SizedBox(height: 28),
              _TrackRow2(
                title: 'Recently added',
                tracks: st.railFresh,
                controller: c,
                source: 'fresh',
                // The rail is one row wide and the library is not. The whole
                // of it is the "Recently added" playlist, rebuilt on the way
                // in — a list you can scroll, queue, shuffle and reorder,
                // rather than the Songs tab under a sort you then have to
                // undo.
                onViewAll: () => c.send(const MusicCmd.playlistOpenFresh()),
              ),
            ],
          ],
        );
      },
    );
  }
}

/// A dashboard heading. 20/700 — the Slint size, and a full step above the
/// 15px the horizontal rails were using, which is what made the port's home
/// read as one undifferentiated column of cards.
class _Heading extends StatelessWidget {
  const _Heading(this.text, {this.trailing});

  final String text;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      children: [
        Text(
          text,
          style: TextStyle(
            fontFamily: Tokens.fontFamily,
            fontSize: 20,
            fontWeight: FontWeight.w700,
            color: t.nInk,
          ),
        ),
        const Spacer(),
        if (trailing != null) trailing!,
      ],
    );
  }
}

/// `View all ›` — a text button at the right edge of a rail's heading.
class _ViewAll extends StatelessWidget {
  const _ViewAll({required this.onTap});

  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) => Material(
        color: Colors.transparent,
        borderRadius: BorderRadius.circular(14),
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(14),
          child: const Padding(
            padding: EdgeInsets.symmetric(horizontal: 10, vertical: 5),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  'View all',
                  style: TextStyle(
                    fontFamily: Tokens.fontFamily,
                    fontSize: 12.5,
                    fontWeight: FontWeight.w700,
                    color: Tokens.secMusic,
                  ),
                ),
                Icon(Icons.chevron_right, size: 16, color: Tokens.secMusic),
              ],
            ),
          ),
        ),
      );
}

class _Recent extends StatelessWidget {
  const _Recent({
    required this.controller,
    required this.rows,
    required this.all,
    required this.offset,
    required this.page,
    required this.pages,
    required this.onGo,
  });

  final MusicController controller;
  final List<Track> rows;
  final List<Track> all;
  final int offset;
  final int page;
  final int pages;
  final void Function(int) onGo;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _Heading(
          'Recently played',
          trailing: pages > 1
              ? _MiniPager(page: page, pages: pages, onGo: onGo)
              : null,
        ),
        const SizedBox(height: 10),
        if (all.isEmpty)
          Text(
            'Play something — your recent tracks land here.',
            style: TextStyle(fontSize: 13, color: t.nInk2),
          ),
        for (var i = 0; i < rows.length; i++) ...[
          if (i > 0) const SizedBox(height: 2),
          TrackRow(
            controller: controller,
            track: rows[i],
            index: offset + i,
            compact: true,
            onPlay: () => controller.playFrom(all, offset + i, 'recent'),
          ),
        ],
      ],
    );
  }
}

/// `‹ 1/3 ›` — smaller than the page-level [Pager], because it belongs to one
/// card on a dashboard rather than to the whole view.
class _MiniPager extends StatelessWidget {
  const _MiniPager({
    required this.page,
    required this.pages,
    required this.onGo,
  });

  final int page;
  final int pages;
  final void Function(int) onGo;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget btn(IconData icon, int to, bool on) => SizedBox(
          width: 30,
          height: 28,
          child: Material(
            color: t.nChip,
            borderRadius: BorderRadius.circular(14),
            child: InkWell(
              borderRadius: BorderRadius.circular(14),
              onTap: on ? () => onGo(to) : null,
              child: Icon(
                icon,
                size: 13,
                color: on ? t.nInk2 : t.nInk3,
              ),
            ),
          ),
        );
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        btn(Icons.chevron_left, page - 1, page > 0),
        const SizedBox(width: 10),
        Text('${page + 1}/$pages',
            style: TextStyle(fontSize: 12, color: t.nInk2)),
        const SizedBox(width: 10),
        btn(Icons.chevron_right, page + 1, page + 1 < pages),
      ],
    );
  }
}

/// Four across, two down, round. `TileGrid { circular: true; force-cols: 4 }`.
///
/// [rowHeight] is what keeps this level with whatever is beside it: the cell
/// is given that height and the circle takes what is left after the label,
/// rather than the circle being the cell width and the block ending wherever
/// it ends.
class _TopArtists extends StatelessWidget {
  const _TopArtists({
    required this.controller,
    required this.cards,
    this.rowHeight,
  });

  final MusicController controller;
  final List<BrowseCard> cards;
  final double? rowHeight;

  @override
  Widget build(BuildContext context) {
    final shown = cards.take(8).toList();
    if (shown.isEmpty) return const SizedBox.shrink();
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const _Heading('Top artists'),
        const SizedBox(height: 10),
        LayoutBuilder(
          builder: (context, box) {
            final cell = box.maxWidth / 4;
            return Wrap(
              children: [
                for (final a in shown)
                  SizedBox(
                    width: cell,
                    height: rowHeight,
                    child: Padding(
                      padding: const EdgeInsets.symmetric(
                          horizontal: 8, vertical: 6),
                      child: MusicCard(
                        controller: controller,
                        title: a.title,
                        subtitle: '',
                        artKind: 'artist',
                        artKey: a.key,
                        direct: a.art,
                        round: true,
                        centred: true,
                        fallback: Icons.person,
                        onTap: () => controller
                            .send(MusicCmd.openArtist(artistId: a.id)),
                      ),
                    ),
                  ),
              ],
            );
          },
        ),
      ],
    );
  }
}

/// One edge-to-edge row of square cards. The cell is fixed and the column
/// count follows the width — `floor(width / 150)`, capped at seven — so the
/// tiles keep their size and the row keeps its rhythm as the window moves.
class _CardRow extends StatelessWidget {
  const _CardRow({
    required this.title,
    required this.cards,
    required this.controller,
    required this.onTap,
  });

  final String title;
  final List<BrowseCard> cards;
  final MusicController controller;
  final void Function(BrowseCard) onTap;

  @override
  Widget build(BuildContext context) {
    if (cards.isEmpty) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 36),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          _Heading(title),
          const SizedBox(height: 12),
          LayoutBuilder(
            builder: (context, box) {
              final cols = (box.maxWidth / 150).floor().clamp(1, 7);
              final cell = box.maxWidth / cols;
              return Row(
                children: [
                  for (final a in cards.take(cols))
                    SizedBox(
                      width: cell,
                      child: Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 8),
                        child: MusicCard(
                          controller: controller,
                          title: a.title,
                          subtitle: '',
                          artKind: 'album',
                          artKey: a.key,
                          direct: a.art,
                          onTap: () => onTap(a),
                        ),
                      ),
                    ),
                ],
              );
            },
          ),
        ],
      ),
    );
  }
}

/// The same row, over tracks rather than browse cards.
class _TrackRow2 extends StatelessWidget {
  const _TrackRow2({
    required this.title,
    required this.tracks,
    required this.controller,
    required this.source,
    this.onViewAll,
  });

  final String title;
  final List<Track> tracks;
  final MusicController controller;
  final String source;

  /// A way off the rail. The row shows one screen's worth of however many
  /// there are, and without this there is nothing saying the rest exist.
  final VoidCallback? onViewAll;

  @override
  Widget build(BuildContext context) => Padding(
        padding: const EdgeInsets.symmetric(horizontal: 36),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            _Heading(
              title,
              trailing: onViewAll == null ? null : _ViewAll(onTap: onViewAll!),
            ),
            const SizedBox(height: 12),
            LayoutBuilder(
              builder: (context, box) {
                final cols = (box.maxWidth / 150).floor().clamp(1, 7);
                final cell = box.maxWidth / cols;
                return Row(
                  children: [
                    for (var i = 0; i < tracks.length && i < cols; i++)
                      SizedBox(
                        width: cell,
                        child: Padding(
                          padding: const EdgeInsets.symmetric(horizontal: 8),
                          child: MusicCard(
                            controller: controller,
                            title: tracks[i].title,
                            subtitle: tracks[i].artist,
                            artKind: 'track',
                            artKey: '${tracks[i].itemId}',
                            direct: tracks[i].art,
                            fallback: Icons.music_note,
                            onTap: () => controller.playFrom(tracks, i, source),
                            onPlay: () =>
                                controller.playFrom(tracks, i, source),
                          ),
                        ),
                      ),
                  ],
                );
              },
            ),
          ],
        ),
      );
}


/// Albums left part-way through.
///
/// Its own bridge call rather than a field on the snapshot: the snapshot is
/// rebuilt on every tick, and walking every recently-played album's tracklist
/// eleven times a second to answer a question that changes when a record ends
/// would be the most expensive thing on the page.
/// How many abandoned records the shelf offers. Five across the page, which is
/// what fits at a readable card width.
const int _kResumeMax = 5;

class _ResumeRail extends StatefulWidget {
  const _ResumeRail({required this.controller, required this.revision});

  final MusicController controller;

  /// Something that changes when listening does. Cheap re-ask trigger: total
  /// listening time only moves when a track has actually played, which is also
  /// the only time a resume point can have moved.
  final String revision;

  @override
  State<_ResumeRail> createState() => _ResumeRailState();
}

class _ResumeRailState extends State<_ResumeRail> {
  late Future<List<ResumeCard>> _future = _load();

  Future<List<ResumeCard>> _load() async {
    try {
      return await musicResumeAlbums();
    } catch (_) {
      return const [];
    }
  }

  @override
  void didUpdateWidget(covariant _ResumeRail old) {
    super.didUpdateWidget(old);
    if (old.revision != widget.revision) _future = _load();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return FutureBuilder<List<ResumeCard>>(
      future: _future,
      builder: (context, snap) {
        final cards = snap.data ?? const <ResumeCard>[];
        // Nothing abandoned is the good case, and an empty "Pick up where you
        // stopped" heading is a reproach.
        if (cards.isEmpty) return const SizedBox.shrink();
        return Padding(
          padding: const EdgeInsets.fromLTRB(36, 22, 36, 0),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                'PICK UP WHERE YOU STOPPED',
                style: TextStyle(
                  fontFamily: Tokens.fontFamily,
                  fontSize: 11,
                  fontWeight: FontWeight.w700,
                  letterSpacing: 0.7,
                  color: t.nInk3,
                ),
              ),
              const SizedBox(height: 10),
              // Five, sharing the width equally, rather than a scroller of
              // whatever was abandoned. A shelf you have to drag sideways to
              // read is a shelf nobody reads, and past the fifth these stop
              // being "where you stopped" and become a list of everything you
              // ever put down.
              SizedBox(
                height: 78,
                child: Row(
                  children: [
                    for (var i = 0; i < cards.length && i < _kResumeMax; i++) ...[
                      if (i > 0) const SizedBox(width: 12),
                      Expanded(
                        child: _ResumeCardTile(
                          card: cards[i],
                          controller: widget.controller,
                        ),
                      ),
                    ],
                    // Fewer than five keep the same card width rather than
                    // stretching to fill the row: two enormous cards beside
                    // three tracks' worth of empty canvas reads as a bug.
                    for (var i = cards.length; i < _kResumeMax; i++) ...[
                      if (i > 0) const SizedBox(width: 12),
                      const Expanded(child: SizedBox.shrink()),
                    ],
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

class _ResumeCardTile extends StatelessWidget {
  const _ResumeCardTile({required this.card, required this.controller});

  final ResumeCard card;
  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
        color: t.nCard,
        borderRadius: BorderRadius.circular(14),
        child: InkWell(
          borderRadius: BorderRadius.circular(14),
          // Tapping resumes rather than opening the page: the card is an
          // offer to continue, and the album is one tap away from its title.
          onTap: () =>
              controller.send(MusicCmd.resumeAlbum(albumId: card.albumId)),
          child: Container(
            padding: const EdgeInsets.all(10),
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(14),
              border: Border.all(color: t.nHair),
            ),
            child: Row(
              children: [
                MusicArt(
                  controller: controller,
                  kind: 'album',
                  artKey: card.albumId.toString(),
                  direct: card.art.isEmpty ? null : card.art,
                  size: 46,
                  radius: 8,
                  fallback: Icons.album_outlined,
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      Text(
                        card.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          fontFamily: Tokens.fontFamily,
                          fontSize: 13,
                          fontWeight: FontWeight.w700,
                          color: t.nInk,
                        ),
                      ),
                      Text(
                        card.note,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11, color: t.nInk3),
                      ),
                      const SizedBox(height: 5),
                      ClipRRect(
                        borderRadius: BorderRadius.circular(2),
                        child: LinearProgressIndicator(
                          value: card.progress.clamp(0.0, 1.0),
                          minHeight: 3,
                          backgroundColor: t.nHair,
                          valueColor: const AlwaysStoppedAnimation<Color>(
                              Tokens.secMusic),
                        ),
                      ),
                    ],
                  ),
                ),
                Icon(Icons.play_arrow_rounded, size: 22, color: t.nInk2),
              ],
            ),
          ),
        ),
    );
  }
}

/// The listening strip: four separate 76px cards, not one panel of five
/// columns. The track count is not among them — that lives in the header's
/// count pill, and repeating it here cost the strip a quarter of its width.
class _StatsStrip extends StatelessWidget {
  const _StatsStrip({required this.stats, required this.controller});

  final Stats stats;
  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    // 84, not 76, and less padding. An 11px caption over a 20px figure is a
    // little over 53 points of line box in Sora, and 76 minus 28 of padding
    // left 48 — so all four cards overflowed by five pixels on every visit to
    // Home. Measured from the type rather than eyeballed: two line boxes plus
    // the four-pixel gap, plus ten of breathing room top and bottom.
    // Four figures are a teaser for the summary behind them: the same play
    // history answers "when do you listen", "who to", and "what did you start
    // eleven times and never finish". Tapping any card opens all of it.
    Widget card(String label, String value) => Expanded(
          child: Material(
            color: t.nCard,
            borderRadius: BorderRadius.circular(14),
            child: InkWell(
              borderRadius: BorderRadius.circular(14),
              onTap: () => listeningSummary(context, controller),
              child: Container(
                height: 84,
                padding:
                    const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(14),
                  border: Border.all(color: t.nHair),
                ),
                child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  label,
                  style: TextStyle(
                    fontFamily: Tokens.fontFamily,
                    fontSize: 11,
                    fontWeight: FontWeight.w700,
                    letterSpacing: 0.6,
                    color: t.nInk3,
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  value.isEmpty ? '—' : value,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontFamily: Tokens.fontFamily,
                    fontSize: 20,
                    fontWeight: FontWeight.w800,
                    color: t.nInk,
                  ),
                    ),
                  ],
                ),
              ),
            ),
          ),
        );
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 36),
      child: Row(
        children: [
          card('THIS WEEK', stats.week),
          const SizedBox(width: 14),
          card('ALL TIME', stats.total),
          const SizedBox(width: 14),
          card('TOP GENRE', stats.genre),
          const SizedBox(width: 14),
          card('DAY STREAK', stats.streak),
          const SizedBox(width: 14),
          // Library maintenance, next to the library's own figures. Its own
          // button because it is the one thing on this row that changes files
          // rather than reporting on them.
          Tooltip(
            message: 'Find duplicate recordings',
            child: Material(
              color: t.nCard,
              borderRadius: BorderRadius.circular(14),
              child: InkWell(
                borderRadius: BorderRadius.circular(14),
                onTap: () => duplicateFinder(context, controller),
                child: Container(
                  width: 56,
                  height: 84,
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(14),
                    border: Border.all(color: t.nHair),
                  ),
                  child: Icon(Icons.content_copy_outlined,
                      size: 20, color: t.nInk3),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// Songs — the edge-to-edge card grid, eight across and four deep.
///
/// Pinned to eight columns rather than driven off a target cell width: the page
/// is `SONGS_PAGE` = 32, and a grid whose column count moves with the window
/// would put a ragged last row under a page that is exactly four full ones.
/// Slint drives the cell off a live 100–300px density slider instead; that
/// control is not ported, and with it the column count would have to move too.
///
/// Nothing above the grid. Play all, Shuffle, the sort chips, the manager and
/// the pager all live in row 2 — see [_SubTabs] — because a third bar of
/// chrome over a page of tiles is a third of the fold spent on buttons.
class _Songs extends StatelessWidget {
  const _Songs({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final songs = st.songs;
    if (songs.isEmpty) return _emptySongs(controller, st);
    return ListView(
      padding: const EdgeInsets.fromLTRB(36, 6, 36, 24),
      children: [
        if (st.query.isNotEmpty) ...[
          const _SubHeading('Matching songs'),
          const SizedBox(height: 12),
        ],
        MusicGrid(
          count: songs.length,
          minCols: 8,
          maxCols: 8,
          builder: (context, i) => SongContextMenu(
            controller: controller,
            track: songs[i],
            onPlay: () => controller.playFrom(songs, i, 'songs'),
            onQueue: () => controller.queueAll([songs[i]]),
            child: MusicCard(
              controller: controller,
              title: songs[i].title,
              subtitle: songs[i].artist,
              artKind: 'track',
              artKey: '${songs[i].itemId}',
              direct: songs[i].art,
              fallback: Icons.music_note,
              // Under the cover and centred, as Slint draws it: on a square
              // tile the title and the artist are one caption for one picture,
              // and ranging them left makes each look like the start of a
              // column that is not there.
              centred: true,
              lyrics: songs[i].lyrics,
              loved: songs[i].loved,
              stars: songs[i].stars,
              onFav: () =>
                  controller.send(MusicCmd.love(itemId: songs[i].itemId)),
              onRate: (n) => controller
                  .send(MusicCmd.rate(itemId: songs[i].itemId, stars: n)),
              onTap: () => controller.playFrom(songs, i, 'songs'),
              onPlay: () => controller.playFrom(songs, i, 'songs'),
            ),
          ),
        ),
      ],
    );
  }
}

Widget _emptySongs(MusicController controller, MusicState st) => MusicEmpty(
      icon: Icons.music_note_outlined,
      title: st.query.isNotEmpty ? 'Nothing matches “${st.query}”' : 'No songs',
      body: st.query.isNotEmpty
          ? 'Try a shorter search, or clear it to see the whole library.'
          : 'Tracks show here once a watched folder has been scanned.',
      action: st.query.isNotEmpty
          ? (
              'Clear search',
              () => controller.send(const MusicCmd.search(query: ''))
            )
          : null,
    );

/// Favourites and History — a plain list of rows, with Play all / Shuffle
/// above it. Slint keeps Favourites' loved artists and albums above the songs;
/// the bridge has no field for those yet, so this is the songs half of it.
/// Loved and History.
///
/// History is one list. Loved is three, the way Slint builds it: the artists
/// you loved, then the albums, then the tracks — because a heart on an album
/// is stored on the album and is not the same claim as a heart on each of its
/// tracks. A page that only listed tracks could not show either of the first
/// two at all.
class _TrackPage extends StatelessWidget {
  const _TrackPage({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final loved = st.libTab == 'favorites';
    final songs = st.songs;
    final artists = loved ? st.railArtists : const <BrowseCard>[];
    final albums = loved ? st.railAlbums : const <BrowseCard>[];

    if (songs.isEmpty && artists.isEmpty && albums.isEmpty) {
      return MusicEmpty(
        icon: loved ? Icons.favorite_border : Icons.history,
        title: loved ? 'Nothing loved yet' : 'Nothing played yet',
        body: loved
            ? 'Tap the heart on a track, an album or an artist and it lands '
                'here.'
            : 'Everything you play is recorded, newest first.',
      );
    }

    return ListView(
      padding: const EdgeInsets.fromLTRB(36, 12, 36, 24),
      children: [
        if (artists.isNotEmpty) ...[
          const _SubHeading('Artists'),
          const SizedBox(height: 12),
          MusicGrid(
            count: artists.length,
            target: 150,
            maxCols: 9,
            labelHeight: 26,
            builder: (context, i) => MusicCard(
              controller: controller,
              title: artists[i].title,
              subtitle: artists[i].subtitle,
              artKind: 'artist',
              artKey: artists[i].key,
              direct: artists[i].art,
              fallback: Icons.person,
              // Square, like every other tile on the page. The circles are the
              // Home dashboard's shelf, where they are the only round thing and
              // read as faces; here they sit directly above a grid of square
              // albums and only made the two rows look misaligned.
              centred: true,
              loved: true,
              stars: artists[i].stars,
              onFav: () =>
                  controller.send(MusicCmd.artistFav(artistId: artists[i].id)),
              onRate: (n) => controller
                  .send(MusicCmd.artistRate(artistId: artists[i].id, stars: n)),
              onTap: () =>
                  controller.send(MusicCmd.openArtist(artistId: artists[i].id)),
            ),
          ),
          const SizedBox(height: 22),
        ],
        if (albums.isNotEmpty) ...[
          const _SubHeading('Albums'),
          const SizedBox(height: 12),
          MusicGrid(
            count: albums.length,
            minCols: 7,
            maxCols: 7,
            builder: (context, i) => MusicCard(
              controller: controller,
              title: albums[i].title,
              subtitle: albums[i].subtitle,
              artKind: 'album',
              artKey: albums[i].key,
              direct: albums[i].art,
              count: albums[i].count,
              loved: true,
              stars: albums[i].stars,
              onFav: () =>
                  controller.send(MusicCmd.albumFav(albumId: albums[i].id)),
              onRate: (n) => controller
                  .send(MusicCmd.albumRate(albumId: albums[i].id, stars: n)),
              onTap: () =>
                  controller.send(MusicCmd.openAlbum(albumId: albums[i].id)),
            ),
          ),
          const SizedBox(height: 22),
        ],
        if (songs.isNotEmpty) ...[
          if (loved && (artists.isNotEmpty || albums.isNotEmpty)) ...[
            const _SubHeading('Songs'),
            const SizedBox(height: 12),
          ],
          // Play all, Shuffle and Clear are on row 2 with the tabs — there is
          // no third bar in this section any more.
          for (var i = 0; i < songs.length; i++) ...[
            if (i > 0) const SizedBox(height: 2),
            TrackRow(
              controller: controller,
              track: songs[i],
              index: i + st.songPage * kListRows,
              onPlay: () => controller.playFrom(songs, i, st.libTab),
              onQueue: () => controller.queueAll([songs[i]]),
            ),
          ],
        ],
      ],
    );
  }
}

Future<void> _shuffleAll(
    MusicController c, List<Track> tracks, String source) async {
  final st = c.state;
  if (st != null && !st.shuffle) await c.send(const MusicCmd.toggleShuffle());
  await c.playFrom(tracks, 0, source);
}

/// An 18/700 heading — the one Slint uses inside a tab, a step below the
/// dashboard's 20.
class _SubHeading extends StatelessWidget {
  const _SubHeading(this.text);

  final String text;

  @override
  Widget build(BuildContext context) => Text(
        text,
        style: TextStyle(
          fontFamily: Tokens.fontFamily,
          fontSize: 18,
          fontWeight: FontWeight.w700,
          color: context.tokens.nInk,
        ),
      );
}

/// Albums, Artists and Genres. One grid, three sets of cards — and the tile
/// carries the heart, the stars and the track count, which is most of what
/// tells the three apart at a glance.
class _Browse extends StatelessWidget {
  const _Browse({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    if (st.cards.isEmpty) {
      return MusicEmpty(
        icon: Icons.grid_view_outlined,
        title: switch (st.libTab) {
          'artists' => 'No artists tagged yet',
          'genres' => 'No genres tagged yet',
          _ => 'No albums tagged yet',
        },
        body: 'This grid fills in once tracks have been tagged.',
      );
    }
    return ListView(
      padding: const EdgeInsets.fromLTRB(36, 12, 36, 24),
      children: [
        // Seven across and four down — twenty-eight, which is what the bridge
        // pages by. The sort and the pager that used to sit above this grid in
        // a row of their own are on row 2 now, with the tabs.
        MusicGrid(
          count: st.cards.length,
          minCols: 7,
          maxCols: 7,
          builder: (context, i) =>
              StaggerIn(index: i, child: _card(st.cards[i])),
        ),
      ],
    );
  }

  /// Every card, whatever the tab, carries the same right-click menu.
  ///
  /// It used to wrap the genre case only, so the same gesture set a cover on
  /// one tab and did nothing on the next -- which reads as broken rather than
  /// as unimplemented. `set_card_art` already accepts album and artist (it
  /// writes `albums.cover_path` and `artists.image_path`), so the menu works on
  /// all three without any bridge change.
  Widget _card(BrowseCard c) => CardContextMenu(
        controller: controller,
        kind: switch (st.libTab) {
          'artists' => 'artist',
          'genres' => 'genre',
          _ => 'album',
        },
        cardKey: c.key,
        hasArt: c.art.isNotEmpty,
        child: switch (st.libTab) {
          'artists' => MusicCard(
              controller: controller,
              title: c.title,
              subtitle: c.subtitle,
              artKind: 'artist',
              artKey: c.key,
              direct: c.art,
              fallback: Icons.person,
              count: c.count,
              loved: c.loved,
              stars: c.stars,
              onFav: () => controller.send(MusicCmd.artistFav(artistId: c.id)),
              onRate: (n) => controller
                  .send(MusicCmd.artistRate(artistId: c.id, stars: n)),
              onTap: () => controller.send(MusicCmd.openArtist(artistId: c.id)),
            ),
          'genres' => MusicCard(
              controller: controller,
              title: c.title,
              subtitle: c.subtitle,
              artKind: 'genre',
              artKey: c.key,
              direct: c.art,
              fallback: Icons.library_music_outlined,
              count: c.count,
              onTap: () => controller.send(MusicCmd.openGenre(name: c.key)),
            ),
          _ => MusicCard(
              controller: controller,
              title: c.title,
              subtitle: c.subtitle,
              artKind: 'album',
              artKey: c.key,
              direct: c.art,
              count: c.count,
              loved: c.loved,
              stars: c.stars,
              onFav: () => controller.send(MusicCmd.albumFav(albumId: c.id)),
              onRate: (n) =>
                  controller.send(MusicCmd.albumRate(albumId: c.id, stars: n)),
              onTap: () => controller.send(MusicCmd.openAlbum(albumId: c.id)),
            ),
        },
      );
}

/// Playlists — the action row Slint puts above the grid, then gradient tiles.
class _Playlists extends StatelessWidget {
  const _Playlists({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.fromLTRB(36, 6, 36, 24),
      children: [
        Wrap(
          spacing: 10,
          runSpacing: 10,
          children: [
            BrowseChip(
              label: '+ New',
              onTap: () => createPlaylist(context, controller),
            ),
            BrowseChip(
              icon: Icons.auto_awesome,
              label: 'Loved',
              onTap: () => controller
                  .send(const MusicCmd.playlistCreateSmart(kind: 'loved')),
            ),
            BrowseChip(
              icon: Icons.auto_awesome,
              label: 'Recent',
              onTap: () => controller
                  .send(const MusicCmd.playlistCreateSmart(kind: 'recent')),
            ),
          ],
        ),
        const SizedBox(height: 16),
        if (st.cards.isEmpty)
          Text(
            'No playlists yet — create one, or add a smart playlist.',
            style: TextStyle(fontSize: 13, color: t.nInk2),
          )
        else
          MusicGrid(
            count: st.cards.length,
            target: 170,
            maxCols: 6,
            labelHeight: 34,
            builder: (context, i) => CardContextMenu(
              controller: controller,
              kind: 'playlist',
              cardKey: st.cards[i].key,
              hasArt: st.cards[i].art.isNotEmpty,
              deleteLabel: 'Delete playlist',
              onDelete: () => _deletePlaylist(context, controller, st.cards[i]),
              child: TileCard(
                controller: controller,
                label: st.cards[i].title,
                icon: Icons.queue_music,
                artKind: 'playlist',
                artKey: st.cards[i].key,
                art: st.cards[i].art,
                onTap: () => controller
                    .send(MusicCmd.openPlaylist(playlistId: st.cards[i].id)),
                onMenu: () => _deletePlaylist(context, controller, st.cards[i]),
              ),
            ),
          ),
      ],
    );
  }
}

class _Folders extends StatelessWidget {
  const _Folders({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    if (st.cards.isEmpty) {
      return MusicEmpty(
        icon: Icons.folder_outlined,
        title: 'No folders yet',
        body: 'Add a music folder and Tulipix will watch it, read the tags '
            'inside it, and group what it finds.',
        action: ('Add a folder', controller.addFolder),
      );
    }
    // A tree beside the grid. The grid is every folder that holds tracks, flat
    // and searchable; the tree is the shape of the disk, which is the one thing
    // a flat list of folders cannot show. Under 1100px there is no room for
    // both and the grid keeps the page.
    return LayoutBuilder(
      builder: (context, box) => box.maxWidth < 1100
          ? _folderGrid(context)
          : Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                SizedBox(
                  width: 320,
                  child: _FolderTree(controller: controller),
                ),
                Expanded(child: _folderGrid(context)),
              ],
            ),
    );
  }

  Widget _folderGrid(BuildContext context) {
    // Just the grid, seven across and three down — a folder tile carries a
    // path under its name and is taller than an album's, so 7x4 put the last
    // row under the fold. There was a row of four chips and, under it, a list
    // of every watched root with its path and its track count, which is the
    // same set of folders the grid was already drawing, printed twice.
    return ListView(
      padding: const EdgeInsets.fromLTRB(36, 12, 36, 24),
      children: [
        MusicGrid(
          count: st.cards.length,
          minCols: 7,
          maxCols: 7,
          labelHeight: 34,
          builder: (context, i) => TileCard(
            label: st.cards[i].title,
            icon: Icons.folder,
            strong: false,
            hint: 'Open  ·  right-click for options',
            onTap: () =>
                controller.send(MusicCmd.openFolder(path: st.cards[i].key)),
            onMenu: () => _folderMenu(
              context,
              controller,
              st.cards[i],
              watched: st.roots.any((r) => r.key == st.cards[i].key),
            ),
            footer: _CountChip(count: st.cards[i].count),
          ),
        ),
      ],
    );
  }
}


/// The disk, as a tree you can walk into.
///
/// One level per fetch: a library on a slow disk should not pay for branches
/// nobody opened. Expanded paths are held here rather than on the snapshot,
/// because which folders you have open is a property of looking at the page,
/// not of the library.
class _FolderTree extends StatefulWidget {
  const _FolderTree({required this.controller});

  final MusicController controller;

  @override
  State<_FolderTree> createState() => _FolderTreeState();
}

class _FolderTreeState extends State<_FolderTree> {
  /// Children by parent path; "" is the root. A key present with an empty list
  /// is a folder that was opened and had nothing under it.
  final Map<String, List<FolderNode>> _kids = {};
  final Set<String> _open = {};
  final Set<String> _loading = {};

  @override
  void initState() {
    super.initState();
    unawaited(_load(''));
  }

  Future<void> _load(String under) async {
    if (_kids.containsKey(under) || _loading.contains(under)) return;
    _loading.add(under);
    List<FolderNode> got;
    try {
      got = await musicFolderChildren(under: under);
    } catch (_) {
      got = const [];
    }
    if (!mounted) return;
    setState(() {
      _kids[under] = got;
      _loading.remove(under);
    });
  }

  void _toggle(FolderNode node) {
    setState(() {
      if (!_open.remove(node.path)) _open.add(node.path);
    });
    if (_open.contains(node.path)) unawaited(_load(node.path));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final roots = _kids[''];
    return Padding(
      padding: const EdgeInsets.fromLTRB(24, 12, 6, 24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'ON DISK',
            style: TextStyle(
              fontFamily: Tokens.fontFamily,
              fontSize: 11,
              fontWeight: FontWeight.w700,
              letterSpacing: 0.7,
              color: t.nInk3,
            ),
          ),
          const SizedBox(height: 8),
          Expanded(
            child: roots == null
                ? const Center(child: CircularProgressIndicator())
                : ListView(
                    padding: EdgeInsets.zero,
                    children: _rows(roots, 0),
                  ),
          ),
        ],
      ),
    );
  }

  /// Flattened, because a ListView of nested Columns loses its scrolling and a
  /// deep tree is exactly where that starts to matter.
  List<Widget> _rows(List<FolderNode> nodes, int depth) {
    final out = <Widget>[];
    for (final node in nodes) {
      out.add(_FolderRow(
        controller: widget.controller,
        node: node,
        depth: depth,
        open: _open.contains(node.path),
        onToggle: () => _toggle(node),
      ));
      if (_open.contains(node.path)) {
        final kids = _kids[node.path];
        if (kids == null) {
          out.add(Padding(
            padding: EdgeInsets.only(left: 18.0 * (depth + 1) + 8, top: 2),
            child: const SizedBox(
              width: 12,
              height: 12,
              child: CircularProgressIndicator(strokeWidth: 1.5),
            ),
          ));
        } else {
          out.addAll(_rows(kids, depth + 1));
        }
      }
    }
    return out;
  }
}

class _FolderRow extends StatelessWidget {
  const _FolderRow({
    required this.controller,
    required this.node,
    required this.depth,
    required this.open,
    required this.onToggle,
  });

  final MusicController controller;
  final FolderNode node;
  final int depth;
  final bool open;
  final VoidCallback onToggle;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      borderRadius: BorderRadius.circular(6),
      // Tapping the row opens the folder's page; the arrow expands it. Two
      // targets because they are two questions — what is in here, and what is
      // under here.
      onTap: node.direct > 0
          ? () => controller.send(MusicCmd.openFolder(path: node.path))
          : onToggle,
      child: Padding(
        padding: EdgeInsets.fromLTRB(18.0 * depth, 3, 4, 3),
        child: Row(
          children: [
            SizedBox(
              width: 18,
              child: node.hasChildren
                  ? InkWell(
                      onTap: onToggle,
                      child: Icon(
                        open
                            ? Icons.keyboard_arrow_down
                            : Icons.keyboard_arrow_right,
                        size: 16,
                        color: t.nInk3,
                      ),
                    )
                  : null,
            ),
            Icon(open ? Icons.folder_open : Icons.folder,
                size: 15, color: t.nInk3),
            const SizedBox(width: 6),
            Expanded(
              child: Text(
                node.name,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12.5, color: t.nInk),
              ),
            ),
            Text(
              '${node.total}',
              style: TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: 11,
                color: t.nInk3,
              ),
            ),
            // The whole discography directory to the queue in one action,
            // rather than one album at a time.
            IconButton(
              iconSize: 15,
              visualDensity: VisualDensity.compact,
              tooltip: 'Play this folder and everything below it',
              icon: const Icon(Icons.playlist_play),
              onPressed: () =>
                  controller.send(MusicCmd.playFolderTree(path: node.path)),
            ),
          ],
        ),
      ),
    );
  }
}

/// The pink pill along the foot of a folder tile. Slint puts the folder's
/// section assignment here; the port shows how much is in it, which is the
/// number the bridge has.
class _CountChip extends StatelessWidget {
  const _CountChip({required this.count});

  final int count;

  @override
  Widget build(BuildContext context) => Container(
        height: 20,
        padding: const EdgeInsets.symmetric(horizontal: 10),
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: Tokens.secMusic,
          borderRadius: BorderRadius.circular(10),
          boxShadow: const [
            BoxShadow(color: Color(0x66000000), blurRadius: 8),
          ],
        ),
        child: Text(
          '$count',
          style: const TextStyle(
            fontFamily: Tokens.fontFamily,
            fontSize: 11,
            fontWeight: FontWeight.w700,
            color: Colors.white,
          ),
        ),
      );
}

Future<void> _folderMenu(
  BuildContext context,
  MusicController c,
  BrowseCard f, {
  bool watched = false,
}) async {
  final box = context.findRenderObject() as RenderBox?;
  final at = box == null
      ? Offset.zero
      : box.localToGlobal(box.size.center(Offset.zero));
  final choice = await showMenu<String>(
    context: context,
    position: RelativeRect.fromLTRB(at.dx, at.dy, at.dx, at.dy),
    items: [
      const PopupMenuItem(value: 'open', child: Text('Open')),
      // Flagging is a column update, not a move: unflagging puts the tracks
      // straight back into My Music.
      const PopupMenuItem(value: 'book', child: Text('Mark as an audiobook')),
      const PopupMenuItem(value: 'music', child: Text('Back to My Music')),
      // The watched-roots list used to carry this; the list is gone and the
      // folder is here.
      if (watched) const PopupMenuDivider(),
      if (watched)
        const PopupMenuItem(value: 'unwatch', child: Text('Stop watching')),
    ],
  );
  if (choice == null || !context.mounted) return;
  switch (choice) {
    case 'open':
      await c.send(MusicCmd.openFolder(path: f.key));
    case 'book':
      await c.send(MusicCmd.bookFlagFolder(folder: f.key, on_: true));
    case 'music':
      await c.send(MusicCmd.bookFlagFolder(folder: f.key, on_: false));
    case 'unwatch':
      await confirmThen(
        context,
        c,
        title: 'Stop watching this folder?',
        body: '“${f.title}” is dropped from the library on the next scan. '
            'The files are left exactly where they are.',
        action: 'Stop watching',
        cmd: MusicCmd.removeRoot(path: f.key),
      );
  }
}

Future<void> _deletePlaylist(
        BuildContext context, MusicController c, BrowseCard p) =>
    confirmThen(
      context,
      c,
      title: 'Delete this playlist?',
      body: '“${p.title}” and its order go. The tracks in it stay in the '
          'library.',
      action: 'Delete playlist',
      cmd: MusicCmd.playlistDelete(playlistId: p.id),
    );
