// The Tags & Lyrics manager — Slint's `np.p5.atmusic.library-manager`.
//
// One modal over the Songs tab in two halves, because the two jobs have the
// same shape: which of the library's songs still have no tags, and which still
// have no words. Both are three stat cards that double as filters, a coverage
// bar, a paged list with a status pill, and a per-row action.
//
// The Lyrics half goes further, because fixing a missing lyric is a search
// rather than a form: a row opens an LRCLIB query seeded from the song's tags,
// the results come back with their bodies attached, picking one previews it,
// and Save writes it to the library both builds read. The Tags half hands the
// row to the tag editor that already exists.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';
import 'music_widgets.dart';

/// Open the manager. Closing it is one command, so the dialog can be dismissed
/// from anywhere and the bridge still lets go of the three COUNT(*)s it runs
/// on every snapshot while it is up.
Future<void> openMetaManager(
  BuildContext context,
  MusicController c, {
  String tab = 'lyrics',
}) async {
  await c.send(MusicCmd.mgrOpen(tab: tab));
  if (!context.mounted) return;
  await showDialog<void>(
    context: context,
    builder: (_) => _Manager(controller: c),
  );
  await c.send(const MusicCmd.mgrClose());
}

class _Manager extends StatelessWidget {
  const _Manager({required this.controller});

  final MusicController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: controller,
      builder: (context, _) {
        final st = controller.state;
        if (st == null) return const SizedBox.shrink();
        return Dialog(
          // Fully opaque. `panel` is 82% white in the light theme -- right for
          // a sheet that slides over a page, wrong for a 640x680 modal, which
          // showed the song grid through its own progress bar and made both
          // unreadable.
          backgroundColor: t.panel.withValues(alpha: 1),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(18),
            side: BorderSide(color: t.outline),
          ),
          child: PopScope(
            // `mgr_dirty` means a lyric candidate has been previewed and not
            // written. Escape and the system back gesture used to throw it
            // away without a word.
            canPop: !st.mgrDirty,
            onPopInvokedWithResult: (didPop, _) async {
              if (didPop) return;
              if (!await confirmDiscardLyrics(context)) return;
              if (context.mounted) Navigator.of(context).pop();
            },
            child: SizedBox(
              // 640 x 680 in Slint.
              width: 640,
              height: 680,
              child: Padding(
                padding: const EdgeInsets.all(22),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    _Head(controller: controller, st: st),
                    const SizedBox(height: 14),
                    Expanded(
                      child: switch (st.mgrMode) {
                        'search' => _Search(controller: controller, st: st),
                        'preview' => _Preview(controller: controller, st: st),
                        'view' => _Lines(st: st),
                        _ => _List(controller: controller, st: st),
                      },
                    ),
                  ],
                ),
              ),
            ),
          ),
        );
      },
    );
  }
}

/// The one thing in this dialog that can be lost by leaving it.
Future<bool> confirmDiscardLyrics(BuildContext context) => confirm(
      context,
      title: 'Discard these words?',
      body: 'The lyrics you previewed have not been written to the track yet. '
          'Leaving now loses them; the search that found them does not have to '
          'be run again.',
      action: 'Discard',
    );

