// Settings — nine tabs over one Settings file.
//
// A left rail of tabs, with a search that looks through every tab's words,
// and one content stage. Each tab lays the rows Rust sends into tiles
// (settings_profile.dart, settings_media.dart, settings_system.dart, and the
// Status page); the rail, the search and the notices are here.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/app_mark.dart';
import '../../design/first_load.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/shell_controller.dart';
import '../../src/rust/api/settings.dart';
import '../status/status_page.dart';
import 'settings_controller.dart';
import 'settings_media.dart';
import 'settings_profile.dart';
import 'settings_system.dart';

class SettingsPage extends StatefulWidget {
  const SettingsPage({super.key, required this.visible});

  final bool visible;

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends State<SettingsPage> {
  final SettingsController _c = SettingsController();
  final TextEditingController _search = TextEditingController();
  final FocusNode _searchFocus = FocusNode();
  String _tab = 'profile';

  /// What the search finds on You & Home and Status, whose words are not
  /// rows. The other tabs are searched through their rows.
  static const _profileWords = [
    'Name',
    'Profile photo',
    'Cover picture',
    'Theme',
    'Light and dark',
    'Reduce motion',
    'Design language',
    'Sidebar logo',
    'Mini player',
    'Home cards',
    'Home layout',
  ];
  static const _statusWords = [
    'Health',
    'Sections',
    'Scans',
    'Today',
    'What happened',
    'System',
    'Rescan',
  ];

  @override
  void initState() {
    super.initState();
    // The sidebar's lamp and Home's status pill open a tab, not just the
    // section: they mean Status, whichever tab was left up.
    ShellController.instance.onOpen(Section.settings, _go);
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _c.visible = widget.visible;
      if (widget.visible) _c.refresh();
    });
  }

  @override
  void didUpdateWidget(SettingsPage old) {
    super.didUpdateWidget(old);
    _c.visible = widget.visible;
    // Tool probes, cache sizes and the watched list all go stale while you are
    // elsewhere, so arriving is when to re-read them.
    if (widget.visible && !old.visible) _c.refresh();
  }

  @override
  void dispose() {
    _c.dispose();
    _search.dispose();
    _searchFocus.dispose();
    super.dispose();
  }

  /// Open a tab. You & Home stages its edits until Save, so leaving it with
  /// some pending asks first rather than dropping them without a word.
  Future<void> _go(String v) async {
    if (v == _tab) return;
    if (_tab == 'profile' && _c.profileDirty.value) {
      final leave = await showDialog<bool>(
        context: context,
        builder: (context) => AlertDialog(
          title: const Text('Leave without saving?'),
          content: const Text(
              'Your changes on You & Home are not saved yet. Leaving the tab '
              'throws them away.'),
          actions: [
            TextButton(
              onPressed: () => Navigator.pop(context, false),
              child: const Text('Stay'),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context, true),
              child: const Text('Discard changes'),
            ),
          ],
        ),
      );
      if (leave != true || !mounted) return;
      _c.profileDirty.value = false;
    }
    setState(() => _tab = v);
    _c.send(SettingsCmd.setTab(tab: v));
  }

  static List<String> _rowWords(List<SettingItem> rows) => [
        for (final r in rows)
          if (r.kind != 'header') '${r.label} ${r.desc}',
      ];

  List<String> _words(String tab, SettingsState st) => switch (tab) {
        'profile' => _profileWords,
        'libraries' => [
            'Watched folders',
            'Watch a folder',
            'Rescan all',
            'Rescan',
            'Thumbnails',
            'Rebuild search index',
            'Stop watching',
            'My Music',
            'Podcasts',
            'Audiobooks',
            'Radio',
            'YouTube',
            for (final l in st.libraries) l.path,
          ],
        'security' => [..._rowWords(st.security), 'Lock now'],
        // Scrobbling lives on Services; its token row is sent with AI's.
        'services' => [
            ..._rowWords(st.services),
            for (final k in st.keys) '${k.label} key',
            'API keys',
            'Scrobbling',
            'ListenBrainz token',
          ],
        'advanced' => [
            ..._rowWords(st.advanced),
            'This build',
            'Version',
            'Credits',
            'Supported file formats',
          ],
        'ai' => [..._rowWords(st.ai), 'Check for updates', 'Models'],
        'data' => [
            ..._rowWords(st.data),
            'Backups',
            'Back up now',
            'Restore',
            'Export the library',
            'Import from another app',
            'Picasa',
            'iTunes',
            'Start over',
            'Reset Tulipix',
            'Data folder',
            'Logs',
          ],
        'status' => _statusWords,
        _ => _rowWords(_c.rowsFor(tab)),
      };

  /// Per tab, how many of its words hold the query. Empty when there is no
  /// query, which is what says the rail is not searching.
  Map<String, int> _hits(SettingsState? st) {
    final q = _search.text.trim().toLowerCase();
    if (q.isEmpty || st == null) return const {};
    return {
      for (final tab in kSettingsTabs)
        tab.id: _words(tab.id, st)
            .where((w) => w.toLowerCase().contains(q))
            .length,
    };
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      // The focus too: the search field's outline follows it.
      animation: Listenable.merge([_c, _search, _searchFocus]),
      builder: (context, _) {
        final st = _c.state;
        final hits = _hits(st);
        return CallbackShortcuts(
          bindings: {
            const SingleActivator(LogicalKeyboardKey.keyF, control: true): () =>
                _searchFocus.requestFocus(),
          },
          child: ColoredBox(
            color: t.bg,
            child: Row(
              children: [
                _Rail(
                  tab: _tab,
                  controller: _c,
                  onTab: _go,
                  search: _search,
                  searchFocus: _searchFocus,
                  hits: hits,
                ),
                Container(width: 1, color: t.outline),
                Expanded(
                  child: st == null
                      ? FirstLoad(error: _c.error, onRetry: _c.refresh)
                      : Column(
                          children: [
                            if (_c.error != null)
                              _Bar(
                                text: '${_c.error}',
                                tint: Tokens.error,
                                onClose: _c.clearError,
                              ),
                            if (_c.notice.isNotEmpty)
                              _Bar(
                                text: _c.notice,
                                tint: Tokens.secSettings,
                                onClose: _c.clearNotice,
                              ),
                            Expanded(child: _stage(st)),
                          ],
                        ),
                ),
              ],
            ),
          ),
        );
      },
    );
  }

  Widget _stage(SettingsState st) => switch (_tab) {
        // The status pill on the identity row goes to the Status tab, so the
        // page that owns the tab has to hand it the way back.
        'profile' => ProfileTab(controller: _c, state: st, onTab: _go),
        'libraries' => LibrariesTab(controller: _c, state: st),
        'playback' => PlaybackTab(controller: _c, state: st),
        'services' => ServicesTab(controller: _c, state: st),
        'ai' => AiTab(controller: _c, state: st),
        'security' => SecurityTab(controller: _c, state: st),
        'data' => DataTab(controller: _c, state: st),
        'advanced' => AdvancedTab(controller: _c, state: st),
        // The dashboard, as its own tab — the same page the sidebar's lamp
        // reports on, not a second rendering of it.
        _ => StatusPage(visible: widget.visible),
      };
}

