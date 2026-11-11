use serde_json::{Value, json};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

pub fn pattern(cmd: &str) -> String {
    format!("{{\"cmd\":{}}}", serde_json::to_string(cmd).unwrap())
}

fn frame(msg: &Value) -> Vec<u8> {
    let s = msg.to_string();
    let len: usize = s.chars().map(char::len_utf16).sum();
    let mut out = format!("{len}#").into_bytes();
    out.extend_from_slice(s.as_bytes());
    out
}

#[derive(Default)]
pub struct Decoder {
    bytes: Vec<u8>,
}

impl Decoder {
    pub fn push(&mut self, data: &[u8]) {
        self.bytes.extend_from_slice(data);
    }

    pub fn next_message(&mut self) -> Result<Option<Value>, String> {
        let Some(hash) = self.bytes.iter().position(|&b| b == b'#') else {
            return Ok(None);
        };
        let len: usize = std::str::from_utf8(&self.bytes[..hash])
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .ok_or_else(|| "corrupted packet length".to_string())?;
        let body = &self.bytes[hash + 1..];
        let mut units = 0;
        let mut i = 0;
        while units < len {
            let Some(&b) = body.get(i) else { return Ok(None) };
            let w = match b {
                0x00..=0x7F => 1,
                0xC0..=0xDF => 2,
                0xE0..=0xEF => 3,
                _ => 4,
            };
            if i + w > body.len() {
                return Ok(None);
            }
            units += if w == 4 { 2 } else { 1 };
            i += w;
        }
        let msg = serde_json::from_slice(&body[..i]).map_err(|e| e.to_string());
        self.bytes.drain(..hash + 1 + i);
        msg.map(Some)
    }
}

fn peer_closed(sock: &TcpStream) -> bool {
    let mut discard = [0u8; 512];
    loop {
        match sock.try_read(&mut discard) {
            Ok(0) => return true,
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return false,
            Err(_) => return true,
        }
    }
}

#[derive(Clone)]
pub struct Client {
    tx: mpsc::Sender<Value>,
}

impl Client {
    pub fn connect(name: &'static str, addr: String) -> Client {
        let (tx, mut rx) = mpsc::channel::<Value>(10_000);
        tokio::spawn(async move {
            let mut pending: Option<Value> = None;
            loop {
                let mut sock = match TcpStream::connect(&addr).await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::debug!("nest {name}: connect {addr}: {e}");
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        continue;
                    }
                };
                let _ = sock.set_nodelay(true);
                tracing::info!("nest {name}: connected to {addr}");
                loop {
                    let msg = match pending.take() {
                        Some(m) => m,
                        None => tokio::select! {
                            m = rx.recv() => match m {
                                Some(m) => m,
                                None => return,
                            },
                            _ = sock.readable() => {
                                if peer_closed(&sock) {
                                    tracing::warn!("nest {name}: {addr} closed the connection; reconnecting");
                                    break;
                                }
                                continue;
                            }
                        },
                    };
                    if peer_closed(&sock) {
                        tracing::warn!("nest {name}: {addr} closed the connection; reconnecting");
                        pending = Some(msg);
                        break;
                    }
                    if let Err(e) = sock.write_all(&frame(&msg)).await {
                        tracing::warn!("nest {name}: write failed ({e}); reconnecting");
                        pending = Some(msg);
                        break;
                    }
                }
            }
        });
        Client { tx }
    }

    pub fn emit(&self, cmd: &str, data: Value) {
        if self
            .tx
            .try_send(json!({ "pattern": pattern(cmd), "data": data }))
            .is_err()
        {
            static DROPPED: AtomicU64 = AtomicU64::new(0);
            static LAST_WARN: AtomicU64 = AtomicU64::new(0);
            let n = DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
            let now = crate::now_ms() as u64;
            let last = LAST_WARN.load(Ordering::Relaxed);
            if now.saturating_sub(last) >= 10_000
                && LAST_WARN
                    .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
            {
                tracing::warn!("nest: outbound queue full; {n} events dropped so far (latest {cmd})");
            }
        }
    }
}

#[derive(Clone)]
pub struct Caller {
    name: &'static str,
    addr: String,
    conn: Arc<tokio::sync::Mutex<Option<CallerConn>>>,
    next_id: Arc<AtomicU64>,
}

struct CallerConn {
    write: tokio::net::tcp::OwnedWriteHalf,
    waiting: Arc<dashmap::DashMap<String, tokio::sync::oneshot::Sender<Value>>>,
}

impl Caller {
    pub fn new(name: &'static str, addr: String) -> Caller {
        Caller {
            name,
            addr,
            conn: Arc::default(),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    pub async fn call(&self, cmd: &str, data: Value, timeout: Duration) -> Option<Value> {
        let id = format!("{}-{}", crate::now_ms(), self.next_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut guard = self.conn.lock().await;
            if guard.is_none() {
                *guard = self.open().await;
            }
            let conn = guard.as_mut()?;
            conn.waiting.insert(id.clone(), tx);
            let msg = json!({ "pattern": pattern(cmd), "data": data, "id": id });
            if let Err(e) = conn.write.write_all(&frame(&msg)).await {
                tracing::warn!("nest {}: call {cmd} failed: {e}", self.name);
                *guard = None;
                return None;
            }
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(v)) => Some(v),
            _ => None,
        }
    }

    async fn open(&self) -> Option<CallerConn> {
        let sock = TcpStream::connect(&self.addr)
            .await
            .map_err(|e| tracing::warn!("nest {}: connect {}: {e}", self.name, self.addr))
            .ok()?;
        let _ = sock.set_nodelay(true);
        let (mut rd, write) = sock.into_split();
        let waiting: Arc<dashmap::DashMap<String, tokio::sync::oneshot::Sender<Value>>> = Arc::default();
        let w = waiting.clone();
        let slot = self.conn.clone();
        tokio::spawn(async move {
            let mut dec = Decoder::default();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = match rd.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                dec.push(&buf[..n]);
                while let Ok(Some(msg)) = dec.next_message() {
                    let Some(id) = msg.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    if let Some((_, tx)) = w.remove(id) {
                        let _ = tx.send(msg.get("response").cloned().unwrap_or(Value::Null));
                    }
                }
            }
            *slot.lock().await = None;
        });
        Some(CallerConn { write, waiting })
    }
}

