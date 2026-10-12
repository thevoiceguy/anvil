// Windows keeps the app's files in the local profile; what an older
// version kept in the roaming one is moved there.

import 'dart:io';

import 'package:anvil/src/phone/rust_phone.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  late Directory root;
  late Map<String, String> env;
  final sep = Platform.pathSeparator;

  setUp(() {
    root = Directory.systemTemp.createTempSync('anvil-profile');
    env = {
      'APPDATA': '${root.path}${sep}Roaming',
      'LOCALAPPDATA': '${root.path}${sep}Local',
    };
  });
  tearDown(() => root.deleteSync(recursive: true));

  String roaming() => '${env['APPDATA']}${sep}Anvil${sep}Anvil';
  String local() => '${env['LOCALAPPDATA']}${sep}Anvil${sep}Anvil';

  test('a session in the roaming profile moves to the local one', () async {
    Directory(roaming()).createSync(recursive: true);
    File('${roaming()}${sep}session.json').writeAsStringSync('{"s":1}');
    File('${roaming()}${sep}settings.json').writeAsStringSync('{"a":1}');

    final dir = await RustPhone.localOnWindows(roaming(), environment: env);
    expect(dir, local());
    expect(File('$dir${sep}session.json').readAsStringSync(), '{"s":1}');
    expect(File('$dir${sep}settings.json').readAsStringSync(), '{"a":1}');
    expect(File('${roaming()}${sep}session.json').existsSync(), isFalse);
  });

  test('a session already local is kept, the roaming one left', () async {
    Directory(roaming()).createSync(recursive: true);
    Directory(local()).createSync(recursive: true);
    File('${roaming()}${sep}session.json').writeAsStringSync('old');
    File('${local()}${sep}session.json').writeAsStringSync('new');

    final dir = await RustPhone.localOnWindows(roaming(), environment: env);
    expect(File('$dir${sep}session.json').readAsStringSync(), 'new');
  });

  test('a first start makes the local folder', () async {
    final dir = await RustPhone.localOnWindows(roaming(), environment: env);
    expect(Directory(dir).existsSync(), isTrue);
    expect(dir, local());
  });

  test(
    'a support folder outside the roaming profile is used as it is',
    () async {
      final elsewhere = '${root.path}${sep}elsewhere';
      expect(
        await RustPhone.localOnWindows(elsewhere, environment: env),
        elsewhere,
      );
    },
  );
}
