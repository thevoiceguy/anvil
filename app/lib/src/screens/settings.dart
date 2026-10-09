// The user's settings: do not disturb, call waiting and forwarding (FCP's,
// shared with every device), this device's microphone and speaker, and the
// account (`docs/APP.md` §4, Settings).

import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../desktop/shell.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';

class SettingsPage extends StatelessWidget {
  const SettingsPage({super.key, required this.model});
  final PhoneModel model;

  String _label(Strings s, Forward when) => switch (when) {
    Forward.all => s.forwardAll,
    Forward.busy => s.forwardBusy,
    Forward.noAnswer => s.forwardNoAnswer,
    Forward.unreachable => s.forwardUnreachable,
  };

  Future<void> _editForward(
    BuildContext context,
    Forward when,
    CallingSettings calling,
  ) async {
    final change = await showDialog<CallingChange>(
      context: context,
      builder: (context) => _ForwardDialog(
        title: _label(Strings.of(context), when),
        when: when,
        current: calling.forward(when) ?? '',
        ringSecs: calling.noAnswerSecs,
      ),
    );
    if (change != null) await model.run(() => model.api.setCalling(change));
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final theme = Theme.of(context);
    final snap = model.snapshot;
    final calling = snap.calling;
    Widget header(String text) => Padding(
      padding: const EdgeInsets.fromLTRB(16, 24, 16, 4),
      child: Text(text, style: theme.textTheme.titleSmall),
    );
    return ListView(
      children: [
        header(s.settingsCalls),
        if (calling == null)
          ListTile(
            key: const Key('needsFcp'),
            leading: const Icon(Icons.info_outline),
            title: Text(s.needsFcp),
          )
        else ...[
          SwitchListTile(
            key: const Key('dnd'),
            title: Text(s.dndOn),
            subtitle: Text(s.dndSubtitle),
            value: calling.dnd,
            onChanged: (on) => model.run(() => model.api.setDnd(on)),
          ),
          SwitchListTile(
            key: const Key('callWaiting'),
            title: Text(s.callWaiting),
            subtitle: Text(s.callWaitingSubtitle),
            value: calling.callWaiting,
            onChanged: (on) => model.run(
              () => model.api.setCalling(CallingChange(callWaiting: on)),
            ),
          ),
          header(s.settingsForwarding),
          for (final when in Forward.values)
            ListTile(
              key: Key('forward-${when.name}'),
              leading: const Icon(Icons.phone_forwarded),
              title: Text(_label(s, when)),
              subtitle: Text(calling.forward(when) ?? s.forwardOff),
              trailing: const Icon(Icons.chevron_right),
              onTap: () => _editForward(context, when, calling),
            ),
        ],
        header(s.settingsAudio),
        _DevicePicker(
          key: const Key('microphone'),
          label: s.microphone,
          icon: Icons.mic,
          devices: snap.inputs,
          chosen: snap.input,
          onChosen: (id) =>
              model.run(() => model.api.chooseAudio(input: true, device: id)),
        ),
        _DevicePicker(
          key: const Key('speaker'),
          label: s.speaker,
          icon: Icons.volume_up,
          devices: snap.outputs,
          chosen: snap.output,
          onChosen: (id) =>
              model.run(() => model.api.chooseAudio(input: false, device: id)),
        ),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16),
          child: Text(s.audioNote, style: theme.textTheme.bodySmall),
        ),
        if (DesktopScope.maybeOf(context) case final shell?
            when shell.canStartAtLogin) ...[
          header(s.settingsDesktop),
          _StartAtLogin(shell: shell),
        ],
        header(s.settingsAccount),
        ListTile(
          leading: const Icon(Icons.person),
          title: Text(
            s.signedInAs(
              model.account?.username ?? '',
              model.account?.server ?? '',
            ),
          ),
          subtitle: Text(snap.aor),
        ),
        Padding(
          padding: const EdgeInsets.all(16),
          child: OutlinedButton.icon(
            key: const Key('signOut'),
            onPressed: model.signOut,
            icon: const Icon(Icons.logout),
            label: Text(s.signOut),
          ),
        ),
      ],
    );
  }
}

