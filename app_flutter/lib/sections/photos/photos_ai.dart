// Photos → AI. Where the on-device models live, and where they are run.
//
// The gap this closes: a model could be downloaded from Settings and then
// nothing happened. The indexer was idle-only and its worker loop lived in the
// Slint runtime, so on this build People stayed empty, Things stayed empty, and
// a search for "beach at sunset" quietly fell back to matching filenames. A
// 154MB download with no visible effect is indistinguishable from a broken one.
//
// So every pass has a Run button and says how much of the library is still
// waiting for it, and every model says what it powers and what it has produced.
// The three editor models — Magic eraser, Smart select, Upscale — have no queue
// and offer the editor instead, because a Run button with nothing to run is the
// same lie in the other direction.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/photos.dart';
import 'photos_controller.dart';

const Color kAiTint = Color(0xFF7C3AED);
const Color kAiTint2 = Color(0xFFEC4899);

/// Icon per stage key. The stage keys are a storage format (`photo_ai_state`),
/// so matching on them is matching on something that cannot drift.
IconData _stageIcon(String stage) => switch (stage) {
      'exif' => Icons.photo_outlined,
      'fts' => Icons.manage_search,
      'faces' => Icons.face_outlined,
      'tags' => Icons.sell_outlined,
      'clip' => Icons.image_search_outlined,
      _ => Icons.auto_awesome,
    };

Color _stageTint(String stage) => switch (stage) {
      'exif' => const Color(0xFF3B82F6),
      'fts' => const Color(0xFF06B6D4),
      'faces' => const Color(0xFFEC4899),
      'tags' => const Color(0xFF22C55E),
      'clip' => const Color(0xFFA855F7),
      _ => kAiTint,
    };

IconData _modelIcon(String name) => switch (name) {
      'face-det-500m' => Icons.face_retouching_natural,
      'face-rec-500m' => Icons.person_search_outlined,
      'clip-vit-b32-q8' => Icons.image_search_outlined,
      'clip-vit-b32-tokenizer' => Icons.text_fields,
      'yolox-s' => Icons.sell_outlined,
      'lama-inpaint' => Icons.auto_fix_high_outlined,
      'mobile-sam' => Icons.highlight_alt_outlined,
      'realesrgan-x4' => Icons.photo_size_select_large_outlined,
      _ => Icons.memory,
    };

String _n(int v) {
  final s = '$v';
  final out = StringBuffer();
  for (var i = 0; i < s.length; i++) {
    if (i > 0 && (s.length - i) % 3 == 0) out.write(',');
    out.write(s[i]);
  }
  return out.toString();
}

class PhotosAiTab extends StatelessWidget {
  const PhotosAiTab({super.key, required this.controller, required this.state});

  final PhotosController controller;
  final PhotosState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final stages = state.aiStages;
    final running = state.aiRunning;
    // Photos, not queued passes: Rust counts each photo once however many
    // passes it still owes.
    final waiting = state.aiWaiting;
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 16, 24, 32),
      children: [
        _Hero(
          controller: controller,
          waiting: waiting,
          running: running,
          anyReady: stages.any((s) => s.ready && s.pending > 0),
        ),
        const SizedBox(height: 14),
        _Machine(machine: state.aiMachine),
        const _Head(
          title: 'When it runs',
          hint: 'without being asked',
        ),
        _When(controller: controller, mode: state.aiWhen),
        _Head(
          title: 'Indexing',
          hint: 'five passes over the library, each with its own Run',
          trailing: running.isEmpty
              ? 'idle'
              : 'running · ${stages.firstWhere((s) => s.stage == running, orElse: () => stages.first).label}',
        ),
        for (final s in stages)
          _StageRow(
              controller: controller, stage: s, blocked: running.isNotEmpty),
        const _Head(
          title: 'On-device models',
          hint: 'downloaded once, verified, loaded only while working',
        ),
        // Rows, not cards. Eight cards in a three-wide grid put every fact on
        // its own line and then ellipsised the one sentence that says what the
        // model is for; a full-width row fits the name, the purpose, the size,
        // the quantisation, what it feeds and what it still owes, all on one
        // line, and reads down the column like the stage list above it.
        for (final m in state.aiModels)
          _ModelRow(
            controller: controller,
            model: m,
            stages: stages,
            blocked: running.isNotEmpty,
          ),
        const SizedBox(height: 16),
        Container(
          padding: const EdgeInsets.fromLTRB(14, 12, 14, 13),
          decoration: BoxDecoration(
            color: t.nCard,
            borderRadius: BorderRadius.circular(12),
          ),
          child: Text(
            'Everything here runs on this computer. Nothing is uploaded, each '
            'model is checked against a pinned SHA-256 before it is used, and '
            'models are unloaded when they are not working.',
            style: TextStyle(fontSize: 12, height: 1.55, color: t.nInk2),
          ),
        ),
      ],
    );
  }
}

