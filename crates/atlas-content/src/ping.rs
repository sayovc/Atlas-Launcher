//! Server List Ping: the same "status" request the game sends to fill in
//! the multiplayer screen.

use atlas_shared::{AtlasError, AtlasResult, ErrorCode};
use serde::Serialize;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug, Clone, Serialize)]
pub struct PingResult {
    pub version: String,
    pub protocol: i64,
    pub online: i64,
    pub max: i64,
    pub motd: String,
    pub icon: Option<String>,
    pub latency_ms: u64,
}

fn varint(mut v: i32, out: &mut Vec<u8>) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v = ((v as u32) >> 7) as i32;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            break;
        }
    }
}

async fn read_varint(s: &mut TcpStream) -> std::io::Result<i32> {
    let mut out = 0i32;
    for i in 0..5 {
        let b = s.read_u8().await?;
        out |= ((b & 0x7f) as i32) << (7 * i);
        if b & 0x80 == 0 {
            return Ok(out);
        }
    }
    Err(std::io::Error::other("varint too long"))
}

fn packet(id: i32, body: &[u8]) -> Vec<u8> {
    let mut inner = vec![];
    varint(id, &mut inner);
    inner.extend_from_slice(body);
    let mut out = vec![];
    varint(inner.len() as i32, &mut out);
    out.extend(inner);
    out
}

/// Flatten a chat component (string or {text, extra}) into plain text.
fn flatten(v: &serde_json::Value, out: &mut String) {
    match v {
        serde_json::Value::String(s) => out.push_str(s),
        serde_json::Value::Object(o) => {
            if let Some(t) = o.get("text").and_then(|t| t.as_str()) {
                out.push_str(t);
            }
            if let Some(extra) = o.get("extra").and_then(|e| e.as_array()) {
                for e in extra {
                    flatten(e, out);
                }
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|e| flatten(e, out)),
        _ => {}
    }
}

fn strip_formatting(s: &str) -> String {
    let mut out = String::new();
    let mut skip = false;
    for c in s.chars() {
        if skip {
            skip = false;
        } else if c == '§' {
            skip = true;
        } else {
            out.push(c);
        }
    }
    out
}

pub async fn ping(host: &str, port: u16) -> AtlasResult<PingResult> {
    let fail = |e: String| {
        AtlasError::new(ErrorCode::NetworkError, format!("Couldn't reach {host}:{port}."))
            .details(e)
            .suggest("The server may be offline, or the address may be wrong.")
    };
    let started = Instant::now();
    let work = async {
        let mut s = TcpStream::connect((host, port)).await.map_err(|e| fail(e.to_string()))?;
        let mut hs = vec![];
        varint(-1, &mut hs); // "any protocol", fine for status
        varint(host.len() as i32, &mut hs);
        hs.extend_from_slice(host.as_bytes());
        hs.extend_from_slice(&port.to_be_bytes());
        varint(1, &mut hs); // next state: status
        s.write_all(&packet(0, &hs)).await.map_err(|e| fail(e.to_string()))?;
        s.write_all(&packet(0, &[])).await.map_err(|e| fail(e.to_string()))?;
        let _len = read_varint(&mut s).await.map_err(|e| fail(e.to_string()))?;
        let id = read_varint(&mut s).await.map_err(|e| fail(e.to_string()))?;
        if id != 0 {
            return Err(fail(format!("unexpected packet {id}")));
        }
        let n = read_varint(&mut s).await.map_err(|e| fail(e.to_string()))?;
        if !(0..=1_048_576).contains(&n) {
            return Err(fail("status response too large".into()));
        }
        let mut buf = vec![0u8; n as usize];
        s.read_exact(&mut buf).await.map_err(|e| fail(e.to_string()))?;
        let latency = started.elapsed().as_millis() as u64;
        let v: serde_json::Value = serde_json::from_slice(&buf).map_err(|e| fail(e.to_string()))?;
        let mut motd = String::new();
        if let Some(d) = v.get("description") {
            flatten(d, &mut motd);
        }
        Ok(PingResult {
            version: v.pointer("/version/name").and_then(|x| x.as_str()).unwrap_or("?").to_string(),
            protocol: v.pointer("/version/protocol").and_then(|x| x.as_i64()).unwrap_or(0),
            online: v.pointer("/players/online").and_then(|x| x.as_i64()).unwrap_or(0),
            max: v.pointer("/players/max").and_then(|x| x.as_i64()).unwrap_or(0),
            motd: strip_formatting(motd.trim()),
            icon: v.get("favicon").and_then(|f| f.as_str()).and_then(|f| f.strip_prefix("data:image/png;base64,")).map(|s| s.replace('\n', "")),
            latency_ms: latency,
        })
    };
    tokio::time::timeout(Duration::from_secs(6), work).await.map_err(|_| fail("timed out".into()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// A pretend Minecraft server that answers one status request.
    async fn fake_server(json: &'static str) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            // read handshake + status request (two packets)
            for _ in 0..2 {
                let len = read_varint(&mut s).await.unwrap();
                let mut b = vec![0; len as usize];
                s.read_exact(&mut b).await.unwrap();
            }
            let mut body = vec![];
            varint(json.len() as i32, &mut body);
            body.extend_from_slice(json.as_bytes());
            s.write_all(&packet(0, &body)).await.unwrap();
        });
        port
    }

    #[tokio::test]
    async fn reads_status() {
        let port = fake_server(r#"{"version":{"name":"26.3","protocol":800},"players":{"max":100,"online":7},"description":{"text":"§aHello ","extra":[{"text":"world"}]},"favicon":"data:image/png;base64,iVBORw0KGgo="}"#).await;
        let r = ping("127.0.0.1", port).await.unwrap();
        assert_eq!(r.version, "26.3");
        assert_eq!((r.online, r.max), (7, 100));
        assert_eq!(r.motd, "Hello world");
        assert_eq!(r.icon.as_deref(), Some("iVBORw0KGgo="));
    }

    #[tokio::test]
    async fn offline_server_is_a_clear_error() {
        let e = ping("127.0.0.1", 9).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::NetworkError);
    }

    #[test]
    fn varints() {
        let mut v = vec![];
        varint(300, &mut v);
        assert_eq!(v, vec![0xac, 0x02]);
        let mut v = vec![];
        varint(-1, &mut v);
        assert_eq!(v, vec![0xff, 0xff, 0xff, 0xff, 0x0f]);
    }
}
