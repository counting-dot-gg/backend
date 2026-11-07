use dashmap::DashMap;
use sqlx::MySqlPool;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tokio::sync::watch;

static COUNTS: LazyLock<DashMap<u32, Arc<watch::Sender<i64>>>> = LazyLock::new(DashMap::new);

fn channel(thread_id: u32) -> Arc<watch::Sender<i64>> {
    COUNTS
        .entry(thread_id)
        .or_insert_with(|| Arc::new(watch::channel(0).0))
        .clone()
}

pub fn published(thread_id: u32, vcn: i64) {
    channel(thread_id).send_if_modified(|v| {
        let raised = vcn > *v;
        if raised {
            *v = vcn;
        }
        raised
    });
}

pub async fn await_count(pool: &MySqlPool, thread_id: u32, vcn: i64, max: Duration) -> bool {
    let mut rx = channel(thread_id).subscribe();
    if *rx.borrow() >= vcn {
        return true;
    }
    let in_db: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM post WHERE threadId = ? AND validCountNumber = ? AND isValidCount = 1 LIMIT 1",
    )
    .bind(thread_id)
    .bind(vcn)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    if in_db.is_some() {
        return true;
    }
    matches!(tokio::time::timeout(max, rx.wait_for(|v| *v >= vcn)).await, Ok(Ok(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_waiter_wakes_when_its_count_is_published() {
        let tx = channel(9_001);
        let mut rx = tx.subscribe();
        let wait = tokio::spawn(async move { rx.wait_for(|v| *v >= 42).await.is_ok() });
        published(9_001, 41);
        published(9_001, 43);
        assert!(wait.await.unwrap());
    }

    #[test]
    fn the_published_value_never_goes_down() {
        published(9_002, 10);
        published(9_002, 7);
        assert_eq!(*channel(9_002).borrow(), 10);
    }
}
