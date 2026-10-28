// The user's contacts and the tenant's directory, as two tabs under one
// search: the directory with favourites first, each person's presence and
// busy lamp (`docs/APP.md` §4, People).

import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../design/format.dart';
import '../design/theme.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';
import 'contacts.dart';

class PeoplePage extends StatefulWidget {
  const PeoplePage({super.key, required this.model, required this.onDial});
  final PhoneModel model;
  final ValueChanged<String> onDial;

  @override
  State<PeoplePage> createState() => _PeoplePageState();
}

class _PeoplePageState extends State<PeoplePage>
    with SingleTickerProviderStateMixin {
  String _query = '';

  // My contacts first when there are any, else the directory; the tab
  // chosen is kept while the app runs (the page's storage).
  static const _tab = 'peopleTab';
  late final TabController _tabs = TabController(
    length: 2,
    vsync: this,
    initialIndex:
        PageStorage.maybeOf(context)?.readState(context, identifier: _tab)
            as int? ??
        (widget.model.snapshot.contacts.isEmpty ? 1 : 0),
  )..addListener(() => _keepTab(_tabs.index));

  void _keepTab(int index) =>
      PageStorage.maybeOf(context)
          ?.writeState(context, index, identifier: _tab);

  @override
  void dispose() {
    _tabs.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final people = widget.model.snapshot.people
        .where((p) => p.matches(_query))
        .toList();
    final query = _query.trim().toLowerCase();
    final contacts = widget.model.snapshot.contacts
        .where(
          (c) =>
              query.isEmpty ||
              c.name.toLowerCase().contains(query) ||
              (c.company ?? '').toLowerCase().contains(query) ||
              c.numbers.any((n) => n.number.contains(query)),
        )
        .toList();
    final favourites = people.where((p) => p.favourite).toList();
    final rest = people.where((p) => !p.favourite).toList();
    final theme = Theme.of(context);
    Widget header(String text) => Padding(
      padding: const EdgeInsets.fromLTRB(16, 16, 16, 4),
      child: Text(text, style: theme.textTheme.titleSmall),
    );
    Widget empty(String text) => Center(
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Text(text, textAlign: TextAlign.center),
      ),
    );
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(12, 12, 12, 0),
          child: Row(
            children: [
              Expanded(
                child: TextField(
                  key: const Key('peopleSearch'),
                  decoration: InputDecoration(
                    prefixIcon: const Icon(Icons.search),
                    hintText: s.peopleSearch,
                  ),
                  onChanged: (q) => setState(() => _query = q),
                ),
              ),
              IconButton(
                key: const Key('newContact'),
                tooltip: s.contactNewTitle,
                icon: const Icon(Icons.person_add_alt_1),
                // Made: shown where it went.
                onPressed: () async {
                  await editContact(context, widget.model);
                  if (mounted) _tabs.animateTo(0);
                },
              ),
            ],
          ),
        ),
        TabBar(
          controller: _tabs,
          tabs: [
            Tab(key: const Key('tab-contacts'), text: s.contactsHeading),
            Tab(key: const Key('tab-directory'), text: s.directoryTab),
          ],
        ),
        Expanded(
          child: TabBarView(
            controller: _tabs,
            children: [
              contacts.isEmpty
                  ? empty(query.isEmpty ? s.contactsEmpty : s.peopleEmpty)
                  : ListView(
                      key: const Key('contactsList'),
                      children: [
                        for (final c in contacts)
                          ContactTile(
                            model: widget.model,
                            contact: c,
                            onDial: widget.onDial,
                          ),
                      ],
                    ),
              people.isEmpty
                  ? empty(s.directoryEmpty)
                  : ListView(
                      key: const Key('directoryList'),
                      children: [
                        if (favourites.isNotEmpty) ...[
                          header(s.favourites),
                          for (final p in favourites) _tile(context, p),
                          if (rest.isNotEmpty) header(s.everyone),
                        ],
                        for (final p in rest) _tile(context, p),
                      ],
                    ),
            ],
          ),
        ),
      ],
    );
  }

  Widget _tile(BuildContext context, Person p) {
    final s = Strings.of(context);
    final model = widget.model;
    final presence = presenceLabel(s, p);
    final details = [?p.extension, ?p.jobTitle, ?p.department].join(' · ');
    return ListTile(
      key: Key('person-${p.key}'),
      leading: PresenceAvatar(person: p),
      title: Text(p.name),
      subtitle: Text(
        [
          if (presence.isNotEmpty) presence,
          if (details.isNotEmpty) details,
        ].join(' — '),
      ),
      trailing: Wrap(
        spacing: 4,
        children: [
          IconButton(
            key: Key('favourite-${p.key}'),
            tooltip: p.favourite ? s.favouriteRemove : s.favouriteAdd,
            icon: Icon(p.favourite ? Icons.star : Icons.star_border),
            color: p.favourite ? AnvilColors.held : null,
            onPressed: () =>
                model.run(() => model.api.favourite(p.key, !p.favourite)),
          ),
          IconButton(
            key: Key('dial-${p.key}'),
            tooltip: s.callPerson(p.name),
            icon: const Icon(Icons.call),
            onPressed: () => widget.onDial(p.dial),
          ),
        ],
      ),
      onTap: () => widget.onDial(p.dial),
    );
  }
}

/// Initials with a dot for presence; red while they are on a call.
class PresenceAvatar extends StatelessWidget {
  const PresenceAvatar({super.key, required this.person});
  final Person person;

  @override
  Widget build(BuildContext context) {
    final initials = person.name
        .split(RegExp(r'\s+'))
        .where((w) => w.isNotEmpty)
        .take(2)
        .map((w) => w.characters.first.toUpperCase())
        .join();
    final shown = person.presence != null || person.onCall;
    return SizedBox(
      width: 40,
      height: 40,
      child: Stack(
        children: [
          CircleAvatar(radius: 20, child: Text(initials)),
          if (shown)
            Positioned(
              right: 0,
              bottom: 0,
              child: Container(
                key: Key(
                  person.onCall ? 'lamp-${person.key}' : 'dot-${person.key}',
                ),
                width: 12,
                height: 12,
                decoration: BoxDecoration(
                  color: AnvilColors.presence(
                    person.presence,
                    onCall: person.onCall,
                  ),
                  shape: BoxShape.circle,
                  border: Border.all(
                    color: Theme.of(context).colorScheme.surface,
                    width: 2,
                  ),
                ),
              ),
            ),
        ],
      ),
    );
  }
}
