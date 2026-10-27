// The user's contacts (`docs/APP.md` §4, Contacts): kept on this device or,
// signed in to FCP, on their account; made from a call, a recent call or
// by hand, changed, moved to the account and deleted; and the "More" menu
// that offers "Create contact" and whatever comes after it.

import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';

/// One entry of a "More" menu: the menu is a list of these, so a feature
/// added later is one more entry.
class MoreAction {
  const MoreAction({
    required this.key,
    required this.icon,
    required this.label,
    required this.onSelected,
  });

  final Key key;
  final IconData icon;
  final String label;
  final VoidCallback onSelected;
}

/// The actions offered for someone at `remote` (a call's or a recent
/// call's address): "Create contact" while they are not one.
List<MoreAction> actionsFor(
  BuildContext context,
  PhoneModel model,
  String remote, {
  String? name,
}) {
  final s = Strings.of(context);
  return [
    if (model.snapshot.contactFor(remote) == null)
      MoreAction(
        key: const Key('createContact'),
        icon: Icons.person_add_alt_1,
        label: s.createContactButton,
        onSelected: () => editContact(
          context,
          model,
          number: shortAddress(remote),
          name: name,
        ),
      ),
  ];
}

/// Show a menu of `actions` below `anchor`'s widget.
Future<void> showMoreMenu(BuildContext anchor, List<MoreAction> actions) async {
  final box = anchor.findRenderObject() as RenderBox?;
  final overlay = Overlay.of(anchor).context.findRenderObject() as RenderBox?;
  if (box == null || overlay == null) return;
  final topLeft = box.localToGlobal(Offset.zero, ancestor: overlay);
  final chosen = await showMenu<MoreAction>(
    context: anchor,
    position: RelativeRect.fromRect(
      topLeft & box.size,
      Offset.zero & overlay.size,
    ),
    items: [
      for (final a in actions)
        PopupMenuItem(
          key: a.key,
          value: a,
          child: ListTile(
            contentPadding: EdgeInsets.zero,
            leading: Icon(a.icon),
            title: Text(a.label),
          ),
        ),
    ],
  );
  chosen?.onSelected();
}

/// Make a contact (from `number` and `name`, when known) or change
/// `contact`. A new one goes on the account when there is one, unless the
/// user picks this device.
Future<void> editContact(
  BuildContext context,
  PhoneModel model, {
  Contact? contact,
  String? number,
  String? name,
}) async {
  final made = await showDialog<Contact>(
    context: context,
    builder: (_) => ContactDialog(
      contact:
          contact ??
          Contact(
            onAccount: model.snapshot.hasAccount,
            name: name ?? '',
            numbers: [if (number != null) ContactNumber(number: number)],
          ),
      canUseAccount: model.snapshot.hasAccount,
    ),
  );
  if (made != null) await model.run(() => model.api.saveContact(made));
}

/// Move a device's `contact` to the account.
Future<void> moveContactToAccount(PhoneModel model, Contact contact) =>
    model.run(() => model.api.saveContact(contact.toAccount()));

/// Delete `contact` once the user says so.
Future<void> deleteContact(
  BuildContext context,
  PhoneModel model,
  Contact contact,
) async {
  final s = Strings.of(context);
  final sure = await showDialog<bool>(
    context: context,
    builder: (context) => AlertDialog(
      content: Text(
        contact.onAccount
            ? s.deleteAccountContactConfirm(contact.name)
            : s.deleteContactConfirm(contact.name),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context, false),
          child: Text(s.cancelButton),
        ),
        TextButton(
          key: const Key('confirmDeleteContact'),
          onPressed: () => Navigator.pop(context, true),
          child: Text(s.deleteButton),
        ),
      ],
    ),
  );
  if (sure == true) await model.run(() => model.api.deleteContact(contact.id));
}

/// A contact's name, company, numbers, email and notes; and, for a new one
/// when there is an account, where it is kept.
class ContactDialog extends StatefulWidget {
  const ContactDialog({
    super.key,
    required this.contact,
    this.canUseAccount = false,
  });
  final Contact contact;
  final bool canUseAccount;

