//! The Tethertone server: TCP, token auth, fan-out to any number of phones,
//! and audio back from one phone at a time (PROTOCOL.md §6).
//!
//! Plain threads, no async runtime: a listener, plus a reader and a writer per
//! phone. Each phone has its own bounded queue that drops the *oldest* audio,
//! so a phone that stalls never slows another phone or the capture callback —
//! a backlog that cannot be sent is latency that cannot be recovered.

use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::pairing::token_matches;
use crate::protocol::{self, ClientStats, ControlMessage, Hello, StreamMode, UpSource, Welcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub block_frames: u32,
}

impl Default for StreamFormat {
    fn default() -> Self {
        StreamFormat { sample_rate: 48_000, channels: 2, block_frames: 480 }
    }
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub token: String,
    pub name: String,
    pub format: StreamFormat,
    /// Audio frames queued per phone before the oldest is dropped.
    pub max_queued: usize,
}

#[derive(Debug, Clone)]
pub struct ClientSnapshot {
    pub id: u64,
    pub name: String,
    pub peer: String,
    pub rtt_ms: f64,
    pub dropped: u64,
    pub buffer_ms: i64,
    pub underruns: i64,
    pub lost: i64,
    pub speed: f64,
    /// Whether this phone can do two-way audio. Older apps cannot.
    pub duplex: bool,
    pub mode: StreamMode,
    pub connected_at: SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    Arrived,
    Connected,
    Rejected,
    Silent,
    Disconnected,
    Info,
}

/// Callbacks from the server's threads. Keep them short; `up_audio` runs on
/// the hot path once per 10 ms block.
#[allow(unused_variables)]
pub trait ServerEvents: Send + Sync + 'static {
    fn clients_changed(&self, clients: Vec<ClientSnapshot>) {}
    fn activity(&self, kind: ActivityKind, text: String) {}
    fn up_started(&self, sample_rate: u32, channels: u16, phone: String) {}
    fn up_stopped(&self) {}
    fn up_audio(&self, pcm: &[u8]) {}
    fn refused(&self, phone: String, reason: String) {}
    fn listener_failed(&self, error: String) {}
}

pub struct Server {
    inner: Arc<Inner>,
    listener: Mutex<Option<thread::JoinHandle<()>>>,
    port: u16,
}

struct Inner {
    config: ServerConfig,
    events: Arc<dyn ServerEvents>,
    clients: Mutex<HashMap<u64, Arc<Client>>>,
    next_id: AtomicU64,
    up: Mutex<UpState>,
    bytes_sent: AtomicU64,
    running: AtomicBool,
    epoch: Instant,
}

#[derive(Default)]
struct UpState {
    owner: Option<u64>,
    format: Option<(u32, u16)>,
    blocked: Option<String>,
}

struct Client {
    id: u64,
    stream: TcpStream,
    outbox: Mutex<Outbox>,
    wake: Condvar,
    snapshot: Mutex<ClientSnapshot>,
    authorised: AtomicBool,
    closed: AtomicBool,
    up_format: Mutex<Option<(u32, u16)>>,
}

#[derive(Default)]
struct Outbox {
    control: VecDeque<Vec<u8>>,
    audio: VecDeque<Vec<u8>>,
    audio_seq: u32,
}

const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// Phones answer every PING; a minute-long silence is a half-open socket.
const READ_TIMEOUT: Duration = Duration::from_secs(15);
const PING_INTERVAL: Duration = Duration::from_secs(1);
const MAX_LINE: usize = 8192;

