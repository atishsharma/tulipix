// The right-click menu on a song, and the Properties sheet it opens.
//
// Slint gives a song the same menu wherever it appears — the Songs grid, the
// detail lists, the rails, the queue — and the port had none of it: every
// action lived behind a three-dot button on one row shape and nowhere else.
// This is that menu, as one widget you wrap a row or a tile in.
//
// Two rows of Slint's are not here, and it is worth saying why rather than
// leaving a gap: "Analyze BPM · key · dynamics" decodes the file through
// `tulipix_sec_music::analysis`, which is a crate that links Slint and so
// cannot be linked from the bridge. Whatever that pass has already written is
// read and shown in Properties; nothing here starts one.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/pick.dart';
import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'music_dialogs.dart';

/// Wraps a row or a tile and gives it Slint's song menu on right-click.
///
/// [onPlay] is the row's own idea of what playing it means — in a queue it is
/// "jump here", in a grid it is "start the library from this track" — so it is
/// passed rather than assumed. Everything else is the same everywhere.
class SongContextMenu extends StatelessWidget {
  const SongContextMenu({
    super.key,
    required this.controller,
    required this.track,
    required this.onPlay,
    required this.child,
    this.onQueue,
    this.onRemove,
    this.removeLabel = 'Remove from this list',
  });

  final MusicController controller;
  final Track track;
  final VoidCallback onPlay;
  final Widget child;
  final VoidCallback? onQueue;
  final VoidCallback? onRemove;
  final String removeLabel;

  @override
  Widget build(BuildContext context) => GestureDetector(
        behavior: HitTestBehavior.deferToChild,
        onSecondaryTapUp: (d) =>
            showSongMenu(context, this, at: d.globalPosition),
        child: child,
      );
}

