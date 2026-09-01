// The Tools section's state.
//
// Everything here is a job: you fill a form, it goes on a queue, a worker in
// Rust runs it and reports back. The controller holds the snapshot and the
// live console; the queue itself lives in `tools.db` and survives a restart.

import 'dart:async';

import 'package:flutter/material.dart';

import '../../src/rust/api/tools.dart';

/// The bookmarked tab's id. Not a `catalog::Category`: an operation belongs to
/// exactly one category, and this is a second axis over all of them.
const String favouritesTab = 'favourite';

/// The category tabs, and the accent each one wears. One entry per
/// `catalog::Category`, plus Favourites at the front — eighty tools is more
/// than anyone uses, and the handful you keep coming back to should be one
/// click from the top of the section.
const List<({String id, String label, IconData icon, Color tint})> toolTabs = [
  (
    id: favouritesTab,
    label: 'Favourite',
    icon: Icons.bookmark,
    tint: Color(0xFFDC2626)
  ),
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

// Favourites is a tab, not a category, so it is never the right answer for a
// tile asking what colour it is: `toolTabs.first` used to be File ops, and is
// now the bookmark red.
const _fallbackTab = 1;

Color tintFor(String category) => toolTabs
    .firstWhere((t) => t.id == category, orElse: () => toolTabs[_fallbackTab])
    .tint;

IconData tabIconFor(String category) => toolTabs
    .firstWhere((t) => t.id == category, orElse: () => toolTabs[_fallbackTab])
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
  // File ops
  'encrypt': Icons.lock_outline,
  'dedupe': Icons.copy_all,
  'sort_files': Icons.sort,
  'empty_dirs': Icons.delete_sweep,
  'file_list': Icons.list_alt,
  // Convert
  'data_convert': Icons.table_chart,
  // Video
  'speed': Icons.speed,
  'gif': Icons.gif,
  'fade': Icons.gradient,
  // Audio
  'tags': Icons.label_outline,
  'silence_trim': Icons.volume_off,
  'audio_speed': Icons.fast_forward,
  'replace_audio': Icons.swap_vert,
  // Photo
  'strip_meta': Icons.privacy_tip,
  'border': Icons.crop_din,
  'collage': Icons.grid_on,
  'adjust': Icons.tune,
  'recolour': Icons.filter_b_and_w,
  'sharpen': Icons.details,
  'censor': Icons.blur_circular,
  'favicon': Icons.star_outline,
  'remove_bg': Icons.auto_fix_high,
  'upscale': Icons.zoom_out_map,
  'photo_batch': Icons.photo_library,
  // PDF
  'pdf_merge': Icons.merge_type,
  'pdf_split': Icons.splitscreen,
  'pdf_impose': Icons.import_contacts,
  'pdf_redact': Icons.password,
  'pdf_forms': Icons.edit_note,
  'pdf_images': Icons.image_search,
  'pdf_text': Icons.text_snippet,
  'pdf_from_images': Icons.picture_as_pdf_outlined,
  'pdf_compress': Icons.compress,
  'pdf_protect': Icons.enhanced_encryption,
  // Subtitles
  'subs_convert': Icons.translate,
  'subs_shift': Icons.schedule,
  'subs_clean': Icons.cleaning_services_outlined,
  'subs_translate': Icons.g_translate,
};

IconData opIcon(String kind, String category) =>
    _opIcons[kind] ?? tabIconFor(category);

class ToolsController extends ChangeNotifier {
  ToolsState? state;
  Object? error;
  bool busy = false;

  /// Whether the queue and the console panels are showing. Local to the UI —
  /// the queue is the same queue whether or not you are looking at it — and
  /// independent of each other, because watching a job run and reading why the
  /// last one failed are different questions.
  bool queueOpen = false;
  bool consoleOpen = false;

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

  Future<void> openTool(String kind) => send(ToolsCmd.openTool(kind: kind));

  Future<void> toggleFavourite(String kind) =>
      send(ToolsCmd.toggleFavourite(kind: kind));

  /// Ask again for whatever the pane is showing. The form has not changed, so
  /// nothing is debounced.
  void refreshPreview() => _schedulePreview(now: true);

  /// Queue the form. The tool stays open behind it: the settings you chose are
  /// still there, and the preview keeps showing what the job is doing to the
  /// file. Running used to close the tool and slide the queue over the top,
  /// which threw away both.
  Future<void> run() => send(const ToolsCmd.run());

  void setQueue(bool open) {
    if (queueOpen == open) return;
    queueOpen = open;
    notifyListeners();
  }

  void setConsole(bool open) {
    if (consoleOpen == open) return;
    consoleOpen = open;
    notifyListeners();
  }

  /// The job this tool most recently put on the queue, if it is still going.
  /// The preview header draws its progress, so the tool you are looking at
  /// tells you how far along it is without opening the queue.
  Job? activeJob() {
    final kind = state?.activeOp ?? '';
    if (kind.isEmpty) return null;
    for (final j in state?.jobs ?? const <Job>[]) {
      if (j.kind == kind && (j.state == 'running' || j.state == 'queued')) {
        return j;
      }
    }
    return null;
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
