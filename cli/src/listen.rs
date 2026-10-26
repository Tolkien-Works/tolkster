// `tolkster listen`: holds a WebSocket open to the owner's relay. A command from the
// Durable Object's machines (machine.rs) is carried out (command.rs, commands/listen.rs)
// and answered with its result on the connection it came on: the Durable Object sends it
// again until a result comes, so one this process dies holding is carried out on its
// resend. Each repo's base tip,
// level state the relay sends on connect and on each push, is taken for `listen`'s own
// update, and the issue waiting for a slot is kept; nothing answers either.
//
// A drain for a new build takes no new command, leaving it for its resend after the
// restart (`Scheduler::restart_due`).
//
// The relay sends IDs, enums, names, SHAs and times only, never Linear or GitHub text.

use std::io::{self, ErrorKind, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::sleep;
use std::time::{Duration, Instant};

use tungstenite::client::IntoClientRequest;
use tungstenite::handshake::HandshakeError;
use tungstenite::http::{HeaderValue, Uri};
use tungstenite::http::header::AUTHORIZATION;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::error::{Context, Error};
use crate::machine::{parse_base, parse_command};
use crate::panel::{self, Connection};
use crate::scheduler::{End, Scheduler};
use crate::timing;
use crate::sync::lock;
use crate::trace::{self, Ended, Realtime, Setting, Step};
use crate::update::Restart;

// The relay answers with "pong" without waking (setWebSocketAutoResponse). Silence for
// DEAD_AFTER means the connection died without closing, as when the workstation sleeps.
const PING: &str = "ping";
const PONG: &str = "pong";
const PING_EVERY: Duration = Duration::from_secs(30);
const DEAD_AFTER: Duration = Duration::from_secs(75);
const READ_TIMEOUT: Duration = Duration::from_secs(5);
const UNREAD_PAUSE: Duration = Duration::from_millis(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

// The code the relay closes a connection with when a newer acting one replaced it
// (src/worker/realtime.ts, `SUPERSEDED`; pinned in test/contract.json). Any other close
// reconnects.
const SUPERSEDED: u16 = 4000;

// A connection that stayed up for STABLE restarts the delays from the first.
const FIRST_DELAY: Duration = Duration::from_secs(1);
const LAST_DELAY: Duration = Duration::from_secs(60);
const STABLE: Duration = Duration::from_secs(60);

pub struct Backoff {
    next: Duration,
}

impl Backoff {
    pub fn new() -> Self {
        Backoff { next: FIRST_DELAY }
    }

    pub fn delay(&mut self, stayed_up: Duration) -> Duration {
        if stayed_up >= STABLE {
            self.next = FIRST_DELAY;
        }
        let delay = self.next;
        self.next = (self.next * 2).min(LAST_DELAY);
        delay
    }
}

// The setting a 503's body names, `{ "missing": [name] }`, if it is one the Worker is known to
// refuse /realtime for.
fn unset(body: Option<&[u8]>) -> Option<Setting> {
    let body: serde_json::Value = serde_json::from_slice(body?).ok()?;
    body["missing"].as_array()?.iter().find_map(|name| Setting::named(name.as_str()?))
}

fn refused(status: u16, body: Option<&[u8]>) -> End {
    match status {
        401 => End::Stop(Error::Failed("the Worker refused the realtime secret: check that it matches REALTIME_SECRET".into())),
        503 => End::Retry(unset(body).map_or(Ended::Status(503), Ended::Unset)),
        404 => End::Stop(Error::Failed("the Worker has no /realtime endpoint: check \"worker\", and that the Worker is deployed".into())),
        status => End::Retry(Ended::Status(status)),
    }
}

// Without a scheduler it only prints, and never restarts. `None`: superseded, so it ends
// without starting again.
pub fn listen(url: &str, secret: &str, out: &mut impl Write, mut scheduler: Option<Scheduler>) -> Result<Option<Restart>, Error> {
    let mut backoff = Backoff::new();
    let mut again = false;
    loop {
        let started = Instant::now();
        match connection(url, secret, out, scheduler.as_mut(), again) {
            End::Stop(error) => {
                panel::connection(Connection::Stopped);
                return Err(error);
            }
            End::Restart(restart) => return Ok(Some(restart)),
            End::Superseded => {
                panel::connection(Connection::Superseded);
                trace::event(Realtime::Superseded);
                scheduler.as_mut().map_or(Ok(()), Scheduler::drain_superseded)?;
                return Ok(None);
            }
            End::Quit => {
                panel::connection(Connection::Stopped);
                return Ok(None);
            }
            End::Retry(reason) => {
                let delay = backoff.delay(started.elapsed());
                panel::connection(Connection::Reconnecting(Instant::now() + delay));
                trace::event(Realtime::Reconnecting { ended: reason, delay });
                again = true;
                // `q` with no connection: drained as a supersede is, as no result can go out.
                if !wait_out(delay) {
                    panel::connection(Connection::Stopped);
                    scheduler.as_mut().map_or(Ok(()), Scheduler::drain_superseded)?;
                    return Ok(None);
                }
                if let Some(restart) = scheduler.as_ref().filter(|_| !panel::quitting()).and_then(Scheduler::restart_due) {
                    return Ok(Some(restart));
                }
            }
        }
    }
}

// The backoff's wait, cut short by `q` on the panel: false then, or quitting already.
fn wait_out(delay: Duration) -> bool {
    let until = Instant::now() + delay;
    while !panel::quitting() {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        sleep(left.min(QUIT_POLL));
    }
    false
}

const QUIT_POLL: Duration = Duration::from_millis(200);

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

// `again`: after a connection that ended, so this one's is a recovery.
fn connection(url: &str, secret: &str, out: &mut impl Write, mut scheduler: Option<&mut Scheduler>, again: bool) -> End {
    panel::connection(Connection::Connecting);
    let socket = match connect(url, secret, HANDSHAKE_TIMEOUT, scheduler.is_none()) {
        Ok(socket) => socket,
        Err(end) => return end,
    };
    trace::event(Realtime::Connected { again });
    panel::connection(Connection::Connected(Instant::now()));
    let (wake, woken) = mpsc::channel();
    let quit = wake.clone();
    panel::waking(move || {
        let _ = quit.send(Wake::Ended);
    });
    let socket = match Reading::start(socket, wake.clone()) {
        Ok(socket) => socket,
        Err(error) => return End::Retry(Ended::failed(Step::Reader, error)),
    };
    if let Some(scheduler) = scheduler.as_deref_mut() {
        scheduler.connected();
        scheduler.waking(move || {
            let _ = wake.send(Wake::Ended);
        });
    }
    let mut last_heard = Instant::now();
    let mut last_ping = Instant::now();
    loop {
        if let Some(scheduler) = scheduler.as_deref_mut().filter(|_| panel::quitting()) {
            scheduler.quit();
        }
        match scheduler.as_deref_mut().and_then(|scheduler| scheduler.step(|text| socket.send(Message::text(text)))) {
            // Quitting outranks a restart: the next start catches up.
            Some(End::Restart(_)) if panel::quitting() => return End::Quit,
            Some(end) => return closed_first(end, &woken),
            None if scheduler.as_deref().is_some_and(Scheduler::drained) => return End::Quit,
            None => {}
        }
        if last_heard.elapsed() >= DEAD_AFTER {
            return End::Retry(Ended::NoAnswer);
        }
        if last_ping.elapsed() >= PING_EVERY {
            if let Err(error) = socket.send(Message::text(PING)) {
                return closed_first(End::Retry(Ended::failed(Step::Ping, error)), &woken);
            }
            last_ping = Instant::now();
            panel::pinged();
        }
        // READ_TIMEOUT still bounds the wait, for the ping, the dead connection and what
        // `step` times.
        let message = match woken.recv_timeout(READ_TIMEOUT) {
            Ok(Wake::Frame(Ok(message))) => message,
            Ok(Wake::Ended) | Err(RecvTimeoutError::Timeout) => continue,
            Ok(Wake::Frame(Err(tungstenite::Error::ConnectionClosed))) => return End::Retry(Ended::Closed),
            Ok(Wake::Frame(Err(error))) => return End::Retry(Ended::failed(Step::Connection, error)),
            Ok(Wake::Stopped) | Err(RecvTimeoutError::Disconnected) => return End::Retry(Ended::ReaderStopped),
        };
        last_heard = Instant::now();
        if superseded(&message) {
            return End::Superseded;
        }
        let Message::Text(text) = message else { continue };
        if text.as_str() == PONG {
            panel::ponged();
            continue;
        }
        let Ok(frame) = serde_json::from_str::<serde_json::Value>(text.as_str()) else { continue };
        if let Some(command) = parse_command(&frame) {
            // One line, as a base tip's is printed.
            if let Err(error) = shown(out, &frame, scheduler.is_none()) {
                return End::Stop(error);
            }
            match (command, scheduler.as_deref_mut()) {
                (Ok(command), Some(scheduler)) => {
                    if !scheduler.take_command(command) {
                        return End::Stop(Error::Failed("the scheduler stopped".into()));
                    }
                }
                // Unanswered: the Durable Object's timeout ends it.
                (Err(reason), Some(_)) => trace::event(Realtime::Skipped(Error::Failed(reason))),
                (_, None) => {}
            }
            continue;
        }
        if let Some(coordinator) = panel::parse_frame(&frame) {
            if let Err(error) = shown(out, &frame, scheduler.is_none()) {
                return End::Stop(error);
            }
            match coordinator {
                Ok(coordinator) => panel::framed(Some(coordinator)),
                // The last one is no longer current: the panel falls back to its own view.
                Err(reason) => {
                    panel::framed(None);
                    trace::event(Realtime::Skipped(Error::Failed(reason)));
                }
            }
            continue;
        }
        if let Some(timing) = timing::forwarded(&frame) {
            if let Err(error) = shown(out, &frame, scheduler.is_none()) {
                return End::Stop(error);
            }
            match (timing, scheduler.as_deref()) {
                (Ok((issue, summary)), Some(scheduler)) => scheduler.keep_timing(issue, summary),
                (Err(reason), Some(_)) => trace::event(Realtime::Skipped(Error::Failed(reason))),
                (_, None) => {}
            }
            continue;
        }
        let Some(tip) = parse_base(&frame) else { continue };
        if let Err(error) = shown(out, &frame, scheduler.is_none()) {
            return End::Stop(error);
        }
        match (tip, scheduler.as_deref_mut()) {
            (Ok(tip), Some(scheduler)) => {
                if !scheduler.take_base(tip) {
                    return End::Stop(Error::Failed("the scheduler stopped".into()));
                }
            }
            (Err(reason), Some(_)) => trace::event(Realtime::Skipped(Error::Failed(reason))),
            (_, None) => {}
        }
    }
}

// A send that fails because the relay closed the connection ends it as a retry before the
// close frame, already read, is looked at: a superseded `listen` that reconnected would
// supersede the one that replaced it.
fn closed_first(end: End, woken: &Receiver<Wake>) -> End {
    let closed = matches!(end, End::Retry(_)) && woken.try_iter().any(|wake| matches!(wake, Wake::Frame(Ok(message)) if superseded(&message)));
    if closed { End::Superseded } else { end }
}

fn superseded(message: &Message) -> bool {
    matches!(message, Message::Close(Some(frame)) if u16::from(frame.code) == SUPERSEDED)
}

// A frame that came in: what `--print` shows, and `--verbose` beside the rest.
fn shown(out: &mut impl Write, frame: &serde_json::Value, print: bool) -> Result<(), Error> {
    if !print && (!trace::verbose() || trace::inbound_to_sink(&frame.to_string())) {
        return Ok(());
    }
    writeln!(out, "{}", trace::stamped_inbound(&frame.to_string())).and_then(|()| out.flush()).context("stdout closed")
}

// `print`: this connection only prints, so the relay never counts it as the newest, which
// commands go to (src/worker/relay.ts, PRINT_HEADER).
fn connect(url: &str, secret: &str, handshake_timeout: Duration, print: bool) -> Result<Socket, End> {
    let mut request = url.into_client_request().map_err(|e| End::Stop(Error::Usage(format!("bad realtime URL: {e}"))))?;
    if print {
        request.headers_mut().insert("X-Tolkster-Print", HeaderValue::from_static("1"));
    }
    let mut bearer = HeaderValue::from_str(&format!("Bearer {secret}")).map_err(|_| End::Stop(Error::Usage("the realtime secret has characters a header can't carry".into())))?;
    bearer.set_sensitive(true);
    request.headers_mut().insert(AUTHORIZATION, bearer);
    let tcp = open(request.uri(), handshake_timeout).map_err(|e| End::Retry(Ended::failed(Step::Connecting, e)))?;
    let (socket, _) = tungstenite::client_tls(request, tcp).map_err(|error| match error {
        HandshakeError::Failure(tungstenite::Error::Http(response)) => refused(response.status().as_u16(), response.body().as_deref()),
        HandshakeError::Failure(error) => End::Retry(Ended::failed(Step::Connecting, error)),
        HandshakeError::Interrupted(_) => End::Retry(Ended::HandshakeTimedOut),
    })?;
    Ok(socket)
}

// The write timeout stays for the connection's life: a send that blocks that long means
// it's dead.
fn open(uri: &Uri, handshake_timeout: Duration) -> io::Result<TcpStream> {
    let host = uri.host().ok_or_else(|| io::Error::other("no host in the realtime URL"))?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let port = uri.port_u16().unwrap_or(if uri.scheme_str() == Some("wss") { 443 } else { 80 });
    let mut last = io::Error::other(format!("{host} has no address"));
    for address in resolve(host, port, CONNECT_TIMEOUT)? {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(tcp) => {
                tcp.set_read_timeout(Some(handshake_timeout))?;
                tcp.set_write_timeout(Some(handshake_timeout))?;
                return Ok(tcp);
            }
            Err(error) => last = error,
        }
    }
    Err(last)
}

type Lookup = ((String, u16), Receiver<io::Result<Vec<SocketAddr>>>);

// A lookup that outlasted its timeout, still running. The next connection waits on it
// rather than starting another, so a resolver that stays stuck holds one thread, not one
// per reconnect. Per process, as it outlives the connection that began it.
static PENDING: Mutex<Option<Lookup>> = Mutex::new(None);

// The standard library's lookup has no timeout of its own, so it runs on a thread of its
// own.
fn resolve(host: &str, port: u16, timeout: Duration) -> io::Result<Vec<SocketAddr>> {
    resolve_in(&PENDING, host, port, timeout)
}

// `resolve` over a slot of its own, so a test can keep other connections' lookups out of it.
fn resolve_in(slot: &Mutex<Option<Lookup>>, host: &str, port: u16, timeout: Duration) -> io::Result<Vec<SocketAddr>> {
    let key = (host.to_string(), port);
    let mut pending = lock(slot);
    let lookup = match pending.take() {
        Some((same, lookup)) if same == key => lookup,
        _ => {
            let (found, lookup) = mpsc::channel();
            let (host, port) = key.clone();
            std::thread::spawn(move || {
                let _ = found.send((host.as_str(), port).to_socket_addrs().map(Iterator::collect));
            });
            lookup
        }
    };
    match lookup.recv_timeout(timeout) {
        Ok(addresses) => addresses,
        Err(RecvTimeoutError::Timeout) => {
            *pending = Some((key, lookup));
            Err(io::Error::new(ErrorKind::TimedOut, "looking up the Worker's address timed out"))
        }
        Err(RecvTimeoutError::Disconnected) => Err(io::Error::other("looking up the Worker's address failed")),
    }
}

fn tcp(socket: &mut Socket) -> io::Result<&mut TcpStream> {
    match socket.get_mut() {
        MaybeTlsStream::Plain(tcp) => Ok(tcp),
        MaybeTlsStream::Rustls(tls) => Ok(&mut tls.sock),
        _ => Err(io::Error::other("unexpected stream type")),
    }
}

// What wakes the connection's loop: a frame, the end of a job, whose result
// goes out at once, or the reader's end, after any error it ended on.
enum Wake {
    Frame(tungstenite::Result<Message>),
    Ended,
    Stopped,
}

// The connection's socket, read on a thread of its own: it waits for bytes on a clone of
// the TCP stream, without the lock, so a send waits at most on a drain of bytes already
// come, and reads under the lock only without blocking. Dropped, it shuts the stream,
// which ends the thread.
struct Reading(Arc<Mutex<Socket>>);

impl Reading {
    fn start(mut socket: Socket, frames: Sender<Wake>) -> io::Result<Reading> {
        let mut waiting = tcp(&mut socket)?.try_clone()?;
        // The wait for bytes lasts only READ_TIMEOUT, so the thread notices the loop has gone.
        waiting.set_read_timeout(Some(READ_TIMEOUT))?;
        let socket = Arc::new(Mutex::new(socket));
        let read = Arc::clone(&socket);
        // A panic in the reader ends the connection too, rather than leaving it to DEAD_AFTER.
        std::thread::spawn(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_frames(&read, &mut waiting, &frames)));
            let _ = frames.send(Wake::Stopped);
        });
        Ok(Reading(socket))
    }

    fn lock(&self) -> MutexGuard<'_, Socket> {
        lock(&self.0)
    }

    fn send(&self, message: Message) -> tungstenite::Result<()> {
        self.lock().send(message)
    }
}

