//! An install set up by its company (`docs/MANAGED_INSTALLS.md`): the
//! profile installed with the app and the policy IT sets, merged field by
//! field into what the sign-in screen, the updater and the phone follow.
//!
//! Policy wins over the profile; what the person typed fills in the rest.
//! The profile's brand is the app's before anyone signs in, and whenever
//! FCP has none.

use std::path::{Path, PathBuf};

use anvil_core::brand::{BrandAsset, BrandColors, BrandProfile, Ringtone};
use serde::Deserialize;

/// The profile's own version this copy reads.
pub const PROFILE_VERSION: u32 = 1;
/// The most a profile's file may hold, as for a brand's asset from FCP.
const MAX_ASSET: u64 = 5 * 1024 * 1024;

/// How the sign-in screen goes on from who and where.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignInMode {
    /// Ask FCP how this person signs in.
    #[default]
    Auto,
    /// Straight to the browser.
    Sso,
    /// The password form.
    Password,
}

/// Whether the app updates itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdatesMode {
    /// Found, fetched and installed when the person says.
    #[default]
    Auto,
    /// Offered, never installed by the app: the company installs it.
    Notify,
    /// Never looked for.
    Off,
}

/// The settings one source gives; each is open (`None`) unless it sets it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub server: Option<String>,
    #[serde(default)]
    pub lock_server: Option<bool>,
    #[serde(default)]
    pub login_domain: Option<String>,
    #[serde(default)]
    pub sign_in: Option<SignInMode>,
    #[serde(default)]
    pub username_from_system: Option<bool>,
    #[serde(default)]
    pub updates: Option<UpdatesMode>,
}

impl Settings {
    /// These, with what they leave open taken from `lower`.
    fn over(self, lower: Settings) -> Settings {
        Settings {
            server: self.server.or(lower.server),
            lock_server: self.lock_server.or(lower.lock_server),
            login_domain: self.login_domain.or(lower.login_domain),
            sign_in: self.sign_in.or(lower.sign_in),
            username_from_system: self.username_from_system.or(lower.username_from_system),
            updates: self.updates.or(lower.updates),
        }
    }

    /// Empty strings are no setting: a policy value left blank.
    fn trimmed(mut self) -> Settings {
        let clean = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        self.server = clean(self.server);
        self.login_domain = clean(self.login_domain).map(|d| d.trim_start_matches('@').to_string());
        self
    }
}

/// `profile.json` (`docs/MANAGED_INSTALLS.md` §2.1). Fields this copy does
/// not know are ignored, so a newer profile still gives what it can.
#[derive(Debug, Clone, Default, Deserialize)]
struct ProfileFile {
    #[serde(default)]
    profile_version: Option<u32>,
    #[serde(flatten)]
    settings: Settings,
    #[serde(default)]
    brand: Option<ProfileBrand>,
}

/// A brand in `/brand/v1`'s shape with file names, beside `profile.json`,
/// in place of URLs.
#[derive(Debug, Clone, Default, Deserialize)]
struct ProfileBrand {
    #[serde(default)]
    app_name: Option<String>,
    #[serde(default)]
    colors: BrandColors,
    #[serde(default)]
    dark_colors: Option<BrandColors>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    logo: Option<String>,
    #[serde(default)]
    logo_dark: Option<String>,
    #[serde(default)]
    ringtone: Option<String>,
}

/// Where the settings are read from: the policy sources, highest first,
/// and the profile's directory.
#[derive(Debug, Clone, Default)]
pub struct Sources {
    /// Each named for the log (`HKLM\Software\Policies\Anvil`).
    pub policies: Vec<(String, Settings)>,
    pub profile_dir: Option<PathBuf>,
    /// The signed-in system user's principal name, when there is one.
    pub system_username: Option<String>,
}

/// What an install was set up with, merged.
#[derive(Debug, Clone, Default)]
pub struct Managed {
    /// What the server field holds.
    pub server: Option<String>,
    /// The server field cannot be changed.
    pub lock_server: bool,
    /// Put after a bare username (`james` → `james@acme.example`).
    pub login_domain: Option<String>,
    pub sign_in: SignInMode,
    /// The username to fill in, from the system.
    pub username: Option<String>,
    pub updates: UpdatesMode,
    /// The profile's brand, its files read.
    pub brand: Option<BrandProfile>,
    /// The brand's icon as the system shows it (`icon.ico`, which Windows'
    /// installer makes from the profile's PNG beside `profile.json`).
    pub system_icon: Option<PathBuf>,
    /// Where each setting came from, for the log and Settings.
    pub sources: Vec<String>,
}

