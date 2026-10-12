import 'package:flutter/material.dart';
import 'package:url_launcher/url_launcher.dart';

import '../../l10n/app_localizations.dart';
import '../phone/phone_model.dart';

/// Opens an address in the system browser.
Future<void> openInBrowser(String url) async {
  if (!await launchUrl(Uri.parse(url), mode: LaunchMode.externalApplication)) {
    throw Exception('no browser opened $url');
  }
}

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
  String? _error;

  Future<void> _submit() async {
    final s = Strings.of(context);
    setState(() => _error = null);
    try {
      await widget.model.signIn(
        place: _server.text.trim(),
        username: _user.text.trim(),
        password: _password.text,
        code: _code.text.trim().isEmpty ? null : _code.text.trim(),
      );
    } catch (e) {
      setState(() => _error = s.signInFailed(e.toString()));
    }
  }

  /// Sign in through the browser: single sign-on, a second factor, whatever
  /// the server's sign-in page asks.
  Future<void> _browser() async {
    final s = Strings.of(context);
    final place = _server.text.trim();
    if (place.isEmpty) {
      setState(() => _error = s.signInServerNeeded);
      return;
    }
    setState(() => _error = null);
    try {
      await widget.model.signInWithBrowser(place: place, open: widget.openUrl);
    } catch (e) {
      if (mounted) setState(() => _error = s.signInFailed(e.toString()));
    }
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final model = widget.model;
    return Scaffold(
      body: Center(
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: 380),
          child: ListView(
            shrinkWrap: true,
            padding: const EdgeInsets.all(24),
            children: [
              Text(
                s.appName,
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
              TextField(
                key: const Key('server'),
                controller: _server,
                decoration: InputDecoration(
                  labelText: s.signInServer,
                  hintText: s.signInServerHint,
                ),
              ),
              const SizedBox(height: 12),
              TextField(
                key: const Key('username'),
                controller: _user,
                decoration: InputDecoration(labelText: s.signInUsername),
              ),
              const SizedBox(height: 12),
              TextField(
                key: const Key('password'),
                controller: _password,
                obscureText: true,
                decoration: InputDecoration(labelText: s.signInPassword),
                onSubmitted: (_) => _submit(),
              ),
              const SizedBox(height: 12),
              TextField(
                key: const Key('code'),
                controller: _code,
                decoration: InputDecoration(labelText: s.signInCode),
              ),
              const SizedBox(height: 20),
              FilledButton(
                key: const Key('signIn'),
                onPressed: model.busy ? null : _submit,
                child: Text(s.signInButton),
              ),
              if (model.api.canSignInWithBrowser) ...[
                const SizedBox(height: 8),
                if (model.browserSignIn) ...[
                  Text(
                    s.signInBrowserWaiting,
                    key: const Key('browserWaiting'),
                    textAlign: TextAlign.center,
                  ),
                  TextButton(
                    key: const Key('cancelBrowser'),
                    onPressed: model.cancelBrowserSignIn,
                    child: Text(s.cancelButton),
                  ),
                ] else
                  OutlinedButton(
                    key: const Key('signInBrowser'),
                    onPressed: model.busy ? null : _browser,
                    child: Text(s.signInBrowserButton),
                  ),
              ],
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
}
