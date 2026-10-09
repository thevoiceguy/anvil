// A call ringing in, the calls up and what can be done with them, and the
// keypad that places them (`docs/APP.md` §4: incoming, in a call, keypad).

import 'dart:async';

import 'package:clock/clock.dart';
import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../design/format.dart';
import '../design/theme.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';

/// The call the in-call screen is about: the one up, else one being placed,
/// else a held one.
PhoneCall? currentCall(List<PhoneCall> calls) {
  final mine = calls.where((c) => !c.isRingingIn).toList();
  for (final pick in [
    (PhoneCall c) => c.isConnected && !c.held,
    (PhoneCall c) => !c.isConnected,
    (PhoneCall c) => true,
  ]) {
    for (final c in mine) {
      if (pick(c)) return c;
    }
  }
  return null;
}

/// The name a call's party goes by: the directory's, else what the call
/// says.
String nameFor(PhoneSnapshot snap, PhoneCall call) =>
    call.displayName ?? snap.personFor(call.remote)?.name ?? call.who;

class IncomingCallCard extends StatelessWidget {
  const IncomingCallCard({super.key, required this.model, required this.call});
  final PhoneModel model;
  final PhoneCall call;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final theme = Theme.of(context);
    final person = model.snapshot.personFor(call.remote);
    final name = nameFor(model.snapshot, call);
    final number = shortAddress(call.remote);
    return Card(
      key: Key('incoming-${call.id}'),
      margin: const EdgeInsets.fromLTRB(12, 12, 12, 0),
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          children: [
            Text(s.incomingCall, style: theme.textTheme.labelLarge),
            const SizedBox(height: 4),
            Text(name, style: theme.textTheme.headlineSmall),
            if (name != number) Text(number),
            if (person?.department != null)
              Text(person!.department!, style: theme.textTheme.bodySmall),
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

/// The calls up: the current one with its controls, the others held below.
class InCallPanel extends StatefulWidget {
  const InCallPanel({super.key, required this.model, required this.onAddCall});
  final PhoneModel model;

  /// Show the keypad to place a second call.
  final VoidCallback onAddCall;

  @override
  State<InCallPanel> createState() => _InCallPanelState();
}

class _InCallPanelState extends State<InCallPanel> {
  bool _keypad = false;
  String _sent = '';

  PhoneModel get model => widget.model;

  Future<void> _transfer(PhoneCall call) async {
    final s = Strings.of(context);
    final choice = await showDialog<(String, bool)>(
      context: context,
      builder: (context) => TransferDialog(
        title: s.transferTitle(nameFor(model.snapshot, call)),
        people: model.snapshot.people,
      ),
    );
    if (choice == null) return;
    final (target, consult) = choice;
    if (consult) {
      await model.consult(call.id, target);
    } else {
      await model.run(() => model.api.transfer(call.id, target));
    }
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final theme = Theme.of(context);
    final snap = model.snapshot;
    final call = currentCall(snap.calls);
    if (call == null) return const SizedBox.shrink();
    final others = snap.calls
        .where((c) => c.id != call.id && !c.isRingingIn)
        .toList();
    final connected = call.isConnected;
    final name = nameFor(snap, call);
    final number = shortAddress(call.remote);
    final canComplete =
        model.consulting != null &&
        model.consulting != call.id &&
        connected &&
        !call.held &&
        others.any((c) => c.id == model.consulting);

    return SingleChildScrollView(
      padding: const EdgeInsets.all(16),
      child: Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 420),
          child: Column(
            children: [
              const SizedBox(height: 8),
              Text(
                name,
                key: const Key('callName'),
                style: theme.textTheme.headlineMedium,
                textAlign: TextAlign.center,
              ),
              if (name != number)
                Text(number, style: theme.textTheme.bodyLarge),
              const SizedBox(height: 4),
              _Status(call: call),
              const SizedBox(height: 8),
              if (connected) _MediaLine(call: call),
              const SizedBox(height: 24),
              if (_keypad && connected)
                DigitPad(
                  sent: _sent,
                  onDigit: (d) {
                    setState(() => _sent += d);
                    model.run(() => model.api.sendDigits(call.id, d));
                  },
                )
              else
                Wrap(
                  spacing: 12,
                  runSpacing: 12,
                  alignment: WrapAlignment.center,
                  children: [
                    _RoundButton(
                      key: const Key('mute'),
                      icon: call.muted ? Icons.mic_off : Icons.mic,
                      label: call.muted ? s.unmuteButton : s.muteButton,
                      on: call.muted,
                      onPressed: connected
                          ? () => model.run(
                              () => model.api.mute(call.id, !call.muted),
                            )
                          : null,
                    ),
                    _RoundButton(
                      key: const Key('hold'),
                      icon: call.held ? Icons.play_arrow : Icons.pause,
                      label: call.held ? s.resumeButton : s.holdButton,
                      on: call.held,
                      onPressed: connected
                          ? () => model.run(
                              () => model.api.hold(call.id, !call.held),
                            )
                          : null,
                    ),
                    _RoundButton(
                      key: const Key('keypad'),
                      icon: Icons.dialpad,
                      label: s.keypadButton,
                      onPressed: connected
                          ? () => setState(() => _keypad = true)
                          : null,
                    ),
                    _RoundButton(
                      key: const Key('transfer'),
                      icon: Icons.phone_forwarded,
                      label: s.transferButton,
                      onPressed: connected ? () => _transfer(call) : null,
                    ),
                    _RoundButton(
                      key: const Key('park'),
                      icon: Icons.local_parking,
                      label: s.parkButton,
                      onPressed: connected
                          ? () => model.run(() => model.api.park(call.id))
                          : null,
                    ),
                    _RoundButton(
                      key: const Key('addCall'),
                      icon: Icons.add_call,
                      label: s.addCallButton,
                      onPressed: connected ? widget.onAddCall : null,
                    ),
                  ],
                ),
              if (_keypad && connected)
                TextButton(
                  key: const Key('hideKeypad'),
                  onPressed: () => setState(() {
                    _keypad = false;
                    _sent = '';
                  }),
                  child: Text(s.hideKeypadButton),
                ),
              const SizedBox(height: 24),
              if (canComplete)
                Padding(
                  padding: const EdgeInsets.only(bottom: 12),
                  child: FilledButton.icon(
                    key: const Key('completeTransfer'),
                    style: FilledButton.styleFrom(
                      minimumSize: const Size.fromHeight(52),
                    ),
                    onPressed: () => model.completeTransfer(call.id),
                    icon: const Icon(Icons.call_merge),
                    label: Text(s.completeTransferButton),
                  ),
                ),
              FilledButton.icon(
                key: const Key('hangup'),
                style: FilledButton.styleFrom(
                  backgroundColor: AnvilColors.hangup,
                  minimumSize: const Size.fromHeight(52),
                ),
                onPressed: () => model.run(() => model.api.hangup(call.id)),
                icon: const Icon(Icons.call_end),
                label: Text(s.hangupButton),
              ),
              if (others.isNotEmpty) ...[
                const SizedBox(height: 24),
                Align(
                  alignment: Alignment.centerLeft,
                  child: Text(s.otherCalls, style: theme.textTheme.titleSmall),
                ),
                for (final other in others)
                  ListTile(
                    key: Key('other-${other.id}'),
                    contentPadding: EdgeInsets.zero,
                    leading: Icon(
                      other.held ? Icons.pause_circle : Icons.call,
                      color: other.held ? AnvilColors.held : null,
                    ),
                    title: Text(nameFor(snap, other)),
                    subtitle: _Status(call: other),
                    trailing: Wrap(
                      spacing: 4,
                      children: [
                        if (other.isConnected && other.held)
                          IconButton(
                            key: Key('swap-${other.id}'),
                            tooltip: s.resumeButton,
                            icon: const Icon(Icons.swap_calls),
                            onPressed: () => model.run(
                              () => model.api.hold(other.id, false),
                            ),
                          ),
                        IconButton(
                          key: Key('hangup-${other.id}'),
                          tooltip: s.hangupButton,
                          color: AnvilColors.hangup,
                          icon: const Icon(Icons.call_end),
                          onPressed: () =>
                              model.run(() => model.api.hangup(other.id)),
                        ),
                      ],
                    ),
                  ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// Where a call is: placing, ringing, held, or how long it has been up.
class _Status extends StatelessWidget {
  const _Status({required this.call});
  final PhoneCall call;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    if (call.held) return Text(s.callHeld, key: const Key('callStatus'));
    return switch (call.state) {
      CallState.dialing => Text(s.callDialing, key: const Key('callStatus')),
      CallState.ringing => Text(s.callRinging, key: const Key('callStatus')),
      CallState.connected =>
        call.connectedAt == null
            ? Text(s.callConnected, key: const Key('callStatus'))
            : CallTimer(since: call.connectedAt!, key: const Key('callStatus')),
    };
  }
}

/// How long a call has been up, counting.
class CallTimer extends StatefulWidget {
  const CallTimer({super.key, required this.since});
  final DateTime since;

  @override
  State<CallTimer> createState() => _CallTimerState();
}

class _CallTimerState extends State<CallTimer> {
  late final Timer _tick;

  @override
  void initState() {
    super.initState();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) => setState(() {}));
  }

  @override
  void dispose() {
    _tick.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final secs = clock.now().difference(widget.since).inSeconds;
    return Text(duration(secs < 0 ? 0 : secs));
  }
}

/// Encryption, the line's quality and the codec.
class _MediaLine extends StatelessWidget {
  const _MediaLine({required this.call});
  final PhoneCall call;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final small = Theme.of(context).textTheme.bodySmall;
    final quality = call.quality;
    final (qIcon, qColor, qLabel) = switch (quality?.level) {
      null => (null, null, null),
      QualityLevel.good => (
        Icons.signal_cellular_alt,
        AnvilColors.answer,
        s.qualityGood,
      ),
      QualityLevel.fair => (
        Icons.signal_cellular_alt_2_bar,
        AnvilColors.held,
        s.qualityFair,
      ),
      QualityLevel.poor => (
        Icons.signal_cellular_alt_1_bar,
        AnvilColors.hangup,
        s.qualityPoor,
      ),
    };
    return Wrap(
      spacing: 12,
      crossAxisAlignment: WrapCrossAlignment.center,
      alignment: WrapAlignment.center,
      children: [
        Tooltip(
          message: call.encrypted ? s.encrypted : s.notEncrypted,
          child: Icon(
            call.encrypted ? Icons.lock : Icons.lock_open,
            key: Key(call.encrypted ? 'encrypted' : 'notEncrypted'),
            size: 16,
          ),
        ),
        if (qIcon != null)
          Tooltip(
            message: qLabel!,
            child: Icon(
              qIcon,
              key: const Key('quality'),
              size: 16,
              color: qColor,
            ),
          ),
        if (call.codec != null) Text(call.codec!.toUpperCase(), style: small),
      ],
    );
  }
}

class _RoundButton extends StatelessWidget {
  const _RoundButton({
    super.key,
    required this.icon,
    required this.label,
    required this.onPressed,
    this.on = false,
  });

  final IconData icon;
  final String label;
  final VoidCallback? onPressed;
  final bool on;

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    return SizedBox(
      width: 88,
      child: Column(
        children: [
          IconButton.filledTonal(
            iconSize: 28,
            style: IconButton.styleFrom(
              fixedSize: const Size(60, 60),
              backgroundColor: on ? scheme.primary : null,
              foregroundColor: on ? scheme.onPrimary : null,
            ),
            onPressed: onPressed,
            icon: Icon(icon),
          ),
          const SizedBox(height: 4),
          Text(label, textAlign: TextAlign.center),
        ],
      ),
    );
  }
}

/// The keys of a phone: each press sent as it is made (DTMF).
class DigitPad extends StatelessWidget {
  const DigitPad({super.key, required this.onDigit, this.sent = ''});
  final ValueChanged<String> onDigit;
  final String sent;

  static const keys = [
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

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return ConstrainedBox(
      constraints: const BoxConstraints(maxWidth: 300),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (sent.isNotEmpty)
            Padding(
              padding: const EdgeInsets.only(bottom: 8),
              child: Text(
                sent,
                key: const Key('digitsSent'),
                style: theme.textTheme.titleLarge,
              ),
            ),
          GridView.count(
            crossAxisCount: 3,
            shrinkWrap: true,
            mainAxisSpacing: 8,
            crossAxisSpacing: 8,
            childAspectRatio: 1.6,
            physics: const NeverScrollableScrollPhysics(),
            children: [
              for (final k in keys)
                OutlinedButton(
                  key: Key('key$k'),
                  onPressed: () => onDigit(k),
                  child: Text(k, style: theme.textTheme.titleLarge),
                ),
            ],
          ),
        ],
      ),
    );
  }
}

/// A transfer's target: typed, or picked from the directory; then at once
/// or after talking to them first.
class TransferDialog extends StatefulWidget {
  const TransferDialog({super.key, required this.title, required this.people});
  final String title;
  final List<Person> people;

  @override
  State<TransferDialog> createState() => _TransferDialogState();
}

class _TransferDialogState extends State<TransferDialog> {
  final _target = TextEditingController();

  @override
  void dispose() {
    _target.dispose();
    super.dispose();
  }

  void _done(bool consult) {
    final target = _target.text.trim();
    if (target.isEmpty) return;
    Navigator.of(context).pop((target, consult));
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final query = _target.text;
    final matches = query.isEmpty
        ? const <Person>[]
        : widget.people.where((p) => p.matches(query)).take(5).toList();
    return AlertDialog(
      title: Text(widget.title),
      content: SizedBox(
        width: 360,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            TextField(
              key: const Key('transferTarget'),
              controller: _target,
              autofocus: true,
              decoration: InputDecoration(hintText: s.keypadHint),
              onChanged: (_) => setState(() {}),
              onSubmitted: (_) => _done(false),
            ),
            for (final p in matches)
              ListTile(
                key: Key('transferPick-${p.key}'),
                dense: true,
                title: Text(p.name),
                subtitle: p.extension == null ? null : Text(p.extension!),
                onTap: () => setState(() {
                  _target.text = p.dial;
                  _target.selection = TextSelection.collapsed(
                    offset: p.dial.length,
                  );
                }),
              ),
          ],
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: Text(s.cancelButton),
        ),
        OutlinedButton(
          key: const Key('transferConsult'),
          onPressed: () => _done(true),
          child: Text(s.transferConsult),
        ),
        FilledButton(
          key: const Key('transferNow'),
          onPressed: () => _done(false),
          child: Text(s.transferNow),
        ),
      ],
    );
  }
}

