use redis::aio::{ConnectionLike, ConnectionManager};
use redis::{Cmd, ErrorKind, Pipeline, RedisError, RedisFuture, RedisResult, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_AFTER: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Slot {
    manager: Option<ConnectionManager>,
    failed_at: Option<Instant>,
}

#[derive(Clone)]
pub struct Redis {
    client: redis::Client,
    slot: Arc<Mutex<Slot>>,
}

fn unavailable(why: &'static str) -> RedisError {
    RedisError::from((ErrorKind::IoError, why))
}

impl Redis {
    pub fn new(url: &str) -> RedisResult<Redis> {
        Ok(Redis {
            client: redis::Client::open(url)?,
            slot: Arc::default(),
        })
    }

    pub async fn warm_up(&self) {
        match self.manager().await {
            Ok(_) => tracing::info!("redis connected"),
            Err(e) => tracing::warn!("redis unavailable at startup, will retry on use: {e}"),
        }
    }

    async fn manager(&self) -> RedisResult<ConnectionManager> {
        let mut slot = self.slot.lock().await;
        if let Some(m) = &slot.manager {
            return Ok(m.clone());
        }
        if slot.failed_at.is_some_and(|t| t.elapsed() < RETRY_AFTER) {
            return Err(unavailable("redis unavailable"));
        }
        let made = tokio::time::timeout(CONNECT_TIMEOUT, self.client.get_connection_manager())
            .await
            .unwrap_or_else(|_| Err(unavailable("redis connect timed out")));
        match made {
            Ok(m) => {
                slot.manager = Some(m.clone());
                slot.failed_at = None;
                Ok(m)
            }
            Err(e) => {
                slot.failed_at = Some(Instant::now());
                Err(e)
            }
        }
    }
}

impl ConnectionLike for Redis {
    fn req_packed_command<'a>(&'a mut self, cmd: &'a Cmd) -> RedisFuture<'a, Value> {
        Box::pin(async move { self.manager().await?.req_packed_command(cmd).await })
    }

    fn req_packed_commands<'a>(
        &'a mut self,
        cmd: &'a Pipeline,
        offset: usize,
        count: usize,
    ) -> RedisFuture<'a, Vec<Value>> {
        Box::pin(async move { self.manager().await?.req_packed_commands(cmd, offset, count).await })
    }

    fn get_db(&self) -> i64 {
        self.client.get_connection_info().redis.db
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_unreachable_redis_fails_fast_instead_of_blocking_startup() {
        let mut r = Redis::new("redis://127.0.0.1:1/0").unwrap();
        r.warm_up().await;
        let started = Instant::now();
        let res: RedisResult<Option<String>> = redis::cmd("GET").arg("k").query_async(&mut r).await;
        assert!(res.is_err());
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
