//! The updater against a stand-in for GitHub's release downloads: a newer
//! release found and fetched, and every way a release can be wrong refused.

use std::collections::HashMap;
use std::sync::Arc;

use anvil_update::{Install, UpdateError, Updater};
use ring::signature::{Ed25519KeyPair, KeyPair};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Release {
    key: Ed25519KeyPair,
    file: Vec<u8>,
}

impl Release {
    fn new() -> Self {
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).unwrap();
        Self {
            key: Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap(),
            file: b"the new AppImage".to_vec(),
        }
    }

    fn public(&self) -> Vec<u8> {
        self.key.public_key().as_ref().to_vec()
    }

    /// The feed for `version`, its file served as `served`.
    async fn feed(&self, version: &str, served: &[u8], signer: &Ed25519KeyPair) -> String {
        let sha256: String = ring::digest::digest(&ring::digest::SHA256, &self.file)
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        // The URL is filled in once the server's address is known.
        let manifest = |base: &str| {
            format!(
                r#"{{"version":"{version}","notes":"{base}notes","files":{{"linux-x86_64-appimage":{{"name":"Anvil-v{version}-x86_64.AppImage","url":"{base}file","sha256":"{sha256}","size":{size}}}}}}}"#,
                size = self.file.len()
            )
        };
        let mut files = HashMap::new();
        files.insert("/file".to_string(), served.to_vec());
        serve_with(files, manifest, signer).await
    }
}

/// Serves `files` over HTTP/1.1 until the test ends, with the manifest made
/// for the server's own address and signed by `signer`.
async fn serve_with(
    mut files: HashMap<String, Vec<u8>>,
    manifest: impl Fn(&str) -> String,
    signer: &Ed25519KeyPair,
) -> String {
    // The server's address must be in the manifest: reserve it, then serve.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let base = format!("http://127.0.0.1:{port}/");
    let body = manifest(&base).into_bytes();
    files.insert(
        "/latest.json.sig".into(),
        signer.sign(&body).as_ref().to_vec(),
    );
    files.insert("/latest.json".into(), body);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    let files = Arc::new(files);
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let files = Arc::clone(&files);
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                let line = String::from_utf8_lossy(&request);
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (status, body) = match files.get(&path) {
                    Some(body) => ("200 OK", body.clone()),
                    None => ("404 Not Found", Vec::new()),
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });
    base
}

fn appimage() -> Install {
    Install::AppImage {
        path: std::env::temp_dir().join("never-touched.AppImage"),
    }
}

fn scratch(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("anvil-update-{name}-{}", std::process::id()))
}

#[tokio::test]
async fn a_newer_release_is_found_and_its_file_fetched_and_checked() {
    let release = Release::new();
    let feed = release.feed("0.2.0", &release.file, &release.key).await;
    let updater = Updater::with("0.1.0", &feed, &release.public(), appimage()).unwrap();
    let available = updater.check().await.unwrap().expect("0.2.0 is newer");
    assert_eq!(available.version(), "0.2.0");
    let file = available.file.clone().expect("an AppImage in the release");
    assert_eq!(file.name, "Anvil-v0.2.0-x86_64.AppImage");

    let dir = scratch("ok");
    let path = updater.download(&available, &dir).await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), release.file);
    std::fs::remove_dir_all(&dir).unwrap();

    // The same release, or an older one, is no update.
    let same = Updater::with("0.2.0", &feed, &release.public(), appimage()).unwrap();
    assert_eq!(same.check().await.unwrap(), None);
    let newer = Updater::with("1.0.0", &feed, &release.public(), appimage()).unwrap();
    assert_eq!(newer.check().await.unwrap(), None);
}

#[tokio::test]
async fn a_manifest_signed_by_another_key_is_refused() {
    let release = Release::new();
    let stranger = Release::new();
    let feed = release.feed("0.2.0", &release.file, &stranger.key).await;
    let updater = Updater::with("0.1.0", &feed, &release.public(), appimage()).unwrap();
    assert!(matches!(
        updater.check().await,
        Err(UpdateError::BadSignature)
    ));
}

#[tokio::test]
async fn a_file_that_is_not_the_releases_is_refused_and_removed() {
    let release = Release::new();
    for (served, why) in [
        (b"the new AppImagX".to_vec(), "SHA-256"),
        (b"the new AppImage, and more".to_vec(), "bytes"),
        (b"short".to_vec(), "bytes"),
    ] {
        let feed = release.feed("0.2.0", &served, &release.key).await;
        let updater = Updater::with("0.1.0", &feed, &release.public(), appimage()).unwrap();
        let available = updater.check().await.unwrap().unwrap();
        let dir = scratch("bad");
        match updater.download(&available, &dir).await {
            Err(UpdateError::BadFile(e)) => assert!(e.contains(why), "{e}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "nothing left behind"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[tokio::test]
async fn a_copy_that_updates_by_hand_is_told_of_the_release_but_fetches_nothing() {
    let release = Release::new();
    let feed = release.feed("0.2.0", &release.file, &release.key).await;
    let manual = Install::Manual {
        reason: "installed from the .deb".into(),
        target: Some("linux-amd64-deb"),
    };
    let updater = Updater::with("0.1.0", &feed, &release.public(), manual).unwrap();
    let available = updater.check().await.unwrap().unwrap();
    assert_eq!(available.file, None);
    assert!(matches!(
        updater.download(&available, &scratch("manual")).await,
        Err(UpdateError::CannotInstall(_))
    ));
}
