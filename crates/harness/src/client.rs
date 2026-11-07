use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

pub struct Event {
    pub name: String,
    pub data: Value,
    pub at: Instant,
}

pub struct Client {
    outgoing: mpsc::UnboundedSender<Message>,
    events: mpsc::UnboundedReceiver<Event>,
    author: Option<String>,
}

impl Client {
    pub async fn connect(port: u16, cookie: Option<&str>) -> Result<Client> {
        match cookie {
            Some(c) => Client::connect_with(port, &[("cookie", c)]).await,
            None => Client::connect_with(port, &[]).await,
        }
    }

    pub async fn connect_with(port: u16, headers: &[(&'static str, &str)]) -> Result<Client> {
        let url = format!("ws://127.0.0.1:{port}/socket.io/?EIO=4&transport=websocket");
        let mut request = url.into_client_request()?;
        for (name, value) in headers {
            request.headers_mut().insert(*name, value.parse()?);
        }
        let (ws, _) = tokio_tungstenite::connect_async(request)
            .await
            .context("websocket connect")?;
        let (mut sink, mut stream) = ws.split();

        let open = next_text(&mut stream).await?;
        if !open.starts_with('0') {
            bail!("expected engine.io open, got {open}");
        }
        sink.send(Message::Text("40".into())).await?;
        loop {
            let frame = next_text(&mut stream).await?;
            if frame == "2" {
                sink.send(Message::Text("3".into())).await?;
            } else if frame.starts_with("40") {
                break;
            } else {
                bail!("socket.io connect refused: {frame}");
            }
        }

        let (outgoing, mut to_send) = mpsc::unbounded_channel::<Message>();
        let (deliver, events) = mpsc::unbounded_channel::<Event>();
        let pong = outgoing.clone();
        tokio::spawn(async move {
            while let Some(msg) = to_send.recv().await {
                let closing = matches!(msg, Message::Close(_));
                if sink.send(msg).await.is_err() || closing {
                    return;
                }
            }
        });
        tokio::spawn(async move {
            while let Some(Ok(msg)) = stream.next().await {
                let Message::Text(text) = msg else { continue };
                let at = Instant::now();
                if text == "2" {
                    let _ = pong.send(Message::Text("3".into()));
                } else if let Some(body) = text.strip_prefix("42")
                    && let Some(event) = parse_event(body, at)
                    && deliver.send(event).is_err()
                {
                    return;
                }
            }
        });
        Ok(Client {
            outgoing,
            events,
            author: None,
        })
    }

    pub fn emit(&self, name: &str, data: Value) -> Result<()> {
        let frame = format!("42{}", Value::Array(vec![Value::String(name.into()), data]));
        self.outgoing
            .send(Message::Text(frame))
            .map_err(|_| anyhow!("connection closed"))
    }

    pub async fn wait_for(&mut self, timeout: Duration, mut wanted: impl FnMut(&Event) -> bool) -> Result<Event> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Ok(Some(e)) if wanted(&e) => return Ok(e),
                Ok(Some(_)) => continue,
                Ok(None) => bail!("connection closed"),
                Err(_) => bail!("timed out after {timeout:?}"),
            }
        }
    }

    pub async fn quiet_for(&mut self, window: Duration, mut unwanted: impl FnMut(&Event) -> bool) -> Result<()> {
        let deadline = tokio::time::Instant::now() + window;
        loop {
            match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Ok(Some(e)) if unwanted(&e) => bail!("unexpected {} event: {}", e.name, e.data),
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => return Ok(()),
            }
        }
    }

    pub fn authored_by(mut self, uuid: &str) -> Client {
        self.author = Some(uuid.to_owned());
        self
    }

    pub fn drain(mut self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move { while self.events.recv().await.is_some() {} })
    }

    pub async fn watch(&mut self, thread: &str) -> Result<()> {
        self.emit("watch", Value::String(thread.into()))?;
        self.wait_for(Duration::from_secs(5), |e| e.name == "watcher_count")
            .await?;
        Ok(())
    }

    pub fn post(&self, thread: &str, text: &str) -> Result<()> {
        self.emit("post", serde_json::json!({ "thread_name": thread, "text": text }))
    }

    pub async fn own_post(&mut self, text: &str, timeout: Duration) -> Result<Event> {
        let author = self.author.clone();
        self.wait_for(timeout, |e| {
            let p = &e.data["post"];
            e.name == "post" && p["rawText"] == text && author.as_deref().is_none_or(|a| p["authorUUID"] == a)
        })
        .await
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.outgoing.send(Message::Close(None));
    }
}

async fn next_text<S>(stream: &mut S) -> Result<String>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let read = async {
        while let Some(msg) = stream.next().await {
            if let Message::Text(t) = msg? {
                return Ok(t);
            }
        }
        bail!("connection closed during handshake")
    };
    tokio::time::timeout(Duration::from_secs(5), read)
        .await
        .context("handshake timed out")?
}

fn parse_event(body: &str, at: Instant) -> Option<Event> {
    let mut parts = match serde_json::from_str::<Value>(body).ok()? {
        Value::Array(a) => a.into_iter(),
        _ => return None,
    };
    let name = parts.next()?.as_str()?.to_owned();
    let data = parts.next().unwrap_or(Value::Null);
    Some(Event { name, data, at })
}

#[cfg(test)]
mod tests {
    #[test]
    fn event_frames_parse() {
        let e = super::parse_event(r#"["post",{"post":{"rawText":"7"}}]"#, std::time::Instant::now()).unwrap();
        assert_eq!(e.name, "post");
        assert_eq!(e.data["post"]["rawText"], "7");
        assert!(super::parse_event(r#"{"not":"an array"}"#, std::time::Instant::now()).is_none());
    }
}
