use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::relay::*;
use super::test_support::{eventually, Scratch};

/// stdin as a channel: each message is a chunk, and dropping the sender is EOF.
struct ChanReader {
    rx: mpsc::Receiver<Vec<u8>>,
    buf: Vec<u8>,
}

impl Read for ChanReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.buf.is_empty() {
            match self.rx.recv() {
                Ok(chunk) => self.buf = chunk,
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.buf.len());
        out[..n].copy_from_slice(&self.buf[..n]);
        self.buf.drain(..n);
        Ok(n)
    }
}

fn stdin() -> (mpsc::Sender<Vec<u8>>, ChanReader) {
    let (tx, rx) = mpsc::channel();
    (tx, ChanReader { rx, buf: Vec::new() })
}

/// A shared byte sink for stdout and stderr.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Write for Sink {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Sink {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

/// Run the relay on a thread; its end arrives on the returned channel.
fn spawn_relay(input: ChanReader, out: Sink, err: Sink, socket: std::path::PathBuf) -> mpsc::Receiver<RelayEnd> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(relay(input, out, err, &socket));
    });
    rx
}

/// Control (M2 amendment item 1): exiting 0, or writing no stderr line, when connect fails. And
/// nothing reaches stdout.
#[test]
fn a_failed_connect_writes_one_stderr_line_and_exits_non_zero() {
    let scratch = Scratch::new("relay-down");
    let (out, err) = (Sink::default(), Sink::default());
    let (_tx, input) = stdin();
    let end = relay(input, out.clone(), err.clone(), &scratch.dir.join("absent.sock"));
    assert_eq!(end, RelayEnd::NotRunning);
    assert_ne!(end.code(), 0);
    assert_eq!(String::from_utf8(err.bytes()).unwrap(), format!("{NOT_RUNNING}\n"));
    assert!(out.bytes().is_empty(), "stdout carries MCP bytes only");
}

/// Control (M2 amendment item 1): writing anything but MCP bytes to stdout. The relay carries a
/// request up and a reply down byte for byte, and stdout holds exactly the server's bytes.
#[test]
fn stdout_carries_exactly_the_servers_bytes_and_stdin_eof_ends_the_relay() {
    let scratch = Scratch::new("relay-echo");
    let socket = scratch.dir.join("s.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut seen = Vec::new();
        for line in BufReader::new(stream).lines() {
            let line = line.unwrap();
            writer.write_all(format!("{{\"echo\":{line}}}\n").as_bytes()).unwrap();
            seen.push(line);
        }
        seen // ends on the relay's close
    });

    let (out, err) = (Sink::default(), Sink::default());
    let (tx, input) = stdin();
    let ended = spawn_relay(input, out.clone(), err.clone(), socket);
    tx.send(b"{\"id\":1}\n".to_vec()).unwrap();
    tx.send(b"{\"id\":2}\n".to_vec()).unwrap();
    let expected = b"{\"echo\":{\"id\":1}}\n{\"echo\":{\"id\":2}}\n".to_vec();
    assert!(eventually(Duration::from_secs(3), || out.bytes() == expected), "{:?}", String::from_utf8_lossy(&out.bytes()));

    drop(tx); // the client session ends
    assert_eq!(ended.recv_timeout(Duration::from_secs(3)).unwrap(), RelayEnd::ClientClosed);
    assert_eq!(server.join().unwrap(), vec!["{\"id\":1}", "{\"id\":2}"], "the relay closed the socket");
    assert_eq!(out.bytes(), expected, "nothing but the server's bytes on stdout");
    assert!(err.bytes().is_empty());
}

/// Control (M2 amendment item 1): not exiting on socket EOF. The server closes while stdin stays
/// open; the relay ends anyway, non-zero, after relaying what the server sent.
#[test]
fn socket_eof_ends_the_relay_while_stdin_is_still_open() {
    let scratch = Scratch::new("relay-eof");
    let socket = scratch.dir.join("s.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(b"bye\n").unwrap();
        // dropped: socket EOF
    });

    let (out, err) = (Sink::default(), Sink::default());
    let (_tx, input) = stdin(); // held open for the whole test
    let ended = spawn_relay(input, out.clone(), err, socket);
    let end = ended.recv_timeout(Duration::from_secs(3)).expect("the relay ends on socket EOF");
    assert_eq!(end, RelayEnd::ServerClosed);
    assert_ne!(end.code(), 0);
    assert_eq!(out.bytes(), b"bye\n");
}
