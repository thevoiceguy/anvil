// The account's lines, as on a desk phone (FCP's `docs/DEVICE_LINES.md`):
// a column of them beside the keypad on a wide window, a picker above it
// on a narrow one. Choosing a line places calls from it; a line's menu
// renames it where the tenant allows.

import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../design/theme.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';

/// The label of the line a call is on, when the account has lines.
String? lineLabelFor(PhoneSnapshot snap, PhoneCall call) =>
    snap.hasLines ? snap.lineOf(call.line ?? 'own')?.label : null;

/// The line calls are placed from: the one chosen, else the account's own.
PhoneLine? chosenLine(PhoneModel model) {
  final snap = model.snapshot;
  return snap.lineOf(model.line ?? 'own') ??
      (snap.lines.isEmpty ? null : snap.lines.first);
}

/// Every line, one row each, on a wide window.
class LinesColumn extends StatelessWidget {
  const LinesColumn({super.key, required this.model});
  final PhoneModel model;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final snap = model.snapshot;
    final chosen = chosenLine(model)?.line;
    return Column(
      key: const Key('lines'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 16, 16, 8),
          child: Text(
            s.linesTitle,
            style: Theme.of(context).textTheme.titleSmall,
          ),
        ),
        Expanded(
          child: ListView(
            children: [
              for (final line in snap.lines)
                LineTile(
                  model: model,
                  line: line,
                  selected: line.line == chosen,
                  onTap: () => model.chooseLine(line.line),
                ),
            ],
          ),
        ),
      ],
    );
  }
}

/// Above the keypad on a narrow window: the line calls go from, and a
/// sheet of every line to choose another.
class LinePicker extends StatelessWidget {
  const LinePicker({super.key, required this.model});
  final PhoneModel model;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final line = chosenLine(model);
    if (line == null) return const SizedBox.shrink();
    return Padding(
      padding: const EdgeInsets.only(bottom: 8),
      child: OutlinedButton.icon(
        key: const Key('linePicker'),
        onPressed: () => _choose(context),
        icon: const Icon(Icons.phone_forwarded_outlined, size: 18),
        label: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Flexible(
              child: Text(
                s.lineFrom(line.label),
                overflow: TextOverflow.ellipsis,
              ),
            ),
            const Icon(Icons.arrow_drop_down),
          ],
        ),
      ),
    );
  }

  Future<void> _choose(BuildContext context) => showModalBottomSheet<void>(
    context: context,
    showDragHandle: true,
    builder: (sheet) => ListenableBuilder(
      listenable: model,
      builder: (sheet, _) {
        final s = Strings.of(sheet);
        final chosen = chosenLine(model)?.line;
        return SafeArea(
          child: ListView(
            key: const Key('lineSheet'),
            shrinkWrap: true,
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
                child: Text(
                  s.lineChoose,
                  style: Theme.of(sheet).textTheme.titleMedium,
                ),
              ),
              for (final line in model.snapshot.lines)
                LineTile(
                  model: model,
                  line: line,
                  selected: line.line == chosen,
                  onTap: () {
                    model.chooseLine(line.line);
                    Navigator.of(sheet).pop();
                  },
                ),
            ],
          ),
        );
      },
    ),
  );
}

/// One line: its label and address, what it is doing, its messages, and
/// a menu to rename it where the tenant allows.
class LineTile extends StatelessWidget {
  const LineTile({
    super.key,
    required this.model,
    required this.line,
    required this.selected,
    required this.onTap,
  });
  final PhoneModel model;
  final PhoneLine line;
  final bool selected;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final scheme = Theme.of(context).colorScheme;
    final snap = model.snapshot;
    final calls = snap.callsOn(line.line);
    // A call on the line says more than its registration does.
    final (status, color) = switch (line.registration) {
      _ when calls.any((c) => c.isRingingIn) => (
        s.lineRinging,
        AnvilColors.answer,
      ),
      _ when calls.any((c) => !c.held) => (s.lineOnCall, AnvilColors.answer),
      _ when calls.isNotEmpty => (s.lineHeld, AnvilColors.held),
      Registration.failed ||
      Registration.unregistered => (s.lineNotRegistered, scheme.error),
      Registration.registering => (s.lineRegistering, null),
      Registration.registered => (s.lineReady, null),
    };
    final waiting = line.voicemailNew ?? 0;
    return ListTile(
      key: Key('line-${line.line}'),
      selected: selected,
      onTap: onTap,
      leading: Icon(
        line.shared
            ? Icons.groups_outlined
            : line.kind == 'on_behalf'
            ? Icons.support_agent
            : Icons.phone_outlined,
        color: color,
      ),
      title: Text(line.label, overflow: TextOverflow.ellipsis),
      subtitle: Text(
        [
          status,
          if (!line.ring) s.lineSilent,
          if (waiting > 0) s.lineVoicemail(waiting),
        ].join(' · '),
        key: Key('lineStatus-${line.line}'),
        style: color == null ? null : TextStyle(color: color),
      ),
      trailing: snap.mayRenameLines
          ? PopupMenuButton<void>(
              key: Key('lineMenu-${line.line}'),
              itemBuilder: (_) => [
                PopupMenuItem(
                  key: Key('lineRename-${line.line}'),
                  onTap: () => renameLine(context, model, line),
                  child: Text(s.lineRename),
                ),
              ],
            )
          : waiting > 0
          ? Badge(label: Text('$waiting'), child: const Icon(Icons.voicemail))
          : null,
    );
  }
}

/// Ask for a line's name and keep it on FCP; Reset gives the line back
/// its administrator's label.
Future<void> renameLine(
  BuildContext context,
  PhoneModel model,
  PhoneLine line,
) async {
  final name = await showDialog<String>(
    context: context,
    builder: (dialog) => _RenameDialog(line: line),
  );
  if (name != null) await model.renameLine(line.line, name);
}

class _RenameDialog extends StatefulWidget {
  const _RenameDialog({required this.line});
  final PhoneLine line;

  @override
  State<_RenameDialog> createState() => _RenameDialogState();
}

class _RenameDialogState extends State<_RenameDialog> {
  late final _name = TextEditingController(text: widget.line.label);

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final line = widget.line;
    return AlertDialog(
      title: Text(s.lineRenameTitle),
      content: TextField(
        key: const Key('lineName'),
        controller: _name,
        autofocus: true,
        decoration: InputDecoration(hintText: line.defaultLabel),
        onSubmitted: (v) => Navigator.of(context).pop(v),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: Text(s.cancelButton),
        ),
        if (line.renamed)
          TextButton(
            key: const Key('lineNameReset'),
            onPressed: () => Navigator.of(context).pop(''),
            child: Text(s.lineRenameReset),
          ),
        FilledButton(
          key: const Key('lineNameSave'),
          onPressed: () => Navigator.of(context).pop(_name.text),
          child: Text(s.saveButton),
        ),
      ],
    );
  }
}