impl Server {
    /// Binds and starts accepting. Fails immediately if the port is taken.
    pub fn start(config: ServerConfig, events: Arc<dyn ServerEvents>) -> io::Result<Server> {
        let listener = TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], config.port)))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let inner = Arc::new(Inner {
            config,
            events,
            clients: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            up: Mutex::new(UpState::default()),
            bytes_sent: AtomicU64::new(0),
            running: AtomicBool::new(true),
            epoch: Instant::now(),
        });
        let accept_inner = inner.clone();
        let handle = thread::Builder::new()
            .name("tethertone-accept".into())
            .spawn(move || accept_loop(accept_inner, listener))?;
        Ok(Server { inner, listener: Mutex::new(Some(handle)), port })
    }

    /// The bound port (useful when started on port 0).
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn format(&self) -> StreamFormat {
        self.inner.config.format
    }

    /// Bytes actually written to sockets — zero while nobody is connected.
    pub fn bytes_sent(&self) -> u64 {
        self.inner.bytes_sent.load(Ordering::Relaxed)
    }

    /// Connected, authorised phones, oldest first.
    pub fn clients(&self) -> Vec<ClientSnapshot> {
        snapshots(&self.inner)
    }

    pub fn client_count(&self) -> usize {
        self.inner.clients.lock().unwrap().values().filter(|c| c.authorised.load(Ordering::Relaxed)).count()
    }

    /// Hot path: one captured block, sent to every phone that wants audio.
    pub fn broadcast(&self, pcm: &[u8]) {
        let clients: Vec<Arc<Client>> = self.inner.clients.lock().unwrap().values().cloned().collect();
        for client in clients {
            if !client.authorised.load(Ordering::Relaxed) {
                continue;
            }
            {
                let snap = client.snapshot.lock().unwrap();
                if snap.duplex && !snap.mode.down {
                    continue; // the phone asked for nothing down: pings only
                }
            }
            let mut outbox = client.outbox.lock().unwrap();
            let seq = outbox.audio_seq;
            outbox.audio_seq = seq.wrapping_add(1);
            outbox.audio.push_back(protocol::frame(protocol::AUDIO, pcm, seq));
            if outbox.audio.len() > self.inner.config.max_queued {
                outbox.audio.pop_front();
                client.snapshot.lock().unwrap().dropped += 1;
            }
            drop(outbox);
            client.wake.notify_one();
        }
    }

    /// Ask a phone to change direction. The phone decides; its answer arrives
    /// as a mode update in `clients_changed`, or through `refused`.
    pub fn request_mode(&self, id: u64, mode: StreamMode) {
        if let Some(client) = self.inner.clients.lock().unwrap().get(&id).cloned() {
            client.send_control(&ControlMessage::set_mode(mode));
        }
    }

    /// While `reason` is set every up stream is refused, and the current one stops.
    pub fn block_up_streams(&self, reason: Option<String>) {
        let mut up = self.inner.up.lock().unwrap();
        up.blocked = reason.clone();
        if let (Some(reason), Some(owner)) = (reason, up.owner) {
            if let Some(client) = self.inner.clients.lock().unwrap().get(&owner).cloned() {
                client.send_control(&ControlMessage::up_denied(&reason));
            }
            up.owner = None;
            up.format = None;
            drop(up);
            self.inner.events.up_stopped();
        }
    }

    pub fn disconnect(&self, id: u64) {
        if let Some(client) = self.inner.clients.lock().unwrap().get(&id).cloned() {
            client.close();
        }
    }

    /// Closes every connection and the listener. Safe to call more than once.
    pub fn stop(&self) {
        self.inner.running.store(false, Ordering::SeqCst);
        let clients: Vec<Arc<Client>> = self.inner.clients.lock().unwrap().values().cloned().collect();
        for client in clients {
            client.close();
        }
        if let Some(handle) = self.listener.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

fn accept_loop(inner: Arc<Inner>, listener: TcpListener) {
    while inner.running.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if let Err(e) = spawn_client(inner.clone(), stream, peer) {
                    inner.events.activity(ActivityKind::Rejected, format!("Could not serve {peer}: {e}"));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(50)),
            Err(e) => {
                inner.events.listener_failed(e.to_string());
                thread::sleep(Duration::from_millis(250));
            }
        }
    }
}

