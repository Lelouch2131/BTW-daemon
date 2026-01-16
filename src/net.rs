use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

/// Best-effort connectivity probe.
///
/// Uses a short TCP connect to a well-known public resolver IP.
/// This avoids DNS and should be fast/fail-safe.
pub fn has_internet(timeout_ms: u64) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::new(1, 1, 1, 1), 53));
    TcpStream::connect_timeout(&addr, Duration::from_millis(timeout_ms)).is_ok()
}
