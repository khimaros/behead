//! one headunit connection: the session core on a byte stream, shared
//! between the thread reading the headunit and the threads sending it
//! pictures and sound. it needs threads and nothing else of its host, so the
//! linux server and the esp32 firmware drive a headunit the same way.

use aap::{Event, Session, Tls};
use std::io::{Read, Write};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Condvar, Mutex};

const READ_BUFFER: usize = 64 * 1024;

/// session plus the queue feeding the transport writer, locked together so
/// tls records reach the wire in the order they were encrypted
pub struct Link<T: Tls, H> {
    pub session: Session<T>,
    outbox: Sender<Vec<u8>>,
    /// what the host keeps for this connection
    pub host: H,
    pub closed: bool,
}

pub type Shared<T, H> = Arc<(Mutex<Link<T, H>>, Condvar)>;

impl<T: Tls, H> Link<T, H> {
    /// hand pending session output to the writer thread
    pub fn flush(&mut self) -> Result<(), String> {
        let out = self.session.take_output();
        if out.is_empty() {
            return Ok(());
        }
        self.outbox.send(out).map_err(|_| "write: transport closed".to_string())
    }
}

/// write queued output on a thread of its own. a usb write blocks until the
/// host collects it, so writing from the read loop would deadlock against a
/// headunit that is itself waiting for us to read.
fn spawn_writer(mut writer: impl Write + Send + 'static) -> Sender<Vec<u8>> {
    let (outbox, queue) = channel::<Vec<u8>>();
    std::thread::spawn(move || queue.iter().all(|out| writer.write_all(&out).is_ok()));
    outbox
}

/// a link on a connection the headunit just made
pub fn open<T: Tls, H>(session: Session<T>, writer: impl Write + Send + 'static, host: H) -> Shared<T, H> {
    Arc::new((Mutex::new(Link { session, outbox: spawn_writer(writer), host, closed: false }), Condvar::new()))
}

/// wait until the headunit's ack window has room, then send. returns false
/// once the stream is over: the link closed, or live() no longer holds.
pub fn send_when_ready<T: Tls, H>(
    shared: &Shared<T, H>,
    live: impl Fn(&Link<T, H>) -> bool,
    ready: impl Fn(&Link<T, H>) -> bool,
    send: impl FnOnce(&mut Link<T, H>) -> Result<(), aap::Error>,
) -> bool {
    let (lock, condvar) = &**shared;
    let live = |link: &Link<T, H>| !link.closed && live(link);
    let mut link = condvar.wait_while(lock.lock().unwrap(), |link| live(link) && !ready(link)).unwrap();
    live(&link) && send(&mut link).is_ok() && link.flush().is_ok()
}

/// read the headunit until it leaves, handing each session event to the
/// host, which returns false to end the connection. the link is left closed
pub fn serve<T: Tls, H>(
    shared: &Shared<T, H>,
    mut reader: impl Read,
    mut handle: impl FnMut(&mut Link<T, H>, Event) -> bool,
) -> Result<(), String> {
    let mut buffer = vec![0; READ_BUFFER];
    let result = loop {
        let n = match reader.read(&mut buffer) {
            Ok(0) => break Ok(()),
            Ok(n) => n,
            Err(e) => break Err(format!("read: {e}")),
        };
        let mut link = shared.0.lock().unwrap();
        let events = match link.session.receive(&buffer[..n]) {
            Ok(events) => events,
            Err(e) => break Err(format!("session: {e:?}")),
        };
        let running = events.into_iter().all(|event| handle(&mut link, event));
        if let Err(e) = link.flush() {
            break Err(e);
        }
        shared.1.notify_all();
        if !running {
            break Ok(());
        }
    };
    shared.0.lock().unwrap().closed = true;
    shared.1.notify_all();
    result
}
