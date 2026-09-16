// YouTube › Cached: the copies streaming leaves behind, and the rules for them.
//
// Everything here is disposable by design -- the cache refills itself -- so
// the page is mostly about space: how much, of what, against what budget, and
// what goes first when it is full. Anything worth keeping moves to Downloads,
// where eviction never reaches it.

import 'package:flutter/material.dart';

import '../../../design/tokens.dart';
import '../../../src/rust/api/music.dart';
import '../music_controller.dart';
import '../music_dialogs.dart';
import '../music_widgets.dart';
import 'yt_card.dart';

/// Audio on the bar and in the rows: violet beside the tab's rose for video,
/// so the two kinds read apart in both themes.
const Color _audioColor = Color(0xFF8B5CF6);
const Color _thumbColor = Color(0xFFF59E0B);

/// The cached videos the All / Audio / Video filter keeps.
List<YtVideo> ytCachedOfKind(MusicState st, String kind) => st.ytCached
    .where((v) => switch (kind) {
          'audio' => ytOfflineKind(v) != 'video',
          'video' => ytOfflineKind(v) == 'video',
          _ => true,
        })
    .toList();

class YtCached extends StatefulWidget {
  const YtCached({
    super.key,
    required this.controller,
    required this.st,
    required this.kind,
    required this.onKind,
    required this.page,
  });

  final MusicController controller;
  final MusicState st;

  /// all | audio | video. Kept by the tab, whose header pages this list.
  final String kind;
  final ValueChanged<String> onKind;
  final int page;

  @override
  State<YtCached> createState() => _YtCachedState();
}

class _YtCachedState extends State<YtCached> {
  Future<YtStorage>? _storage;
  String _storageKey = '';

  void _setPolicy({String? mode, int? cap, String? evict, int? idle}) {
    final p = widget.st.ytCachePolicy;
    widget.controller.send(MusicCmd.ytSetCachePolicy(
      mode: mode ?? p.mode,
      capBytes: cap ?? p.capBytes,
      evict: evict ?? p.evict,
      idleDays: idle ?? p.idleDays,
    ));
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = widget.controller;
    final st = widget.st;
    final p = st.ytCachePolicy;
    final rowBytes = st.ytCached.fold<int>(0, (a, v) => a + (v.bytes > 0 ? v.bytes : 0));
    final key = '${st.ytCached.length}|$rowBytes|${p.capBytes}';
    if (key != _storageKey) {
      _storageKey = key;
      _storage = musicYtStorage();
    }
    final audio = st.ytCached.where((v) => ytOfflineKind(v) != 'video').length;
    final video = st.ytCached.length - audio;
    final shown = ytCachedOfKind(st, widget.kind)
        .skip(widget.page * ytPerPage)
        .take(ytPerPage);

    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 12, 24, 32),
      children: [
        LayoutBuilder(builder: (context, box) {
          final meter = _Meter(storage: _storage);
          final policy = _Policy(policy: p, onChange: _setPolicy);
          if (box.maxWidth < 820) {
            return Column(children: [meter, const SizedBox(height: 12), policy]);
          }
          return IntrinsicHeight(
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Expanded(flex: 3, child: meter),
                const SizedBox(width: 12),
                Expanded(flex: 2, child: policy),
              ],
            ),
          );
        }),
        Padding(
          padding: const EdgeInsets.fromLTRB(0, 24, 0, 10),
          child: Wrap(
            spacing: 8,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              Text('Cached',
                  style: TextStyle(
                      fontSize: 17,
                      fontWeight: FontWeight.w700,
                      letterSpacing: -0.3,
                      color: t.nInk)),
              const SizedBox(width: 6),
              for (final (k, label) in [
                ('all', 'All ${st.ytCached.length}'),
                ('audio', 'Audio $audio'),
                ('video', 'Video $video'),
              ])
                SortChip(
                  label: label,
                  active: widget.kind == k,
                  onTap: () => widget.onKind(k),
                ),
              const SizedBox(width: 6),
              TextButton.icon(
                icon: const Icon(Icons.history_toggle_off, size: 16),
                label: const Text('Clear unplayed 30 d'),
                onPressed: st.ytCached.isEmpty
                    ? null
                    : () => c.send(const MusicCmd.ytClearUnplayed(days: 30)),
              ),
              TextButton.icon(
                icon: const Icon(Icons.cleaning_services_outlined, size: 16),
                label: const Text('Clean up now'),
                onPressed: st.ytCached.isEmpty
                    ? null
                    : () => c.send(const MusicCmd.ytCleanCache()),
              ),
              TextButton.icon(
                icon: const Icon(Icons.delete_sweep_outlined, size: 16),
                label: const Text('Empty the cache'),
                onPressed: st.ytCached.isEmpty
                    ? null
                    : () async {
                        final ok = await confirm(
                          context,
                          title: 'Empty the cache?',
                          body: 'Every cached file is deleted. Anything you play '
                              'again is simply pulled down again. Downloads are '
                              'not touched.',
                          action: 'Empty',
                        );
                        if (ok) await c.send(const MusicCmd.ytClearCached());
                      },
              ),
            ],
          ),
        ),
        if (st.ytCached.isEmpty)
          MusicEmpty(
            icon: Icons.offline_pin_outlined,
            title: p.mode == 'never' ? 'Caching is off' : 'Nothing cached yet',
            body: p.mode == 'never'
                ? 'Turn it back on above and what you play is kept for next time.'
                : 'What you play is pulled down in the background, so the '
                    'second listen is local and works offline.',
          )
        else
          YtGrid(padding: 0, children: [
            for (final v in shown) _CachedCard(controller: c, video: v),
          ]),
      ],
    );
  }
}

