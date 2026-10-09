//! How this copy of the app was installed, and so how it updates.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::UpdateError;

/// How this copy was installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    /// By the Windows setup, in `%LOCALAPPDATA%\Programs\Anvil`: the new
    /// setup runs silently over it.
    WindowsSetup,
    /// An app bundle on macOS, in a folder this user may write: the new one
    /// takes its place.
    MacBundle { bundle: PathBuf },
    /// An AppImage: the new file takes its place.
    AppImage { path: PathBuf },
    /// Not by a way this copy can update itself (the .deb, which apt
    /// installs; a build run from its folder; a bundle in a folder this user
    /// cannot write). `target` names the release file to point the user at.
    Manual {
        reason: String,
        target: Option<&'static str>,
    },
}

impl Install {
    /// This copy.
    pub fn this() -> Self {
        let Ok(exe) = std::env::current_exe() else {
            return Self::manual("where this copy is cannot be told", None);
        };
        let exe = exe.canonicalize().unwrap_or(exe);
        if cfg!(target_os = "windows") {
            let installed = std::env::var_os("LOCALAPPDATA")
                .map(|d| PathBuf::from(d).join("Programs").join("Anvil"));
            match (exe.parent(), installed) {
                (Some(dir), Some(installed)) if same_dir(dir, &installed) => Self::WindowsSetup,
                _ => Self::manual("not installed by the setup", Some("windows-x86_64-setup")),
            }
        } else if cfg!(target_os = "macos") {
            // …/Anvil.app/Contents/MacOS/Anvil
            match exe.ancestors().nth(3) {
                Some(bundle) if bundle.extension().is_some_and(|e| e == "app") => {
                    let writable = bundle.parent().is_some_and(writable);
                    if writable {
                        Self::MacBundle {
                            bundle: bundle.to_path_buf(),
                        }
                    } else {
                        Self::manual(
                            "its folder is not yours to write: install the new disk image",
                            Some("macos-dmg"),
                        )
                    }
                }
                _ => Self::manual("not an app bundle", Some("macos-dmg")),
            }
        } else if let Some(appimage) = std::env::var_os("APPIMAGE") {
            Self::AppImage {
                path: PathBuf::from(appimage),
            }
        } else if exe.starts_with("/opt/anvil") {
            Self::manual(
                "installed from the .deb: install the new one with apt",
                Some("linux-amd64-deb"),
            )
        } else {
            Self::manual("run from its build folder", None)
        }
    }

    fn manual(reason: &str, target: Option<&'static str>) -> Self {
        Self::Manual {
            reason: reason.to_string(),
            target,
        }
    }

    /// The release file this copy updates from (or, done by hand, the one
    /// to point the user at).
    pub fn target(&self) -> Option<&'static str> {
        match self {
            Self::WindowsSetup => Some("windows-x86_64-setup"),
            Self::MacBundle { .. } => Some("macos-dmg"),
            Self::AppImage { .. } => Some("linux-x86_64-appimage"),
            Self::Manual { .. } => None,
        }
    }

    /// Whether this copy can install an update itself.
    pub fn can_install(&self) -> bool {
        !matches!(self, Self::Manual { .. })
    }

    /// Start installing `file` once process `pid` (this one) has exited,
    /// and start the app again after.
    pub(crate) fn start(&self, file: &Path, pid: u32) -> Result<(), UpdateError> {
        match self {
            Self::WindowsSetup => {
                // The setup waits for nothing: it closes what is still
                // running (Restart Manager), and `/relaunch=1` starts the app
                // when it is done (windows.iss).
                let mut command = Command::new(file);
                command.args([
                    "/VERYSILENT",
                    "/SUPPRESSMSGBOXES",
                    "/NORESTART",
                    "/CLOSEAPPLICATIONS",
                    "/relaunch=1",
                ]);
                detach(&mut command);
                command.spawn()?;
                Ok(())
            }
            Self::MacBundle { bundle } => {
                let script = format!(
                    r#"set -e
while kill -0 {pid} 2>/dev/null; do sleep 0.5; done
mnt=$(mktemp -d)
hdiutil attach -nobrowse -noautoopen -mountpoint "$mnt" {dmg}
new={bundle}.new.$$
old={bundle}.old.$$
ditto "$mnt/Anvil.app" "$new"
hdiutil detach "$mnt" -quiet || true
mv {bundle} "$old"
mv "$new" {bundle}
rm -rf "$old" {dmg}
open {bundle}
"#,
                    dmg = quote(file),
                    bundle = quote(bundle),
                );
                let mut command = Command::new("/bin/sh");
                command.args(["-c", &script]);
                detach(&mut command);
                command.spawn()?;
                Ok(())
            }
            Self::AppImage { path } => {
                // In place now (a running AppImage's file may be replaced),
                // started again once this one has gone.
                let dir = path.parent().ok_or_else(|| {
                    UpdateError::CannotInstall("the AppImage has no folder".into())
                })?;
                let staged = dir.join(".Anvil.AppImage.new");
                std::fs::copy(file, &staged)?;
                set_executable(&staged)?;
                std::fs::rename(&staged, path)?;
                let _ = std::fs::remove_file(file);
                let script = format!(
                    "while kill -0 {pid} 2>/dev/null; do sleep 0.5; done; exec {app}",
                    app = quote(path)
                );
                let mut command = Command::new("/bin/sh");
                command.args(["-c", &script]);
                detach(&mut command);
                command.spawn()?;
                Ok(())
            }
            Self::Manual { reason, .. } => Err(UpdateError::CannotInstall(reason.clone())),
        }
    }
}

/// A path for `sh`, single-quoted.
fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let a = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let b = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

/// Whether this user may write `dir`: tried, not guessed from modes.
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".anvil-update-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(unix)]
fn set_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn set_executable(_: &Path) -> std::io::Result<()> {
    Ok(())
}

/// The helper outlives the app: no console, its own process group.
#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_quoted_for_the_shell() {
        assert_eq!(quote(Path::new("/tmp/a b")), "'/tmp/a b'");
        assert_eq!(quote(Path::new("/tmp/it's")), r"'/tmp/it'\''s'");
    }

    #[test]
    fn a_copy_run_from_a_build_folder_updates_by_hand() {
        // The test binary is not installed any way the updater knows.
        let this = Install::this();
        if std::env::var_os("APPIMAGE").is_none() {
            assert!(!this.can_install(), "{this:?}");
            assert_eq!(this.target(), None);
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_appimage_is_replaced_in_place_and_started_once_this_has_gone() {
        let dir = std::env::temp_dir().join(format!("anvil-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let app = dir.join("Anvil.AppImage");
        let marker = dir.join("started");
        std::fs::write(&app, "old").unwrap();
        let new = dir.join("download");
        std::fs::write(
            &new,
            format!("#!/bin/sh\necho started > '{}'\n", marker.display()),
        )
        .unwrap();
        // A process that is already gone stands for this one.
        let gone = std::process::Command::new("true").spawn().unwrap();
        let pid = gone.id();
        let mut gone = gone;
        gone.wait().unwrap();

        Install::AppImage { path: app.clone() }
            .start(&new, pid)
            .unwrap();
        assert!(std::fs::read_to_string(&app)
            .unwrap()
            .starts_with("#!/bin/sh"));
        assert!(!new.exists(), "the download is moved, not copied");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !marker.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "the new AppImage was not started"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
