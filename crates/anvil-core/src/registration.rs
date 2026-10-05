//! Keeping a registration alive: what the registrar granted, and when to
//! ask again.

use std::time::Duration;

use sip_core::Response;

/// The expiry, in seconds, a registrar granted in its 2xx: the `expires`
/// parameter on a Contact (RFC 3261 §10.2.4; we register one contact), else
/// the `Expires` header, else what was asked for.
pub(crate) fn granted_expires(response: &Response, asked: u32) -> u32 {
    let from_contact = response
        .headers()
        .get_all("Contact")
        .flat_map(|value| value.split(','))
        .find_map(|contact| {
            contact.split(';').skip(1).find_map(|param| {
                let (name, value) = param.split_once('=')?;
                if name.trim().eq_ignore_ascii_case("expires") {
                    value.trim().trim_matches('"').parse::<u32>().ok()
                } else {
                    None
                }
            })
        });
    from_contact
        .or_else(|| {
            response
                .headers()
                .get("Expires")
                .and_then(|v| v.trim().parse::<u32>().ok())
        })
        .unwrap_or(asked)
}

/// The `Min-Expires` of a 423 Interval Too Brief.
pub(crate) fn min_expires(response: &Response) -> Option<u32> {
    response
        .headers()
        .get("Min-Expires")
        .and_then(|v| v.trim().parse::<u32>().ok())
}

/// When to refresh a registration granted for `granted` seconds: with a
/// tenth of it to spare (at least half of it, and never sooner than five
/// seconds), so a slow answer still lands before it lapses.
pub(crate) fn refresh_after(granted: u32) -> Duration {
    let granted = u64::from(granted);
    let spare = (granted / 10).max(5);
    let at = if granted > spare * 2 {
        granted - spare
    } else {
        granted / 2
    };
    Duration::from_secs(at.max(5))
}

/// How long to wait before the `failures`th retry of a failed refresh:
/// 30 seconds, doubling, at most 5 minutes.
pub(crate) fn retry_after(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(4);
    Duration::from_secs((30u64 << doublings).min(300))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    fn response(headers: &str) -> Response {
        let raw = format!(
            "SIP/2.0 200 OK\r\nVia: SIP/2.0/UDP 127.0.0.1:5060;branch=z9hG4bK1\r\n\
             From: <sip:alice@example.com>;tag=a\r\nTo: <sip:alice@example.com>;tag=b\r\n\
             Call-ID: reg-1\r\nCSeq: 1 REGISTER\r\n{headers}Content-Length: 0\r\n\r\n"
        );
        sip_parse::parse_response(&Bytes::from(raw)).expect("a valid response")
    }

    #[test]
    fn the_contact_expires_is_what_was_granted() {
        let r = response("Contact: <sip:alice@192.0.2.1:5060>;expires=600\r\nExpires: 3600\r\n");
        assert_eq!(granted_expires(&r, 3600), 600);
    }

    #[test]
    fn the_expires_header_is_used_without_a_contact_parameter() {
        let r = response("Contact: <sip:alice@192.0.2.1:5060>\r\nExpires: 1800\r\n");
        assert_eq!(granted_expires(&r, 3600), 1800);
    }

    #[test]
    fn what_was_asked_is_assumed_when_the_registrar_says_nothing() {
        let r = response("");
        assert_eq!(granted_expires(&r, 3600), 3600);
    }

    #[test]
    fn a_423_says_the_shortest_expiry_allowed() {
        let r = response("Min-Expires: 300\r\n");
        assert_eq!(min_expires(&r), Some(300));
        assert_eq!(min_expires(&response("")), None);
    }

    #[test]
    fn a_refresh_leaves_time_to_spare() {
        assert_eq!(refresh_after(3600), Duration::from_secs(3240));
        assert_eq!(refresh_after(120), Duration::from_secs(108));
        assert_eq!(refresh_after(60), Duration::from_secs(54));
        assert_eq!(refresh_after(10), Duration::from_secs(5));
        assert_eq!(refresh_after(0), Duration::from_secs(5));
    }

    #[test]
    fn a_failed_refresh_backs_off_to_five_minutes() {
        let waits: Vec<u64> = (1..=7).map(|n| retry_after(n).as_secs()).collect();
        assert_eq!(waits, vec![30, 60, 120, 240, 300, 300, 300]);
    }
}
