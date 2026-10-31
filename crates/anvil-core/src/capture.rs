//! Call captures (FCP's `docs/CALL_DIAGNOSTICS.md` §3): a call as this
//! phone saw it, written as pcapng for Wireshark.
//!
//! - SIP is taken where it is still plain: as the dispatcher sends it,
//!   before TLS, and as the packet pump reads it, after TLS. Every message
//!   is written as a UDP datagram between the real addresses and ports, so
//!   Wireshark's SIP heuristic reads it whatever it travelled over; a
//!   packet comment names the transport it did.
//! - RTP and RTCP are written as they are once SRTP is off them: sent
//!   before SRTP is applied, received after it is removed. pcapng has no
//!   secrets block for SRTP and Wireshark does not decrypt it, so this is
//!   how the audio plays; the headers, sequence numbers and timing are the
//!   wire's (SRTP encrypts only the payload and adds a tag), and the keys
//!   are in the SDP the capture holds. A packet that fails SRTP is written
//!   as it arrived, so a key mismatch shows.
//! - The file's section header names the app, the account and the call.
//!
//! A capture stops writing at its size limit and says so in its last
//! packet's comment; one too big is never what the app uploads.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;

/// Raw IP, version by the first nibble (LINKTYPE_RAW).
const LINKTYPE_RAW: u16 = 101;
const SHB: u32 = 0x0A0D_0D0A;
const IDB: u32 = 0x0000_0001;
const EPB: u32 = 0x0000_0006;
const BYTE_ORDER_MAGIC: u32 = 0x1A2B_3C4D;
const OPT_END: u16 = 0;
const OPT_COMMENT: u16 = 1;
const SHB_HARDWARE: u16 = 2;
const SHB_OS: u16 = 3;
const SHB_USERAPPL: u16 = 4;
const IF_NAME: u16 = 2;
const EPB_FLAGS: u16 = 2;
/// The last packet of a capture at its limit says so.
const LIMIT_NOTE: &str = "capture limit reached: nothing after this was written";
/// What that packet takes: 64 octets of it, the note, the block around them.
const LIMIT_NOTE_ROOM: u64 = 64 + LIMIT_NOTE.len() as u64 + 48;
/// The largest UDP payload a synthesised datagram may carry.
const MAX_DATAGRAM: usize = 65_507;

/// Which way a packet went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Inbound,
    Outbound,
}

/// What a capture is of, for its section header.
#[derive(Debug, Clone, Default)]
pub struct CaptureInfo {
    /// The app and its version (`Anvil 0.2.30`).
    pub application: String,
    /// The account's address of record.
    pub account: String,
    /// The phone's local call, and the server's id for it when known.
    pub call: String,
}

/// One capture being written.
pub struct Capture {
    path: PathBuf,
    max_bytes: u64,
    inner: Mutex<Writer>,
}

struct Writer {
    out: Option<BufWriter<File>>,
    written: u64,
    full: bool,
}

impl std::fmt::Debug for Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capture")
            .field("path", &self.path)
            .field("max_bytes", &self.max_bytes)
            .finish_non_exhaustive()
    }
}

