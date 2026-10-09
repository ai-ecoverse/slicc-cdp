//! std-shaped sockets over the WASIX calls of slicc's kernel.
#![allow(unsafe_code)]

use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

mod sys {
    #[link(wasm_import_module = "wasix_32v1")]
    extern "C" {
        pub fn sock_open(af: u32, socktype: u32, proto: u32, fd: *mut u32) -> u16;
        pub fn sock_bind(fd: u32, addr: *const u8) -> u16;
        pub fn sock_listen(fd: u32, backlog: u32) -> u16;
        pub fn sock_connect(fd: u32, addr: *const u8) -> u16;
        pub fn sock_accept_v2(fd: u32, flags: u32, out_fd: *mut u32, out_addr: *mut u8) -> u16;
        pub fn sock_addr_local(fd: u32, out: *mut u8) -> u16;
        pub fn sock_addr_peer(fd: u32, out: *mut u8) -> u16;
        pub fn sock_set_opt_flag(fd: u32, opt: u32, flag: u32) -> u16;
        pub fn sock_get_opt_flag(fd: u32, opt: u32, out: *mut u8) -> u16;
        pub fn resolve(
            name: *const u8,
            len: u32,
            port: u32,
            addrs: *mut u8,
            naddrs: u32,
            out: *mut u32,
        ) -> u16;
        pub fn fd_dup(fd: u32, out: *mut u32) -> u16;
    }

    #[link(wasm_import_module = "wasi_snapshot_preview1")]
    extern "C" {
        pub fn sock_shutdown(fd: u32, how: u32) -> u16;
        pub fn poll_oneoff(subs: *const u8, events: *mut u8, nsubs: u32, nevents: *mut u32) -> u16;
        pub fn fd_fdstat_get(fd: u32, out: *mut u8) -> u16;
        pub fn fd_fdstat_set_flags(fd: u32, flags: u32) -> u16;
    }
}

const AF_INET: u32 = 1;
const AF_INET6: u32 = 2;
const SOCK_STREAM: u32 = 1;
const OPT_REUSE_ADDR: u32 = 2;
const OPT_NO_DELAY: u32 = 3;
const FDFLAGS_NONBLOCK: u16 = 4;
const SHUT_RD: u32 = 1;
const SHUT_WR: u32 = 2;
/// `__wasi_addr_port_t` with room for the largest (unix) variant.
const ADDR_SIZE: usize = 128;
const ADDR_IP_SIZE: usize = 18;

fn cvt(errno: u16) -> io::Result<()> {
    match errno {
        0 => Ok(()),
        e => Err(io::Error::from_raw_os_error(i32::from(e))),
    }
}

fn encode(addr: &SocketAddr) -> [u8; ADDR_SIZE] {
    // Family tag, then the port (little-endian, as the kernel reads it) at
    // 2 and the address at 4.
    let mut out = [0u8; ADDR_SIZE];
    out[2..4].copy_from_slice(&addr.port().to_le_bytes());
    match addr.ip() {
        IpAddr::V4(ip) => {
            out[0] = AF_INET as u8;
            out[4..8].copy_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            out[0] = AF_INET6 as u8;
            out[4..20].copy_from_slice(&ip.octets());
        }
    }
    out
}

fn decode(raw: &[u8; ADDR_SIZE]) -> io::Result<SocketAddr> {
    let port = u16::from_le_bytes([raw[2], raw[3]]);
    match u32::from(raw[0]) {
        AF_INET => {
            let ip = Ipv4Addr::new(raw[4], raw[5], raw[6], raw[7]);
            Ok(SocketAddr::V4(SocketAddrV4::new(ip, port)))
        }
        AF_INET6 => {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&raw[4..20]);
            Ok(SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::from(octets),
                port,
                0,
                0,
            )))
        }
        _ => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "not an IP socket address",
        )),
    }
}

fn open(addr: &SocketAddr) -> io::Result<File> {
    let af = if addr.is_ipv4() { AF_INET } else { AF_INET6 };
    let mut fd = 0u32;
    cvt(unsafe { sys::sock_open(af, SOCK_STREAM, 0, &mut fd) })?;
    // SAFETY: `sock_open` returned a new descriptor that we now own.
    Ok(unsafe { File::from_raw_fd(fd as RawFd) })
}

fn fd_of(file: &File) -> u32 {
    file.as_raw_fd() as u32
}

fn sock_name(file: &File, peer: bool) -> io::Result<SocketAddr> {
    let mut raw = [0u8; ADDR_SIZE];
    let fd = fd_of(file);
    cvt(unsafe {
        if peer {
            sys::sock_addr_peer(fd, raw.as_mut_ptr())
        } else {
            sys::sock_addr_local(fd, raw.as_mut_ptr())
        }
    })?;
    decode(&raw)
}

