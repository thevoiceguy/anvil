// The user's settings: do not disturb, call waiting and forwarding (FCP's,
// shared with every device), this device's microphone and speaker, and the
// account (`docs/APP.md` §4, Settings).

import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:url_launcher/url_launcher.dart';

import '../../l10n/app_localizations.dart';
import '../desktop/shell.dart';
import '../mobile/bridge.dart';
import '../update/update_banner.dart';
import '../update/updates.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';
import 'audio_controls.dart';

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
        // A phone chooses where a call's audio goes in the call itself.
        if (MobileScope.maybeOf(context) == null) ...[
          header(s.settingsAudio),
          AudioControls(snap: snap, model: model),
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16),
            child: Text(s.audioNote, style: theme.textTheme.bodySmall),
          ),
          DevicePicker(
            key: const Key('ringDevice'),
            label: s.ringOn,
            icon: Icons.notifications_active,
            devices: snap.outputs,
            chosen: snap.ringDevice,
            onChosen: (id) => model.run(() => model.api.chooseRingDevice(id)),
          ),
          if (snap.ringtones.isNotEmpty)
            _RingtonePicker(snap: snap, model: model),
          if (snap.echoCancellation case final on?)
            SwitchListTile(
              key: const Key('echoCancellation'),
              secondary: const Icon(Icons.surround_sound),
              title: Text(s.echoCancellation),
              subtitle: Text(s.echoCancellationNote),
              value: on,
              onChanged: (on) => model.run(
                () => model.api.setAudioProcessing(echoCancellation: on),
              ),
            ),
          if (snap.noiseSuppression case final on?)
            SwitchListTile(
              key: const Key('noiseSuppression'),
              secondary: const Icon(Icons.noise_aware),
              title: Text(s.noiseSuppression),
              subtitle: Text(s.noiseSuppressionNote),
              value: on,
              onChanged: (on) => model.run(
                () => model.api.setAudioProcessing(noiseSuppression: on),
              ),
            ),
        ],
        if (DesktopScope.maybeOf(context) case final shell?
            when shell.canStartAtLogin) ...[
          header(s.settingsDesktop),
          _StartAtLogin(shell: shell),
        ],
        if (UpdateScope.maybeOf(context) case final update?) ...[
          header(s.settingsUpdates),
          _Updates(scope: update, model: model),
        ],
        if (model.api.logPath != null || model.managed.isManaged) ...[
          header(s.settingsTroubleshooting),
          if (model.api.logPath case final log?) _LogFile(path: log),
          if (model.managed.isManaged)
            ListTile(
              key: const Key('managedBy'),
              leading: const Icon(Icons.business),
              title: Text(s.managedBy),
              subtitle: Text(model.managed.sources.join('\n')),
            ),
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
/// The ringtone: Anvil's own, or the brand's; choosing one plays it.
class _RingtonePicker extends StatelessWidget {
  const _RingtonePicker({required this.snap, required this.model});

  final PhoneSnapshot snap;
  final PhoneModel model;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    String name(Ringtone r) => switch (r.id) {
      'anvil' => s.ringtoneAnvil,
      'classic' => s.ringtoneClassic,
      _ => r.name,
    };
    final known = snap.ringtones.any((r) => r.id == snap.ringtone);
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
      child: DropdownMenu<String>(
        key: const Key('ringtone'),
        expandedInsets: EdgeInsets.zero,
        leadingIcon: const Icon(Icons.music_note),
        label: Text(s.ringtone),
        initialSelection: known ? snap.ringtone : null,
        dropdownMenuEntries: [
          for (final r in snap.ringtones)
            DropdownMenuEntry(value: r.id, label: name(r)),
        ],
        onSelected: (id) {
          if (id == null) return;
          model.run(() => model.api.chooseRingtone(id));
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

/// This copy's version, and checking for (and taking) a newer one.
class _Updates extends StatelessWidget {
  const _Updates({required this.scope, required this.model});
  final UpdateScope scope;
  final PhoneModel model;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final updates = scope.updates;
    return ListenableBuilder(
      listenable: updates,
      builder: (context, _) {
        final info = updates.available;
        final status = switch (updates.status) {
          UpdateStatus.idle => null,
          UpdateStatus.checking => s.updateChecking,
          UpdateStatus.upToDate => s.updateNone,
          UpdateStatus.available => s.updateAvailable(info!.version),
          UpdateStatus.downloading => s.updateDownloading,
          UpdateStatus.failed => s.updateFailed(updates.error ?? ''),
        };
        final busy =
            updates.status == UpdateStatus.checking ||
            updates.status == UpdateStatus.downloading;
        return ListTile(
          key: const Key('updates'),
          leading: const Icon(Icons.system_update),
          title: Text(s.versionLabel(updates.api.version())),
          subtitle: Text(
            [
              ?status,
              if (info != null && !info.canInstall && info.reason != null)
                info.reason!,
            ].join(' — '),
          ),
          trailing: info != null && info.canInstall
              ? FilledButton(
                  key: const Key('settingsUpdateNow'),
                  onPressed: busy || model.snapshot.calls.isNotEmpty
                      ? null
                      : () => startUpdate(model, updates, scope.quit),
                  child: Text(s.updateNow),
                )
              : OutlinedButton(
                  key: const Key('checkForUpdates'),
                  onPressed: busy ? null : updates.check,
                  child: Text(s.checkForUpdates),
                ),
        );
      },
    );
  }
}

/// Where the log file is, to copy or (on a desktop) to open its folder: what
/// a user sends when a call goes wrong.
class _LogFile extends StatelessWidget {
  const _LogFile({required this.path});
  final String path;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final desktop = Platform.isWindows || Platform.isMacOS || Platform.isLinux;
    return ListTile(
      key: const Key('logFile'),
      leading: const Icon(Icons.description_outlined),
      title: Text(s.logFile),
      subtitle: SelectableText(path),
      trailing: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          IconButton(
            key: const Key('logFileCopy'),
            tooltip: s.logFileCopy,
            icon: const Icon(Icons.copy),
            onPressed: () async {
              await Clipboard.setData(ClipboardData(text: path));
              if (context.mounted) {
                ScaffoldMessenger.maybeOf(context)
                    ?.showSnackBar(SnackBar(content: Text(s.logFileCopied)));
              }
            },
          ),
          if (desktop)
            IconButton(
              key: const Key('logFileOpenFolder'),
              tooltip: s.logFileOpenFolder,
              icon: const Icon(Icons.folder_open),
              onPressed: () => launchUrl(File(path).parent.uri),
            ),
        ],
      ),
    );
  }
}
