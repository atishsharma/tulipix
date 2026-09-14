// Find subtitles for whatever is playing, without leaving the player.
//
// The search itself is Rust — it needs the OpenSubtitles key out of the OS
// keyring and it hashes the file, neither of which Dart can do here. This is
// the dialog over it: pick a language, take what the hash matched, and the
// result comes back as a path the player loads as an external track.
//
// The hash search is the one that matters. It identifies the exact cut of the
// film, so the subtitle it returns is in sync; a title query is a guess, which
// is why the release name is on every row — it is how you tell a 23.976 fps rip
// from a 25 fps one before you spend two minutes finding out.

import 'package:flutter/material.dart';

import '../src/rust/api/videos.dart';

/// Languages offered, as OpenSubtitles' own codes. Not the whole list: these
/// are the ones with enough uploads to be worth a menu entry, and the search
/// takes any code the API knows if one is typed into settings later.
const Map<String, String> kSubtitleLanguages = {
  'en': 'English',
  'es': 'Spanish',
  'fr': 'French',
  'de': 'German',
  'it': 'Italian',
  'pt-BR': 'Portuguese (Brazil)',
  'pt-PT': 'Portuguese',
  'nl': 'Dutch',
  'pl': 'Polish',
  'ru': 'Russian',
  'uk': 'Ukrainian',
  'tr': 'Turkish',
  'ar': 'Arabic',
  'he': 'Hebrew',
  'hi': 'Hindi',
  'ta': 'Tamil',
  'te': 'Telugu',
  'bn': 'Bengali',
  'zh-CN': 'Chinese (simplified)',
  'ja': 'Japanese',
  'ko': 'Korean',
  'sv': 'Swedish',
  'da': 'Danish',
  'fi': 'Finnish',
  'no': 'Norwegian',
  'cs': 'Czech',
  'el': 'Greek',
  'id': 'Indonesian',
  'vi': 'Vietnamese',
};

/// Opens the finder. Returns the path of a downloaded subtitle, or null.
Future<String?> findSubtitles(
  BuildContext context, {
  required String source,
  required String title,
}) {
  return showDialog<String>(
    context: context,
    builder: (context) => _SubtitleFinder(source: source, title: title),
  );
}

class _SubtitleFinder extends StatefulWidget {
  const _SubtitleFinder({required this.source, required this.title});

  final String source;
  final String title;

  @override
  State<_SubtitleFinder> createState() => _SubtitleFinderState();
}

class _SubtitleFinderState extends State<_SubtitleFinder> {
  late final TextEditingController _query =
      TextEditingController(text: widget.title);
  final TextEditingController _season = TextEditingController();
  final TextEditingController _episode = TextEditingController();

  String _language = 'en';
  bool _busy = false;
  String? _error;
  List<SubtitleHit>? _hits;

  /// The row being fetched, so only that one shows a spinner. The index, not
  /// the file id: an Addic7ed result has no file id, so every one of them
  /// would be row 0.
  int? _downloading;

  @override
  void initState() {
    super.initState();
    // The hash search needs nothing typed, and it is the one worth running, so
    // the dialog opens with its answer already on the way.
    _search();
  }

  @override
  void dispose() {
    _query.dispose();
    _season.dispose();
    _episode.dispose();
    super.dispose();
  }

