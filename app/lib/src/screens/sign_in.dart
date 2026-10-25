import 'package:flutter/material.dart';
import 'package:url_launcher/url_launcher.dart';

import '../../l10n/app_localizations.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';

/// Opens an address in the system browser.
Future<void> openInBrowser(String url) async {
  if (!await launchUrl(Uri.parse(url), mode: LaunchMode.externalApplication)) {
    throw Exception('no browser opened $url');
  }
}

/// Where a sign-in stands: who and where first, then whatever the server
/// says that person signs in with, then a one-time code if it asks.
enum _Step { identify, password, sso, code }

class SignInScreen extends StatefulWidget {
  const SignInScreen({
    super.key,
    required this.model,
    this.openUrl = openInBrowser,
  });
  final PhoneModel model;

  /// How a sign-in through the browser opens it (the tests watch it).
  final Future<void> Function(String url) openUrl;

  @override
  State<SignInScreen> createState() => _SignInScreenState();
}

class _SignInScreenState extends State<SignInScreen> {
  final _server = TextEditingController();
  final _user = TextEditingController();
  final _password = TextEditingController();
  final _code = TextEditingController();
  _Step _step = _Step.identify;

  /// Single sign-on is offered beside the password.
  bool _sso = false;
  String? _error;

  @override
  void initState() {
    super.initState();
    _fillIn();
  }

  Managed get _managed => widget.model.managed;

  /// Filled in with what the install was set up with, then the last server
  /// and username signed in with (a locked server stays the install's).
  Future<void> _fillIn() async {
    final managed = _managed;
    _server.text = managed.server ?? '';
    _user.text = managed.username ?? '';
    RememberedSignIn? last;
    try {
      last = await widget.model.api.lastSignIn();
    } catch (_) {
      return;
    }
    if (last == null || !mounted) return;
    final remembered = last;
    setState(() {
      if (!managed.lockServer && remembered.place.isNotEmpty) {
        _server.text = remembered.place;
      }
      if (remembered.username.isNotEmpty) _user.text = remembered.username;
    });
  }

  @override
  void dispose() {
    _server.dispose();
    _user.dispose();
    _password.dispose();
    _code.dispose();
    super.dispose();
  }

  String get _place => _server.text.trim();

  /// The username (with the install's login domain after a bare one), or
  /// the email address typed as the server.
  String get _username {
    final user = _managed.login(_user.text);
    if (user.isEmpty && _place.contains('@') && !_place.contains('://')) {
      return _place;
    }
    return user;
  }

  bool get _canUseBrowser => widget.model.api.canSignInWithBrowser;

  void _fail(Object e) {
    if (!mounted) return;
    final s = Strings.of(context);
    setState(() => _error = s.signInFailed(PhoneModel.reason(e)));
  }

  /// Ask the server how this person signs in, and go on to that.
  Future<void> _continue() async {
    final s = Strings.of(context);
    if (_place.isEmpty && !_username.contains('@')) {
      setState(() => _error = s.signInServerNeeded);
      return;
    }
    if (_username.isEmpty) {
      setState(() => _error = s.signInUsernameNeeded);
      return;
    }
    setState(() => _error = null);
    // The install says how its people sign in: no need to ask.
    switch (_managed.signIn) {
      case ManagedSignIn.password:
        setState(() {
          _sso = false;
          _password.clear();
          _step = _Step.password;
        });
        return;
      case ManagedSignIn.sso when _canUseBrowser:
        setState(() => _step = _Step.sso);
        await _browser();
        return;
      case ManagedSignIn.sso || ManagedSignIn.auto:
        break;
    }
    final SignInMethods methods;
    try {
      methods = await widget.model.signInMethods(
        place: _place,
        username: _username,
      );
    } catch (e) {
      _fail(e);
      return;
    }
    if (!mounted) return;
    if (methods.password) {
      setState(() {
        _sso = methods.sso && _canUseBrowser;
        _password.clear();
        _step = _Step.password;
      });
    } else if (methods.sso && _canUseBrowser) {
      setState(() => _step = _Step.sso);
      await _browser();
    } else {
      setState(() => _error = s.signInSsoUnavailable);
    }
  }

  Future<void> _submitPassword() async {
    setState(() => _error = null);
    try {
      final signedIn = await widget.model.signIn(
        place: _place,
        username: _username,
        password: _password.text,
      );
      if (!signedIn && mounted) {
        setState(() {
          _code.clear();
          _step = _Step.code;
        });
      }
    } catch (e) {
      _fail(e);
    }
  }

  Future<void> _submitCode() async {
    setState(() => _error = null);
    try {
      await widget.model.signInWithCode(_code.text.trim());
    } catch (e) {
      _fail(e);
    }
  }

  /// Sign in through the browser: single sign-on, whatever the server's
  /// sign-in page asks.
  Future<void> _browser() async {
    setState(() => _error = null);
    try {
      await widget.model.signInWithBrowser(
        place: _place,
        username: _username,
        open: widget.openUrl,
      );
    } catch (e) {
      _fail(e);
    }
  }

