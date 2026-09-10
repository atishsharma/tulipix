// Downloader — My Music's eighth sub-tab.
//
// Two blocks side by side, which is Slint's layout: the work on the left — a
// header strip carrying the run's counters and its buttons, and under it the
// entries table filling the rest of the height — and a fixed rail on the right
// holding the three forms (where the music comes from, how it lands on disk,
// and the activity log). The Slint page paged the queue at 25 rows because a
// Slint model of 500 is expensive; a `ListView.builder` is not, so the whole
// queue is one list here and the pager is gone with it.

import 'dart:math' as math;

import 'package:flutter/material.dart';
// frb's Int64List, not dart:typed_data's — they are different types and
// `playList` takes the former.
import 'package:flutter_rust_bridge/flutter_rust_bridge.dart' show Int64List;

import '../../design/pick.dart';
import '../../design/tokens.dart';
import '../../src/rust/api/mdl.dart';
import '../../src/rust/api/music.dart';
import 'mdl_controller.dart';
import 'music_controller.dart';
import 'music_widgets.dart';

const Color kMdlAccent = Color(0xFFEC4899);
const Color kMdlAccent2 = Color(0xFF8B5CF6);

class DownloaderTab extends StatefulWidget {
  const DownloaderTab({super.key});

  @override
  State<DownloaderTab> createState() => _DownloaderTabState();
}

class _DownloaderTabState extends State<DownloaderTab> {
  final MdlController _c = MdlController();
  final TextEditingController _url = TextEditingController();
  final TextEditingController _dest = TextEditingController();

  @override
  void initState() {
    super.initState();
    // Not straight: `initState` runs inside a build and `send` notifies before
    // it awaits, which would mark an already-built listener dirty mid-build.
    WidgetsBinding.instance.addPostFrameCallback((_) => _c.refresh());
    _c.addListener(_adopt);
  }

  /// Keep the two text fields in step with Rust without fighting the cursor.
  /// Rust only moves these on a history replay or a clear, so a difference is
  /// always Rust's news, never a keystroke racing itself.
  void _adopt() {
    final st = _c.state;
    if (st == null) return;
    if (_url.text != st.url) _url.text = st.url;
    if (_dest.text != st.dest) _dest.text = st.dest;
  }

  @override
  void dispose() {
    _c.removeListener(_adopt);
    _c.dispose();
    _url.dispose();
    _dest.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        final st = _c.state;
        if (st == null) {
          return const Center(child: CircularProgressIndicator());
        }
        // Two blocks, as in Slint: the work on the left, the settings on the
        // right. The port stacked all five cards in one scrolling column, so
        // the queue — the thing you are actually reading — started below the
        // fold behind two forms you had already filled in, and the table never
        // got more than the height of one screenful of rows.
        return LayoutBuilder(
          builder: (context, box) {
            // min(377, 44.5%) — Slint's rail. Capped as a fraction too, or a
            // narrow window leaves the table a sliver.
            final railW = math.min(377.0, box.maxWidth * 0.445);
            return Padding(
              padding: const EdgeInsets.all(16),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.stretch,
                      children: [
                        _Head(controller: _c, state: st),
                        const SizedBox(height: 14),
                        if (_c.error != null) ...[
                          _Bar(
                            text: '${_c.error}',
                            tint: Tokens.error,
                            onClose: _c.clearError,
                          ),
                          const SizedBox(height: 12),
                        ],
                        if (st.error.isNotEmpty) ...[
                          _Bar(
                              text: st.error,
                              tint: Tokens.error,
                              onClose: null),
                          const SizedBox(height: 12),
                        ],
                        if (st.ytWarn) ...[
                          const _Bar(
                            text: 'That YouTube link carries no album tag, so '
                                'it may be a video rather than a track. It '
                                'will still download — the cover and album '
                                'will just be whatever YouTube had.',
                            tint: Color(0xFFF59E0B),
                            onClose: null,
                          ),
                          const SizedBox(height: 12),
                        ],
                        Expanded(child: _Tables(controller: _c, state: st)),
                      ],
                    ),
                  ),
                  const SizedBox(width: 14),
                  SizedBox(
                    width: railW,
                    child: ListView(
                      padding: EdgeInsets.zero,
                      children: [
                        _Source(controller: _c, state: st, url: _url),
                        const SizedBox(height: 14),
                        _Output(controller: _c, state: st, dest: _dest),
                        const SizedBox(height: 14),
                        _Cli(controller: _c),
                        const SizedBox(height: 8),
                        Text(
                          'Audio comes from YouTube; the title, artist, album '
                          'and cover come from '
                          '${st.providerBadge.isEmpty ? "the provider" : st.providerBadge}. '
                          'Finished tracks are added to the library straight '
                          'away — no rescan.',
                          style: TextStyle(
                              fontSize: 11.5, color: t.textDim, height: 1.5),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            );
          },
        );
      },
    );
  }
}

