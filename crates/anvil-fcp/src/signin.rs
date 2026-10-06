//! Signing in (`docs/APP_SIGN_IN.md` in FCP).

use std::time::Duration;

use base64::Engine;
use rand::Rng;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::{http, refused, AppClient, FcpError, Server, Session};

/// A sign-in through the system browser: a loopback listener for the
/// redirect, a PKCE verifier, and the address to open.
pub struct BrowserSignIn {
    server: Server,
    client: AppClient,
    listener: TcpListener,
    redirect_uri: String,
    verifier: String,
    state: String,
    authorize_url: String,
}

fn random_token(bytes: usize) -> String {
    let raw: Vec<u8> = (0..bytes).map(|_| rand::thread_rng().gen()).collect();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
}

impl BrowserSignIn {
    /// Listen on a loopback port and make the address the browser opens.
    pub async fn start(server: Server, client: AppClient) -> Result<Self, FcpError> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| FcpError::BrowserSignIn(format!("loopback listener: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| FcpError::BrowserSignIn(e.to_string()))?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}/callback");
        let verifier = random_token(48);
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        let state = random_token(16);
        let mut url = url::Url::parse(&server.authorize_url)
            .map_err(|e| FcpError::Invalid(format!("authorize URL: {e}")))?;
        url.query_pairs_mut()
            .append_pair("client_id", "anvil")
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state);
        Ok(Self {
            server,
            client,
            listener,
            redirect_uri,
            verifier,
            state,
            authorize_url: url.to_string(),
        })
    }

    /// What the host opens in the system browser.
    pub fn authorize_url(&self) -> &str {
        &self.authorize_url
    }

    /// Wait for the browser to come back with the code (up to `within`),
    /// and trade it for the app's session.
    pub async fn finish(self, within: Duration) -> Result<Session, FcpError> {
        let code = tokio::time::timeout(within, self.catch_code())
            .await
            .map_err(|_| FcpError::BrowserSignIn("nobody signed in in time".to_string()))??;
        let mut form = vec![
            ("grant_type", "authorization_code".to_string()),
            ("code", code),
            ("code_verifier", self.verifier.clone()),
            ("redirect_uri", self.redirect_uri.clone()),
            ("client_id", "anvil".to_string()),
            ("client_name", self.client.client_name.clone()),
            ("install_id", self.client.install_id.clone()),
        ];
        if let Some(p) = &self.client.platform {
            form.push(("platform", p.clone()));
        }
        if let Some(v) = &self.client.version {
            form.push(("version", v.clone()));
        }
        let response = http()
            .post(&self.server.token_url)
            .form(&form)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(refused(response).await);
        }
        let tokens: serde_json::Value = response.json().await?;
        Session::from_tokens(self.server, self.client, &tokens, None)
    }

    /// One request on the loopback listener: the redirect, its code checked
    /// against our state; a page telling the person to go back to the app.
    async fn catch_code(&self) -> Result<String, FcpError> {
        loop {
            let (mut stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|e| FcpError::BrowserSignIn(e.to_string()))?;
            let mut buf = vec![0u8; 8192];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]);
            let target = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or_default()
                .to_string();
            let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) else {
                continue;
            };
            if url.path() != "/callback" {
                // A favicon or the like.
                let _ = stream
                    .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                    .await;
                continue;
            }
            let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
            let page = |title: &str| {
                format!(
                    "<!doctype html><meta charset=utf-8><title>Anvil</title>\
                     <body style=\"font-family:sans-serif;margin:3em\"><h1>{title}</h1>\
                     <p>You can close this tab and return to Anvil.</p>"
                )
            };
            let (body, result) = match (query.get("code"), query.get("state")) {
                (Some(code), Some(state)) if *state == self.state => {
                    (page("Signed in"), Ok(code.clone()))
                }
                _ => (
                    page("Signing in did not finish"),
                    Err(FcpError::BrowserSignIn(
                        query
                            .get("error")
                            .cloned()
                            .unwrap_or_else(|| "the browser came back without a code".into()),
                    )),
                ),
            };
            let _ = stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await;
            return result;
        }
    }
}

/// Sign in without a browser (`anvil-cli`, tests): the password — and the
/// one-time code when FCP asks for one — as the web sign-in takes them,
/// then a session of the app's own from that fresh sign-in.
pub async fn password_sign_in(
    server: Server,
    client: AppClient,
    username: &str,
    password: &str,
    totp: Option<&str>,
) -> Result<Session, FcpError> {
    let http = http();
    let response = http
        .post(format!("{}/auth/login", server.api_url))
        .json(&serde_json::json!({ "username": username, "password": password }))
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(refused(response).await);
    }
    let mut login: serde_json::Value = response.json().await?;
    if login["mfa_required"].is_string() {
        let code = totp.ok_or(FcpError::TotpRequired)?;
        let response = http
            .post(format!("{}/auth/login/totp", server.api_url))
            .json(&serde_json::json!({ "mfa_token": login["mfa_token"], "code": code }))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(refused(response).await);
        }
        login = response.json().await?;
    }
    let web = login["token"]
        .as_str()
        .ok_or_else(|| FcpError::Invalid("no token in the sign-in's answer".into()))?;
    let response = http
        .post(format!("{}/auth/app/sessions", server.api_url))
        .bearer_auth(web)
        .json(&client)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(refused(response).await);
    }
    let tokens: serde_json::Value = response.json().await?;
    Session::from_tokens(server, client, &tokens, None)
}
