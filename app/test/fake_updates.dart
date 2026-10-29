import 'package:anvil/src/update/updates.dart';

/// An updater for the tests: what it finds is set by the test, and what it
/// was asked to do is kept.
class FakeUpdates implements UpdateApi {
  UpdateInfo? next;
  Object? failWith;
  final List<String> log = [];

  /// The version the last check found when [download] was called: what the
  /// updater fetches is what it last found.
  String? downloaded;

  @override
  String version() => '0.1.0';

  @override
  Future<UpdateInfo?> check() async {
    log.add('check');
    if (failWith != null) throw failWith!;
    return next;
  }

  @override
  Future<void> download() async {
    log.add('download');
    downloaded = next?.version;
  }

  @override
  Future<void> install() async => log.add('install');
}