// ── header ──────────────────────────────────────────────────────────────────

class _Head extends StatelessWidget {
  const _Head({required this.controller, required this.state});

  final MdlController controller;
  final MdlState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final running = state.status == 'downloading';
    final denom = state.selected > 0 ? state.selected : state.rows.length;
    final moved = controller.done + controller.skipped;
    return Container(
      height: 76,
      padding: const EdgeInsets.symmetric(horizontal: 16),
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(18),
        border: Border.all(
          color: running ? const Color(0xFF22C55E) : t.outline,
          width: 1.5,
        ),
      ),
      child: Row(
        children: [
          Container(
            width: 42,
            height: 42,
            decoration: BoxDecoration(
              gradient: const LinearGradient(colors: [kMdlAccent, kMdlAccent2]),
              borderRadius: BorderRadius.circular(13),
            ),
            child: const Icon(Icons.download, color: Colors.white, size: 21),
          ),
          const SizedBox(width: 12),
          // Flexible, not a Spacer after it: a resolved album title is
          // arbitrary length and this row has four fixed-width controls on its
          // right that must not be pushed off the end.
          Flexible(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Text('Music Downloader',
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontFamily: Tokens.fontFamily,
                        fontSize: 18,
                        fontWeight: FontWeight.w800,
                        color: t.text)),
                const SizedBox(height: 2),
                Text(
                  state.status == 'resolving'
                      ? 'Resolving…'
                      : state.rows.isEmpty
                          ? 'Paste a link or search — tracks land in your '
                              'library with full metadata'
                          : '${state.title.isEmpty ? "" : "${state.title}  ·  "}'
                              '${state.rows.length} tracks resolved',
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11.5, color: t.textDim),
                ),
              ],
            ),
          ),
          const Spacer(),
          _Counters(state: state, controller: controller),
          const SizedBox(width: 10),
          // The run's progress, over what is *selected* rather than over the
          // whole queue — the selection is what will actually download.
          _DlPill(
            active: running,
            frac: denom > 0 ? (moved / denom).clamp(0.0, 1.0) : 0,
            fill: denom > 0 && moved >= denom
                ? const Color(0xFF22C55E)
                : kMdlAccent2,
            label: '$moved / $denom '
                '${running ? "downloading" : "downloaded"}',
          ),
          const SizedBox(width: 10),
          FilledButton.icon(
            style: FilledButton.styleFrom(
              backgroundColor: const Color(0xFF22C55E),
              foregroundColor: const Color(0xFF07120B),
            ),
            onPressed: state.rows.isEmpty || running
                ? null
                : () {
                    controller.setPane(MdlPane.queue);
                    controller.send(const MdlCmd.download());
                  },
            icon: const Icon(Icons.download, size: 17),
            label: Text(
                state.selected > 0 ? 'Download ${state.selected}' : 'Download'),
          ),
          const SizedBox(width: 8),
          IconButton(
            tooltip: 'Cancel',
            onPressed: running
                ? () => controller.send(const MdlCmd.cancel())
                : () => controller.send(const MdlCmd.cancelResolve()),
            icon: const Icon(Icons.close, size: 18),
            color: Tokens.error,
          ),
          IconButton(
            tooltip: 'Rescan the library',
            // The downloader adds finished tracks itself, so this is for files
            // that arrived some other way — a copy into the watched folder.
            onPressed: () =>
                MusicController.instance.send(const MusicCmd.scan()),
            icon: const Icon(Icons.refresh, size: 18),
            color: t.textDim,
          ),
        ],
      ),
    );
  }
}

/// Four numbers with their own icon, in one pill. Queued, done, skipped,
/// failed — Slint's counter chip, which has height for one line only.
class _Counters extends StatelessWidget {
  const _Counters({required this.state, required this.controller});

  final MdlState state;
  final MdlController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget one(IconData icon, Color tint, int n) => Padding(
          padding: const EdgeInsets.only(right: 12),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(icon, size: 12, color: tint),
              const SizedBox(width: 5),
              Text('$n',
                  style: TextStyle(
                      fontFamily: Tokens.fontFamily,
                      fontSize: 12,
                      fontWeight: FontWeight.w800,
                      color: t.text)),
            ],
          ),
        );
    return Container(
      height: 34,
      padding: const EdgeInsets.only(left: 13, right: 1),
      decoration: BoxDecoration(
        color: t.panel2,
        borderRadius: BorderRadius.circular(17),
        border: Border.all(color: t.outline),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          one(Icons.queue_music, kMdlAccent, state.selected),
          one(Icons.check, const Color(0xFF22C55E), controller.done),
          one(Icons.skip_next, const Color(0xFF64748B), controller.skipped),
          one(Icons.close, const Color(0xFFEF4444), controller.failed),
        ],
      ),
    );
  }
}

