// What a start of the app asks for: links to call and start at login.

import 'package:anvil/src/desktop/links.dart';
import 'package:anvil/src/desktop/shell.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('a tel: link is its number', () {
    expect(dialTarget('tel:+1-512-555-0100'), '+15125550100');
    expect(dialTarget('TEL:(512) 555.0100'), '5125550100');
    expect(dialTarget('tel:1002;phone-context=example.com'), '1002');
    expect(dialTarget('tel:%2B44%2020%207946%200000'), '+442079460000');
    expect(dialTarget('tel:*72#'), '*72#');
    expect(dialTarget('tel:;ext=1'), isNull);
  });

  test('a SIP address is kept but for its headers', () {
    expect(dialTarget('sip:bob@example.com'), 'sip:bob@example.com');
    expect(
      dialTarget('sips:bob@example.com;transport=tls?subject=hi'),
      'sips:bob@example.com;transport=tls',
    );
    expect(dialTarget('sip:'), isNull);
  });

  test("callto: is Skype's", () {
    expect(dialTarget('callto:+15125550100'), '+15125550100');
    expect(dialTarget('callto://1002'), '1002');
  });

  test('anything else is not a link to call', () {
    expect(dialTarget('--hidden'), isNull);
    expect(dialTarget('https://example.com'), isNull);
    expect(dialTarget('C:\\Users\\ann'), isNull);
    expect(dialTarget('tel:%zz'), isNull);
  });

  test('a start asks for its links; one handed on also for the window', () {
    const dial = ShellEvent(ShellAction.dial, target: '1002');
    const show = ShellEvent(ShellAction.show);
    expect(startEvents(['tel:1002'], handedOn: false), [dial]);
    expect(startEvents([], handedOn: false), isEmpty);
    expect(startEvents([], handedOn: true), [show]);
    expect(startEvents(['tel:1002'], handedOn: true), [dial]);
    expect(
      startEvents(['--hidden'], handedOn: true),
      isEmpty,
      reason: 'start at login while running changes nothing',
    );
  });
}
