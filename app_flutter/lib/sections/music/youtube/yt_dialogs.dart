// YouTube's small dialogs: add a channel, import a playlist, new or renamed
// playlist, add a video to one, the caption language. Every page of the tab
// can open them.

import 'package:flutter/material.dart';

import '../../../src/rust/api/music.dart';
import '../music_controller.dart';

/// Subscribe to a channel you already have the URL for, rather than having to
/// find one of its videos first.
Future<void> addYtChannel(BuildContext context, MusicController c) async {
  final url = await _askText(
    context,
    title: 'Add a channel',
    label: 'Channel URL or @handle',
    hint: '@veritasium',
    action: 'Add',
  );
  if (url != null) await c.send(MusicCmd.ytAddChannelUrl(url: url));
}

/// Import a YouTube playlist by URL — ids and title in one go.
Future<void> importYtPlaylist(BuildContext context, MusicController c) async {
  final url = await _askText(
    context,
    title: 'Import a playlist',
    label: 'Playlist URL',
    hint: 'https://www.youtube.com/playlist?list=…',
    action: 'Import',
  );
  if (url != null) await c.send(MusicCmd.ytImportPlaylistUrl(url: url));
}

Future<void> newYtPlaylist(BuildContext context, MusicController c) async {
  final name = await _askText(
    context,
    title: 'New YouTube playlist',
    label: 'Name',
    action: 'Create',
  );
  if (name != null) await c.send(MusicCmd.ytCreatePlaylist(name: name));
}

Future<void> renameYtPlaylist(
    BuildContext context, MusicController c, int id, String current) async {
  final name = await _askText(
    context,
    title: 'Rename playlist',
    label: 'Name',
    action: 'Rename',
    initial: current,
  );
  if (name != null && name != current) {
    await c.send(MusicCmd.ytRenamePlaylist(playlistId: id, name: name));
  }
}

/// The language captions are offered in while watching.
Future<void> setYtCaptionLang(BuildContext context, MusicController c) async {
  final lang = await _askText(
    context,
    title: 'Caption language',
    label: 'Language code',
    hint: 'en, de, pt-BR…',
    action: 'Save',
    initial: c.state?.ytCaptionLang ?? '',
  );
  if (lang != null) await c.send(MusicCmd.ytSetCaptionLang(lang: lang));
}

/// Pick one of the local YouTube playlists for this video. Every YouTube
/// snapshot carries the list. It used to be fetched by switching to the
/// Playlists tab and back, which rebuilt the page under the card whose menu
/// asked, so the dialog found its context gone and quietly did nothing.
Future<void> addToYtPlaylist(
    BuildContext context, MusicController c, YtVideo v) async {
  final playlists = c.state?.ytPlaylists ?? const <YtPlaylist>[];
  if (playlists.isEmpty) {
    await newYtPlaylist(context, c);
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
            child: Text('${p.name}  ·  ${p.count}'),
          ),
      ],
    ),
  );
  if (chosen == null) return;
  await c
      .send(MusicCmd.ytAddToPlaylist(playlistId: chosen, videoId: v.videoId));
}

/// One text field and a button; the trimmed text, or null when cancelled or
/// left empty.
Future<String?> _askText(
  BuildContext context, {
  required String title,
  required String label,
  required String action,
  String? hint,
  String initial = '',
}) async {
  final text = TextEditingController(text: initial)
    ..selection = TextSelection(baseOffset: 0, extentOffset: initial.length);
  final value = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(title),
      content: SizedBox(
        width: 460,
        child: TextField(
          controller: text,
          autofocus: true,
          decoration: InputDecoration(labelText: label, hintText: hint),
          onSubmitted: (v) => Navigator.pop(ctx, v),
        ),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx), child: const Text('Cancel')),
        FilledButton(
            onPressed: () => Navigator.pop(ctx, text.text),
            child: Text(action)),
      ],
    ),
  );
  text.dispose();
  final trimmed = value?.trim() ?? '';
  return trimmed.isEmpty ? null : trimmed;
}
