//! Bridge to DCC apps, client side (SPEC §9.3). OWNER: export.
//!
//! TCP on 127.0.0.1 only. One request per connection: the app connects,
//! sends a `hello` (probe) or `push` frame, and reads one `hello`/`ack` or
//! `error` frame back. The per-install token authenticates every request.

pub mod codec;
pub mod payload;

pub use codec::{Header, MsgType, PROTOCOL_VERSION};
pub use payload::BridgeObject;

use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

#[cfg(target_endian = "big")]
compile_error!("the bridge payload and export writers assume a little-endian target");

/// Default ports (SPEC §9.3).
pub const PORT_BLENDER: u16 = 47110;
pub const PORT_MAYA: u16 = 47111;
pub const PORT_MAX: u16 = 47112;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Host {
    Blender,
    Maya,
    Max,
}

impl Host {
    pub const ALL: [Host; 3] = [Host::Blender, Host::Maya, Host::Max];
    pub fn default_port(self) -> u16 {
        match self {
            Host::Blender => PORT_BLENDER,
            Host::Maya => PORT_MAYA,
            Host::Max => PORT_MAX,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Host::Blender => "Blender",
            Host::Maya => "Maya",
            Host::Max => "3ds Max",
        }
    }
}

/// A new per-install token: 32 lowercase hex characters (128 random bits
/// from the OS). Generate once, store in preferences, paste into each addon.
pub fn generate_token() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("OS random source");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("{0} is not running or the add-on isn't listening on port {1}")]
    NotListening(&'static str, u16),
    #[error("connection: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol: {0}")]
    Codec(#[from] codec::CodecError),
    #[error(
        "the host rejected the token; paste the token from newtopo into the add-on preferences"
    )]
    BadToken,
    #[error(
        "the host speaks protocol version {0}, this app speaks {PROTOCOL_VERSION}; update the add-on"
    )]
    Version(u32),
    #[error("host error: {0}")]
    Host(String),
    #[error("too large for the bridge: {0}")]
    TooLarge(String),
    #[error("unexpected reply: {0:?}")]
    Unexpected(MsgType),
}

/// Result of a push, from the host's `ack`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ack {
    /// One entry per pushed object, in order.
    #[serde(default)]
    pub objects: Vec<AckObject>,
    /// Host-side processing time.
    #[serde(default)]
    pub elapsed_ms: f64,
    #[serde(default)]
    pub host: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AckObject {
    /// Name as sent.
    pub name: String,
    /// Name the host gave it (differs when the host made it unique).
    pub actual: String,
}

impl Ack {
    /// Objects whose name the host changed (Maya suffixes duplicates).
    pub fn renamed(&self) -> impl Iterator<Item = &AckObject> {
        self.objects.iter().filter(|o| o.name != o.actual)
    }
}

/// Connection status for the bridge panel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BridgeStatus {
    /// Nothing listening on the port.
    Offline,
    /// The add-on answered and accepted the token.
    Ready { host: String, addon_version: String },
    /// The add-on answered but rejected the token.
    BadToken,
    /// The add-on speaks another protocol version.
    Incompatible { version: u32 },
    /// Something answered, but not like a newtopo add-on.
    Error(String),
}

/// Options for [`send_with`].
#[derive(Clone, Debug, PartialEq)]
pub struct SendOptions {
    /// Unit scale the host applies (positions and translations).
    pub scale: f64,
    /// Keep same-named objects in the host instead of replacing them.
    pub keep_both: bool,
    pub connect_timeout: Duration,
    /// How long to wait for the host to build the scene and ack.
    pub reply_timeout: Duration,
}

impl Default for SendOptions {
    fn default() -> Self {
        SendOptions {
            scale: 1.0,
            keep_both: false,
            connect_timeout: Duration::from_secs(2),
            reply_timeout: Duration::from_secs(120),
        }
    }
}

fn connect(port: u16, timeout: Duration) -> Result<TcpStream, BridgeError> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    match TcpStream::connect_timeout(&addr, timeout) {
        Ok(s) => {
            s.set_nodelay(true)?;
            Ok(s)
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::TimedOut
            ) =>
        {
            Err(BridgeError::NotListening(host_label(port), port))
        }
        Err(e) => Err(e.into()),
    }
}

fn host_label(port: u16) -> &'static str {
    Host::ALL
        .iter()
        .find(|h| h.default_port() == port)
        .map(|h| h.label())
        .unwrap_or("The host")
}

