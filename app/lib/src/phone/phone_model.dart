// What the screens watch: who is signed in, the phone's snapshot, and the
// last thing worth telling the user.

import 'dart:async';

import 'package:flutter/foundation.dart';

import '../mobile/platform.dart';
import 'phone_api.dart';

/// Something to tell the user, and what kind of thing it is, so a screen can
/// say it in the user's language.
class Notice {
  const Notice(this.kind, this.text);

  /// `call_ended`, `transfer`, `microphone` or `refused`.
  final String kind;
  final String text;
}

class PhoneModel extends ChangeNotifier {
  PhoneModel(this.api, {this.mobile});

  final PhoneApi api;

  /// The phone around the app on Android and iOS, asked for the
  /// microphone before the phone starts.
  final MobilePlatform? mobile;
  SignedInAccount? account;
  PhoneSnapshot snapshot = PhoneSnapshot.empty;
  bool busy = false;

  /// Waiting for the system browser to come back from a sign-in.
  bool browserSignIn = false;

  /// The last thing to tell the user (a call ended, a command refused).
  Notice? notice;

  /// An attended transfer under way: the call being transferred, held
  /// while the user consults on a second call.
  int? consulting;

  /// A number for the keypad from outside the app (a tel: or sip: link);
  /// the home screen takes it.
  final dialRequest = ValueNotifier<String?>(null);

  StreamSubscription<PhoneEvent>? _events;

  /// A saved session starts the phone at once.
  Future<void> boot() async {
    account = await api.savedAccount();
    if (account != null) await _start();
    notifyListeners();
  }

  /// How `username` signs in on the server (`place`).
  Future<SignInMethods> signInMethods({
    required String place,
    required String username,
  }) async {
    busy = true;
    notifyListeners();
    try {
      return await api.signInMethods(place: place, username: username);
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  /// Sign in with a password: true when signed in, false when the server
  /// wants a one-time code next ([signInWithCode]).
  Future<bool> signIn({
    required String place,
    required String username,
    required String password,
  }) async {
    busy = true;
    notice = null;
    notifyListeners();
    try {
      final signedIn = await api.signIn(
        place: place,
        username: username,
        password: password,
      );
      if (signedIn == null) return false;
      account = signedIn;
      await _start();
      return true;
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  /// Finish a password sign-in with the one-time code it asked for.
  Future<void> signInWithCode(String code) async {
    busy = true;
    notifyListeners();
    try {
      account = await api.signInWithCode(code);
      await _start();
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  /// Sign in through the system browser, as single sign-on does: `open`
  /// shows the address (the system browser); the sign-in finishes when the
  /// browser comes back, or fails when it is cancelled.
  Future<void> signInWithBrowser({
    required String place,
    String username = '',
    required Future<void> Function(String url) open,
  }) async {
    busy = true;
    browserSignIn = true;
    notice = null;
    notifyListeners();
    try {
      final url = await api.startBrowserSignIn(
        place: place,
        username: username,
      );
      await open(url);
      account = await api.finishBrowserSignIn();
      await _start();
    } finally {
      busy = false;
      browserSignIn = false;
      notifyListeners();
    }
  }

  /// Give up waiting for the browser.
  Future<void> cancelBrowserSignIn() => api.cancelBrowserSignIn();

  Future<void> signOut() async {
    _stopEvents();
    await api.signOut();
    account = null;
    consulting = null;
    snapshot = PhoneSnapshot.empty;
    notifyListeners();
  }

  /// Stop the phone before the app goes: it unregisters, its calls end.
  Future<void> stop() async {
    _stopEvents();
    try {
      await api.stop();
    } catch (_) {
      // Going anyway.
    }
  }

  Future<void> _start() async {
    // Without the microphone the phone still rings; the user is told why
    // nobody hears them.
    if (mobile != null) {
      if (!await mobile!.requestMicrophone()) {
        notice = const Notice('microphone', '');
      }
      await mobile!.startAudio();
    }
    await api.start();
    snapshot = api.snapshot();
    _stopEvents();
    _events = api.events().listen((event) {
      snapshot = api.snapshot();
      switch (event.kind) {
        case 'call_ended':
          if (event.reason != null) {
            notice = Notice('call_ended', event.reason!);
          }
          if (consulting != null &&
              !snapshot.calls.any((c) => c.id == consulting)) {
            consulting = null;
          }
        case 'transfer_progress':
          if (event.reason != null) notice = Notice('transfer', event.reason!);
      }
      notifyListeners();
    });
  }

  /// Run a command; a refusal becomes the notice.
  Future<void> run(Future<void> Function() command) async {
    try {
      await command();
      notice = null;
    } catch (e) {
      notice = Notice('refused', _reason(e));
    }
    snapshot = api.snapshot();
    notifyListeners();
  }

  /// Consult before transferring: `call` is held and `target` called.
  Future<void> consult(int call, String target) => run(() async {
    await api.call(target);
    consulting = call;
  });

  /// Join the held call's party to the one consulted, ending both of ours.
  Future<void> completeTransfer(int consulted) => run(() async {
    final held = consulting;
    if (held == null) return;
    await api.transferAttended(held, consulted);
    consulting = null;
  });

  /// Stop following the phone's changes. Not awaited: a cancelled
  /// subscription's future may not complete until its stream closes.
  void _stopEvents() {
    unawaited(_events?.cancel());
    _events = null;
  }

  void clearNotice() {
    notice = null;
    notifyListeners();
  }

  /// What an error says, without the wrapping a Dart or Rust error adds.
  static String reason(Object e) => _reason(e);

  static String _reason(Object e) {
    final s = e.toString();
    // `Exception: …` and flutter_rust_bridge's `AnyhowException(…)` read
    // better as what they say.
    if (s.startsWith('Exception: ')) return s.substring('Exception: '.length);
    final m = RegExp(r'^AnyhowException\((.*)\)$', dotAll: true).firstMatch(s);
    return m?.group(1) ?? s;
  }

  bool _disposed = false;

  /// A boot, a sign-in or a command may finish after the app is gone (its
  /// window closed, a test over); what it would have told nobody is dropped.
  @override
  void notifyListeners() {
    if (!_disposed) super.notifyListeners();
  }

  @override
  void dispose() {
    _disposed = true;
    _stopEvents();
    dialRequest.dispose();
    super.dispose();
  }
}
