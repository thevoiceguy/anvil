//! The C interface's FCP functions against a running FCP (as anvil-fcp's
//! tests: `ANVIL_FCP_ADMIN`, `ANVIL_FCP_TOKEN`; skipped without them).

use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::mpsc;
use std::time::Duration;

use anvil_ffi::fcp::*;
use anvil_ffi::AnvilStatus;

fn take(out: *mut c_char) -> serde_json::Value {
    let text = unsafe { CStr::from_ptr(out) }.to_str().unwrap().to_string();
    unsafe { anvil_string_free(out) };
    serde_json::from_str(&text).unwrap()
}

unsafe extern "C" fn on_event(name: *const c_char, _body: *const c_char, user_data: *mut c_void) {
    let tx = unsafe { &*(user_data as *const mpsc::Sender<String>) };
    let _ = tx.send(
        unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned(),
    );
}

#[test]
fn the_c_interface_reads_and_changes_the_users_data() {
    let (Ok(admin), Ok(token)) = (
        std::env::var("ANVIL_FCP_ADMIN"),
        std::env::var("ANVIL_FCP_TOKEN"),
    ) else {
        eprintln!("ANVIL_FCP_ADMIN and ANVIL_FCP_TOKEN not set; skipping");
        return;
    };
    // A user to sign in as, in the tenant anvil-fcp's tests use.
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (username, password) = rt.block_on(async {
        use rand::Rng;
        let http = reqwest::Client::new();
        let tenants: serde_json::Value = http
            .get(format!("{admin}/api/v1/tenants"))
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let tenant = tenants["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t.to_string().contains("anvil"))
            .map(|t| t["id"].as_str().unwrap().to_string())
            .expect("the anvil tenant");
        let n: u32 = rand::thread_rng().gen_range(100_000..999_999);
        let (u, p) = (format!("anvil-ffi-{n}"), format!("Anvil-ffi-{n}-pw!"));
        let made = http
            .post(format!("{admin}/api/v1/users"))
            .bearer_auth(&token)
            .header("X-FCP-Tenant-Id", &tenant)
            .json(&serde_json::json!({"username": u, "password": p, "tenant_id": tenant}))
            .send()
            .await
            .unwrap();
        assert!(made.status().is_success());
        (u, p)
    });

    let path = std::env::temp_dir().join(format!("anvil-ffi-{username}.json"));
    let c = |s: &str| CString::new(s).unwrap();
    let (place, user, pw, store) = (
        c(&admin),
        c(&username),
        c(&password),
        c(&path.to_string_lossy()),
    );
    let mut fcp: *mut AnvilFcp = std::ptr::null_mut();
    let status = unsafe {
        anvil_fcp_sign_in(
            place.as_ptr(),
            user.as_ptr(),
            pw.as_ptr(),
            std::ptr::null(),
            store.as_ptr(),
            &mut fcp,
        )
    };
    assert_eq!(status, AnvilStatus::Ok);

    // Live events first, so the settings change below is seen.
    let (tx, rx) = mpsc::channel::<String>();
    let tx = Box::new(tx);
    let status =
        unsafe { anvil_fcp_events_start(fcp, Some(on_event), &*tx as *const _ as *mut c_void) };
    assert_eq!(status, AnvilStatus::Ok);

    let mut out: *mut c_char = std::ptr::null_mut();
    let update = c(r#"{"dnd": true}"#);
    assert_eq!(
        unsafe { anvil_fcp_set_calling(fcp, update.as_ptr(), &mut out) },
        AnvilStatus::Ok
    );
    assert_eq!(take(out)["dnd"], true);
    let mut names = Vec::new();
    while let Ok(name) = rx.recv_timeout(Duration::from_secs(5)) {
        let user_event = name.starts_with("user.");
        names.push(name);
        if user_event {
            break;
        }
    }
    assert!(
        names.iter().any(|n| n.starts_with("user.")),
        "the settings change arrived as a user event: {names:?}"
    );

    let me = c(&username);
    assert_eq!(
        unsafe { anvil_fcp_directory(fcp, me.as_ptr(), std::ptr::null(), &mut out) },
        AnvilStatus::Ok
    );
    let directory = take(out);
    assert!(
        directory["data"].to_string().contains(&username),
        "{directory}"
    );

    assert_eq!(
        unsafe { anvil_fcp_calls(fcp, 10, 0, std::ptr::null(), &mut out) },
        AnvilStatus::Ok
    );
    assert!(take(out)["data"].as_array().unwrap().is_empty());

    // No voicemail box made for this user.
    assert_eq!(
        unsafe { anvil_fcp_voicemail_stats(fcp, &mut out) },
        AnvilStatus::NotFound
    );

    assert_eq!(unsafe { anvil_fcp_events_stop(fcp) }, AnvilStatus::Ok);
    assert_eq!(unsafe { anvil_fcp_sign_out(fcp) }, AnvilStatus::Ok);
    unsafe { anvil_fcp_close(fcp) };
    drop(tx);
    let _ = std::fs::remove_file(path);
}