impl Capture {
    /// Start a capture at `path`, at most `max_bytes` long.
    pub fn create(path: &Path, max_bytes: u64, info: &CaptureInfo) -> std::io::Result<Arc<Self>> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut out = BufWriter::new(File::create(path)?);
        let mut head = section_header(info);
        head.extend(interface_description());
        out.write_all(&head)?;
        Ok(Arc::new(Self {
            path: path.to_path_buf(),
            max_bytes,
            inner: Mutex::new(Writer {
                out: Some(out),
                written: head.len() as u64,
                full: false,
            }),
        }))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file's length so far.
    pub fn len(&self) -> u64 {
        self.inner.lock().written
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether it reached its limit and stopped.
    pub fn is_full(&self) -> bool {
        self.inner.lock().full
    }

    /// A SIP message as it was before TLS (sent) or after it (received),
    /// over `transport` (`udp`, `tcp`, `tls`).
    pub fn sip(
        &self,
        direction: Direction,
        transport: &str,
        local: SocketAddr,
        remote: SocketAddr,
        message: &[u8],
    ) {
        let comment = (transport != "udp").then(|| format!("SIP over {transport}"));
        self.datagram(direction, local, remote, message, comment.as_deref());
    }

    /// An RTP or RTCP packet, `note` saying anything unusual about it.
    pub fn media(
        &self,
        direction: Direction,
        local: SocketAddr,
        remote: SocketAddr,
        packet: &[u8],
        note: Option<&str>,
    ) {
        self.datagram(direction, local, remote, packet, note);
    }

    fn datagram(
        &self,
        direction: Direction,
        local: SocketAddr,
        remote: SocketAddr,
        payload: &[u8],
        comment: Option<&str>,
    ) {
        self.datagram_at(
            SystemTime::now(),
            direction,
            local,
            remote,
            payload,
            comment,
        );
    }

    fn datagram_at(
        &self,
        at: SystemTime,
        direction: Direction,
        local: SocketAddr,
        remote: SocketAddr,
        payload: &[u8],
        comment: Option<&str>,
    ) {
        let (src, dst) = match direction {
            Direction::Outbound => (local, remote),
            Direction::Inbound => (remote, local),
        };
        let payload = &payload[..payload.len().min(MAX_DATAGRAM)];
        let Some(frame) = ip_udp(src, dst, payload) else {
            return;
        };
        let mut w = self.inner.lock();
        if w.full {
            return;
        }
        let mut block = packet_block(at, &frame, direction, comment);
        // Room is kept for the last packet, which says why the rest is not.
        if w.written + block.len() as u64 + LIMIT_NOTE_ROOM > self.max_bytes {
            w.full = true;
            block = packet_block(
                at,
                &frame[..frame.len().min(64)],
                direction,
                Some(LIMIT_NOTE),
            );
            if w.written + block.len() as u64 > self.max_bytes {
                return;
            }
        }
        let ok = w
            .out
            .as_mut()
            .is_some_and(|out| out.write_all(&block).is_ok());
        if ok {
            w.written += block.len() as u64;
        } else {
            w.out = None;
            w.full = true;
        }
    }

    /// Flush and close; the file's length.
    pub fn finish(&self) -> u64 {
        let mut w = self.inner.lock();
        if let Some(mut out) = w.out.take() {
            let _ = out.flush();
        }
        w.written
    }
}

fn pad4(len: usize) -> usize {
    (4 - len % 4) % 4
}

fn option(out: &mut Vec<u8>, code: u16, value: &[u8]) {
    out.extend(code.to_le_bytes());
    out.extend((value.len() as u16).to_le_bytes());
    out.extend(value);
    out.extend(std::iter::repeat_n(0u8, pad4(value.len())));
}

/// A block: type, length, body, length again.
fn block(kind: u32, body: &[u8]) -> Vec<u8> {
    let len = (12 + body.len()) as u32;
    let mut out = Vec::with_capacity(len as usize);
    out.extend(kind.to_le_bytes());
    out.extend(len.to_le_bytes());
    out.extend(body);
    out.extend(len.to_le_bytes());
    out
}

fn section_header(info: &CaptureInfo) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(BYTE_ORDER_MAGIC.to_le_bytes());
    body.extend(1u16.to_le_bytes());
    body.extend(0u16.to_le_bytes());
    body.extend((-1i64).to_le_bytes());
    let comment = format!(
        "Call {} of {}. SIP is as it was inside TLS, written as UDP; RTP and RTCP \
         are as they were outside SRTP (the keys are in the SDP). In Wireshark, \
         turn off SDP's \"Establish Media Conversation\" and turn on RTP's \
         heuristic (Preferences > Protocols), or the media is shown as SRTP and \
         does not play.",
        info.call, info.account
    );
    option(&mut body, OPT_COMMENT, comment.as_bytes());
    option(&mut body, SHB_HARDWARE, std::env::consts::ARCH.as_bytes());
    option(&mut body, SHB_OS, std::env::consts::OS.as_bytes());
    option(&mut body, SHB_USERAPPL, info.application.as_bytes());
    option(&mut body, OPT_END, &[]);
    block(SHB, &body)
}

fn interface_description() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(LINKTYPE_RAW.to_le_bytes());
    body.extend(0u16.to_le_bytes());
    body.extend(0u32.to_le_bytes());
    option(&mut body, IF_NAME, b"anvil");
    option(&mut body, OPT_END, &[]);
    block(IDB, &body)
}

