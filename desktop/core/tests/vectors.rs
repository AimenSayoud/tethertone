//! docs/test-vectors.json — shared with the Swift self-test, the Kotlin tests
//! and tools/probe.py, so the implementations cannot drift apart.

use serde_json::Value;
use tethertone_core::protocol::{decode_header, encode_header, ControlMessage};

fn vectors() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/test-vectors.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("docs/test-vectors.json")).unwrap()
}

#[test]
fn headers_match_the_shared_bytes() {
    for v in vectors()["headers"].as_array().unwrap() {
        let name = v["name"].as_str().unwrap();
        let encoded = encode_header(
            v["type"].as_u64().unwrap() as u8,
            v["length"].as_u64().unwrap() as usize,
            v["seq"].as_u64().unwrap() as u32,
        );
        let hex: String = encoded.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, v["hex"].as_str().unwrap(), "{name}");
        assert_eq!(decode_header(&encoded).unwrap().seq as u64, v["seq"].as_u64().unwrap(), "{name}");
    }
}

#[test]
fn control_messages_decode_as_specified() {
    for v in vectors()["control"].as_array().unwrap() {
        let name = v["name"].as_str().unwrap();
        let message: ControlMessage = serde_json::from_str(v["json"].as_str().unwrap()).unwrap();
        let expect = &v["expect"];
        assert_eq!(message.t, expect["t"].as_str().unwrap(), "{name}");
        if let Some(down) = expect["down"].as_bool() {
            assert_eq!(message.stream_mode().down, down, "{name}");
        }
        if let Some(up) = expect["up"].as_str() {
            assert_eq!(message.stream_mode().up.wire(), up, "{name}");
        }
        if let Some(rate) = expect["upRate"].as_u64() {
            assert_eq!(message.up_rate, Some(rate as u32), "{name}");
        }
        if let Some(ch) = expect["upCh"].as_u64() {
            assert_eq!(message.up_ch, Some(ch as u16), "{name}");
        }
        if let Some(reason) = expect["reason"].as_str() {
            assert_eq!(message.reason.as_deref(), Some(reason), "{name}");
        }
    }
}
