//! Frame codec (SPEC §9.3): `header_len` (u32 LE) + UTF-8 JSON header +
//! `payload_len` bytes of binary payload.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};

/// Protocol version spoken by this build.
pub const PROTOCOL_VERSION: u32 = 1;
/// Largest JSON header (the push layout lives in it; ~10k objects fit).
/// Same limit as the add-ons' `protocol.py`.
pub const MAX_HEADER: usize = 4 << 20;
/// Largest payload (same limit as the add-ons).
pub const MAX_PAYLOAD: u64 = 512 << 20;
/// Largest payload accepted in a reply (hello/ack/error carry none).
pub const MAX_REPLY_PAYLOAD: u64 = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MsgType {
    Hello,
    Push,
    Ack,
    Error,
}

/// JSON header of a frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub version: u32,
    #[serde(rename = "type")]
    pub kind: MsgType,
    pub token: String,
    pub payload_len: u64,
    /// Payload layout (push only), see [`super::payload::Layout`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Value>,
    /// Free-form details: hello host info, ack results, error message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<Value>,
}

impl Header {
    pub fn new(kind: MsgType, token: &str) -> Self {
        Header {
            version: PROTOCOL_VERSION,
            kind,
            token: token.to_string(),
            payload_len: 0,
            layout: None,
            info: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("bad header JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("frame too large ({0} bytes)")]
    TooLarge(u64),
}

/// Encode one frame. `header.payload_len` is set from `payload`.
pub fn encode(header: &Header, payload: &[u8]) -> Vec<u8> {
    let mut h = header.clone();
    h.payload_len = payload.len() as u64;
    let js = serde_json::to_vec(&h).expect("header serialises");
    let mut out = Vec::with_capacity(4 + js.len() + payload.len());
    out.extend_from_slice(&(js.len() as u32).to_le_bytes());
    out.extend_from_slice(&js);
    out.extend_from_slice(payload);
    out
}

/// Write one frame.
pub fn write_frame(w: &mut impl Write, header: &Header, payload: &[u8]) -> Result<(), CodecError> {
    let mut h = header.clone();
    h.payload_len = payload.len() as u64;
    let js = serde_json::to_vec(&h)?;
    w.write_all(&(js.len() as u32).to_le_bytes())?;
    w.write_all(&js)?;
    w.write_all(payload)?;
    w.flush()?;
    Ok(())
}

/// Read one frame (blocking), payload up to [`MAX_PAYLOAD`].
pub fn read_frame(r: &mut impl Read) -> Result<(Header, Vec<u8>), CodecError> {
    read_frame_limited(r, MAX_PAYLOAD)
}

/// Read one frame, refusing a declared payload over `max_payload` right
/// after the header. The payload buffer grows with the bytes that actually
/// arrive, so a large declared size allocates nothing up front.
pub fn read_frame_limited(
    r: &mut impl Read,
    max_payload: u64,
) -> Result<(Header, Vec<u8>), CodecError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let hl = u32::from_le_bytes(len) as usize;
    if hl > MAX_HEADER {
        return Err(CodecError::TooLarge(hl as u64));
    }
    let mut js = vec![0u8; hl];
    r.read_exact(&mut js)?;
    let header: Header = serde_json::from_slice(&js)?;
    if header.payload_len > max_payload.min(MAX_PAYLOAD) {
        return Err(CodecError::TooLarge(header.payload_len));
    }
    let mut payload = Vec::new();
    r.take(header.payload_len).read_to_end(&mut payload)?;
    if payload.len() as u64 != header.payload_len {
        return Err(CodecError::Io(std::io::ErrorKind::UnexpectedEof.into()));
    }
    Ok((header, payload))
}

/// Incremental decode from a buffer: `Ok(None)` if incomplete, else the
/// frame and the number of bytes it used.
pub fn decode(buf: &[u8]) -> Result<Option<(Header, Vec<u8>, usize)>, CodecError> {
    if buf.len() < 4 {
        return Ok(None);
    }
    let hl = u32::from_le_bytes(buf[..4].try_into().expect("4 bytes")) as usize;
    if hl > MAX_HEADER {
        return Err(CodecError::TooLarge(hl as u64));
    }
    if buf.len() < 4 + hl {
        return Ok(None);
    }
    let header: Header = serde_json::from_slice(&buf[4..4 + hl])?;
    if header.payload_len > MAX_PAYLOAD {
        return Err(CodecError::TooLarge(header.payload_len));
    }
    let end = 4 + hl + header.payload_len as usize;
    if buf.len() < end {
        return Ok(None);
    }
    Ok(Some((header.clone(), buf[4 + hl..end].to_vec(), end)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_incremental() {
        let mut h = Header::new(MsgType::Push, "tok");
        h.layout = Some(serde_json::json!({"objects": []}));
        let payload: Vec<u8> = (0..=255).collect();
        let bytes = encode(&h, &payload);
        let (h2, p2) = read_frame(&mut &bytes[..]).unwrap();
        assert_eq!(h2.payload_len, 256);
        assert_eq!(h2.kind, MsgType::Push);
        assert_eq!(h2.layout, h.layout);
        assert_eq!(p2, payload);
        // Every prefix is "incomplete", the whole thing decodes, extra bytes stay.
        for cut in 0..bytes.len() {
            assert!(decode(&bytes[..cut]).unwrap().is_none());
        }
        let mut two = bytes.clone();
        two.extend_from_slice(&bytes);
        let (_, p, used) = decode(&two).unwrap().unwrap();
        assert_eq!((p, used), (payload, bytes.len()));
        // The wire header uses "type" and lowercase names.
        let js = std::str::from_utf8(
            &bytes[4..4 + u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize],
        )
        .unwrap()
        .to_string();
        assert!(js.contains("\"type\":\"push\""), "{js}");
    }

    #[test]
    fn reply_payload_limit_and_truncation() {
        let mut h = Header::new(MsgType::Ack, "");
        let big = encode(&h, &vec![0u8; 2 << 20]);
        assert!(matches!(
            read_frame_limited(&mut &big[..], MAX_REPLY_PAYLOAD),
            Err(CodecError::TooLarge(_))
        ));
        // A huge declared payload that never arrives: error, no big buffer.
        h.payload_len = 0;
        let mut b = encode(&h, b"");
        let hl = u32::from_le_bytes(b[..4].try_into().unwrap()) as usize;
        let js = String::from_utf8(b[4..4 + hl].to_vec()).unwrap().replace(
            "\"payload_len\":0",
            &format!("\"payload_len\":{}", 500u64 << 20),
        );
        b = (js.len() as u32).to_le_bytes().to_vec();
        b.extend_from_slice(js.as_bytes());
        b.extend_from_slice(b"short");
        assert!(matches!(read_frame(&mut &b[..]), Err(CodecError::Io(_))));
    }

    #[test]
    fn rejects_huge_header() {
        let mut b = (MAX_HEADER as u32 + 1).to_le_bytes().to_vec();
        b.extend_from_slice(b"{}");
        assert!(matches!(decode(&b), Err(CodecError::TooLarge(_))));
        assert!(read_frame(&mut &b[..]).is_err());
    }
}
