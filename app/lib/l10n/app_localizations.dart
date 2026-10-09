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

  /// No description provided for @navKeypad.
  ///
  /// In en, this message translates to:
  /// **'Keypad'**
  String get navKeypad;

  /// No description provided for @navRecents.
  ///
  /// In en, this message translates to:
  /// **'Recents'**
  String get navRecents;

  /// No description provided for @navPeople.
  ///
  /// In en, this message translates to:
  /// **'People'**
  String get navPeople;

  /// No description provided for @navVoicemail.
  ///
  /// In en, this message translates to:
  /// **'Voicemail'**
  String get navVoicemail;

  /// No description provided for @navSettings.
  ///
  /// In en, this message translates to:
  /// **'Settings'**
  String get navSettings;

  /// No description provided for @transferButton.
  ///
  /// In en, this message translates to:
  /// **'Transfer'**
  String get transferButton;

  /// No description provided for @parkButton.
  ///
  /// In en, this message translates to:
  /// **'Park'**
  String get parkButton;

  /// No description provided for @addCallButton.
  ///
  /// In en, this message translates to:
  /// **'Add call'**
  String get addCallButton;

  /// No description provided for @hideKeypadButton.
  ///
  /// In en, this message translates to:
  /// **'Hide keypad'**
  String get hideKeypadButton;

  /// No description provided for @backToCallButton.
  ///
  /// In en, this message translates to:
  /// **'Back to the call'**
  String get backToCallButton;

  /// No description provided for @completeTransferButton.
  ///
  /// In en, this message translates to:
  /// **'Complete transfer'**
  String get completeTransferButton;

  /// No description provided for @encrypted.
  ///
  /// In en, this message translates to:
  /// **'Encrypted'**
  String get encrypted;

  /// No description provided for @notEncrypted.
  ///
  /// In en, this message translates to:
  /// **'Not encrypted'**
  String get notEncrypted;

  /// No description provided for @qualityGood.
  ///
  /// In en, this message translates to:
  /// **'Good connection'**
  String get qualityGood;

  /// No description provided for @qualityFair.
  ///
  /// In en, this message translates to:
  /// **'Fair connection'**
  String get qualityFair;

  /// No description provided for @qualityPoor.
  ///
  /// In en, this message translates to:
  /// **'Poor connection'**
  String get qualityPoor;

  /// No description provided for @transferTitle.
  ///
  /// In en, this message translates to:
  /// **'Transfer {name}'**
  String transferTitle(String name);

  /// No description provided for @transferNow.
  ///
  /// In en, this message translates to:
  /// **'Transfer now'**
  String get transferNow;

  /// No description provided for @transferConsult.
  ///
  /// In en, this message translates to:
  /// **'Talk first'**
  String get transferConsult;

  /// No description provided for @cancelButton.
  ///
  /// In en, this message translates to:
  /// **'Cancel'**
  String get cancelButton;

  /// No description provided for @transferProgress.
  ///
  /// In en, this message translates to:
  /// **'Transfer: {reason}'**
  String transferProgress(String reason);

  /// No description provided for @otherCalls.
  ///
  /// In en, this message translates to:
  /// **'Other calls'**
  String get otherCalls;

  /// No description provided for @recentsEmpty.
  ///
  /// In en, this message translates to:
  /// **'No recent calls'**
  String get recentsEmpty;

  /// No description provided for @recentMissed.
  ///
  /// In en, this message translates to:
  /// **'Missed'**
  String get recentMissed;

  /// No description provided for @callBackButton.
  ///
  /// In en, this message translates to:
  /// **'Call back'**
  String get callBackButton;

  /// No description provided for @refreshButton.
  ///
  /// In en, this message translates to:
  /// **'Refresh'**
  String get refreshButton;

  /// No description provided for @yesterday.
  ///
  /// In en, this message translates to:
  /// **'Yesterday'**
  String get yesterday;

  /// No description provided for @peopleSearch.
  ///
  /// In en, this message translates to:
  /// **'Search people'**
  String get peopleSearch;

  /// No description provided for @peopleEmpty.
  ///
  /// In en, this message translates to:
  /// **'Nobody to show'**
  String get peopleEmpty;

  /// No description provided for @favourites.
  ///
  /// In en, this message translates to:
  /// **'Favourites'**
  String get favourites;

  /// No description provided for @everyone.
  ///
  /// In en, this message translates to:
  /// **'Everyone'**
  String get everyone;

  /// No description provided for @onACall.
  ///
  /// In en, this message translates to:
  /// **'On a call'**
  String get onACall;

  /// No description provided for @presenceAvailable.
  ///
  /// In en, this message translates to:
  /// **'Available'**
  String get presenceAvailable;

  /// No description provided for @presenceAway.
  ///
  /// In en, this message translates to:
  /// **'Away'**
  String get presenceAway;

  /// No description provided for @presenceBusy.
  ///
  /// In en, this message translates to:
  /// **'Busy'**
  String get presenceBusy;

  /// No description provided for @presenceDnd.
  ///
  /// In en, this message translates to:
  /// **'Do not disturb'**
  String get presenceDnd;

  /// No description provided for @presenceOffline.
  ///
  /// In en, this message translates to:
  /// **'Offline'**
  String get presenceOffline;

  /// No description provided for @favouriteAdd.
  ///
  /// In en, this message translates to:
  /// **'Add to favourites'**
  String get favouriteAdd;

  /// No description provided for @favouriteRemove.
  ///
  /// In en, this message translates to:
  /// **'Remove from favourites'**
  String get favouriteRemove;

  /// No description provided for @callPerson.
  ///
  /// In en, this message translates to:
  /// **'Call {name}'**
  String callPerson(String name);

  /// No description provided for @voicemailEmpty.
  ///
  /// In en, this message translates to:
  /// **'No messages'**
  String get voicemailEmpty;

  /// No description provided for @urgent.
  ///
  /// In en, this message translates to:
  /// **'Urgent'**
  String get urgent;

  /// No description provided for @playButton.
  ///
  /// In en, this message translates to:
  /// **'Play'**
  String get playButton;

  /// No description provided for @stopButton.
  ///
  /// In en, this message translates to:
  /// **'Stop'**
  String get stopButton;

  /// No description provided for @markHeardButton.
  ///
  /// In en, this message translates to:
  /// **'Mark heard'**
  String get markHeardButton;

  /// No description provided for @deleteButton.
  ///
  /// In en, this message translates to:
  /// **'Delete'**
  String get deleteButton;

  /// No description provided for @deleteMessageConfirm.
  ///
  /// In en, this message translates to:
  /// **'Delete the message from {name}?'**
  String deleteMessageConfirm(String name);

  /// No description provided for @settingsCalls.
  ///
  /// In en, this message translates to:
  /// **'Calls'**
  String get settingsCalls;

  /// No description provided for @dndSubtitle.
  ///
  /// In en, this message translates to:
  /// **'Calls go to voicemail, or are refused'**
  String get dndSubtitle;

  /// No description provided for @callWaiting.
  ///
  /// In en, this message translates to:
  /// **'Call waiting'**
  String get callWaiting;

  /// No description provided for @callWaitingSubtitle.
  ///
  /// In en, this message translates to:
  /// **'Ring with a second call while one is up'**
  String get callWaitingSubtitle;

  /// No description provided for @settingsForwarding.
  ///
  /// In en, this message translates to:
  /// **'Forwarding'**
  String get settingsForwarding;

  /// No description provided for @forwardAll.
  ///
  /// In en, this message translates to:
  /// **'Always'**
  String get forwardAll;

  /// No description provided for @forwardBusy.
  ///
  /// In en, this message translates to:
  /// **'When busy'**
  String get forwardBusy;

  /// No description provided for @forwardNoAnswer.
  ///
  /// In en, this message translates to:
  /// **'When not answered'**
  String get forwardNoAnswer;

  /// No description provided for @forwardUnreachable.
  ///
  /// In en, this message translates to:
  /// **'When unreachable'**
  String get forwardUnreachable;

  /// No description provided for @forwardOff.
  ///
  /// In en, this message translates to:
  /// **'Off'**
  String get forwardOff;

  /// No description provided for @forwardTo.
  ///
  /// In en, this message translates to:
  /// **'Forward to'**
  String get forwardTo;

  /// No description provided for @forwardRingSeconds.
  ///
  /// In en, this message translates to:
  /// **'Ring for (seconds)'**
  String get forwardRingSeconds;

  /// No description provided for @saveButton.
  ///
  /// In en, this message translates to:
  /// **'Save'**
  String get saveButton;

  /// No description provided for @clearButton.
  ///
  /// In en, this message translates to:
  /// **'Clear'**
  String get clearButton;

  /// No description provided for @settingsAudio.
  ///
  /// In en, this message translates to:
  /// **'Audio'**
  String get settingsAudio;

  /// No description provided for @microphone.
  ///
  /// In en, this message translates to:
  /// **'Microphone'**
  String get microphone;

  /// No description provided for @speaker.
  ///
  /// In en, this message translates to:
  /// **'Speaker'**
  String get speaker;

  /// No description provided for @systemDefault.
  ///
  /// In en, this message translates to:
  /// **'System default'**
  String get systemDefault;

  /// No description provided for @audioNote.
  ///
  /// In en, this message translates to:
  /// **'Applies from the next call'**
  String get audioNote;

  /// No description provided for @settingsAccount.
  ///
  /// In en, this message translates to:
  /// **'Account'**
  String get settingsAccount;

  /// No description provided for @signedInAs.
  ///
  /// In en, this message translates to:
  /// **'{user} on {server}'**
  String signedInAs(String user, String server);

  /// No description provided for @needsFcp.
  ///
  /// In en, this message translates to:
  /// **'Needs a phone signed in to FCP'**
  String get needsFcp;
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
