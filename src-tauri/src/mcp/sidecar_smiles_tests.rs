//! Tests for `SidecarSmiles` (ADR-027 decision 4, M1b Part B). No running sidecar: each HTTP test
//! serves one canned response from a throwaway `TcpListener` on `127.0.0.1:0`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::charge_mult::ChargeMult;
use super::classify::SourceKind;
use super::resolve::{resolve_smiles, SmilesTo3d};
use super::sidecar_smiles::{check_reply, SidecarSmiles};
use crate::sidecar::SidecarStatus;

const QUICK: Duration = Duration::from_secs(5);

/// Water as RDKit's `MolToXYZBlock` writes it (count, empty comment, rows), in the sidecar's reply.
const WATER_XYZ: &str = "3\n\nO      0.006180    0.398030    0.000000\nH     -0.766720   -0.193330    0.000000\nH      0.760540   -0.204700    0.000000\n";

fn reply(xyz: &str, charge: &str, multiplicity: &str, num_atoms: &str) -> String {
    format!(
        r#"{{"xyz":{},"formula":"H2O","charge":{charge},"multiplicity":{multiplicity},"num_atoms":{num_atoms}}}"#,
        serde_json::to_string(xyz).unwrap()
    )
}

fn water() -> String {
    reply(WATER_XYZ, "0", "1", "3")
}

fn http(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Read one HTTP request: the head up to the blank line, then `Content-Length` bytes of body.
fn read_request(stream: &mut std::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).unwrap() == 0 {
            break;
        }
        buf.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&buf).to_string();
    let len = head
        .lines()
        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).unwrap();
    head + &String::from_utf8_lossy(&body)
}

/// Serve `response` to one connection after `delay`; the request comes back on the channel.
fn serve_once_after(response: String, delay: Duration) -> (u16, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request(&mut stream);
        tx.send(request).ok();
        std::thread::sleep(delay);
        stream.write_all(response.as_bytes()).ok();
    });
    (port, rx)
}

fn serve_once(response: String) -> (u16, mpsc::Receiver<String>) {
    serve_once_after(response, Duration::ZERO)
}

/// The sidecar at `port` answering `response`.
fn call(response: String) -> Result<super::resolve::SmilesStructure, String> {
    let (port, _rx) = serve_once(response);
    SidecarSmiles::at_port(port, QUICK).smiles_to_3d("O")
}

/// A port nothing listens on: bound, then released.
fn closed_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

