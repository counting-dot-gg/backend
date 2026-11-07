use anyhow::Result;
use futures_util::future::BoxFuture;
use redis::aio::ConnectionManager;
use sqlx::MySqlPool;
use stats::achievements::{AchievementDef, AchievementEngine, AchievementRef, Award, LiveEvent};
use stats::models::Thread;

const FIRST_COUNT: &str = "First Count";
const FIRST_REACTION: &str = "Reactor";
const JOINED: &str = "Counter";
const FIRST_BLOG: &str = "Author";
const FIRST_BOT_COMMAND: &str = "Bot Whisperer";
const FIRST_SHUFFLE_WIN: &str = "Shuffled";

const XP_MILESTONES: [(&str, &str, i64); 3] = [
    ("Rising", "Reach 10,000 XP.", 10_000),
    ("Seasoned", "Reach 100,000 XP.", 100_000),
    ("Veteran", "Reach 1,000,000 XP.", 1_000_000),
];

fn def(name: &'static str, description: &'static str) -> AchievementDef {
    AchievementDef {
        name,
        description,
        public: true,
        max_progress: 1,
    }
}

pub fn definitions() -> Vec<AchievementDef> {
    let mut defs = vec![
        def(FIRST_COUNT, "Make a valid count."),
        def(FIRST_REACTION, "React to a post."),
        def(JOINED, "Become a counter."),
        def(FIRST_BLOG, "Publish a blog post."),
        def(FIRST_BOT_COMMAND, "Ask the bot something."),
        def(FIRST_SHUFFLE_WIN, "Win a number shuffle."),
    ];
    defs.extend(
        XP_MILESTONES
            .iter()
            .map(|(name, description, _)| def(name, description)),
    );
    defs
}

pub fn live_awards(event: &LiveEvent) -> Vec<Award> {
    let name = match event {
        LiveEvent::ValidCount { .. } => FIRST_COUNT,
        LiveEvent::Reacted { .. } => FIRST_REACTION,
        LiveEvent::JoinedAsCounter => JOINED,
        LiveEvent::BlogPublished => FIRST_BLOG,
        LiveEvent::BotCommand { .. } => FIRST_BOT_COMMAND,
        LiveEvent::ShuffleWon { .. } => FIRST_SHUFFLE_WIN,
        _ => return Vec::new(),
    };
    vec![Award::Earn(AchievementRef::Name(name))]
}

pub struct Engine;

pub fn engine() -> std::sync::Arc<dyn AchievementEngine> {
    std::sync::Arc::new(Engine)
}

impl AchievementEngine for Engine {
    fn check<'a>(
        &'a self,
        pool: &'a MySqlPool,
        _redis: &'a mut ConnectionManager,
        _threads: &'a [Thread],
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(award_xp_milestones(pool))
    }
}

async fn award_xp_milestones(pool: &MySqlPool) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis()
        .to_string();
    for (name, _, min_xp) in XP_MILESTONES {
        sqlx::query(
            "INSERT INTO counter_achievements (counterUUID, achievementId, isComplete, lastChecked, progress, timestamp) \
             SELECT c.uuid, a.id, 1, 1, 1, ? FROM counters c JOIN achievements a ON a.name = ? \
             WHERE c.xp >= ? AND NOT EXISTS ( \
               SELECT 1 FROM counter_achievements ca WHERE ca.counterUUID = c.uuid AND ca.achievementId = a.id)",
        )
        .bind(&now)
        .bind(name)
        .bind(min_xp)
        .execute(pool)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_live_award_names_a_defined_achievement() {
        let names: Vec<&str> = definitions().iter().map(|d| d.name).collect();
        let events = [
            LiveEvent::Reacted { reaction: "x" },
            LiveEvent::JoinedAsCounter,
            LiveEvent::BlogPublished,
        ];
        for e in &events {
            for award in live_awards(e) {
                let Award::Earn(AchievementRef::Name(n)) = award else {
                    panic!("unexpected award {award:?}")
                };
                assert!(names.contains(&n), "{n} is not defined");
            }
        }
    }

    #[test]
    fn achievement_names_are_unique() {
        let mut names: Vec<&str> = definitions().iter().map(|d| d.name).collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before);
    }
}