fn packet_block(
    at: SystemTime,
    frame: &[u8],
    direction: Direction,
    comment: Option<&str>,
) -> Vec<u8> {
    let micros = at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;
    let mut body = Vec::with_capacity(frame.len() + 48);
    body.extend(0u32.to_le_bytes());
    body.extend(((micros >> 32) as u32).to_le_bytes());
    body.extend((micros as u32).to_le_bytes());
    body.extend((frame.len() as u32).to_le_bytes());
    body.extend((frame.len() as u32).to_le_bytes());
    body.extend(frame);
    body.extend(std::iter::repeat_n(0u8, pad4(frame.len())));
    let flags: u32 = match direction {
        Direction::Inbound => 1,
        Direction::Outbound => 2,
    };
    option(&mut body, EPB_FLAGS, &flags.to_le_bytes());
    if let Some(comment) = comment {
        option(&mut body, OPT_COMMENT, comment.as_bytes());
    }
    option(&mut body, OPT_END, &[]);
    block(EPB, &body)
}

/// `payload` as a UDP datagram from `src` to `dst`, in an IP packet of the
/// addresses' family; `None` when the two are of different families.
fn ip_udp(src: SocketAddr, dst: SocketAddr, payload: &[u8]) -> Option<Vec<u8>> {
    let udp_len = 8 + payload.len();
    let mut udp = Vec::with_capacity(udp_len);
    udp.extend(src.port().to_be_bytes());
    udp.extend(dst.port().to_be_bytes());
    udp.extend((udp_len as u16).to_be_bytes());
    udp.extend([0, 0]);
    udp.extend(payload);
    match (src.ip(), dst.ip()) {
        (IpAddr::V4(s), IpAddr::V4(d)) => {
            let total = 20 + udp_len;
            let mut ip = Vec::with_capacity(total);
            ip.extend([0x45, 0]);
            ip.extend((total as u16).to_be_bytes());
            ip.extend([0, 0, 0x40, 0, 64, 17, 0, 0]);
            ip.extend(s.octets());
            ip.extend(d.octets());
            let sum = checksum(&ip, 0);
            ip[10..12].copy_from_slice(&sum.to_be_bytes());
            // IPv4's UDP checksum may be zero: none.
            ip.extend(udp);
            Some(ip)
        }
        (IpAddr::V6(s), IpAddr::V6(d)) => {
            // IPv6 requires the UDP checksum, over a pseudo-header.
            let mut pseudo = Vec::with_capacity(40);
            pseudo.extend(s.octets());
            pseudo.extend(d.octets());
            pseudo.extend((udp_len as u32).to_be_bytes());
            pseudo.extend([0, 0, 0, 17]);
            let start = pseudo_sum(&pseudo);
            let sum = match checksum(&udp, start) {
                0 => 0xffff,
                s => s,
            };
            udp[6..8].copy_from_slice(&sum.to_be_bytes());
            let mut ip = Vec::with_capacity(40 + udp_len);
            ip.extend([0x60, 0, 0, 0]);
            ip.extend((udp_len as u16).to_be_bytes());
            ip.extend([17, 64]);
            ip.extend(s.octets());
            ip.extend(d.octets());
            ip.extend(udp);
            Some(ip)
        }
        _ => None,
    }
}

fn pseudo_sum(bytes: &[u8]) -> u32 {
    bytes
        .chunks(2)
        .map(|c| u32::from(u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])))
        .sum()
}

/// The Internet checksum of `bytes`, starting from `start`.
fn checksum(bytes: &[u8], start: u32) -> u16 {
    let mut sum = start + pseudo_sum(bytes);
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// A call's capture, when one is taken, shared by its media tasks.
pub(crate) type CaptureSlot = Arc<Mutex<Option<Arc<Capture>>>>;

/// A media packet into the call's capture, if it has one: `socket`'s
/// address is the phone's side.
pub(crate) fn tap(
    slot: &CaptureSlot,
    direction: Direction,
    socket: &tokio::net::UdpSocket,
    remote: SocketAddr,
    packet: &[u8],
    note: Option<&str>,
) {
    let Some(capture) = slot.lock().clone() else {
        return;
    };
    if let Ok(local) = socket.local_addr() {
        capture.media(direction, local, remote, packet, note);
    }
}

/// What captures are taken: where, how large, and for which calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureSettings {
    /// Where capture files are written.
    pub dir: PathBuf,
    /// The largest a capture may grow.
    pub max_bytes: u64,
    /// Every call until turned off; else the next call only.
    pub every_call: bool,
    /// The app and its version, for the file's header.
    pub application: String,
}

/// A finished capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishedCapture {
    pub path: PathBuf,
    pub bytes: u64,
    /// It reached its limit: the end of the call is not in it.
    pub full: bool,
}