// -------------------------------------------------------------------- hero --

class _Hero extends StatelessWidget {
  const _Hero({
    required this.controller,
    required this.waiting,
    required this.running,
    required this.anyReady,
  });

  final PhotosController controller;
  final int waiting;
  final String running;
  final bool anyReady;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.fromLTRB(16, 15, 16, 15),
      decoration: BoxDecoration(
        gradient: LinearGradient(
          colors: [
            Color.alphaBlend(kAiTint.withValues(alpha: 0.12), t.nCard),
            Color.alphaBlend(kAiTint2.withValues(alpha: 0.09), t.nCard),
          ],
        ),
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: kAiTint.withValues(alpha: 0.22)),
      ),
      child: Row(
        children: [
          Container(
            width: 44,
            height: 44,
            decoration: BoxDecoration(
              color: kAiTint.withValues(alpha: 0.18),
              borderRadius: BorderRadius.circular(13),
            ),
            child: const Icon(Icons.auto_awesome, color: kAiTint),
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  waiting == 0
                      ? 'Your library is up to date'
                      : waiting == 1
                          ? 'One photo is waiting to be understood'
                          : '${_n(waiting)} photos are waiting to be understood',
                  style: TextStyle(
                    fontSize: 15.5,
                    fontWeight: FontWeight.w700,
                    color: t.nInk,
                  ),
                ),
                const SizedBox(height: 3),
                Text(
                  'People, Things and searching by what is in a photo all come '
                  'from these passes.',
                  style: TextStyle(fontSize: 12.5, color: t.nInk2),
                ),
              ],
            ),
          ),
          const SizedBox(width: 12),
          if (running.isEmpty)
            FilledButton.icon(
              icon: const Icon(Icons.play_arrow, size: 18),
              label: const Text('Run everything now'),
              style: FilledButton.styleFrom(backgroundColor: kAiTint),
              onPressed: anyReady
                  ? () => controller.send(const PhotosCmd.aiRunAll())
                  : null,
            )
          else
            FilledButton.icon(
              icon: const Icon(Icons.stop, size: 18),
              label: const Text('Stop'),
              style: FilledButton.styleFrom(backgroundColor: Tokens.error),
              onPressed: () => controller.send(const PhotosCmd.aiStop()),
            ),
        ],
      ),
    );
  }
}

class _Machine extends StatelessWidget {
  const _Machine({required this.machine});

  final AiMachine machine;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final gb = machine.ramMb / 1024;
    Widget pill(String label, String value, Color dot) => Container(
          height: 30,
          padding: const EdgeInsets.symmetric(horizontal: 11),
          decoration: BoxDecoration(
            color: t.nChip,
            borderRadius: BorderRadius.circular(999),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Container(
                width: 7,
                height: 7,
                decoration: BoxDecoration(color: dot, shape: BoxShape.circle),
              ),
              const SizedBox(width: 7),
              Text(label, style: TextStyle(fontSize: 12, color: t.nInk2)),
              const SizedBox(width: 6),
              Text(
                value,
                style: TextStyle(
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                  color: t.nInk,
                ),
              ),
            ],
          ),
        );
    return Wrap(
      spacing: 8,
      runSpacing: 8,
      children: [
        pill(
          'Memory',
          machine.ramMb == 0 ? 'Unknown' : '${gb.toStringAsFixed(1)} GB',
          machine.ramMb == 0
              ? t.nInk3
              : machine.ramMb >= 8192
                  ? Tokens.ok
                  : Tokens.warn,
        ),
        pill(
          'Inference',
          machine.onnx ? 'ONNX runtime' : 'not in this build',
          machine.onnx ? Tokens.ok : Tokens.warn,
        ),
        pill(
          'Models',
          '${machine.installed}/${machine.total} installed',
          machine.installed == machine.total ? Tokens.ok : t.nInk3,
        ),
      ],
    );
  }
}

/// The three run policies. This is the setting that did not exist: indexing was
/// idle-only and hard-coded, so a machine that is never idle never indexed.
class _When extends StatelessWidget {
  const _When({required this.controller, required this.mode});

  final PhotosController controller;
  final String mode;

