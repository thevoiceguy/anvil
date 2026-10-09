// The app updating itself (`docs/APP.md` §7, U3b), as the screens see it:
// an interface over `anvil-update`, and the model the banner and settings
// watch.

import 'dart:async';

import 'package:flutter/foundation.dart';

/// A newer release.
class UpdateInfo {
  const UpdateInfo({
    required this.version,
    required this.notes,
    required this.canInstall,
    this.reason,
    this.download,
  });

  final String version;

  /// The release's page.
  final String notes;

  /// This copy installs it itself; else [reason] says why and [download]
  /// is the file to install by hand.
  final bool canInstall;
  final String? reason;
  final String? download;
}

abstract class UpdateApi {
  String version();
  Future<UpdateInfo?> check();
  Future<void> download();

  /// Start installing what was downloaded once the app has quit; the app
  /// quits right after.
  Future<void> install();
}

enum UpdateStatus { idle, checking, upToDate, available, downloading, failed }

class UpdateModel extends ChangeNotifier {
  UpdateModel(this.api);

  final UpdateApi api;
  UpdateStatus status = UpdateStatus.idle;
  UpdateInfo? available;
  String? error;

  /// The banner was put away for this release.
  bool dismissed = false;
  Timer? _timer;

  /// Check soon after start, then twice a day.
  void schedule({
    Duration first = const Duration(seconds: 30),
    Duration every = const Duration(hours: 12),
  }) {
    _timer?.cancel();
    _timer = Timer(first, () {
      unawaited(check());
      _timer = Timer.periodic(every, (_) => unawaited(check()));
    });
  }

  Future<void> check() async {
    if (status == UpdateStatus.checking || status == UpdateStatus.downloading) {
      return;
    }
    status = UpdateStatus.checking;
    notifyListeners();
    try {
      final found = await api.check();
      if (found?.version != available?.version) dismissed = false;
      available = found;
      status = found == null ? UpdateStatus.upToDate : UpdateStatus.available;
      error = null;
    } catch (e) {
      status = UpdateStatus.failed;
      error = '$e';
    }
    notifyListeners();
  }

  /// Fetch the update and start installing it; `quit` then ends the app.
  Future<void> update(Future<void> Function() quit) async {
    status = UpdateStatus.downloading;
    notifyListeners();
    try {
      await api.download();
      await api.install();
    } catch (e) {
      status = UpdateStatus.failed;
      error = '$e';
      notifyListeners();
      return;
    }
    await quit();
  }

  void dismiss() {
    dismissed = true;
    notifyListeners();
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }
}