/// How far back a new capture starts: the INVITE that made its call was
/// read, or sent, before the call existed.
const LOOKBACK: std::time::Duration = std::time::Duration::from_secs(10);
/// The most SIP messages kept for that.
const LOOKBACK_MESSAGES: usize = 64;

struct Recent {
    at: SystemTime,
    direction: Direction,
    transport: &'static str,
    remote: SocketAddr,
    message: Vec<u8>,
}

/// The captures being taken, one per call that asked for one; SIP goes to
/// every one of them, a call's media to its own.
#[derive(Default)]
pub(crate) struct Captures {
    settings: Mutex<Option<CaptureSettings>>,
    active: Mutex<Vec<(u64, Arc<Capture>)>>,
    /// The last few seconds of SIP, while captures are on.
    recent: Mutex<std::collections::VecDeque<Recent>>,
    /// The phone's SIP address: where its messages are written from.
    pub(crate) local_sip: Mutex<Option<SocketAddr>>,
}

impl Captures {
    pub(crate) fn set(&self, settings: Option<CaptureSettings>) {
        if settings.is_none() {
            self.recent.lock().clear();
        }
        *self.settings.lock() = settings;
    }

    pub(crate) fn settings(&self) -> Option<CaptureSettings> {
        self.settings.lock().clone()
    }

    /// Start a capture of `call` when captures are on, with the last few
    /// seconds of SIP in it; the next-call-only setting is then spent.
    pub(crate) fn begin(&self, call: u64, account: &str) -> Option<Arc<Capture>> {
        let settings = {
            let mut slot = self.settings.lock();
            let settings = slot.clone()?;
            if !settings.every_call {
                *slot = None;
            }
            settings
        };
        let stamp = chrono_stamp(SystemTime::now());
        let path = settings.dir.join(format!("{stamp}-call{call}.pcapng"));
        let info = CaptureInfo {
            application: settings.application.clone(),
            account: account.to_string(),
            call: format!("{call}"),
        };
        let capture = match Capture::create(&path, settings.max_bytes, &info) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(%e, path = %path.display(), "a capture could not be started");
                return None;
            }
        };
        let since = SystemTime::now() - LOOKBACK;
        for r in self.recent.lock().iter().filter(|r| r.at >= since) {
            let local = self
                .local_for(Some(r.remote))
                .unwrap_or_else(|| SocketAddr::new(unspecified_like(r.remote.ip()), 0));
            let comment = (r.transport != "udp").then(|| format!("SIP over {}", r.transport));
            capture.datagram_at(
                r.at,
                r.direction,
                local,
                r.remote,
                &r.message,
                comment.as_deref(),
            );
        }
        self.active.lock().push((call, Arc::clone(&capture)));
        tracing::info!(call, path = %path.display(), "capturing the call");
        Some(capture)
    }

    /// Stop capturing `call`: the file closed.
    pub(crate) fn finish(&self, call: u64) -> Option<FinishedCapture> {
        let capture = {
            let mut active = self.active.lock();
            let at = active.iter().position(|(c, _)| *c == call)?;
            active.remove(at).1
        };
        let bytes = capture.finish();
        Some(FinishedCapture {
            path: capture.path().to_path_buf(),
            bytes,
            full: capture.is_full(),
        })
    }

    pub(crate) fn is_capturing(&self, call: u64) -> bool {
        self.active.lock().iter().any(|(c, _)| *c == call)
    }

    /// Every capture closed (the softphone shutting down).
    pub(crate) fn finish_all(&self) {
        for (_, capture) in self.active.lock().drain(..) {
            capture.finish();
        }
    }

    /// The phone's address, in the family of `remote` (unspecified when
    /// the phone's is of the other).
    fn local_for(&self, remote: Option<SocketAddr>) -> Option<SocketAddr> {
        let local = (*self.local_sip.lock())?;
        Some(match remote {
            Some(r) if r.is_ipv4() != local.is_ipv4() => {
                SocketAddr::new(unspecified_like(r.ip()), local.port())
            }
            _ => local,
        })
    }

    /// A SIP message, to every capture being taken, and kept a few seconds
    /// for one about to start.
    pub(crate) fn sip(
        &self,
        direction: Direction,
        transport: &'static str,
        remote: SocketAddr,
        message: &[u8],
    ) {
        if self.settings.lock().is_none() && self.active.lock().is_empty() {
            return;
        }
        let local = self
            .local_for(Some(remote))
            .unwrap_or_else(|| SocketAddr::new(unspecified_like(remote.ip()), 0));
        for (_, capture) in self.active.lock().iter() {
            capture.sip(direction, transport, local, remote, message);
        }
        if self.settings.lock().is_some() {
            let mut recent = self.recent.lock();
            let since = SystemTime::now() - LOOKBACK;
            while recent
                .front()
                .is_some_and(|r| r.at < since || recent.len() >= LOOKBACK_MESSAGES)
            {
                recent.pop_front();
            }
            recent.push_back(Recent {
                at: SystemTime::now(),
                direction,
                transport,
                remote,
                message: message.to_vec(),
            });
        }
    }
}

