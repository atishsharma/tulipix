// YouTube › History: what was played, audio or picture, newest first.
//
// A video counts as watched when it is played to its last seconds or marked by
// hand; watched ones wear a full bar everywhere and can be hidden from the
// feeds and channel pages. The header search filters this list, and the tab's
// header carries this page's two buttons and its pager.

import 'package:flutter/material.dart';

import '../../../src/rust/api/music.dart';
import '../music_controller.dart';
import '../music_dialogs.dart';
import '../music_widgets.dart';
import 'yt_card.dart';

class YtHistory extends StatelessWidget {
  const YtHistory({
    super.key,
    required this.controller,
    required this.st,
    required this.page,
  });

  final MusicController controller;
  final MusicState st;
  final int page;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    if (st.ytHistory.isEmpty) {
      return const MusicEmpty(
        icon: Icons.history,
        title: 'Nothing played yet',
        body: 'Videos you listen to or watch show up here, newest first.',
      );
    }
    return ListView(
      padding: const EdgeInsets.only(top: 12, bottom: 32),
      children: [
        YtGrid(children: [
          for (final v in st.ytHistory.skip(page * ytPerPage).take(ytPerPage))
            YtVideoCard(
              controller: c,
              video: v,
              extraMenu: [
                (
                  'Remove from history',
                  () => c.send(MusicCmd.ytRemoveHistory(videoId: v.videoId))
                ),
              ],
            ),
        ]),
      ],
    );
  }
}

/// History's buttons, for the tab header: hide watched videos from the feeds,
/// and clear the list.
class YtHistoryActions extends StatelessWidget {
  const YtHistoryActions({super.key, required this.controller, required this.st});

  final MusicController controller;
  final MusicState st;

  @override
  Widget build(BuildContext context) {
    final c = controller;
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        SortChip(
          label: 'Hide watched in feeds',
          active: st.ytHideWatched,
          onTap: () => c.send(const MusicCmd.ytToggleHideWatched()),
        ),
        const SizedBox(width: 6),
        TextButton.icon(
          style: musicQuietStyle(context),
          icon: const Icon(Icons.delete_sweep_outlined, size: 16),
          label: const Text('Clear history'),
          onPressed: st.ytHistory.isEmpty
              ? null
              : () async {
                  final ok = await confirm(
                    context,
                    title: 'Clear the history?',
                    body: 'Every entry goes, and with it which videos '
                        'count as watched. Resume points stay.',
                    action: 'Clear',
                  );
                  if (ok) await c.send(const MusicCmd.ytClearHistory());
                },
        ),
      ],
    );
  }
}