/// Slint's `DlPill` — a rounded strip that is its own progress bar, so the
/// number and the fill are the same control rather than a label above a track.
class _DlPill extends StatelessWidget {
  const _DlPill({
    required this.active,
    required this.frac,
    required this.fill,
    required this.label,
  });

  final bool active;
  final double frac;
  final Color fill;
  final String label;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      width: 208,
      height: 34,
      clipBehavior: Clip.antiAlias,
      decoration: BoxDecoration(
        color: t.panel2,
        borderRadius: BorderRadius.circular(17),
        border: Border.all(color: active ? fill : t.outline),
      ),
      child: Stack(
        fit: StackFit.expand,
        children: [
          Align(
            alignment: Alignment.centerLeft,
            child: FractionallySizedBox(
              widthFactor: frac.clamp(0.0, 1.0),
              child: ColoredBox(color: fill.withValues(alpha: 0.5)),
            ),
          ),
          Center(
            child: Text(
              label,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontFamily: Tokens.fontFamily,
                fontSize: 12,
                fontWeight: FontWeight.w700,
                color: t.text,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _Bar extends StatelessWidget {
  const _Bar({required this.text, required this.tint, required this.onClose});

  final String text;
  final Color tint;
  final VoidCallback? onClose;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 10, 6, 10),
      decoration: BoxDecoration(
        color: tint.withValues(alpha: 0.1),
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: tint.withValues(alpha: 0.35)),
      ),
      child: Row(
        children: [
          Icon(Icons.info_outline, size: 16, color: tint),
          const SizedBox(width: 9),
          Expanded(
            child: Text(text,
                style: TextStyle(fontSize: 12.5, color: t.text, height: 1.45)),
          ),
          if (onClose != null)
            IconButton(
              icon: const Icon(Icons.close, size: 16),
              onPressed: onClose,
              visualDensity: VisualDensity.compact,
            ),
        ],
      ),
    );
  }
}

// ── cards ───────────────────────────────────────────────────────────────────

class _Card extends StatelessWidget {
  const _Card({
    required this.title,
    required this.child,
    this.trailing,
    this.fill = false,
  });

  final String title;
  final Widget child;
  final Widget? trailing;

  /// Take the height the parent offers rather than the height of the content.
  /// The entries table does; the rail's three forms do not.
  final bool fill;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: t.outline),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: fill ? MainAxisSize.max : MainAxisSize.min,
        children: [
          Row(
            children: [
              Text(title,
                  style: TextStyle(
                      fontSize: 13.5,
                      fontWeight: FontWeight.w700,
                      color: t.text)),
              const Spacer(),
              if (trailing != null) trailing!,
            ],
          ),
          const SizedBox(height: 12),
          if (fill) Expanded(child: child) else child,
        ],
      ),
    );
  }
}

/// Where the music comes from: a URL to resolve, or a search to run.
class _Source extends StatefulWidget {
  const _Source({
    required this.controller,
    required this.state,
    required this.url,
  });

  final MdlController controller;
  final MdlState state;
  final TextEditingController url;

  @override
  State<_Source> createState() => _SourceState();
}

