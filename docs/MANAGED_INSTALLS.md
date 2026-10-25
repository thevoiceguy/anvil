# Anvil — managed installs and branded builds

Designed 2026-10-24 from D8's findings on FCP (`thevoiceguy/fcp`,
`docs/DEEP_TESTING.md`) and agreed the same day on every recommendation
in §7.

Progress: M1 done (v0.2.6), see §4.1; M2 done (v0.2.7), see §4.2; M3 done
(v0.2.8), see §4.3; M4 done (v0.2.9), see §4.4.
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
   - Linux: an `anvil-profile/` directory beside the AppImage, else
     `/etc/anvil/profile/`.
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

### 4.1 M1 as built (v0.2.6)

- **`anvil_app::managed`:**
  - `load()` reads the platform's sources, and `load_from(Sources)` merges
    them, so tests give their own.
  - Policy goes over the profile field by field. A blank policy value is no
    setting.
  - A server set by policy is always locked. A profile's server is locked
    unless the profile says `"lock_server": false`.
  - `Managed::login` puts the login domain after a bare username.
  - `ANVIL_PROFILE` (a directory) and `ANVIL_POLICY` (a JSON file in
    `/etc/anvil/policy.json`'s shape) go first, for a trial or a test.
- **Policy sources:**
  - Windows: `HKLM`, then `HKCU`, `Software\Policies\Anvil`, with
    `REG_SZ` or `REG_DWORD` values (`managed_windows.rs`).
  - macOS: the user's, then the machine's, managed preferences plist for
    `com.thevoiceguy.anvil`.
  - Linux: `/etc/anvil/policy.json`.
- **The username from the system:** Windows' user principal name, through
  `GetUserNameExW`. Other desktops have none yet.
- **The profile's brand:**
  - Its files are read beside `profile.json` and nowhere else, each up to
    5 MB. A file that is missing is left out.
  - It is `PhoneConfig::fallback_brand`: shown when the phone starts, and
    again when FCP clears its own. It brings its ringtone with it, the
    default one.
  - Before sign-in, the app takes the name, colour and logo from
    `managed_settings()`.
- **The sign-in screen:**
  - The install's server is shown, locked with "Set by your organisation".
    A remembered server never replaces a locked one.
  - The username comes from the system, else the last one used. The login
    domain shows after a bare username.
  - `password` goes straight to the password, and `sso` straight to the
    browser, without asking the server.
- **Updates:**
  - `off`: no update model at all, so no checks and no Settings section.
  - `notify`: the banner says the version and that the organisation
    installs updates, with no button to install. The FFI refuses a
    download.
- **Settings:** Troubleshooting shows "Set up by your organisation" and
  where each setting came from.
- **Not done:** the `CaPem` policy of §3 (a company's CA from policy).
  `AccountConfig.tls_extra_ca_pem` exists for SIP; FCP's HTTPS client
  would need the same.
- **Tests:**
  - `managed.rs`: nothing set, precedence, locking, the login domain, the
    brand's files, a broken profile, named values.
  - `tests/phone.rs`: the profile's brand and ringtone on a phone without
    FCP.
  - `app/test/managed_test.dart`: the locked server, the login domain, each
    sign-in mode, the brand before sign-in, `off` and `notify`.
  - The Windows registry code is type-checked against `windows-sys` for
    `x86_64-pc-windows-msvc`. The macOS code is built by CI's macOS job.

### 4.2 M2 as built (v0.2.7)

- **The setup's options:**
  - `/PROFILE=` takes a directory, a `.zip` (with `profile.json` at the top
    or in its one folder) or an `https` URL of a `.zip`. A relative path is
    taken from the current directory, else from beside the setup, as an
    Intune package runs it.
  - `/SERVER=`, `/LOGINDOMAIN=`, `/SIGNIN=` (`auto`, `sso`, `password`) and
    `/LOCKSERVER=` (`1`, `0`) are written into `profile.json`, over the
    profile given or the one already installed.
  - A new profile replaces the old one whole; fields given before do not
    outlive it.
  - A profile that cannot be fetched or read, or a field with a value it
    cannot take, fails the install before anything changes (exit code 7,
    the reason in the setup's log).
- **`profile.ps1`**, run from `{tmp}` by `PrepareToInstall`, does the work
  Inno's Pascal does badly: it fetches and unzips the profile, writes the
  fields, makes `icon.ico` from the brand's PNG (16, 32, 48 and 256 pixels,
  or the PNG as it is where System.Drawing is missing), and tells the setup
  the brand's name and icon. It needs Windows PowerShell, which every
  supported Windows has.
- **Named from the profile:** the Start menu and desktop shortcuts (name
  and icon), the notifications' app id (`DisplayName`, `IconUri`) and the
  uninstall entry (name and icon). The app registers the app id again at
  every start, so it now passes the profile's name and `icon.ico` too
  (`Managed::system_icon`, `brandIconPath`), or the start would turn it
  back into Anvil.
- **`shortcuts.json`** in `{app}\profile` lists the shortcuts the setup
  made. The next install removes the ones a new brand renamed; an install
  from before profiles is taken to have made `Anvil.lnk`.
- **A self-update** runs the stock setup with no options: the profile stays,
  and the shortcuts are named from it again.
- **A branded setup:** `app/packaging/brand.py <profile> --bundle <app's
  files> --version …` runs Inno Setup with `/DProfile` (the profile as a
  `.zip` inside the setup), `/DName` (the setup's and uninstaller's name)
  and `/DOutName` (`Acme-Voice-v0.2.7-windows-x86_64-setup.exe`). The app's
  files are a new release asset,
  `Anvil-<version>-windows-x86_64-bundle.zip`. `/PROFILE=` on its command
  line still goes first.
- **`profile.json`** may be saved with a byte order mark (Windows
  PowerShell 5 writes one); the app now reads it either way.
- **Proved** by the release workflow's Windows job: the stock install, then
  a profile with `/SIGNIN=` over it (the shortcuts renamed with the
  brand's icon, Anvil's gone, the app id and uninstaller named, the app's
  own start keeping them), the stock setup again as an update keeping all
  of it, a `brand.py` setup renaming it again, a missing profile failing
  the install, and the uninstaller taking all of it away.

### 4.3 M3 as built (v0.2.8)

- **One brand on the system:** the app's brand as its screens show it,
  FCP's once signed in, else the installed profile's, else none. The
  desktop bridge hands its name and icon to the shell
  (`DesktopShell.setBrand`) when either changes, and Anvil's at start when
  there is none.
- **The icons** are made by `anvil-app` (`system_brand.rs`) from the brand's
  PNG, JPEG or WebP image, in the app's directory under `brand\`
  (`%LOCALAPPDATA%\Anvil\Anvil\brand` on Windows):
  - the window's: an ICO of 16, 24, 32, 48 and 256 pixels on Windows, a
    256-pixel PNG elsewhere (the Dock's and a Linux menu's too);
  - the tray's, one per state: the brand's icon with a dot in the state's
    colour cut out of its bottom right corner (Q5), an ICO of 16 to 32
    pixels on Windows, a 64-pixel PNG elsewhere.
  - Each file is named by the image's hash: Windows keeps an icon by its
    path, so a new image at an old path would go unseen. An earlier
    image's files go.
  - An image that cannot be read (an SVG) leaves the icons Anvil's and is
    logged; the name is still shown.
- **The window, the tray and the Dock:** `window_manager.setIcon` (Windows,
  Linux), the tray's icons by state, and `NSApplication.applicationIconImage`
  over `anvil/dock` (macOS). Without a brand they are Anvil's again
  (`assets/app`, the Dock's bundle icon). On macOS tray_manager takes an icon
  only from the app's assets, so a brand's is handed to its channel as its
  own Dart side would.
- **Windows:**
  - The shortcuts listed in `{app}\profile\shortcuts.json`, and no others,
    are saved under the brand's name (`IShellLinkW`, keeping their target
    and app id) with the brand's ICO, or Anvil's name and the program's
    icon without a brand. The list is rewritten with the new names, with a
    byte order mark as the installer reads it.
  - The notifications' app id is given the name and the ICO
    (`DisplayName`, `IconUri`; the icon taken away without a brand).
  - The next install still names the shortcuts from the profile and
    removes the ones the app renamed (it goes by the list); the app,
    started again, names them for FCP's brand once more.
  - The uninstaller removes the shortcuts in the list, which the uninstall
    log does not know under their new names.
- **Linux:** `~/.local/share/applications/com.thevoiceguy.anvil.desktop`,
  the packaged entry with the brand's name (every language's replaced) and
  its PNG, marked `X-Anvil-Brand=true`; removed without a brand. Nothing is
  written where no entry is packaged (an AppImage), and a user's own entry
  without the mark is never touched.
- **Mobile:** nothing, as designed.
- **Proved** by unit tests (the icons' sizes, the dot in each state's
  colour, files by hash, `shortcuts.json` as the installer writes it, the
  menu entry), widget tests (the brand reaching the shell once per change,
  Anvil's back without one, a managed install's before FCP's), the release
  workflow's Windows job (the app re-icons the installer's shortcuts and
  app id, a brand given to the app alone renames the shortcuts on the
  Start menu and the desktop, no brand names them Anvil with the program's
  icon, the uninstaller removes the renamed ones) and its `.deb` job (the
  menu entry written for a brand and gone without one).

### 4.4 M4 as built (v0.2.9)

- **Where a profile and policy are read** was built in M1 and is unchanged:
  `/Library/Application Support/Anvil/profile` and managed preferences
  (`com.thevoiceguy.anvil`) on macOS, `/etc/anvil/profile` and
  `/etc/anvil/policy.json` on Linux, a folder `anvil-profile` beside an
  AppImage. An MDM or a configuration tool can put them there for the
  stock packages; M4 adds packages that carry them.
- **`brand.py`** builds for every desktop, from the release's own file for
  it; the target follows from `--bundle` (`.dmg` or `.app`, `.deb`, else
  Windows) or `--target`:
  - **macOS** (`--bundle Anvil-<label>-macos.dmg`, on a Mac):
    `Acme-Voice-<label>-macos.pkg`. It installs `/Applications/Acme
    Voice.app` (`CFBundleName` and `CFBundleDisplayName` the brand's,
    `Brand.icns` made with sips and iconutil as `CFBundleIconFile`, the
    asset catalog's `CFBundleIconName` removed so the file wins, signed
    again ad hoc) and the profile in
    `/Library/Application Support/Anvil/profile`. The bundle's identity
    stays `com.thevoiceguy.anvil`, so the app's files and managed
    preferences are the same; the package is not relocatable, so it never
    lands on a stock `Anvil.app` found elsewhere.
  - **Linux** (`--bundle anvil_<label>_amd64.deb`, with dpkg-deb):
    `acme-voice_<label>_amd64.deb`, the stock package with the profile in
    `/etc/anvil/profile` and its menu entry named from the brand, with the
    profile's icon. It is still the package `anvil`, so the stock package
    replaces it as an update. The profile's files are conffiles: kept when
    the stock package replaces this one, gone on a purge; the menu entry
    goes back to Anvil's, and the app names it for the brand again for each
    user (M3).
  - **Windows** as in M2.
- **Updates** on macOS and from a `.deb` offer the stock download (§1); a
  company that hands out branded builds sets `updates` to `notify` or
  `off` and hands out the next one.
- **Proved** by the release workflow: on Linux, a branded `.deb` built from
  the release's, installed (the profile, the menu entry, the app showing
  the brand), the stock package over it keeping the profile, a purge taking
  it away; on macOS, a branded `.pkg` built from the release's disk image,
  installed (the renamed app, its icon, its identity, a valid signature,
  the profile) and started.

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