  static const List<(String, String, String)> modes = [
    ('idle', 'Only when I am away', 'after ten minutes of quiet'),
    ('plugged', 'Whenever plugged in', 'pauses on battery'),
    ('always', 'All the time', 'fastest, warmest laptop'),
  ];

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Wrap(
      spacing: 7,
      runSpacing: 7,
      children: [
        for (final (id, label, hint) in modes)
          Tooltip(
            message: hint,
            child: ChoiceChip(
              selected: mode == id,
              showCheckmark: false,
              label: Text(label),
              labelStyle: TextStyle(
                fontSize: 12.5,
                fontWeight: FontWeight.w600,
                color: mode == id ? Colors.white : t.nInk2,
              ),
              selectedColor: kAiTint,
              backgroundColor: t.nChip,
              side: BorderSide.none,
              onSelected: (_) => controller.send(PhotosCmd.aiSetWhen(mode: id)),
            ),
          ),
      ],
    );
  }
}

class _Head extends StatelessWidget {
  const _Head({required this.title, this.hint, this.trailing});

  final String title;
  final String? hint;
  final String? trailing;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.fromLTRB(2, 22, 2, 10),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          Text(
            title,
            style: TextStyle(
              fontSize: 15.5,
              fontWeight: FontWeight.w700,
              color: t.nInk,
            ),
          ),
          if (hint != null) ...[
            const SizedBox(width: 9),
            Expanded(
              child: Text(
                hint!,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12, color: t.nInk3),
              ),
            ),
          ] else
            const Spacer(),
          if (trailing != null)
            Text(
              trailing!,
              style: TextStyle(fontSize: 12, color: t.nInk3),
            ),
        ],
      ),
    );
  }
}

// ------------------------------------------------------------------ stages --

class _StageRow extends StatelessWidget {
  const _StageRow({
    required this.controller,
    required this.stage,
    required this.blocked,
  });

  final PhotosController controller;
  final AiStage stage;

  /// Another pass is running. They contend for the same disk and cores, so one
  /// at a time — the row that is running keeps its Stop, the rest grey out.
  final bool blocked;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint = _stageTint(stage.stage);
    final live = stage.frac >= 0;
    final done = stage.pending == 0;
    final missing = stage.needs
        .where((n) => !_installed(context, n))
        .toList(growable: false);
    // The models are downloadable whatever the binary can do with them, so a
    // stage can be un-runnable with nothing missing. Saying "needs a model"
    // there sends people to download one they already have.
    final noEngine =
        stage.needs.isNotEmpty && !(controller.state?.aiMachine.onnx ?? true);

    return Container(
      margin: const EdgeInsets.only(bottom: 9),
      padding: const EdgeInsets.fromLTRB(13, 11, 13, 11),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(13),
        border: Border.all(
          color: live ? tint.withValues(alpha: 0.45) : Colors.transparent,
        ),
      ),
      child: Row(
        children: [
          Container(
            width: 36,
            height: 36,
            decoration: BoxDecoration(
              color: tint.withValues(alpha: 0.16),
              borderRadius: BorderRadius.circular(10),
            ),
            child: Icon(_stageIcon(stage.stage), size: 19, color: tint),
          ),
          const SizedBox(width: 13),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  stage.label,
                  style: TextStyle(
                    fontSize: 13.5,
                    fontWeight: FontWeight.w600,
                    color: t.nInk,
                  ),
                ),
                const SizedBox(height: 1),
                Text(
                  done
                      ? 'Up to date — nothing waiting'
                      : noEngine
                          ? 'This build has no inference engine, so this pass '
                              'cannot run'
                          : !stage.ready
                              ? 'Needs ${missing.isEmpty ? stage.needs.join(" and ") : missing.join(" and ")} first'
                              : stage.blurb,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 11.5, color: t.nInk3),
                ),
                if (live)
                  Padding(
                    padding: const EdgeInsets.only(top: 7),
                    child: ClipRRect(
                      borderRadius: BorderRadius.circular(2),
                      child: LinearProgressIndicator(
                        value: stage.frac.clamp(0.0, 1.0),
                        minHeight: 4,
                        backgroundColor: t.nTile,
                        valueColor: AlwaysStoppedAnimation(tint),
                      ),
                    ),
                  ),
              ],
            ),
          ),
          const SizedBox(width: 12),
          SizedBox(
            width: 74,
            child: Text(
              live
                  ? '${(stage.frac * 100).round()}%'
                  : done
                      ? '✓'
                      : _n(stage.pending),
              textAlign: TextAlign.right,
              style: TextStyle(
                fontSize: 12,
                fontWeight: FontWeight.w600,
                fontFeatures: const [FontFeature.tabularFigures()],
                color: t.nInk2,
              ),
            ),
          ),
          const SizedBox(width: 10),
          _action(context, live: live, done: done),
        ],
      ),
    );
  }

  Widget _action(BuildContext context,
      {required bool live, required bool done}) {
    if (live) {
      return OutlinedButton.icon(
        icon: const Icon(Icons.stop, size: 16),
        label: const Text('Stop'),
        onPressed: () => controller.send(const PhotosCmd.aiStop()),
      );
    }
    if (!stage.ready &&
        stage.needs.isNotEmpty &&
        !(controller.state?.aiMachine.onnx ?? true)) {
      return const OutlinedButton(
        onPressed: null,
        child: Text('Unavailable'),
      );
    }
    if (!stage.ready) {
      // The pass has no model. Offering Run here would raise an error the user
      // could have been told about before pressing.
      return OutlinedButton.icon(
        icon: const Icon(Icons.download_outlined, size: 16),
        label: const Text('Get model'),
        onPressed: stage.needs.isEmpty
            ? null
            : () =>
                controller.send(PhotosCmd.aiDownload(name: stage.needs.first)),
      );
    }
    if (done) {
      return OutlinedButton(
        onPressed: blocked
            ? null
            : () => controller.send(PhotosCmd.aiRerun(stage: stage.stage)),
        child: const Text('Run again'),
      );
    }
    return FilledButton.icon(
      icon: const Icon(Icons.play_arrow, size: 16),
      label: const Text('Run'),
      style: FilledButton.styleFrom(backgroundColor: kAiTint),
      onPressed: blocked
          ? null
          : () => controller.send(PhotosCmd.aiRun(stage: stage.stage)),
    );
  }

  /// Whether a manifest name is installed, read off the model list in the same
  /// snapshot — so a row and a card can never disagree.
  bool _installed(BuildContext context, String name) {
    final models = controller.state?.aiModels ?? const <AiModel>[];
    return models.any((m) => m.name == name && m.installed);
  }
}