/// `20261031-142501`, in UTC: a capture's file name sorts by its start.
fn chrono_stamp(at: SystemTime) -> String {
    let secs = at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Days since 1970 to a civil date (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

fn unspecified_like(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(_) => IpAddr::from([0, 0, 0, 0]),
        IpAddr::V6(_) => IpAddr::from([0u16; 8]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of the test's own, removed when it is dropped.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("anvil-capture-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn read_u32(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    /// The blocks of a pcapng file: (type, body).
    fn blocks(file: &[u8]) -> Vec<(u32, Vec<u8>)> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < file.len() {
            let kind = read_u32(file, at);
            let len = read_u32(file, at + 4) as usize;
            assert_eq!(len % 4, 0, "a block is 32-bit aligned");
            assert_eq!(
                read_u32(file, at + len - 4) as usize,
                len,
                "trailing length"
            );
            out.push((kind, file[at + 8..at + len - 4].to_vec()));
            at += len;
        }
        out
    }

    #[test]
    fn a_capture_is_pcapng_with_sip_and_media_as_ip_datagrams() {
        let dir = TempDir::new("datagrams");
        let path = dir.path().join("call.pcapng");
        let info = CaptureInfo {
            application: "Anvil test".into(),
            account: "sip:carol@example.com".into(),
            call: "call-1".into(),
        };
        let capture = Capture::create(&path, 1 << 20, &info).unwrap();
        let phone: SocketAddr = "192.168.1.20:5060".parse().unwrap();
        let server: SocketAddr = "203.0.113.5:5061".parse().unwrap();
        let invite = b"INVITE sip:bob@example.com SIP/2.0\r\n\r\n";
        capture.sip(Direction::Outbound, "tls", phone, server, invite);
        capture.media(
            Direction::Inbound,
            "192.168.1.20:40000".parse().unwrap(),
            "203.0.113.5:30000".parse().unwrap(),
            &[0x80, 0, 0, 1],
            None,
        );
        let v6 = Capture::create(&dir.path().join("v6.pcapng"), 1 << 20, &info).unwrap();
        v6.sip(
            Direction::Inbound,
            "udp",
            "[2001:db8::20]:5060".parse().unwrap(),
            "[2001:db8::5]:5060".parse().unwrap(),
            b"SIP/2.0 200 OK\r\n\r\n",
        );
        let len = capture.finish();
        v6.finish();

        let file = std::fs::read(&path).unwrap();
        assert_eq!(file.len() as u64, len);
        assert_eq!(file[..4], crate::capture::SHB.to_le_bytes());
        let all = blocks(&file);
        assert_eq!(
            all.iter().map(|b| b.0).collect::<Vec<_>>(),
            vec![SHB, IDB, EPB, EPB]
        );
        assert_eq!(read_u32(&all[0].1, 0), BYTE_ORDER_MAGIC);
        let shb = String::from_utf8_lossy(&all[0].1);
        assert!(
            shb.contains("call-1") && shb.contains("Anvil test"),
            "{shb}"
        );
        assert_eq!(
            u16::from_le_bytes(all[1].1[0..2].try_into().unwrap()),
            LINKTYPE_RAW
        );

        // The INVITE: IPv4, UDP, from the phone to the server, the message
        // whole, the header's checksum right, marked outbound over TLS.
        let epb = &all[2].1;
        let caplen = read_u32(epb, 12) as usize;
        let frame = &epb[20..20 + caplen];
        assert_eq!(frame[0], 0x45);
        assert_eq!(frame[9], 17, "UDP");
        assert_eq!(checksum(&frame[..20], 0), 0, "a valid IPv4 header");
        assert_eq!(&frame[12..16], &[192, 168, 1, 20]);
        assert_eq!(&frame[16..20], &[203, 0, 113, 5]);
        assert_eq!(u16::from_be_bytes([frame[20], frame[21]]), 5060);
        assert_eq!(u16::from_be_bytes([frame[22], frame[23]]), 5061);
        assert_eq!(&frame[28..], invite);
        let options = String::from_utf8_lossy(&epb[20 + caplen..]);
        assert!(options.contains("SIP over tls"), "{options}");
        // The RTP packet comes from the server's media address.
        let rtp = &all[3].1;
        let frame = &rtp[20..20 + read_u32(rtp, 12) as usize];
        assert_eq!(&frame[12..16], &[203, 0, 113, 5]);
        assert_eq!(&frame[28..], &[0x80, 0, 0, 1]);

        // IPv6: a version-6 header, and a UDP checksum that sums to zero.
        let file = std::fs::read(dir.path().join("v6.pcapng")).unwrap();
        let epb = blocks(&file)[2].1.clone();
        let frame = &epb[20..20 + read_u32(&epb, 12) as usize];
        assert_eq!(frame[0] >> 4, 6);
        let mut pseudo = frame[8..40].to_vec();
        pseudo.extend(((frame.len() - 40) as u32).to_be_bytes());
        pseudo.extend([0, 0, 0, 17]);
        assert_eq!(checksum(&frame[40..], pseudo_sum(&pseudo)), 0);
    }

    #[test]
    fn a_capture_stops_at_its_limit_and_says_so() {
        let dir = TempDir::new("full");
        let path = dir.path().join("full.pcapng");
        let capture = Capture::create(&path, 2_000, &CaptureInfo::default()).unwrap();
        let (a, b): (SocketAddr, SocketAddr) = (
            "10.0.0.1:4000".parse().unwrap(),
            "10.0.0.2:4000".parse().unwrap(),
        );
        for _ in 0..20 {
            capture.media(Direction::Outbound, a, b, &[0u8; 172], None);
        }
        assert!(capture.is_full());
        let len = capture.finish();
        assert!(len <= 2_000, "{len}");
        let file = std::fs::read(&path).unwrap();
        let last = blocks(&file).pop().unwrap();
        assert!(String::from_utf8_lossy(&last.1).contains("capture limit reached"));
    }

    #[test]
    fn a_new_capture_starts_with_the_last_seconds_of_sip_and_the_next_call_only_once() {
        let dir = TempDir::new("begin");
        let captures = Captures::default();
        *captures.local_sip.lock() = Some("192.168.1.20:5060".parse().unwrap());
        let server: SocketAddr = "203.0.113.5:5060".parse().unwrap();
        // Off: nothing kept.
        captures.sip(Direction::Inbound, "udp", server, b"OPTIONS before\r\n\r\n");
        assert!(captures.begin(1, "sip:carol@example.com").is_none());
        captures.set(Some(CaptureSettings {
            dir: dir.path().to_path_buf(),
            max_bytes: 1 << 20,
            every_call: false,
            application: "Anvil test".into(),
        }));
        captures.sip(
            Direction::Inbound,
            "udp",
            server,
            b"INVITE sip:carol SIP/2.0\r\n\r\n",
        );
        let capture = captures
            .begin(7, "sip:carol@example.com")
            .expect("a capture");
        assert!(captures.is_capturing(7));
        assert_eq!(captures.settings(), None, "the next call only");
        assert!(captures.begin(8, "sip:carol@example.com").is_none());
        captures.sip(
            Direction::Outbound,
            "udp",
            server,
            b"SIP/2.0 180 Ringing\r\n\r\n",
        );
        let done = captures.finish(7).expect("finished");
        assert!(!captures.is_capturing(7));
        assert!(!done.full);
        assert_eq!(done.path, capture.path());
        let name = done
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert!(
            name.ends_with("-call7.pcapng") && name.len() == 28,
            "{name}"
        );
        let file = std::fs::read(&done.path).unwrap();
        assert_eq!(file.len() as u64, done.bytes);
        let text = String::from_utf8_lossy(&file);
        assert!(text.contains("INVITE sip:carol") && text.contains("180 Ringing"));
        assert!(
            !text.contains("OPTIONS before"),
            "nothing from before captures were on"
        );
    }

    #[test]
    fn a_file_name_is_its_start_in_utc() {
        let at = UNIX_EPOCH + std::time::Duration::from_secs(1_793_463_627);
        assert_eq!(chrono_stamp(at), "20261031-162027");
    }
}