impl Managed {
    /// Whether anything set this install up.
    pub fn is_managed(&self) -> bool {
        !self.sources.is_empty()
    }

    /// The username to sign in with: `login_domain` after a bare one.
    pub fn login(&self, username: &str) -> String {
        let user = username.trim();
        match &self.login_domain {
            Some(domain) if !user.is_empty() && !user.contains('@') => format!("{user}@{domain}"),
            _ => user.to_string(),
        }
    }
}

/// This install's settings, from the platform's places.
pub fn load() -> Managed {
    let managed = load_from(platform_sources());
    if managed.is_managed() {
        tracing::info!(sources = ?managed.sources, "a managed install");
    }
    managed
}

/// The settings from `sources`: policy field by field over the profile.
pub fn load_from(sources: Sources) -> Managed {
    let mut named = Vec::new();
    let mut policy = Settings::default();
    // Lowest first, so each higher source goes over what is below it.
    for (name, settings) in sources.policies.into_iter().rev() {
        let settings = settings.trimmed();
        if settings != Settings::default() {
            named.insert(0, name);
        }
        policy = settings.over(policy);
    }
    let (profile, brand) = match &sources.profile_dir {
        Some(dir) => match read_profile(dir) {
            Some(file) => {
                named.push(format!("profile {}", dir.display()));
                let brand = file.brand.map(|b| brand_from(dir, b));
                (file.settings.trimmed(), brand)
            }
            None => (Settings::default(), None),
        },
        None => (Settings::default(), None),
    };
    // A server set by policy is always locked (Q3); a profile's is unless
    // it says otherwise.
    let lock_server = if policy.server.is_some() {
        true
    } else {
        let server = profile.server.is_some();
        policy.lock_server.or(profile.lock_server).unwrap_or(server) && server
    };
    let merged = policy.over(profile);
    let username = merged
        .username_from_system
        .unwrap_or(false)
        .then_some(sources.system_username)
        .flatten();
    Managed {
        server: merged.server,
        lock_server,
        login_domain: merged.login_domain,
        sign_in: merged.sign_in.unwrap_or_default(),
        username,
        updates: merged.updates.unwrap_or_default(),
        system_icon: sources
            .profile_dir
            .as_ref()
            .map(|dir| dir.join("icon.ico"))
            .filter(|ico| brand.is_some() && ico.is_file()),
        brand,
        sources: named,
    }
}

fn read_profile(dir: &Path) -> Option<ProfileFile> {
    let path = dir.join("profile.json");
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(%e, path = %path.display(), "the profile is not read");
            return None;
        }
    };
    match serde_json::from_slice::<ProfileFile>(without_bom(&bytes)) {
        Ok(file) => {
            if file.profile_version.is_some_and(|v| v > PROFILE_VERSION) {
                tracing::info!(
                    version = file.profile_version,
                    "a newer profile: what this copy knows of it is used"
                );
            }
            Some(file)
        }
        Err(e) => {
            tracing::warn!(%e, path = %path.display(), "the profile is not read");
            None
        }
    }
}

/// A file saved by Windows PowerShell or an older Notepad starts with a
/// UTF-8 byte order mark, which JSON does not allow.
fn without_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)
}

/// The profile's brand as FCP's would be, with its files' bytes. A file
/// that cannot be read is left out, as a failed asset from FCP is.
fn brand_from(dir: &Path, b: ProfileBrand) -> BrandProfile {
    let asset = |name: &Option<String>| name.as_deref().and_then(|n| read_asset(dir, n));
    let ringtones = b
        .ringtone
        .as_deref()
        .and_then(|n| {
            let asset = read_asset(dir, n)?;
            let name = Path::new(n)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| n.to_string());
            Some(Ringtone {
                name,
                asset,
                default: true,
            })
        })
        .into_iter()
        .collect();
    BrandProfile {
        schema_version: 1,
        tenant_id: "profile".into(),
        etag: String::new(),
        app_name: b
            .app_name
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Anvil".into()),
        colors: b.colors,
        dark_colors: b.dark_colors,
        logo: asset(&b.logo),
        logo_dark: asset(&b.logo_dark),
        icon: asset(&b.icon),
        ringtones,
        links: Default::default(),
        dial_plan: Default::default(),
        fetched_at: None,
    }
}

