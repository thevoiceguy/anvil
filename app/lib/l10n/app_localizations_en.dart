// ignore: unused_import
import 'package:intl/intl.dart' as intl;

import 'app_localizations.dart';

// ignore_for_file: type=lint

/// The translations for English (`en`).
class StringsEn extends Strings {
  StringsEn([String locale = 'en']) : super(locale);

  @override
  String get appName => 'Anvil';

  @override
  String get signInTitle => 'Sign in';

  @override
  String get signInServer => 'Server or email address';

  @override
  String get signInServerHint => 'pbx.example.com or you@example.com';

  @override
  String get signInUsername => 'Username';

  @override
  String get signInPassword => 'Password';

  @override
  String get signInCode => 'One-time code';

  @override
  String get signInButton => 'Sign in';

  @override
  String signInFailed(String reason) {
    return 'Could not sign in: $reason';
  }

  @override
  String get registrationRegistered => 'Ready';

  @override
  String get registrationRegistering => 'Connecting…';

  @override
  String get registrationUnregistered => 'Offline';

  @override
  String get registrationFailed => 'Could not connect';

  @override
  String get keypadHint => 'Number, extension or name';

  @override
  String get callButton => 'Call';

  @override
  String get answerButton => 'Answer';

  @override
  String get declineButton => 'Decline';

  @override
  String get hangupButton => 'End';

  @override
  String get muteButton => 'Mute';

  @override
  String get unmuteButton => 'Unmute';

  @override
  String get holdButton => 'Hold';

  @override
  String get resumeButton => 'Resume';

  @override
  String get keypadButton => 'Keypad';

  @override
  String get incomingCall => 'Incoming call';

  @override
  String get callDialing => 'Calling…';

  @override
  String get callRinging => 'Ringing…';

  @override
  String get callConnected => 'Connected';

  @override
  String get callHeld => 'On hold';

  @override
  String get noCalls => 'No calls';

  @override
  String voicemailCount(int count) {
    String _temp0 = intl.Intl.pluralLogic(
      count,
      locale: localeName,
      other: '$count new voicemails',
      one: '1 new voicemail',
      zero: 'No new voicemail',
    );
    return '$_temp0';
  }

  @override
  String get dndOn => 'Do not disturb';

  @override
  String get signOut => 'Sign out';

  @override
  String callEnded(String reason) {
    return 'Call ended: $reason';
  }

  @override
  String commandFailed(String reason) {
    return '$reason';
  }
}