class _SourceState extends State<_Source> {
  void _go() {
    final c = widget.controller;
    c.send(MdlCmd.setUrl(url: widget.url.text));
    c.send(widget.state.mode == 'search'
        ? const MdlCmd.search()
        : const MdlCmd.resolve());
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = widget.state;
    final c = widget.controller;
    final searching = st.mode == 'search';
    final running = c.status == 'resolving';
    return _Card(
      title: 'Source',
      trailing: _Segmented(
        options: const ['url', 'search'],
        labels: const ['Link', 'Search'],
        active: st.mode,
        onPick: (v) => c.send(MdlCmd.setMode(mode: v)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          Row(
            children: [
              Expanded(
                child: TextField(
                  controller: widget.url,
                  onChanged: (v) => c.send(MdlCmd.setUrl(url: v)),
                  onSubmitted: (_) => _go(),
                  decoration: InputDecoration(
                    isDense: true,
                    hintText: searching
                        ? 'Artist, song or album'
                        : 'https://open.spotify.com/album/…',
                    prefixIcon:
                        Icon(searching ? Icons.search : Icons.link, size: 18),
                    suffixIcon: st.providerBadge.isEmpty
                        ? null
                        : Padding(
                            padding: const EdgeInsets.only(right: 8),
                            child: _ProviderBadge(name: st.providerBadge),
                          ),
                    suffixIconConstraints:
                        const BoxConstraints(minWidth: 0, minHeight: 0),
                    border: const OutlineInputBorder(),
                  ),
                ),
              ),
              const SizedBox(width: 10),
              if (searching)
                _Segmented(
                  options: const ['YT Music', 'Spotify'],
                  labels: const ['YT Music', 'Spotify'],
                  active: st.searchProvider,
                  onPick: (v) => c.send(MdlCmd.setSearchProvider(name: v)),
                ),
              if (searching) const SizedBox(width: 10),
              FilledButton.icon(
                onPressed: running ? null : _go,
                style: FilledButton.styleFrom(backgroundColor: kMdlAccent),
                icon: Icon(searching ? Icons.search : Icons.playlist_add_check,
                    size: 17),
                label: Text(searching ? 'Search' : 'Resolve'),
              ),
              if (running) ...[
                const SizedBox(width: 8),
                OutlinedButton(
                  onPressed: () => c.send(const MdlCmd.cancelResolve()),
                  child: const Text('Stop'),
                ),
              ],
            ],
          ),
          const SizedBox(height: 10),
          Text(
            searching
                ? 'Searches ${st.searchProvider}. Results land in the same '
                    'queue a link does, so tagging and naming are identical.'
                : 'Understands ${mdlProviders().join(" · ")}. Short links '
                    'resolve first.',
            style: TextStyle(fontSize: 11.5, color: t.textDim),
          ),
        ],
      ),
    );
  }
}

class _ProviderBadge extends StatelessWidget {
  const _ProviderBadge({required this.name});

  final String name;

  @override
  Widget build(BuildContext context) {
    final tint = providerColor(name);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 9, vertical: 4),
      decoration: BoxDecoration(
        color: tint.withValues(alpha: 0.14),
        borderRadius: BorderRadius.circular(999),
        border: Border.all(color: tint.withValues(alpha: 0.45)),
      ),
      child: Text(name,
          style: TextStyle(
              fontSize: 10.5, fontWeight: FontWeight.w700, color: tint)),
    );
  }
}

/// How the files land: folder, codec, bitrate, naming, concurrency.
class _Output extends StatelessWidget {
  const _Output({
    required this.controller,
    required this.state,
    required this.dest,
  });

  final MdlController controller;
  final MdlState state;
  final TextEditingController dest;

  /// Bitrate is meaningless for the lossless codecs, so the field goes away
  /// rather than sitting there being ignored.
  static const Set<String> _lossy = {'opus', 'm4a', 'mp3'};

