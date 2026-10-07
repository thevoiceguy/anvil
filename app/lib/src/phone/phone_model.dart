// What the screens watch: who is signed in, the phone's snapshot, and the
// last thing worth telling the user.

import 'dart:async';

import 'package:flutter/foundation.dart';

import 'phone_api.dart';

class PhoneModel extends ChangeNotifier {
  PhoneModel(this.api);

  final PhoneApi api;
  SignedInAccount? account;
  PhoneSnapshot snapshot = PhoneSnapshot.empty;
  bool busy = false;

  /// The last thing to tell the user (a call ended, a command refused).
  String? notice;

  StreamSubscription<PhoneEvent>? _events;

  /// A saved session starts the phone at once.
  Future<void> boot() async {
    account = await api.savedAccount();
    if (account != null) await _start();
    notifyListeners();
  }

  Future<void> signIn({
    required String place,
    required String username,
    required String password,
    String? code,
  }) async {
    busy = true;
    notice = null;
    notifyListeners();
    try {
      account = await api.signIn(
        place: place,
        username: username,
        password: password,
        code: code,
      );
      await _start();
    } finally {
      busy = false;
      notifyListeners();
    }
  }

  Future<void> signOut() async {
    await _events?.cancel();
    await api.signOut();
    account = null;
    snapshot = PhoneSnapshot.empty;
    notifyListeners();
  }

  Future<void> _start() async {
    await api.start();
    snapshot = api.snapshot();
    await _events?.cancel();
    _events = api.events().listen((event) {
      snapshot = api.snapshot();
      if (event.kind == 'call_ended' && event.reason != null) {
        notice = event.reason;
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
      notice = e.toString();
    }
    snapshot = api.snapshot();
    notifyListeners();
  }

  @override
  void dispose() {
    _events?.cancel();
    super.dispose();
  }
}
