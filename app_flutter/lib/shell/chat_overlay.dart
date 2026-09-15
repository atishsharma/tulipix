// The chat assistant overlay (Ctrl/Cmd+J), ported from ui/chat_overlay.slint.
//
// The overlay is wired; the engine behind it is not, in this build or in the
// Slint one -- `chat_reply` says so rather than pretending. What is real here
// is the surface: the shortcut, the transcript, the draft box, and the fact
// that it floats over every section instead of belonging to one.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../design/skin.dart';
import '../design/tokens.dart';
import '../src/rust/api/chat.dart';

/// Wraps the app and adds the shortcut. The turn list lives here: it is a
/// list of two-field records, and round-tripping it through the bridge on
/// every message would buy nothing but latency.
class ChatOverlay extends StatefulWidget {
  const ChatOverlay({super.key, required this.child});

  final Widget child;

  @override
  State<ChatOverlay> createState() => _ChatOverlayState();
}

class _ChatOverlayState extends State<ChatOverlay> {
  bool _open = false;
  bool _sending = false;
  final _turns = <ChatTurn>[];
  final _draft = TextEditingController();
  final _scroll = ScrollController();
  final _focus = FocusNode();

  @override
  void initState() {
    super.initState();
    _turns.add(chatGreeting());
  }

  @override
  void dispose() {
    _draft.dispose();
    _scroll.dispose();
    _focus.dispose();
    super.dispose();
  }

  void _toggle() {
    setState(() => _open = !_open);
    // Opening puts the caret in the box: the shortcut exists so you can ask
    // without reaching for the mouse, and landing unfocused undoes that.
    if (_open) _focus.requestFocus();
  }

  Future<void> _submit() async {
    final q = _draft.text.trim();
    if (q.isEmpty || _sending) return;
    setState(() {
      _turns.add(ChatTurn(role: 'user', content: q));
      _draft.clear();
      _sending = true;
    });
    _toBottom();
    final reply = await chatReply(question: q);
    if (!mounted) return;
    setState(() {
      if (reply != null) _turns.add(reply);
      _sending = false;
    });
    _toBottom();
    _focus.requestFocus();
  }

  /// After the frame that added the turn, not during it -- the new extent
  /// does not exist until the list has laid out.
  void _toBottom() {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!_scroll.hasClients) return;
      _scroll.animateTo(
        _scroll.position.maxScrollExtent,
        duration: const Duration(milliseconds: 180),
        curve: Curves.easeOut,
      );
    });
  }

  @override
  Widget build(BuildContext context) {
    return CallbackShortcuts(
      bindings: {
        const SingleActivator(LogicalKeyboardKey.keyJ, control: true): _toggle,
        const SingleActivator(LogicalKeyboardKey.keyJ, meta: true): _toggle,
      },
      child: Focus(
        autofocus: true,
        child: Stack(
          // As `LockOverlay` and `OnboardingOverlay` both already do. Without
          // it this handed the whole app below loose constraints, which is what
          // let an `Offstage` down there collapse the window to nothing.
          fit: StackFit.expand,
          children: [
            widget.child,
            if (_open) _panel(context),
          ],
        ),
      ),
    );
  }

  Widget _panel(BuildContext context) {
    final t = context.tokens;
    return Positioned.fill(
      child: Stack(
        children: [
          // The scrim closes it, the way clicking outside the Slint modal does.
          Positioned.fill(
            child: GestureDetector(
              onTap: _toggle,
              child: Container(color: const Color(0x80000000)),
            ),
          ),
          Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 680, maxHeight: 560),
              child: Material(
                // A language's dialog surface: Glass's panel is translucent,
                // and this floats on a black scrim.
                color: context.skin.isStandard ? t.panel : t.modal,
                elevation: 24,
                borderRadius: BorderRadius.circular(Tokens.radiusLg),
                clipBehavior: Clip.antiAlias,
                child: Padding(
                  padding: const EdgeInsets.all(18),
                  child: Column(
                    children: [
                      Row(
                        children: [
                          Icon(context.skin.icon(Icons.auto_awesome),
                              size: 16, color: t.text),
                          const SizedBox(width: 6),
                          Text('Chat',
                              style: TextStyle(
                                  fontSize: 16,
                                  fontWeight: FontWeight.w700,
                                  color: t.text)),
                          const Spacer(),
                          Text('Ctrl/Cmd+J',
                              style: TextStyle(fontSize: 11, color: t.textDim)),
                          const SizedBox(width: 4),
                          IconButton(
                            onPressed: _toggle,
                            icon: Icon(context.skin.icon(Icons.close),
                                size: 18),
                          ),
                        ],
                      ),
                      const SizedBox(height: 10),
                      Expanded(
                        child: Container(
                          decoration: BoxDecoration(
                            color: t.panel2,
                            borderRadius: BorderRadius.circular(10),
                            border: Border.all(color: t.outline),
                          ),
                          child: ListView.builder(
                            controller: _scroll,
                            padding: const EdgeInsets.all(10),
                            itemCount: _turns.length,
                            itemBuilder: (context, i) => _Turn(turn: _turns[i]),
                          ),
                        ),
                      ),
                      const SizedBox(height: 10),
                      Row(
                        children: [
                          Expanded(
                            child: TextField(
                              controller: _draft,
                              focusNode: _focus,
                              style: TextStyle(fontSize: 13, color: t.text),
                              decoration: const InputDecoration(
                                isDense: true,
                                hintText: 'Ask about your library…',
                                border: OutlineInputBorder(),
                              ),
                              onSubmitted: (_) => _submit(),
                            ),
                          ),
                          const SizedBox(width: 8),
                          FilledButton(
                            onPressed: _sending ? null : _submit,
                            child: _sending
                                ? const SizedBox(
                                    width: 14,
                                    height: 14,
                                    child: CircularProgressIndicator(
                                        strokeWidth: 2))
                                : const Text('Send'),
                          ),
                        ],
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _Turn extends StatelessWidget {
  const _Turn({required this.turn});

  final ChatTurn turn;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final mine = turn.role == 'user';
    return Container(
      margin: const EdgeInsets.only(bottom: 6),
      padding: const EdgeInsets.all(8),
      decoration: BoxDecoration(
        color: mine ? t.panel : Colors.transparent,
        borderRadius: BorderRadius.circular(8),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(turn.role,
              style: TextStyle(
                  fontSize: 10, fontWeight: FontWeight.w600, color: t.textDim)),
          const SizedBox(height: 2),
          Text(turn.content, style: TextStyle(fontSize: 12, color: t.text)),
        ],
      ),
    );
  }
}
