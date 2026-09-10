// The section's modals: the tag editor, the audio settings and the playlist
// picker.
//
// There was a one-line path prompt here too, which every import and export
// used, because the shell was supposed to own file dialogs one day. It does
// now — `lib/design/pick.dart` — so the prompt is gone and the callers open a
// native chooser instead.

import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart' show Int64List;

import '../../design/tokens.dart';
import '../../src/rust/api/music.dart';
import 'music_controller.dart';
import 'smart_editor.dart';

/// The section's confirm button. Left to itself a `FilledButton` takes the
/// app's own primary, which is the shell's violet -- so every "Delete
/// playlist" and "Clear history" in Music was asking in a colour from another
/// section. Music's pink, with ink dark enough to read on it.
ButtonStyle musicFilledStyle({Color? fill}) => FilledButton.styleFrom(
      backgroundColor: fill ?? Tokens.secMusic,
      foregroundColor: const Color(0xFF0B0B0F),
      textStyle: const TextStyle(
        fontFamily: Tokens.fontFamily,
        fontSize: 13,
        fontWeight: FontWeight.w700,
      ),
    );

/// Its Cancel. Neutral: the way out of a confirm should not compete with the
/// thing being confirmed.
ButtonStyle musicQuietStyle(BuildContext context) => TextButton.styleFrom(
      foregroundColor: context.tokens.nInk2,
    );

/// Yes/no, for the three things in this section that cannot be undone.
Future<bool> confirm(
  BuildContext context, {
  required String title,
  required String body,
  String action = 'Delete',
  bool danger = true,
}) async {
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(title),
      content: SizedBox(width: 420, child: Text(body)),
      actions: [
        TextButton(
          style: musicQuietStyle(ctx),
          onPressed: () => Navigator.pop(ctx, false),
          child: const Text('Cancel'),
        ),
        FilledButton(
          // Destructive confirms wear the error red; everything else is the
          // section's pink, so "Rescan" and "Delete" do not look alike.
          style: musicFilledStyle(fill: danger ? Tokens.error : null),
          onPressed: () => Navigator.pop(ctx, true),
          child: Text(action),
        ),
      ],
    ),
  );
  return ok ?? false;
}

/// Ask, then dispatch.
///
/// Slint puts a confirm sheet in front of everything in this section that
/// cannot be taken back — deleting a playlist, clearing the history, dropping
/// a watched folder, a rescan that will run for minutes. The port fired most
/// of them straight off the click. Same yes/no, one call site.
Future<bool> confirmThen(
  BuildContext context,
  MusicController c, {
  required String title,
  required String body,
  required String action,
  required MusicCmd cmd,
  bool danger = true,
}) async {
  final ok = await confirm(
    context,
    title: title,
    body: body,
    action: action,
    danger: danger,
  );
  if (!ok) return false;
  await c.send(cmd);
  return true;
}

