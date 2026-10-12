// What a start of the app asks for (Windows hands a second start's arguments
// to the running app): a link to call — `tel:`, `sip:`, `sips:`, `callto:` —
// and `--hidden` from start at login.

import 'shell.dart';

/// Start at login's argument: the app starts in the tray.
const hiddenArgument = '--hidden';

/// What `link` asks to call, for the keypad, or null when it is not a link
/// to call. A `tel:` number keeps its digits, `+`, `*` and `#` (RFC 3966's
/// separators and parameters dropped); a SIP address is kept whole but for
/// its headers; `callto:` is Skype's (`callto://` too).
String? dialTarget(String link) {
  final colon = link.indexOf(':');
  if (colon <= 0) return null;
  final scheme = link.substring(0, colon).toLowerCase();
  var rest = link.substring(colon + 1);
  try {
    rest = Uri.decodeComponent(rest);
  } on ArgumentError {
    return null;
  }
  switch (scheme) {
    case 'tel':
      final number = rest.split(';').first.replaceAll(RegExp(r'[^0-9+*#]'), '');
      return number.isEmpty ? null : number;
    case 'callto':
      final target = rest.replaceFirst(RegExp(r'^//'), '').split('?').first;
      return target.trim().isEmpty ? null : target.trim();
    case 'sip':
    case 'sips':
      final address = rest.split('?').first.trim();
      return address.isEmpty ? null : '$scheme:$address';
    default:
      return null;
  }
}

/// What a start's `args` ask of the app. The first start shows its window
/// itself (unless hidden), so only its links count; a later one, handed
/// on, also brings the window forward, unless it is start at login.
List<ShellEvent> startEvents(List<String> args, {required bool handedOn}) {
  final dials = [
    for (final arg in args)
      if (dialTarget(arg) case final target?)
        ShellEvent(ShellAction.dial, target: target),
  ];
  final show = handedOn && dials.isEmpty && !args.contains(hiddenArgument);
  return [if (show) const ShellEvent(ShellAction.show), ...dials];
}