class _Panel extends StatelessWidget {
  const _Panel({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return DecoratedBox(
      decoration: BoxDecoration(
        color: t.nCard,
        border: Border.all(color: t.nHair),
        borderRadius: BorderRadius.circular(Tokens.radiusLg),
      ),
      child: Padding(padding: const EdgeInsets.all(18), child: child),
    );
  }
}

class _Meter extends StatelessWidget {
  const _Meter({required this.storage});

  final Future<YtStorage>? storage;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return _Panel(
      child: FutureBuilder<YtStorage>(
        future: storage,
        builder: (context, snap) {
          final s = snap.data;
          if (s == null) return const SizedBox(height: 96);
          final used = s.cacheAudioBytes + s.cacheVideoBytes;
          final cap = s.capBytes <= 0 ? 1 : s.capBytes;
          int flex(int b) => (b * 1000 / cap).round();
          final free = (cap - used).clamp(0, cap);
          const tabular = [FontFeature.tabularFigures()];
          return Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Wrap(
                spacing: 10,
                crossAxisAlignment: WrapCrossAlignment.end,
                children: [
                  Text(ytSize(used),
                      style: TextStyle(
                          fontSize: 30,
                          fontWeight: FontWeight.w800,
                          letterSpacing: -1,
                          color: t.nInk,
                          fontFeatures: tabular)),
                  Padding(
                    padding: const EdgeInsets.only(bottom: 6),
                    child: Text('of ${ytSize(cap)} cache budget',
                        style: TextStyle(fontSize: 13, color: t.nInk3)),
                  ),
                ],
              ),
              const SizedBox(height: 12),
              ClipRRect(
                borderRadius: BorderRadius.circular(6),
                child: SizedBox(
                  height: 12,
                  child: Row(
                    children: [
                      if (s.cacheVideoBytes > 0)
                        Expanded(flex: flex(s.cacheVideoBytes).clamp(1, 1000), child: Container(color: ytRose)),
                      if (s.cacheAudioBytes > 0)
                        Expanded(flex: flex(s.cacheAudioBytes).clamp(1, 1000), child: Container(color: _audioColor)),
                      if (s.thumbsBytes > 0)
                        Expanded(flex: flex(s.thumbsBytes).clamp(1, 1000), child: Container(color: _thumbColor)),
                      if (free > 0)
                        Expanded(flex: flex(free).clamp(1, 1000), child: Container(color: t.nChip)),
                    ],
                  ),
                ),
              ),
              const SizedBox(height: 10),
              Wrap(
                spacing: 18,
                runSpacing: 6,
                children: [
                  _Legend(color: ytRose, label: 'Video', value: ytSize(s.cacheVideoBytes)),
                  _Legend(color: _audioColor, label: 'Audio', value: ytSize(s.cacheAudioBytes)),
                  // Shown, not budgeted: eviction removes media only.
                  _Legend(
                    color: _thumbColor,
                    label: 'Thumbnails, outside the budget',
                    value: ytSize(s.thumbsBytes),
                  ),
                  _Legend(
                    color: t.nInk3,
                    label: 'Downloads, kept apart',
                    value: ytSize(s.downloadsAudioBytes + s.downloadsVideoBytes),
                  ),
                ],
              ),
            ],
          );
        },
      ),
    );
  }
}

