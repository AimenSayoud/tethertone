//! Wire format. Normative source: docs/PROTOCOL.md. This file, the Swift
//! `Proto` enum and the Kotlin `Proto` object must agree byte for byte.

use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
pub const HEADER_SIZE: usize = 8;
pub const MAX_PAYLOAD: usize = 65_535;

pub const AUDIO: u8 = 0;
pub const PING: u8 = 1;
pub const PONG: u8 = 2;
pub const STATS: u8 = 3;
pub const BYE: u8 = 4;
pub const CONTROL: u8 = 5;

/// Capability: audio in both directions plus CONTROL frames (PROTOCOL.md §6).
pub const CAP_DUPLEX: &str = "duplex";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameHeader {
    pub frame_type: u8,
    pub flags: u8,
    pub length: usize,
    pub seq: u32,
}

/// Header fields are big-endian; AUDIO payloads are little-endian PCM.
pub fn encode_header(frame_type: u8, length: usize, seq: u32) -> [u8; HEADER_SIZE] {
    assert!(length <= MAX_PAYLOAD, "payload {length} out of range");
    let len = (length as u16).to_be_bytes();
    let seq = seq.to_be_bytes();
    [frame_type, 0, len[0], len[1], seq[0], seq[1], seq[2], seq[3]]
}

pub fn decode_header(bytes: &[u8]) -> Option<FrameHeader> {
    if bytes.len() < HEADER_SIZE {
        return None;
    }
    Some(FrameHeader {
        frame_type: bytes[0],
        flags: bytes[1],
        length: u16::from_be_bytes([bytes[2], bytes[3]]) as usize,
        seq: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
    })
}

/// A complete frame: header followed by payload.
pub fn frame(frame_type: u8, payload: &[u8], seq: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_SIZE + payload.len());
    out.extend_from_slice(&encode_header(frame_type, payload.len(), seq));
    out.extend_from_slice(payload);
    out
}

#[derive(Debug, Clone, Deserialize)]
pub struct Hello {
    pub v: u32,
    #[serde(default)]
    pub role: Option<String>,
    pub token: String,
    pub name: String,
    #[serde(default)]
    pub caps: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Welcome {
    pub v: u32,
    pub ok: bool,
    pub rate: u32,
    pub ch: u16,
    pub fmt: &'static str,
    pub block: u32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub err: Option<String>,
    pub caps: Vec<&'static str>,
}

impl Welcome {
    pub fn reject(reason: &str) -> Self {
        Welcome {
            v: VERSION,
            ok: false,
            rate: 0,
            ch: 0,
            fmt: "pcm_s16le",
            block: 0,
            name: String::new(),
            err: Some(reason.to_string()),
            caps: vec![],
        }
    }
}

/// Telemetry a client reports back with each PONG.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct ClientStats {
    #[serde(rename = "bufMs", default)]
    pub buf_ms: i64,
    #[serde(default)]
    pub under: i64,
    #[serde(default)]
    pub lost: i64,
    #[serde(default = "one")]
    pub speed: f64,
}

fn one() -> f64 {
    1.0
}

/// What a client sends up, if anything. Unknown values read as `Off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UpSource {
    #[default]
    Off,
    Mic,
    Playback,
}

impl UpSource {
    pub fn from_wire(value: Option<&str>) -> Self {
        match value {
            Some("mic") => UpSource::Mic,
            Some("playback") => UpSource::Playback,
            _ => UpSource::Off,
        }
    }

    pub fn wire(self) -> &'static str {
        match self {
            UpSource::Off => "off",
            UpSource::Mic => "mic",
            UpSource::Playback => "playback",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            UpSource::Off => "Off",
            UpSource::Mic => "Phone microphone",
            UpSource::Playback => "Phone's audio",
        }
    }
}

/// Which way audio flows on one connection. Owned by the client (§6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamMode {
    pub down: bool,
    pub up: UpSource,
}

impl Default for StreamMode {
    fn default() -> Self {
        StreamMode { down: true, up: UpSource::Off }
    }
}

impl StreamMode {
    pub fn is_valid(self) -> bool {
        self.down || self.up != UpSource::Off
    }
}

/// Payload of a CONTROL frame. Only the fields a given `t` uses are set.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ControlMessage {
    pub t: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub down: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub up: Option<String>,
    #[serde(rename = "upRate", skip_serializing_if = "Option::is_none", default)]
    pub up_rate: Option<u32>,
    #[serde(rename = "upCh", skip_serializing_if = "Option::is_none", default)]
    pub up_ch: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reason: Option<String>,
}

impl ControlMessage {
    pub const MODE: &'static str = "mode";
    pub const SET_MODE: &'static str = "setMode";
    pub const REFUSED: &'static str = "refused";
    pub const UP_DENIED: &'static str = "upDenied";

    pub fn stream_mode(&self) -> StreamMode {
        StreamMode {
            down: self.down.unwrap_or(true),
            up: UpSource::from_wire(self.up.as_deref()),
        }
    }

    pub fn set_mode(mode: StreamMode) -> Self {
        ControlMessage {
            t: Self::SET_MODE.into(),
            down: Some(mode.down),
            up: Some(mode.up.wire().into()),
            ..Default::default()
        }
    }

    pub fn up_denied(reason: &str) -> Self {
        ControlMessage { t: Self::UP_DENIED.into(), reason: Some(reason.into()), ..Default::default() }
    }
}

/// Peak absolute amplitude of s16le PCM, 0.0..=1.0, sampled every 4th frame.
pub fn peak_s16le(pcm: &[u8]) -> f32 {
    let mut peak = 0i32;
    for chunk in pcm.chunks_exact(2).step_by(4) {
        let sample = i16::from_le_bytes([chunk[0], chunk[1]]) as i32;
        peak = peak.max(sample.abs());
    }
    (peak as f32 / 32768.0).min(1.0)
}