fn read_asset(dir: &Path, name: &str) -> Option<BrandAsset> {
    // A file beside profile.json, never one elsewhere.
    let file = Path::new(name);
    if file.components().count() != 1 || file.is_absolute() {
        tracing::warn!(name, "a profile's file must be beside profile.json");
        return None;
    }
    let path = dir.join(file);
    let read = std::fs::metadata(&path).and_then(|m| {
        if m.len() > MAX_ASSET {
            Err(std::io::Error::other("larger than 5 MB"))
        } else {
            std::fs::read(&path)
        }
    });
    match read {
        Ok(bytes) => Some(BrandAsset {
            url: format!("file:{}", path.display()),
            mime: mime_of(name).into(),
            sha256: String::new(),
            bytes,
            width: None,
            height: None,
            local_path: Some(path.display().to_string()),
        }),
        Err(e) => {
            tracing::warn!(%e, path = %path.display(), "a profile's file left out");
            None
        }
    }
}

fn mime_of(name: &str) -> &'static str {
    let ext = Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "ogg" | "oga" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// The places this platform keeps policy and the profile. `ANVIL_PROFILE`
/// (a directory) and `ANVIL_POLICY` (a JSON file) go first, for a test or
/// a trial.
fn platform_sources() -> Sources {
    let mut policies = Vec::new();
    if let Some(path) = std::env::var_os("ANVIL_POLICY") {
        let path = PathBuf::from(path);
        if let Some(s) = json_policy(&path) {
            policies.push((format!("policy {}", path.display()), s));
        }
    }
    policies.extend(platform::policies());
    let profile_dir = std::env::var_os("ANVIL_PROFILE")
        .map(PathBuf::from)
        .or_else(platform::profile_dir);
    Sources {
        policies,
        profile_dir,
        system_username: platform::system_username(),
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn json_policy(path: &Path) -> Option<Settings> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(%e, path = %path.display(), "a policy is not read");
            return None;
        }
    };
    serde_json::from_slice(without_bom(&bytes))
        .map_err(|e| tracing::warn!(%e, path = %path.display(), "a policy is not read"))
        .ok()
}

/// A policy's value by its name in the registry or a managed preference
/// (`Server`, `LockServer`, …), as the settings take it.
#[cfg_attr(target_os = "linux", allow(dead_code))]
enum Value {
    Text(String),
    Flag(bool),
}

/// Settings from named values, whatever stores them.
#[cfg_attr(target_os = "linux", allow(dead_code))]
fn settings_from(get: impl Fn(&str) -> Option<Value>) -> Settings {
    let text = |name: &str| match get(name)? {
        Value::Text(t) => Some(t),
        Value::Flag(_) => None,
    };
    let flag = |name: &str| match get(name)? {
        Value::Flag(f) => Some(f),
        Value::Text(t) => match t.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        },
    };
    fn mode<M: serde::de::DeserializeOwned>(text: Option<String>) -> Option<M> {
        let t = text?.trim().to_ascii_lowercase();
        serde_json::from_value(serde_json::Value::String(t)).ok()
    }
    Settings {
        server: text("Server"),
        lock_server: flag("LockServer"),
        login_domain: text("LoginDomain"),
        sign_in: mode(text("SignIn")),
        username_from_system: flag("UsernameFromSystem"),
        updates: mode(text("Updates")),
    }
}

#[cfg(windows)]
mod platform {
    use super::*;

    pub fn policies() -> Vec<(String, Settings)> {
        crate::managed_windows::policies()
            .into_iter()
            .map(|(name, values)| {
                let settings = settings_from(|key| {
                    values.iter().find(|(k, _)| k == key).map(|(_, v)| match v {
                        crate::managed_windows::RegValue::Text(t) => Value::Text(t.clone()),
                        crate::managed_windows::RegValue::Dword(d) => Value::Flag(*d != 0),
                    })
                });
                (name, settings)
            })
            .collect()
    }

    /// `{app}\profile`, beside the exe.
    pub fn profile_dir() -> Option<PathBuf> {
        Some(std::env::current_exe().ok()?.parent()?.join("profile"))
    }

