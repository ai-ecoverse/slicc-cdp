use super::Error;
#[cfg(any(unix, target_os = "wasi"))]
use std::time::{Duration, Instant};

pub(super) fn configured() -> bool {
    ["SLICC_CDP_LEASE_URL", "SLICC_LEASE_FD"]
        .iter()
        .any(|key| std::env::var_os(key).is_some())
}

#[cfg(any(unix, target_os = "wasi"))]
const LIMIT: usize = 128 * 1024;
#[cfg(any(unix, target_os = "wasi"))]
const TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(target_os = "wasi")]
pub(super) fn exchange(request: &str, unconfirmed: Error) -> Result<Vec<u8>, Error> {
    let selector = std::env::var("SLICC_CDP_LEASE_URL").map_err(|_| Error::Unavailable)?;
    let mut ws =
        cdp_client::net::ws_connect_kernel_lease(&selector).map_err(|_| Error::Unavailable)?;
    ws.send_text(request).map_err(|_| unconfirmed)?;
    let reply = ws
        .recv_text_limited(Instant::now() + TIMEOUT, LIMIT)
        .map_err(|_| unconfirmed)?
        .ok_or(unconfirmed)?;
    Ok(reply.into_bytes())
}

#[cfg(all(unix, not(target_os = "wasi")))]
pub(super) fn exchange(request: &str, unconfirmed: Error) -> Result<Vec<u8>, Error> {
    use std::io::{Read, Write};
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::net::UnixStream;

    if std::env::var("SLICC_LEASE_FD").as_deref() != Ok("3")
        || std::env::var_os("SLICC_CDP_LEASE_URL").is_some()
    {
        return Err(Error::Unavailable);
    }
    let fd = unsafe { libc::fcntl(3, libc::F_DUPFD_CLOEXEC, 4) };
    if fd == -1 {
        return Err(Error::Unavailable);
    }
    let mut socket = UnixStream::from(unsafe { OwnedFd::from_raw_fd(fd) });
    if !socket
        .peer_addr()
        .map_err(|_| Error::Unavailable)?
        .is_unnamed()
        || !socket
            .local_addr()
            .map_err(|_| Error::Unavailable)?
            .is_unnamed()
    {
        return Err(Error::Unavailable);
    }
    let mut kind: libc::c_int = 0;
    let mut len = size_of_val(&kind) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&raw mut kind).cast(),
            &mut len,
        )
    } != 0
        || kind != libc::SOCK_STREAM
    {
        return Err(Error::Unavailable);
    }
    socket
        .set_write_timeout(Some(TIMEOUT))
        .map_err(|_| Error::Unavailable)?;
    let deadline = Instant::now() + TIMEOUT;
    socket
        .write_all(format!("{request}\n").as_bytes())
        .map_err(|_| unconfirmed)?;
    let mut reply = Vec::new();
    let mut buf = [0; 8192];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(unconfirmed)?;
        socket
            .set_read_timeout(Some(remaining))
            .map_err(|_| unconfirmed)?;
        let n = socket.read(&mut buf).map_err(|_| unconfirmed)?;
        if n == 0 || reply.len() + n > LIMIT {
            return Err(unconfirmed);
        }
        reply.extend_from_slice(&buf[..n]);
        if buf[..n].contains(&b'\n') {
            return Ok(reply);
        }
    }
}

#[cfg(not(any(unix, target_os = "wasi")))]
pub(super) fn exchange(_: &str, _: Error) -> Result<Vec<u8>, Error> {
    Err(Error::Unavailable)
}
