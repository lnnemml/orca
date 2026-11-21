//! The accept loop of one start: peer check, connection slot, then one `rmcp` service per
//! connection, all on Tauri's runtime.
//!
//! - **Peer check** (decision 2): `SO_PEERCRED` on every accept; a peer whose uid differs from ours
//!   is dropped. The uid source is injected ([`PeerUid`]), so a test can play a foreign uid (no
//!   second account exists: Open question c′).
//! - **Slots** (decision 1): at most [`CONNECTIONS`] connections. A further one is closed at once. A
//!   slot frees only once its connection's last request permit has returned ([`ConnState::drain`]),
//!   so the slots are the lifecycle's, shared by every start: a draining connection of an older
//!   start still counts.
//! - **The registry** of one start holds its live connections, so switch-off can set every
//!   `closed` flag. Once shut, it registers nothing, so a connection accepted during switch-off is
//!   closed rather than missed.

use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

use rmcp::ServiceExt;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use super::create_job_task::CreateJobWiring;
use super::service::{ConnState, HostPaths, McpService};
use super::DbSource;

/// The peer's uid, from `SO_PEERCRED` in production.
pub type PeerUid = Arc<dyn Fn(&UnixStream) -> io::Result<u32> + Send + Sync>;

/// `SO_PEERCRED`, through tokio's `UnixStream::peer_cred`.
pub fn kernel_peer_uid() -> PeerUid {
    Arc::new(|stream: &UnixStream| stream.peer_cred().map(|cred| cred.uid()))
}

/// This process's uid.
pub fn own_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// The peer check: only a peer whose uid is known and equal to ours is served.
pub fn peer_allowed(peer: &io::Result<u32>, own_uid: u32) -> bool {
    matches!(peer, Ok(uid) if *uid == own_uid)
}

/// The live connections of one start.
#[derive(Default)]
pub struct Registry {
    inner: Mutex<RegistryInner>,
}

#[derive(Default)]
struct RegistryInner {
    shut: bool,
    conns: Vec<Arc<ConnState>>,
}

impl Registry {
    fn inner(&self) -> MutexGuard<'_, RegistryInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A new connection's state, or `None` once [`Registry::shut`] has run.
    fn register(&self) -> Option<Arc<ConnState>> {
        let mut inner = self.inner();
        if inner.shut {
            return None;
        }
        let conn = Arc::new(ConnState::new());
        inner.conns.push(conn.clone());
        Some(conn)
    }

    fn remove(&self, conn: &Arc<ConnState>) {
        self.inner().conns.retain(|c| !Arc::ptr_eq(c, conn));
    }

    /// Switch-off: set every connection's `closed` flag and register nothing more.
    pub fn shut(&self) {
        let mut inner = self.inner();
        inner.shut = true;
        for conn in &inner.conns {
            conn.closed.store(true, Ordering::SeqCst);
        }
    }

    /// The registered connections (tests, and M3's checks).
    pub fn connections(&self) -> Vec<Arc<ConnState>> {
        self.inner().conns.clone()
    }
}

/// What the accept loop of one start needs.
#[derive(Clone)]
pub struct AcceptContext {
    pub cancel: CancellationToken,
    pub registry: Arc<Registry>,
    pub slots: Arc<Semaphore>,
    /// Transports currently open, across starts (the status's `connections`).
    pub connected: Arc<AtomicUsize>,
    pub peer_uid: PeerUid,
    pub own_uid: u32,
    pub db: Arc<dyn DbSource>,
    pub host: Arc<HostPaths>,
    pub create_job: CreateJobWiring,
}

/// Accept until `ctx.cancel` fires. The listener (and so the socket's fd) closes when this returns.
pub async fn accept_loop(listener: UnixListener, ctx: AcceptContext) {
    loop {
        let stream = tokio::select! {
            _ = ctx.cancel.cancelled() => return,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(e) => {
                    // EMFILE and the like: back off rather than spin, and keep serving.
                    eprintln!("mcp: accept failed: {e}");
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }
            },
        };
        admit(stream, &ctx);
    }
}

/// Serve `stream`, or drop it (which closes it) if the peer, the slots or the registry refuse.
fn admit(stream: UnixStream, ctx: &AcceptContext) {
    if !peer_allowed(&(ctx.peer_uid)(&stream), ctx.own_uid) {
        return;
    }
    let Ok(slot) = ctx.slots.clone().try_acquire_owned() else {
        return;
    };
    let Some(conn) = ctx.registry.register() else {
        return;
    };
    let service = McpService::new(conn.clone(), ctx.db.clone(), ctx.host.clone(), ctx.create_job.clone());
    let task = serve_connection(stream, service, conn, slot, ctx.clone());
    tauri::async_runtime::spawn(task);
}

async fn serve_connection(
    stream: UnixStream,
    service: McpService,
    conn: Arc<ConnState>,
    slot: OwnedSemaphorePermit,
    ctx: AcceptContext,
) {
    ctx.connected.fetch_add(1, Ordering::SeqCst);
    let transport = Transport { stream, conn: conn.clone() };
    // A child token: switch-off cancels the start's token, and with it every service.
    if let Ok(running) = service.serve_with_ct(transport, ctx.cancel.child_token()).await {
        let _ = running.waiting().await;
    }
    // Already set at the read EOF, unless the service ended another way (a cancel, an error).
    conn.closed.store(true, Ordering::SeqCst);
    ctx.connected.fetch_sub(1, Ordering::SeqCst);
    conn.drain().await;
    ctx.registry.remove(&conn);
    drop(slot);
}

/// The connection's stream, which sets `closed` the moment its read side ends (EOF or an error).
///
/// `rmcp`'s `waiting()` returns only after its response drain, up to 5 s after the transport
/// closed while handlers are in flight (`rmcp-3.11.0/src/service.rs:1834-1836`: `Closed` → 5 s,
/// `Cancelled` → 2 s). A waiting handler must see the hang-up at once, not after that drain.
struct Transport {
    stream: UnixStream,
    conn: Arc<ConnState>,
}

impl AsyncRead for Transport {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let polled = Pin::new(&mut this.stream).poll_read(cx, buf);
        let ended = match &polled {
            Poll::Ready(Ok(())) => buf.filled().len() == before && buf.remaining() > 0,
            Poll::Ready(Err(_)) => true,
            Poll::Pending => false,
        };
        if ended {
            this.conn.closed.store(true, Ordering::SeqCst);
        }
        polled
    }
}

impl AsyncWrite for Transport {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}
