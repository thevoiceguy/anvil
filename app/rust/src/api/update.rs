//! The app updating itself (`docs/APP.md` §7, U3b): a thin layer over
//! `anvil-update`, keeping what was found and fetched between calls.

use std::path::PathBuf;
use std::sync::OnceLock;

use anvil_update::{Available, Install, Updater};
use flutter_rust_bridge::frb;
use parking_lot::Mutex;

static AVAILABLE: Mutex<Option<Available>> = Mutex::new(None);
static DOWNLOADED: Mutex<Option<PathBuf>> = Mutex::new(None);

fn updater() -> anyhow::Result<&'static Updater> {
    static UPDATER: OnceLock<Updater> = OnceLock::new();
    if let Some(u) = UPDATER.get() {
        return Ok(u);
    }
    let u = Updater::new(env!("CARGO_PKG_VERSION"))?;
    Ok(UPDATER.get_or_init(|| u))
}

/// This copy's version.
#[frb(sync)]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// A newer release, as the app shows it.
pub struct UpdateInfo {
    pub version: String,
    /// The release's page.
    pub notes: String,
    /// This copy can install it itself.
    pub can_install: bool,
    /// Why not, when it cannot.
    pub reason: Option<String>,
    /// The file to install by hand, when it cannot.
    pub download: Option<String>,
}

/// The latest release, when it is newer than this copy.
pub async fn check_for_update() -> anyhow::Result<Option<UpdateInfo>> {
    let updater = updater()?;
    let available = super::phone::on_runtime_pub(async move { Ok(updater.check().await?) }).await?;
    let info = available.as_ref().map(|a| {
        let install = updater.install_kind();
        let (reason, download) = match install {
            Install::Manual { reason, target } => (
                Some(reason.clone()),
                target
                    .and_then(|t| a.manifest.files.get(t))
                    .map(|f| f.url.clone())
                    .or_else(|| Some(a.manifest.notes.clone())),
            ),
            _ => (None, None),
        };
        UpdateInfo {
            version: a.manifest.version.clone(),
            notes: a.manifest.notes.clone(),
            can_install: a.file.is_some(),
            reason,
            download,
        }
    });
    *AVAILABLE.lock() = available;
    Ok(info)
}

/// Fetch the update found, checked against its manifest.
pub async fn download_update() -> anyhow::Result<()> {
    let updater = updater()?;
    let available = AVAILABLE
        .lock()
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no update has been found"))?;
    let dir = std::env::temp_dir().join("anvil-update");
    let path =
        super::phone::on_runtime_pub(async move { Ok(updater.download(&available, &dir).await?) })
            .await?;
    *DOWNLOADED.lock() = Some(path);
    Ok(())
}

/// Install the update fetched once the app has quit, and start it again.
/// The app quits right after.
pub fn install_update() -> anyhow::Result<()> {
    let path = DOWNLOADED
        .lock()
        .clone()
        .ok_or_else(|| anyhow::anyhow!("no update has been fetched"))?;
    updater()?.install(&path)?;
    Ok(())
}