/// The tag editor. Writes into the file with ffmpeg and then re-reads it into
/// the library, so the change is visible in both builds rather than only in
/// this one's database.
Future<void> editTags(
  BuildContext context,
  MusicController c,
  Track track,
) async {
  final title = TextEditingController(text: track.title);
  final artist = TextEditingController(text: track.artist);
  final album = TextEditingController(text: track.album);
  final albumArtist = TextEditingController();
  final genre = TextEditingController(text: track.genre);
  final date =
      TextEditingController(text: track.year > 0 ? '${track.year}' : '');
  final trackNo =
      TextEditingController(text: track.trackNo > 0 ? '${track.trackNo}' : '');
  final discNo = TextEditingController();

  Widget field(String label, TextEditingController ctrl) => Padding(
        padding: const EdgeInsets.only(bottom: 10),
        child: TextField(
          controller: ctrl,
          decoration: InputDecoration(labelText: label, isDense: true),
        ),
      );

  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('Edit tags'),
      content: SizedBox(
        width: 460,
        child: SingleChildScrollView(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              field('Title', title),
              field('Artist', artist),
              field('Album', album),
              field('Album artist', albumArtist),
              field('Genre', genre),
              field('Date (YYYY or YYYY-MM-DD)', date),
              Row(
                children: [
                  Expanded(child: field('Track no.', trackNo)),
                  const SizedBox(width: 12),
                  Expanded(child: field('Disc no.', discNo)),
                ],
              ),
              Text(
                track.path,
                style: const TextStyle(fontSize: 11),
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
              ),
            ],
          ),
        ),
      ),
      actions: [
        TextButton(
          style: musicQuietStyle(ctx),
          onPressed: () => Navigator.pop(ctx, false),
          child: const Text('Cancel'),
        ),
        FilledButton(
          style: musicFilledStyle(),
          onPressed: () => Navigator.pop(ctx, true),
          child: const Text('Write to file'),
        ),
      ],
    ),
  );
  if (ok ?? false) {
    await c.send(MusicCmd.saveTags(
      itemId: track.itemId,
      title: title.text.trim(),
      artist: artist.text.trim(),
      album: album.text.trim(),
      albumArtist: albumArtist.text.trim(),
      genre: genre.text.trim(),
      date: date.text.trim(),
      trackNo: int.tryParse(trackNo.text.trim()) ?? 0,
      discNo: int.tryParse(discNo.text.trim()) ?? 0,
    ));
  }
}

/// Every playlist the library has.
///
/// It used to be fetched by switching My Music to the Playlists tab, reading
/// the cards the snapshot then held, and switching back — two full reloads of
/// the library for a list of a dozen names, and the tab you were on came back
/// only if nothing else moved in between. It is on every snapshot now.
List<BrowseCard> _playlists(MusicController c) =>
    c.state?.playlists ?? const <BrowseCard>[];

/// Name a new playlist. Returns its id once the bridge has made it, or null.
/// Marks the "Smart playlist…" answer apart from a name. A control character
/// rather than a word, because a playlist may legitimately be called "smart:".
const String _kSmart = '\u0001';

Future<int?> createPlaylist(BuildContext context, MusicController c) async {
  final name = TextEditingController();
  final chosen = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: const Text('New playlist'),
      content: SizedBox(
        width: 360,
        child: TextField(
          controller: name,
          autofocus: true,
          decoration: const InputDecoration(
            labelText: 'Name',
            hintText: 'Late night, Gym, Rediscovered…',
          ),
          onSubmitted: (v) => Navigator.pop(ctx, v.trim()),
        ),
      ),
      actions: [
        TextButton(
          style: musicQuietStyle(ctx),
          onPressed: () => Navigator.pop(ctx),
          child: const Text('Cancel'),
        ),
        // The other kind. A smart playlist is a rule rather than a list, so it
        // needs the editor rather than this box -- the name typed here carries
        // across so nothing is retyped.
        TextButton(
          style: musicQuietStyle(ctx),
          onPressed: () => Navigator.pop(ctx, '$_kSmart${name.text.trim()}'),
          child: const Text('Smart playlist…'),
        ),
        FilledButton(
          style: musicFilledStyle(),
          onPressed: () => Navigator.pop(ctx, name.text.trim()),
          child: const Text('Create'),
        ),
      ],
    ),
  );
  if (chosen == null) return null;
  if (chosen.startsWith(_kSmart)) {
    if (!context.mounted) return null;
    return editSmartPlaylist(context, c,
        name: chosen.substring(_kSmart.length));
  }
  if (chosen.isEmpty) return null;
  await c.send(MusicCmd.playlistCreate(name: chosen));
  // The snapshot that came back already has it; matching by name is how the
  // id gets back here, since the command answers with the whole section.
  final made = _playlists(c).where((p) => p.title == chosen);
  return made.isEmpty ? null : made.last.id;
}

