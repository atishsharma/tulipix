// The Arcade section — docs/mockups/NewSections/arcade-deck.html.
//
// Library (a shelf of covers, filters, the last game to go back to, a game
// open), Stats, Sources, and couch mode: the shelf full screen, big tiles, the
// arrow keys and Enter.

import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/first_load.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/section_tabs.dart';
import '../../src/rust/api/arcade.dart';
import '../../src/rust/api/dialog.dart';
import '../kitchen/kitchen_page.dart' show Grid, Quiet, Strip, askLine, cardDeco;
import '../papers/papers_page.dart' show TabPill;
import 'arcade_controller.dart';

class ArcadePage extends StatefulWidget {
  const ArcadePage({super.key, required this.visible});

  final bool visible;

  @override
  State<ArcadePage> createState() => _ArcadePageState();
}

class _ArcadePageState extends State<ArcadePage> {
  final ArcadeController _c = ArcadeController();

  @override
  void initState() {
    super.initState();
    _c.refresh();
  }

  @override
  void didUpdateWidget(ArcadePage old) {
    super.didUpdateWidget(old);
    if (widget.visible && !old.visible) _c.refresh(quiet: true);
  }

  @override
  void dispose() {
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
        if (st != null && _c.couch) return _Couch(c: _c, st: st);
        return ColoredBox(
          color: t.nCanvas,
          child: Column(
            children: [
              _Header(c: _c, st: st),
              if (_c.busy)
                const LinearProgressIndicator(minHeight: 2, color: kArcade)
              else
                const SizedBox(height: 2),
              if (_c.notice.isNotEmpty)
                Strip(
                  icon: Icons.check_circle_outline,
                  tint: kArcade,
                  text: _c.notice,
                  onClose: _c.dismissNotice,
                ),
              if (_c.error != null && st != null)
                Strip(
                  icon: Icons.error_outline,
                  tint: Tokens.error,
                  text: plainError(_c.error!),
                  onClose: _c.clearError,
                ),
              Expanded(
                child: st == null
                    ? FirstLoad(error: _c.error, onRetry: _c.refresh)
                    : switch (st.tab) {
                        'stats' => _Stats(c: _c, st: st),
                        'sources' => _Sources(c: _c, st: st),
                        _ => st.open != null
                            ? _GamePage(c: _c, g: st.open!)
                            : _Library(c: _c, st: st),
                      },
              ),
            ],
          ),
        );
      },
    );
  }
}

class _Header extends StatefulWidget {
  const _Header({required this.c, required this.st});

  final ArcadeController c;
  final ArcadeState? st;

  @override
  State<_Header> createState() => _HeaderState();
}

class _HeaderState extends State<_Header> {
  final TextEditingController _q = TextEditingController();

  @override
  void dispose() {
    _q.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = widget.c;
    final st = widget.st;
    final tab = st?.tab ?? 'library';
    final r = context.skin.controlRadius ?? 10;
    return Container(
      height: 64,
      padding: const EdgeInsets.fromLTRB(22, 0, 20, 0),
      decoration: BoxDecoration(
        color: t.panel,
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          Container(
            width: 32,
            height: 32,
            decoration: BoxDecoration(
              color: kArcade.withValues(alpha: 0.17),
              borderRadius: BorderRadius.circular(10),
            ),
            child: const Icon(Icons.sports_esports_outlined,
                size: 18, color: kArcade),
          ),
          const SizedBox(width: 10),
          Text('Arcade',
              style: TextStyle(
                  fontSize: 19, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(width: 14),
          Expanded(
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: Container(
                padding: const EdgeInsets.all(3),
                decoration: BoxDecoration(
                  color: t.nChip,
                  borderRadius: BorderRadius.circular(r + 3),
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    for (final f in keepTabs('arcade', arcadeTabs, (f) => f.id,
                        active: (f) => tab == f.id))
                      TabPill(
                        label: f.label,
                        on: f.id == tab,
                        count: 0,
                        radius: r,
                        tint: kArcade,
                        onTap: () => c.send(ArcadeCmd.setTab(tab: f.id)),
                      ),
                  ],
                ),
              ),
            ),
          ),
          const SizedBox(width: 12),
          SizedBox(
            width: 220,
            height: 38,
            child: TextField(
              controller: _q,
              style: TextStyle(fontSize: 13, color: t.nInk),
              onChanged: (v) {
                setState(() {});
                c.send(ArcadeCmd.search(text: v), quiet: true);
              },
              decoration: InputDecoration(
                isDense: true,
                hintText: 'Find a game',
                hintStyle: TextStyle(fontSize: 13, color: t.nInk3),
                prefixIcon: Icon(Icons.search, size: 17, color: t.nInk3),
                filled: true,
                fillColor: t.nChip,
                contentPadding: const EdgeInsets.symmetric(vertical: 10),
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(r),
                  borderSide: BorderSide.none,
                ),
              ),
            ),
          ),
          const SizedBox(width: 10),
          OutlinedButton.icon(
            icon: const Icon(Icons.tv, size: 18),
            label: const Text('Couch mode'),
            onPressed: st == null ? null : () => c.setCouch(true),
          ),
        ],
      ),
    );
  }
}