fn dup(file: &File) -> io::Result<File> {
    let mut out = 0u32;
    cvt(unsafe { sys::fd_dup(fd_of(file), &mut out) })?;
    // SAFETY: `fd_dup` returned a new descriptor that we now own.
    Ok(unsafe { File::from_raw_fd(out as RawFd) })
}

fn set_nonblocking(file: &File, on: bool) -> io::Result<()> {
    // fdstat: filetype u8, then the flags (u16) at 2.
    let mut stat = [0u8; 24];
    let fd = fd_of(file);
    cvt(unsafe { sys::fd_fdstat_get(fd, stat.as_mut_ptr()) })?;
    let mut flags = u16::from_le_bytes([stat[2], stat[3]]);
    if on {
        flags |= FDFLAGS_NONBLOCK;
    } else {
        flags &= !FDFLAGS_NONBLOCK;
    }
    cvt(unsafe { sys::fd_fdstat_set_flags(fd, u32::from(flags)) })
}

/// Waits until `fd` is readable (or writable), for at most `timeout`.
/// The kernel takes socket timeouts as options but does not enforce them,
/// so they are kept here with `poll_oneoff`.
fn wait(fd: u32, write: bool, timeout: Duration) -> io::Result<()> {
    const SUB: usize = 48;
    const EVENT: usize = 32;
    let mut subs = [0u8; SUB * 2];
    // Subscription 1: the descriptor (eventtype 1 = fd_read, 2 = fd_write).
    subs[0..8].copy_from_slice(&1u64.to_le_bytes());
    subs[8] = if write { 2 } else { 1 };
    subs[16..20].copy_from_slice(&fd.to_le_bytes());
    // Subscription 2: a relative timeout on the monotonic clock.
    let clock = &mut subs[SUB..];
    clock[0..8].copy_from_slice(&2u64.to_le_bytes());
    clock[8] = 0;
    clock[16..20].copy_from_slice(&1u32.to_le_bytes());
    let nanos = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX);
    clock[24..32].copy_from_slice(&nanos.to_le_bytes());
    let mut events = [0u8; EVENT * 2];
    let mut n = 0u32;
    cvt(unsafe { sys::poll_oneoff(subs.as_ptr(), events.as_mut_ptr(), 2, &mut n) })?;
    for event in events.chunks_exact(EVENT).take(n as usize) {
        if u64::from_le_bytes(event[0..8].try_into().unwrap()) == 1 {
            return cvt(u16::from_le_bytes([event[8], event[9]]));
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "socket timed out"))
}

fn check_timeout(timeout: Option<Duration>) -> io::Result<u64> {
    match timeout {
        None => Ok(0),
        Some(d) if d.is_zero() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot set a 0 duration timeout",
        )),
        Some(d) => Ok(u64::try_from(d.as_nanos()).unwrap_or(u64::MAX).max(1)),
    }
}

fn load_timeout(cell: &AtomicU64) -> Option<Duration> {
    match cell.load(Ordering::Relaxed) {
        0 => None,
        n => Some(Duration::from_nanos(n)),
    }
}

/// Mirrors `std::net::ToSocketAddrs`. Names other than literal addresses
/// go to the kernel's resolver, which knows the loopback names.
pub trait ToSocketAddrs {
    type Iter: Iterator<Item = SocketAddr>;
    fn to_socket_addrs(&self) -> io::Result<Self::Iter>;
}

type Addrs = std::vec::IntoIter<SocketAddr>;

impl ToSocketAddrs for SocketAddr {
    type Iter = Addrs;
    fn to_socket_addrs(&self) -> io::Result<Addrs> {
        Ok(vec![*self].into_iter())
    }
}

impl ToSocketAddrs for SocketAddrV4 {
    type Iter = Addrs;
    fn to_socket_addrs(&self) -> io::Result<Addrs> {
        SocketAddr::V4(*self).to_socket_addrs()
    }
}

impl ToSocketAddrs for SocketAddrV6 {
    type Iter = Addrs;
    fn to_socket_addrs(&self) -> io::Result<Addrs> {
        SocketAddr::V6(*self).to_socket_addrs()
    }
}

impl ToSocketAddrs for (IpAddr, u16) {
    type Iter = Addrs;
    fn to_socket_addrs(&self) -> io::Result<Addrs> {
        SocketAddr::new(self.0, self.1).to_socket_addrs()
    }
}

