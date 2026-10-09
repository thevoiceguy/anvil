// The real updater: `anvil-update` through flutter_rust_bridge.

import '../rust/api/update.dart' as rust;
import 'updates.dart';

class RustUpdates implements UpdateApi {
  @override
  String version() => rust.appVersion();

  @override
  Future<UpdateInfo?> check() async {
    final u = await rust.checkForUpdate();
    if (u == null) return null;
    return UpdateInfo(
      version: u.version,
      notes: u.notes,
      canInstall: u.canInstall,
      reason: u.reason,
      download: u.download,
    );
  }

  @override
  Future<void> download() => rust.downloadUpdate();

  @override
  Future<void> install() => rust.installUpdate();
}