class _Rail extends StatelessWidget {
  const _Rail({
    required this.tab,
    required this.controller,
    required this.onTab,
    required this.search,
    required this.searchFocus,
    required this.hits,
  });

  final String tab;
  final SettingsController controller;
  final ValueChanged<String> onTab;
  final TextEditingController search;
  final FocusNode searchFocus;
  final Map<String, int> hits;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final searching = hits.isNotEmpty;
    return SizedBox(
      width: 224,
      child: ValueListenableBuilder(
        valueListenable: controller.profileDirty,
        builder: (context, dirty, _) => Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(18, 16, 18, 10),
              child: Text('Settings',
                  style: TextStyle(
                      fontSize: 19,
                      fontWeight: FontWeight.w800,
                      color: t.text)),
            ),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 10),
              child: _SearchField(
                controller: search,
                focus: searchFocus,
                // Enter goes to the first tab that has it.
                onSubmit: () {
                  for (final x in kSettingsTabs) {
                    if ((hits[x.id] ?? 0) > 0) {
                      onTab(x.id);
                      return;
                    }
                  }
                },
              ),
            ),
            const SizedBox(height: 10),
            Expanded(
              child: ListView(
                padding: const EdgeInsets.symmetric(horizontal: 10),
                children: [
                  for (final item in kSettingsTabs)
                    _RailItem(
                      label: item.label,
                      icon: item.icon,
                      tint: item.tint,
                      active: tab == item.id,
                      unsaved: dirty && item.id == 'profile',
                      hits: searching ? hits[item.id] ?? 0 : null,
                      onTap: () => onTab(item.id),
                    ),
                ],
              ),
            ),
            // The app's own card, as Advanced › This build draws it: the mark
            // chosen for the sidebar, the version, and the byline.
            Container(
              margin: const EdgeInsets.fromLTRB(10, 6, 10, 12),
              padding: const EdgeInsets.all(10),
              decoration: BoxDecoration(
                color: t.bg,
                borderRadius: BorderRadius.circular(12),
                border: Border.all(color: t.outline),
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      AppMark(
                        size: 36,
                        radius: 9,
                        choice:
                            ShellController.instance.state?.logoChoice ?? 0,
                      ),
                      const SizedBox(width: 10),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text('Tulipix',
                                style: TextStyle(
                                    fontSize: 15,
                                    fontWeight: FontWeight.w700,
                                    letterSpacing: -0.1,
                                    color: t.text)),
                            Text(
                                'Version '
                                '${controller.state?.appVersion ?? '—'}',
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(
                                    fontSize: 11, color: t.textDim)),
                          ],
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 8),
                  Text('© 2026 — Developed by Atish',
                      style: TextStyle(fontSize: 10.5, color: t.textDim)),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _SearchField extends StatelessWidget {
  const _SearchField({
    required this.controller,
    required this.focus,
    required this.onSubmit,
  });

  final TextEditingController controller;
  final FocusNode focus;
  final VoidCallback onSubmit;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.escape): controller.clear,
      },
      child: Container(
        height: 36,
        padding: const EdgeInsets.only(left: 10),
        decoration: BoxDecoration(
          color: t.bg,
          borderRadius: BorderRadius.circular(10),
          border: Border.all(
              color: focus.hasFocus ? Tokens.brand : t.outline),
        ),
        child: Row(
          children: [
            Icon(Icons.search, size: 16, color: t.textDim),
            const SizedBox(width: 8),
            Expanded(
              child: TextField(
                controller: controller,
                focusNode: focus,
                onSubmitted: (_) => onSubmit(),
                style: TextStyle(fontSize: 12.5, color: t.text),
                decoration: InputDecoration.collapsed(
                  hintText: 'Search settings',
                  hintStyle: TextStyle(fontSize: 12.5, color: t.textDim),
                ),
              ),
            ),
            if (controller.text.isNotEmpty)
              IconButton(
                tooltip: 'Clear',
                visualDensity: VisualDensity.compact,
                iconSize: 15,
                onPressed: controller.clear,
                icon: Icon(Icons.close, color: t.textDim),
              ),
          ],
        ),
      ),
    );
  }
}

