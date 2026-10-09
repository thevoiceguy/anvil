// A newer release, said above every page: update and restart, or (a copy
// that cannot update itself) the file to install by hand.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:url_launcher/url_launcher.dart';

import '../../l10n/app_localizations.dart';
import '../phone/phone_model.dart';
import 'updates.dart';

/// Update now, unless a call is up: then after it.
Future<void> startUpdate(
  PhoneModel phone,
  UpdateModel updates,
  Future<void> Function() quit,
) async {
  if (phone.snapshot.calls.isNotEmpty) return;
  await updates.update(() async {
    await phone.stop();
    await quit();
  });
}

class UpdateBanner extends StatelessWidget {
  const UpdateBanner({
    super.key,
    required this.updates,
    required this.phone,
    required this.quit,
  });

  final UpdateModel updates;
  final PhoneModel phone;
  final Future<void> Function() quit;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: updates,
      builder: (context, _) {
        final info = updates.available;
        if (info == null || updates.dismissed) return const SizedBox.shrink();
        final s = Strings.of(context);
        final downloading = updates.status == UpdateStatus.downloading;
        final inCall = phone.snapshot.calls.isNotEmpty;
        return Material(
          key: const Key('updateBanner'),
          color: Theme.of(context).colorScheme.secondaryContainer,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(16, 6, 8, 6),
            child: Row(
              children: [
                const Icon(Icons.system_update, size: 20),
                const SizedBox(width: 12),
                Expanded(
                  child: Text(
                    downloading
                        ? s.updateDownloading
                        : updates.status == UpdateStatus.failed
                        ? s.updateFailed(updates.error ?? '')
                        : s.updateAvailable(info.version),
                  ),
                ),
                if (!downloading) ...[
                  TextButton(
                    key: const Key('updateLater'),
                    onPressed: updates.dismiss,
                    child: Text(s.updateLater),
                  ),
                  if (info.canInstall)
                    FilledButton(
                      key: const Key('updateNow'),
                      onPressed: inCall
                          ? null
                          : () => startUpdate(phone, updates, quit),
                      child: Text(inCall ? s.updateAfterCall : s.updateNow),
                    )
                  else if (info.download != null)
                    FilledButton(
                      key: const Key('updateDownload'),
                      onPressed: () =>
                          unawaited(launchUrl(Uri.parse(info.download!))),
                      child: Text(s.updateGetIt),
                    ),
                ],
              ],
            ),
          ),
        );
      },
    );
  }
}

/// The updater, for the widgets that show it (the banner, settings).
class UpdateScope extends InheritedWidget {
  const UpdateScope({
    super.key,
    required this.updates,
    required this.quit,
    required super.child,
  });

  final UpdateModel updates;

  /// End the app (after an update has been started).
  final Future<void> Function() quit;

  static UpdateScope? maybeOf(BuildContext context) =>
      context.dependOnInheritedWidgetOfExactType<UpdateScope>();

  @override
  bool updateShouldNotify(UpdateScope old) =>
      updates != old.updates || quit != old.quit;
}
