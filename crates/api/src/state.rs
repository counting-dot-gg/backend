use crate::config::Config;
use crate::throttle::Throttler;
use cgg_web::nest;
use sqlx::MySqlPool;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState(pub Arc<Inner>);

pub struct Inner {
    pub cfg: Config,
    pub pool: MySqlPool,
    pub redis: crate::redis_conn::Redis,
    pub socket: nest::Client,
    pub socket_calls: nest::Caller,
    pub bot: nest::Client,
    pub throttle: Throttler,
    pub http: reqwest::Client,
    pub caches: crate::cache::Caches,
    pub flags: crate::routes::features::FlagCache,
    pub goals: crate::routes::sitegoals::GoalCache,
    pub jobs: tokio::sync::Mutex<()>,
    pub client_errors: crate::routes::api::ClientErrors,
    pub clock: cgg_web::ids::Clock,
    pub stats: crate::statsread::store::StatsCaches,
    pub coverage: crate::statsread::coverage::CoverageCaches,
    pub owners: crate::statsread::owners::OwnerCaches,
}

impl std::ops::Deref for AppState {
    type Target = Inner;

    fn deref(&self) -> &Inner {
        &self.0
    }
}