  @override
  State<ContactDialog> createState() => _ContactDialogState();
}

class _ContactDialogState extends State<ContactDialog> {
  final _form = GlobalKey<FormState>();
  late final _name = TextEditingController(text: widget.contact.name);
  late final _company = TextEditingController(text: widget.contact.company);
  late final _email = TextEditingController(text: widget.contact.email);
  late final _notes = TextEditingController(text: widget.contact.notes);
  late bool _onAccount = widget.contact.onAccount;
  late final List<(TextEditingController, TextEditingController)> _numbers = [
    for (final n in widget.contact.numbers)
      (
        TextEditingController(text: n.label),
        TextEditingController(text: n.number),
      ),
    if (widget.contact.numbers.isEmpty)
      (TextEditingController(), TextEditingController()),
  ];

  @override
  void dispose() {
    for (final c in [_name, _company, _email, _notes]) {
      c.dispose();
    }
    for (final (label, number) in _numbers) {
      label.dispose();
      number.dispose();
    }
    super.dispose();
  }

  String? _text(TextEditingController c) {
    final t = c.text.trim();
    return t.isEmpty ? null : t;
  }

  void _save() {
    if (!_form.currentState!.validate()) return;
    Navigator.pop(
      context,
      Contact(
        id: widget.contact.id,
        onAccount: _onAccount,
        name: _name.text.trim(),
        company: _text(_company),
        numbers: [
          for (final (label, number) in _numbers)
            if (number.text.trim().isNotEmpty)
              ContactNumber(
                label: label.text.trim(),
                number: number.text.trim(),
              ),
        ],
        email: _text(_email),
        notes: _text(_notes),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    return AlertDialog(
      title: Text(
        widget.contact.id.isEmpty ? s.contactNewTitle : s.contactEditTitle,
      ),
      content: SizedBox(
        width: 400,
        child: Form(
          key: _form,
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (widget.canUseAccount && widget.contact.id.isEmpty)
                  Padding(
                    padding: const EdgeInsets.only(bottom: 8),
                    child: SegmentedButton<bool>(
                      key: const Key('contactPlace'),
                      segments: [
                        ButtonSegment(
                          value: true,
                          icon: const Icon(Icons.cloud_outlined),
                          label: Text(
                            s.contactPlaceAccount,
                            key: const Key('contactPlace-account'),
                          ),
                        ),
                        ButtonSegment(
                          value: false,
                          icon: const Icon(Icons.smartphone),
                          label: Text(
                            s.contactPlaceDevice,
                            key: const Key('contactPlace-device'),
                          ),
                        ),
                      ],
                      selected: {_onAccount},
                      onSelectionChanged: (v) =>
                          setState(() => _onAccount = v.first),
                    ),
                  ),
                TextFormField(
                  key: const Key('contactName'),
                  controller: _name,
                  autofocus: widget.contact.name.isEmpty,
                  decoration: InputDecoration(labelText: s.contactNameField),
                  validator: (v) =>
                      (v ?? '').trim().isEmpty ? s.contactNeedsName : null,
                ),
                TextFormField(
                  key: const Key('contactCompany'),
                  controller: _company,
                  decoration: InputDecoration(labelText: s.contactCompanyField),
                ),
                for (final (i, (label, number)) in _numbers.indexed)
                  Row(
                    children: [
                      Expanded(
                        flex: 3,
                        child: TextFormField(
                          key: Key('contactNumber-$i'),
                          controller: number,
                          keyboardType: TextInputType.phone,
                          decoration: InputDecoration(
                            labelText: s.contactNumberField,
                          ),
                        ),
                      ),
                      const SizedBox(width: 8),
                      Expanded(
                        flex: 2,
                        child: TextFormField(
                          key: Key('contactLabel-$i'),
                          controller: label,
                          decoration: InputDecoration(
                            labelText: s.contactLabelField,
                          ),
                        ),
                      ),
                      if (_numbers.length > 1)
                        IconButton(
                          key: Key('removeNumber-$i'),
                          tooltip: s.removeNumberButton,
                          icon: const Icon(Icons.remove_circle_outline),
                          onPressed: () => setState(() => _numbers.removeAt(i)),
                        ),
                    ],
                  ),
                Align(
                  alignment: Alignment.centerLeft,
                  child: TextButton.icon(
                    key: const Key('addNumber'),
                    icon: const Icon(Icons.add),
                    label: Text(s.addNumberButton),
                    onPressed: () => setState(
                      () => _numbers.add((
                        TextEditingController(),
                        TextEditingController(),
                      )),
                    ),
                  ),
                ),
                TextFormField(
                  key: const Key('contactEmail'),
                  controller: _email,
                  keyboardType: TextInputType.emailAddress,
                  decoration: InputDecoration(labelText: s.contactEmailField),
                ),
                TextFormField(
                  key: const Key('contactNotes'),
                  controller: _notes,
                  maxLines: 3,
                  minLines: 1,
                  decoration: InputDecoration(labelText: s.contactNotesField),
                ),
              ],
            ),
          ),
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.pop(context),
          child: Text(s.cancelButton),
        ),
        FilledButton(
          key: const Key('saveContact'),
          onPressed: _save,
          child: Text(s.saveButton),
        ),
      ],
    );
  }
}