  @override
  Widget build(BuildContext context) {
    final st = state;
    final lossy = _lossy.contains(st.format);
    return _Card(
      title: 'Output',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        mainAxisSize: MainAxisSize.min,
        children: [
          TextField(
            controller: dest,
            onSubmitted: (v) => controller.send(MdlCmd.setDest(path: v)),
            onChanged: (v) => controller.send(MdlCmd.setDest(path: v)),
            decoration: InputDecoration(
              isDense: true,
              labelText: 'Download folder',
              prefixIcon: const Icon(Icons.folder_outlined, size: 18),
              border: const OutlineInputBorder(),
              // Typed or chosen: the field stays editable because a path
              // pasted from somewhere else is often faster than walking to it.
              suffixIcon: IconButton(
                tooltip: 'Choose a folder',
                icon: const Icon(Icons.more_horiz, size: 18),
                onPressed: () async {
                  final path = await pickDirectory(initial: dest.text);
                  if (path == null) return;
                  dest.text = path;
                  await controller.send(MdlCmd.setDest(path: path));
                },
              ),
              helperText: 'Also added to the watched folders, so it survives a '
                  'rescan',
            ),
          ),
          const SizedBox(height: 14),
          Wrap(
            spacing: 12,
            runSpacing: 12,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              _Pick(
                label: 'Format',
                value: st.format,
                options: mdlFormats(),
                onPick: (v) => controller.send(MdlCmd.setFormat(name: v)),
              ),
              // Say what a lossless container does and does not buy here. The
              // audio is always yt-dlp's bestaudio — Opus or AAC — so FLAC
              // wraps a lossy master in a lossless box: five to ten times the
              // bytes, not one bit more music. Worth having for a library that
              // must be one format throughout, worth knowing about either way.
              if (!lossy)
                SizedBox(
                  width: 260,
                  child: Text(
                    '${st.format.toUpperCase()} is lossless, but the source is '
                    'not — YouTube audio is Opus or AAC, so this re-wraps it '
                    'rather than recovering anything.',
                    style: TextStyle(
                        fontSize: 11, color: context.tokens.nInk2, height: 1.35),
                  ),
                ),
              if (lossy)
                _Pick(
                  label: 'Bitrate',
                  value: '${st.bitrate}',
                  options: const ['96', '128', '192', '256', '320'],
                  suffix: 'k',
                  onPick: (v) => controller
                      .send(MdlCmd.setBitrate(kbps: int.tryParse(v) ?? 128)),
                ),
              _Pick(
                label: 'Naming',
                value: st.nameMethod,
                options: mdlNameMethods(),
                onPick: (v) => controller.send(MdlCmd.setNameMethod(label: v)),
              ),
              _Stepper(
                label: 'Parallel',
                value: st.parallel,
                min: 1,
                max: 4,
                onSet: (v) => controller.send(MdlCmd.setParallel(n: v)),
              ),
              _Stepper(
                label: 'Threads',
                value: st.threads,
                min: 1,
                max: 8,
                onSet: (v) => controller.send(MdlCmd.setThreads(n: v)),
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class _Pick extends StatelessWidget {
  const _Pick({
    required this.label,
    required this.value,
    required this.options,
    required this.onPick,
    this.suffix = '',
  });

  final String label;
  final String value;
  final List<String> options;
  final String suffix;
  final ValueChanged<String> onPick;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(label,
            style: TextStyle(
                fontSize: 10.5,
                fontWeight: FontWeight.w700,
                letterSpacing: .5,
                color: t.textDim)),
        const SizedBox(height: 4),
        Container(
          height: 36,
          padding: const EdgeInsets.symmetric(horizontal: 10),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(9),
            border: Border.all(color: t.outline),
          ),
          child: DropdownButtonHideUnderline(
            child: DropdownButton<String>(
              value: options.contains(value) ? value : options.first,
              isDense: true,
              items: [
                for (final o in options)
                  DropdownMenuItem(
                    value: o,
                    child: Text('$o$suffix',
                        style: TextStyle(fontSize: 12.5, color: t.text)),
                  ),
              ],
              onChanged: (v) {
                if (v != null) onPick(v);
              },
            ),
          ),
        ),
      ],
    );
  }
}

class _Stepper extends StatelessWidget {
  const _Stepper({
    required this.label,
    required this.value,
    required this.min,
    required this.max,
    required this.onSet,
  });

  final String label;
  final int value;
  final int min;
  final int max;
  final ValueChanged<int> onSet;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(label,
            style: TextStyle(
                fontSize: 10.5,
                fontWeight: FontWeight.w700,
                letterSpacing: .5,
                color: t.textDim)),
        const SizedBox(height: 4),
        Container(
          height: 36,
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(9),
            border: Border.all(color: t.outline),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              _Key(
                icon: Icons.remove,
                enabled: value > min,
                onTap: () => onSet(value - 1),
              ),
              SizedBox(
                width: 26,
                child: Text('$value',
                    textAlign: TextAlign.center,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        color: t.text)),
              ),
              _Key(
                icon: Icons.add,
                enabled: value < max,
                onTap: () => onSet(value + 1),
              ),
            ],
          ),
        ),
      ],
    );
  }
}

class _Key extends StatelessWidget {
  const _Key({required this.icon, required this.enabled, required this.onTap});

  final IconData icon;
  final bool enabled;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return InkWell(
      onTap: enabled ? onTap : null,
      borderRadius: BorderRadius.circular(8),
      child: SizedBox(
        width: 30,
        height: 34,
        child: Icon(icon, size: 15, color: enabled ? kMdlAccent : t.outline),
      ),
    );
  }
}

class _Segmented extends StatelessWidget {
  const _Segmented({
    required this.options,
    required this.labels,
    required this.active,
    required this.onPick,
  });

