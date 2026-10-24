//! Signing in against a stand-in for FCP's sign-in routes: how a person
//! signs in (`/auth/discover`), a password answered with a one-time code
//! wanted, a wrong code tried again, and an FCP without the discover route.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anvil_fcp::{
    begin_password_sign_in, password_sign_in, sign_in_methods, AppClient, FcpError, PasswordStep,
    Server,
};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A route's answer: status and JSON body.
type Answer = (u16, Value);

/// A stand-in FCP: each request's path and JSON body are logged and
/// answered by `route`.
struct StandIn {
    base: String,
    log: Arc<Mutex<Vec<(String, Value)>>>,
}

async fn stand_in(route: impl Fn(&str, &Value) -> Answer + Send + Sync + 'static) -> StandIn {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let log = Arc::new(Mutex::new(Vec::new()));
    let route = Arc::new(route);
    let seen = log.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let route = route.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                // Headers, then as much body as Content-Length says.
                let (head_end, length) = loop {
                    let n = stream.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
                        let length = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while buf.len() < head_end + length {
                    let n = stream.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
                let path = head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                let body: Value = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
                seen.lock().unwrap().push((path.clone(), body.clone()));
                let (status, answer) = route(&path, &body);
                let text = answer.to_string();
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    StandIn { base, log }
}

fn tokens() -> Value {
    json!({
        "access_token": "a", "token_type": "Bearer", "expires_in": 900,
        "refresh_token": "r", "session_id": "sess-1",
        "user": {"username": "ann"},
        "device": {"id": "dev-1", "name": "Anvil", "sip_username": "app-1",
                   "sip_password": "p", "realm": "fcp.local"}
    })
}

/// FCP with a second factor for ann: password `right`, code `123456`.
fn with_totp(path: &str, body: &Value) -> Answer {
    match path {
        "/api/v1/auth/login" if body["password"] == "right" => (
            200,
            json!({"user": {"username": "ann"}, "mfa_required": "totp",
                   "mfa_token": "ticket", "expires_in": 300}),
        ),
        "/api/v1/auth/login" => (
            401,
            json!({"error": "Invalid username or password", "code": "unauthorized"}),
        ),
        "/api/v1/auth/login/totp" if body["mfa_token"] == "ticket" && body["code"] == "123456" => {
            (200, json!({"token": "web", "user": {"username": "ann"}}))
        }
        "/api/v1/auth/login/totp" => (
            401,
            json!({"error": "The code is not right", "code": "unauthorized"}),
        ),
        "/api/v1/auth/app/sessions" => (200, tokens()),
        _ => (404, json!({"error": "not found"})),
    }
}

fn client() -> AppClient {
    AppClient::this_machine("0123456789abcdef0123456789abcdef")
}

#[tokio::test]
async fn a_password_then_a_code_signs_in_and_a_wrong_code_may_be_tried_again() {
    let fcp = stand_in(with_totp).await;
    let server = Server::at(&fcp.base);

    let wrong = begin_password_sign_in(server.clone(), client(), "ann", "wrong").await;
    match wrong {
        Err(FcpError::Rejected(m)) => assert_eq!(m, "Invalid username or password"),
        other => panic!("a wrong password is rejected in FCP's words: {other:?}"),
    }

    let step = begin_password_sign_in(server.clone(), client(), "ann", "right")
        .await
        .unwrap();
    let PasswordStep::CodeNeeded(second) = step else {
        panic!("FCP asked for a code");
    };
    match second.finish("000000").await {
        Err(FcpError::Rejected(m)) => assert_eq!(m, "The code is not right"),
        other => panic!("a wrong code is rejected: {other:?}"),
    }
    let session = second.finish(" 123456 ").await.unwrap();
    assert_eq!(session.username, "ann");
    assert_eq!(session.device.unwrap().sip_username, "app-1");

    let paths: Vec<String> = fcp
        .log
        .lock()
        .unwrap()
        .iter()
        .map(|(p, _)| p.clone())
        .collect();
    assert_eq!(
        paths.iter().filter(|p| p.ends_with("/auth/login")).count(),
        2,
        "the password is sent once per attempt, not again with the code: {paths:?}"
    );
}

#[tokio::test]
async fn without_a_code_the_one_call_sign_in_says_one_is_needed() {
    let fcp = stand_in(with_totp).await;
    let server = Server::at(&fcp.base);
    assert!(matches!(
        password_sign_in(server.clone(), client(), "ann", "right", None).await,
        Err(FcpError::TotpRequired)
    ));
    let session = password_sign_in(server, client(), "ann", "right", Some("123456"))
        .await
        .unwrap();
    assert_eq!(session.session_id, "sess-1");
}

#[tokio::test]
async fn a_second_factor_not_set_up_is_for_the_browser() {
    let fcp = stand_in(|path, _| match path {
        "/api/v1/auth/login" => (
            200,
            json!({"user": {"username": "ann"}, "mfa_required": "totp_enrollment",
                   "mfa_token": "ticket"}),
        ),
        _ => (404, json!({})),
    })
    .await;
    assert!(matches!(
        begin_password_sign_in(Server::at(&fcp.base), client(), "ann", "right").await,
        Err(FcpError::SecondFactorSetup)
    ));
}

#[tokio::test]
async fn fcp_says_how_a_person_signs_in() {
    let answers: HashMap<&str, Value> = HashMap::from([
        (
            "ann@sso.example.com",
            json!({"tenant_name": "SSO Co", "methods": ["sso"],
                   "providers": [{"id": "p1", "name": "Okta"}]}),
        ),
        (
            "bob@both.example.com",
            json!({"methods": ["password", "sso"], "providers": [{"id": "p2", "name": "Entra ID"}]}),
        ),
        ("carol", json!({"methods": ["password"]})),
    ]);
    let fcp = stand_in(move |path, body| match path {
        "/api/v1/auth/discover" => (
            200,
            answers[body["login"].as_str().unwrap_or_default()].clone(),
        ),
        _ => (404, json!({})),
    })
    .await;
    let server = Server::at(&fcp.base);

    let m = sign_in_methods(&server, "ann@sso.example.com")
        .await
        .unwrap()
        .unwrap();
    assert!(!m.password && m.sso);
    assert_eq!(m.tenant_name.as_deref(), Some("SSO Co"));
    let m = sign_in_methods(&server, "bob@both.example.com")
        .await
        .unwrap()
        .unwrap();
    assert!(m.password && m.sso);
    let m = sign_in_methods(&server, " carol ").await.unwrap().unwrap();
    assert!(m.password && !m.sso);
}

#[tokio::test]
async fn an_fcp_without_the_discover_route_says_nothing() {
    let fcp = stand_in(|_, _| (404, json!({"error": "not found"}))).await;
    assert_eq!(
        sign_in_methods(&Server::at(&fcp.base), "ann")
            .await
            .unwrap(),
        None
    );
}
