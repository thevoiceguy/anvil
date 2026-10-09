// The mailbox: each message with its transcription, played through the
// speaker calls use, marked heard, deleted (`docs/APP.md` §4, Voicemail).

import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../design/format.dart';
import '../design/theme.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';

class VoicemailPage extends StatelessWidget {
  const VoicemailPage({super.key, required this.model, required this.onDial});
  final PhoneModel model;
  final ValueChanged<String> onDial;

  Future<void> _delete(BuildContext context, VoicemailMessage m) async {
    final s = Strings.of(context);
    final yes = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        content: Text(s.deleteMessageConfirm(m.who)),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: Text(s.cancelButton),
          ),
          FilledButton(
            key: const Key('confirmDelete'),
            style: FilledButton.styleFrom(backgroundColor: AnvilColors.hangup),
            onPressed: () => Navigator.of(context).pop(true),
            child: Text(s.deleteButton),
          ),
        ],
      ),
    );
    if (yes == true) await model.run(() => model.api.deleteVoicemail(m.id));
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final snap = model.snapshot;
    final messages = snap.voicemail;
    final theme = Theme.of(context);
    return RefreshIndicator(
      onRefresh: () => model.run(model.api.refresh),
      child: ListView(
        padding: const EdgeInsets.all(12),
        children: [
          if (messages.isEmpty)
            Padding(
              padding: const EdgeInsets.all(32),
              child: Center(child: Text(s.voicemailEmpty)),
            ),
          for (final m in messages)
            Card(
              key: Key('message-${m.id}'),
              child: Padding(
                padding: const EdgeInsets.fromLTRB(16, 12, 8, 4),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        if (m.isNew)
                          Padding(
                            padding: const EdgeInsets.only(right: 8),
                            child: Icon(
                              Icons.circle,
                              key: Key('new-${m.id}'),
                              size: 10,
                              color: theme.colorScheme.primary,
                            ),
                          ),
                        Expanded(
                          child: Text(
                            snap.personFor(m.caller)?.name ?? m.who,
                            style: theme.textTheme.titleMedium?.copyWith(
                              fontWeight: m.isNew ? FontWeight.w700 : null,
                            ),
                          ),
                        ),
                        if (m.urgent)
                          Padding(
                            padding: const EdgeInsets.only(right: 8),
                            child: Chip(
                              label: Text(s.urgent),
                              visualDensity: VisualDensity.compact,
                            ),
                          ),
                        Text(
                          [
                            when(context, m.receivedAt),
                            duration(m.durationSecs),
                          ].where((t) => t.isNotEmpty).join(' · '),
                          style: theme.textTheme.bodySmall,
                        ),
                      ],
                    ),
                    if (m.transcription != null) ...[
                      const SizedBox(height: 8),
                      Text(m.transcription!, key: Key('transcript-${m.id}')),
                    ],
                    Wrap(
                      spacing: 4,
                      children: [
                        snap.playing == m.id
                            ? TextButton.icon(
                                key: Key('stop-${m.id}'),
                                onPressed: () =>
                                    model.run(model.api.stopPlaying),
                                icon: const Icon(Icons.stop),
                                label: Text(s.stopButton),
                              )
                            : TextButton.icon(
                                key: Key('play-${m.id}'),
                                onPressed: () =>
                                    model.run(() => model.api.play(m.id)),
                                icon: const Icon(Icons.play_arrow),
                                label: Text(s.playButton),
                              ),
                        TextButton.icon(
                          key: Key('callBack-${m.id}'),
                          onPressed: () => onDial(m.caller),
                          icon: const Icon(Icons.call),
                          label: Text(s.callBackButton),
                        ),
                        if (m.isNew)
                          TextButton.icon(
                            key: Key('heard-${m.id}'),
                            onPressed: () =>
                                model.run(() => model.api.markHeard(m.id)),
                            icon: const Icon(Icons.done),
                            label: Text(s.markHeardButton),
                          ),
                        TextButton.icon(
                          key: Key('delete-${m.id}'),
                          onPressed: () => _delete(context, m),
                          icon: const Icon(Icons.delete_outline),
                          label: Text(s.deleteButton),
                        ),
                      ],
                    ),
                  ],
                ),
              ),
            ),
        ],
      ),
    );
  }
}