  final List<String> options;
  final List<String> labels;
  final String active;
  final ValueChanged<String> onPick;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(3),
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(9),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          for (var i = 0; i < options.length; i++)
            InkWell(
              onTap: () => onPick(options[i]),
              borderRadius: BorderRadius.circular(7),
              child: Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 12, vertical: 5),
                decoration: BoxDecoration(
                  color: active == options[i] ? kMdlAccent : null,
                  borderRadius: BorderRadius.circular(7),
                ),
                child: Text(
                  labels[i],
                  style: TextStyle(
                    fontSize: 11.5,
                    fontWeight: FontWeight.w700,
                    color: active == options[i] ? Colors.white : t.textDim,
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}

// ── the three tables ────────────────────────────────────────────────────────

/// Queue · Downloaded · Searches. One card, three panes — the same arrangement
/// the Slint page uses, so nothing about the downloader covers the downloader.
class _Tables extends StatelessWidget {
  const _Tables({required this.controller, required this.state});

  final MdlController controller;
  final MdlState state;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    return _Card(
      fill: true,
      title: switch (c.pane) {
        MdlPane.queue => 'Queue',
        MdlPane.downloaded => 'Downloaded',
        MdlPane.searches => 'Searches',
      },
      trailing: _Segmented(
        options: const ['queue', 'downloaded', 'searches'],
        labels: [
          'Queue${state.rows.isEmpty ? "" : " ${state.rows.length}"}',
          'Downloaded',
          'Searches',
        ],
        active: c.pane.name,
        onPick: (v) => c.setPane(MdlPane.values.byName(v)),
      ),
      child: switch (c.pane) {
        MdlPane.queue => _Queue(controller: c, state: state),
        MdlPane.downloaded => _History(controller: c, state: state),
        MdlPane.searches => _Searches(controller: c, state: state),
      },
    );
  }
}

class _Queue extends StatelessWidget {
  const _Queue({required this.controller, required this.state});

  final MdlController controller;
  final MdlState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = state;
    if (st.rows.isEmpty) {
      return const MusicEmpty(
        icon: Icons.download_outlined,
        title: 'Nothing queued',
        body: 'Paste an album, playlist or track link above — or switch to '
            'Search and type what you want. Everything that comes back lands '
            'here for you to pick over before anything is downloaded.',
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _QueueBar(controller: controller, state: st),
        const SizedBox(height: 10),
        // The card is as tall as the column now, so the list takes what is
        // left of it. It used to be capped at 520 and shrink-wrapped, because
        // it sat in a page-length scroll view where an unbounded list would
        // have made the page taller than the window.
        Expanded(
          child: ListView.separated(
            itemCount: st.rows.length,
            separatorBuilder: (_, __) => Divider(height: 1, color: t.nHair),
            itemBuilder: (context, i) => _Row(
              controller: controller,
              row: st.rows[i],
              index: i,
              allArtists: st.allArtists,
            ),
          ),
        ),
      ],
    );
  }
}

/// Select-all, the bulk primary-artist menu, the sort keys, and Download.
class _QueueBar extends StatelessWidget {
  const _QueueBar({required this.controller, required this.state});

  final MdlController controller;
  final MdlState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = state;
    final all = st.selected == st.rows.length;
    return Wrap(
      spacing: 10,
      runSpacing: 8,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: [
        TextButton.icon(
          onPressed: () => controller.send(MdlCmd.selectAll(all: !all)),
          icon: Icon(all ? Icons.deselect : Icons.select_all, size: 16),
          label: Text(all ? 'None' : 'All'),
        ),
        Text('${st.selected} of ${st.rows.length} selected',
            style: TextStyle(fontSize: 12, color: t.textDim)),
        if (st.allArtists.length > 1)
          _Pick(
            label: 'Primary artist, everywhere',
            value: st.allArtists.first,
            options: st.allArtists,
            onPick: (v) => controller.send(MdlCmd.bulkMainArtist(artist: v)),
          ),
        for (final key in const ['name', 'length', 'artist'])
          MusicChip(
            label: switch (key) {
              'name' => 'Title',
              'length' => 'Length',
              _ => 'Artist',
            },
            active: st.sort == key,
            // The direction is only meaningful on the active key, so it only
            // shows there.
            icon: st.sort != key
                ? null
                : st.sortDir == 1
                    ? Icons.arrow_upward
                    : Icons.arrow_downward,
            onTap: () => controller.send(MdlCmd.sortBy(key: key)),
          ),
        // Download and Cancel are in the header strip, with the counters and
        // the progress they belong to. This row is what you do *to the list*.
      ],
    );
  }
}

class _Row extends StatelessWidget {
  const _Row({
    required this.controller,
    required this.row,
    required this.index,
    required this.allArtists,
  });