fn spawn_client(inner: Arc<Inner>, stream: TcpStream, peer: SocketAddr) -> io::Result<()> {
    stream.set_nonblocking(false)?;
    // Audio packets are small and frequent; Nagle would add tens of milliseconds.
    stream.set_nodelay(true)?;
    let id = inner.next_id.fetch_add(1, Ordering::Relaxed);
    let client = Arc::new(Client {
        id,
        stream: stream.try_clone()?,
        outbox: Mutex::new(Outbox::default()),
        wake: Condvar::new(),
        snapshot: Mutex::new(ClientSnapshot {
            id,
            name: "?".into(),
            peer: peer.to_string(),
            rtt_ms: 0.0,
            dropped: 0,
            buffer_ms: 0,
            underruns: 0,
            lost: 0,
            speed: 1.0,
            duplex: false,
            mode: StreamMode::default(),
            connected_at: SystemTime::now(),
        }),
        authorised: AtomicBool::new(false),
        closed: AtomicBool::new(false),
        up_format: Mutex::new(None),
    });
    inner.clients.lock().unwrap().insert(id, client.clone());
    // Logged before the handshake on purpose: this line is the proof that the
    // phone's packets reach this computer at all.
    inner.events.activity(ActivityKind::Arrived, format!("Connection from {peer}"));

    let writer_inner = inner.clone();
    let writer_client = client.clone();
    let writer_stream = stream.try_clone()?;
    thread::Builder::new()
        .name(format!("tethertone-tx-{id}"))
        .spawn(move || writer_loop(writer_inner, writer_client, writer_stream))?;
    thread::Builder::new()
        .name(format!("tethertone-rx-{id}"))
        .spawn(move || {
            reader_loop(&inner, &client, stream);
            finish(&inner, &client);
        })?;
    Ok(())
}

impl Client {
    fn send_control(&self, message: &ControlMessage) {
        if !self.authorised.load(Ordering::Relaxed) || !self.snapshot.lock().unwrap().duplex {
            return;
        }
        let Ok(payload) = serde_json::to_vec(message) else { return };
        self.outbox.lock().unwrap().control.push_back(protocol::frame(protocol::CONTROL, &payload, 0));
        self.wake.notify_one();
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let _ = self.stream.shutdown(Shutdown::Both);
        self.wake.notify_all();
    }
}

/// Sends queued frames, control first, and a PING every second.
fn writer_loop(inner: Arc<Inner>, client: Arc<Client>, mut stream: TcpStream) {
    let mut next_ping = Instant::now() + PING_INTERVAL;
    let mut ping_seq: u32 = 0;
    loop {
        let packet = {
            let mut outbox = client.outbox.lock().unwrap();
            loop {
                if client.closed.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(p) = outbox.control.pop_front().or_else(|| outbox.audio.pop_front()) {
                    break Some(p);
                }
                let now = Instant::now();
                if client.authorised.load(Ordering::Relaxed) && now >= next_ping {
                    break None;
                }
                let wait = next_ping.saturating_duration_since(now).max(Duration::from_millis(5));
                outbox = client.wake.wait_timeout(outbox, wait).unwrap().0;
            }
        };
        let packet = packet.unwrap_or_else(|| {
            next_ping = Instant::now() + PING_INTERVAL;
            ping_seq = ping_seq.wrapping_add(1);
            let micros = inner.epoch.elapsed().as_micros() as u64;
            protocol::frame(protocol::PING, &micros.to_be_bytes(), ping_seq)
        });
        if stream.write_all(&packet).is_err() {
            client.close();
            return;
        }
        inner.bytes_sent.fetch_add(packet.len() as u64, Ordering::Relaxed);
    }
}

