// The YouTube tab's search box: words, or a pasted link, with suggestions.
//
// Two answers race for the list under the field. The library's (subscribed
// channels, videos already cached or downloaded or listed, playlists, recent
// searches) is SQLite and arrives at once, offline included. YouTube's own
// autocomplete waits 150 ms after the last keystroke and is dropped if a newer
// keystroke has happened since -- so it can only ever add rows, never delay or
// replace the library's.
//
// It lives in the Music header's search pill while YouTube is open, which
// hands it the pill's text and focus; the Offline switch is the list's first
// row.
//
// Keys: `/` from anywhere on the tab, arrows to move, Enter to take the row.
// Escape is Music's: its keyboard handler unfocuses a focused field, and
// losing focus is what closes the list.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../../design/tokens.dart';
import '../../../src/rust/api/music.dart';
import '../music_controller.dart';
import '../music_widgets.dart';
import 'yt_card.dart';

class YtSearchField extends StatefulWidget {
  const YtSearchField({
    super.key,
    required this.controller,
    required this.text,
    required this.focus,
    this.style,
    this.hintStyle,
  });

  final MusicController controller;

  /// The host's text and focus: the header pill's, which draws the field.
  final TextEditingController text;
  final FocusNode focus;
  final TextStyle? style;
  final TextStyle? hintStyle;

  @override
  State<YtSearchField> createState() => _YtSearchFieldState();
}

enum _Kind { query, channel, video, playlist }

class _Item {
  const _Item(this.kind, this.label,
      {this.group = '',
      this.detail = '',
      this.value = '',
      this.video,
      this.sub,
      this.playlist,
      this.icon});

  final _Kind kind;
  final String label;

  /// The heading this row sits under; '' for the top row.
  final String group;
  final String detail;
  final String value;
  final YtVideo? video;
  final YtSub? sub;
  final YtPlaylist? playlist;
  final IconData? icon;
}

class _YtSearchFieldState extends State<YtSearchField> {
  TextEditingController get _text => widget.text;
  FocusNode get _focus => widget.focus;
  final _portal = OverlayPortalController();
  final _link = LayerLink();
  Timer? _debounce;
  int _seq = 0;
  bool _offline = false;
  bool _visible = true;
  double _width = 480;
  YtSuggestLocal? _local;
  List<String> _remote = const [];
  String _linkKind = '';
  int _index = 0;

  @override
  void initState() {
    super.initState();
    _focus.onKeyEvent = _onKey;
    _focus.addListener(_onFocus);
    HardwareKeyboard.instance.addHandler(_slash);
  }

  void _onFocus() {
    if (_focus.hasFocus) {
      _refresh();
      _portal.show();
    } else {
      _portal.hide();
    }
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    // False while Music is behind another section; `/` then belongs to it.
    _visible = TickerMode.valuesOf(context).enabled;
  }

  @override
  void dispose() {
    HardwareKeyboard.instance.removeHandler(_slash);
    _debounce?.cancel();
    _focus.removeListener(_onFocus);
    _focus.onKeyEvent = null;
    super.dispose();
  }

  bool _slash(KeyEvent e) {
    if (e is! KeyDownEvent || e.character != '/' || !_visible || !mounted) {
      return false;
    }
    final focused = FocusManager.instance.primaryFocus?.context;
    if (focused?.findAncestorStateOfType<EditableTextState>() != null) {
      return false;
    }
    _focus.requestFocus();
    return true;
  }

  KeyEventResult _onKey(FocusNode node, KeyEvent e) {
    if (e is! KeyDownEvent && e is! KeyRepeatEvent) {
      return KeyEventResult.ignored;
    }
    final items = _items();
    switch (e.logicalKey) {
      case LogicalKeyboardKey.arrowDown when items.isNotEmpty:
        setState(() => _index = (_index + 1) % items.length);
        return KeyEventResult.handled;
      case LogicalKeyboardKey.arrowUp when items.isNotEmpty:
        setState(() => _index = (_index - 1 + items.length) % items.length);
        return KeyEventResult.handled;
      case LogicalKeyboardKey.enter || LogicalKeyboardKey.numpadEnter:
        if (items.isNotEmpty) _run(items[_index.clamp(0, items.length - 1)]);
        return KeyEventResult.handled;
      default:
        return KeyEventResult.ignored;
    }
  }

