// The connect wizard, and the two small dialogs the section needs.
//
// Step one is rclone's own list of providers; step two is a form generated from
// rclone's own schema for whichever one you pick. Nothing about any particular
// backend is written down here — that is the point. When rclone adds a
// provider, this form grows a page for it without a line changing.

import 'package:flutter/material.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/cloud.dart';
import 'cloud_controller.dart';

class ConnectWizard extends StatelessWidget {
  const ConnectWizard({
    super.key,
    required this.controller,
    required this.state,
  });

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ColoredBox(
      color: Colors.black54,
      child: Center(
        child: Material(
          color: t.nCard,
          borderRadius: BorderRadius.circular(16),
          clipBehavior: Clip.antiAlias,
          child: SizedBox(
            width: 720,
            height: 620,
            child: Column(
              children: [
                _Head(controller: controller, state: state),
                Expanded(
                  child: state.connectStep == 0
                      ? _PickBackend(controller: controller, state: state)
                      : _Form(controller: controller, state: state),
                ),
                if (state.connectError.isNotEmpty)
                  Container(
                    width: double.infinity,
                    padding: const EdgeInsets.symmetric(
                        horizontal: 20, vertical: 10),
                    color: Tokens.error.withValues(alpha: 0.14),
                    child: Text(state.connectError,
                        style:
                            const TextStyle(fontSize: 12, color: Tokens.error)),
                  ),
                _Foot(controller: controller, state: state),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _Head extends StatelessWidget {
  const _Head({required this.controller, required this.state});

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.fromLTRB(20, 14, 10, 14),
      decoration: BoxDecoration(
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          Text(
            state.connectEdit
                ? 'Edit ${state.formName}'
                : (state.connectStep == 0
                    ? 'Pick a provider'
                    : 'Configure ${state.formBackend}'),
            style: TextStyle(
                fontSize: 17, fontWeight: FontWeight.w700, color: t.nInk),
          ),
          const Spacer(),
          IconButton(
            icon: const Icon(Icons.close),
            onPressed: () => controller.send(const CloudCmd.connectClose()),
          ),
        ],
      ),
    );
  }
}

class _PickBackend extends StatelessWidget {
  const _PickBackend({required this.controller, required this.state});

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(20, 14, 20, 8),
          child: TextField(
            autofocus: true,
            decoration: const InputDecoration(
              isDense: true,
              hintText: 'Drive, S3, Dropbox, SFTP…',
              prefixIcon: Icon(Icons.search, size: 18),
              border: OutlineInputBorder(),
            ),
            onChanged: (v) =>
                controller.send(CloudCmd.backendSearch(text: v.trim())),
          ),
        ),
        Expanded(
          child: state.backends.isEmpty
              ? Center(
                  child: Text('Loading rclone’s provider list…',
                      style: TextStyle(fontSize: 13, color: t.nInk2)),
                )
              : ListView.builder(
                  itemCount: state.backends.length,
                  itemBuilder: (_, i) {
                    final b = state.backends[i];
                    return ListTile(
                      dense: true,
                      leading: Icon(
                        b.oauth ? Icons.vpn_key_outlined : Icons.storage,
                        size: 18,
                        color: t.nInk2,
                      ),
                      title: Text(b.name,
                          style: TextStyle(
                              fontSize: 13,
                              fontWeight: FontWeight.w600,
                              color: t.nInk)),
                      subtitle: Text(b.description,
                          maxLines: 1, overflow: TextOverflow.ellipsis),
                      onTap: () =>
                          controller.send(CloudCmd.backendPick(name: b.name)),
                    );
                  },
                ),
        ),
      ],
    );
  }
}

class _Form extends StatelessWidget {
  const _Form({required this.controller, required this.state});

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.fromLTRB(20, 16, 20, 20),
      children: [
        _Row(
          label: 'Name',
          hint: 'What this remote is called here',
          child: _Text(
            value: state.formName,
            hint: 'mydrive',
            obscure: false,
            // Renaming an existing remote would orphan its mounts and saved
            // jobs, so it is fixed once created.
            enabled: !state.connectEdit,
            onChanged: (v) => controller.send(CloudCmd.setFormName(name: v)),
          ),
        ),
        if (state.formOauth) ...[
          const SizedBox(height: 4),
          Container(
            padding: const EdgeInsets.all(14),
            decoration: BoxDecoration(
              color: t.nChip,
              borderRadius: BorderRadius.circular(10),
            ),
            child: Row(
              children: [
                Icon(
                  state.authorized
                      ? Icons.check_circle
                      : Icons.vpn_key_outlined,
                  size: 20,
                  color: state.authorized
                      ? const Color(0xFF2FBF71)
                      : Tokens.secCloud,
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: Text(
                    state.authorized
                        ? 'Authorised. The token is in the form below.'
                        : 'This provider signs in through your browser. '
                            'rclone opens it and waits.',
                    style: TextStyle(fontSize: 12, height: 1.4, color: t.nInk2),
                  ),
                ),
                const SizedBox(width: 12),
                FilledButton(
                  style:
                      FilledButton.styleFrom(backgroundColor: Tokens.secCloud),
                  onPressed: controller.busy
                      ? null
                      : () => controller.send(const CloudCmd.authorize()),
                  child: Text(state.authorized ? 'Re-authorise' : 'Authorise'),
                ),
              ],
            ),
          ),
          const SizedBox(height: 16),
        ],
        for (final o in state.opts) _OptRowView(controller: controller, opt: o),
        const SizedBox(height: 8),
        Row(
          children: [
            Switch(
              value: state.showAdvanced,
              activeThumbColor: Tokens.secCloud,
              onChanged: (_) =>
                  controller.send(const CloudCmd.toggleAdvanced()),
            ),
            const SizedBox(width: 6),
            Text('Show advanced options',
                style: TextStyle(fontSize: 12.5, color: t.nInk2)),
          ],
        ),
      ],
    );
  }
}

