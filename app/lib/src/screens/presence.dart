// The user's presence (FCP's docs/PRESENCE.md, D8 finding 33): the chip
// at the top of the home screen says what others see — the status chosen,
// "On a call" over it while a call is up, away while idle — and opens the
// menu to pick a status, set a note, choose when it clears and edit the
// user's own statuses. Without an FCP that keeps presence, the chip is the
// registration's, as it always was.

import 'package:clock/clock.dart';
import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../design/theme.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';

/// The four statuses every user has, in the menu's order.
const builtInStatuses = ['available', 'away', 'busy', 'dnd'];

/// A state's name on screen.
String stateName(Strings s, String state) => switch (state) {
  'available' => s.presenceAvailable,
  'away' => s.presenceAway,
  'busy' => s.presenceBusy,
  'dnd' => s.presenceDnd,
  'ringing' => s.presenceRinging,
  'on_call' => s.onACall,
  _ => s.presenceOffline,
};

/// What a presence says: the user's own label in place of its state's
/// name, kept beside a call ("On a call · WFH"), and idle said so.
String shownText(Strings s, String state, String? label, {bool idle = false}) {
  if (idle) return s.presenceIdle;
  return switch (state) {
    'on_call' ||
    'ringing' when label != null => '${stateName(s, state)} · $label',
    'available' || 'away' || 'busy' || 'dnd' when label != null => label,
    _ => stateName(s, state),
  };
}

/// When a "clear after" choice ends, from `now`; null is never.
DateTime? clearAt(String choice, DateTime now) => switch (choice) {
  '30m' => now.add(const Duration(minutes: 30)),
  '1h' => now.add(const Duration(hours: 1)),
  '2h' => now.add(const Duration(hours: 2)),
  'today' => DateTime(now.year, now.month, now.day, 23, 59),
  _ => null,
};

/// The presence chip, or the registration's without presence.
class PresenceChip extends StatelessWidget {
  const PresenceChip({super.key, required this.model});
  final PhoneModel model;

  @override
  Widget build(BuildContext context) {
    final snap = model.snapshot;
    final presence = snap.presence;
    if (presence == null) {
      return RegistrationChip(registration: snap.registration);
    }
    final s = Strings.of(context);
    final registered = snap.registration == Registration.registered;
    // This app not registered: the user sees why, not "offline".
    final (label, avatar) = registered || presence.shown != 'offline'
        ? (
            shownText(
              s,
              presence.shown,
              presence.shownLabel,
              idle: presence.idle,
            ),
            CircleAvatar(
              backgroundColor: AnvilColors.presence(presence.shown),
              radius: 5,
            ),
          )
        : (
            registrationName(s, snap.registration),
            Icon(Icons.warning_amber, size: 16, color: Colors.amber.shade700),
          );
    return PopupMenuButton<String>(
      key: const Key('presence'),
      tooltip: s.presenceChoose,
      onSelected: (choice) => _chosen(context, choice),
      itemBuilder: (context) => _items(context, presence),
      child: Chip(
        avatar: avatar,
        label: Text(label, key: const Key('presenceLabel')),
        deleteIcon: const Icon(Icons.arrow_drop_down, size: 18),
        onDeleted: null,
      ),
    );
  }

  List<PopupMenuEntry<String>> _items(BuildContext context, MyPresence p) {
    final s = Strings.of(context);
    PopupMenuItem<String> status(String id, String label, String state) =>
        PopupMenuItem(
          key: Key('status-$id'),
          value: 'status:$id',
          child: Row(
            children: [
              CircleAvatar(
                backgroundColor: AnvilColors.presence(state),
                radius: 5,
              ),
              const SizedBox(width: 12),
              Expanded(child: Text(label)),
              if (p.status == id) const Icon(Icons.check, size: 18),
            ],
          ),
        );
    return [
      for (final b in builtInStatuses) status(b, stateName(s, b), b),
      if (p.statuses.isNotEmpty) const PopupMenuDivider(),
      for (final o in p.statuses) status(o.id, o.label, o.state),
      const PopupMenuDivider(),
      PopupMenuItem(
        key: const Key('presenceNote'),
        value: 'note',
        child: Text(
          p.note == null ? s.presenceNote : s.presenceNoteIs(p.note!),
        ),
      ),
      PopupMenuItem(
        key: const Key('presenceClearAfter'),
        value: 'clear',
        child: Text(
          p.until == null
              ? s.presenceClearAfter
              : s.presenceUntil(
                  MaterialLocalizations.of(context)
                      .formatTimeOfDay(TimeOfDay.fromDateTime(p.until!)),
                ),
        ),
      ),
      PopupMenuItem(
        key: const Key('editStatuses'),
        value: 'edit',
        child: Text(s.editStatuses),
      ),
    ];
  }