fn reader_loop(inner: &Arc<Inner>, client: &Arc<Client>, stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(HELLO_TIMEOUT));
    let mut reader = BufReader::new(stream);

    let hello = match read_line(&mut reader) {
        Ok(line) => line,
        Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
            let peer = client.snapshot.lock().unwrap().peer.clone();
            inner.events.activity(ActivityKind::Silent, format!("{peer} connected but never sent a handshake"));
            return;
        }
        Err(_) => return,
    };
    if !handshake(inner, client, reader.get_mut(), &hello) {
        return;
    }
    let _ = reader.get_ref().set_read_timeout(Some(READ_TIMEOUT));

    let mut header = [0u8; protocol::HEADER_SIZE];
    let mut payload = Vec::with_capacity(4096);
    let mut up_expected: Option<u32> = None;
    loop {
        if reader.read_exact(&mut header).is_err() {
            return;
        }
        let Some(h) = protocol::decode_header(&header) else { return };
        payload.resize(h.length, 0);
        if reader.read_exact(&mut payload).is_err() {
            return;
        }
        match h.frame_type {
            protocol::PONG if payload.len() == 8 => {
                let sent = u64::from_be_bytes(payload[..8].try_into().unwrap());
                let now = inner.epoch.elapsed().as_micros() as u64;
                client.snapshot.lock().unwrap().rtt_ms = now.saturating_sub(sent) as f64 / 1000.0;
                publish(inner);
            }
            protocol::STATS => {
                if let Ok(stats) = serde_json::from_slice::<ClientStats>(&payload) {
                    let mut snap = client.snapshot.lock().unwrap();
                    snap.buffer_ms = stats.buf_ms;
                    snap.underruns = stats.under;
                    snap.lost = stats.lost;
                    snap.speed = stats.speed;
                }
            }
            protocol::CONTROL if is_duplex(client) => {
                if let Ok(message) = serde_json::from_slice::<ControlMessage>(&payload) {
                    handle_control(inner, client, message, &mut up_expected);
                }
            }
            protocol::AUDIO if is_duplex(client) && client.snapshot.lock().unwrap().mode.up != UpSource::Off => {
                if let Some(expected) = up_expected
                    && h.seq != expected
                {
                    client.snapshot.lock().unwrap().lost += h.seq.wrapping_sub(expected) as i64;
                }
                up_expected = Some(h.seq.wrapping_add(1));
                if inner.up.lock().unwrap().owner == Some(client.id) {
                    inner.events.up_audio(&payload);
                }
            }
            protocol::BYE => return,
            _ => {} // unknown types are ignored (PROTOCOL.md §2)
        }
    }
}

fn is_duplex(client: &Client) -> bool {
    client.snapshot.lock().unwrap().duplex
}

fn read_line(reader: &mut BufReader<TcpStream>) -> io::Result<String> {
    let mut line = Vec::new();
    let read = reader.by_ref().take(MAX_LINE as u64 + 1).read_until(b'\n', &mut line)?;
    if read == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    if line.last() != Some(&b'\n') {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "hello too large"));
    }
    line.pop();
    Ok(String::from_utf8_lossy(&line).into_owned())
}

fn reply(stream: &mut TcpStream, welcome: &Welcome) -> bool {
    let Ok(mut payload) = serde_json::to_vec(welcome) else { return false };
    payload.push(b'\n');
    stream.write_all(&payload).is_ok()
}

fn handshake(inner: &Arc<Inner>, client: &Arc<Client>, stream: &mut TcpStream, line: &str) -> bool {
    let peer = client.snapshot.lock().unwrap().peer.clone();
    let Ok(hello) = serde_json::from_str::<Hello>(line) else {
        inner.events.activity(ActivityKind::Rejected, format!("{peer} sent something that is not a Tethertone handshake"));
        reply(stream, &Welcome::reject("malformed hello"));
        return false;
    };
    if hello.v != protocol::VERSION {
        inner.events.activity(
            ActivityKind::Rejected,
            format!("{peer} speaks protocol v{}, this server speaks v{}", hello.v, protocol::VERSION),
        );
        reply(stream, &Welcome::reject(&format!("unsupported version {}", hello.v)));
        return false;
    }
    if !token_matches(&hello.token, &inner.config.token) {
        inner.events.activity(
            ActivityKind::Rejected,
            format!("{} was turned away: its token does not match. Re-scan the QR.", hello.name),
        );
        reply(stream, &Welcome::reject("invalid token"));
        return false;
    }

    {
        let mut snap = client.snapshot.lock().unwrap();
        snap.name = hello.name.chars().take(48).collect();
        snap.duplex = hello.caps.iter().any(|c| c == protocol::CAP_DUPLEX);
        snap.connected_at = SystemTime::now();
    }
    let format = inner.config.format;
    let welcome = Welcome {
        v: protocol::VERSION,
        ok: true,
        rate: format.sample_rate,
        ch: format.channels,
        fmt: "pcm_s16le",
        block: format.block_frames,
        name: inner.config.name.clone(),
        err: None,
        caps: vec![protocol::CAP_DUPLEX],
    };
    // Welcome goes out before the writer may send anything else: it is only
    // released once `authorised` flips below.
    if !reply(stream, &welcome) {
        return false;
    }
    client.authorised.store(true, Ordering::SeqCst);
    client.wake.notify_one();
    let name = client.snapshot.lock().unwrap().name.clone();
    inner.events.activity(ActivityKind::Connected, format!("{name} connected from {peer}"));
    publish(inner);
    true
}