// ------------------------------------------------------------------- cover --

/// A game's cover, or its title on the section's colours when there is none.
class _Cover extends StatelessWidget {
  const _Cover({required this.g, this.radius = 12, this.big = false});

  final GameTile g;
  final double radius;
  final bool big;

  @override
  Widget build(BuildContext context) {
    final seed = g.title.hashCode;
    final hue = (seed % 360).abs().toDouble();
    final fallback = Container(
      decoration: BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: [
            HSLColor.fromAHSL(1, hue, 0.45, 0.28).toColor(),
            HSLColor.fromAHSL(1, (hue + 50) % 360, 0.55, 0.45).toColor(),
          ],
        ),
      ),
      alignment: Alignment.bottomLeft,
      padding: const EdgeInsets.all(12),
      child: Text(g.title,
          maxLines: 3,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(
              fontSize: big ? 24 : 15,
              fontWeight: FontWeight.w800,
              color: Colors.white,
              height: 1.15)),
    );
    return ClipRRect(
      borderRadius: BorderRadius.circular(radius),
      child: AspectRatio(
        aspectRatio: 3 / 4,
        child: g.cover.isEmpty
            ? fallback
            : Image.file(File(g.cover),
                fit: BoxFit.cover,
                cacheWidth: big ? 600 : 300,
                errorBuilder: (_, __, ___) => fallback),
      ),
    );
  }
}

String _sourceShort(String s) => switch (s) {
      'steam' => 'STEAM',
      'epic' => 'EPIC',
      'gog' => 'GOG',
      'lutris' => 'LUTRIS',
      'native' => 'LINUX',
      'rom' => 'ROM',
      _ => s.toUpperCase(),
    };

class _Tile extends StatelessWidget {
  const _Tile({required this.g, required this.onTap});

  final GameTile g;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      borderRadius: BorderRadius.circular(12),
      onTap: onTap,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Stack(children: [
            _Cover(g: g),
            Positioned(
              top: 8,
              left: 8,
              child: Container(
                padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
                decoration: BoxDecoration(
                  color: Colors.black.withValues(alpha: 0.55),
                  borderRadius: BorderRadius.circular(5),
                ),
                child: Text(
                    g.system.isNotEmpty && g.source == 'rom'
                        ? g.system.toUpperCase()
                        : _sourceShort(g.source),
                    style: const TextStyle(
                        fontSize: 9.5,
                        fontWeight: FontWeight.w800,
                        color: Colors.white)),
              ),
            ),
            if (g.fav)
              const Positioned(
                top: 6,
                right: 6,
                child: Icon(Icons.star_rounded, color: Color(0xFFFACC15)),
              ),
          ]),
          const SizedBox(height: 6),
          Text(g.title,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 13, fontWeight: FontWeight.w700, color: t.nInk)),
          Text(
              [g.played, g.last].where((x) => x.isNotEmpty).join(' · ').isEmpty
                  ? 'Not played yet'
                  : [g.played, g.last].where((x) => x.isNotEmpty).join(' · '),
              style: TextStyle(fontSize: 11.5, color: t.nInk3)),
        ],
      ),
    );
  }
}

// ----------------------------------------------------------------- library --

class _Library extends StatelessWidget {
  const _Library({required this.c, required this.st});