pub type Reply = Option<Value>;
pub type Handler = Arc<dyn Fn(String, Value) -> Pin<Box<dyn Future<Output = Reply> + Send>> + Send + Sync>;

pub fn bind_v6_only(addr: std::net::Ipv6Addr, port: u16) -> std::io::Result<TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
    s.set_only_v6(true)?;
    #[cfg(unix)]
    s.set_reuse_address(true)?;
    s.bind(&std::net::SocketAddr::from((addr, port)).into())?;
    s.listen(1024)?;
    s.set_nonblocking(true)?;
    TcpListener::from_std(s.into())
}

pub async fn serve(port: u16, handler: Handler) -> anyhow::Result<()> {
    let v4 = TcpListener::bind(("127.0.0.1", port)).await?;
    let v6 = bind_v6_only(std::net::Ipv6Addr::LOCALHOST, port).ok();
    tracing::info!("nest command server on localhost:{port} (ipv6 {})", v6.is_some());
    if let Some(v6) = v6 {
        tokio::spawn(accept_loop(v6, handler.clone()));
    }
    accept_loop(v4, handler).await
}

async fn accept_loop(listener: TcpListener, handler: Handler) -> anyhow::Result<()> {
    loop {
        let (sock, _) = listener.accept().await?;
        let handler = handler.clone();
        tokio::spawn(async move {
            let _ = sock.set_nodelay(true);
            let (mut rd, wr) = sock.into_split();
            let wr = Arc::new(tokio::sync::Mutex::new(wr));
            let mut dec = Decoder::default();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = match rd.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                dec.push(&buf[..n]);
                loop {
                    let msg = match dec.next_message() {
                        Ok(Some(m)) => m,
                        Ok(None) => break,
                        Err(e) => {
                            tracing::warn!("nest: {e}; closing connection");
                            return;
                        }
                    };
                    let cmd = command_of(&msg["pattern"]);
                    let data = msg.get("data").cloned().unwrap_or(Value::Null);
                    let id = msg.get("id").cloned();
                    let (handler, wr) = (handler.clone(), wr.clone());
                    tokio::spawn(async move {
                        let resp = handler(cmd, data).await;
                        if let Some(id) = id {
                            let mut out = json!({ "id": id, "isDisposed": true });
                            if let Some(r) = resp {
                                out["response"] = r;
                            }
                            let _ = wr.lock().await.write_all(&frame(&out)).await;
                        }
                    });
                }
            }
        });
    }
}

fn command_of(p: &Value) -> String {
    let obj = match p {
        Value::String(s) => serde_json::from_str::<Value>(s).unwrap_or(Value::String(s.clone())),
        other => other.clone(),
    };
    obj.get("cmd")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| obj.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_round_trip_counts_utf16() {
        let m = json!({ "pattern": pattern("x"), "data": { "name": "émoji 😀" } });
        let f = frame(&m);
        let s = String::from_utf8(f.clone()).unwrap();
        let (len, body) = s.split_once('#').unwrap();
        assert_eq!(len.parse::<usize>().unwrap(), body.encode_utf16().count());
        let mut d = Decoder::default();
        d.push(&f[..7]);
        assert_eq!(d.next_message().unwrap(), None);
        d.push(&f[7..]);
        d.push(&f);
        assert_eq!(d.next_message().unwrap().unwrap(), m);
        assert_eq!(d.next_message().unwrap().unwrap(), m);
        assert_eq!(d.next_message().unwrap(), None);
        assert_eq!(command_of(&m["pattern"]), "x");
    }

    async fn read_message(conn: &mut TcpStream) -> Value {
        let mut d = Decoder::default();
        let mut buf = [0u8; 4096];
        loop {
            if let Some(m) = d.next_message().unwrap() {
                return m;
            }
            let n = conn.read(&mut buf).await.unwrap();
            assert!(n > 0, "connection closed before a message arrived");
            d.push(&buf[..n]);
        }
    }

    #[tokio::test]
    async fn a_message_sent_after_the_server_restarts_is_not_lost() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let client = Client::connect("test", addr);
        let (first, _) = listener.accept().await.unwrap();
        drop(first);
        tokio::time::sleep(Duration::from_millis(100)).await;
        client.emit("dailyStatsReport", json!({ "n": 1 }));
        let (mut second, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
            .await
            .expect("client never reconnected")
            .unwrap();
        let m = tokio::time::timeout(Duration::from_secs(5), read_message(&mut second))
            .await
            .expect("message never arrived");
        assert_eq!(m["data"]["n"], 1);
        assert_eq!(command_of(&m["pattern"]), "dailyStatsReport");
    }
}