    pub fn system_username() -> Option<String> {
        crate::managed_windows::user_principal_name()
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    const DOMAIN: &str = "com.thevoiceguy.anvil";

    /// Managed preferences from a configuration profile: the user's, then
    /// the machine's.
    pub fn policies() -> Vec<(String, Settings)> {
        let mut places = Vec::new();
        if let Ok(user) = std::env::var("USER") {
            places.push(PathBuf::from(format!(
                "/Library/Managed Preferences/{user}/{DOMAIN}.plist"
            )));
        }
        places.push(PathBuf::from(format!(
            "/Library/Managed Preferences/{DOMAIN}.plist"
        )));
        places
            .into_iter()
            .filter_map(|path| {
                let dict = plist::Value::from_file(&path).ok()?.into_dictionary()?;
                let settings = settings_from(|key| match dict.get(key)? {
                    plist::Value::String(s) => Some(Value::Text(s.clone())),
                    plist::Value::Boolean(b) => Some(Value::Flag(*b)),
                    plist::Value::Integer(i) => Some(Value::Flag(i.as_signed() != Some(0))),
                    _ => None,
                });
                Some((format!("policy {}", path.display()), settings))
            })
            .collect()
    }

    pub fn profile_dir() -> Option<PathBuf> {
        Some(PathBuf::from("/Library/Application Support/Anvil/profile"))
    }

    pub fn system_username() -> Option<String> {
        None
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use super::*;

    pub fn policies() -> Vec<(String, Settings)> {
        let path = Path::new("/etc/anvil/policy.json");
        json_policy(path)
            .map(|s| vec![(format!("policy {}", path.display()), s)])
            .unwrap_or_default()
    }

    /// Beside an AppImage (`anvil-profile`), else `/etc/anvil/profile`.
    pub fn profile_dir() -> Option<PathBuf> {
        if let Some(image) = std::env::var_os("APPIMAGE") {
            let beside = Path::new(&image).parent()?.join("anvil-profile");
            if beside.join("profile.json").exists() {
                return Some(beside);
            }
        }
        Some(PathBuf::from("/etc/anvil/profile"))
    }

    pub fn system_username() -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "anvil-profile-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn profile(json: &str) -> PathBuf {
        let dir = dir();
        std::fs::write(dir.join("profile.json"), json).unwrap();
        dir
    }

    #[test]
    fn nothing_set_is_an_ordinary_install() {
        let m = load_from(Sources {
            profile_dir: Some(dir()),
            ..Default::default()
        });
        assert!(!m.is_managed());
        assert_eq!(m.server, None);
        assert!(!m.lock_server);
        assert_eq!(m.sign_in, SignInMode::Auto);
        assert_eq!(m.updates, UpdatesMode::Auto);
        assert!(m.brand.is_none());
    }

    #[test]
    fn policy_goes_over_the_profile_field_by_field() {
        let dir = profile(
            r#"{"profile_version": 1, "server": "https://p.example", "lock_server": false,
                "login_domain": "p.example", "sign_in": "password", "updates": "notify"}"#,
        );
        let m = load_from(Sources {
            policies: vec![
                (
                    "machine".into(),
                    Settings {
                        updates: Some(UpdatesMode::Off),
                        ..Default::default()
                    },
                ),
                (
                    "user".into(),
                    Settings {
                        updates: Some(UpdatesMode::Auto),
                        sign_in: Some(SignInMode::Sso),
                        login_domain: Some("  ".into()),
                        ..Default::default()
                    },
                ),
            ],
            profile_dir: Some(dir.clone()),
            system_username: Some("james@acme.example".into()),
        });
        assert_eq!(
            m.updates,
            UpdatesMode::Off,
            "the machine's policy is highest"
        );
        assert_eq!(
            m.sign_in,
            SignInMode::Sso,
            "the user's policy over the profile"
        );
        assert_eq!(
            m.login_domain.as_deref(),
            Some("p.example"),
            "a blank policy value is no setting"
        );
        assert_eq!(m.server.as_deref(), Some("https://p.example"));
        assert!(!m.lock_server, "the profile may leave its server open");
        assert_eq!(m.username, None, "the system's name only when asked for");
        assert_eq!(m.sources.len(), 3);
    }