  /// Back to who and where.
  void _change() {
    setState(() {
      _error = null;
      _password.clear();
      _code.clear();
      _step = _Step.identify;
    });
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    return Scaffold(
      body: Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 380),
          child: ListView(
            shrinkWrap: true,
            padding: const EdgeInsets.all(24),
            children: [
              if (widget.model.brandLogo case final logo?)
                Padding(
                  padding: const EdgeInsets.only(bottom: 12),
                  child: SizedBox(
                    height: 64,
                    child: Image.memory(
                      logo,
                      key: const Key('signInLogo'),
                      errorBuilder: (_, _, _) => const SizedBox.shrink(),
                    ),
                  ),
                ),
              Text(
                widget.model.brandName ?? s.appName,
                key: const Key('signInTitle'),
                style: Theme.of(context).textTheme.displaySmall,
                textAlign: TextAlign.center,
              ),
              const SizedBox(height: 8),
              Text(
                s.signInTitle,
                style: Theme.of(context).textTheme.titleMedium,
                textAlign: TextAlign.center,
              ),
              const SizedBox(height: 24),
              ...switch (_step) {
                _Step.identify => _identify(s),
                _Step.password => _passwordStep(s),
                _Step.sso => _ssoStep(s),
                _Step.code => _codeStep(s),
              },
              if (_error != null) ...[
                const SizedBox(height: 12),
                Text(
                  _error!,
                  key: const Key('signInError'),
                  style: TextStyle(color: Theme.of(context).colorScheme.error),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }

  List<Widget> _identify(Strings s) {
    final busy = widget.model.busy;
    return [
      TextField(
        key: const Key('server'),
        controller: _server,
        keyboardType: TextInputType.url,
        autocorrect: false,
        readOnly: _managed.lockServer,
        enabled: !_managed.lockServer,
        decoration: InputDecoration(
          labelText: s.signInServer,
          hintText: s.signInServerHint,
          helperText: _managed.lockServer ? s.signInServerLocked : null,
        ),
      ),
      const SizedBox(height: 12),
      TextField(
        key: const Key('username'),
        controller: _user,
        autocorrect: false,
        decoration: InputDecoration(
          labelText: s.signInUsername,
          suffixText: _managed.loginDomain != null && !_user.text.contains('@')
              ? '@${_managed.loginDomain}'
              : null,
        ),
        onChanged: (_) => setState(() {}),
        onSubmitted: (_) {
          if (!busy) _continue();
        },
      ),
      const SizedBox(height: 20),
      FilledButton(
        key: const Key('continue'),
        onPressed: busy ? null : _continue,
        child: Text(s.signInContinue),
      ),
    ];
  }

  /// Who is signing in where, and a way back to change it.
  Widget _who(Strings s) {
    return Row(
      children: [
        Expanded(
          child: Text(
            _place.isEmpty || _place == _username
                ? _username
                : s.signedInAs(_username, _place),
            key: const Key('signingInAs'),
            overflow: TextOverflow.ellipsis,
          ),
        ),
        TextButton(
          key: const Key('changeAccount'),
          onPressed: widget.model.busy ? null : _change,
          child: Text(s.signInChange),
        ),
      ],
    );
  }

  List<Widget> _passwordStep(Strings s) {
    final busy = widget.model.busy;
    return [
      _who(s),
      const SizedBox(height: 12),
      TextField(
        key: const Key('password'),
        controller: _password,
        obscureText: true,
        autofocus: true,
        decoration: InputDecoration(labelText: s.signInPassword),
        onSubmitted: (_) {
          if (!busy) _submitPassword();
        },
      ),
      const SizedBox(height: 20),
      if (widget.model.browserSignIn)
        ..._waiting(s)
      else ...[
        FilledButton(
          key: const Key('signIn'),
          onPressed: busy ? null : _submitPassword,
          child: Text(s.signInButton),
        ),
        if (_sso) ...[
          const SizedBox(height: 4),
          TextButton(
            key: const Key('signInBrowser'),
            onPressed: busy ? null : _browser,
            child: Text(s.signInUseSso),
          ),
        ],
      ],
    ];
  }

  List<Widget> _ssoStep(Strings s) {
    return [
      _who(s),
      const SizedBox(height: 20),
      if (widget.model.browserSignIn)
        ..._waiting(s)
      else
        FilledButton(
          key: const Key('signInBrowser'),
          onPressed: widget.model.busy ? null : _browser,
          child: Text(s.signInBrowserButton),
        ),
    ];
  }

  List<Widget> _codeStep(Strings s) {
    final busy = widget.model.busy;
    return [
      _who(s),
      const SizedBox(height: 12),
      Text(s.signInCodePrompt),
      const SizedBox(height: 12),
      TextField(
        key: const Key('code'),
        controller: _code,
        autofocus: true,
        keyboardType: TextInputType.number,
        autofillHints: const [AutofillHints.oneTimeCode],
        decoration: InputDecoration(labelText: s.signInCode),
        onSubmitted: (_) {
          if (!busy) _submitCode();
        },
      ),
      const SizedBox(height: 20),
      FilledButton(
        key: const Key('verifyCode'),
        onPressed: busy ? null : _submitCode,
        child: Text(s.signInVerify),
      ),
    ];
  }

  List<Widget> _waiting(Strings s) => [
    Text(
      s.signInBrowserWaiting,
      key: const Key('browserWaiting'),
      textAlign: TextAlign.center,
    ),
    TextButton(
      key: const Key('cancelBrowser'),
      onPressed: widget.model.cancelBrowserSignIn,
      child: Text(s.cancelButton),
    ),
  ];
}
