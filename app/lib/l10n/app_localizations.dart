import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:intl/intl.dart' as intl;

import 'app_localizations_en.dart';

// ignore_for_file: type=lint

/// Callers can lookup localized strings with an instance of Strings
/// returned by `Strings.of(context)`.
///
/// Applications need to include `Strings.delegate()` in their app's
/// `localizationDelegates` list, and the locales they support in the app's
/// `supportedLocales` list. For example:
///
/// ```dart
/// import 'l10n/app_localizations.dart';
///
/// return MaterialApp(
///   localizationsDelegates: Strings.localizationsDelegates,
///   supportedLocales: Strings.supportedLocales,
///   home: MyApplicationHome(),
/// );
/// ```
///
/// ## Update pubspec.yaml
///
/// Please make sure to update your pubspec.yaml to include the following
/// packages:
///
/// ```yaml
/// dependencies:
///   # Internationalization support.
///   flutter_localizations:
///     sdk: flutter
///   intl: any # Use the pinned version from flutter_localizations
///
///   # Rest of dependencies
/// ```
///
/// ## iOS Applications
///
/// iOS applications define key application metadata, including supported
/// locales, in an Info.plist file that is built into the application bundle.
/// To configure the locales supported by your app, you’ll need to edit this
/// file.
///
/// First, open your project’s ios/Runner.xcworkspace Xcode workspace file.
/// Then, in the Project Navigator, open the Info.plist file under the Runner
/// project’s Runner folder.
///
/// Next, select the Information Property List item, select Add Item from the
/// Editor menu, then select Localizations from the pop-up menu.
///
/// Select and expand the newly-created Localizations item then, for each
/// locale your application supports, add a new item and select the locale
/// you wish to add from the pop-up menu in the Value field. This list should
/// be consistent with the languages listed in the Strings.supportedLocales
/// property.
abstract class Strings {
  Strings(String locale)
    : localeName = intl.Intl.canonicalizedLocale(locale.toString());

  final String localeName;

  static Strings of(BuildContext context) {
    return Localizations.of<Strings>(context, Strings)!;
  }

  static const LocalizationsDelegate<Strings> delegate = _StringsDelegate();

  /// A list of this localizations delegate along with the default localizations
  /// delegates.
  ///
  /// Returns a list of localizations delegates containing this delegate along with
  /// GlobalMaterialLocalizations.delegate, GlobalCupertinoLocalizations.delegate,
  /// and GlobalWidgetsLocalizations.delegate.
  ///
  /// Additional delegates can be added by appending to this list in
  /// MaterialApp. This list does not have to be used at all if a custom list
  /// of delegates is preferred or required.
  static const List<LocalizationsDelegate<dynamic>> localizationsDelegates =
      <LocalizationsDelegate<dynamic>>[
        delegate,
        GlobalMaterialLocalizations.delegate,
        GlobalCupertinoLocalizations.delegate,
        GlobalWidgetsLocalizations.delegate,
      ];

  /// A list of this localizations delegate's supported locales.
  static const List<Locale> supportedLocales = <Locale>[Locale('en')];

  /// No description provided for @appName.
  ///
  /// In en, this message translates to:
  /// **'Anvil'**
  String get appName;

  /// No description provided for @signInTitle.
  ///
  /// In en, this message translates to:
  /// **'Sign in'**
  String get signInTitle;

  /// No description provided for @signInServer.
  ///
  /// In en, this message translates to:
  /// **'Server or email address'**
  String get signInServer;

  /// No description provided for @signInServerHint.
  ///
  /// In en, this message translates to:
  /// **'pbx.example.com or you@example.com'**
  String get signInServerHint;

  /// No description provided for @signInUsername.
  ///
  /// In en, this message translates to:
  /// **'Username'**
  String get signInUsername;

  /// No description provided for @signInPassword.
  ///
  /// In en, this message translates to:
  /// **'Password'**
  String get signInPassword;

  /// No description provided for @signInCode.
  ///
  /// In en, this message translates to:
  /// **'One-time code'**
  String get signInCode;

  /// No description provided for @signInButton.
  ///
  /// In en, this message translates to:
  /// **'Sign in'**
  String get signInButton;

  /// No description provided for @signInFailed.
  ///
  /// In en, this message translates to:
  /// **'Could not sign in: {reason}'**
  String signInFailed(String reason);