  void _refresh() {
    final q = _text.text.trim();
    final seq = ++_seq;
    _linkKind = q.isEmpty ? '' : musicYtLinkKind(text: q);
    _index = 0;
    musicYtSuggestLocal(query: q, offlineOnly: _offline).then((r) {
      if (mounted && seq == _seq) setState(() => _local = r);
    }, onError: (_) {});
    _debounce?.cancel();
    if (q.isEmpty || _offline || _linkKind.isNotEmpty) {
      setState(() => _remote = const []);
      return;
    }
    _debounce = Timer(const Duration(milliseconds: 150), () async {
      final r = await musicYtSuggestRemote(query: q);
      if (mounted && seq == _seq) setState(() => _remote = r);
    });
    setState(() {});
  }

  List<_Item> _items() {
    final q = _text.text.trim();
    final local = _local;
    final out = <_Item>[];
    if (q.isEmpty) {
      for (final r in local?.recent ?? const <String>[]) {
        out.add(_Item(_Kind.query, r,
            group: 'Recent', value: r, icon: Icons.history));
      }
      return out;
    }
    out.add(_Item(
      _Kind.query,
      _linkKind.isNotEmpty
          ? 'Open this $_linkKind'
          : _offline
              ? 'Search offline copies for “$q”'
              : 'Search YouTube for “$q”',
      value: q,
      icon: _linkKind.isNotEmpty ? Icons.link : Icons.search,
    ));
    if (_linkKind.isNotEmpty) return out;
    for (final s in local?.channels ?? const <YtSub>[]) {
      out.add(_Item(_Kind.channel, s.title,
          group: 'Channels',
          sub: s,
          detail: s.subs > 0 ? '${s.subs} subscribers' : ''));
    }
    for (final v in local?.videos ?? const <YtVideo>[]) {
      out.add(_Item(_Kind.video, v.title,
          group: 'In your library', video: v, detail: v.channel));
    }
    for (final p in local?.playlists ?? const <YtPlaylist>[]) {
      out.add(_Item(_Kind.playlist, p.name,
          group: 'Your playlists',
          playlist: p,
          detail: '${p.count} videos',
          icon: Icons.playlist_play));
    }
    for (final r in local?.recent ?? const <String>[]) {
      out.add(_Item(_Kind.query, r,
          group: 'Recent', value: r, icon: Icons.history));
    }
    for (final s in _remote) {
      if (out.any((i) =>
          i.kind == _Kind.query && i.value.toLowerCase() == s.toLowerCase())) {
        continue;
      }
      out.add(_Item(_Kind.query, s,
          group: 'Suggestions', value: s, icon: Icons.search));
    }
    return out;
  }

  void _run(_Item item) {
    final c = widget.controller;
    switch (item.kind) {
      case _Kind.query:
        _text.text = item.value;
        c.send(MusicCmd.ytSearch(query: item.value, offlineOnly: _offline));
      case _Kind.channel:
        c.send(MusicCmd.ytOpenChannel(channelId: item.sub!.channelId));
      case _Kind.video:
        playYtAudio(c, item.video!);
      case _Kind.playlist:
        c.send(MusicCmd.ytOpenPlaylist(playlistId: item.playlist!.id));
    }
    _focus.unfocus();
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(builder: (context, box) {
      _width = box.maxWidth.isFinite ? box.maxWidth : 480;
      return OverlayPortal(
        controller: _portal,
        overlayChildBuilder: _overlay,
        child: CompositedTransformTarget(
          link: _link,
          child: TextField(
            controller: _text,
            focusNode: _focus,
            onChanged: (_) => _refresh(),
            onTapOutside: (_) => _focus.unfocus(),
            style: widget.style,
            textInputAction: TextInputAction.search,
            decoration: InputDecoration(
              isCollapsed: true,
              border: InputBorder.none,
              hintText: _offline
                  ? 'Search offline copies'
                  : 'Search YouTube',
              hintStyle: widget.hintStyle,
            ),
          ),
        ),
      );
    });
  }