  Future<void> _chosen(BuildContext context, String choice) async {
    final p = model.snapshot.presence;
    if (p == null) return;
    if (choice.startsWith('status:')) {
      // A new status starts without the old one's note or end.
      await model.run(() => model.api.setPresence(choice.substring(7)));
    } else if (choice == 'note') {
      final note = await showDialog<String>(
        context: context,
        builder: (_) => _NoteDialog(note: p.note),
      );
      if (note != null) {
        await model.run(
          () => model.api.setPresenceNote(
            note.trim().isEmpty ? null : note.trim(),
          ),
        );
      }
    } else if (choice == 'clear' && context.mounted) {
      final when = await showDialog<String>(
        context: context,
        builder: (_) => const _ClearAfterDialog(),
      );
      if (when != null) {
        await model.run(
          () => model.api.setPresence(
            p.status,
            note: p.note,
            until: clearAt(when, clock.now()),
          ),
        );
      }
    } else if (choice == 'edit' && context.mounted) {
      await showDialog<void>(
        context: context,
        builder: (_) => StatusesDialog(model: model),
      );
    }
  }
}

/// A registration's name on screen.
String registrationName(Strings s, Registration r) => switch (r) {
  Registration.registered => s.registrationRegistered,
  Registration.registering => s.registrationRegistering,
  Registration.unregistered => s.registrationUnregistered,
  Registration.failed => s.registrationFailed,
};

/// Whether the phone is registered: the chip without presence.
class RegistrationChip extends StatelessWidget {
  const RegistrationChip({super.key, required this.registration});
  final Registration registration;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final color = switch (registration) {
      Registration.registered => AnvilColors.answer,
      Registration.registering => Colors.amber,
      Registration.unregistered => Colors.grey,
      Registration.failed => AnvilColors.hangup,
    };
    return Chip(
      key: const Key('registration'),
      avatar: CircleAvatar(backgroundColor: color, radius: 5),
      label: Text(registrationName(s, registration)),
    );
  }
}

class _NoteDialog extends StatefulWidget {
  const _NoteDialog({this.note});
  final String? note;

  @override
  State<_NoteDialog> createState() => _NoteDialogState();
}

class _NoteDialogState extends State<_NoteDialog> {
  late final _text = TextEditingController(text: widget.note);

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    return AlertDialog(
      title: Text(s.presenceNoteTitle),
      content: TextField(
        key: const Key('presenceNoteText'),
        controller: _text,
        autofocus: true,
        maxLength: 100,
        decoration: InputDecoration(hintText: s.presenceNoteHint),
        onSubmitted: (v) => Navigator.pop(context, v),
      ),
      actions: [
        if (widget.note != null)
          TextButton(
            key: const Key('presenceNoteClear'),
            onPressed: () => Navigator.pop(context, ''),
            child: Text(s.clearButton),
          ),
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: Text(s.cancelButton),
        ),
        FilledButton(
          key: const Key('presenceNoteSave'),
          onPressed: () => Navigator.pop(context, _text.text),
          child: Text(s.saveButton),
        ),
      ],
    );
  }
}

class _ClearAfterDialog extends StatelessWidget {
  const _ClearAfterDialog();

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    return SimpleDialog(
      title: Text(s.presenceClearAfterTitle),
      children: [
        for (final (id, label) in [
          ('30m', s.clearAfter30m),
          ('1h', s.clearAfter1h),
          ('2h', s.clearAfter2h),
          ('today', s.clearAfterToday),
          ('never', s.clearAfterNever),
        ])
          SimpleDialogOption(
            key: Key('clearAfter-$id'),
            onPressed: () => Navigator.pop(context, id),
            child: Text(label),
          ),
      ],
    );
  }
}

