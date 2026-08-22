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
import 'music_widgets.dart';

class RadioTab extends StatefulWidget {
  const RadioTab({super.key, required this.controller});

  final MusicController controller;

  @override
  State<RadioTab> createState() => _RadioTabState();
}

class _RadioTabState extends State<RadioTab> {
  final _search = TextEditingController();

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final c = widget.controller;
    final st = c.state;
    if (st == null) return const Center(child: CircularProgressIndicator());
    final t = context.tokens;

    return Column(
      children: [
        SizedBox(
          height: 52,
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
                    active: !st.radioCatOpen && st.radioTab == tab.$1,
                    tint: const Color(0xFF06B6D4),
                    tint2: const Color(0xFF3B82F6),
                    onTap: () => c.send(MusicCmd.radioSetTab(name: tab.$1)),
                  ),
                ),
              const SizedBox(width: 16),
              SizedBox(
                width: 260,
                child: TextField(
                  controller: _search,
                  decoration: const InputDecoration(
                    isDense: true,
                    prefixIcon: Icon(Icons.search, size: 18),
                    hintText: 'Search stations',
                    border: OutlineInputBorder(),
                  ),
                  onSubmitted: (q) => c.send(MusicCmd.radioSearch(query: q)),
                ),
              ),
              const Spacer(),
              Text('${st.radioTotal} cached',
                  style: TextStyle(fontSize: 12, color: t.nInk2)),
              TextButton.icon(
                icon: const Icon(Icons.refresh, size: 16),
                label: const Text('Refresh all'),
                onPressed: () => c.send(const MusicCmd.radioRefresh()),
              ),
              FilledButton.icon(
                icon: const Icon(Icons.add, size: 16),
                label: const Text('Add station'),
                onPressed: () => _addStation(context, c),
              ),
              const SizedBox(width: 12),
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
      return CardGrid(
        min: 160,
        children: [
          for (var i = 0; i < st.radioCategories.length; i++)
            _CategoryTile(
              category: st.radioCategories[i],
              onTap: () => c.send(MusicCmd.radioSetGenre(index: i)),
            ),
        ],
      );
    }
    return _StationList(
      controller: c,
      st: st,
      title: st.radioTab == 'favourites' ? 'Favourites' : 'Recently played',
    );
  }
}

class _CategoryTile extends StatelessWidget {
  const _CategoryTile({required this.category, required this.onTap});

  final RadioCategory category;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: onTap,
      borderRadius: BorderRadius.circular(Tokens.radiusMd),
      child: Container(
        decoration: BoxDecoration(
          color: t.nCard,
          borderRadius: BorderRadius.circular(Tokens.radiusMd),
          border: Border.all(color: t.nHair),
        ),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Text(category.icon, style: const TextStyle(fontSize: 32)),
            const SizedBox(height: 8),
            Text(category.label,
                textAlign: TextAlign.center,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 13, fontWeight: FontWeight.w600, color: t.nInk)),
            Text(
              // Zero means "never fetched", not "no stations" — Refresh is
              // what fills the cache, and saying so is more useful than "0".
              category.count > 0 ? '${category.count} stations' : 'not cached',
              style: TextStyle(fontSize: 11, color: t.nInk2),
            ),
          ],
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
                onPressed: () =>
                    controller.send(const MusicCmd.radioClearRecent()),
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
        kind: 'yt',
        artKey: s.favicon,
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

Future<void> _addStation(BuildContext context, MusicController c) async {
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
  if (ok ?? false) {
    await c
        .send(MusicCmd.radioAdd(name: name.text.trim(), url: url.text.trim()));
  }
}
