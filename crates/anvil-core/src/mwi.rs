//! Message waiting (RFC 3842): an `application/simple-message-summary`
//! body read into counts, from an unsolicited NOTIFY or one inside a
//! `message-summary` subscription.

/// The event package.
pub const EVENT: &str = "message-summary";

/// What a mailbox holds, as a message summary says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MessageSummary {
    /// `Messages-Waiting: yes`.
    pub waiting: bool,
    pub new: u32,
    pub old: u32,
    pub urgent_new: u32,
    pub urgent_old: u32,
}

/// Read a summary body; `None` without a `Messages-Waiting` line.
pub fn parse(body: &str) -> Option<MessageSummary> {
    let mut summary = MessageSummary::default();
    let mut seen = false;
    for line in body.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "messages-waiting" => {
                seen = true;
                summary.waiting = value.eq_ignore_ascii_case("yes");
            }
            // `Voice-Message: 2/5 (1/0)`; other media lines are not counted.
            "voice-message" => {
                let (counts, urgent) = match value.split_once('(') {
                    Some((c, u)) => (c.trim(), Some(u.trim_end_matches(')').trim())),
                    None => (value, None),
                };
                let pair = |s: &str| -> (u32, u32) {
                    let (a, b) = s.split_once('/').unwrap_or((s, "0"));
                    (a.trim().parse().unwrap_or(0), b.trim().parse().unwrap_or(0))
                };
                (summary.new, summary.old) = pair(counts);
                if let Some(u) = urgent {
                    (summary.urgent_new, summary.urgent_old) = pair(u);
                }
            }
            _ => {}
        }
    }
    seen.then_some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_is_read() {
        let s = parse("Messages-Waiting: yes\r\nMessage-Account: sip:alice@a.test\r\nVoice-Message: 2/5 (1/0)\r\n").unwrap();
        assert_eq!(
            s,
            MessageSummary {
                waiting: true,
                new: 2,
                old: 5,
                urgent_new: 1,
                urgent_old: 0
            }
        );
        let none = parse("Messages-Waiting: no\r\n").unwrap();
        assert!(!none.waiting && none.new == 0);
        assert_eq!(parse("Voice-Message: 1/0\r\n"), None, "no Messages-Waiting");
        assert_eq!(
            parse("Messages-Waiting: yes\r\nVoice-Message: 3/1\r\n")
                .unwrap()
                .old,
            1
        );
    }
}