/// The menu, at a point on screen.
Future<void> showSongMenu(
  BuildContext context,
  SongContextMenu host, {
  required Offset at,
}) async {
  final overlay = Overlay.of(context).context.findRenderObject() as RenderBox?;
  if (overlay == null) return;
  final c = host.controller;
  final tr = host.track;

  final choice = await showMenu<String>(
    context: context,
    position: RelativeRect.fromLTRB(
      at.dx,
      at.dy,
      overlay.size.width - at.dx,
      overlay.size.height - at.dy,
    ),
    items: <PopupMenuEntry<String>>[
      _row('play', Icons.play_arrow, 'Play'),
      _row('default', Icons.open_in_new, 'Play in default player'),
      if (host.onQueue != null)
        _row('queue', Icons.playlist_play, 'Add to queue'),
      _row('playlist', Icons.playlist_add, 'Add to playlist…'),
      const PopupMenuDivider(),
      _row('album', Icons.album_outlined, 'View album'),
      _row('artist', Icons.person_outline, 'View artist'),
      _row('lyrics', Icons.lyrics_outlined, 'View lyrics'),
      _row(
        'love',
        tr.loved ? Icons.favorite : Icons.favorite_border,
        tr.loved ? 'Remove from favourites' : 'Add to favourites',
      ),
      // The stars are the row, not a submenu: rating is a single click and a
      // menu that opens another menu to take it is three.
      PopupMenuItem<String>(
        height: 36,
        // It closes itself, with the star that was hit.
        enabled: false,
        child: _RatingRow(controller: c, track: tr),
      ),
      const PopupMenuDivider(),
      _row('props', Icons.info_outline, 'Properties'),
      _row('tags', Icons.edit_outlined, 'Edit media info for library'),
      _row(
          'sonic',
          Icons.auto_awesome,
          'Sonic similar — queue what sounds '
              'like this'),
      _row('video', Icons.movie_outlined, 'Play music video'),
      _row('link', Icons.link, 'Link music video…'),
      // Stem separation already ships — it is a Tools operation over demucs.
      // What it did not have was a way in from a song, which is the only place
      // anyone thinks of it. Tools opens with the file already filled.
      _row('stems', Icons.graphic_eq, 'Separate stems (karaoke)…'),
      if (host.onRemove != null) ...[
        const PopupMenuDivider(),
        _row('remove', Icons.playlist_remove, host.removeLabel),
      ],
      _row('delete', Icons.delete_outline, 'Delete song (from system)',
          danger: true),
    ],
  );
  if (choice == null || !context.mounted) return;

  switch (choice) {
    case 'play':
      host.onPlay();
    case 'default':
      await c.send(MusicCmd.songPlayDefault(itemId: tr.itemId));
    case 'queue':
      host.onQueue?.call();
    case 'playlist':
      await addToPlaylist(context, c, [tr]);
    case 'album':
      await c.send(MusicCmd.songViewAlbum(itemId: tr.itemId));
    case 'artist':
      await c.send(MusicCmd.songViewArtist(itemId: tr.itemId));
    case 'lyrics':
      // Fetch first when there are none stored: the panel is a teleprompter,
      // and opening it on nothing is what "view lyrics" must not mean.
      if (tr.lyrics.isEmpty) {
        await c.send(MusicCmd.fetchLyrics(itemId: tr.itemId));
      }
      c.showPanel('lyrics');
    case 'love':
      await c.send(MusicCmd.love(itemId: tr.itemId));
    case 'props':
      await songProperties(context, c, tr);
    case 'tags':
      await editTags(context, c, tr);
    case 'sonic':
      await c.send(MusicCmd.songSonic(itemId: tr.itemId));
    case 'stems':
      // Not run from here: demucs takes minutes on the processor and belongs
      // in the queue that already knows how to wait for it. This is the door,
      // not the deed — the form opens with the track in it and the person
      // presses Run.
      ShellController.instance
          .goTab(Section.tools, 'stems\u0000${tr.path}');
    case 'video':
      await c.send(MusicCmd.songPlayVideo(itemId: tr.itemId));
    case 'link':
      final path = await pickFile(
        label: 'Videos',
        extensions: const ['mp4', 'mkv', 'mov', 'webm', 'avi', 'm4v'],
      );
      if (path == null) return;
      await c.send(MusicCmd.songLinkVideo(itemId: tr.itemId, path: path));
    case 'remove':
      host.onRemove?.call();
    case 'delete':
      final ok = await confirm(
        context,
        title: 'Delete this file?',
        body: '“${tr.title}” goes to the system recycle bin and leaves the '
            'library. You can put it back from there.',
      );
      if (ok) await c.send(MusicCmd.deleteTrack(itemId: tr.itemId));
  }
}

PopupMenuItem<String> _row(
  String value,
  IconData icon,
  String label, {
  bool danger = false,
}) =>
    PopupMenuItem<String>(
      value: value,
      height: 34,
      child: Row(
        children: [
          Icon(icon, size: 15, color: danger ? Tokens.error : null),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontSize: 13,
                color: danger ? Tokens.error : null,
              ),
            ),
          ),
        ],
      ),
    );

/// Five stars, inline in the menu. Clicking the lit star clears the rating,
/// which is the only way back to unrated.
class _RatingRow extends StatelessWidget {
  const _RatingRow({required this.controller, required this.track});

  final MusicController controller;
  final Track track;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      children: [
        Text('Rating', style: TextStyle(fontSize: 13, color: t.nInk2)),
        const Spacer(),
        for (var n = 1; n <= 5; n++)
          InkWell(
            onTap: () {
              Navigator.pop(context);
              controller.send(MusicCmd.rate(
                itemId: track.itemId,
                stars: track.stars == n ? 0 : n,
              ));
            },
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: 2, vertical: 4),
              child: Icon(
                track.stars >= n ? Icons.star : Icons.star_border,
                size: 16,
                color: track.stars >= n ? Tokens.warn : t.nInk2,
              ),
            ),
          ),
      ],
    );
  }
}