/// Pick one of the library's playlists and add tracks to it.
Future<void> addToPlaylist(
  BuildContext context,
  MusicController c,
  List<Track> tracks,
) async {
  final playlists = _playlists(c);
  final chosen = await showDialog<int>(
    context: context,
    builder: (ctx) => SimpleDialog(
      title: const Text('Add to playlist'),
      children: [
        for (final p in playlists)
          SimpleDialogOption(
            onPressed: () => Navigator.pop(ctx, p.id),
            child: Text('${p.title}  ·  ${p.count}'),
          ),
        if (playlists.isEmpty)
          const Padding(
            padding: EdgeInsets.fromLTRB(24, 4, 24, 12),
            child: Text('No playlists yet.'),
          ),
        SimpleDialogOption(
          onPressed: () => Navigator.pop(ctx, _kNewPlaylist),
          child: const Row(
            children: [
              Icon(Icons.add, size: 16),
              SizedBox(width: 10),
              Text('New playlist…'),
            ],
          ),
        ),
      ],
    ),
  );
  if (chosen == null) return;
  if (!context.mounted) return;
  await _addTo(context, c, chosen, tracks);
}

/// The sentinel the two pickers use for their "new playlist" row. Playlist ids
/// are rowids, so nothing real is ever negative.
const int _kNewPlaylist = -1;

Future<void> _addTo(
  BuildContext context,
  MusicController c,
  int playlistId,
  List<Track> tracks,
) async {
  var id = playlistId;
  if (id == _kNewPlaylist) {
    id = await createPlaylist(context, c) ?? -1;
    if (id < 0) return;
  }
  await c.send(MusicCmd.playlistAdd(
    playlistId: id,
    itemIds: Int64List.fromList(tracks.map((t) => t.itemId).toList()),
  ));
}

/// A menu that rises out of the button it belongs to.
///
/// Every menu in the player bar has the same problem: the bar is at the foot of
/// the window, and Material's default is to open downward over the anchor, so
/// each one either landed off-screen or covered the control that opened it.
/// `showMenu` with a rect pinned to the button's *top* edge leaves it nowhere
/// to grow but up, which is what Slint's anchored popups do.
///
/// No package for the motion. `showMenu` is a `PopupRoute` with a transition
/// of its own — the menu fades in while its height and the items' opacity
/// animate over about 300 ms — so a dependency here would be buying an
/// animation the framework already runs.
Future<T?> dropUp<T>(
  BuildContext anchor, {
  required List<PopupMenuEntry<T>> items,
}) async {
  final box = anchor.findRenderObject() as RenderBox?;
  final overlayBox =
      Overlay.of(anchor).context.findRenderObject() as RenderBox?;
  if (box == null || overlayBox == null) return null;
  final topLeft = box.localToGlobal(Offset.zero, ancestor: overlayBox);
  return showMenu<T>(
    context: anchor,
    // `bottom` is measured from the overlay's bottom edge, so anchoring it to
    // the button's *top* is what puts the menu above rather than over.
    position: RelativeRect.fromLTRB(
      topLeft.dx,
      topLeft.dy,
      overlayBox.size.width - topLeft.dx - box.size.width,
      overlayBox.size.height - topLeft.dy,
    ),
    items: items,
  );
}

/// The same picker, as a menu that opens upward from the button that asked.
///
/// Slint's `add-to-playlist` is an anchored popup over the player, not a modal:
/// the bar is at the bottom of the window, so the list rises out of the button
/// rather than dimming the page and landing in the middle of it. A dialog for
/// "which of these six" is a heavier gesture than the choice deserves.
///
/// [anchor] must be the context of the widget the menu belongs under — pass a
/// `Builder`'s context if the button is built inline.
Future<void> playlistDropUp(
  BuildContext anchor,
  MusicController c,
  List<Track> tracks,
) async {
  final playlists = _playlists(c);
  final chosen = await dropUp<int>(
    anchor,
    items: [
      for (final p in playlists)
        PopupMenuItem<int>(
          value: p.id,
          child: Row(
            children: [
              const Icon(Icons.queue_music, size: 16),
              const SizedBox(width: 10),
              Expanded(
                child: Text(p.title, overflow: TextOverflow.ellipsis),
              ),
              const SizedBox(width: 12),
              Text('${p.count}', style: const TextStyle(fontSize: 11)),
            ],
          ),
        ),
      if (playlists.isNotEmpty) const PopupMenuDivider(),
      const PopupMenuItem<int>(
        value: _kNewPlaylist,
        child: Row(
          children: [
            Icon(Icons.add, size: 16),
            SizedBox(width: 10),
            Text('New playlist…'),
          ],
        ),
      ),
    ],
  );
  if (chosen == null || !anchor.mounted) return;
  await _addTo(anchor, c, chosen, tracks);
}

