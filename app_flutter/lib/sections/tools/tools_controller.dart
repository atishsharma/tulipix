// The Tools section's state.
//
// Everything here is a job: you fill a form, it goes on a queue, a worker in
// Rust runs it and reports back. The controller holds the snapshot and the
// live console; the queue itself lives in `tools.db` and survives a restart.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/tools.dart';

/// The category tabs, and the accent each one wears. One entry per
/// `catalog::Category` — there is no Queue tab, because the queue was never a
/// category: it is a drawer now.
const List<({String id, String label, IconData icon, Color tint})> toolTabs = [
  (
    id: 'fileops',
    label: 'File ops',
    icon: Icons.folder_copy_outlined,
    tint: Color(0xFF6C4DF6)
  ),
  (
    id: 'convert',
    label: 'Convert',
    icon: Icons.swap_horiz,
    tint: Color(0xFF00B4D8)
  ),
  (
    id: 'video',
    label: 'Video',
    icon: Icons.movie_outlined,
    tint: Color(0xFFE0518F)
  ),
  (
    id: 'audio',
    label: 'Audio',
    icon: Icons.graphic_eq,
    tint: Color(0xFF2FBF71)
  ),
  (
    id: 'photo',
    label: 'Photo',
    icon: Icons.image_outlined,
    tint: Color(0xFFF5A623)
  ),
  (
    id: 'pdf',
    label: 'PDF',
    icon: Icons.picture_as_pdf_outlined,
    tint: Color(0xFFDC2626)
  ),
  (
    id: 'subtitles',
    label: 'Subtitles',
    icon: Icons.subtitles_outlined,
    tint: Color(0xFF3A86FF)
  ),
];

Color tintFor(String category) => toolTabs
    .firstWhere((t) => t.id == category, orElse: () => toolTabs.first)
    .tint;

IconData tabIconFor(String category) => toolTabs
    .firstWhere((t) => t.id == category, orElse: () => toolTabs.first)
    .icon;

/// A face for each operation. Purely presentational, so it stays on this side
/// of the bridge; anything unlisted falls back to its category's icon.
const Map<String, IconData> _opIcons = {
  'rename': Icons.drive_file_rename_outline,
  'merge': Icons.merge_type,
  'split': Icons.call_split,
  'hash': Icons.tag,
  'folder_diff': Icons.difference_outlined,
  'cache_clean': Icons.cleaning_services_outlined,
  'mediainfo': Icons.info_outline,
  'compress_video': Icons.compress,
  'trim': Icons.content_cut,
  'convert': Icons.swap_horiz,
  'thumbnail': Icons.photo_size_select_actual_outlined,
  'resize': Icons.aspect_ratio,
  'contact_sheet': Icons.grid_view,
  'download': Icons.download_outlined,
  'download_live': Icons.sensors,
  'download_playlist': Icons.playlist_add_check,
  'compress_audio': Icons.compress,
  'normalize': Icons.equalizer,
  'extract': Icons.music_note_outlined,
  'compress_photo': Icons.photo_size_select_small_outlined,
  'watermark': Icons.branding_watermark_outlined,
  'transcribe': Icons.record_voice_over_outlined,
  'burn_subs': Icons.closed_caption_outlined,
  'audio_convert': Icons.audiotrack_outlined,
  'image_convert': Icons.image_outlined,
  'crop': Icons.crop,
  'rotate': Icons.rotate_90_degrees_cw_outlined,
  'denoise': Icons.blur_on,
  'add_subs': Icons.subtitles_outlined,
  'denoise_audio': Icons.noise_control_off,
  'mirror': Icons.sync_alt,
  'pdf_pages': Icons.content_copy_outlined,
  'pdf_delete': Icons.delete_outline,
  'pdf_rotate': Icons.rotate_right,
  'pdf_stamp': Icons.format_list_numbered,
  'archive_create': Icons.archive_outlined,
  'archive_extract': Icons.unarchive_outlined,
  'archive_repack': Icons.inventory_2_outlined,
  'doc_convert': Icons.description_outlined,
  'ebook_convert': Icons.menu_book_outlined,
  'stems': Icons.graphic_eq,
  'subs_sync': Icons.av_timer,
};

IconData opIcon(String kind, String category) =>
    _opIcons[kind] ?? tabIconFor(category);

class ToolsController extends ChangeNotifier {
  ToolsState? state;
  Object? error;
  bool busy = false;

  /// Whether the queue drawer is showing. Local to the UI: the queue is the
  /// same queue whether or not you are looking at it.
  bool drawerOpen = false;

  /// What the open tool is about to do. Null until the first answer arrives,
  /// and again whenever the tool changes.
  PreviewResult? preview;
  bool previewBusy = false;

  int _previewEpoch = 0;
  Timer? _previewTimer;
  String _previewOp = '';

  /// Long enough that a slider drag is one request rather than one per pixel,
  /// short enough that letting go feels like the answer was already there.
  static const Duration _previewDebounce = Duration(milliseconds: 250);