class _RailItem extends StatelessWidget {
  const _RailItem({
    required this.label,
    required this.icon,
    required this.tint,
    required this.active,
    required this.onTap,
    this.unsaved = false,
    this.hits,
  });

  final String label;
  final IconData icon;
  final Color tint;
  final bool active;
  final VoidCallback onTap;

  /// An amber dot: this tab has edits that are not saved yet.
  final bool unsaved;

  /// While searching, how many of this tab's settings match; null otherwise.
  /// None dims the tab.
  final int? hits;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final n = hits;
    return Opacity(
      opacity: n == 0 ? 0.35 : 1,
      child: Padding(
        padding: const EdgeInsets.only(bottom: 4),
        child: Material(
          // Under a skin the open item is the skin's latched key, drawn below.
          color: active && context.skin.isStandard
              ? Tokens.secSettings.withValues(alpha: 0.16)
              : Colors.transparent,
          borderRadius: BorderRadius.circular(10),
          child: InkWell(
            borderRadius: BorderRadius.circular(10),
            onTap: onTap,
            child: Container(
              decoration: active
                  ? context.skin.control(
                      active: true, tint: Tokens.secSettings, radius: 10)
                  : null,
              padding: const EdgeInsets.symmetric(horizontal: 11, vertical: 10),
              child: Row(
                children: [
                  Icon(context.skin.icon(icon),
                      size: 17, color: active ? tint : t.textDim),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Text(label,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 12.5,
                            fontWeight:
                                active ? FontWeight.w700 : FontWeight.w500,
                            color: active ? t.text : t.textDim)),
                  ),
                  if (n != null && n > 0)
                    Container(
                      padding: const EdgeInsets.symmetric(
                          horizontal: 6, vertical: 1),
                      decoration: BoxDecoration(
                        color: Tokens.brand.withValues(alpha: 0.18),
                        borderRadius: BorderRadius.circular(6),
                      ),
                      child: Text('$n',
                          style: const TextStyle(
                              fontSize: 10.5,
                              fontWeight: FontWeight.w700,
                              color: Tokens.brand)),
                    )
                  else if (unsaved)
                    Tooltip(
                      message: 'Unsaved changes',
                      child: Container(
                        width: 7,
                        height: 7,
                        decoration: const BoxDecoration(
                            color: Tokens.warn, shape: BoxShape.circle),
                      ),
                    ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _Bar extends StatelessWidget {
  const _Bar({required this.text, required this.tint, required this.onClose});

  final String text;
  final Color tint;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: tint.withValues(alpha: 0.12),
      child: ListTile(
        dense: true,
        leading: Icon(Icons.info_outline, color: tint, size: 18),
        title: Text(text, style: TextStyle(fontSize: 12.5, color: t.text)),
        trailing: IconButton(
          icon: const Icon(Icons.close, size: 16),
          onPressed: onClose,
        ),
      ),
    );
  }
}