  final MdlController controller;
  final MdlRow row;
  final int index;
  final List<String> allArtists;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = stageColor(row.stage);
    final active = row.percent > 0 && row.percent < 100;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 7),
      child: Row(
        children: [
          Checkbox(
            value: row.selected,
            activeColor: kMdlAccent,
            visualDensity: VisualDensity.compact,
            onChanged: (_) => controller.send(MdlCmd.toggleRow(index: index)),
          ),
          _Art(url: row.artUrl),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(row.title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w600,
                        color: t.text)),
                const SizedBox(height: 2),
                Text(
                  [
                    if (row.mainArtist.isNotEmpty) row.mainArtist,
                    if (row.album.isNotEmpty) row.album,
                    if (row.length.isNotEmpty) row.length,
                  ].join(' · '),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11.5, color: t.textDim),
                ),
                if (row.file.isNotEmpty) ...[
                  const SizedBox(height: 2),
                  Text(row.file,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                          fontSize: 11,
                          color: row.stage == 'failed' ? Tokens.error : tint)),
                ],
              ],
            ),
          ),
          // Only a track credited to more than one artist needs the menu.
          if (row.artists.length > 1) ...[
            const SizedBox(width: 8),
            _ArtistMenu(controller: controller, row: row, index: index),
          ],
          const SizedBox(width: 10),
          SizedBox(
            width: 96,
            child: active
                ? LinearProgressIndicator(
                    value: row.percent / 100,
                    color: tint,
                    backgroundColor: tint.withValues(alpha: 0.18),
                  )
                : _StagePill(stage: row.stage, tint: tint),
          ),
          if (row.stage == 'failed')
            IconButton(
              tooltip: 'Try again',
              icon: const Icon(Icons.refresh, size: 17),
              onPressed: () => controller.send(MdlCmd.retry(index: index)),
            ),
        ],
      ),
    );
  }
}

class _StagePill extends StatelessWidget {
  const _StagePill({required this.stage, required this.tint});

  final String stage;
  final Color tint;

  @override
  Widget build(BuildContext context) => Container(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
        decoration: BoxDecoration(
          color: tint.withValues(alpha: 0.13),
          borderRadius: BorderRadius.circular(999),
          border: Border.all(color: tint.withValues(alpha: 0.4)),
        ),
        child: Text(stage,
            textAlign: TextAlign.center,
            style: TextStyle(
                fontSize: 10.5, fontWeight: FontWeight.w700, color: tint)),
      );
}

/// Which of a track's credited artists the library should file it under. The
/// full credit list still goes into the file's own tags.
class _ArtistMenu extends StatelessWidget {
  const _ArtistMenu({
    required this.controller,
    required this.row,
    required this.index,
  });

  final MdlController controller;
  final MdlRow row;
  final int index;

  @override
  Widget build(BuildContext context) => PopupMenuButton<String>(
        tooltip: 'File under',
        initialValue: row.mainArtist,
        onSelected: (v) =>
            controller.send(MdlCmd.setMainArtist(index: index, artist: v)),
        itemBuilder: (context) => [
          for (final a in row.artists) PopupMenuItem(value: a, child: Text(a)),
        ],
        child: const Icon(Icons.person_outline, size: 17),
      );
}

/// Cover art, straight from the provider's URL. Flutter caches it, so the
/// bytes never cross the bridge.
class _Art extends StatelessWidget {
  const _Art({required this.url});

  final String url;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final plate = Container(
      width: 34,
      height: 34,
      decoration: BoxDecoration(
        color: t.nTile,
        borderRadius: BorderRadius.circular(6),
      ),
      child: Icon(Icons.music_note, size: 16, color: t.textDim),
    );
    if (url.isEmpty) return plate;
    return ClipRRect(
      borderRadius: BorderRadius.circular(6),
      child: Image.network(
        url,
        width: 34,
        height: 34,
        fit: BoxFit.cover,
        errorBuilder: (_, __, ___) => plate,
      ),
    );
  }
}

class _History extends StatelessWidget {
  const _History({required this.controller, required this.state});