class _Head extends StatelessWidget {
  const _Head({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final list = st.mgrMode == 'list';
    // No heading. Two chips say which half you are looking at and the close
    // says how to leave; a line of type over the top of that only repeats the
    // chip that is already lit.
    return Row(
      children: [
        SizedBox(
          width: 96,
          child: list
              ? null
              : Align(
                  alignment: Alignment.centerLeft,
                  child: DetailActionBtn(
                    icon: Icons.chevron_left,
                    label: 'Back',
                    onTap: () => controller.send(const MusicCmd.mgrBack()),
                  ),
                ),
        ),
        Expanded(
          child: Center(
            child: list
                ? Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      MusicChip(
                        icon: Icons.label_outline,
                        label: 'Tags',
                        active: st.mgrTab == 'tags',
                        tint: const Color(0xFF3B82F6),
                        tint2: const Color(0xFF06B6D4),
                        onTap: () => controller
                            .send(const MusicCmd.mgrSetTab(tab: 'tags')),
                      ),
                      const SizedBox(width: 10),
                      MusicChip(
                        icon: Icons.lyrics_outlined,
                        label: 'Lyrics',
                        active: st.mgrTab == 'lyrics',
                        tint: const Color(0xFFEC4899),
                        tint2: const Color(0xFF8B5CF6),
                        onTap: () => controller
                            .send(const MusicCmd.mgrSetTab(tab: 'lyrics')),
                      ),
                    ],
                  )
                // Away from the list the song being worked on is the only thing
                // worth naming, and it is not a heading — it is the subject.
                : Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Flexible(
                        child: Text(
                          st.mgrTitle,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                            fontFamily: Tokens.fontFamily,
                            fontSize: 15,
                            fontWeight: FontWeight.w700,
                            color: t.nInk,
                          ),
                        ),
                      ),
                      // Unsaved. A dot rather than a word: the subject is the
                      // song, and "• unsaved" beside it would be a second
                      // title competing with the first.
                      if (st.mgrDirty) ...[
                        const SizedBox(width: 8),
                        Tooltip(
                          message: 'Previewed, not yet written to the track',
                          child: Container(
                            width: 7,
                            height: 7,
                            decoration: const BoxDecoration(
                              color: Tokens.secMusic,
                              shape: BoxShape.circle,
                            ),
                          ),
                        ),
                      ],
                    ],
                  ),
          ),
        ),
        SizedBox(
          width: 96,
          child: Align(
            alignment: Alignment.centerRight,
            child: IconButton(
              icon: const Icon(Icons.close, size: 18),
              // `Navigator.pop` goes straight past the `PopScope` guarding the
              // dialog, so the button asks for itself.
              onPressed: () async {
                if (st.mgrDirty && !await confirmDiscardLyrics(context)) {
                  return;
                }
                if (context.mounted) Navigator.of(context).pop();
              },
            ),
          ),
        ),
      ],
    );
  }
}

/// The list half: three stat cards that filter, a coverage bar, the rows, the
/// pager.
class _List extends StatelessWidget {
  const _List({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tags = st.mgrTab == 'tags';
    final okKey = tags ? 'tagged' : 'synced';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            _Stat(
              label: 'Songs',
              value: st.mgrTotal,
              colour: const Color(0xFF8B5CF6),
              on: st.mgrFilter == 'all',
              onTap: () =>
                  controller.send(const MusicCmd.mgrSetFilter(key: 'all')),
            ),
            const SizedBox(width: 10),
            _Stat(
              label: tags ? 'Tagged' : 'Synced',
              value: st.mgrOk,
              colour: Tokens.secMusic,
              on: st.mgrFilter == okKey,
              onTap: () => controller.send(MusicCmd.mgrSetFilter(key: okKey)),
            ),
            const SizedBox(width: 10),
            _Stat(
              label: 'Missing',
              value: st.mgrMissing,
              colour: const Color(0xFFEF4444),
              on: st.mgrFilter == 'missing',
              onTap: () =>
                  controller.send(const MusicCmd.mgrSetFilter(key: 'missing')),
            ),
          ],
        ),
        const SizedBox(height: 10),
        Row(
          children: [
            Expanded(child: _Coverage(st: st)),
            const SizedBox(width: 10),
            DetailActionBtn(
              icon: tags ? Icons.auto_fix_high : Icons.download,
              label: st.mgrBusy
                  ? 'Fetching…'
                  : (tags ? 'Read all tags' : 'Sync all missing'),
              onTap: () => controller.send(
                tags ? const MusicCmd.readTags() : const MusicCmd.mgrSyncAll(),
              ),
            ),
          ],
        ),
        const SizedBox(height: 10),
        Expanded(
          child: DecoratedBox(
            decoration: BoxDecoration(
              color: t.nCard,
              borderRadius: BorderRadius.circular(12),
              border: Border.all(color: t.nHair),
            ),
            child: st.mgrRows.isEmpty
                ? Center(
                    child: Text('No songs.',
                        style: TextStyle(fontSize: 13, color: t.nInk2)),
                  )
                : ListView.builder(
                    padding: const EdgeInsets.all(8),
                    itemCount: st.mgrRows.length,
                    itemBuilder: (context, i) => _Row(
                      controller: controller,
                      row: st.mgrRows[i],
                      tags: tags,
                    ),
                  ),
          ),
        ),
        const SizedBox(height: 6),
        Center(
          child: Pager(
            page: st.mgrPage,
            pages: st.mgrPages,
            compact: true,
            onGo: (p) => controller.send(MusicCmd.mgrSetPage(page: p)),
          ),
        ),
      ],
    );
  }
}

