use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use wasm_bindgen::prelude::*;
use web_sys::{BinaryType, MessageEvent, WebSocket};

/// How long a dial waits for the bridge to open the socket.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

enum Request {
    Open {
        id: u64,
        port: u16,
        inbound: Sender<Inbound>,
    },
    Send(u64, Vec<u8>),
    Close(u64),
}

enum Inbound {
    Opened,
    Data(Vec<u8>),
    Closed,
}

fn requests() -> &'static (Sender<Request>, Receiver<Request>) {
    static REQUESTS: OnceLock<(Sender<Request>, Receiver<Request>)> = OnceLock::new();
    REQUESTS.get_or_init(unbounded)
}

struct Shared {
    id: u64,
    inbound: Receiver<Inbound>,
    /// Bytes received but not yet read, and whether the socket has closed.
    pending: Mutex<(Vec<u8>, usize, bool)>,
    timeout: Mutex<Option<Duration>>,
}

impl Drop for Shared {
    fn drop(&mut self) {
        let _ = requests().0.send(Request::Close(self.id));
    }
}

/// A socket on the page's bridge, with `std::net::TcpStream`'s blocking API. Its calls block, so
/// they run on a worker thread; the WebSocket itself lives on the main thread.
#[derive(Clone)]
pub struct TcpStream(Arc<Shared>);

impl TcpStream {
    pub fn connect(addr: &str) -> io::Result<Self> {
        let port = addr
            .rsplit_once(':')
            .and_then(|(_, p)| p.parse::<u16>().ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, addr.to_string()))?;
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = unbounded();
        requests()
            .0
            .send(Request::Open {
                id,
                port,
                inbound: tx,
            })
            .map_err(|_| io::Error::from(io::ErrorKind::NotConnected))?;
        let stream = Self(Arc::new(Shared {
            id,
            inbound: rx,
            pending: Mutex::new((Vec::new(), 0, false)),
            timeout: Mutex::new(None),
        }));
        match stream.0.inbound.recv_timeout(CONNECT_TIMEOUT) {
            Ok(Inbound::Opened) => Ok(stream),
            Ok(_) | Err(_) => Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!("the bridge did not open port {port}"),
            )),
        }
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        *self.0.timeout.lock().unwrap() = timeout;
        Ok(())
    }

    pub fn set_nodelay(&self, _nodelay: bool) -> io::Result<()> {
        Ok(())
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(self.clone())
    }
}

impl Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut pending = self.0.pending.lock().unwrap();
        loop {
            let (bytes, at, closed) = &mut *pending;
            if *at < bytes.len() {
                let n = buf.len().min(bytes.len() - *at);
                buf[..n].copy_from_slice(&bytes[*at..*at + n]);
                *at += n;
                return Ok(n);
            }
            if *closed {
                return Ok(0);
            }
            let timeout = *self.0.timeout.lock().unwrap();
            let next = match timeout {
                Some(t) => self.0.inbound.recv_timeout(t),
                None => self
                    .0
                    .inbound
                    .recv()
                    .map_err(|_| RecvTimeoutError::Disconnected),
            };
            match next {
                Ok(Inbound::Data(data)) => *pending = (data, 0, false),
                Ok(Inbound::Opened) => {}
                Ok(Inbound::Closed) | Err(RecvTimeoutError::Disconnected) => pending.2 = true,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(io::Error::from(io::ErrorKind::WouldBlock))
                }
            }
        }
    }
}

impl Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        requests()
            .0
            .send(Request::Send(self.0.id, buf.to_vec()))
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

thread_local! {
    static SOCKETS: RefCell<HashMap<u64, WebSocket>> = RefCell::default();
}

/// Carries out the sockets' requests; called every frame on the page's main thread.
pub fn pump() {
    while let Ok(request) = requests().1.try_recv() {
        match request {
            Request::Open { id, port, inbound } => open(id, port, inbound),
            Request::Send(id, bytes) => SOCKETS.with(|s| {
                if let Some(ws) = s.borrow().get(&id) {
                    // `send` refuses a view onto the shared wasm memory: copy out first.
                    let out = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
                    out.copy_from(&bytes);
                    if let Err(e) = ws.send_with_array_buffer(&out.buffer()) {
                        web_sys::console::error_2(&"net: send failed".into(), &e);
                    }
                }
            }),
            Request::Close(id) => SOCKETS.with(|s| {
                if let Some(ws) = s.borrow_mut().remove(&id) {
                    let _ = ws.close();
                }
            }),
        }
    }
}

fn open(id: u64, port: u16, inbound: Sender<Inbound>) {
    let location = web_sys::window().expect("a page").location();
    let scheme = if location.protocol().as_deref() == Ok("https:") {
        "wss"
    } else {
        "ws"
    };
    let host = location.host().unwrap_or_default();
    let url = format!("{scheme}://{host}/tcp/{port}");
    web_sys::console::log_1(&format!("net: opening {url}").into());
    let ws = match WebSocket::new(&url) {
        Ok(ws) => ws,
        Err(_) => {
            let _ = inbound.send(Inbound::Closed);
            return;
        }
    };
    ws.set_binary_type(BinaryType::Arraybuffer);
    let tx = inbound.clone();
    let on_open = Closure::<dyn FnMut()>::new(move || {
        let _ = tx.send(Inbound::Opened);
    });
    let tx = inbound.clone();
    let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        match e.data().dyn_into::<js_sys::ArrayBuffer>() {
            Ok(buffer) => {
                let _ = tx.send(Inbound::Data(js_sys::Uint8Array::new(&buffer).to_vec()));
            }
            Err(other) => web_sys::console::log_2(&"net: recv non-binary".into(), &other),
        }
    });
    let tx = inbound;
    let on_close = Closure::<dyn FnMut()>::new(move || {
        let _ = tx.send(Inbound::Closed);
    });
    ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    ws.set_onerror(Some(on_close.as_ref().unchecked_ref()));
    // The handlers live as long as the socket, which outlives this call.
    on_open.forget();
    on_message.forget();
    on_close.forget();
    SOCKETS.with(|s| s.borrow_mut().insert(id, ws));
}