impl ToSocketAddrs for (Ipv4Addr, u16) {
    type Iter = Addrs;
    fn to_socket_addrs(&self) -> io::Result<Addrs> {
        SocketAddr::new(IpAddr::V4(self.0), self.1).to_socket_addrs()
    }
}

impl ToSocketAddrs for (Ipv6Addr, u16) {
    type Iter = Addrs;
    fn to_socket_addrs(&self) -> io::Result<Addrs> {
        SocketAddr::new(IpAddr::V6(self.0), self.1).to_socket_addrs()
    }
}

impl ToSocketAddrs for (&str, u16) {
    type Iter = Addrs;
    fn to_socket_addrs(&self) -> io::Result<Addrs> {
        Ok(resolve(self.0, self.1)?.into_iter())
    }
}

impl ToSocketAddrs for (String, u16) {
    type Iter = Addrs;
    fn to_socket_addrs(&self) -> io::Result<Addrs> {
        (self.0.as_str(), self.1).to_socket_addrs()
    }
}

impl ToSocketAddrs for str {
    type Iter = Addrs;
    fn to_socket_addrs(&self) -> io::Result<Addrs> {
        if let Ok(addr) = self.parse::<SocketAddr>() {
            return addr.to_socket_addrs();
        }
        let (host, port) = self
            .rsplit_once(':')
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid socket address"))?;
        let port = port
            .parse::<u16>()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid port value"))?;
        (host, port).to_socket_addrs()
    }
}

impl ToSocketAddrs for String {
    type Iter = Addrs;
    fn to_socket_addrs(&self) -> io::Result<Addrs> {
        self.as_str().to_socket_addrs()
    }
}

impl<'a> ToSocketAddrs for &'a [SocketAddr] {
    type Iter = std::iter::Copied<std::slice::Iter<'a, SocketAddr>>;
    fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
        Ok(self.iter().copied())
    }
}

impl<T: ToSocketAddrs + ?Sized> ToSocketAddrs for &T {
    type Iter = T::Iter;
    fn to_socket_addrs(&self) -> io::Result<T::Iter> {
        (**self).to_socket_addrs()
    }
}

pub fn resolve(host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    // __wasi_addr_t: family tag, then the address at 2.
    let mut addrs = [0u8; ADDR_IP_SIZE * 4];
    let mut n = 0u32;
    cvt(unsafe {
        sys::resolve(
            host.as_ptr(),
            host.len() as u32,
            u32::from(port),
            addrs.as_mut_ptr(),
            4,
            &mut n,
        )
    })
    .map_err(|e| io::Error::new(e.kind(), format!("cannot resolve `{host}`: {e}")))?;
    let mut out = Vec::new();
    for a in addrs.chunks_exact(ADDR_IP_SIZE).take(n as usize) {
        match u32::from(a[0]) {
            AF_INET => out.push(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(a[2], a[3], a[4], a[5])),
                port,
            )),
            AF_INET6 => {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(&a[2..18]);
                out.push(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port));
            }
            _ => {}
        }
    }
    Ok(out)
}

fn each_addr<A: ToSocketAddrs + ?Sized, T>(
    addr: &A,
    mut f: impl FnMut(&SocketAddr) -> io::Result<T>,
) -> io::Result<T> {
    let mut last = None;
    for a in addr.to_socket_addrs()? {
        match f(&a) {
            Ok(t) => return Ok(t),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "could not resolve to any addresses",
        )
    }))
}

/// A TCP connection on the kernel's loopback network.
#[derive(Debug)]
pub struct TcpStream {
    file: File,
    read_timeout: AtomicU64,
    write_timeout: AtomicU64,
}

impl TcpStream {
    fn from_file(file: File) -> TcpStream {
        TcpStream {
            file,
            read_timeout: AtomicU64::new(0),
            write_timeout: AtomicU64::new(0),
        }
    }

    pub fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<TcpStream> {
        each_addr(&addr, |a| {
            let file = open(a)?;
            let raw = encode(a);
            cvt(unsafe { sys::sock_connect(fd_of(&file), raw.as_ptr()) })?;
            Ok(TcpStream::from_file(file))
        })
    }

    /// Loopback connections complete at once, so the timeout only has to be
    /// valid.
    pub fn connect_timeout(addr: &SocketAddr, timeout: Duration) -> io::Result<TcpStream> {
        check_timeout(Some(timeout))?;
        TcpStream::connect(addr)
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        sock_name(&self.file, true)
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        sock_name(&self.file, false)
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        let how = match how {
            Shutdown::Read => SHUT_RD,
            Shutdown::Write => SHUT_WR,
            Shutdown::Both => SHUT_RD | SHUT_WR,
        };
        cvt(unsafe { sys::sock_shutdown(fd_of(&self.file), how) })
    }

