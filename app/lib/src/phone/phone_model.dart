// What the screens watch: who is signed in, the phone's snapshot, and the
// last thing worth telling the user.

import 'dart:async';

import 'package:flutter/foundation.dart';

import 'phone_api.dart';

/// Something to tell the user, and what kind of thing it is, so a screen can
/// say it in the user's language.
class Notice {
  const Notice(this.kind, this.text);

  /// `call_ended`, `transfer` or `refused`.
  final String kind;
  final String text;
}

class PhoneModel extends ChangeNotifier {
  PhoneModel(this.api);

  final PhoneApi api;
  SignedInAccount? account;
  PhoneSnapshot snapshot = PhoneSnapshot.empty;
  bool busy = false;

  /// The last thing to tell the user (a call ended, a command refused).
  Notice? notice;

  /// An attended transfer under way: the call being transferred, held
  /// while the user consults on a second call.
  int? consulting;

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

  static String _reason(Object e) {
    final s = e.toString();
    // `Exception: …` and flutter_rust_bridge's `AnyhowException(…)` read
    // better as what they say.
    if (s.startsWith('Exception: ')) return s.substring('Exception: '.length);
    final m = RegExp(r'^AnyhowException\((.*)\)$', dotAll: true).firstMatch(s);
    return m?.group(1) ?? s;
  }

  @override
  void dispose() {
    _stopEvents();
    super.dispose();
  }
}
