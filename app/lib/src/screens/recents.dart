// The user's latest calls: missed ones marked, each a tap from calling
// back (`docs/APP.md` §4, Recents).

import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../design/format.dart';
import '../design/theme.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';

class RecentsPage extends StatelessWidget {
  const RecentsPage({super.key, required this.model, required this.onDial});
  final PhoneModel model;
  final ValueChanged<String> onDial;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final snap = model.snapshot;
    final recents = snap.recents;
    return RefreshIndicator(
      onRefresh: () => model.run(model.api.refresh),
      child: recents.isEmpty
          ? ListView(
              children: [
                Padding(
                  padding: const EdgeInsets.all(32),
                  child: Center(child: Text(s.recentsEmpty)),
                ),
              ],
            )
          : ListView.separated(
              itemCount: recents.length,
              separatorBuilder: (_, _) => const Divider(height: 1),
              itemBuilder: (context, i) {
                final r = recents[i];
                final name = snap.personFor(r.remote)?.name ?? r.who;
                final (icon, color) = r.missed
                    ? (Icons.call_missed, AnvilColors.hangup)
                    : r.direction == Direction.incoming
                    ? (Icons.call_received, null)
                    : (Icons.call_made, null);
                final details = [
                  if (r.missed) s.recentMissed,
                  when(context, r.startedAt),
                  if (r.durationSecs != null && r.durationSecs! > 0)
                    duration(r.durationSecs!),
                ].where((d) => d.isNotEmpty).join(' · ');
                return ListTile(
                  key: Key('recent-${r.id}'),
                  leading: Icon(icon, color: color),
                  title: Text(
                    name,
                    style: r.missed
                        ? TextStyle(color: AnvilColors.hangup)
                        : null,
                  ),
                  subtitle: Text(details),
                  trailing: IconButton(
                    key: Key('callBack-${r.id}'),
                    tooltip: s.callBackButton,
                    icon: const Icon(Icons.call),
                    onPressed: () => onDial(r.remote),
                  ),
                  onTap: () => onDial(r.remote),
                );
              },
            ),
    );
  }
}