fn handle_control(inner: &Arc<Inner>, client: &Arc<Client>, message: ControlMessage, up_expected: &mut Option<u32>) {
    match message.t.as_str() {
        ControlMessage::MODE => {
            let mode = message.stream_mode();
            *client.up_format.lock().unwrap() = if mode.up == UpSource::Off {
                None
            } else {
                Some((message.up_rate.unwrap_or(48_000), message.up_ch.unwrap_or(1)))
            };
            {
                let mut snap = client.snapshot.lock().unwrap();
                if snap.mode.up != mode.up {
                    *up_expected = None;
                }
                snap.mode = mode;
            }
            mode_changed(inner, client);
            publish(inner);
        }
        ControlMessage::REFUSED => {
            let name = client.snapshot.lock().unwrap().name.clone();
            inner.events.refused(name, message.reason.unwrap_or_else(|| "The phone declined.".into()));
        }
        _ => {}
    }
}

/// Claims, keeps or releases the up stream as a phone's mode changes.
fn mode_changed(inner: &Arc<Inner>, client: &Arc<Client>) {
    let wants_up = client.snapshot.lock().unwrap().mode.up != UpSource::Off;
    let mut up = inner.up.lock().unwrap();

    if !wants_up {
        if up.owner == Some(client.id) {
            up.owner = None;
            up.format = None;
            drop(up);
            inner.events.up_stopped();
        }
        return;
    }
    if let Some(reason) = up.blocked.clone() {
        drop(up);
        client.send_control(&ControlMessage::up_denied(&reason));
        return;
    }
    if let Some(owner) = up.owner
        && owner != client.id
    {
        drop(up);
        let other = inner
            .clients
            .lock()
            .unwrap()
            .get(&owner)
            .map(|c| c.snapshot.lock().unwrap().name.clone())
            .unwrap_or_else(|| "Another phone".into());
        client.send_control(&ControlMessage::up_denied(&format!("{other} is already sending audio to this computer.")));
        return;
    }
    let Some(format) = *client.up_format.lock().unwrap() else { return };
    if up.owner == Some(client.id) && up.format == Some(format) {
        return; // same phone, same format: nothing to restart
    }
    up.owner = Some(client.id);
    up.format = Some(format);
    drop(up);
    let name = client.snapshot.lock().unwrap().name.clone();
    inner.events.up_started(format.0, format.1, name);
}

fn finish(inner: &Arc<Inner>, client: &Arc<Client>) {
    client.close();
    inner.clients.lock().unwrap().remove(&client.id);
    {
        let mut up = inner.up.lock().unwrap();
        if up.owner == Some(client.id) {
            up.owner = None;
            up.format = None;
            drop(up);
            inner.events.up_stopped();
        }
    }
    if client.authorised.load(Ordering::SeqCst) {
        let name = client.snapshot.lock().unwrap().name.clone();
        inner.events.activity(ActivityKind::Disconnected, format!("{name} disconnected"));
    }
    publish(inner);
}

/// Only authorised phones are anybody's business.
fn publish(inner: &Arc<Inner>) {
    inner.events.clients_changed(snapshots(inner));
}

fn snapshots(inner: &Arc<Inner>) -> Vec<ClientSnapshot> {
    let mut list: Vec<ClientSnapshot> = inner
        .clients
        .lock()
        .unwrap()
        .values()
        .filter(|c| c.authorised.load(Ordering::Relaxed))
        .map(|c| c.snapshot.lock().unwrap().clone())
        .collect();
    list.sort_by_key(|c| c.connected_at);
    list
}