/// Output device, gapless, crossfade, ReplayGain and pre-amp — the five knobs
/// that change the mpv flags. All of them land in settings.json under the same
/// `music.*` keys the Slint build uses, so a change here holds there.
Future<void> audioSettings(BuildContext context, MusicController c) async {
  await showDialog<void>(
    context: context,
    builder: (ctx) => AnimatedBuilder(
      animation: c,
      builder: (ctx, _) {
        final st = c.state;
        if (st == null) return const SizedBox.shrink();
        return AlertDialog(
          title: const Text('Audio'),
          content: SizedBox(
            width: 460,
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text('Output device'),
                DropdownButton<String>(
                  value: st.devices.contains(st.device)
                      ? st.device
                      : (st.devices.isEmpty ? null : st.devices.first),
                  isExpanded: true,
                  items: [
                    for (final d in st.devices)
                      DropdownMenuItem(value: d, child: Text(d)),
                  ],
                  onChanged: (v) => v == null
                      ? null
                      : c.send(
                          MusicCmd.setAudio(key: 'music.device', value: v)),
                ),
                SwitchListTile(
                  contentPadding: EdgeInsets.zero,
                  title: const Text('Gapless'),
                  subtitle: const Text('mpv keeps the demuxer warm between '
                      'tracks — the difference on a live album.'),
                  value: st.gapless,
                  onChanged: (v) => c.send(MusicCmd.setAudio(
                      key: 'music.gapless', value: v ? '1' : '0')),
                ),
                const SizedBox(height: 8),
                Text('Crossfade · ${st.crossfade.toStringAsFixed(1)}s'),
                Slider(
                  value: st.crossfade.clamp(0, 12),
                  max: 12,
                  divisions: 24,
                  onChanged: (v) => c.send(MusicCmd.setAudio(
                      key: 'music.crossfade', value: v.toStringAsFixed(1))),
                ),
                const SizedBox(height: 4),
                const Text('ReplayGain'),
                Row(
                  children: [
                    for (final m in const ['off', 'track', 'album'])
                      Padding(
                        padding: const EdgeInsets.only(right: 8),
                        child: ChoiceChip(
                          label: Text(m),
                          selected: st.replaygain == m,
                          onSelected: (_) => c.send(MusicCmd.setAudio(
                              key: 'music.replaygain', value: m)),
                        ),
                      ),
                  ],
                ),
                const SizedBox(height: 12),
                Text('Pre-amp · ${st.preampDb.toStringAsFixed(1)} dB'),
                Slider(
                  value: st.preampDb.clamp(-12, 12),
                  min: -12,
                  max: 12,
                  divisions: 48,
                  onChanged: (v) => c.send(MusicCmd.setAudio(
                      key: 'music.preamp', value: v.toStringAsFixed(1))),
                ),
                Text(
                  'Device, ReplayGain and crossfade apply to the next track — '
                  'they are launch flags, not properties.',
                  style: Theme.of(ctx).textTheme.bodySmall,
                ),
              ],
            ),
          ),
          actions: [
            FilledButton(
              style: musicFilledStyle(),
              onPressed: () => Navigator.pop(ctx),
              child: const Text('Done'),
            ),
          ],
        );
      },
    ),
  );
}