  final ArcadeController c;
  final ArcadeState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final total = st.counts.isEmpty ? 0 : st.counts.first.n.toInt();
    if (total == 0 && st.filter == 'all' && st.query.isEmpty) {
      return Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 520),
          child: Column(mainAxisSize: MainAxisSize.min, children: [
            const Quiet(
              icon: Icons.sports_esports_outlined,
              title: 'No games found yet',
              body: 'Arcade reads Steam, Heroic (Epic and GOG), Lutris and the '
                  'games installed on this computer. None of them has an '
                  'installed game here. ROM folders can be added on Sources.',
            ),
            const SizedBox(height: 12),
            OutlinedButton(
              onPressed: () => c.send(const ArcadeCmd.rescan()),
              child: const Text('Look again'),
            ),
          ]),
        ),
      );
    }
    final resume = st.resume;
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 30),
      children: [
        if (resume != null && st.filter == 'all' && st.query.isEmpty) ...[
          _Resume(c: c, g: resume),
          const SizedBox(height: 18),
        ],
        Wrap(spacing: 6, runSpacing: 6, children: [
          for (final f in st.counts)
            ChoiceChip(
              label: Text('${f.label} ${f.n}'),
              selected: st.filter == f.id,
              selectedColor: kArcade.withValues(alpha: 0.18),
              onSelected: (_) => c.send(ArcadeCmd.setFilter(filter: f.id)),
            ),
        ]),
        const SizedBox(height: 16),
        if (st.games.isEmpty)
          Text('Nothing here.', style: TextStyle(color: t.nInk3))
        else
          Grid(min: 150, children: [
            for (final g in st.games)
              _Tile(g: g, onTap: () => c.send(ArcadeCmd.open(key: g.key))),
          ]),
      ],
    );
  }
}

class _Resume extends StatelessWidget {
  const _Resume({required this.c, required this.g});

  final ArcadeController c;
  final GameTile g;

  @override
  Widget build(BuildContext context) {
    return Container(
      clipBehavior: Clip.antiAlias,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(context.skin.panelRadius ?? 16),
        gradient: const LinearGradient(colors: [Color(0xFF0B1F14), Color(0xFF0E3B3F)]),
      ),
      padding: const EdgeInsets.all(18),
      child: Row(children: [
        SizedBox(width: 110, child: _Cover(g: g, radius: 10)),
        const SizedBox(width: 20),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text('CONTINUE PLAYING',
                  style: TextStyle(
                      fontSize: 11,
                      letterSpacing: 1.4,
                      fontWeight: FontWeight.w700,
                      color: Colors.white70)),
              const SizedBox(height: 6),
              Text(g.title,
                  style: const TextStyle(
                      fontSize: 30,
                      fontWeight: FontWeight.w800,
                      color: Colors.white)),
              const SizedBox(height: 4),
              Text([g.played, 'last ${g.last.toLowerCase()}'].where((x) => x.isNotEmpty).join(' · '),
                  style: const TextStyle(color: Colors.white70)),
              const SizedBox(height: 14),
              Row(children: [
                FilledButton.icon(
                  style: FilledButton.styleFrom(
                      backgroundColor: kArcade, foregroundColor: Colors.black),
                  icon: const Icon(Icons.play_arrow_rounded),
                  label: const Text('Play'),
                  onPressed: () => c.send(ArcadeCmd.play(key: g.key)),
                ),
                const SizedBox(width: 10),
                TextButton(
                  style: TextButton.styleFrom(foregroundColor: Colors.white),
                  onPressed: () => c.send(ArcadeCmd.open(key: g.key)),
                  child: const Text('Details'),
                ),
              ]),
            ],
          ),
        ),
      ]),
    );
  }
}

// ---------------------------------------------------------------- one game --

class _GamePage extends StatelessWidget {
  const _GamePage({required this.c, required this.g});

