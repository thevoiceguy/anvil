// The user's latest calls: missed ones marked, each a tap from calling
// back (`docs/APP.md` §4, Recents).

import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../design/format.dart';
import '../design/theme.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';
import 'contacts.dart';

class RecentsPage extends StatelessWidget {
  const RecentsPage({super.key, required this.model, required this.onDial});
  final PhoneModel model;
  final ValueChanged<String> onDial;

  Future<void> _clear(BuildContext context) async {
    final s = Strings.of(context);
    final yes = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: Text(s.clearRecentsButton),
        content: Text(s.clearRecentsConfirm),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: Text(s.cancelButton),
          ),
          FilledButton(
            key: const Key('confirmClearRecents'),
            onPressed: () => Navigator.of(context).pop(true),
            child: Text(s.clearButton),
          ),
        ],
      ),
    );
    if (yes == true) await model.run(model.api.clearRecents);
  }

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
              itemCount: recents.length + 1,
              separatorBuilder: (_, i) =>
                  i == 0 ? const SizedBox.shrink() : const Divider(height: 1),
              itemBuilder: (context, i) {
                if (i == 0) {
                  return Align(
                    alignment: Alignment.centerRight,
                    child: Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 8),
                      child: TextButton.icon(
                        key: const Key('clearRecents'),
                        icon: const Icon(Icons.delete_sweep_outlined),
                        label: Text(s.clearRecentsButton),
                        onPressed: () => _clear(context),
                      ),
                    ),
                  );
                }
                final r = recents[i - 1];
                final contact = snap.contactFor(r.remote);
                final name =
                    snap.personFor(r.remote)?.name ?? contact?.name ?? r.who;
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
                  if (r.mosHundredths case final mos?)
                    (r.poor ? s.recentPoorAudio : s.recentAudio)(
                      (mos / 100).toStringAsFixed(1),
                    ),
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
                  trailing: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      IconButton(
                        key: Key('callBack-${r.id}'),
                        tooltip: s.callBackButton,
                        icon: const Icon(Icons.call),
                        onPressed: () => onDial(r.remote),
                      ),
                      PopupMenuButton<void>(
                        key: Key('recentMenu-${r.id}'),
                        tooltip: s.moreButton,
                        itemBuilder: (_) => [
                          if (contact == null)
                            PopupMenuItem(
                              key: Key('createContact-${r.id}'),
                              onTap: () => editContact(
                                context,
                                model,
                                number: shortAddress(r.remote),
                                name: r.displayName,
                              ),
                              child: Text(s.createContactButton),
                            ),
                          PopupMenuItem(
                            key: Key('removeRecent-${r.id}'),
                            onTap: () =>
                                model.run(() => model.api.hideRecent(r.id)),
                            child: Text(s.removeRecentButton),
                          ),
                        ],
                      ),
                    ],
                  ),
                  onTap: () => onDial(r.remote),
                );
              },
            ),
    );
  }
}
