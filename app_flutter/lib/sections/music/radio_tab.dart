// Internet radio — curated categories, favourites, recents, and search.
//
// Two things make this tab different from the other four. Its content is not
// in the library: a station is a URL, and the cache in radio.db is what makes
// a category page open without the network. And a stream has no end, so the
// player bar drops its scrubber and shows the ICY title instead — which is the
// only place the actual song is readable, because the station name is ours.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';
import 'music_widgets.dart';

class RadioTab extends StatefulWidget {
  const RadioTab({super.key, required this.controller});

  final MusicController controller;

  @override
  State<RadioTab> createState() => _RadioTabState();
}

class _RadioTabState extends State<RadioTab> {
  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final st = c.state;
    if (st == null) return const Center(child: CircularProgressIndicator());

    return Column(
      children: [
        SizedBox(
          // 57, matching My Music's row 2 -- the two sections' second bars sit
          // at the same height whichever tab is open.
          height: 57,
          child: Row(
            children: [
              const SizedBox(width: 20),
              for (final tab in const [
                ('home', 'Browse'),
                ('favourites', 'Favourites'),
                ('recent', 'Recent'),
              ])
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 4),
                  child: MusicChip(
                    label: tab.$2,
                    icon: switch (tab.$1) {
                      'favourites' => Icons.favorite,
                      'recent' => Icons.history,
                      _ => Icons.travel_explore,
                    },
                    active: !st.radioCatOpen && st.radioTab == tab.$1,
                    tint: const Color(0xFF06B6D4),
                    tint2: const Color(0xFF3B82F6),
                    minWidth: 118,
                    onTap: () => c.send(MusicCmd.radioSetTab(name: tab.$1)),
                  ),
                ),
              const Spacer(),
              // Nothing else. The search box was a second one -- the header's
              // pill searches stations while this tab is open -- and Add
              // station is the header's `+ Add`, which is where every other
              // tab's is. "N cached" is the header's count pill.
              _RefreshPill(controller: c),
              const SizedBox(width: 20),
            ],
          ),
        ),
        Expanded(child: _body(context, c, st)),
      ],
    );
  }

  Widget _body(BuildContext context, MusicController c, MusicState st) {
    if (st.radioCatOpen) {
      return _StationList(controller: c, st: st, title: st.radioCatTitle);
    }
    if (st.radioTab == 'home') {
      // Five columns, pinned -- `RadioCatGrid` in ui/page_music.slint hardcodes
      // `cols: 5` and squares the cell (`ch: cw`), so the fifteen presets are
      // always a 5x3 block. A max-extent grid reflowed to eleven skinny columns
      // on a wide window and three on a narrow one, which is why the port never
      // looked like the same page.
      return SingleChildScrollView(
        padding: const EdgeInsets.fromLTRB(16, 8, 16, 24),
        child: MusicGrid(
          count: st.radioCategories.length,
          minCols: 5,
          maxCols: 5,
          // The label lives inside the tile here, not under it.
          labelHeight: 0,
          builder: (_, i) => _CategoryTile(
            category: st.radioCategories[i],
            index: i,
            onTap: () => c.send(MusicCmd.radioSetGenre(index: i)),
          ),
        ),
      );
    }
    return _StationList(
      controller: c,
      st: st,
      title: st.radioTab == 'favourites' ? 'Favourites' : 'Recently played',
    );
  }
}

/// Refresh all, as a pill that becomes its own progress bar.
///
/// The refresh walks fourteen curated queries against radio-browser and takes
/// the better part of a minute on a cold cache; the port fired it off a text
/// button and gave no sign it was running, so it looked broken and got clicked
/// again. `ScanProgress` already carries the category and the count -- the
/// scan strip at the top of the section reads the same events -- so the fill
/// is the real position, not an indeterminate spinner.
class _RefreshPill extends StatelessWidget {
  const _RefreshPill({required this.controller});