  final ArcadeController c;
  final GameView g;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = c.state;
    final tile = GameTile(
      key: g.key,
      title: g.title,
      source: '',
      system: g.system,
      cover: g.cover,
      played: g.played,
      minutes: 0,
      last: g.last,
      fav: g.fav,
    );
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 16, 24, 30),
      children: [
        Row(children: [
          TextButton.icon(
            icon: const Icon(Icons.arrow_back, size: 17),
            label: const Text('Library'),
            onPressed: () => c.send(const ArcadeCmd.close()),
          ),
        ]),
        const SizedBox(height: 8),
        Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SizedBox(width: 220, child: _Cover(g: tile, radius: 14, big: true)),
            const SizedBox(width: 24),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(g.title,
                      style: TextStyle(
                          fontSize: 32,
                          fontWeight: FontWeight.w800,
                          letterSpacing: -0.5,
                          color: t.nInk)),
                  const SizedBox(height: 4),
                  Text(
                      [g.sourceLabel, if (g.system.isNotEmpty) g.system, if (g.size.isNotEmpty) g.size]
                          .join(' · '),
                      style: TextStyle(fontSize: 13, color: t.nInk2)),
                  const SizedBox(height: 12),
                  Wrap(spacing: 8, runSpacing: 8, children: [
                    if (g.tier.isNotEmpty)
                      Container(
                        padding: const EdgeInsets.symmetric(
                            horizontal: 8, vertical: 3),
                        decoration: BoxDecoration(
                          color: tierColour(g.tier),
                          borderRadius: BorderRadius.circular(6),
                        ),
                        child: Text(
                            g.tier == 'native'
                                ? 'NATIVE'
                                : 'PROTONDB ${g.tier.toUpperCase()}',
                            style: const TextStyle(
                                fontSize: 11,
                                fontWeight: FontWeight.w800,
                                color: Colors.black)),
                      ),
                    if (g.played.isNotEmpty) Chip(label: Text('${g.played} played')),
                    if (g.last.isNotEmpty) Chip(label: Text('Last ${g.last.toLowerCase()}')),
                  ]),
                  const SizedBox(height: 18),
                  Row(children: [
                    FilledButton.icon(
                      style: FilledButton.styleFrom(
                          backgroundColor: kArcade,
                          foregroundColor: Colors.black,
                          minimumSize: const Size(150, 48)),
                      icon: const Icon(Icons.play_arrow_rounded),
                      label: const Text('Play',
                          style: TextStyle(fontWeight: FontWeight.w800)),
                      onPressed: () => c.send(ArcadeCmd.play(key: g.key)),
                    ),
                    const SizedBox(width: 10),
                    IconButton(
                      tooltip: g.fav ? 'Not a favourite' : 'Favourite',
                      icon: Icon(g.fav ? Icons.star_rounded : Icons.star_border_rounded,
                          color: g.fav ? const Color(0xFFFACC15) : null),
                      onPressed: () => c.send(ArcadeCmd.fav(key: g.key)),
                    ),
                    IconButton(
                      tooltip: g.hidden ? 'Show in the library' : 'Hide from the library',
                      icon: Icon(g.hidden ? Icons.visibility : Icons.visibility_off_outlined),
                      onPressed: () => c.send(ArcadeCmd.hide_(key: g.key)),
                    ),
                    if (st?.ludusavi ?? false)
                      TextButton.icon(
                        icon: const Icon(Icons.save_outlined, size: 18),
                        label: const Text('Back up saves'),
                        onPressed: () =>
                            c.send(ArcadeCmd.backupSaves(key: g.key)),
                      ),
                  ]),
                  const SizedBox(height: 18),
                  Container(
                    padding: const EdgeInsets.all(14),
                    decoration: cardDeco(context),
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(children: [
                          Text('Notes',
                              style: TextStyle(
                                  fontWeight: FontWeight.w700, color: t.nInk)),
                          const Spacer(),
                          TextButton(
                            onPressed: () async {
                              final n = await askLine(context, 'Notes',
                                  'Builds, codes, where you got to',
                                  initial: g.note);
                              if (n != null) {
                                await c.send(ArcadeCmd.setNote(key: g.key, note: n));
                              }
                            },
                            child: Text(g.note.isEmpty ? 'Add' : 'Edit'),
                          ),
                        ]),
                        Text(g.note.isEmpty ? 'Nothing yet.' : g.note,
                            style: TextStyle(color: t.nInk2)),
                      ],
                    ),
                  ),
                  if (g.sessions.isNotEmpty) ...[
                    const SizedBox(height: 14),
                    Container(
                      padding: const EdgeInsets.all(14),
                      decoration: cardDeco(context),
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text('Sessions Arcade timed',
                              style: TextStyle(
                                  fontWeight: FontWeight.w700, color: t.nInk)),
                          const SizedBox(height: 6),
                          for (final s in g.sessions)
                            Text(s, style: TextStyle(color: t.nInk2)),
                        ],
                      ),
                    ),
                  ],
                ],
              ),
            ),
          ],
        ),
      ],
    );
  }
}

// ------------------------------------------------------------------- stats --

class _Stats extends StatelessWidget {
  const _Stats({required this.c, required this.st});

