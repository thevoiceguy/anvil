# Anvil — managed installs and branded builds

Designed 2026-10-24 from D8's findings on FCP (`thevoiceguy/fcp`,
`docs/DEEP_TESTING.md`). The questions are in §7; nothing is agreed yet.
The findings this answers:

- **8c.** The server address should be set without the user, so the app
  can be deployed with nobody typing it.
- **12.** A brand should rename the app's icon, not only its screens.
- **13.** A brand should change the icon itself.
- **14.** A company should be able to deploy the app with its brand already
  in place: the icon, the images, the name.

## 1. What there is

- **Sign-in** asks for a server (or an email address) and a username. The
  last ones used are remembered in `last_sign_in.json`. Nothing else can
  fill them in. No setting, file or command-line option sets the server,
  and the person can always change it.
- **The brand** comes from FCP after sign-in (`/brand/v1`, `docs/BRANDING.md`):
  the name, colors, logo, icon and ringtones, cached by tenant.
  - The app uses the name for the window's title and the tray (v0.2.5).
  - It uses the colors for its theme and the logo in the header.
  - It fetches and caches the icon but never uses it.
  - Before the first sign-in, the app is plain Anvil.
- **The Windows installer** (`app/packaging/windows.iss`) hard-codes the name
  "Anvil":
  - the Start menu and desktop shortcuts are named "Anvil";
  - the icon is the one built into `anvil.exe`;
  - the notifications' app id is `Anvil.Softphone`;
  - the AppId is fixed, so there is one Anvil per Windows account.
  - It takes no options of its own; it does take Inno Setup's standard ones
    (`/VERYSILENT`, `/DIR` is disabled).
- **Updates** come from one feed, the latest GitHub release's `latest.json`,
  signed with Anvil's Ed25519 key (`crates/anvil-update`).
  - Windows runs the new setup silently over the old one.
  - An AppImage replaces itself.
  - macOS and the `.deb` offer a download instead.
- **macOS** (`com.thevoiceguy.anvil`) and **Linux** (`.deb` in `/opt/anvil`,
  an AppImage) also carry the name and icon in the package.
- **Mobile** apps get their name and icon at build time, under one store
  listing.

The data directory (`Anvil\Anvil` on Windows) holds the session, settings
and the brand cache. People never see it, so it keeps its name.

## 2. The model

### 2.1 A profile

A **profile** is a directory, or a `.zip` of one, holding `profile.json`
and the files it names:

```json
{
  "profile_version": 1,
  "server": "https://voice.acme.example:8444",
  "lock_server": true,
  "login_domain": "acme.example",
  "sign_in": "auto",
  "username_from_system": true,
  "updates": "auto",
  "brand": {
    "app_name": "Acme Voice",
    "colors": { "primary": "#1A73E8", "accent": "#34A853" },
    "dark_colors": { "primary": "#8AB4F8" },
    "icon": "icon.png",
    "logo": "logo.svg",
    "logo_dark": "logo-dark.svg",
    "ringtone": "ring.ogg"
  }
}
```

Every field is optional.

- **`server`**: what the sign-in screen's server field holds. It is the
  same thing a person types today: a URL, a host, or nothing when
  `login_domain` finds the server.
- **`lock_server`**: the field is shown but cannot be edited. Defaults to
  true when `server` is set.
- **`login_domain`**: appended to a bare username (`james` becomes
  `james@acme.example`). FCP finds the tenant from an email address's
  domain (`/auth/discover`), so this is what makes one shared address work
  for every tenant (D8 finding 8a). Without it, a username that exists in
  two tenants is refused.
- **`sign_in`**:
  - `auto`: ask FCP, as today;
  - `sso`: straight to the browser;
  - `password`: the password form.
- **`username_from_system`**: fill the username in from the system on
  Windows, using the user principal name (`james@acme.example`, from
  `GetUserNameExW(NameUserPrincipal)`). On a domain-joined PC with
  `sign_in: "sso"`, the person only presses Continue.
- **`updates`**:
  - `auto`: as today;
  - `notify`: offer the update without installing it;
  - `off`: the company deploys versions itself.
- **`brand`**: the shape of FCP's `/brand/v1`, with file names in place of
  URLs. It applies before anyone signs in, and is the brand of an app that
  never gets one from FCP.

The profile describes the install, not a person. It holds no username,
password or token.

### 2.2 Where a setting comes from

The highest source wins, field by field:

