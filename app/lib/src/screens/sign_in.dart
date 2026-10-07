import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../phone/phone_model.dart';

class SignInScreen extends StatefulWidget {
  const SignInScreen({super.key, required this.model});
  final PhoneModel model;

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
                onPressed: widget.model.busy ? null : _submit,
                child: Text(s.signInButton),
              ),
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