/// A microphone or speaker: the system's default, or one by name.
class _DevicePicker extends StatelessWidget {
  const _DevicePicker({
    super.key,
    required this.label,
    required this.icon,
    required this.devices,
    required this.chosen,
    required this.onChosen,
  });

  final String label;
  final IconData icon;
  final List<AudioDevice> devices;
  final String? chosen;
  final ValueChanged<String?> onChosen;

  // DropdownMenu's entries need a value; the system's default is ''.
  static const _system = '';

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final known = devices.any((d) => d.id == chosen);
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
      child: DropdownMenu<String>(
        expandedInsets: EdgeInsets.zero,
        leadingIcon: Icon(icon),
        label: Text(label),
        initialSelection: known ? chosen : _system,
        dropdownMenuEntries: [
          DropdownMenuEntry(value: _system, label: s.systemDefault),
          for (final d in devices)
            DropdownMenuEntry(value: d.id, label: d.name),
        ],
        onSelected: (id) {
          if (id == null) return;
          onChosen(id == _system ? null : id);
        },
      ),
    );
  }
}

class _ForwardDialog extends StatefulWidget {
  const _ForwardDialog({
    required this.title,
    required this.when,
    required this.current,
    this.ringSecs,
  });

  final String title;
  final Forward when;
  final String current;
  final int? ringSecs;

  @override
  State<_ForwardDialog> createState() => _ForwardDialogState();
}

class _ForwardDialogState extends State<_ForwardDialog> {
  late final _to = TextEditingController(text: widget.current);
  late final _secs = TextEditingController(
    text: widget.ringSecs?.toString() ?? '',
  );

  @override
  void dispose() {
    _to.dispose();
    _secs.dispose();
    super.dispose();
  }

  CallingChange _change(String to) {
    final forward = CallingChange.forward(widget.when, to);
    final secs = int.tryParse(_secs.text.trim());
    if (widget.when != Forward.noAnswer || secs == null) return forward;
    return CallingChange(
      forwardNoAnswer: forward.forwardNoAnswer,
      noAnswerSecs: secs,
    );
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    return AlertDialog(
      title: Text(widget.title),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          TextField(
            key: const Key('forwardTarget'),
            controller: _to,
            autofocus: true,
            decoration: InputDecoration(labelText: s.forwardTo),
          ),
          if (widget.when == Forward.noAnswer) ...[
            const SizedBox(height: 12),
            TextField(
              key: const Key('forwardSecs'),
              controller: _secs,
              keyboardType: TextInputType.number,
              decoration: InputDecoration(labelText: s.forwardRingSeconds),
            ),
          ],
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: Text(s.cancelButton),
        ),
        TextButton(
          key: const Key('forwardClear'),
          onPressed: () => Navigator.of(context).pop(_change('')),
          child: Text(s.clearButton),
        ),
        FilledButton(
          key: const Key('forwardSave'),
          onPressed: () => Navigator.of(context).pop(_change(_to.text.trim())),
          child: Text(s.saveButton),
        ),
      ],
    );
  }
}

/// Start at login: read from the system, changed there.
class _StartAtLogin extends StatefulWidget {
  const _StartAtLogin({required this.shell});
  final DesktopShell shell;

  @override
  State<_StartAtLogin> createState() => _StartAtLoginState();
}

class _StartAtLoginState extends State<_StartAtLogin> {
  bool? _on;

  @override
  void initState() {
    super.initState();
    widget.shell.startsAtLogin().then((on) {
      if (mounted) setState(() => _on = on);
    });
  }

  Future<void> _set(bool on) async {
    setState(() => _on = on);
    try {
      await widget.shell.setStartsAtLogin(on);
    } finally {
      final now = await widget.shell.startsAtLogin();
      if (mounted) setState(() => _on = now);
    }
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    return SwitchListTile(
      key: const Key('startAtLogin'),
      title: Text(s.startAtLogin),
      subtitle: Text(s.startAtLoginSubtitle),
      value: _on ?? false,
      onChanged: _on == null ? null : _set,
    );
  }
}
