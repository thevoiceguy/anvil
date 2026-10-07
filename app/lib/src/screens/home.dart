import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../design/theme.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';

class HomeScreen extends StatelessWidget {
  const HomeScreen({super.key, required this.model});
  final PhoneModel model;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final snap = model.snapshot;
    final ringing = snap.calls.where((c) => c.isRingingIn).toList();
    final active = snap.calls
        .where(
          (c) =>
              c.state != CallState.ringing || c.direction == Direction.outgoing,
        )
        .toList();
    return Scaffold(
      appBar: AppBar(
        leading: snap.brandLogo == null
            ? null
            : Padding(
                padding: const EdgeInsets.all(8),
                child: Image.memory(
                  snap.brandLogo!,
                  key: const Key('brandLogo'),
                  errorBuilder: (_, _, _) => const SizedBox.shrink(),
                ),
              ),
        title: Text(
          snap.brandName ?? model.account?.username ?? s.appName,
          key: const Key('title'),
        ),
        actions: [
          _RegistrationChip(registration: snap.registration),
          if (snap.dnd == true)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8),
              child: Chip(label: Text(s.dndOn)),
            ),
          IconButton(
            key: const Key('signOut'),
            tooltip: s.signOut,
            icon: const Icon(Icons.logout),
            onPressed: model.signOut,
          ),
        ],
      ),
      body: SafeArea(
        child: Column(
          children: [
            if (snap.voicemailNew > 0)
              ListTile(
                leading: const Icon(Icons.voicemail),
                title: Text(s.voicemailCount(snap.voicemailNew)),
              ),
            for (final call in ringing)
              IncomingCallCard(model: model, call: call),
            for (final call in active) CallCard(model: model, call: call),
            if (model.notice != null)
              Padding(
                padding: const EdgeInsets.all(12),
                child: Text(model.notice!, key: const Key('notice')),
              ),
            const Spacer(),
            if (active.isEmpty && ringing.isEmpty) Keypad(model: model),
          ],
        ),
      ),
    );
  }
}

class _RegistrationChip extends StatelessWidget {
  const _RegistrationChip({required this.registration});
  final Registration registration;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final (label, color) = switch (registration) {
      Registration.registered => (s.registrationRegistered, AnvilColors.answer),
      Registration.registering => (s.registrationRegistering, Colors.amber),
      Registration.unregistered => (s.registrationUnregistered, Colors.grey),
      Registration.failed => (s.registrationFailed, AnvilColors.hangup),
    };
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 8),
      child: Chip(
        key: const Key('registration'),
        avatar: CircleAvatar(backgroundColor: color, radius: 5),
        label: Text(label),
      ),
    );
  }
}

class IncomingCallCard extends StatelessWidget {
  const IncomingCallCard({super.key, required this.model, required this.call});
  final PhoneModel model;
  final PhoneCall call;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    return Card(
      margin: const EdgeInsets.all(12),
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          children: [
            Text(s.incomingCall, style: Theme.of(context).textTheme.labelLarge),
            const SizedBox(height: 4),
            Text(call.who, style: Theme.of(context).textTheme.headlineSmall),
            const SizedBox(height: 16),
            Row(
              mainAxisAlignment: MainAxisAlignment.spaceEvenly,
              children: [
                FilledButton.icon(
                  key: const Key('decline'),
                  style: FilledButton.styleFrom(
                    backgroundColor: AnvilColors.hangup,
                  ),
                  onPressed: () => model.run(() => model.api.decline(call.id)),
                  icon: const Icon(Icons.call_end),
                  label: Text(s.declineButton),
                ),
                FilledButton.icon(
                  key: const Key('answer'),
                  style: FilledButton.styleFrom(
                    backgroundColor: AnvilColors.answer,
                  ),
                  onPressed: () => model.run(() => model.api.answer(call.id)),
                  icon: const Icon(Icons.call),
                  label: Text(s.answerButton),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

class CallCard extends StatelessWidget {
  const CallCard({super.key, required this.model, required this.call});
  final PhoneModel model;
  final PhoneCall call;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final status = call.held
        ? s.callHeld
        : switch (call.state) {
            CallState.dialing => s.callDialing,
            CallState.ringing => s.callRinging,
            CallState.connected => s.callConnected,
          };
    final connected = call.state == CallState.connected;
    return Card(
      margin: const EdgeInsets.all(12),
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          children: [
            Text(call.who, style: Theme.of(context).textTheme.headlineSmall),
            Text(status, key: const Key('callStatus')),
            const SizedBox(height: 16),
            Wrap(
              spacing: 12,
              alignment: WrapAlignment.center,
              children: [
                if (connected)
                  OutlinedButton.icon(
                    key: const Key('mute'),
                    onPressed: () =>
                        model.run(() => model.api.mute(call.id, !call.muted)),
                    icon: Icon(call.muted ? Icons.mic_off : Icons.mic),
                    label: Text(call.muted ? s.unmuteButton : s.muteButton),
                  ),
                if (connected)
                  OutlinedButton.icon(
                    key: const Key('hold'),
                    onPressed: () =>
                        model.run(() => model.api.hold(call.id, !call.held)),
                    icon: Icon(call.held ? Icons.play_arrow : Icons.pause),
                    label: Text(call.held ? s.resumeButton : s.holdButton),
                  ),
                FilledButton.icon(
                  key: const Key('hangup'),
                  style: FilledButton.styleFrom(
                    backgroundColor: AnvilColors.hangup,
                  ),
                  onPressed: () => model.run(() => model.api.hangup(call.id)),
                  icon: const Icon(Icons.call_end),
                  label: Text(s.hangupButton),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

class Keypad extends StatefulWidget {
  const Keypad({super.key, required this.model});
  final PhoneModel model;

  @override
  State<Keypad> createState() => _KeypadState();
}

class _KeypadState extends State<Keypad> {
  final _target = TextEditingController();
  static const _keys = [
    '1',
    '2',
    '3',
    '4',
    '5',
    '6',
    '7',
    '8',
    '9',
    '*',
    '0',
    '#',
  ];

  void _press(String key) => setState(() => _target.text += key);

  void _call() {
    final target = _target.text.trim();
    if (target.isEmpty) return;
    widget.model.run(() => widget.model.api.call(target));
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    return ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: 320),
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              key: const Key('target'),
              controller: _target,
              textAlign: TextAlign.center,
              style: Theme.of(context).textTheme.headlineSmall,
              decoration: InputDecoration(hintText: s.keypadHint),
              onSubmitted: (_) => _call(),
            ),
            const SizedBox(height: 12),
            GridView.count(
              crossAxisCount: 3,
              shrinkWrap: true,
              mainAxisSpacing: 8,
              crossAxisSpacing: 8,
              childAspectRatio: 1.6,
              physics: const NeverScrollableScrollPhysics(),
              children: [
                for (final k in _keys)
                  OutlinedButton(
                    key: Key('key$k'),
                    onPressed: () => _press(k),
                    child: Text(
                      k,
                      style: Theme.of(context).textTheme.titleLarge,
                    ),
                  ),
              ],
            ),
            const SizedBox(height: 12),
            FilledButton.icon(
              key: const Key('call'),
              style: FilledButton.styleFrom(
                backgroundColor: AnvilColors.answer,
                minimumSize: const Size.fromHeight(52),
              ),
              onPressed: _call,
              icon: const Icon(Icons.call),
              label: Text(s.callButton),
            ),
          ],
        ),
      ),
    );
  }
}
