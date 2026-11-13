//! Test-only HTTP client helpers.
#![allow(clippy::unwrap_used)]

use std::net::SocketAddr;

use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use hyper::{HeaderMap, Request, StatusCode};
use hyper_util::rt::TokioIo;
use sha2::{Digest, Sha256};

pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    /// `Err` when the connection was cut in the middle of the body.
    pub body: Result<Vec<u8>, usize>,
}

pub fn sha256(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub async fn get(addr: SocketAddr, path: &str, headers: &[(&str, &str)]) -> Reply {
    let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .unwrap();
    tokio::spawn(conn);
    let mut req = Request::get(path).header("host", addr.to_string());
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let resp = sender
        .send_request(req.body(Empty::<Bytes>::new()).unwrap())
        .await
        .unwrap();
    let (parts, mut body) = resp.into_parts();
    let mut data = Vec::new();
    let body = loop {
        match body.frame().await {
            None => break Ok(data),
            Some(Ok(f)) => {
                if let Ok(d) = f.into_data() {
                    data.extend_from_slice(&d);
                }
            }
            Some(Err(_)) => break Err(data.len()),
        }
    };
    Reply {
        status: parts.status,
        headers: parts.headers,
        body,
    }
}

pub fn header<'a>(r: &'a Reply, name: &str) -> &'a str {
    r.headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}