  /// Live per-job progress from the event stream, which arrives far more often
  /// than a snapshot does. Keyed by job id.
  final Map<int, double> liveProgress = <int, double>{};

  StreamSubscription<ToolsEvent>? _events;

  ToolsController() {
    _events = toolsEvents().listen(_onEvent, onError: (Object e) {
      error = e;
      notifyListeners();
    });
  }

  void _onEvent(ToolsEvent event) {
    switch (event) {
      case ToolsEvent_Progress(:final id, :final progress):
        liveProgress[id] = progress;
        notifyListeners();
      case ToolsEvent_Log():
        // The console is read off the snapshot, not accumulated here — one
        // copy of a capped log is enough, and it lives in Rust.
        break;
      case ToolsEvent_Finished(:final id):
        liveProgress.remove(id);
        refresh();
      case ToolsEvent_Failed(:final message):
        error = message;
        notifyListeners();
    }
  }

  Future<void> send(ToolsCmd cmd) async {
    busy = true;
    error = null;
    notifyListeners();
    try {
      state = await toolsDispatch(cmd: cmd);
      // Opening, closing or switching tools invalidates whatever is on the
      // right immediately — a stale dry run under a different tool's name is
      // worse than an empty pane.
      final op = state?.activeOp ?? '';
      if (op != _previewOp) {
        _previewOp = op;
        preview = null;
        _schedulePreview(now: true);
      }
    } catch (e) {
      error = e;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  /// Set a field and ask what that changed. The form round-trips through the
  /// bridge, so by the time the timer fires the session already holds the value
  /// the preview is about to describe.
  Future<void> setField(String key, String value) async {
    await send(ToolsCmd.setField(key: key, value: value));
    _schedulePreview();
  }

  Future<void> reset() async {
    await send(const ToolsCmd.reset());
    _schedulePreview(now: true);
  }

  Future<void> closeTool() => send(const ToolsCmd.closeTool());

  void _schedulePreview({bool now = false}) {
    _previewTimer?.cancel();
    if ((state?.activeOp ?? '').isEmpty) {
      preview = null;
      return;
    }
    if (now) {
      _requestPreview();
    } else {
      _previewTimer = Timer(_previewDebounce, _requestPreview);
    }
  }

  Future<void> _requestPreview() async {
    final epoch = ++_previewEpoch;
    previewBusy = true;
    notifyListeners();
    try {
      final result = await toolsPreview(epoch: epoch);
      // Three ways an answer can arrive too late to be true: a newer request
      // has gone out, Rust noticed the same thing first, or the tool changed
      // underneath it.
      if (epoch != _previewEpoch ||
          result.kind == 'stale' ||
          result.op != (state?.activeOp ?? '')) {
        return;
      }
      preview = result;
    } catch (_) {
      // A preview that fails is a pane that says nothing, never an error
      // banner over a form the user is still filling in.
      preview = null;
    } finally {
      if (epoch == _previewEpoch) previewBusy = false;
      notifyListeners();
    }
  }

  Future<void> refresh() => send(const ToolsCmd.refresh());

  /// Open a tool. The drawer closes on the way in: the preview is the point of
  /// the screen you are moving to, and the drawer covers it.
  Future<void> openTool(String kind) {
    drawerOpen = false;
    return send(ToolsCmd.openTool(kind: kind));
  }

  /// Ask again for whatever the pane is showing. The form has not changed, so
  /// nothing is debounced.
  void refreshPreview() => _schedulePreview(now: true);

  /// Queue the form. This is the one moment the queue is worth looking at, so
  /// it is the one moment it appears by itself.
  Future<void> run() async {
    await send(const ToolsCmd.run());
    if (state?.error.isEmpty ?? false) setDrawer(true);
  }

  void setDrawer(bool open) {
    if (drawerOpen == open) return;
    drawerOpen = open;
    notifyListeners();
  }

  void clearError() {
    error = null;
    notifyListeners();
  }

  /// The queue's own progress, overridden by anything newer from the stream.
  double progressOf(Job job) => liveProgress[job.id] ?? job.progress;

  @override
  void dispose() {
    _previewTimer?.cancel();
    _events?.cancel();
    super.dispose();
  }
}

/// A job state as a colour and an icon.
({Color colour, IconData icon}) jobLook(String state) => switch (state) {
      'running' => (colour: const Color(0xFF3A86FF), icon: Icons.play_arrow),
      'queued' => (colour: const Color(0xFF8A8A94), icon: Icons.schedule),
      'paused' => (colour: const Color(0xFFF5A623), icon: Icons.pause),
      'done' => (colour: const Color(0xFF2FBF71), icon: Icons.check),
      'failed' => (colour: const Color(0xFFEF4444), icon: Icons.error_outline),
      'cancelled' => (colour: const Color(0xFF8A8A94), icon: Icons.block),
      _ => (colour: const Color(0xFF8A8A94), icon: Icons.circle_outlined),
    };
