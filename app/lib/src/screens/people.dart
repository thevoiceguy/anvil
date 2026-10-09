// The tenant's directory: searchable, favourites first, each person's
// presence and busy lamp (`docs/APP.md` §4, People).

import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../design/format.dart';
import '../design/theme.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';

class PeoplePage extends StatefulWidget {
  const PeoplePage({super.key, required this.model, required this.onDial});
  final PhoneModel model;
  final ValueChanged<String> onDial;

  @override
  State<PeoplePage> createState() => _PeoplePageState();
}

class _PeoplePageState extends State<PeoplePage> {
  String _query = '';

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final people = widget.model.snapshot.people
        .where((p) => p.matches(_query))
        .toList();
    final favourites = people.where((p) => p.favourite).toList();
    final rest = people.where((p) => !p.favourite).toList();
    final theme = Theme.of(context);
    Widget header(String text) => Padding(
      padding: const EdgeInsets.fromLTRB(16, 16, 16, 4),
      child: Text(text, style: theme.textTheme.titleSmall),
    );
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.all(12),
          child: TextField(
            key: const Key('peopleSearch'),
            decoration: InputDecoration(
              prefixIcon: const Icon(Icons.search),
              hintText: s.peopleSearch,
            ),
            onChanged: (q) => setState(() => _query = q),
          ),
        ),
        Expanded(
          child: people.isEmpty
              ? Center(child: Text(s.peopleEmpty))
              : ListView(
                  children: [
                    if (favourites.isNotEmpty) ...[
                      header(s.favourites),
                      for (final p in favourites) _tile(context, p),
                      header(s.everyone),
                    ],
                    for (final p in rest) _tile(context, p),
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