    /// A second handle on the same connection. Timeouts are per handle.
    pub fn try_clone(&self) -> io::Result<TcpStream> {
        let clone = TcpStream::from_file(dup(&self.file)?);
        clone
            .read_timeout
            .store(self.read_timeout.load(Ordering::Relaxed), Ordering::Relaxed);
        clone.write_timeout.store(
            self.write_timeout.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        Ok(clone)
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.read_timeout
            .store(check_timeout(timeout)?, Ordering::Relaxed);
        Ok(())
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.write_timeout
            .store(check_timeout(timeout)?, Ordering::Relaxed);
        Ok(())
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(load_timeout(&self.read_timeout))
    }

    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(load_timeout(&self.write_timeout))
    }

    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        cvt(unsafe { sys::sock_set_opt_flag(fd_of(&self.file), OPT_NO_DELAY, u32::from(nodelay)) })
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        let mut out = 0u8;
        cvt(unsafe { sys::sock_get_opt_flag(fd_of(&self.file), OPT_NO_DELAY, &mut out) })?;
        Ok(out != 0)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        set_nonblocking(&self.file, nonblocking)
    }
}

impl Read for &TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(timeout) = load_timeout(&self.read_timeout) {
            wait(fd_of(&self.file), false, timeout)?;
        }
        (&self.file).read(buf)
    }
}

impl Write for &TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(timeout) = load_timeout(&self.write_timeout) {
            wait(fd_of(&self.file), true, timeout)?;
        }
        (&self.file).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        (&*self).read(buf)
    }
}

impl Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A listening TCP socket on the kernel's loopback network.
#[derive(Debug)]
pub struct TcpListener {
    file: File,
}

impl TcpListener {
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<TcpListener> {
        each_addr(&addr, |a| {
            let file = open(a)?;
            let fd = fd_of(&file);
            // As std does on unix; the kernel may not know the option.
            let _ = unsafe { sys::sock_set_opt_flag(fd, OPT_REUSE_ADDR, 1) };
            let raw = encode(a);
            cvt(unsafe { sys::sock_bind(fd, raw.as_ptr()) })?;
            cvt(unsafe { sys::sock_listen(fd, 128) })?;
            Ok(TcpListener { file })
        })
    }

    pub fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        let mut fd = 0u32;
        let mut raw = [0u8; ADDR_SIZE];
        cvt(unsafe { sys::sock_accept_v2(fd_of(&self.file), 0, &mut fd, raw.as_mut_ptr()) })?;
        // SAFETY: `sock_accept_v2` returned a new descriptor that we now own.
        let stream = TcpStream::from_file(unsafe { File::from_raw_fd(fd as RawFd) });
        let peer = decode(&raw).or_else(|_| stream.peer_addr())?;
        Ok((stream, peer))
    }

    pub fn incoming(&self) -> Incoming<'_> {
        Incoming { listener: self }
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        sock_name(&self.file, false)
    }

    pub fn try_clone(&self) -> io::Result<TcpListener> {
        Ok(TcpListener {
            file: dup(&self.file)?,
        })
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        set_nonblocking(&self.file, nonblocking)
    }
}

/// The connections a [`TcpListener`] accepts, without end.
#[derive(Debug)]
pub struct Incoming<'a> {
    listener: &'a TcpListener,
}

impl Iterator for Incoming<'_> {
    type Item = io::Result<TcpStream>;
    fn next(&mut self) -> Option<io::Result<TcpStream>> {
        Some(self.listener.accept().map(|(s, _)| s))
    }
}

macro_rules! fd_traits {
    ($ty:ident, $make:expr) => {
        impl AsRawFd for $ty {
            fn as_raw_fd(&self) -> RawFd {
                self.file.as_raw_fd()
            }
        }
        impl AsFd for $ty {
            fn as_fd(&self) -> BorrowedFd<'_> {
                self.file.as_fd()
            }
        }
        impl IntoRawFd for $ty {
            fn into_raw_fd(self) -> RawFd {
                self.file.into_raw_fd()
            }
        }
        impl FromRawFd for $ty {
            unsafe fn from_raw_fd(fd: RawFd) -> $ty {
                $make(File::from_raw_fd(fd))
            }
        }
        impl From<$ty> for OwnedFd {
            fn from(s: $ty) -> OwnedFd {
                s.file.into()
            }
        }
        impl From<OwnedFd> for $ty {
            fn from(fd: OwnedFd) -> $ty {
                $make(File::from(fd))
            }
        }
    };
}

fd_traits!(TcpStream, TcpStream::from_file);
fd_traits!(TcpListener, |file| TcpListener { file });
