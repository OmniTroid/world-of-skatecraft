//! The sockets: `std::net::TcpStream` natively. On the web, a WebSocket to the page's own bridge,
//! `ws://<page host>/tcp/<port>`, which the page's main thread drives through [`pump`].

#[cfg(not(target_arch = "wasm32"))]
pub use std::net::TcpStream;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::{pump, TcpStream};

/// Connect to `host:port`.
pub fn connect(addr: &str) -> std::io::Result<TcpStream> {
    TcpStream::connect(addr)
}