fn error_of(h: &Header) -> BridgeError {
    let msg = h
        .info
        .as_ref()
        .and_then(|i| i.get("message"))
        .and_then(|m| m.as_str())
        .unwrap_or("unknown error")
        .to_string();
    let code = h
        .info
        .as_ref()
        .and_then(|i| i.get("code"))
        .and_then(|m| m.as_str())
        .unwrap_or("");
    if code == "bad_token" {
        BridgeError::BadToken
    } else if code == "version" {
        BridgeError::Version(h.version)
    } else {
        BridgeError::Host(msg)
    }
}

/// Send objects to the host on `port` (127.0.0.1 only) with default options.
pub fn send(port: u16, token: &str, objects: &[BridgeObject]) -> Result<Ack, BridgeError> {
    send_with(port, token, objects, &SendOptions::default())
}

/// Send objects and wait for the host's ack.
pub fn send_with(
    port: u16,
    token: &str,
    objects: &[BridgeObject],
    opts: &SendOptions,
) -> Result<Ack, BridgeError> {
    let (layout, bytes) = payload::build(objects, opts.scale, opts.keep_both);
    let mut header = Header::new(MsgType::Push, token);
    header.layout = Some(serde_json::to_value(&layout).expect("layout serialises"));
    // The add-ons refuse anything over these limits; say so before sending.
    let header_len = serde_json::to_vec(&header)
        .map(|v| v.len())
        .unwrap_or(usize::MAX);
    if header_len > codec::MAX_HEADER {
        return Err(BridgeError::TooLarge(format!(
            "{} objects need a {header_len}-byte header (limit {}); send fewer objects at once",
            objects.len(),
            codec::MAX_HEADER
        )));
    }
    if bytes.len() as u64 > codec::MAX_PAYLOAD {
        return Err(BridgeError::TooLarge(format!(
            "{} bytes of geometry (limit {}); send fewer objects at once",
            bytes.len(),
            codec::MAX_PAYLOAD
        )));
    }
    let mut s = connect(port, opts.connect_timeout)?;
    s.set_write_timeout(Some(opts.reply_timeout))?;
    s.set_read_timeout(Some(opts.reply_timeout))?;
    let t = Instant::now();
    if let Err(e) = codec::write_frame(&mut s, &header, &bytes) {
        // The host refuses a bad token, version or size right after the
        // header and closes without reading the payload; its error frame
        // may already be here, which explains the failed write.
        return match codec::read_frame_limited(&mut s, codec::MAX_REPLY_PAYLOAD) {
            Ok((reply, _)) if reply.kind == MsgType::Error => Err(error_of(&reply)),
            _ => Err(e.into()),
        };
    }
    let (reply, _) = codec::read_frame_limited(&mut s, codec::MAX_REPLY_PAYLOAD)?;
    log::debug!(
        "bridge push to port {port}: {} bytes, reply after {:?}",
        bytes.len(),
        t.elapsed()
    );
    match reply.kind {
        MsgType::Ack => Ok(reply
            .info
            .map(serde_json::from_value)
            .transpose()
            .map_err(codec::CodecError::Json)?
            .unwrap_or_default()),
        MsgType::Error => Err(error_of(&reply)),
        k => Err(BridgeError::Unexpected(k)),
    }
}

/// Probe the host on `port`: is the add-on listening, and does it accept
/// `token`? Short timeouts; never blocks the UI for long.
pub fn probe(port: u16, token: &str) -> BridgeStatus {
    let run = || -> Result<BridgeStatus, BridgeError> {
        let mut s = connect(port, Duration::from_millis(500))?;
        s.set_read_timeout(Some(Duration::from_secs(2)))?;
        s.set_write_timeout(Some(Duration::from_secs(2)))?;
        codec::write_frame(&mut s, &Header::new(MsgType::Hello, token), &[])?;
        let (reply, _) = codec::read_frame_limited(&mut s, codec::MAX_REPLY_PAYLOAD)?;
        let info = reply.info.clone().unwrap_or_default();
        let field = |k: &str| {
            info.get(k)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        Ok(match reply.kind {
            MsgType::Hello if reply.version == PROTOCOL_VERSION => BridgeStatus::Ready {
                host: field("host"),
                addon_version: field("addon_version"),
            },
            MsgType::Hello => BridgeStatus::Incompatible {
                version: reply.version,
            },
            MsgType::Error => match error_of(&reply) {
                BridgeError::BadToken => BridgeStatus::BadToken,
                BridgeError::Version(v) => BridgeStatus::Incompatible { version: v },
                e => BridgeStatus::Error(e.to_string()),
            },
            k => BridgeStatus::Error(format!("unexpected reply {k:?}")),
        })
    };
    match run() {
        Ok(s) => s,
        Err(BridgeError::NotListening(..)) => BridgeStatus::Offline,
        Err(e) => BridgeStatus::Error(e.to_string()),
    }
}

#[cfg(test)]
mod tests;
