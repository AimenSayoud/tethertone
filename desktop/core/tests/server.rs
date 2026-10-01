//! The server end to end over localhost: handshake, audio down, a live mode
//! switch, audio up, and a second sender being refused by name.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tethertone_core::protocol::{self, decode_header, frame};
use tethertone_core::server::{ServerEvents, StreamFormat};
use tethertone_core::{Server, ServerConfig};

#[derive(Debug)]
enum Event {
    UpStarted(u32, u16, String),
    UpAudio(usize),
}

struct Recorder(Mutex<Sender<Event>>);

impl ServerEvents for Recorder {
    fn up_started(&self, rate: u32, channels: u16, phone: String) {
        let _ = self.0.lock().unwrap().send(Event::UpStarted(rate, channels, phone));
    }
    fn up_audio(&self, pcm: &[u8]) {
        let _ = self.0.lock().unwrap().send(Event::UpAudio(pcm.len()));
    }
}

const TOKEN: &str = "test-token";

fn server() -> (Server, std::sync::mpsc::Receiver<Event>) {
    let (tx, rx) = channel();
    let config = ServerConfig {
        port: 0,
        token: TOKEN.into(),
        name: "Test PC".into(),
        format: StreamFormat::default(),
        max_queued: 24,
    };
    (Server::start(config, Arc::new(Recorder(Mutex::new(tx)))).unwrap(), rx)
}

struct Phone {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
}

impl Phone {
    fn connect(port: u16, name: &str, token: &str) -> (Phone, Value) {
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut phone = Phone { reader: BufReader::new(stream.try_clone().unwrap()), stream };
        let hello = json!({"v": 1, "role": "sink", "token": token, "name": name, "caps": ["pcm_s16le", "duplex"]});
        phone.stream.write_all(format!("{hello}\n").as_bytes()).unwrap();
        let mut line = String::new();
        phone.reader.read_line(&mut line).unwrap();
        (phone, serde_json::from_str(&line).unwrap())
    }

    fn send(&mut self, frame_type: u8, payload: &[u8], seq: u32) {
        self.stream.write_all(&frame(frame_type, payload, seq)).unwrap();
    }

    fn control(&mut self, message: Value) {
        self.send(protocol::CONTROL, message.to_string().as_bytes(), 0);
    }

    /// Next frame of `wanted` type, skipping others.
    fn next(&mut self, wanted: u8) -> Vec<u8> {
        loop {
            let mut header = [0u8; 8];
            self.reader.read_exact(&mut header).unwrap();
            let h = decode_header(&header).unwrap();
            let mut payload = vec![0u8; h.length];
            self.reader.read_exact(&mut payload).unwrap();
            if h.frame_type == wanted {
                return payload;
            }
        }
    }
}

#[test]
fn wrong_token_is_turned_away() {
    let (server, _rx) = server();
    let (_phone, welcome) = Phone::connect(server.port(), "Intruder", "nope");
    assert_eq!(welcome["ok"], false);
    assert_eq!(welcome["err"], "invalid token");
}

#[test]
fn audio_down_mode_switch_and_audio_up() {
    let (server, rx) = server();
    let (mut phone, welcome) = Phone::connect(server.port(), "Pixel", TOKEN);
    assert_eq!(welcome["ok"], true);
    assert_eq!(welcome["rate"], 48000);
    assert!(welcome["caps"].as_array().unwrap().contains(&json!("duplex")));

    // Down: a broadcast block arrives as AUDIO.
    std::thread::sleep(Duration::from_millis(100));
    server.broadcast(&[1u8; 1920]);
    assert_eq!(phone.next(protocol::AUDIO).len(), 1920);

    // Up: announce the microphone, then send a block.
    phone.control(json!({"t": "mode", "down": true, "up": "mic", "upRate": 48000, "upCh": 1}));
    match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
        Event::UpStarted(rate, ch, name) => assert_eq!((rate, ch, name.as_str()), (48000, 1, "Pixel")),
        other => panic!("expected up_started, got {other:?}"),
    }
    phone.send(protocol::AUDIO, &[0u8; 960], 0);
    match rx.recv_timeout(Duration::from_secs(2)).unwrap() {
        Event::UpAudio(len) => assert_eq!(len, 960),
        other => panic!("expected up_audio, got {other:?}"),
    }

    // A second phone asking to send is refused, naming the first.
    let (mut other, _) = Phone::connect(server.port(), "Tablet", TOKEN);
    other.control(json!({"t": "mode", "down": true, "up": "mic", "upRate": 48000, "upCh": 1}));
    let denied: Value = serde_json::from_slice(&other.next(protocol::CONTROL)).unwrap();
    assert_eq!(denied["t"], "upDenied");
    assert_eq!(denied["reason"], "Pixel is already sending audio to this computer.");

    // The computer asks the first phone for a new mode.
    let id = server.clients().iter().find(|c| c.name == "Pixel").unwrap().id;
    server.request_mode(id, tethertone_core::StreamMode { down: false, up: tethertone_core::UpSource::Playback });
    let request: Value = serde_json::from_slice(&phone.next(protocol::CONTROL)).unwrap();
    assert_eq!(request, json!({"t": "setMode", "down": false, "up": "playback"}));
}

#[test]
fn send_only_mode_stops_audio_down_but_keeps_pinging() {
    let (server, _rx) = server();
    let (mut phone, _) = Phone::connect(server.port(), "Pixel", TOKEN);
    phone.control(json!({"t": "mode", "down": false, "up": "mic", "upRate": 48000, "upCh": 1}));
    std::thread::sleep(Duration::from_millis(200));
    server.broadcast(&[1u8; 1920]);
    // The next frame must be a PING (within ~1 s), not AUDIO.
    let mut header = [0u8; 8];
    loop {
        phone.reader.read_exact(&mut header).unwrap();
        let h = decode_header(&header).unwrap();
        let mut payload = vec![0u8; h.length];
        phone.reader.read_exact(&mut payload).unwrap();
        assert_ne!(h.frame_type, protocol::AUDIO, "no audio may flow down in send-only mode");
        if h.frame_type == protocol::PING {
            break;
        }
    }
}