  static const Color _tint = Color(0xFF06B6D4);

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final p = controller.progress;
    final running = p != null && p.total > 0;
    final frac = running ? (p.done / p.total).clamp(0.0, 1.0) : 0.0;
    return SizedBox(
      height: 34,
      width: 188,
      child: Material(
        color: Colors.transparent,
        borderRadius: BorderRadius.circular(17),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: running
              ? null
              : () => controller.send(const MusicCmd.radioRefresh()),
          child: DecoratedBox(
            decoration: BoxDecoration(
              color:
                  running ? Colors.transparent : _tint.withValues(alpha: 0.18),
              borderRadius: BorderRadius.circular(17),
              border: Border.all(color: _tint, width: 1.5),
            ),
            child: Stack(
              fit: StackFit.expand,
              children: [
                // The fill. An `Align` with a factor rather than a
                // LinearProgressIndicator so it keeps the pill's own radius and
                // the label stays legible over it.
                Align(
                  alignment: Alignment.centerLeft,
                  child: FractionallySizedBox(
                    widthFactor: frac,
                    child: ColoredBox(color: _tint.withValues(alpha: 0.55)),
                  ),
                ),
                Row(
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    Icon(running ? Icons.downloading : Icons.refresh,
                        size: 16, color: running ? Colors.white : _tint),
                    const SizedBox(width: 7),
                    Flexible(
                      child: Text(
                        running
                            ? '${p.done} / ${p.total}  ${p.label}'
                            : 'Refresh all',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          fontFamily: Tokens.fontFamily,
                          fontSize: 12,
                          fontWeight: FontWeight.w700,
                          color: running ? Colors.white : t.nInk,
                        ),
                      ),
                    ),
                  ],
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// One preset tile in the 5x3 browse block.
///
/// `RadioCatGrid` gives every tile one of six gradients by `i % 6` and spends
/// it twice: a wash on hover, and the ring around the station count. Without
/// the index the port had fifteen identical grey cards.
class _CategoryTile extends StatefulWidget {
  const _CategoryTile({
    required this.category,
    required this.index,
    required this.onTap,
  });

  static const List<List<Color>> _grads = [
    [Color(0xFF14B8A6), Color(0xFF0EA5E9)],
    [Color(0xFFF59E0B), Color(0xFFEF4444)],
    [Color(0xFF8B5CF6), Color(0xFFEC4899)],
    [Color(0xFF22C55E), Color(0xFF14B8A6)],
    [Color(0xFF3B82F6), Color(0xFF8B5CF6)],
    [Color(0xFFEC4899), Color(0xFFF59E0B)],
  ];

  final RadioCategory category;
  final int index;
  final VoidCallback onTap;

  @override
  State<_CategoryTile> createState() => _CategoryTileState();
}

class _CategoryTileState extends State<_CategoryTile> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final g = _CategoryTile._grads[widget.index % 6];
    final grad = LinearGradient(
      colors: g,
      begin: Alignment.topLeft,
      end: Alignment.bottomRight,
    );
    return Padding(
      padding: const EdgeInsets.all(6),
      child: MouseRegion(
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: AnimatedContainer(
          duration: t.reduceMotion
              ? Duration.zero
              : const Duration(milliseconds: 140),
          // The section's hover glow, in this category's own first colour:
          // six gradients, six glows.
          decoration: cardGlowBox(color: g.first, on: _hover, radius: 16)
              .copyWith(
            color: t.nCard,
            border: _hover ? null : Border.all(color: t.nHair),
          ),
          clipBehavior: Clip.antiAlias,
          child: Stack(
            fit: StackFit.expand,
            children: [
              AnimatedOpacity(
                duration: t.reduceMotion
                    ? Duration.zero
                    : const Duration(milliseconds: 140),
                opacity: _hover ? 0.16 : 0,
                child: DecoratedBox(decoration: BoxDecoration(gradient: grad)),
              ),
              Material(
                color: Colors.transparent,
                child: InkWell(
                  onTap: widget.onTap,
                  child: Padding(
                    padding: const EdgeInsets.all(14),
                    child: Column(
                      mainAxisAlignment: MainAxisAlignment.center,
                      children: [
                        Text(widget.category.icon,
                            style: const TextStyle(fontSize: 42)),
                        const SizedBox(height: 10),
                        Text(
                          widget.category.label,
                          textAlign: TextAlign.center,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontFamily: Tokens.fontFamily,
                              fontSize: 21,
                              fontWeight: FontWeight.w800,
                              height: 1.1,
                              color: t.nInk),
                        ),
                        const SizedBox(height: 10),
                        _CountPill(
                          // Zero means "never fetched", not "no stations" --
                          // Refresh is what fills the cache.
                          text: widget.category.count > 0
                              ? '${widget.category.count} stations'
                              : 'tap to load',
                          gradient: grad,
                        ),
                      ],
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// The gradient-ringed count pill. Light theme fills it and goes white-on-
/// gradient; dark keeps the card colour inside the ring, as in the Slint one.
class _CountPill extends StatelessWidget {
  const _CountPill({required this.text, required this.gradient});

  final String text;
  final Gradient gradient;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 30,
      padding: const EdgeInsets.all(1.5),
      decoration: BoxDecoration(
          gradient: gradient, borderRadius: BorderRadius.circular(15)),
      child: DecoratedBox(
        decoration: BoxDecoration(
          color: t.dark ? t.nCard : Colors.transparent,
          borderRadius: BorderRadius.circular(13.5),
        ),
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 14),
          // A min-size Row, not a Center: Center fills the width it is
          // offered, which stretched the pill across the whole tile.
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Flexible(
                child: Text(
                  text,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontFamily: Tokens.fontFamily,
                    fontSize: 12,
                    fontWeight: FontWeight.w700,
                    color: t.dark ? t.nInk2 : Colors.white,
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _StationList extends StatelessWidget {
  const _StationList({
    required this.controller,
    required this.st,
    required this.title,
  });

  final MusicController controller;
  final MusicState st;
  final String title;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    if (st.radioStations.isEmpty) {
      return MusicEmpty(
        icon: Icons.radio_outlined,
        title: 'No stations here',
        body: st.radioTab == 'favourites'
            ? 'Heart a station and it stays here, playable offline the moment '
                'the network is back.'
            : 'Refresh pulls every curated category from radio-browser into '
                'the local cache.',
        action: (
          'Refresh all',
          () => controller.send(const MusicCmd.radioRefresh())
        ),
      );
    }
    return Column(
      children: [
        Row(
          children: [
            const SizedBox(width: 24),
            if (st.radioCatOpen)
              IconButton(
                icon: const Icon(Icons.arrow_back, size: 18),
                tooltip: 'Back to categories',
                onPressed: () => controller.send(const MusicCmd.radioBack()),
              ),
            Text(title,
                style: TextStyle(
                    fontSize: 15, fontWeight: FontWeight.w700, color: t.nInk)),
            const Spacer(),
            for (final mode in const [
              ('default', 'Default'),
              ('name', 'Name'),
              ('bitrate', 'Bitrate'),
            ])
              Padding(
                padding: const EdgeInsets.only(right: 6),
                child: MusicChip(
                  label: mode.$2,
                  active: st.radioSort == mode.$1,
                  tint: const Color(0xFF06B6D4),
                  tint2: const Color(0xFF3B82F6),
                  onTap: () =>
                      controller.send(MusicCmd.radioSetSort(mode: mode.$1)),
                ),
              ),
            if (st.radioTab == 'recent')
              TextButton(
                onPressed: () => confirmThen(
                  context,
                  controller,
                  title: 'Clear recently played stations?',
                  body: 'The list of what you have tuned into is forgotten. '
                      'Your favourites are untouched.',
                  action: 'Clear',
                  cmd: const MusicCmd.radioClearRecent(),
                ),
                child: const Text('Clear'),
              ),
            const SizedBox(width: 16),
          ],
        ),
        Expanded(
          child: ListView.builder(
            itemCount: st.radioStations.length,
            itemBuilder: (_, i) => _StationRow(
              controller: controller,
              station: st.radioStations[i],
              index: i,
            ),
          ),
        ),
        Pager(
          page: st.radioPage,
          pages: st.radioPages,
          onGo: (p) => controller.send(MusicCmd.radioSetPage(page: p)),
        ),
      ],
    );
  }
}

class _StationRow extends StatelessWidget {
  const _StationRow({
    required this.controller,
    required this.station,
    required this.index,
  });

  final MusicController controller;
  final Station station;
  final int index;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = station;
    final playing =
        controller.now?.mode == 'radio' && controller.now?.key == s.uuid;
    return ListTile(
      leading: MusicArt(
        controller: controller,
        // The bridge checks the favicon and caches it, or draws a tile from
        // the name when there is none that decodes.
        kind: 'radio',
        artKey: '${s.name}\n${s.favicon}',
        size: 40,
        radius: 8,
        fallback: Icons.radio,
      ),
      title: Text(
        s.name,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: TextStyle(
          fontSize: 13,
          fontWeight: playing ? FontWeight.w700 : FontWeight.w500,
          color: playing ? const Color(0xFF06B6D4) : t.nInk,
        ),
      ),
      subtitle: Text(
        [
          if (s.country.isNotEmpty) s.country,
          if (s.tags.isNotEmpty) s.tags,
          if (s.bitrate > 0) '${s.bitrate} kbps',
          if (s.codec.isNotEmpty) s.codec,
        ].join(' · '),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: TextStyle(fontSize: 11, color: t.nInk2),
      ),
      trailing: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (playing)
            const Padding(
              padding: EdgeInsets.only(right: 8),
              child: Icon(Icons.graphic_eq, size: 16, color: Color(0xFF06B6D4)),
            ),
          IconButton(
            iconSize: 18,
            tooltip: s.favourite ? 'Remove from favourites' : 'Favourite',
            icon: Icon(
              s.favourite ? Icons.favorite : Icons.favorite_border,
              color: s.favourite ? const Color(0xFF06B6D4) : t.nInk2,
            ),
            onPressed: () => controller.send(MusicCmd.radioFav(index: index)),
          ),
        ],
      ),
      onTap: () => controller.send(MusicCmd.radioPlay(index: index)),
    );
  }
}

/// Save a stream URL as a favourite station. Reached from the header's `+ Add`
/// while Radio is open -- the tab's own row has no room for it and every other
/// tab's add is in the same place.
Future<void> addStation(BuildContext context, MusicController c) async {
  final name = TextEditingController();
  final url = TextEditingController();
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Add a station'),
      content: SizedBox(
        width: 420,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              controller: name,
              autofocus: true,
              decoration: const InputDecoration(labelText: 'Name'),
            ),
            const SizedBox(height: 12),
            TextField(
              controller: url,
              decoration: const InputDecoration(
                labelText: 'Stream URL',
                hintText: 'https://…',
              ),
            ),
          ],
        ),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Cancel')),
        FilledButton(
          onPressed: () => Navigator.pop(ctx, true),
          child: const Text('Save to favourites'),
        ),
      ],
    ),
  );
  name.dispose();
  url.dispose();
  if (ok ?? false) {
    await c
        .send(MusicCmd.radioAdd(name: name.text.trim(), url: url.text.trim()));
  }
}