    #[test]
    fn a_server_is_locked_unless_the_profile_says_otherwise_and_always_by_policy() {
        let m = load_from(Sources {
            profile_dir: Some(profile(r#"{"server": "pbx.example"}"#)),
            ..Default::default()
        });
        assert!(m.lock_server);
        let m = load_from(Sources {
            policies: vec![(
                "machine".into(),
                Settings {
                    server: Some("https://policy.example".into()),
                    lock_server: Some(false),
                    ..Default::default()
                },
            )],
            profile_dir: Some(profile(
                r#"{"server": "pbx.example", "lock_server": false}"#,
            )),
            ..Default::default()
        });
        assert_eq!(m.server.as_deref(), Some("https://policy.example"));
        assert!(m.lock_server);
        let m = load_from(Sources {
            profile_dir: Some(profile(r#"{"lock_server": true}"#)),
            ..Default::default()
        });
        assert!(!m.lock_server, "nothing to lock without a server");
    }

    #[test]
    fn the_login_domain_completes_a_bare_username() {
        let m = load_from(Sources {
            profile_dir: Some(profile(
                r#"{"login_domain": "@acme.example", "username_from_system": true}"#,
            )),
            system_username: Some("james@acme.example".into()),
            ..Default::default()
        });
        assert_eq!(m.login("james"), "james@acme.example");
        assert_eq!(m.login(" ann@other.example "), "ann@other.example");
        assert_eq!(m.login(""), "");
        assert_eq!(m.username.as_deref(), Some("james@acme.example"));
    }

    #[test]
    fn the_profiles_brand_is_read_with_its_files() {
        let dir = profile(
            r##"{"profile_version": 2, "future_field": [1, 2],
                "brand": {"app_name": "Acme Voice",
                          "colors": {"primary": "#1A73E8", "accent": "nope"},
                          "logo": "logo.png", "icon": "missing.png",
                          "logo_dark": "../outside.png", "ringtone": "ring.ogg"}}"##,
        );
        std::fs::write(dir.join("logo.png"), b"\x89PNG logo").unwrap();
        std::fs::write(dir.join("ring.ogg"), b"OggS").unwrap();
        let m = load_from(Sources {
            profile_dir: Some(dir),
            ..Default::default()
        });
        assert!(m.is_managed(), "a newer profile is still read");
        let brand = m.brand.unwrap();
        assert_eq!(brand.app_name, "Acme Voice");
        assert_eq!(brand.colors.primary.unwrap().to_hex(), "#1A73E8");
        assert!(brand.colors.accent.is_none(), "a bad colour is dropped");
        let logo = brand.logo.unwrap();
        assert_eq!(logo.mime, "image/png");
        assert_eq!(logo.bytes, b"\x89PNG logo");
        assert!(brand.icon.is_none(), "a missing file is left out");
        assert!(brand.logo_dark.is_none(), "only files beside profile.json");
        assert_eq!(brand.ringtones.len(), 1);
        assert_eq!(brand.ringtones[0].name, "ring");
        assert_eq!(brand.ringtones[0].asset.mime, "audio/ogg");
        assert!(brand.ringtones[0].default);
    }

    #[test]
    fn a_profile_saved_with_a_byte_order_mark_is_read() {
        let dir = dir();
        std::fs::write(
            dir.join("profile.json"),
            b"\xEF\xBB\xBF{\"server\": \"pbx.example\"}",
        )
        .unwrap();
        let m = load_from(Sources {
            profile_dir: Some(dir),
            ..Default::default()
        });
        assert_eq!(m.server.as_deref(), Some("pbx.example"));
    }

    #[test]
    fn the_installers_icon_is_the_systems_when_the_profile_has_a_brand() {
        let dir = profile(r#"{"brand": {"app_name": "Acme Voice", "icon": "icon.png"}}"#);
        let m = load_from(Sources {
            profile_dir: Some(dir.clone()),
            ..Default::default()
        });
        assert_eq!(m.system_icon, None, "no icon.ico yet");
        std::fs::write(dir.join("icon.ico"), b"ico").unwrap();
        let m = load_from(Sources {
            profile_dir: Some(dir.clone()),
            ..Default::default()
        });
        assert_eq!(m.system_icon, Some(dir.join("icon.ico")));
        std::fs::write(dir.join("profile.json"), r#"{"server": "pbx.example"}"#).unwrap();
        let m = load_from(Sources {
            profile_dir: Some(dir),
            ..Default::default()
        });
        assert_eq!(
            m.system_icon, None,
            "a profile without a brand names no icon"
        );
    }

    #[test]
    fn a_profile_that_is_not_json_is_ignored() {
        let m = load_from(Sources {
            profile_dir: Some(profile("server = nope")),
            ..Default::default()
        });
        assert!(!m.is_managed());
    }

    #[test]
    fn named_values_become_settings() {
        let s = settings_from(|key| match key {
            "Server" => Some(Value::Text("https://pbx.example".into())),
            "LockServer" => Some(Value::Flag(false)),
            "SignIn" => Some(Value::Text("SSO".into())),
            "UsernameFromSystem" => Some(Value::Text("1".into())),
            "Updates" => Some(Value::Text("sometimes".into())),
            _ => None,
        });
        assert_eq!(s.server.as_deref(), Some("https://pbx.example"));
        assert_eq!(s.lock_server, Some(false));
        assert_eq!(s.sign_in, Some(SignInMode::Sso));
        assert_eq!(s.username_from_system, Some(true));
        assert_eq!(s.updates, None, "an unknown mode is no setting");
        assert_eq!(s.login_domain, None);
    }
}