#[test]
fn a_good_reply_becomes_atoms_and_the_request_is_the_sidecars_shape() {
    let (port, rx) = serve_once(http("200 OK", &water()));
    let s = SidecarSmiles::at_port(port, QUICK).smiles_to_3d("O").unwrap();
    assert_eq!((s.num_atoms, s.charge, s.multiplicity), (3, 0, 1));
    assert_eq!(s.xyz, WATER_XYZ);

    let request = rx.recv().unwrap();
    assert!(request.starts_with("POST /smiles-to-3d HTTP/1.1\r\n"), "{request}");
    assert!(request.to_ascii_lowercase().contains("content-type: application/json"), "{request}");
    assert!(request.ends_with(r#"{"smiles":"O"}"#), "the body is SmilesToXyzRequest: {request}");
}

#[test]
fn a_good_reply_resolves_through_the_seam() {
    let (port, _rx) = serve_once(http("200 OK", &water()));
    let source = resolve_smiles("O", &SidecarSmiles::at_port(port, QUICK), "smiles:O").unwrap();
    assert_eq!(source.kind, SourceKind::Smiles);
    assert_eq!(source.atoms.iter().map(|a| a.element.as_str()).collect::<Vec<_>>(), ["O", "H", "H"]);
    assert_eq!((source.atoms[1].x_ang, source.atoms[1].y_ang), (-0.76672, -0.19333), "Å, as written");
    assert_eq!(source.charge_mult, Ok(ChargeMult { charge: 0, multiplicity: 1 }));
}

#[test]
fn an_http_error_is_an_err_naming_the_status() {
    let e = call(http("500 Internal Server Error", r#"{"detail":"boom"}"#)).unwrap_err();
    assert!(e.contains("HTTP 500") && e.contains("boom"), "{e}");
    let e = call(http("400 Bad Request", r#"{"detail":"Invalid SMILES"}"#)).unwrap_err();
    assert!(e.contains("HTTP 400") && e.contains("Invalid SMILES"), "{e}");
}

#[test]
fn a_2xx_other_than_200_is_an_err() {
    let e = call(http("202 Accepted", &water())).unwrap_err();
    assert!(e.contains("HTTP 202"), "{e}");
}

#[test]
fn a_malformed_body_is_an_err() {
    for body in ["{\"xyz\": ", "<html>ok</html>", "", "{}"] {
        let e = call(http("200 OK", body)).unwrap_err();
        assert!(e.contains("not a structure"), "{body:?}: {e}");
    }
}

#[test]
fn a_nan_or_infinite_coordinate_is_an_err() {
    for bad in ["NaN", "inf", "-inf"] {
        let xyz = WATER_XYZ.replace("0.398030", bad);
        let e = call(http("200 OK", &reply(&xyz, "0", "1", "3"))).unwrap_err();
        assert!(e.contains("not a finite number"), "{bad}: {e}");
    }
}

#[test]
fn an_unknown_element_is_an_err() {
    let xyz = WATER_XYZ.replacen("H ", "Xx", 1);
    let e = call(http("200 OK", &reply(&xyz, "0", "1", "3"))).unwrap_err();
    assert!(e.contains("unknown element `Xx`"), "{e}");
}

#[test]
fn zero_atoms_is_an_err() {
    let e = call(http("200 OK", &reply("0\n\n", "0", "1", "0"))).unwrap_err();
    assert!(e.contains("no atoms"), "{e}");
}

#[test]
fn a_closed_port_is_an_err() {
    let e = SidecarSmiles::at_port(closed_port(), QUICK).smiles_to_3d("O").unwrap_err();
    assert!(e.contains("could not be reached"), "{e}");
}

#[test]
fn a_hung_sidecar_is_an_err_within_the_timeout() {
    let (port, _rx) = serve_once_after(http("200 OK", &water()), Duration::from_secs(3));
    let start = Instant::now();
    let e = SidecarSmiles::at_port(port, Duration::from_millis(300)).smiles_to_3d("O").unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(2), "the call is bounded: {:?}", start.elapsed());
    assert!(e.contains("could not be reached"), "{e}");
}

fn status(status: &str, port: Option<u16>) -> SidecarStatus {
    SidecarStatus { status: status.into(), port, version: None, expected_version: "0.2.0".into() }
}

/// No port, or a sidecar that is down, is an error at the call, with no request made: the port here
/// is closed, so a request would read "could not be reached" instead.
#[test]
fn a_sidecar_not_started_or_down_is_an_err_before_any_request() {
    let e = SidecarSmiles::from_status(&status("down", None)).smiles_to_3d("O").unwrap_err();
    assert!(e.contains("port is unknown"), "{e}");
    let e = SidecarSmiles::from_status(&status("down", Some(closed_port()))).smiles_to_3d("O").unwrap_err();
    assert!(e.contains("not running"), "{e}");
    let e = SidecarSmiles::from_status(&status("healthy", Some(closed_port()))).smiles_to_3d("O").unwrap_err();
    assert!(e.contains("could not be reached"), "a healthy one is called: {e}");
}

// --- The post-condition on its own (`check_reply`) ---------------------------------------------

#[test]
fn check_reply_accepts_the_sidecars_shape() {
    assert_eq!(check_reply(&water()).unwrap().num_atoms, 3);
    // Element case is normalised as the classifier does (C6), and a later sidecar may add fields.
    let lower = WATER_XYZ.replacen("O ", "o ", 1);
    assert!(check_reply(&reply(&lower, "0", "1", "3")).is_ok());
    let extra = water().replacen('{', r#"{"smiles_canonical":"O","#, 1);
    assert!(check_reply(&extra).is_ok(), "{extra}");
}

#[test]
fn charge_and_multiplicity_must_be_integers_and_multiplicity_1() {
    for (charge, mult) in [("0.5", "1"), ("\"0\"", "1"), ("0", "1.0"), ("0", "\"1\""), ("null", "1")] {
        assert!(check_reply(&reply(WATER_XYZ, charge, mult, "3")).is_err(), "charge {charge}, multiplicity {mult}");
    }
    let e = check_reply(&reply(WATER_XYZ, "0", "2", "3")).unwrap_err();
    assert!(e.contains("multiplicity 2"), "{e}");
    assert!(check_reply(&reply(WATER_XYZ, "-1", "1", "3")).is_ok(), "a charge is the sidecar's (R5 checks the parity)");
}

#[test]
fn the_xyz_must_be_exactly_one_frame_of_num_atoms_rows() {
    let cases = [
        ("count line not num_atoms", reply(WATER_XYZ, "0", "1", "2")),
        ("a row short", reply(&WATER_XYZ.replace("H      0.760540   -0.204700    0.000000\n", ""), "0", "1", "3")),
        ("a fifth field", reply(&WATER_XYZ.replace("0.398030", "0.398030 9"), "0", "1", "3")),
        ("a fragment suffix", reply(&WATER_XYZ.replacen("O ", "O(1) ", 1), "0", "1", "3")),
        ("a second frame", reply(&format!("{WATER_XYZ}{WATER_XYZ}"), "0", "1", "3")),
        ("num_atoms negative", reply(WATER_XYZ, "0", "1", "-3")),
        ("num_atoms huge", reply(WATER_XYZ, "0", "1", "18446744073709551615")),
    ];
    for (what, body) in cases {
        assert!(check_reply(&body).is_err(), "{what}");
    }
}
