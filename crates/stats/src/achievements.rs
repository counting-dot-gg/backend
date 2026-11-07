use anyhow::Result;
use futures_util::future::BoxFuture;
use redis::aio::ConnectionManager;
use sqlx::MySqlPool;

use crate::models::Thread;

pub trait AchievementEngine: Send + Sync {
    fn check<'a>(
        &'a self,
        pool: &'a MySqlPool,
        redis: &'a mut ConnectionManager,
        threads: &'a [Thread],
    ) -> BoxFuture<'a, Result<()>>;
}

#[derive(Debug, Clone, Copy)]
pub enum LiveEvent<'a> {
    ValidCount {
        counts_today: Option<i64>,
        thread_validation: &'a str,
        first_count_in_thread_today: bool,
    },
    Reacted {
        reaction: &'a str,
    },
    Robbed {
        amount: i64,
    },
    LrwoedPlayed {
        score: f64,
    },
    ShuffleWon {
        daily: bool,
        preseeded: bool,
        size: i64,
    },
    BingoWon {
        winning_team_size: usize,
        opposing_team_sizes: &'a [usize],
        teams_that_played: usize,
    },
    BingoGameStarted {
        team: &'a str,
    },
    PostPositionChosen {
        position: &'a str,
    },
    BlogPublished,
    JoinedAsCounter,
    NotesEdited,
    CommentPosted {
        comment: &'a str,
        thread_validation: &'a str,
    },
    BotCommand {
        name: &'a str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AchievementDef {
    pub name: &'static str,
    pub description: &'static str,
    pub public: bool,
    pub max_progress: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AchievementRef {
    Id(i32),
    Name(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Award {
    Earn(AchievementRef),
    Progress(AchievementRef, i64),
}
