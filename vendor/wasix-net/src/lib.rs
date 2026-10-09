//! TCP sockets and HTTP for WASI programs in slicc's kernel.
//!
//! Rust std for `wasm32-wasip1` stubs `TcpStream::connect` and
//! `TcpListener::bind`. slicc's kernel implements the WASIX socket calls
//! (`sock_open`, `sock_connect`, `sock_bind`, `sock_listen`,
//! `sock_accept_v2`) on its own loopback network, so on WASI preview1 this
//! crate's [`TcpStream`] and [`TcpListener`] use them. On every other target
//! they are std's, so code written against them needs no cfg: switch the
//! import and keep to the API both share.
//!
//! [`http`] is a small blocking HTTP/1.1 client over those sockets. It has
//! no TLS: in the realm every request leaves through the kernel's proxy
//! (`https_proxy` / `http_proxy`), which takes absolute-form requests and
//! does TLS itself, while hosts in `no_proxy` (the realm's loopback) are
//! reached directly.

#[cfg(all(target_os = "wasi", target_env = "p1"))]
mod wasix;

#[cfg(all(target_os = "wasi", target_env = "p1"))]
pub use wasix::{Incoming, TcpListener, TcpStream, ToSocketAddrs};

#[cfg(not(all(target_os = "wasi", target_env = "p1")))]
pub use std::net::{Incoming, TcpListener, TcpStream, ToSocketAddrs};

pub use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4, SocketAddrV6};

pub mod http;

/// The addresses of `host` (a name or a literal address) at `port`.
///
/// In the realm only loopback names resolve (`localhost`), besides literal
/// addresses; elsewhere this is the system resolver.
pub fn resolve(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    #[cfg(all(target_os = "wasi", target_env = "p1"))]
    return wasix::resolve(host, port);
    #[cfg(not(all(target_os = "wasi", target_env = "p1")))]
    {
        let host = host.trim_start_matches('[').trim_end_matches(']');
        Ok(ToSocketAddrs::to_socket_addrs(&(host, port))?.collect())
    }
}