/// The user's own statuses: added, renamed, given a state, reordered and
/// deleted.
class StatusesDialog extends StatelessWidget {
  const StatusesDialog({super.key, required this.model});
  final PhoneModel model;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    return ListenableBuilder(
      listenable: model,
      builder: (context, _) {
        final statuses = model.snapshot.presence?.statuses ?? const [];
        return AlertDialog(
          title: Text(s.statusesTitle),
          content: SizedBox(
            width: 420,
            height: 360,
            child: statuses.isEmpty
                ? Center(child: Text(s.statusesNone))
                : ReorderableListView(
                    buildDefaultDragHandles: false,
                    onReorderItem: (from, to) {
                      final ids = [for (final o in statuses) o.id];
                      final moved = ids.removeAt(from);
                      ids.insert(to, moved);
                      model.run(() => model.api.orderStatuses(ids));
                    },
                    children: [
                      for (final (i, o) in statuses.indexed)
                        ListTile(
                          key: Key('ownStatus-${o.id}'),
                          leading: ReorderableDragStartListener(
                            index: i,
                            child: const Icon(Icons.drag_indicator),
                          ),
                          title: Text(o.label),
                          subtitle: Text(stateName(s, o.state)),
                          onTap: () => _edit(context, o),
                          trailing: IconButton(
                            key: Key('deleteStatus-${o.id}'),
                            tooltip: s.deleteButton,
                            icon: const Icon(Icons.delete_outline),
                            onPressed: () =>
                                model.run(() => model.api.removeStatus(o.id)),
                          ),
                        ),
                    ],
                  ),
          ),
          actions: [
            TextButton(
              key: const Key('addStatus'),
              onPressed: statuses.length >= 20
                  ? null
                  : () => _edit(context, null),
              child: Text(s.statusAdd),
            ),
            FilledButton(
              onPressed: () => Navigator.pop(context),
              child: Text(s.doneButton),
            ),
          ],
        );
      },
    );
  }

  Future<void> _edit(BuildContext context, OwnStatus? status) async {
    final edited = await showDialog<(String, String)>(
      context: context,
      builder: (_) => _StatusDialog(status: status),
    );
    if (edited == null) return;
    final (label, state) = edited;
    if (status == null) {
      await model.run(() => model.api.addStatus(label, state));
    } else {
      await model.run(
        () => model.api.editStatus(
          status.id,
          label: label == status.label ? null : label,
          state: state == status.state ? null : state,
        ),
      );
    }
  }
}

class _StatusDialog extends StatefulWidget {
  const _StatusDialog({this.status});
  final OwnStatus? status;

  @override
  State<_StatusDialog> createState() => _StatusDialogState();
}

class _StatusDialogState extends State<_StatusDialog> {
  late final _label = TextEditingController(text: widget.status?.label);
  late String _state = widget.status?.state ?? 'available';

  @override
  void dispose() {
    _label.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    return AlertDialog(
      title: Text(widget.status == null ? s.statusAdd : s.statusEdit),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          TextField(
            key: const Key('statusLabel'),
            controller: _label,
            autofocus: true,
            maxLength: 40,
            decoration: InputDecoration(labelText: s.statusLabel),
            onChanged: (_) => setState(() {}),
          ),
          DropdownButtonFormField<String>(
            key: const Key('statusState'),
            initialValue: _state,
            decoration: InputDecoration(labelText: s.statusState),
            items: [
              for (final b in builtInStatuses)
                DropdownMenuItem(
                  key: Key('statusState-$b'),
                  value: b,
                  child: Text(stateName(s, b)),
                ),
            ],
            onChanged: (v) => setState(() => _state = v ?? _state),
          ),
          if (_state == 'dnd')
            Padding(
              padding: const EdgeInsets.only(top: 8),
              child: Text(
                s.statusDndNote,
                style: Theme.of(context).textTheme.bodySmall,
              ),
            ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: Text(s.cancelButton),
        ),
        FilledButton(
          key: const Key('statusSave'),
          onPressed: _label.text.trim().isEmpty
              ? null
              : () => Navigator.pop(context, (_label.text.trim(), _state)),
          child: Text(s.saveButton),
        ),
      ],
    );
  }
}