class _OptRowView extends StatelessWidget {
  const _OptRowView({required this.controller, required this.opt});

  final CloudController controller;
  final OptRow opt;

  @override
  Widget build(BuildContext context) {
    void set(String v) =>
        controller.send(CloudCmd.setOpt(name: opt.name, value: v));

    Widget control() {
      if (opt.kind == 'bool') {
        return Switch(
          value: opt.value == 'true',
          activeThumbColor: Tokens.secCloud,
          onChanged: (v) => set(v ? 'true' : 'false'),
        );
      }
      if (opt.exclusive && opt.examples.isNotEmpty) {
        return DropdownButtonFormField<String>(
          initialValue:
              opt.examples.contains(opt.value) ? opt.value : opt.examples.first,
          decoration: const InputDecoration(
              isDense: true, border: OutlineInputBorder()),
          items: [
            for (final e in opt.examples)
              DropdownMenuItem(value: e, child: Text(e)),
          ],
          onChanged: (v) => v == null ? null : set(v),
        );
      }
      return _Text(
        value: opt.value,
        hint: opt.examples.isEmpty ? '' : opt.examples.first,
        obscure: opt.secret,
        enabled: true,
        onChanged: set,
      );
    }

    return _Row(
        label: opt.name,
        hint: opt.help,
        required: opt.required_,
        child: control());
  }
}

class _Row extends StatelessWidget {
  const _Row({
    required this.label,
    required this.hint,
    required this.child,
    this.required = false,
  });

  final String label;
  final String hint;
  final Widget child;
  final bool required;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.only(bottom: 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Text(label,
                  style: TextStyle(
                      fontSize: 12.5,
                      fontWeight: FontWeight.w600,
                      color: t.nInk)),
              if (required)
                const Text(' *', style: TextStyle(color: Tokens.error)),
            ],
          ),
          if (hint.isNotEmpty) ...[
            const SizedBox(height: 3),
            Text(hint,
                style: TextStyle(fontSize: 11, height: 1.4, color: t.nInk3)),
          ],
          const SizedBox(height: 6),
          child,
        ],
      ),
    );
  }
}

class _Text extends StatefulWidget {
  const _Text({
    required this.value,
    required this.hint,
    required this.obscure,
    required this.enabled,
    required this.onChanged,
  });

  final String value;
  final String hint;
  final bool obscure;
  final bool enabled;
  final ValueChanged<String> onChanged;

  @override
  State<_Text> createState() => _TextState();
}

class _TextState extends State<_Text> {
  late final TextEditingController _c =
      TextEditingController(text: widget.value);

  @override
  void didUpdateWidget(_Text old) {
    super.didUpdateWidget(old);
    if (widget.value != _c.text && widget.value != old.value) {
      _c.text = widget.value;
    }
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => TextField(
        controller: _c,
        enabled: widget.enabled,
        obscureText: widget.obscure,
        decoration: InputDecoration(
          isDense: true,
          hintText: widget.hint,
          border: const OutlineInputBorder(),
        ),
        onChanged: widget.onChanged,
      );
}

class _Foot extends StatelessWidget {
  const _Foot({required this.controller, required this.state});

  final CloudController controller;
  final CloudState state;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 12),
      decoration: BoxDecoration(
        border: Border(top: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          if (state.connectStep == 1 && !state.connectEdit)
            TextButton.icon(
              icon: const Icon(Icons.arrow_back, size: 16),
              label: const Text('Providers'),
              onPressed: () => controller.send(const CloudCmd.connectOpen()),
            ),
          const Spacer(),
          TextButton(
            onPressed: () => controller.send(const CloudCmd.connectClose()),
            child: const Text('Cancel'),
          ),
          const SizedBox(width: 8),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: Tokens.secCloud),
            onPressed: state.connectStep == 0 || controller.busy
                ? null
                : () => controller.send(const CloudCmd.saveRemote()),
            child: Text(state.connectEdit ? 'Save' : 'Connect'),
          ),
        ],
      ),
    );
  }
}

/// One line of text, with a title and a hint. A typed path rather than a native
/// chooser, for the same reason every other section does it: the shell owns
/// file dialogs (phase 04).
Future<String?> promptText(
  BuildContext context, {
  required String title,
  required String label,
  String hint = '',
  String confirm = 'OK',
  String initial = '',
}) async {
  final text = TextEditingController(text: initial);
  final value = await showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(title),
      content: SizedBox(
        width: 460,
        child: TextField(
          controller: text,
          autofocus: true,
          decoration: InputDecoration(labelText: label, hintText: hint),
          onSubmitted: (v) => Navigator.pop(ctx, v),
        ),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx), child: const Text('Cancel')),
        FilledButton(
            onPressed: () => Navigator.pop(ctx, text.text),
            child: Text(confirm)),
      ],
    ),
  );
  final trimmed = value?.trim() ?? '';
  return trimmed.isEmpty ? null : trimmed;
}

Future<bool> confirmAction(
  BuildContext context, {
  required String title,
  required String body,
  String action = 'Delete',
}) async {
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(title),
      content: SizedBox(width: 420, child: Text(body)),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Cancel')),
        FilledButton(
            onPressed: () => Navigator.pop(ctx, true), child: Text(action)),
      ],
    ),
  );
  return ok ?? false;
}