/// A call up while another page shows: who, and the way back to it.
class CallBar extends StatelessWidget {
  const CallBar({super.key, required this.model, required this.onOpen});
  final PhoneModel model;
  final VoidCallback onOpen;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final call = currentCall(model.snapshot.calls);
    if (call == null) return const SizedBox.shrink();
    final scheme = Theme.of(context).colorScheme;
    return Material(
      color: call.held ? AnvilColors.held : scheme.primaryContainer,
      child: InkWell(
        key: const Key('callBar'),
        onTap: onOpen,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
          child: Row(
            children: [
              const Icon(Icons.call, size: 18),
              const SizedBox(width: 8),
              Expanded(child: Text(nameFor(model.snapshot, call))),
              _Status(call: call),
              const SizedBox(width: 8),
              Text(s.backToCallButton),
            ],
          ),
        ),
      ),
    );
  }
}

class Keypad extends StatefulWidget {
  const Keypad({super.key, required this.onCall, this.onBack});

  /// Place a call to what was typed.
  final ValueChanged<String> onCall;

  /// Back to the call up, when the keypad is for a second one.
  final VoidCallback? onBack;

  @override
  State<Keypad> createState() => _KeypadState();
}

class _KeypadState extends State<Keypad> {
  final _target = TextEditingController();

  @override
  void dispose() {
    _target.dispose();
    super.dispose();
  }

  void _call() {
    final target = _target.text.trim();
    if (target.isEmpty) return;
    widget.onCall(target);
    _target.clear();
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    return SingleChildScrollView(
      padding: const EdgeInsets.all(16),
      child: Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 320),
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
              DigitPad(onDigit: (k) => setState(() => _target.text += k)),
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
              if (widget.onBack != null)
                TextButton(
                  key: const Key('backToCall'),
                  onPressed: widget.onBack,
                  child: Text(s.backToCallButton),
                ),
            ],
          ),
        ),
      ),
    );
  }
}