  /// No description provided for @registrationRegistered.
  ///
  /// In en, this message translates to:
  /// **'Ready'**
  String get registrationRegistered;

  /// No description provided for @registrationRegistering.
  ///
  /// In en, this message translates to:
  /// **'Connecting…'**
  String get registrationRegistering;

  /// No description provided for @registrationUnregistered.
  ///
  /// In en, this message translates to:
  /// **'Offline'**
  String get registrationUnregistered;

  /// No description provided for @registrationFailed.
  ///
  /// In en, this message translates to:
  /// **'Could not connect'**
  String get registrationFailed;

  /// No description provided for @keypadHint.
  ///
  /// In en, this message translates to:
  /// **'Number, extension or name'**
  String get keypadHint;

  /// No description provided for @callButton.
  ///
  /// In en, this message translates to:
  /// **'Call'**
  String get callButton;

  /// No description provided for @answerButton.
  ///
  /// In en, this message translates to:
  /// **'Answer'**
  String get answerButton;

  /// No description provided for @declineButton.
  ///
  /// In en, this message translates to:
  /// **'Decline'**
  String get declineButton;

  /// No description provided for @hangupButton.
  ///
  /// In en, this message translates to:
  /// **'End'**
  String get hangupButton;

  /// No description provided for @muteButton.
  ///
  /// In en, this message translates to:
  /// **'Mute'**
  String get muteButton;

  /// No description provided for @unmuteButton.
  ///
  /// In en, this message translates to:
  /// **'Unmute'**
  String get unmuteButton;

  /// No description provided for @holdButton.
  ///
  /// In en, this message translates to:
  /// **'Hold'**
  String get holdButton;

  /// No description provided for @resumeButton.
  ///
  /// In en, this message translates to:
  /// **'Resume'**
  String get resumeButton;

  /// No description provided for @keypadButton.
  ///
  /// In en, this message translates to:
  /// **'Keypad'**
  String get keypadButton;

  /// No description provided for @incomingCall.
  ///
  /// In en, this message translates to:
  /// **'Incoming call'**
  String get incomingCall;

  /// No description provided for @callDialing.
  ///
  /// In en, this message translates to:
  /// **'Calling…'**
  String get callDialing;

  /// No description provided for @callRinging.
  ///
  /// In en, this message translates to:
  /// **'Ringing…'**
  String get callRinging;

  /// No description provided for @callConnected.
  ///
  /// In en, this message translates to:
  /// **'Connected'**
  String get callConnected;

  /// No description provided for @callHeld.
  ///
  /// In en, this message translates to:
  /// **'On hold'**
  String get callHeld;

  /// No description provided for @noCalls.
  ///
  /// In en, this message translates to:
  /// **'No calls'**
  String get noCalls;

  /// No description provided for @voicemailCount.
  ///
  /// In en, this message translates to:
  /// **'{count, plural, =0{No new voicemail} =1{1 new voicemail} other{{count} new voicemails}}'**
  String voicemailCount(int count);

  /// No description provided for @dndOn.
  ///
  /// In en, this message translates to:
  /// **'Do not disturb'**
  String get dndOn;

  /// No description provided for @signOut.
  ///
  /// In en, this message translates to:
  /// **'Sign out'**
  String get signOut;

  /// No description provided for @callEnded.
  ///
  /// In en, this message translates to:
  /// **'Call ended: {reason}'**
  String callEnded(String reason);

  /// No description provided for @commandFailed.
  ///
  /// In en, this message translates to:
  /// **'{reason}'**
  String commandFailed(String reason);
}

class _StringsDelegate extends LocalizationsDelegate<Strings> {
  const _StringsDelegate();

  @override
  Future<Strings> load(Locale locale) {
    return SynchronousFuture<Strings>(lookupStrings(locale));
  }

  @override
  bool isSupported(Locale locale) =>
      <String>['en'].contains(locale.languageCode);

  @override
  bool shouldReload(_StringsDelegate old) => false;
}

Strings lookupStrings(Locale locale) {
  // Lookup logic when only language code is specified.
  switch (locale.languageCode) {
    case 'en':
      return StringsEn();
  }

  throw FlutterError(
    'Strings.delegate failed to load unsupported locale "$locale". This is likely '
    'an issue with the localizations generation tool. Please file an issue '
    'on GitHub with a reproducible sample app and the gen-l10n configuration '
    'that was used.',
  );
}
