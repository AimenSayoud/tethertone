//! The platform-free half of Tethertone for Windows and Linux: the wire
//! protocol, pairing, the jitter buffer with drift correction, and the server.
//!
//! The protocol is specified in `docs/PROTOCOL.md` and checked against
//! `docs/test-vectors.json`, the same vectors the Swift and Kotlin
//! implementations use.

pub mod jitter;
pub mod pairing;
pub mod protocol;
pub mod server;

pub use pairing::PairingInfo;
pub use protocol::{StreamMode, UpSource};
pub use server::{ClientSnapshot, Server, ServerConfig, ServerEvents, StreamFormat};