// ------------------------------------------------------------------ models --

class _ModelRow extends StatelessWidget {
  const _ModelRow({
    required this.controller,
    required this.model,
    required this.stages,
    required this.blocked,
  });

  final PhotosController controller;
  final AiModel model;
  final List<AiStage> stages;
  final bool blocked;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final tint =
        model.stage.isEmpty ? const Color(0xFFF59E0B) : _stageTint(model.stage);
    final dl = model.frac >= 0;
    final stage = stages.where((s) => s.stage == model.stage).firstOrNull;
    final live = stage != null && stage.frac >= 0;

    return Container(
      margin: const EdgeInsets.only(bottom: 9),
      padding: const EdgeInsets.fromLTRB(13, 10, 13, 10),
      decoration: BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(13),
        border: Border.all(
          color: dl ? kAiTint.withValues(alpha: 0.45) : Colors.transparent,
        ),
      ),
      child: Column(
        children: [
          Row(
            children: [
              Container(
                width: 34,
                height: 34,
                decoration: BoxDecoration(
                  color: tint.withValues(alpha: 0.16),
                  borderRadius: BorderRadius.circular(10),
                ),
                child: Icon(_modelIcon(model.name), size: 18, color: tint),
              ),
              const SizedBox(width: 13),
              Expanded(
                flex: 5,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Text(
                      model.label,
                      style: TextStyle(
                        fontSize: 13.5,
                        fontWeight: FontWeight.w600,
                        color: t.nInk,
                      ),
                    ),
                    const SizedBox(height: 1),
                    Text(
                      model.purpose,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 11.5, color: t.nInk3),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 12),
              // Every fact the card used to stack, on one line and right up
              // against the buttons that act on it.
              Expanded(
                flex: 4,
                // One line, whatever the width: a list row, not a card. Too
                // narrow and it scrolls from the right, keeping the status in
                // view beside the button.
                child: SingleChildScrollView(
                  scrollDirection: Axis.horizontal,
                  reverse: true,
                  child: Row(
                    spacing: 6,
                    children: [
                      _Pill(
                        label: dl
                            ? 'Downloading'
                            : model.installed
                                ? 'Installed'
                                : 'Not downloaded',
                        tone: dl
                            ? kAiTint
                            : model.installed
                                ? Tokens.ok
                                : null,
                      ),
                      _Pill(label: '${model.mb} MB'),
                      _Pill(label: model.quant),
                      _Pill(
                        label: model.stage.isEmpty
                            ? 'editor tool'
                            : (stage?.label ?? model.stage),
                      ),
                      // What it has been worth so far. A row that says only
                      // "Installed · 154 MB" never answers the question people
                      // actually have, which is whether it did anything.
                      if (model.installed && model.pending >= 0)
                        _Pill(
                          label: model.pending == 0
                              ? 'Everything indexed'
                              : '${_n(model.pending)} waiting',
                        ),
                    ],
                  ),
                ),
              ),
              const SizedBox(width: 12),
              _action(context, dl: dl, live: live),
              if (model.installed && !dl) ...[
                const SizedBox(width: 2),
                IconButton(
                  icon: const Icon(Icons.verified_outlined, size: 18),
                  tooltip: 'Check the file against its pinned SHA-256',
                  onPressed: () =>
                      controller.send(PhotosCmd.aiVerify(name: model.name)),
                ),
                IconButton(
                  icon: const Icon(Icons.delete_outline, size: 18),
                  tooltip: 'Remove the downloaded file',
                  onPressed: () =>
                      controller.send(PhotosCmd.aiRemove(name: model.name)),
                ),
              ],
            ],
          ),
          if (dl)
            Padding(
              padding: const EdgeInsets.only(top: 9),
              child: ClipRRect(
                borderRadius: BorderRadius.circular(2),
                child: LinearProgressIndicator(
                  value: model.frac.clamp(0.0, 1.0),
                  minHeight: 3,
                  backgroundColor: t.nTile,
                  valueColor: const AlwaysStoppedAnimation(kAiTint),
                ),
              ),
            ),
        ],
      ),
    );
  }

  Widget _action(BuildContext context, {required bool dl, required bool live}) {
    if (dl) {
      return OutlinedButton(
        onPressed: null,
        child: Text('${(model.frac * 100).round()}%'),
      );
    }
    if (!model.installed) {
      return FilledButton.icon(
        icon: const Icon(Icons.download_outlined, size: 16),
        label: const Text('Download'),
        style: FilledButton.styleFrom(backgroundColor: kAiTint),
        onPressed: () =>
            controller.send(PhotosCmd.aiDownload(name: model.name)),
      );
    }
    if (model.stage.isEmpty) {
      // No queue: it is a per-photo tool. The honest button is the one that
      // takes you to where it is used.
      return OutlinedButton.icon(
        icon: const Icon(Icons.auto_fix_high_outlined, size: 16),
        label: const Text('Open in editor'),
        onPressed: () => ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(
            content: Text(
              '${model.label} is in the editor — open a photo and press E.',
            ),
          ),
        ),
      );
    }
    if (live) {
      return OutlinedButton.icon(
        icon: const Icon(Icons.stop, size: 16),
        label: const Text('Stop'),
        onPressed: () => controller.send(const PhotosCmd.aiStop()),
      );
    }
    // Half a pair is no pair: CLIP needs its tokenizer, detection needs
    // recognition. The row names what is missing rather than offering a Run
    // that only raises an error.
    final gate = stages.where((s) => s.stage == model.stage).firstOrNull;
    if (gate != null && !gate.ready) {
      final missing = gate.needs.firstWhere(
        (n) => !(controller.state?.aiModels ?? const <AiModel>[])
            .any((m) => m.name == n && m.installed),
        orElse: () => '',
      );
      return OutlinedButton.icon(
        icon: const Icon(Icons.download_outlined, size: 16),
        label: Text(missing.isEmpty ? 'Needs another model' : 'Needs a model'),
        onPressed: missing.isEmpty
            ? null
            : () => controller.send(PhotosCmd.aiDownload(name: missing)),
      );
    }
    return FilledButton.icon(
      icon: const Icon(Icons.play_arrow, size: 16),
      label: const Text('Run now'),
      style: FilledButton.styleFrom(backgroundColor: kAiTint),
      onPressed: blocked
          ? null
          : () => controller.send(PhotosCmd.aiRun(stage: model.stage)),
    );
  }
}

class _Pill extends StatelessWidget {
  const _Pill({required this.label, this.tone});

  final String label;
  final Color? tone;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      height: 23,
      padding: const EdgeInsets.symmetric(horizontal: 9),
      decoration: BoxDecoration(
        color: t.nChip,
        borderRadius: BorderRadius.circular(999),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (tone != null) ...[
            Container(
              width: 6,
              height: 6,
              decoration: BoxDecoration(color: tone, shape: BoxShape.circle),
            ),
            const SizedBox(width: 5),
          ],
          Text(
            label,
            style: TextStyle(
              fontSize: 11,
              fontWeight: FontWeight.w500,
              color: tone ?? t.nInk3,
            ),
          ),
        ],
      ),
    );
  }
}