  Widget _overlay(BuildContext context) {
    final t = context.tokens;
    final items = _items();
    final rows = <Widget>[
      // Where the search looks, as the list's first row.
      Padding(
        padding: const EdgeInsets.fromLTRB(8, 4, 8, 4),
        child: Row(
          children: [
            Text('Search', style: TextStyle(fontSize: 11, color: t.nInk3)),
            const SizedBox(width: 10),
            for (final (off, label) in const [
              (false, 'YouTube and library'),
              (true, 'Offline copies only'),
            ]) ...[
              ChoiceChip(
                label: Text(label, style: const TextStyle(fontSize: 11)),
                selected: _offline == off,
                showCheckmark: false,
                visualDensity: VisualDensity.compact,
                materialTapTargetSize: MaterialTapTargetSize.shrinkWrap,
                selectedColor: ytRose.withValues(alpha: 0.18),
                onSelected: (_) {
                  setState(() => _offline = off);
                  _refresh();
                },
              ),
              const SizedBox(width: 6),
            ],
          ],
        ),
      ),
    ];
    var lastGroup = '';
    for (var i = 0; i < items.length; i++) {
      final item = items[i];
      if (item.group.isNotEmpty && item.group != lastGroup) {
        rows.add(Padding(
          padding: const EdgeInsets.fromLTRB(12, 10, 12, 4),
          child: Text(item.group.toUpperCase(),
              style: TextStyle(
                  fontSize: 10,
                  letterSpacing: 1.3,
                  fontWeight: FontWeight.w600,
                  color: t.nInk3)),
        ));
      }
      lastGroup = item.group;
      rows.add(_Row(
        controller: widget.controller,
        item: item,
        selected: i == _index,
        onTap: () => _run(item),
        onHover: () => setState(() => _index = i),
      ));
    }
    return Positioned(
      width: _width.clamp(420.0, 640.0),
      child: CompositedTransformFollower(
        link: _link,
        showWhenUnlinked: false,
        targetAnchor: Alignment.bottomLeft,
        offset: const Offset(0, 6),
        // Inside the field's tap region: a click on a row is not a click
        // outside the field, so it does not unfocus before the row can act.
        child: TextFieldTapRegion(
          child: Material(
            color: t.modalSolid,
            elevation: 10,
            borderRadius: BorderRadius.circular(Tokens.radiusMd),
            clipBehavior: Clip.antiAlias,
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxHeight: 520),
              child: ListView(
                shrinkWrap: true,
                padding: const EdgeInsets.all(6),
                children: rows,
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _Row extends StatelessWidget {
  const _Row({
    required this.controller,
    required this.item,
    required this.selected,
    required this.onTap,
    required this.onHover,
  });

  final MusicController controller;
  final _Item item;
  final bool selected;
  final VoidCallback onTap;
  final VoidCallback onHover;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final Widget lead = switch (item.kind) {
      _Kind.video => SizedBox(
          width: 64,
          height: 36,
          child: YtThumb(controller: controller, video: item.video!)),
      _Kind.channel => MusicArt(
          controller: controller,
          kind: 'yt',
          artKey: item.sub!.avatar,
          direct: item.sub!.avatar,
          size: 32,
          radius: 16,
          fallback: Icons.person,
        ),
      _ => SizedBox(
          width: 32,
          child: Icon(item.icon ?? Icons.search, size: 18, color: t.nInk2)),
    };
    return MouseRegion(
      onEnter: (_) => onHover(),
      child: InkWell(
        borderRadius: BorderRadius.circular(10),
        onTap: onTap,
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 6),
          decoration: BoxDecoration(
            color: selected ? ytRose.withValues(alpha: 0.10) : null,
            borderRadius: BorderRadius.circular(10),
          ),
          child: Row(
            children: [
              lead,
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(item.label,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                            fontSize: 13,
                            fontWeight: item.group.isEmpty
                                ? FontWeight.w600
                                : FontWeight.w400,
                            color: t.nInk)),
                    if (item.detail.isNotEmpty)
                      Text(item.detail,
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(fontSize: 11, color: t.nInk3)),
                  ],
                ),
              ),
              if (item.video?.offline.isNotEmpty ?? false)
                Icon(Icons.offline_pin_outlined, size: 16, color: t.nInk3),
              if (selected)
                Padding(
                  padding: const EdgeInsets.only(left: 8),
                  child: Icon(Icons.keyboard_return, size: 14, color: t.nInk3),
                ),
            ],
          ),
        ),
      ),
    );
  }
}