/// Everything the library and the file know about one song.
Future<void> songProperties(
  BuildContext context,
  MusicController c,
  Track track,
) async {
  final props = await musicSongProps(itemId: track.itemId);
  if (!context.mounted) return;
  await showDialog<void>(
    context: context,
    builder: (ctx) {
      final t = ctx.tokens;
      Widget line(String label, String value) {
        if (value.isEmpty) return const SizedBox.shrink();
        return Padding(
          padding: const EdgeInsets.only(bottom: 10),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              SizedBox(
                width: 104,
                child: Text(
                  label,
                  style: TextStyle(fontSize: 12, color: t.nInk3),
                ),
              ),
              Expanded(
                child: SelectableText(
                  value,
                  style: TextStyle(fontSize: 13, color: t.nInk),
                ),
              ),
            ],
          ),
        );
      }

      return AlertDialog(
        title: const Text('Properties'),
        content: SizedBox(
          width: 520,
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                line('Title', props.title),
                line('Artist', props.artist),
                line('Album', props.album),
                line('Genre', props.genre),
                line('Released', props.release),
                line('Credits', props.credits),
                line('Length', props.duration),
                line('Format', props.format),
                line('Stream', props.stream),
                line('Size', props.size),
                // Never empty, and the one line anybody copies out of here.
                line('File', props.path),
                line(
                  'Analysis',
                  props.analysis.isEmpty
                      ? 'Not analysed — run it from the Slint build'
                      : props.analysis,
                ),
                if (props.hasVideo) line('Music video', 'Linked'),
              ],
            ),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Clipboard.setData(ClipboardData(text: props.path)),
            child: const Text('Copy path'),
          ),
          FilledButton(
            style: musicFilledStyle(),
            onPressed: () => Navigator.pop(ctx),
            child: const Text('Done'),
          ),
        ],
      );
    },
  );
}

/// Right-click on a browse tile that has no column to hold a cover.
///
/// Slint's `set-art`, which it wires to genre tiles and to the playlist page.
/// Albums and artists are not here on purpose: their cover comes off the disc
/// or out of the tags, and overriding that is a different argument.
class CardContextMenu extends StatelessWidget {
  const CardContextMenu({
    super.key,
    required this.controller,
    required this.kind,
    required this.cardKey,
    required this.child,
    this.hasArt = false,
    this.onDelete,
    this.deleteLabel = 'Delete',
  });

  final MusicController controller;

  /// genre | playlist — the settings namespace the cover is kept under.
  final String kind;
  final String cardKey;
  final Widget child;
  final bool hasArt;
  final VoidCallback? onDelete;
  final String deleteLabel;

  Future<void> _menu(BuildContext context, Offset at) async {
    final overlay =
        Overlay.of(context).context.findRenderObject() as RenderBox?;
    if (overlay == null) return;
    final choice = await showMenu<String>(
      context: context,
      position: RelativeRect.fromLTRB(
        at.dx,
        at.dy,
        overlay.size.width - at.dx,
        overlay.size.height - at.dy,
      ),
      items: <PopupMenuEntry<String>>[
        _row('art', Icons.image_outlined,
            hasArt ? 'Change cover…' : 'Set cover…'),
        if (hasArt) _row('clear', Icons.hide_image_outlined, 'Remove cover'),
        if (onDelete != null) ...[
          const PopupMenuDivider(),
          _row('delete', Icons.delete_outline, deleteLabel, danger: true),
        ],
      ],
    );
    if (choice == null || !context.mounted) return;
    switch (choice) {
      case 'art':
        final path = await pickFile(
          label: 'Images',
          extensions: const ['jpg', 'jpeg', 'png', 'webp', 'bmp'],
        );
        if (path == null) return;
        await controller.setCardArt(kind, cardKey, path);
      case 'clear':
        await controller.setCardArt(kind, cardKey, '');
      case 'delete':
        onDelete?.call();
    }
  }

  @override
  Widget build(BuildContext context) => GestureDetector(
        behavior: HitTestBehavior.deferToChild,
        onSecondaryTapUp: (d) => _menu(context, d.globalPosition),
        child: child,
      );
}