  Future<void> _search() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final hits = await videosSubtitleSearch(
        source: widget.source,
        title: _query.text.trim(),
        language: _language,
        season: int.tryParse(_season.text.trim()) ?? 0,
        episode: int.tryParse(_episode.text.trim()) ?? 0,
      );
      if (!mounted) return;
      setState(() {
        _hits = hits;
        _busy = false;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = '$e';
        _busy = false;
      });
    }
  }

  Future<void> _download(int index, SubtitleHit hit) async {
    setState(() {
      _downloading = index;
      _error = null;
    });
    try {
      final path = await videosSubtitleDownload(
        source: widget.source,
        provider: hit.provider,
        fileId: hit.fileId,
        link: hit.link,
        referer: hit.referer,
        language: hit.language,
      );
      if (!mounted) return;
      Navigator.of(context).pop(path);
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = '$e';
        _downloading = null;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return AlertDialog(
      title: const Text('Find subtitles'),
      content: SizedBox(
        width: 560,
        height: 460,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Row(
              children: [
                Expanded(
                  child: TextField(
                    controller: _query,
                    decoration: const InputDecoration(
                      labelText: 'Title',
                      helperText:
                          'Only used when the file hash matches nothing',
                      isDense: true,
                    ),
                    onSubmitted: (_) => _search(),
                  ),
                ),
                const SizedBox(width: 10),
                SizedBox(
                  width: 168,
                  child: DropdownButtonFormField<String>(
                    initialValue: _language,
                    isDense: true,
                    decoration: const InputDecoration(
                      labelText: 'Language',
                      isDense: true,
                    ),
                    items: [
                      for (final e in kSubtitleLanguages.entries)
                        DropdownMenuItem(value: e.key, child: Text(e.value)),
                    ],
                    onChanged: (v) => setState(() => _language = v ?? 'en'),
                  ),
                ),
              ],
            ),
            const SizedBox(height: 10),
            Row(
              children: [
                SizedBox(
                  width: 96,
                  child: TextField(
                    controller: _season,
                    keyboardType: TextInputType.number,
                    decoration: const InputDecoration(
                        labelText: 'Season', isDense: true),
                  ),
                ),
                const SizedBox(width: 10),
                SizedBox(
                  width: 96,
                  child: TextField(
                    controller: _episode,
                    keyboardType: TextInputType.number,
                    decoration: const InputDecoration(
                        labelText: 'Episode', isDense: true),
                  ),
                ),
                const Spacer(),
                FilledButton.icon(
                  onPressed: _busy ? null : _search,
                  icon: const Icon(Icons.search, size: 18),
                  label: const Text('Search'),
                ),
              ],
            ),
            const SizedBox(height: 14),
            Expanded(child: _results(theme)),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
      ],
    );
  }

  Widget _results(ThemeData theme) {
    if (_busy) return const Center(child: CircularProgressIndicator());

    final error = _error;
    if (error != null) {
      return Center(
        child: Padding(
          padding: const EdgeInsets.all(12),
          child: Text(
            error,
            textAlign: TextAlign.center,
            style: TextStyle(color: theme.colorScheme.error),
          ),
        ),
      );
    }

    final hits = _hits;
    if (hits == null) return const SizedBox.shrink();
    if (hits.isEmpty) {
      return const Center(
        child: Text('Nothing found. Try a shorter title, or another language.'),
      );
    }

    return ListView.separated(
      itemCount: hits.length,
      separatorBuilder: (context, index) => const Divider(height: 1),
      itemBuilder: (context, i) {
        final hit = hits[i];
        final busy = _downloading == i;
        return ListTile(
          dense: true,
          enabled: _downloading == null,
          leading: hit.fromTrusted
              ? const Tooltip(
                  message: 'From a trusted uploader',
                  child: Icon(Icons.verified, size: 18),
                )
              : const Icon(Icons.subtitles_outlined, size: 18),
          title: Text(
            hit.release.isEmpty ? 'Untitled release' : hit.release,
            maxLines: 1,
            overflow: TextOverflow.ellipsis,
          ),
          subtitle: Text(
            '${hit.language} · ${hit.downloads} downloads · '
            '${hit.provider == 'addic7ed' ? 'Addic7ed' : 'OpenSubtitles'}',
            style: theme.textTheme.bodySmall,
          ),
          trailing: busy
              ? const SizedBox(
                  width: 18,
                  height: 18,
                  child: CircularProgressIndicator(strokeWidth: 2),
                )
              : const Icon(Icons.download, size: 18),
          onTap: busy ? null : () => _download(i, hit),
        );
      },
    );
  }
}
