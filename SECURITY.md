# Security policy

## Reporting a vulnerability

Please report security vulnerabilities privately, through GitHub's
[private vulnerability reporting](https://github.com/feryx-io/anvil/security/advisories/new)
(the repository's **Security** tab → **Report a vulnerability**). Don't open
a public issue, pull request or discussion for a vulnerability.

A useful report says:

- what is affected: the platform (Linux, macOS, Windows, Android, iOS), the
  app or the CLI, and the release or commit;
- what an attacker can do, and from where (a SIP server or peer, a call's
  media, a link, an update, another app on the device);
- how to reproduce it;
- anything you know about a fix.

The report stays private between you and the maintainers until it is fixed
and published as a security advisory. We'll credit you in the advisory unless
you'd rather not be named.

A vulnerability in the SIP stack ([siphon-rs](https://github.com/feryx-io/siphon-rs)),
the media stack ([forge-media](https://github.com/feryx-io/forge-media)) or
FCP ([fcp](https://github.com/feryx-io/fcp)) can be reported in that
repository, or here if you found it through Anvil.

## Supported versions

Only the latest release is supported.
