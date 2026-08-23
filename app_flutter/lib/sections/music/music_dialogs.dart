// The section's modals: the tag editor, the audio settings and the playlist
// picker.
//
// There was a one-line path prompt here too, which every import and export
// used, because the shell was supposed to own file dialogs one day. It does
// now — `lib/design/pick.dart` — so the prompt is gone and the callers open a
// native chooser instead.

import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart' show Int64List;

import '../../src/rust/api/music.dart';
import 'music_controller.dart';

/// Yes/no, for the three things in this section that cannot be undone.
Future<bool> confirm(
  BuildContext context, {
  required String title,
  required String body,
  String action = 'Delete',
}) async {
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(title),
      content: SizedBox(width: 420, child: Text(body)),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Cancel')),
        FilledButton(
          onPressed: () => Navigator.pop(ctx, true),
          child: Text(action),
        ),
      ],
    ),
  );
  return ok ?? false;
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
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Cancel')),
        FilledButton(
            onPressed: () => Navigator.pop(ctx, true),
            child: const Text('Write to file')),
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

/// Pick one of the library's playlists and add tracks to it.
Future<void> addToPlaylist(
  BuildContext context,
  MusicController c,
  List<Track> tracks,
) async {
  // The playlist list only sits in the snapshot while that browse tab is open,
  // so ask for it before showing a picker that would otherwise be empty — and
  // put the tab back afterwards, because the user asked to add a track, not to
  // be moved somewhere else.
  final previous = c.state?.libTab ?? 'songs';
  await c.send(const MusicCmd.setLibTab(name: 'playlists'));
  final playlists = c.state?.cards ?? const <BrowseCard>[];
  await c.send(MusicCmd.setLibTab(name: previous));
  if (!context.mounted) return;
  if (playlists.isEmpty) {
    await showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('No playlists yet'),
        content: const Text(
            'Create one from My Music → Playlists, then add tracks to it.'),
        actions: [
          FilledButton(
              onPressed: () => Navigator.pop(ctx), child: const Text('OK')),
        ],
      ),
    );
    return;
  }
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
      ],
    ),
  );
  if (chosen == null) return;
  await c.send(MusicCmd.playlistAdd(
    playlistId: chosen,
    itemIds: Int64List.fromList(tracks.map((t) => t.itemId).toList()),
  ));
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
                onPressed: () => Navigator.pop(ctx), child: const Text('Done')),
          ],
        );
      },
    ),
  );
}
