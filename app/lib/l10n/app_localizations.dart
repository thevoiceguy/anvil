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

  /// No description provided for @signInContinue.
  ///
  /// In en, this message translates to:
  /// **'Continue'**
  String get signInContinue;

  /// No description provided for @signInChange.
  ///
  /// In en, this message translates to:
  /// **'Change'**
  String get signInChange;

  /// No description provided for @signInUseSso.
  ///
  /// In en, this message translates to:
  /// **'Use single sign-on'**
  String get signInUseSso;

  /// No description provided for @signInBrowserButton.
  ///
  /// In en, this message translates to:
  /// **'Continue in your browser'**
  String get signInBrowserButton;

  /// No description provided for @signInBrowserWaiting.
  ///
  /// In en, this message translates to:
  /// **'Finish signing in in your browser.'**
  String get signInBrowserWaiting;

  /// No description provided for @signInSsoUnavailable.
  ///
  /// In en, this message translates to:
  /// **'This account signs in with single sign-on, which Anvil cannot do on this device yet.'**
  String get signInSsoUnavailable;

  /// No description provided for @signInCodePrompt.
  ///
  /// In en, this message translates to:
  /// **'Enter the one-time code from your authenticator app.'**
  String get signInCodePrompt;

  /// No description provided for @signInVerify.
  ///
  /// In en, this message translates to:
  /// **'Verify'**
  String get signInVerify;

  /// No description provided for @signInServerNeeded.
  ///
  /// In en, this message translates to:
  /// **'Enter your server or email address first.'**
  String get signInServerNeeded;

  /// No description provided for @signInUsernameNeeded.
  ///
  /// In en, this message translates to:
  /// **'Enter your username.'**
  String get signInUsernameNeeded;

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

  /// No description provided for @microphoneRefused.
  ///
  /// In en, this message translates to:
  /// **'Anvil may not use the microphone, so callers won\'t hear you. Allow it in the system\'s settings for Anvil.'**
  String get microphoneRefused;

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
  /// **'Contacts'**
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

  /// No description provided for @clearRecentsButton.
  ///
  /// In en, this message translates to:
  /// **'Clear recent calls'**
  String get clearRecentsButton;

  /// No description provided for @clearRecentsConfirm.
  ///
  /// In en, this message translates to:
  /// **'Clear your recent calls from this device? Your organisation keeps its call records.'**
  String get clearRecentsConfirm;

  /// No description provided for @clearButton.
  ///
  /// In en, this message translates to:
  /// **'Clear'**
  String get clearButton;

  /// No description provided for @removeRecentButton.
  ///
  /// In en, this message translates to:
  /// **'Remove from recents'**
  String get removeRecentButton;

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
  /// **'Search contacts'**
  String get peopleSearch;

  /// No description provided for @peopleEmpty.
  ///
  /// In en, this message translates to:
  /// **'No contacts to show'**
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

  /// No description provided for @routeEarpiece.
  ///
  /// In en, this message translates to:
  /// **'Phone'**
  String get routeEarpiece;

  /// No description provided for @routeSpeaker.
  ///
  /// In en, this message translates to:
  /// **'Speaker'**
  String get routeSpeaker;

  /// No description provided for @routeBluetooth.
  ///
  /// In en, this message translates to:
  /// **'Bluetooth'**
  String get routeBluetooth;

  /// No description provided for @routeWired.
  ///
  /// In en, this message translates to:
  /// **'Headset'**
  String get routeWired;

  /// No description provided for @systemDefault.
  ///
  /// In en, this message translates to:
  /// **'System default'**
  String get systemDefault;

  /// No description provided for @audioNote.
  ///
  /// In en, this message translates to:
  /// **'A call that is up moves to the devices chosen'**
  String get audioNote;

  /// No description provided for @microphoneVolume.
  ///
  /// In en, this message translates to:
  /// **'Microphone volume'**
  String get microphoneVolume;

  /// No description provided for @speakerVolume.
  ///
  /// In en, this message translates to:
  /// **'Speaker volume'**
  String get speakerVolume;

  /// No description provided for @audioButton.
  ///
  /// In en, this message translates to:
  /// **'Audio'**
  String get audioButton;

  /// No description provided for @ringOn.
  ///
  /// In en, this message translates to:
  /// **'Ring on'**
  String get ringOn;

  /// No description provided for @ringtone.
  ///
  /// In en, this message translates to:
  /// **'Ringtone'**
  String get ringtone;

  /// No description provided for @ringtoneAnvil.
  ///
  /// In en, this message translates to:
  /// **'Anvil'**
  String get ringtoneAnvil;

  /// No description provided for @ringtoneClassic.
  ///
  /// In en, this message translates to:
  /// **'Classic'**
  String get ringtoneClassic;

  /// No description provided for @ringtoneFile.
  ///
  /// In en, this message translates to:
  /// **'Choose a file…'**
  String get ringtoneFile;

  /// No description provided for @ringtoneFileType.
  ///
  /// In en, this message translates to:
  /// **'Sound files'**
  String get ringtoneFileType;

  /// No description provided for @ringtoneRemove.
  ///
  /// In en, this message translates to:
  /// **'Remove this ringtone'**
  String get ringtoneRemove;

  /// No description provided for @audioTests.
  ///
  /// In en, this message translates to:
  /// **'Test your audio'**
  String get audioTests;

  /// No description provided for @testMicrophone.
  ///
  /// In en, this message translates to:
  /// **'Test microphone'**
  String get testMicrophone;

  /// No description provided for @testMicrophoneNote.
  ///
  /// In en, this message translates to:
  /// **'Say something for five seconds, then hear it played back on your speaker at your volumes.'**
  String get testMicrophoneNote;

  /// No description provided for @testSpeaker.
  ///
  /// In en, this message translates to:
  /// **'Test speaker'**
  String get testSpeaker;

  /// No description provided for @testRinging.
  ///
  /// In en, this message translates to:
  /// **'Test ringing'**
  String get testRinging;

  /// No description provided for @testRecording.
  ///
  /// In en, this message translates to:
  /// **'Listening… speak now'**
  String get testRecording;

  /// No description provided for @testPlayingBack.
  ///
  /// In en, this message translates to:
  /// **'Playing back what you said'**
  String get testPlayingBack;

  /// No description provided for @testSpeakerPlaying.
  ///
  /// In en, this message translates to:
  /// **'Playing on your speaker'**
  String get testSpeakerPlaying;

  /// No description provided for @testRingPlaying.
  ///
  /// In en, this message translates to:
  /// **'Ringing on the ringing speaker'**
  String get testRingPlaying;

  /// No description provided for @testStop.
  ///
  /// In en, this message translates to:
  /// **'Stop'**
  String get testStop;

  /// No description provided for @echoCancellation.
  ///
  /// In en, this message translates to:
  /// **'Echo cancellation'**
  String get echoCancellation;

  /// No description provided for @echoCancellationNote.
  ///
  /// In en, this message translates to:
  /// **'Stops the other person hearing themselves through your speakers. Turn it off only with a headset, if your voice sounds clipped.'**
  String get echoCancellationNote;

  /// No description provided for @noiseSuppression.
  ///
  /// In en, this message translates to:
  /// **'Noise suppression'**
  String get noiseSuppression;

  /// No description provided for @noiseSuppressionNote.
  ///
  /// In en, this message translates to:
  /// **'Turns down steady noise around you, such as fans and hum.'**
  String get noiseSuppressionNote;

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

  /// No description provided for @trayShow.
  ///
  /// In en, this message translates to:
  /// **'Show {app}'**
  String trayShow(String app);

  /// No description provided for @trayQuit.
  ///
  /// In en, this message translates to:
  /// **'Quit'**
  String get trayQuit;

  /// No description provided for @settingsDesktop.
  ///
  /// In en, this message translates to:
  /// **'This computer'**
  String get settingsDesktop;

  /// No description provided for @startAtLogin.
  ///
  /// In en, this message translates to:
  /// **'Start at login'**
  String get startAtLogin;

  /// No description provided for @startAtLoginSubtitle.
  ///
  /// In en, this message translates to:
  /// **'Ready for calls when you sign in to the computer'**
  String get startAtLoginSubtitle;

  /// No description provided for @updateAvailable.
  ///
  /// In en, this message translates to:
  /// **'Anvil {version} is available'**
  String updateAvailable(String version);

  /// No description provided for @updateNow.
  ///
  /// In en, this message translates to:
  /// **'Update and restart'**
  String get updateNow;

  /// No description provided for @updateGetIt.
  ///
  /// In en, this message translates to:
  /// **'Download'**
  String get updateGetIt;

  /// No description provided for @updateLater.
  ///
  /// In en, this message translates to:
  /// **'Later'**
  String get updateLater;

  /// No description provided for @updateAfterCall.
  ///
  /// In en, this message translates to:
  /// **'After the call'**
  String get updateAfterCall;

  /// No description provided for @updateDownloading.
  ///
  /// In en, this message translates to:
  /// **'Downloading the update…'**
  String get updateDownloading;

  /// No description provided for @updateChecking.
  ///
  /// In en, this message translates to:
  /// **'Checking for updates…'**
  String get updateChecking;

  /// No description provided for @updateNone.
  ///
  /// In en, this message translates to:
  /// **'Anvil is up to date'**
  String get updateNone;

  /// No description provided for @updateFailed.
  ///
  /// In en, this message translates to:
  /// **'Could not update: {reason}'**
  String updateFailed(String reason);

  /// No description provided for @settingsUpdates.
  ///
  /// In en, this message translates to:
  /// **'Updates'**
  String get settingsUpdates;

  /// No description provided for @versionLabel.
  ///
  /// In en, this message translates to:
  /// **'Anvil {version}'**
  String versionLabel(String version);

  /// No description provided for @checkForUpdates.
  ///
  /// In en, this message translates to:
  /// **'Check for updates'**
  String get checkForUpdates;

  /// No description provided for @qualityPackets.
  ///
  /// In en, this message translates to:
  /// **'{received} packets received, {played} played, {dropped} dropped'**
  String qualityPackets(int received, int played, int dropped);

  /// No description provided for @qualityUnexpectedPt.
  ///
  /// In en, this message translates to:
  /// **'Payload type {pt} arrived; this call\'s codec is {expected}'**
  String qualityUnexpectedPt(int pt, int expected);

  /// No description provided for @qualityFrom.
  ///
  /// In en, this message translates to:
  /// **'From {source}'**
  String qualityFrom(String source);

  /// No description provided for @settingsTroubleshooting.
  ///
  /// In en, this message translates to:
  /// **'Troubleshooting'**
  String get settingsTroubleshooting;

  /// No description provided for @signInServerLocked.
  ///
  /// In en, this message translates to:
  /// **'Set by your organisation'**
  String get signInServerLocked;

  /// No description provided for @managedBy.
  ///
  /// In en, this message translates to:
  /// **'Set up by your organisation'**
  String get managedBy;

  /// No description provided for @updateManaged.
  ///
  /// In en, this message translates to:
  /// **'Your organisation installs updates to this app.'**
  String get updateManaged;

  /// No description provided for @logFile.
  ///
  /// In en, this message translates to:
  /// **'Log file'**
  String get logFile;

  /// No description provided for @logFileCopy.
  ///
  /// In en, this message translates to:
  /// **'Copy the path'**
  String get logFileCopy;

  /// No description provided for @logFileCopied.
  ///
  /// In en, this message translates to:
  /// **'Path copied'**
  String get logFileCopied;

  /// No description provided for @logFileOpenFolder.
  ///
  /// In en, this message translates to:
  /// **'Open the folder'**
  String get logFileOpenFolder;

  /// No description provided for @recordButton.
  ///
  /// In en, this message translates to:
  /// **'Record'**
  String get recordButton;

  /// No description provided for @stopRecordingButton.
  ///
  /// In en, this message translates to:
  /// **'Stop recording'**
  String get stopRecordingButton;

  /// No description provided for @recordingNow.
  ///
  /// In en, this message translates to:
  /// **'Recording'**
  String get recordingNow;

  /// No description provided for @moreButton.
  ///
  /// In en, this message translates to:
  /// **'More'**
  String get moreButton;

  /// No description provided for @createContactButton.
  ///
  /// In en, this message translates to:
  /// **'Create contact'**
  String get createContactButton;

  /// No description provided for @editContactButton.
  ///
  /// In en, this message translates to:
  /// **'Edit'**
  String get editContactButton;

  /// No description provided for @contactNewTitle.
  ///
  /// In en, this message translates to:
  /// **'New contact'**
  String get contactNewTitle;

  /// No description provided for @contactEditTitle.
  ///
  /// In en, this message translates to:
  /// **'Edit contact'**
  String get contactEditTitle;

  /// No description provided for @contactNameField.
  ///
  /// In en, this message translates to:
  /// **'Name'**
  String get contactNameField;

  /// No description provided for @contactCompanyField.
  ///
  /// In en, this message translates to:
  /// **'Company'**
  String get contactCompanyField;

  /// No description provided for @contactNumberField.
  ///
  /// In en, this message translates to:
  /// **'Number'**
  String get contactNumberField;

  /// No description provided for @contactLabelField.
  ///
  /// In en, this message translates to:
  /// **'Label'**
  String get contactLabelField;

  /// No description provided for @labelMobile.
  ///
  /// In en, this message translates to:
  /// **'Mobile'**
  String get labelMobile;

  /// No description provided for @labelWork.
  ///
  /// In en, this message translates to:
  /// **'Work'**
  String get labelWork;

  /// No description provided for @labelHome.
  ///
  /// In en, this message translates to:
  /// **'Home'**
  String get labelHome;

  /// No description provided for @labelMain.
  ///
  /// In en, this message translates to:
  /// **'Main'**
  String get labelMain;

  /// No description provided for @labelWorkFax.
  ///
  /// In en, this message translates to:
  /// **'Work fax'**
  String get labelWorkFax;

  /// No description provided for @labelOther.
  ///
  /// In en, this message translates to:
  /// **'Other'**
  String get labelOther;

  /// No description provided for @labelCustom.
  ///
  /// In en, this message translates to:
  /// **'Custom…'**
  String get labelCustom;

  /// No description provided for @labelBackToList.
  ///
  /// In en, this message translates to:
  /// **'Choose from the list'**
  String get labelBackToList;

  /// No description provided for @directoryTab.
  ///
  /// In en, this message translates to:
  /// **'Directory'**
  String get directoryTab;

  /// No description provided for @contactsEmpty.
  ///
  /// In en, this message translates to:
  /// **'No contacts yet. Add one with the button above.'**
  String get contactsEmpty;

  /// No description provided for @directoryEmpty.
  ///
  /// In en, this message translates to:
  /// **'No one to show'**
  String get directoryEmpty;

  /// No description provided for @contactNumbers.
  ///
  /// In en, this message translates to:
  /// **'{count} numbers'**
  String contactNumbers(int count);

  /// No description provided for @contactEmailField.
  ///
  /// In en, this message translates to:
  /// **'Email'**
  String get contactEmailField;

  /// No description provided for @contactNotesField.
  ///
  /// In en, this message translates to:
  /// **'Notes'**
  String get contactNotesField;

  /// No description provided for @contactNeedsName.
  ///
  /// In en, this message translates to:
  /// **'Enter a name'**
  String get contactNeedsName;

  /// No description provided for @addNumberButton.
  ///
  /// In en, this message translates to:
  /// **'Add a number'**
  String get addNumberButton;

  /// No description provided for @removeNumberButton.
  ///
  /// In en, this message translates to:
  /// **'Remove this number'**
  String get removeNumberButton;

  /// No description provided for @contactsHeading.
  ///
  /// In en, this message translates to:
  /// **'My contacts'**
  String get contactsHeading;

  /// No description provided for @contactOnDevice.
  ///
  /// In en, this message translates to:
  /// **'On this device'**
  String get contactOnDevice;

  /// No description provided for @contactOnAccount.
  ///
  /// In en, this message translates to:
  /// **'On your account'**
  String get contactOnAccount;

  /// No description provided for @contactPlaceAccount.
  ///
  /// In en, this message translates to:
  /// **'My account'**
  String get contactPlaceAccount;

  /// No description provided for @contactPlaceDevice.
  ///
  /// In en, this message translates to:
  /// **'This device'**
  String get contactPlaceDevice;

  /// No description provided for @moveContactToAccountButton.
  ///
  /// In en, this message translates to:
  /// **'Move to my account'**
  String get moveContactToAccountButton;

  /// No description provided for @deleteContactConfirm.
  ///
  /// In en, this message translates to:
  /// **'Delete {name} from your contacts?'**
  String deleteContactConfirm(String name);

  /// No description provided for @deleteAccountContactConfirm.
  ///
  /// In en, this message translates to:
  /// **'Delete {name} from your contacts? They go from every app you sign in to.'**
  String deleteAccountContactConfirm(String name);

  /// No description provided for @contactsMenu.
  ///
  /// In en, this message translates to:
  /// **'Import and export'**
  String get contactsMenu;

  /// No description provided for @contactsImport.
  ///
  /// In en, this message translates to:
  /// **'Import contacts…'**
  String get contactsImport;

  /// No description provided for @contactsImportToAccount.
  ///
  /// In en, this message translates to:
  /// **'Import to my account…'**
  String get contactsImportToAccount;

  /// No description provided for @contactsImportToDevice.
  ///
  /// In en, this message translates to:
  /// **'Import to this device…'**
  String get contactsImportToDevice;

  /// No description provided for @contactsExport.
  ///
  /// In en, this message translates to:
  /// **'Export contacts…'**
  String get contactsExport;

  /// No description provided for @contactsFileType.
  ///
  /// In en, this message translates to:
  /// **'Contacts (vCard or CSV)'**
  String get contactsFileType;

  /// No description provided for @contactsImported.
  ///
  /// In en, this message translates to:
  /// **'{imported} imported, {duplicates} already kept'**
  String contactsImported(int imported, int duplicates);

  /// No description provided for @contactsImportSkipped.
  ///
  /// In en, this message translates to:
  /// **'{count} left out: {first}'**
  String contactsImportSkipped(int count, String first);

  /// No description provided for @contactsExported.
  ///
  /// In en, this message translates to:
  /// **'Contacts saved to {path}'**
  String contactsExported(String path);

  /// No description provided for @contactRingtoneField.
  ///
  /// In en, this message translates to:
  /// **'Ringtone'**
  String get contactRingtoneField;

  /// No description provided for @contactRingtoneDefault.
  ///
  /// In en, this message translates to:
  /// **'The phone\'s ringtone'**
  String get contactRingtoneDefault;

  /// No description provided for @contactRingtoneElsewhere.
  ///
  /// In en, this message translates to:
  /// **'Chosen on another device'**
  String get contactRingtoneElsewhere;

  /// No description provided for @directoryListed.
  ///
  /// In en, this message translates to:
  /// **'Listed in the directory'**
  String get directoryListed;

  /// No description provided for @directoryListedSubtitle.
  ///
  /// In en, this message translates to:
  /// **'People in your organisation can find you under People'**
  String get directoryListedSubtitle;

  /// No description provided for @directoryListedLocked.
  ///
  /// In en, this message translates to:
  /// **'Your administrator decides whether you are listed'**
  String get directoryListedLocked;
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
