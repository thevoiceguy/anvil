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
  String get signInContinue => 'Continue';

  @override
  String get signInChange => 'Change';

  @override
  String get signInUseSso => 'Use single sign-on';

  @override
  String get signInBrowserButton => 'Continue in your browser';

  @override
  String get signInBrowserWaiting => 'Finish signing in in your browser.';

  @override
  String get signInSsoUnavailable =>
      'This account signs in with single sign-on, which Anvil cannot do on this device yet.';

  @override
  String get signInCodePrompt =>
      'Enter the one-time code from your authenticator app.';

  @override
  String get signInVerify => 'Verify';

  @override
  String get signInServerNeeded => 'Enter your server or email address first.';

  @override
  String get signInUsernameNeeded => 'Enter your username.';

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
  String get microphoneRefused =>
      'Anvil may not use the microphone, so callers won\'t hear you. Allow it in the system\'s settings for Anvil.';

  @override
  String commandFailed(String reason) {
    return '$reason';
  }

  @override
  String get navKeypad => 'Keypad';

  @override
  String get navRecents => 'Recents';

  @override
  String get navPeople => 'Contacts';

  @override
  String get navVoicemail => 'Voicemail';

  @override
  String get navSettings => 'Settings';

  @override
  String get transferButton => 'Transfer';

  @override
  String get parkButton => 'Park';

  @override
  String get addCallButton => 'Add call';

  @override
  String get hideKeypadButton => 'Hide keypad';

  @override
  String get backToCallButton => 'Back to the call';

  @override
  String get completeTransferButton => 'Complete transfer';

  @override
  String get encrypted => 'Encrypted';

  @override
  String get notEncrypted => 'Not encrypted';

  @override
  String get qualityGood => 'Good connection';

  @override
  String get qualityFair => 'Fair connection';

  @override
  String get qualityPoor => 'Poor connection';

  @override
  String transferTitle(String name) {
    return 'Transfer $name';
  }

  @override
  String get transferNow => 'Transfer now';

  @override
  String get transferConsult => 'Talk first';

  @override
  String get cancelButton => 'Cancel';

  @override
  String transferProgress(String reason) {
    return 'Transfer: $reason';
  }

  @override
  String get otherCalls => 'Other calls';

  @override
  String get recentsEmpty => 'No recent calls';

  @override
  String get recentMissed => 'Missed';

  @override
  String get callBackButton => 'Call back';

  @override
  String get clearRecentsButton => 'Clear recent calls';

  @override
  String get clearRecentsConfirm =>
      'Clear your recent calls from this device? Your organisation keeps its call records.';

  @override
  String get clearButton => 'Clear';

  @override
  String get removeRecentButton => 'Remove from recents';

  @override
  String get refreshButton => 'Refresh';

  @override
  String get yesterday => 'Yesterday';

  @override
  String get peopleSearch => 'Search contacts';

  @override
  String get peopleEmpty => 'No contacts to show';

  @override
  String get favourites => 'Favourites';

  @override
  String get everyone => 'Everyone';

  @override
  String get onACall => 'On a call';

  @override
  String get presenceAvailable => 'Available';

  @override
  String get presenceAway => 'Away';

  @override
  String get presenceBusy => 'Busy';

  @override
  String get presenceDnd => 'Do not disturb';

  @override
  String get presenceOffline => 'Offline';

  @override
  String get favouriteAdd => 'Add to favourites';

  @override
  String get favouriteRemove => 'Remove from favourites';

  @override
  String callPerson(String name) {
    return 'Call $name';
  }

  @override
  String get voicemailEmpty => 'No messages';

  @override
  String get urgent => 'Urgent';

  @override
  String get playButton => 'Play';

  @override
  String get stopButton => 'Stop';

  @override
  String get markHeardButton => 'Mark heard';

  @override
  String get deleteButton => 'Delete';

  @override
  String deleteMessageConfirm(String name) {
    return 'Delete the message from $name?';
  }

  @override
  String get settingsCalls => 'Calls';

  @override
  String get dndSubtitle => 'Calls go to voicemail, or are refused';

  @override
  String get callWaiting => 'Call waiting';

  @override
  String get callWaitingSubtitle => 'Ring with a second call while one is up';

  @override
  String get settingsForwarding => 'Forwarding';

  @override
  String get forwardAll => 'Always';

  @override
  String get forwardBusy => 'When busy';

  @override
  String get forwardNoAnswer => 'When not answered';

  @override
  String get forwardUnreachable => 'When unreachable';

  @override
  String get forwardOff => 'Off';

  @override
  String get forwardTo => 'Forward to';

  @override
  String get forwardRingSeconds => 'Ring for (seconds)';

  @override
  String get saveButton => 'Save';

  @override
  String get settingsAudio => 'Audio';

  @override
  String get microphone => 'Microphone';

  @override
  String get speaker => 'Speaker';

  @override
  String get routeEarpiece => 'Phone';

  @override
  String get routeSpeaker => 'Speaker';

  @override
  String get routeBluetooth => 'Bluetooth';

  @override
  String get routeWired => 'Headset';

  @override
  String get systemDefault => 'System default';

  @override
  String get audioNote => 'A call that is up moves to the devices chosen';

  @override
  String get microphoneVolume => 'Microphone volume';

  @override
  String get speakerVolume => 'Speaker volume';

  @override
  String get audioButton => 'Audio';

  @override
  String get ringOn => 'Ring on';

  @override
  String get ringtone => 'Ringtone';

  @override
  String get ringtoneAnvil => 'Anvil';

  @override
  String get ringtoneClassic => 'Classic';

  @override
  String get ringtoneFile => 'Choose a file…';

  @override
  String get ringtoneFileType => 'Sound files';

  @override
  String get ringtoneRemove => 'Remove this ringtone';

  @override
  String get audioTests => 'Test your audio';

  @override
  String get testMicrophone => 'Test microphone';

  @override
  String get testMicrophoneNote =>
      'Say something for five seconds, then hear it played back on your speaker at your volumes.';

  @override
  String get testSpeaker => 'Test speaker';

  @override
  String get testRinging => 'Test ringing';

  @override
  String get testRecording => 'Listening… speak now';

  @override
  String get testPlayingBack => 'Playing back what you said';

  @override
  String get testSpeakerPlaying => 'Playing on your speaker';

  @override
  String get testRingPlaying => 'Ringing on the ringing speaker';

  @override
  String get testStop => 'Stop';

  @override
  String get echoCancellation => 'Echo cancellation';

  @override
  String get echoCancellationNote =>
      'Stops the other person hearing themselves through your speakers. Turn it off only with a headset, if your voice sounds clipped.';

  @override
  String get noiseSuppression => 'Noise suppression';

  @override
  String get noiseSuppressionNote =>
      'Turns down steady noise around you, such as fans and hum.';

  @override
  String get settingsAccount => 'Account';

  @override
  String signedInAs(String user, String server) {
    return '$user on $server';
  }

  @override
  String get needsFcp => 'Needs a phone signed in to FCP';

  @override
  String trayShow(String app) {
    return 'Show $app';
  }

  @override
  String get trayQuit => 'Quit';

  @override
  String get settingsDesktop => 'This computer';

  @override
  String get startAtLogin => 'Start at login';

  @override
  String get startAtLoginSubtitle =>
      'Ready for calls when you sign in to the computer';

  @override
  String updateAvailable(String version) {
    return 'Anvil $version is available';
  }

  @override
  String get updateNow => 'Update and restart';

  @override
  String get updateGetIt => 'Download';

  @override
  String get updateLater => 'Later';

  @override
  String get updateAfterCall => 'After the call';

  @override
  String get updateDownloading => 'Downloading the update…';

  @override
  String get updateChecking => 'Checking for updates…';

  @override
  String get updateNone => 'Anvil is up to date';

  @override
  String updateFailed(String reason) {
    return 'Could not update: $reason';
  }

  @override
  String get settingsUpdates => 'Updates';

  @override
  String versionLabel(String version) {
    return 'Anvil $version';
  }

  @override
  String get checkForUpdates => 'Check for updates';

  @override
  String qualityPackets(int received, int played, int dropped) {
    return '$received packets received, $played played, $dropped dropped';
  }

  @override
  String qualityUnexpectedPt(int pt, int expected) {
    return 'Payload type $pt arrived; this call\'s codec is $expected';
  }

  @override
  String qualityFrom(String source) {
    return 'From $source';
  }

  @override
  String get settingsTroubleshooting => 'Troubleshooting';

  @override
  String get signInServerLocked => 'Set by your organisation';

  @override
  String get managedBy => 'Set up by your organisation';

  @override
  String get updateManaged => 'Your organisation installs updates to this app.';

  @override
  String get logFile => 'Log file';

  @override
  String get logFileCopy => 'Copy the path';

  @override
  String get logFileCopied => 'Path copied';

  @override
  String get logFileOpenFolder => 'Open the folder';

  @override
  String get recordButton => 'Record';

  @override
  String get stopRecordingButton => 'Stop recording';

  @override
  String get recordingNow => 'Recording';

  @override
  String get moreButton => 'More';

  @override
  String get createContactButton => 'Create contact';

  @override
  String get editContactButton => 'Edit';

  @override
  String get contactNewTitle => 'New contact';

  @override
  String get contactEditTitle => 'Edit contact';

  @override
  String get contactNameField => 'Name';

  @override
  String get contactCompanyField => 'Company';

  @override
  String get contactNumberField => 'Number';

  @override
  String get contactLabelField => 'Label';

  @override
  String get labelMobile => 'Mobile';

  @override
  String get labelWork => 'Work';

  @override
  String get labelHome => 'Home';

  @override
  String get labelMain => 'Main';

  @override
  String get labelWorkFax => 'Work fax';

  @override
  String get labelOther => 'Other';

  @override
  String get labelCustom => 'Custom…';

  @override
  String get labelBackToList => 'Choose from the list';

  @override
  String get directoryTab => 'Directory';

  @override
  String get contactsEmpty => 'No contacts yet. Add one with the button above.';

  @override
  String get directoryEmpty => 'No one to show';

  @override
  String contactNumbers(int count) {
    return '$count numbers';
  }

  @override
  String get contactEmailField => 'Email';

  @override
  String get contactNotesField => 'Notes';

  @override
  String get contactNeedsName => 'Enter a name';

  @override
  String get addNumberButton => 'Add a number';

  @override
  String get removeNumberButton => 'Remove this number';

  @override
  String get contactsHeading => 'My contacts';

  @override
  String get contactOnDevice => 'On this device';

  @override
  String get contactOnAccount => 'On your account';

  @override
  String get contactPlaceAccount => 'My account';

  @override
  String get contactPlaceDevice => 'This device';

  @override
  String get moveContactToAccountButton => 'Move to my account';

  @override
  String deleteContactConfirm(String name) {
    return 'Delete $name from your contacts?';
  }

  @override
  String deleteAccountContactConfirm(String name) {
    return 'Delete $name from your contacts? They go from every app you sign in to.';
  }
}
