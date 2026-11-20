//! Addresses for the nodes of a test to listen on.
//!
//! A node binds its own address, so a test has to name one nothing holds and
//! let go of it first. Asking the system for any free port and releasing it
//! is not enough: such a port comes from the range the system also gives
//! outgoing connections their local ports from, and the nodes of the tests
//! running beside this one open many. Now and then one of them, or another
//! test asking for a free port, was given the port between its release and
//! the node's bind (`cannot bind the consensus listener … Address already
//! in use`, and a node of one test heard by another's).
//!
//! So the ports are taken below that range, and each is given once in the
//! life of the process: a counter says which comes next, started from the
//! process id so that two test binaries running at once do not walk the
//! same ports.

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::atomic::{AtomicU32, Ordering};

/// The ports given: below 32768, where Linux starts handing out the local
/// ports of outgoing connections (`net.ipv4.ip_local_port_range`), and above
/// the ones the development services listen on.
const FIRST: u32 = 20_000;
const SPAN: u32 = 12_000;

static NEXT: AtomicU32 = AtomicU32::new(0);

/// `count` loopback addresses nothing listens on, none of them given before
/// in this process.
pub fn free_addrs(count: usize) -> Vec<SocketAddr> {
    let start = std::process::id().wrapping_mul(7919);
    let mut found = Vec::with_capacity(count);
    // Every port of the span tried once at most: more would mean none is
    // free, which no retry cures.
    for _ in 0..SPAN {
        let port = FIRST + start.wrapping_add(NEXT.fetch_add(1, Ordering::Relaxed)) % SPAN;
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port as u16));
        // Bound to see that nothing holds it, and let go of at once.
        if TcpListener::bind(addr).is_ok() {
            found.push(addr);
            if found.len() == count {
                return found;
            }
        }
    }
    panic!("no {count} free ports between {FIRST} and {}", FIRST + SPAN);
}