class _Legend extends StatelessWidget {
  const _Legend({required this.color, required this.label, required this.value});

  final Color color;
  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Container(
          width: 10,
          height: 10,
          decoration: BoxDecoration(color: color, borderRadius: BorderRadius.circular(3)),
        ),
        const SizedBox(width: 6),
        Text('$label  ', style: TextStyle(fontSize: 12, color: t.nInk2)),
        Text(value,
            style: TextStyle(
                fontSize: 12,
                color: t.nInk,
                fontFeatures: const [FontFeature.tabularFigures()])),
      ],
    );
  }
}

class _Policy extends StatelessWidget {
  const _Policy({required this.policy, required this.onChange});

  final YtCachePolicy policy;
  final void Function({String? mode, int? cap, String? evict, int? idle}) onChange;

  static const _gb = 1024 * 1024 * 1024;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final p = policy;
    // The saved cap may be a value the menu does not list (set by hand, or by
    // an older build); it stays selectable rather than silently changing.
    final caps = {1, 2, 5, 10, 20, 50}.map((g) => g * _gb).toSet()..add(p.capBytes);
    Widget row(String label, Widget control) => Padding(
          padding: const EdgeInsets.symmetric(vertical: 3),
          child: Row(
            children: [
              Expanded(child: Text(label, style: TextStyle(fontSize: 13, color: t.nInk2))),
              control,
            ],
          ),
        );
    DropdownButton<T> menu<T>(T value, Map<T, String> items, ValueChanged<T> onPick) =>
        DropdownButton<T>(
          value: value,
          isDense: true,
          underline: const SizedBox.shrink(),
          style: TextStyle(fontSize: 13, color: t.nInk),
          dropdownColor: t.modalSolid,
          items: [
            for (final e in items.entries)
              DropdownMenuItem(value: e.key, child: Text(e.value)),
          ],
          onChanged: (v) {
            if (v != null) onPick(v);
          },
        );

    return _Panel(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          row(
            'Cache while streaming',
            menu(p.mode, const {'audio': 'Audio only', 'all': 'Audio and video', 'never': 'Never'},
                (v) => onChange(mode: v)),
          ),
          row(
            'Budget',
            menu(p.capBytes, {for (final b in caps.toList()..sort()) b: ytSize(b)},
                (v) => onChange(cap: v)),
          ),
          row(
            'When full, remove',
            menu(p.evict, const {
              'lru': 'Least recently played',
              'oldest': 'Oldest first',
              'largest': 'Largest first',
            }, (v) => onChange(evict: v)),
          ),
          row(
            'Remove if unplayed for',
            menu(p.idleDays, {
              7: '7 days',
              30: '30 days',
              90: '90 days',
              // A value set elsewhere stays in the menu, or the dropdown
              // would have no item for its own value.
              if (![7, 30, 90, 0].contains(p.idleDays)) p.idleDays: '${p.idleDays} days',
              0: 'Never',
            }, (v) => onChange(idle: v)),
          ),
        ],
      ),
    );
  }
}

/// A cached copy as a card: what it is kept as, its size and since when under
/// the title; keeping it for good or dropping it in the menu.
class _CachedCard extends StatelessWidget {
  const _CachedCard({required this.controller, required this.video});

  final MusicController controller;
  final YtVideo video;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    final v = video;
    return YtVideoCard(
      controller: c,
      video: v,
      // "Kept as OPUS audio · 4.6 MB · cached 3 days ago".
      subtitle: [
        if (v.quality.isNotEmpty) 'Kept as ${v.quality}',
        if (v.bytes >= 0) ytSize(v.bytes),
        if (v.meta.isNotEmpty) v.meta,
      ].join(' · '),
      extraMenu: [
        (
          'Move to Downloads',
          () => c.send(MusicCmd.ytPromoteCached(videoId: v.videoId))
        ),
        (
          'Remove from the cache',
          () => c.send(MusicCmd.ytRemoveCached(videoId: v.videoId))
        ),
      ],
    );
  }
}