class _Stat extends StatelessWidget {
  const _Stat({
    required this.label,
    required this.value,
    required this.colour,
    required this.on,
    required this.onTap,
  });

  final String label;
  final int value;
  final Color colour;
  final bool on;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Expanded(
      child: Material(
        color: on ? colour.withValues(alpha: 0.16) : t.nCard,
        borderRadius: BorderRadius.circular(12),
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(12),
          child: Container(
            height: 62,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(12),
              border: Border.all(
                color: on ? colour : t.nHair,
                width: on ? 2 : 1,
              ),
            ),
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Text(
                  '$value',
                  style: TextStyle(
                    fontFamily: Tokens.fontFamily,
                    fontSize: 24,
                    fontWeight: FontWeight.w800,
                    color: colour,
                  ),
                ),
                Text(
                  label,
                  style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w700,
                    color: t.nInk2,
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

class _Coverage extends StatelessWidget {
  const _Coverage({required this.st});

  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final f = st.mgrProgress.clamp(0.0, 1.0);
    return Container(
      height: 34,
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(17),
        border: Border.all(color: t.nHair),
      ),
      clipBehavior: Clip.antiAlias,
      child: LayoutBuilder(
        builder: (context, box) => Stack(
          alignment: Alignment.center,
          children: [
            // The gradient is painted across the *whole* bar and then cut to
            // the fraction, rather than squeezed into it. A gradient scaled to
            // the fill is violet at 5% and violet again at 95%, which tells you
            // nothing; cut from a full-width one, the colour at a given point
            // is fixed and the edge advances through it.
            ClipRect(
              child: Align(
                alignment: Alignment.centerLeft,
                widthFactor: f == 0 ? 0.0001 : f,
                child: SizedBox(
                  width: box.maxWidth,
                  height: 34,
                  child: const DecoratedBox(
                    decoration: BoxDecoration(
                      gradient: LinearGradient(
                        colors: [Color(0xFFEC4899), Color(0xFF8B5CF6)],
                      ),
                    ),
                  ),
                ),
              ),
            ),
            Text(
              st.mgrStatus.isNotEmpty
                  ? st.mgrStatus
                  : '${(f * 100).round()}% covered',
              style: TextStyle(
                fontSize: 12,
                fontWeight: FontWeight.w700,
                color: f > 0.5 ? Colors.white : t.nInk2,
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _Row extends StatelessWidget {
  const _Row({required this.controller, required this.row, required this.tags});

  final MusicController controller;
  final MgrRow row;
  final bool tags;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final missing = row.status == 'Missing';
    final colour = switch (row.status) {
      'Synced' || 'Tagged' => Tokens.secMusic,
      'Normal' => const Color(0xFF3B82F6),
      _ => const Color(0xFFEF4444),
    };
    return InkWell(
      borderRadius: BorderRadius.circular(8),
      onTap: () => _act(context),
      child: SizedBox(
        height: 46,
        child: Row(
          children: [
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    row.track.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w600,
                      color: t.nInk,
                    ),
                  ),
                  Text(
                    row.track.artist.isEmpty ? '—' : row.track.artist,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11, color: t.nInk2),
                  ),
                ],
              ),
            ),
            const SizedBox(width: 10),
            Container(
              width: 64,
              height: 20,
              alignment: Alignment.center,
              decoration: BoxDecoration(
                color: colour.withValues(alpha: 0.15),
                borderRadius: BorderRadius.circular(10),
              ),
              child: Text(
                row.status,
                style: TextStyle(
                  fontSize: 10,
                  fontWeight: FontWeight.w700,
                  color: colour,
                ),
              ),
            ),
            const SizedBox(width: 10),
            DetailActionBtn(
              label: tags ? 'Edit tags' : (missing ? 'Find lyrics' : 'Replace'),
              onTap: () => _act(context),
            ),
            const SizedBox(width: 8),
          ],
        ),
      ),
    );
  }

  /// Tags open the editor that already exists; lyrics open the search, except
  /// that a row which already has words offers to show them first — replacing
  /// is one more tap from there.
  void _act(BuildContext context) {
    if (tags) {
      editTags(context, controller, row.track);
      return;
    }
    controller.send(MusicCmd.mgrSearchOpen(itemId: row.track.itemId));
  }
}

/// The find-lyrics flow on its own, for the player's side panel.
///
/// Slint reaches the same search from two places — the Lyrics manager and the
/// panel's own Find-lyrics toggle — and it is one flow either way: a form
/// seeded from the song's tags, a list of LRCLIB candidates, a preview of the
/// one you picked, and Save. This is that flow without the manager's chrome
/// around it.
class LyricSearch extends StatelessWidget {
  const LyricSearch({super.key, required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) => st.mgrMode == 'preview'
      ? _Preview(controller: controller, st: st)
      : _Search(controller: controller, st: st);
}

/// The search form and its results, in one column: the fields are what you
/// change, the list is what changed because of them.
class _Search extends StatefulWidget {
  const _Search({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  State<_Search> createState() => _SearchState();
}

class _SearchState extends State<_Search> {
  late final TextEditingController _name =
      TextEditingController(text: widget.st.mgrQName);
  late final TextEditingController _artist =
      TextEditingController(text: widget.st.mgrQArtist);
  late final TextEditingController _album =
      TextEditingController(text: widget.st.mgrQAlbum);

  @override
  void dispose() {
    _name.dispose();
    _artist.dispose();
    _album.dispose();
    super.dispose();
  }

  Future<void> _go() async {
    await widget.controller.send(MusicCmd.mgrSetQuery(
      name: _name.text.trim(),
      artist: _artist.text.trim(),
      album: _album.text.trim(),
    ));
    await widget.controller.send(const MusicCmd.mgrSearch());
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.st;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(
          child: DecoratedBox(
            decoration: BoxDecoration(
              color: t.nCard,
              borderRadius: BorderRadius.circular(12),
              border: Border.all(color: t.nHair),
            ),
            child: st.mgrResults.isEmpty
                ? Center(
                    child: Text(
                      st.mgrSearching
                          ? 'Searching LRCLIB…'
                          : 'Edit the fields below and press Search.',
                      style: TextStyle(fontSize: 13, color: t.nInk2),
                    ),
                  )
                : ListView.builder(
                    padding: const EdgeInsets.all(8),
                    itemCount: st.mgrResults.length,
                    itemBuilder: (context, i) => _Hit(
                      hit: st.mgrResults[i],
                      onTap: () =>
                          widget.controller.send(MusicCmd.mgrPick(index: i)),
                    ),
                  ),
          ),
        ),
        const SizedBox(height: 10),
        _Field(label: 'Song name', controller: _name, onSubmit: _go),
        const SizedBox(height: 8),
        _Field(label: 'Artist', controller: _artist, onSubmit: _go),
        const SizedBox(height: 8),
        _Field(label: 'Album (optional)', controller: _album, onSubmit: _go),
        const SizedBox(height: 10),
        SizedBox(
          height: 40,
          child: Material(
            borderRadius: BorderRadius.circular(9),
            clipBehavior: Clip.antiAlias,
            color: Colors.transparent,
            child: Ink(
              decoration: const BoxDecoration(
                gradient: LinearGradient(
                  begin: Alignment(-1, -0.58),
                  end: Alignment(1, 0.58),
                  colors: [Color(0xFFEC4899), Color(0xFF8B5CF6)],
                ),
              ),
              child: InkWell(
                onTap: st.mgrSearching ? null : _go,
                child: Center(
                  child: Text(
                    st.mgrSearching ? 'Searching…' : 'Search',
                    style: const TextStyle(
                      fontFamily: Tokens.fontFamily,
                      fontSize: 14,
                      fontWeight: FontWeight.w700,
                      color: Colors.white,
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

class _Field extends StatelessWidget {
  const _Field({
    required this.label,
    required this.controller,
    required this.onSubmit,
  });

  final String label;
  final TextEditingController controller;
  final VoidCallback onSubmit;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(label, style: TextStyle(fontSize: 11, color: t.nInk2)),
        const SizedBox(height: 4),
        SizedBox(
          // 76, twice what it was: a 38px box around a 13px line is a rule with
          // text on it, not a field you would think to type in.
          height: 76,
          child: TextField(
            controller: controller,
            onSubmitted: (_) => onSubmit(),
            style: TextStyle(fontSize: 13, color: t.nInk),
            decoration: InputDecoration(
              isDense: true,
              contentPadding: const EdgeInsets.symmetric(horizontal: 14),
              filled: true,
              fillColor: t.nCard,
              border: OutlineInputBorder(
                borderRadius: BorderRadius.circular(9),
                borderSide: BorderSide(color: t.nHair),
              ),
              enabledBorder: OutlineInputBorder(
                borderRadius: BorderRadius.circular(9),
                borderSide: BorderSide(color: t.nHair),
              ),
            ),
          ),
        ),
      ],
    );
  }
}

class _Hit extends StatelessWidget {
  const _Hit({required this.hit, required this.onTap});

  final LyricHit hit;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final synced = hit.kind == 'Synced';
    final colour = synced ? Tokens.secMusic : const Color(0xFF3B82F6);
    return Padding(
      padding: const EdgeInsets.only(bottom: 6),
      child: Material(
        color: t.nTile,
        borderRadius: BorderRadius.circular(9),
        child: InkWell(
          onTap: onTap,
          borderRadius: BorderRadius.circular(9),
          child: SizedBox(
            height: 56,
            child: Row(
              children: [
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    mainAxisAlignment: MainAxisAlignment.center,
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        hit.title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          fontSize: 13,
                          fontWeight: FontWeight.w600,
                          color: t.nInk,
                        ),
                      ),
                      Text(
                        hit.sub,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(fontSize: 11, color: t.nInk2),
                      ),
                    ],
                  ),
                ),
                Container(
                  width: 60,
                  height: 20,
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: colour.withValues(alpha: 0.15),
                    borderRadius: BorderRadius.circular(10),
                  ),
                  child: Text(
                    hit.kind,
                    style: TextStyle(
                      fontSize: 10,
                      fontWeight: FontWeight.w700,
                      color: colour,
                    ),
                  ),
                ),
                const SizedBox(width: 12),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// A picked candidate, with the two things you can do about it.
class _Preview extends StatelessWidget {
  const _Preview({required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) => Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Expanded(child: _Lines(st: st)),
          const SizedBox(height: 10),
          Row(
            children: [
              DetailActionBtn(
                icon: Icons.chevron_left,
                label: 'Back to results',
                onTap: () => controller.send(const MusicCmd.mgrBack()),
              ),
              const Spacer(),
              DetailActionBtn(
                icon: Icons.check,
                label: 'Save lyrics',
                onTap: () => controller.send(const MusicCmd.mgrSave()),
              ),
            ],
          ),
        ],
      );
}

/// Whatever words are in hand — the stored set, or the previewed candidate.
class _Lines extends StatelessWidget {
  const _Lines({required this.st});

  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return DecoratedBox(
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(12),
        border: Border.all(color: t.nHair),
      ),
      child: st.mgrViewRows.isEmpty && st.mgrViewPlain.isEmpty
          ? Center(
              child: Text('No lyrics.',
                  style: TextStyle(fontSize: 13, color: t.nInk2)),
            )
          : SingleChildScrollView(
              padding: const EdgeInsets.all(16),
              child: Text(
                st.mgrViewRows.isNotEmpty
                    ? st.mgrViewRows.map((l) => l.text).join('\n')
                    : st.mgrViewPlain,
                textAlign: TextAlign.center,
                style: TextStyle(fontSize: 14, height: 1.5, color: t.nInk2),
              ),
            ),
    );
  }
}