1. **Policy**, set by IT and not writable by the user:
   - **Windows:** `HKLM\Software\Policies\Anvil`, then
     `HKCU\Software\Policies\Anvil`. The values are named after the
     fields (`Server`, `LockServer`, `LoginDomain`, `SignIn`,
     `UsernameFromSystem`, `Updates`), so Group Policy and Intune can
     write them with no file to deploy.
   - **macOS:** managed preferences for `com.thevoiceguy.anvil`, from a
     configuration profile.
   - **Linux:** `/etc/anvil/policy.json`.
   - Policy never carries a brand: that is the profile's job.
2. **The installed profile:**
   - Windows: `{app}\profile\`;
   - macOS: `/Library/Application Support/Anvil/profile/`, outside the
     bundle so the bundle's signature still holds;
   - Linux: `/etc/anvil/profile/`, or beside the AppImage.
3. **What the person typed last** (`last_sign_in.json`), for the fields the
   first two leave open.

Once signed in, **FCP's brand wins over the profile's**: a tenant's edit
is live, as it is today. The profile's brand is what shows before that,
and what stays if FCP has none (Q2).

### 2.3 One installer, given a profile

The same stock installer serves every company. It takes a profile and
keeps it:

- **On the command line:**
  - `Anvil-…-setup.exe /PROFILE=<dir or .zip or https URL>`;
  - or the single fields: `/SERVER=`, `/LOGINDOMAIN=`, `/SIGNIN=`,
    `/LOCKSERVER=`.
  - Intune or a script runs it with `/VERYSILENT`.
- **On install:**
  - the profile is copied to `{app}\profile\`, and the icon is converted
    to `.ico` there;
  - the Start menu and desktop shortcuts take the profile's `app_name`
    and icon (Inno's `[Code]` reads `profile.json`);
  - the notifications' app id is registered with the same name and
    icon, so a call's notification says "Acme Voice";
  - the uninstaller removes all of it.
- **A self-update** runs the stock setup again, without `/PROFILE`. The
  setup finds the profile already in `{app}\profile\` and names the
  shortcuts from it again, so an update never turns Acme Voice back into
  Anvil.
- **A branded setup** (`app/packaging/brand.py <profile> <stock setup>`) is
  the stock setup built with the profile inside it (`iscc /DProfile=…`),
  named after the brand (`Acme-Voice-0.3.0-setup.exe`). It is for a company
  that wants one file to hand out. It updates from the same feed as every
  other copy.

The exe's own embedded icon stays Anvil's. It is only seen when browsing
the install folder: the shortcuts, the taskbar (through the shortcut's app
id), the window, the tray and the notifications all show the brand.

### 2.4 The brand on the system, at run time (findings 12, 13)

When a brand arrives, from the profile or from FCP, and its name or icon
differs from what the system shows, Anvil changes:

- **Every desktop:** the window's icon, the tray (Q5) and the window's
  title.
- **Windows:**
  - the shortcuts Anvil's installer made: renamed, and pointed at
    `%LOCALAPPDATA%\Anvil\Anvil\brand\icon.ico`;
  - the app id's display name and icon.
  - The installer records which shortcuts it made, in
    `{app}\profile\shortcuts.json`, so Anvil never touches one it didn't
    make.
- **Linux:** a user-level `.desktop` file with the same id, in
  `~/.local/share/applications`, holding the name and icon. It overrides
  the packaged one.
- **macOS:** the Dock icon while the app runs
  (`NSApplication.applicationIconImage`). The name in Finder and the
  Applications folder is the bundle's, fixed at build time, so on macOS
  only a branded build (§2.5) renames the app.
- **Mobile:** nothing at run time. The name and icon are the store
  build's.

### 2.5 Branded builds where the package carries the name

- **macOS:** `brand.py` also rewrites a built `Anvil.app`:
  - `CFBundleName` and `CFBundleDisplayName`;
  - the `.icns` made from the profile's icon;
  - the profile placed in `/Library/Application Support/Anvil/profile/`
    by a `.pkg`.
  - The app stays unsigned until U3c (code signing), when the rewrite is
    re-signed with the same certificate.
- **Linux:** a `.deb` built with the profile in `/etc/anvil/profile/` and
  its `.desktop` named from it.
- **Mobile white-label apps** (their own store listing, push keys and
  name) are a separate project. They are not in this design (§6).

### 2.6 FCP's part

FCP already holds most of a profile: its public URL, the tenant's login
domains and sign-in methods, and the brand. A new route,
`GET /api/v1/branding/deployment-profile` (under `branding_manage`),
returns the tenant's profile as a `.zip`. The admin UI's Branding page
gets a **Softphone deployment** section with:

- the download;
- the Intune command line:
  `Anvil-…-setup.exe /VERYSILENT /PROFILE=acme-profile.zip`;
- the `.reg` file for the policy keys, as an alternative.

## 3. Security

- **Who can write a profile:**
  - A per-user Windows install is user-writable, so a profile in
    `{app}\profile\` is too. Anyone who can write there can also replace
    `anvil.exe`, so a profile adds no new risk.
  - Policy keys under `HKLM` need an administrator, which is why policy
    beats the profile.
- **What a profile can do:** point the app at a server. It cannot sign
  anyone in, and the server's certificate is still checked.
  - A company whose server needs its own CA ships the CA as policy
    (`CaPem`), not in the profile, so a profile alone cannot add trust.
- **The username from the system** is only a suggestion in a field. Sign-in
  is unchanged.
- **Unsigned profiles** (Q7): a signature would tell the app who made a
  profile, but not whether to trust them. Policy is the trust boundary.

## 4. Phases

- **M1, the profile and policy** (`anvil-app`, every desktop):
  - read `profile.json` and the policy sources;
  - the sign-in screen: server and lock, `login_domain`, `sign_in`, the
    username from the system;
  - the `updates` setting in `anvil-update`;
  - the profile's brand before sign-in, and FCP's after it.
  - Tests: precedence field by field, a locked field, each sign-in mode,
    `updates: off` never fetching.
- **M2, the Windows installer:**
  - `/PROFILE` and the single-field options;
  - `{app}\profile\`, shortcuts and the app id named from it, kept across
    a silent update;
  - `brand.py` for a branded setup.
  - Proved on the Windows runner: a silent install with a profile, the
    shortcuts' names and icons, a self-update to a stock setup keeping
    them, and uninstall removing them.
- **M3, the brand on the system at run time:**
  - the window and tray icons;
  - Windows shortcuts and the app id;
  - the Linux `.desktop` override;
  - the macOS Dock icon.
  - A brand change from FCP renames and re-icons the shortcuts, and
    removing the brand restores Anvil's.
- **M4, macOS and Linux packages:**
  - the profile in `/Library/Application Support` and `/etc/anvil`, and
    managed preferences;
  - `brand.py` producing a renamed `.app` and `.pkg`, and a branded
    `.deb`.
- **F1 (in FCP):** the deployment profile route, the Branding page's
  section, and `docs/BRANDING.md`.

M1 can ship alone, and is what finding 8c needs. M2 and F1 together make
"deploy with no one typing anything" true on Windows.

## 5. How it is proved

- **Unit tests:** the precedence rules, the profile's parser (unknown
  fields kept for a newer profile version and ignored, a missing file
  dropped like a failed asset), and the ICO writer (PNG-in-ICO at 16, 32,
  48 and 256).
- **Widget tests:** a locked server field, sign-in modes, and the
  profile's brand before sign-in.
- **The Windows job:** install, inspect the `.lnk` files and the
  registry, update, uninstall.
- **By hand on D8:** an Intune-style silent install on the test PC with a
  profile downloaded from D8's admin; sign-in with the domain user's UPN;
  a brand edit in FCP renaming the Start menu entry.

## 6. Not in this design

- **A "SIP account" sign-in** for servers other than FCP (D8 finding 9). A
  profile would later carry that account type too.
- **Mobile white-label builds.** The server preset on mobile comes from
  managed app configuration (iOS AppConfig, Android managed
  configurations). That is a small later phase once the mobile apps ship.
- **Two brands on one machine** (Q9).

## 7. Questions

1. **One installer with a profile, or a build per company?** Recommended:
   one stock installer that takes a profile, plus `brand.py` packing a
   profile into it for those who want one file. Every copy then updates
   from one feed, and a company's brand survives updates. A build per
   company would need a feed and a signing key per company.
2. **The profile's brand or FCP's, once signed in?** Recommended: FCP's,
   because the tenant's edits are live. The profile's applies before
   sign-in and when FCP has no brand.
3. **Is the server locked when a profile sets it?** Recommended: yes by
   default (`lock_server` defaults to true), and always when policy sets it.
4. **Rename the shortcuts when FCP's `app_name` changes?** Recommended: yes
   on Windows and Linux, only for the shortcuts Anvil's installer made.
5. **The tray icon under a brand?** Recommended: the brand's icon with a
   small state dot (ready, do not disturb, ringing, in a call, offline),
   drawn at run time. Without a brand, Anvil's own state icons as today.
6. **Updates under management?** Recommended: `auto`, `notify` and `off`,
   settable by policy, so IT can hold a version.
7. **Signed profiles?** Recommended: no. Policy is the trust boundary, and
   a profile cannot add a CA.
8. **The username from the system?** Recommended: yes, on Windows when the
   profile or policy asks, as a pre-filled field.
9. **Two brands on one machine?** Recommended: no. One Anvil per user
   account, as the fixed AppId already makes it.
10. **Order?** Recommended: M1, then M2 with F1, then M3, then M4.