  final ArcadeController c;
  final ArcadeState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final s = st.stats;
    Widget stat(String v, String l) => Container(
          constraints: const BoxConstraints(minWidth: 150),
          padding: const EdgeInsets.all(14),
          decoration: cardDeco(context),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(v,
                  style: TextStyle(
                      fontSize: 24, fontWeight: FontWeight.w800, color: t.nInk)),
              Text(l, style: TextStyle(fontSize: 12, color: t.nInk3)),
            ],
          ),
        );
    final maxMin =
        s.bySource.fold<int>(1, (m, f) => math.max(m, f.n.toInt()));
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 30),
      children: [
        Wrap(spacing: 12, runSpacing: 12, children: [
          stat(s.total, 'played, as the launchers count it'),
          stat('${s.games}', 'games'),
          stat('${s.played}', 'played'),
          stat('${s.never}', 'never started'),
        ]),
        const SizedBox(height: 20),
        Text('Most played',
            style: TextStyle(
                fontSize: 16, fontWeight: FontWeight.w700, color: t.nInk)),
        const SizedBox(height: 10),
        Grid(min: 150, children: [
          for (final g in s.top)
            _Tile(g: g, onTap: () => c.send(ArcadeCmd.open(key: g.key))),
        ]),
        if (s.bySource.isNotEmpty) ...[
          const SizedBox(height: 20),
          Text('Where the hours went',
              style: TextStyle(
                  fontSize: 16, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(height: 10),
          for (final f in s.bySource)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 5),
              child: Row(children: [
                SizedBox(
                    width: 190,
                    child: Text(f.label, style: TextStyle(color: t.nInk2))),
                Expanded(
                  child: LinearProgressIndicator(
                    value: f.n.toInt() / maxMin,
                    minHeight: 10,
                    color: kArcade,
                    borderRadius: BorderRadius.circular(6),
                  ),
                ),
                const SizedBox(width: 10),
                SizedBox(
                    width: 70,
                    child: Text('${f.n.toInt() ~/ 60} h',
                        textAlign: TextAlign.right,
                        style: TextStyle(color: t.nInk))),
              ]),
            ),
        ],
        if (s.never > 0) ...[
          const SizedBox(height: 20),
          OutlinedButton.icon(
            icon: const Icon(Icons.casino_outlined, size: 18),
            label: Text('Pick one of the ${s.never} I have not started'),
            onPressed: () async {
              await c.send(const ArcadeCmd.setFilter(filter: 'unplayed'));
              final games = c.state?.games ?? const <GameTile>[];
              if (games.isEmpty) return;
              final pick = games[math.Random().nextInt(games.length)];
              await c.send(ArcadeCmd.open(key: pick.key));
            },
          ),
        ],
      ],
    );
  }
}

// ----------------------------------------------------------------- sources --

class _Sources extends StatelessWidget {
  const _Sources({required this.c, required this.st});

  final ArcadeController c;
  final ArcadeState st;

  Future<void> _addRoms() async {
    final dir = await dialogPickFolder(title: 'A folder of ROMs', initial: '');
    if (dir != null && dir.isNotEmpty) await c.send(ArcadeCmd.addRoms(path: dir));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 20, 24, 30),
      children: [
        Row(children: [
          Expanded(
            child: Text(
                'Arcade reads each launcher\'s own files and starts games through '
                'it. Nothing is changed in any launcher.',
                style: TextStyle(fontSize: 12.5, color: t.nInk2)),
          ),
          OutlinedButton.icon(
            icon: const Icon(Icons.refresh, size: 17),
            label: const Text('Look again'),
            onPressed: () => c.send(const ArcadeCmd.rescan()),
          ),
        ]),
        const SizedBox(height: 12),
        Container(
          decoration: cardDeco(context),
          child: Column(children: [
            for (final s in st.sources)
              ListTile(
                leading: Icon(
                    s.found ? Icons.check_circle : Icons.remove_circle_outline,
                    color: s.found ? Tokens.ok : t.nInk3),
                title: Text(s.label,
                    style: TextStyle(fontWeight: FontWeight.w700, color: t.nInk)),
                subtitle: Text(s.how,
                    style: TextStyle(fontSize: 12, color: t.nInk2)),
                trailing: Text(plural(s.games.toInt(), 'game'),
                    style: TextStyle(color: t.nInk2)),
              ),
          ]),
        ),
        const SizedBox(height: 18),
        Row(children: [
          Expanded(
            child: Text('ROM folders',
                style: TextStyle(
                    fontSize: 16, fontWeight: FontWeight.w700, color: t.nInk)),
          ),
          FilledButton.icon(
            style: FilledButton.styleFrom(
                backgroundColor: kArcade, foregroundColor: Colors.black),
            icon: const Icon(Icons.create_new_folder_outlined, size: 18),
            label: const Text('Add a folder'),
            onPressed: _addRoms,
          ),
        ]),
        const SizedBox(height: 6),
        Text(
            'A ROM opens in the emulator your system opens that kind of file with.',
            style: TextStyle(fontSize: 12.5, color: t.nInk2)),
        const SizedBox(height: 8),
        for (final r in st.roms)
          ListTile(
            leading: const Icon(Icons.folder_outlined),
            title: Text(r.path),
            subtitle: Text(plural(r.games.toInt(), 'game')),
            trailing: IconButton(
              tooltip: 'Remove',
              icon: const Icon(Icons.close),
              onPressed: () => c.send(ArcadeCmd.removeRoms(id: r.id)),
            ),
          ),
        const SizedBox(height: 18),
        SwitchListTile(
          value: st.proton,
          activeThumbColor: kArcade,
          title: const Text('Ask ProtonDB how Steam games run on Linux'),
          subtitle: const Text(
              'Sends the Steam app number of a game when you open it; asked once a week at most.'),
          onChanged: (v) => c.send(ArcadeCmd.setProton(on_: v)),
        ),
        ListTile(
          leading: Icon(st.ludusavi ? Icons.check_circle : Icons.info_outline,
              color: st.ludusavi ? Tokens.ok : t.nInk3),
          title: const Text('Save backups with ludusavi'),
          subtitle: Text(st.ludusavi
              ? 'Found. A game\'s page has Back up saves.'
              : 'Not installed. ludusavi knows where 19,000 games keep their saves.'),
        ),
      ],
    );
  }
}