/// A contact in a list: tapped, its first number is dialled; its menu
/// edits, moves to the account or deletes it. With an account, each is
/// marked with where it is kept.
class ContactTile extends StatelessWidget {
  const ContactTile({
    super.key,
    required this.model,
    required this.contact,
    required this.onDial,
  });

  final PhoneModel model;
  final Contact contact;
  final ValueChanged<String> onDial;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final initials = contact.name
        .split(RegExp(r'\s+'))
        .where((w) => w.isNotEmpty)
        .take(2)
        .map((w) => w.characters.first.toUpperCase())
        .join();
    final numbers = contact.numbers
        .map((n) => n.label.isEmpty ? n.number : '${n.number} (${n.label})')
        .join(' · ');
    final dial = contact.dial;
    final hasAccount = model.snapshot.hasAccount;
    final place = contact.onAccount ? s.contactOnAccount : s.contactOnDevice;
    return ListTile(
      key: Key('contact-${contact.id}'),
      leading: CircleAvatar(radius: 20, child: Text(initials)),
      title: Row(
        children: [
          Flexible(child: Text(contact.name, overflow: TextOverflow.ellipsis)),
          if (hasAccount)
            Padding(
              padding: const EdgeInsets.only(left: 6),
              child: Tooltip(
                message: place,
                child: Icon(
                  contact.onAccount ? Icons.cloud_outlined : Icons.smartphone,
                  key: Key('contactPlace-${contact.id}'),
                  size: 16,
                  semanticLabel: place,
                ),
              ),
            ),
        ],
      ),
      subtitle: Text(
        [?contact.company, if (numbers.isNotEmpty) numbers].join(' — '),
      ),
      trailing: Wrap(
        spacing: 4,
        children: [
          if (dial != null)
            IconButton(
              key: Key('dialContact-${contact.id}'),
              tooltip: s.callPerson(contact.name),
              icon: const Icon(Icons.call),
              onPressed: () => onDial(dial),
            ),
          PopupMenuButton<void>(
            key: Key('contactMenu-${contact.id}'),
            tooltip: s.moreButton,
            itemBuilder: (_) => [
              PopupMenuItem(
                key: Key('editContact-${contact.id}'),
                onTap: () => editContact(context, model, contact: contact),
                child: Text(s.editContactButton),
              ),
              if (hasAccount && !contact.onAccount)
                PopupMenuItem(
                  key: Key('moveContact-${contact.id}'),
                  onTap: () => moveContactToAccount(model, contact),
                  child: Text(s.moveContactToAccountButton),
                ),
              PopupMenuItem(
                key: Key('deleteContact-${contact.id}'),
                onTap: () => deleteContact(context, model, contact),
                child: Text(s.deleteButton),
              ),
            ],
          ),
        ],
      ),
      onTap: dial == null ? null : () => onDial(dial),
    );
  }
}