  final MdlController controller;
  final MdlState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = state;
    if (st.history.isEmpty) {
      return const MusicEmpty(
        icon: Icons.history,
        title: 'Nothing downloaded yet',
        body: 'Every track this downloader finishes is logged here with where '
            'it came from and where it landed.',
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Scrolls inside the card. The pane is as tall as the column now, and
        // twenty-five rows plus a pager is taller than that on a short window.
        Expanded(
          child: ListView(
            padding: EdgeInsets.zero,
            children: [
              for (final r in st.history)
                ListTile(
                  dense: true,
                  contentPadding: EdgeInsets.zero,
                  title: Text(r.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 13, color: t.text)),
                  subtitle: Text(
                    [
                      r.artists,
                      if (r.album.isNotEmpty) r.album,
                      if (r.provider.isNotEmpty) r.provider,
                      r.when,
                    ].join(' · '),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(fontSize: 11.5, color: t.textDim),
                  ),
                  trailing: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      IconButton(
                        tooltip: 'Play',
                        icon: const Icon(Icons.play_arrow, size: 18),
                        // -1 means the file is no longer in the library — the row
                        // stays, but there is nothing to play.
                        onPressed: r.itemId < 0
                            ? null
                            : () => MusicController.instance.send(
                                  MusicCmd.playList(
                                    itemIds: Int64List.fromList([r.itemId]),
                                    index: 0,
                                    source: 'downloads',
                                  ),
                                ),
                      ),
                      IconButton(
                        tooltip: 'Show the file',
                        icon: const Icon(Icons.folder_open, size: 18),
                        onPressed: () =>
                            controller.send(MdlCmd.revealFile(path: r.path)),
                      ),
                    ],
                  ),
                ),
            ],
          ),
        ),
        const SizedBox(height: 6),
        Row(
          children: [
            Pager(
              page: st.historyPage,
              pages: st.historyPages,
              onGo: (p) => controller.send(MdlCmd.loadHistory(page: p)),
            ),
            const Spacer(),
            TextButton(
              onPressed: () => controller.send(const MdlCmd.clearHistory()),
              child: const Text('Clear log'),
            ),
          ],
        ),
      ],
    );
  }
}

class _Searches extends StatelessWidget {
  const _Searches({required this.controller, required this.state});

  final MdlController controller;
  final MdlState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final st = state;
    if (st.searches.isEmpty) {
      return const MusicEmpty(
        icon: Icons.manage_search,
        title: 'No searches yet',
        body: 'Every link you resolve is remembered here, so getting back to '
            'an album you looked at last week is one tap.',
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(
          child: ListView(
            padding: EdgeInsets.zero,
            children: [
              for (final r in st.searches)
                ListTile(
                  dense: true,
                  contentPadding: EdgeInsets.zero,
                  onTap: () => controller.send(MdlCmd.useSearch(url: r.url)),
                  title: Text(r.title.isEmpty ? r.url : r.title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 13, color: t.text)),
                  subtitle: Text(
                    [
                      r.kind,
                      if (r.provider.isNotEmpty) r.provider,
                      r.when,
                    ].join(' · '),
                    style: TextStyle(fontSize: 11.5, color: t.textDim),
                  ),
                  trailing: const Icon(Icons.north_east, size: 16),
                ),
            ],
          ),
        ),
        const SizedBox(height: 6),
        Row(
          children: [
            Pager(
              page: st.searchPage,
              pages: st.searchPages,
              onGo: (p) => controller.send(MdlCmd.loadSearches(page: p)),
            ),
            const Spacer(),
            TextButton(
              onPressed: () => controller.send(const MdlCmd.clearSearches()),
              child: const Text('Clear log'),
            ),
          ],
        ),
      ],
    );
  }
}

// ── activity log ────────────────────────────────────────────────────────────

/// What it is doing, in the words yt-dlp and the provider used. Collapsed by
/// default: this is the thing you open when a track failed and the pill only
/// had room for the first eighty characters of why.
class _Cli extends StatelessWidget {
  const _Cli({required this.controller});

  final MdlController controller;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final lines = controller.cli;
    return Container(
      decoration: BoxDecoration(
        color: t.panel,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: t.outline),
      ),
      clipBehavior: Clip.antiAlias,
      child: ExpansionTile(
        title: Text('Activity',
            style: TextStyle(
                fontSize: 13.5, fontWeight: FontWeight.w700, color: t.text)),
        subtitle: Text(
          lines.isEmpty ? 'Nothing yet' : lines.last,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: TextStyle(
              fontSize: 11.5, color: t.textDim, fontFamily: 'monospace'),
        ),
        children: [
          Container(
            constraints: const BoxConstraints(maxHeight: 260),
            width: double.infinity,
            color: t.nCanvas,
            padding: const EdgeInsets.all(12),
            child: lines.isEmpty
                ? Text('Resolve something and the log fills in.',
                    style: TextStyle(fontSize: 12, color: t.textDim))
                : ListView.builder(
                    shrinkWrap: true,
                    reverse: true,
                    itemCount: lines.length,
                    itemBuilder: (context, i) => Text(
                      lines[lines.length - 1 - i],
                      style: TextStyle(
                          fontSize: 11.5,
                          height: 1.5,
                          fontFamily: 'monospace',
                          color: t.text),
                    ),
                  ),
          ),
          if (lines.isNotEmpty)
            Align(
              alignment: Alignment.centerRight,
              child: TextButton(
                onPressed: controller.clearCli,
                child: const Text('Clear'),
              ),
            ),
        ],
      ),
    );
  }
}
