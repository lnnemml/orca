//! The `orcastudio --mcp` relay (ADR-027 decision 2, "The relay"; M2 amendment, item 3): a dumb
//! byte relay, stdin → socket and socket → stdout. It holds no logic, so no guard lives in it.
//!
//! - It ends on **stdin EOF** (the client session ended: it closes the socket) and on **socket
//!   EOF** (the app went away or switched MCP off), whichever comes first.
//! - If it cannot connect, it writes exactly one line to **stderr**, [`NOT_RUNNING`], and exits
//!   non-zero.
//! - **stdout carries MCP bytes only**: nothing but what the socket sent is ever written there. Every
//!   diagnostic goes to stderr.
//!
//! The function is generic over its three streams, so a test can drive it. Part B's `main.rs`
//! branch passes the process's own.

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;

/// The one stderr line when the socket cannot be reached.
pub const NOT_RUNNING: &str = "OrcaStudio is not running, or its MCP server is off";

/// How the relay ended. Each maps to an exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayEnd {
    /// stdin reached EOF: the client ended the session. Exit 0.
    ClientClosed,
    /// The socket reached EOF: the server went away. Exit 1.
    ServerClosed,
    /// The socket could not be reached. Exit 2.
    NotRunning,
    /// A stream failed mid-relay. Exit 1.
    Failed,
}

impl RelayEnd {
    pub fn code(self) -> u8 {
        match self {
            RelayEnd::ClientClosed => 0,
            RelayEnd::ServerClosed | RelayEnd::Failed => 1,
            RelayEnd::NotRunning => 2,
        }
    }
}

impl From<RelayEnd> for ExitCode {
    fn from(end: RelayEnd) -> Self {
        ExitCode::from(end.code())
    }
}

/// Which direction ended first, and how.
enum Ended {
    Stdin(io::Result<()>),
    Socket(io::Result<()>),
}

/// Relay `stdin` → the socket at `socket_path` → `stdout` until either side ends.
pub fn relay<I, O, E>(stdin: I, stdout: O, mut stderr: E, socket_path: &Path) -> RelayEnd
where
    I: Read + Send + 'static,
    O: Write + Send + 'static,
    E: Write,
{
    let Ok(socket) = UnixStream::connect(socket_path) else {
        let _ = writeln!(stderr, "{NOT_RUNNING}");
        return RelayEnd::NotRunning;
    };
    let (up, down) = match (socket.try_clone(), socket.try_clone()) {
        (Ok(up), Ok(down)) => (up, down),
        (Err(e), _) | (_, Err(e)) => {
            let _ = writeln!(stderr, "orcastudio --mcp: cannot use the socket: {e}");
            return RelayEnd::Failed;
        }
    };

    let (tx, rx) = mpsc::channel();
    let to_server = tx.clone();
    // Detached: a blocked stdin read cannot be interrupted, and the process exits after we return.
    thread::spawn(move || {
        let _ = to_server.send(Ended::Stdin(pump(stdin, up)));
    });
    let to_client = thread::spawn(move || {
        let _ = tx.send(Ended::Socket(pump(down, stdout)));
    });

    let ended = rx.recv();
    let _ = socket.shutdown(Shutdown::Both);
    match ended {
        Ok(Ended::Stdin(Ok(()))) => {
            // Let the socket → stdout side finish writing what it already read.
            let _ = to_client.join();
            RelayEnd::ClientClosed
        }
        Ok(Ended::Socket(Ok(()))) => RelayEnd::ServerClosed,
        Ok(Ended::Stdin(Err(e)) | Ended::Socket(Err(e))) => {
            let _ = writeln!(stderr, "orcastudio --mcp: relay failed: {e}");
            RelayEnd::Failed
        }
        Err(_) => RelayEnd::Failed,
    }
}

/// Copy `from` → `to` until EOF, flushing after every chunk so a reply is never held back.
fn pump(mut from: impl Read, mut to: impl Write) -> io::Result<()> {
    let mut buf = [0u8; 8192];
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        to.write_all(&buf[..n])?;
        to.flush()?;
    }
}
