//! The control-channel client: talks a source Craft app over its loopback JSON-lines control
//! channel (`--control PORT`). One connection; one JSON object per line in each direction.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::{Value, json};

use crate::{Error, LinkTarget, Result};

const READ_TIMEOUT: Duration = Duration::from_secs(65);

/// A session with a source app's control channel.
pub struct SourceClient {
    addr: String,
    conn: Option<(BufReader<TcpStream>, TcpStream)>,
    next_id: u64,
}

impl SourceClient {
    /// Connect to the source app's control port on loopback.
    pub fn connect(target: &LinkTarget) -> Result<Self> {
        let addr = format!("127.0.0.1:{}", target.port);
        let sa = addr.to_socket_addrs().map_err(|e| Error::Bridge(format!("{addr}: {e}")))?.next().ok_or_else(|| Error::BadAddr(addr.clone()))?;
        let s = TcpStream::connect_timeout(&sa, Duration::from_secs(5)).map_err(|_| Error::Unreachable(target.port))?;
        s.set_read_timeout(Some(READ_TIMEOUT)).ok();
        s.set_nodelay(true).ok();
        let r = s.try_clone().map_err(|e| Error::Bridge(e.to_string()))?;
        Ok(Self { addr, conn: Some((BufReader::new(r), s)), next_id: 1 })
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let mut last = Error::Bridge("not attempted".into());
        for attempt in 0..2 {
            let mut conn = self.conn.take();
            let Some((r, w)) = conn.as_mut() else { continue };
            let id = self.next_id;
            self.next_id = self.next_id.checked_add(1).ok_or_else(|| Error::Bridge("control request id space exhausted".into()))?;
            let line = format!("{}\n", json!({"id": id, "method": method, "params": params}));
            let res = (|| -> Result<std::result::Result<Value, Error>> {
                if let Err(e) = w.write_all(line.as_bytes()) {
                    return Ok(Err(Error::Bridge(e.to_string())));
                }
                let mut buf = String::new();
                match r.read_line(&mut buf) {
                    Ok(0) => return Ok(Err(Error::Bridge("connection closed by the source app".into()))),
                    Ok(_) => {}
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                        return Err(Error::Bridge(format!("no reply to `{method}` within {}s", READ_TIMEOUT.as_secs())));
                    }
                    Err(e) => return Ok(Err(Error::Bridge(e.to_string()))),
                }
                serde_json::from_str(&buf).map(Ok).map_err(|e| Error::Bridge(format!("bad reply: {e}")))
            })();
            self.conn = conn;
            match res {
                Ok(Ok(v)) if v.get("ok").and_then(Value::as_bool) == Some(true) => return Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                Ok(Ok(v)) => return Err(Error::App(v.get("error").and_then(Value::as_str).unwrap_or("error").to_string())),
                Ok(Err(e)) => {
                    last = e;
                    if attempt == 1 {
                        break;
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }

    /// The source project's linkable items (comps / sequences).
    /// The source project's revision: a counter that changes with every edit there. A linked
    /// preview compares it to the one its cached frames were made at.
    pub fn revision(&mut self) -> Result<u64> {
        let v = self.call("dynamicLink.revision", json!({}))?;
        v.get("revision").and_then(Value::as_u64).ok_or_else(|| Error::Other("source did not return a `revision`".into()))
    }

    pub fn items(&mut self) -> Result<Vec<super::model::LinkItem>> {        let v = self.call("dynamicLink.items", json!({}))?;
        let arr = v.get("items").and_then(Value::as_array).ok_or_else(|| Error::Other("source did not return an `items` list".into()))?;
        arr.iter()
            .map(|item| {
                Ok(super::model::LinkItem {
                    id: item.get("id").and_then(Value::as_u64).ok_or_else(|| Error::Other("item missing `id`".into()))?,
                    name: item.get("name").and_then(Value::as_str).ok_or_else(|| Error::Other("item missing `name`".into()))?.to_string(),
                    width: item.get("width").and_then(Value::as_u64).ok_or_else(|| Error::Other("item missing `width`".into()))? as u32,
                    height: item.get("height").and_then(Value::as_u64).ok_or_else(|| Error::Other("item missing `height`".into()))? as u32,
                    frame_rate_num: item.get("frameRateNum").and_then(Value::as_u64).unwrap_or(0) as u32,
                    frame_rate_den: item.get("frameRateDen").and_then(Value::as_u64).unwrap_or(1) as u32,
                    duration_seconds: item.get("durationSeconds").and_then(Value::as_f64).unwrap_or(0.0),
                })
            })
            .collect()
    }

    /// Run one engine command in the source app, e.g. `engine.execute` with `{"command": …}`.
    pub fn execute(&mut self, method: &str, params: Value) -> Result<Value> {
        self.call(method, params)
    }

    /// One frame of `item` at `time_seconds` as PNG bytes.
    pub fn frame(&mut self, item: u64, time_seconds: f64, max_side: u32) -> Result<(Vec<u8>, u32, u32)> {
        match self.frame_at(item, time_seconds, max_side, None)? {
            (Some(f), _) => Ok(f),
            (None, _) => Err(Error::Other("source answered `unchanged` to a plain frame request".into())),
        }
    }

    /// One frame of `item`, with the source project's revision. Given `known_revision` — the
    /// revision the caller's copy of this very frame was made at — the source answers `None`
    /// (nothing to send) while the project is still at that revision, so a preview keeps
    /// re-checking that it is current at the cost of a round trip, not a render.
    pub fn frame_at(&mut self, item: u64, time_seconds: f64, max_side: u32, known_revision: Option<u64>) -> Result<(Option<(Vec<u8>, u32, u32)>, u64)> {
        let mut params = json!({"item": item, "time": time_seconds, "maxSide": max_side, "base64": true});
        if let Some(rev) = known_revision {
            params["revision"] = json!(rev);
        }
        let v = self.call("dynamicLink.frame", params)?;
        let revision = v.get("revision").and_then(Value::as_u64).unwrap_or(0);
        if v.get("unchanged").and_then(Value::as_bool).unwrap_or(false) {
            return Ok((None, revision));
        }
        let png_b64 = v.get("png").and_then(Value::as_str).ok_or_else(|| Error::Other("source did not return a `png` frame".into()))?;
        let width = v.get("width").and_then(Value::as_u64).ok_or_else(|| Error::Other("frame missing `width`".into()))? as u32;
        let height = v.get("height").and_then(Value::as_u64).ok_or_else(|| Error::Other("frame missing `height`".into()))? as u32;
        let png = base64_decode(png_b64).ok_or_else(|| Error::Other("source frame was not valid base64".into()))?;
        Ok((Some((png, width, height)), revision))
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    use base64::engine::{general_purpose::STANDARD, Engine as _};
    STANDARD.decode(input).ok()
}