// ------------------------------------------------------------------- couch --

/// The shelf full screen: big covers, arrow keys to move, Enter to play,
/// Escape to leave.
class _Couch extends StatelessWidget {
  const _Couch({required this.c, required this.st});

  final ArcadeController c;
  final ArcadeState st;

  @override
  Widget build(BuildContext context) {
    final games = [
      if (st.resume != null) st.resume!,
      for (final g in st.games)
        if (g.key != st.resume?.key) g,
    ];
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): () => c.setCouch(false),
      },
      child: Focus(
        autofocus: true,
        child: Container(
          color: const Color(0xFF05070A),
          padding: const EdgeInsets.fromLTRB(40, 30, 40, 24),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Row(children: [
                const Icon(Icons.sports_esports, color: Colors.white70),
                const SizedBox(width: 10),
                const Text('Arcade',
                    style: TextStyle(
                        fontSize: 22,
                        fontWeight: FontWeight.w800,
                        color: Colors.white)),
                const Spacer(),
                TextButton.icon(
                  style: TextButton.styleFrom(foregroundColor: Colors.white70),
                  icon: const Icon(Icons.close),
                  label: const Text('Esc to leave'),
                  onPressed: () => c.setCouch(false),
                ),
              ]),
              const SizedBox(height: 24),
              Expanded(
                child: GridView.builder(
                  gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(
                    maxCrossAxisExtent: 240,
                    mainAxisSpacing: 28,
                    crossAxisSpacing: 28,
                    childAspectRatio: 3 / 4.6,
                  ),
                  itemCount: games.length,
                  itemBuilder: (context, i) =>
                      _CouchTile(g: games[i], first: i == 0, c: c),
                ),
              ),
              const Text('← → ↑ ↓ move · Enter plays · Esc leaves',
                  style: TextStyle(color: Colors.white54)),
            ],
          ),
        ),
      ),
    );
  }
}

class _CouchTile extends StatefulWidget {
  const _CouchTile({required this.g, required this.first, required this.c});

  final GameTile g;
  final bool first;
  final ArcadeController c;

  @override
  State<_CouchTile> createState() => _CouchTileState();
}

class _CouchTileState extends State<_CouchTile> {
  bool _focus = false;

  @override
  Widget build(BuildContext context) {
    return InkWell(
      autofocus: widget.first,
      onFocusChange: (f) => setState(() => _focus = f),
      onTap: () => widget.c.send(ArcadeCmd.play(key: widget.g.key)),
      child: AnimatedScale(
        scale: _focus ? 1.06 : 0.96,
        duration: const Duration(milliseconds: 140),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Container(
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(16),
                border: Border.all(
                    color: _focus ? Colors.white : Colors.transparent,
                    width: 4),
              ),
              child: _Cover(g: widget.g, radius: 12, big: true),
            ),
            const SizedBox(height: 8),
            Text(widget.g.title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                    fontSize: 16,
                    fontWeight: FontWeight.w700,
                    color: _focus ? Colors.white : Colors.white70)),
          ],
        ),
      ),
    );
  }
}