impl Drop for Reading {
    fn drop(&mut self) {
        if let Ok(tcp) = tcp(&mut self.lock()) {
            let _ = tcp.shutdown(Shutdown::Both);
        }
    }
}

// Ends once a frame can't be handed over, the connection fails, or the loop has gone.
fn read_frames(socket: &Arc<Mutex<Socket>>, waiting: &mut TcpStream, frames: &Sender<Wake>) {
    let mut byte = [0; 1];
    // `peeked` is None on the first pass, which reads without waiting for bytes: frames
    // that came with the handshake's response are already in the socket's buffer, and none
    // may follow them to end a wait.
    let mut first = true;
    loop {
        let peeked = if std::mem::take(&mut first) {
            None
        } else {
            match waiting.peek(&mut byte) {
                Ok(peeked) => Some(peeked),
                Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    if Arc::strong_count(socket) == 1 {
                        return;
                    }
                    continue;
                }
                Err(error) => {
                    let _ = frames.send(Wake::Frame(Err(tungstenite::Error::Io(error))));
                    return;
                }
            }
        };
        let mut locked = lock(socket);
        if let Err(error) = tcp(&mut locked).and_then(|tcp| tcp.set_nonblocking(true)) {
            let _ = frames.send(Wake::Frame(Err(tungstenite::Error::Io(error))));
            return;
        }
        // Every frame already buffered is handed over, not only the first: no more bytes
        // may come to wake the wait for them.
        let mut handed = false;
        loop {
            match locked.read() {
                Err(tungstenite::Error::Io(error)) if error.kind() == ErrorKind::WouldBlock => break,
                read => {
                    handed = true;
                    let failed = read.is_err();
                    if frames.send(Wake::Frame(read)).is_err() || failed {
                        return;
                    }
                }
            }
        }
        // Sends block, bounded by the write timeout.
        if let Err(error) = tcp(&mut locked).and_then(|tcp| tcp.set_nonblocking(false)) {
            let _ = frames.send(Wake::Frame(Err(tungstenite::Error::Io(error))));
            return;
        }
        // The stream ended, and reading it found no error to say so.
        if peeked == Some(0) {
            let _ = frames.send(Wake::Frame(Err(tungstenite::Error::ConnectionClosed)));
            return;
        }
        drop(locked);
        // TLS can't read while a write of its own is stuck, so the bytes peeked stay unread:
        // a pause, not a spin, until the send buffer drains or a send times out.
        if !handed && peeked.is_some() {
            sleep(UNREAD_PAUSE);
        }
    }
}

#[cfg(test)]
mod tests;
