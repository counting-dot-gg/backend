use super::{Column, Entity, Kind};

pub static ACHIEVEMENT: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "name",
            column: "name",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "icon",
            column: "icon",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "isPublic",
            column: "isPublic",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "description",
            column: "description",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "countersEarned",
            column: "countersEarned",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "maxProgress",
            column: "maxProgress",
            kind: Kind::Num,
            hidden: false,
        },
    ],
};

pub static BINGO_GAME: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "hostUserUuid",
            column: "hostUserUuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "status",
            column: "status",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "mode",
            column: "mode",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "rank",
            column: "rank",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "seasonId",
            column: "seasonId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "allowJoin",
            column: "allowJoin",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "joinMode",
            column: "joinMode",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "teamSwapMode",
            column: "teamSwapMode",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "joinCode",
            column: "joinCode",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "timeLimitSeconds",
            column: "timeLimitSeconds",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "minTimeSeconds",
            column: "minTimeSeconds",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "createdAt",
            column: "createdAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "startedAt",
            column: "startedAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "endsAt",
            column: "endsAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "graceEndsAt",
            column: "graceEndsAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "finishedAt",
            column: "finishedAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "winnerUserUuid",
            column: "winnerUserUuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "winnerTeamId",
            column: "winnerTeamId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "winnerAt",
            column: "winnerAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "winnerTeamSize",
            column: "winnerTeamSize",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "lineAt",
            column: "lineAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "boardSeed",
            column: "boardSeed",
            kind: Kind::Str,
            hidden: false,
        },
    ],
};

pub static BINGO_GAME_MEMBER: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "gameId",
            column: "gameId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "teamId",
            column: "teamId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "userUuid",
            column: "userUuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "role",
            column: "role",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "status",
            column: "status",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "joinedAt",
            column: "joinedAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "leftAt",
            column: "leftAt",
            kind: Kind::BigStr,
            hidden: false,
        },
    ],
};

pub static BINGO_GAME_SQUARE: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "gameId",
            column: "gameId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "index",
            column: "index",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "challengeType",
            column: "challengeType",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "rank",
            column: "rank",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "threadUuid",
            column: "threadUuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "rankChallengeId",
            column: "rankChallengeId",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "threadName",
            column: "threadName",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "threadTitle",
            column: "threadTitle",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "target",
            column: "target",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "params",
            column: "params",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "progressValue",
            column: "progressValue",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "isUnlocked",
            column: "isUnlocked",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "mostRecentUnlockerUserUuid",
            column: "mostRecentUnlockerUserUuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "mostRecentUnlockerTeamId",
            column: "mostRecentUnlockerTeamId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "mostRecentUnlockAt",
            column: "mostRecentUnlockAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "lastProgressAt",
            column: "lastProgressAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "createdAt",
            column: "createdAt",
            kind: Kind::BigStr,
            hidden: false,
        },
    ],
};

pub static BINGO_GAME_TEAM: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "gameId",
            column: "gameId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "name",
            column: "name",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "isDefault",
            column: "isDefault",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "locked",
            column: "locked",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "createdAt",
            column: "createdAt",
            kind: Kind::BigStr,
            hidden: false,
        },
    ],
};

pub static BLOG: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "uuid",
            column: "uuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "timestamp",
            column: "timestamp",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "edited_timestamp",
            column: "edited_timestamp",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "title",
            column: "title",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "body",
            column: "body",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "tags",
            column: "tags",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "reactions",
            column: "reactions",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "views",
            column: "views",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "hidden",
            column: "hidden",
            kind: Kind::Bool,
            hidden: false,
        },
    ],
};

pub static BLOG_LIKE: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "vote_type",
            column: "vote_type",
            kind: Kind::Num,
            hidden: false,
        },
    ],
};

pub static CHALLENGE_LOG: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "counterUuid",
            column: "counterUuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "challengeId",
            column: "challengeId",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "seasonId",
            column: "seasonId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "context",
            column: "context",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "bingoGameId",
            column: "bingoGameId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "progress",
            column: "progress",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "target",
            column: "target",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "lastCreditedSecond",
            column: "lastCreditedSecond",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "accuracyWindow",
            column: "accuracyWindow",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "completedAt",
            column: "completedAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "cancelledAt",
            column: "cancelledAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "ggReward",
            column: "ggReward",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "ggAwarded",
            column: "ggAwarded",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "assignmentOrdinal",
            column: "assignmentOrdinal",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "createdAt",
            column: "createdAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "updatedAt",
            column: "updatedAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "seenAt",
            column: "seenAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "entranceSeenAt",
            column: "entranceSeenAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "availableAt",
            column: "availableAt",
            kind: Kind::BigStr,
            hidden: false,
        },
    ],
};

pub static COUNTER: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "uuid",
            column: "uuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "discordId",
            column: "discordId",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "name",
            column: "name",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "username",
            column: "username",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "color",
            column: "color",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "roles",
            column: "roles",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "avatar",
            column: "avatar",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "cardStyle",
            column: "cardStyle",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "cardBorderStyle",
            column: "cardBorderStyle",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pronouns",
            column: "pronouns",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "title",
            column: "title",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "xp",
            column: "xp",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "xp2",
            column: "xp2",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "rainbow",
            column: "rainbow",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "lastRob",
            column: "lastRob",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "emoji",
            column: "emoji",
            kind: Kind::Str,
            hidden: false,
        },
    ],
};

pub static COUNTER_ACHIEVEMENT: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "counterUUID",
            column: "counterUUID",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "achievementId",
            column: "achievementId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "progress",
            column: "progress",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "isComplete",
            column: "isComplete",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "lastChecked",
            column: "lastChecked",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "timestamp",
            column: "timestamp",
            kind: Kind::Str,
            hidden: false,
        },
    ],
};

pub static COUNTER_PROFILE_PAGE: Entity = Entity {
    columns: &[
        Column {
            prop: "counterUUID",
            column: "counterUUID",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "bio",
            column: "bio",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "socialLinks",
            column: "socialLinks",
            kind: Kind::Json,
            hidden: false,
        },
    ],
};

pub static DAILY_CHALLENGE: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "day",
            column: "day",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "timestamp",
            column: "timestamp",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "thread",
            column: "thread",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "progress",
            column: "progress",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "maxProgress",
            column: "maxProgress",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "reward",
            column: "reward",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "rewardType",
            column: "rewardType",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "difficulty",
            column: "difficulty",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "status",
            column: "status",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "category",
            column: "category",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "challengeNumber",
            column: "challengeNumber",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "title",
            column: "title",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "description",
            column: "description",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "misc",
            column: "misc",
            kind: Kind::Json,
            hidden: false,
        },
    ],
};

pub static EMOTE: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "name",
            column: "name",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "filename",
            column: "filename",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "ownerUUID",
            column: "ownerUUID",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "approved",
            column: "approved",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "createdByUUID",
            column: "createdByUUID",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "slotUserItemId",
            column: "slotUserItemId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "createdAt",
            column: "createdAt",
            kind: Kind::BigStr,
            hidden: false,
        },
    ],
};

pub static FEATURE_FLAG: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "flagKey",
            column: "flagKey",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "enabled",
            column: "enabled",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "description",
            column: "description",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "updatedAt",
            column: "updatedAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "config",
            column: "config",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "updatedBy",
            column: "updatedBy",
            kind: Kind::Str,
            hidden: false,
        },
    ],
};

pub static FEEDBACK: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "authorUuid",
            column: "authorUuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "title",
            column: "title",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "body",
            column: "body",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "kind",
            column: "kind",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "status",
            column: "status",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "isPrivate",
            column: "isPrivate",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "score",
            column: "score",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "createdAt",
            column: "createdAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "updatedAt",
            column: "updatedAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "resolvedAt",
            column: "resolvedAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "staffNote",
            column: "staffNote",
            kind: Kind::Str,
            hidden: false,
        },
    ],
};

pub static FEEDBACK_COMMENT: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "feedbackId",
            column: "feedbackId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "authorUuid",
            column: "authorUuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "body",
            column: "body",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "deleted",
            column: "deleted",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "createdAt",
            column: "createdAt",
            kind: Kind::BigStr,
            hidden: false,
        },
    ],
};

pub static ITEM: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "uuid",
            column: "uuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "name",
            column: "name",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "internal_name",
            column: "internal_name",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "description",
            column: "description",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "category",
            column: "category",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "unlockMethod",
            column: "unlockMethod",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "unlockDescription",
            column: "unlockDescription",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "price",
            column: "price",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "quantity",
            column: "quantity",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "levelToUnlock",
            column: "levelToUnlock",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "achievementId",
            column: "achievementId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "season",
            column: "season",
            kind: Kind::Num,
            hidden: false,
        },
    ],
};

pub static MACRO_ENTRY: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "triggerKey",
            column: "triggerKey",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "macroType",
            column: "macroType",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "payloadJson",
            column: "payloadJson",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "createdAt",
            column: "createdAt",
            kind: Kind::Date,
            hidden: false,
        },
        Column {
            prop: "updatedAt",
            column: "updatedAt",
            kind: Kind::Date,
            hidden: false,
        },
    ],
};

pub static MACRO_PRESET: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "name",
            column: "name",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "handle",
            column: "handle",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "description",
            column: "description",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "visibility",
            column: "visibility",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "isDeleted",
            column: "isDeleted",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "createdAt",
            column: "createdAt",
            kind: Kind::Date,
            hidden: false,
        },
        Column {
            prop: "updatedAt",
            column: "updatedAt",
            kind: Kind::Date,
            hidden: false,
        },
    ],
};

pub static MACRO_PRESET_VERSION: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "versionNumber",
            column: "versionNumber",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "changeNote",
            column: "changeNote",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "createdAt",
            column: "createdAt",
            kind: Kind::Date,
            hidden: false,
        },
        Column {
            prop: "keyboardLayout",
            column: "keyboardLayout",
            kind: Kind::Json,
            hidden: false,
        },
    ],
};

pub static MACRO_UPDATE_QUEUE: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "requestPayloadJson",
            column: "requestPayloadJson",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "status",
            column: "status",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "errorMessage",
            column: "errorMessage",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "createdAt",
            column: "createdAt",
            kind: Kind::Date,
            hidden: false,
        },
        Column {
            prop: "processedAt",
            column: "processedAt",
            kind: Kind::Date,
            hidden: false,
        },
    ],
};

pub static MENTIONS: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "readStatus",
            column: "readStatus",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "postUUID",
            column: "postUUID",
            kind: Kind::Str,
            hidden: false,
        },
    ],
};

pub static MISC_SETTINGS: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "categories",
            column: "categories",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "lastUpdated",
            column: "lastUpdated",
            kind: Kind::BigStr,
            hidden: false,
        },
    ],
};

pub static NUMBER_SHUFFLE: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "client_id",
            column: "client_id",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "user_uuid",
            column: "user_uuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "seed",
            column: "seed",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "fakeSeed",
            column: "fakeSeed",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "size",
            column: "size",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "moves",
            column: "moves",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "gameStatus",
            column: "gameStatus",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "score",
            column: "score",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "isPreseeded",
            column: "isPreseeded",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "day",
            column: "day",
            kind: Kind::Str,
            hidden: false,
        },
    ],
};

pub static POST: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "uuid",
            column: "uuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "timestamp",
            column: "timestamp",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "timeSinceLastCount",
            column: "timeSinceLastCount",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "timeSinceLastPost",
            column: "timeSinceLastPost",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "rawText",
            column: "rawText",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "isCount",
            column: "isCount",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "isValidCount",
            column: "isValidCount",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "validCountNumber",
            column: "validCountNumber",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "countContent",
            column: "countContent",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "rawCount",
            column: "rawCount",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "stricken",
            column: "stricken",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "thread",
            column: "thread",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "threadId",
            column: "threadId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "hasComment",
            column: "hasComment",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "hasThreeCharComment",
            column: "hasThreeCharComment",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "comment",
            column: "comment",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "authorUUID",
            column: "authorUUID",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "authorId",
            column: "authorId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "isDeleted",
            column: "isDeleted",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "isCommentDeleted",
            column: "isCommentDeleted",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "reactions",
            column: "reactions",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "macroPresetId",
            column: "macroPresetId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "macroPresetVersionId",
            column: "macroPresetVersionId",
            kind: Kind::Num,
            hidden: false,
        },
    ],
};

pub static RANK_CHALLENGE: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "type",
            column: "type",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "rank",
            column: "rank",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "sequence",
            column: "sequence",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "ggReward",
            column: "ggReward",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "rewardType",
            column: "rewardType",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "target",
            column: "target",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "params",
            column: "params",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "threadUuid",
            column: "threadUuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "bingoCategory",
            column: "bingoCategory",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "seasonId",
            column: "seasonId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "clonedFromId",
            column: "clonedFromId",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "baseReassignmentWaitMs",
            column: "baseReassignmentWaitMs",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "createdAt",
            column: "createdAt",
            kind: Kind::BigStr,
            hidden: false,
        },
    ],
};

pub static RANK_SEASON: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "name",
            column: "name",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "startedAt",
            column: "startedAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "endedAt",
            column: "endedAt",
            kind: Kind::BigStr,
            hidden: false,
        },
    ],
};

pub static RANK_UP_EVENT: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "counterUuid",
            column: "counterUuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "threadUuid",
            column: "threadUuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "seasonId",
            column: "seasonId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "fromRank",
            column: "fromRank",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "fromDivision",
            column: "fromDivision",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "toRank",
            column: "toRank",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "toDivision",
            column: "toDivision",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "createdAt",
            column: "createdAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "seenAt",
            column: "seenAt",
            kind: Kind::BigStr,
            hidden: false,
        },
    ],
};

pub static ROB: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "counterUUID",
            column: "counterUUID",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "postUUID",
            column: "postUUID",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "moneyRobbed",
            column: "moneyRobbed",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "timestamp",
            column: "timestamp",
            kind: Kind::Str,
            hidden: false,
        },
    ],
};

pub static SERVER: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "discordID",
            column: "discordID",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "name",
            column: "name",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "avatar",
            column: "avatar",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "counts",
            column: "counts",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "botMods",
            column: "botMods",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "nsfw",
            column: "nsfw",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "isBanned",
            column: "isBanned",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "isActive",
            column: "isActive",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "canUsersJoinWithoutApproval",
            column: "canUsersJoinWithoutApproval",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "canUsersNotInTheServerJoin",
            column: "canUsersNotInTheServerJoin",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "kickUsersAfterTheyLeaveTheDiscordServer",
            column: "kickUsersAfterTheyLeaveTheDiscordServer",
            kind: Kind::Bool,
            hidden: false,
        },
    ],
};

pub static SITE_GOAL: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "goalKey",
            column: "goalKey",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "name",
            column: "name",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "description",
            column: "description",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "funding",
            column: "funding",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "targetCost",
            column: "targetCost",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "raised",
            column: "raised",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "unlockedAt",
            column: "unlockedAt",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "unlockedBy",
            column: "unlockedBy",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "active",
            column: "active",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "sortOrder",
            column: "sortOrder",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "createdAt",
            column: "createdAt",
            kind: Kind::BigStr,
            hidden: false,
        },
    ],
};

pub static THREAD: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "uuid",
            column: "uuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "name",
            column: "name",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "title",
            column: "title",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "shortDescription",
            column: "shortDescription",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "description",
            column: "description",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "rules",
            column: "rules",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "firstValidCount",
            column: "firstValidCount",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "validationType",
            column: "validationType",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "resetOnMistakes",
            column: "resetOnMistakes",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "allowDoublePosts",
            column: "allowDoublePosts",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "visibleTo",
            column: "visibleTo",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "updatableBy",
            column: "updatableBy",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "locked",
            column: "locked",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "autoValidated",
            column: "autoValidated",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "moderators",
            column: "moderators",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "verifiers",
            column: "verifiers",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "countBans",
            column: "countBans",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "postBans",
            column: "postBans",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "color1",
            column: "color1",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "color2",
            column: "color2",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "category",
            column: "category",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "countsPerSplit",
            column: "countsPerSplit",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "splitsPerGet",
            column: "splitsPerGet",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "splitOffset",
            column: "splitOffset",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "threadOfTheDay",
            column: "threadOfTheDay",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "type",
            column: "type",
            kind: Kind::Str,
            hidden: false,
        },
    ],
};

pub static THREAD_PREFERENCES: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "enabled",
            column: "enabled",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "pref_online",
            column: "pref_online",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "pref_discord_pings",
            column: "pref_discord_pings",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "pref_load_from_bottom",
            column: "pref_load_from_bottom",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "pref_strike_color",
            column: "pref_strike_color",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_submit_shortcut",
            column: "pref_submit_shortcut",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_clear",
            column: "pref_clear",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_nightMode",
            column: "pref_nightMode",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_standardize_format",
            column: "pref_standardize_format",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_time_since_last_count",
            column: "pref_time_since_last_count",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "pref_custom_stricken",
            column: "pref_custom_stricken",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_post_style",
            column: "pref_post_style",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_post_style_mobile",
            column: "pref_post_style_mobile",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_reply_time_interval",
            column: "pref_reply_time_interval",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "pref_night_mode_colors",
            column: "pref_night_mode_colors",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_post_position",
            column: "pref_post_position",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_hide_stricken",
            column: "pref_hide_stricken",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_highlight_last_count",
            column: "pref_highlight_last_count",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "pref_highlight_last_count_color",
            column: "pref_highlight_last_count_color",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_sound_on_stricken",
            column: "pref_sound_on_stricken",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_hide_thread_picker",
            column: "pref_hide_thread_picker",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "pref_stricken_count_opacity",
            column: "pref_stricken_count_opacity",
            kind: Kind::DecStr,
            hidden: false,
        },
        Column {
            prop: "pref_timestamp_display",
            column: "pref_timestamp_display",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_pace_mode",
            column: "pref_pace_mode",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_pace_counts",
            column: "pref_pace_counts",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "pref_pace_target_ms",
            column: "pref_pace_target_ms",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "pref_show_latency",
            column: "pref_show_latency",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "pref_post_font_size",
            column: "pref_post_font_size",
            kind: Kind::Num,
            hidden: false,
        },
    ],
};

pub static THREAD_RANK: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "counterUuid",
            column: "counterUuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "threadUuid",
            column: "threadUuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "seasonId",
            column: "seasonId",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "rank",
            column: "rank",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "division",
            column: "division",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "gg",
            column: "gg",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "ggTotal",
            column: "ggTotal",
            kind: Kind::Num,
            hidden: false,
        },
    ],
};

pub static USER: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "uuid",
            column: "uuid",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "discordId",
            column: "discord_id",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "accessToken",
            column: "access_token",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "refreshToken",
            column: "refresh_token",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "username",
            column: "username",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "discriminator",
            column: "discriminator",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_online",
            column: "pref_online",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "pref_discord_pings",
            column: "pref_discord_pings",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "pref_load_from_bottom",
            column: "pref_load_from_bottom",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "pref_strike_color",
            column: "pref_strike_color",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_submit_shortcut",
            column: "pref_submit_shortcut",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_clear",
            column: "pref_clear",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_nightMode",
            column: "pref_nightMode",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_standardize_format",
            column: "pref_standardize_format",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_time_since_last_count",
            column: "pref_time_since_last_count",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "pref_custom_stricken",
            column: "pref_custom_stricken",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_post_style",
            column: "pref_post_style",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_post_style_mobile",
            column: "pref_post_style_mobile",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_reply_time_interval",
            column: "pref_reply_time_interval",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "pref_night_mode_colors",
            column: "pref_night_mode_colors",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_post_position",
            column: "pref_post_position",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_hide_stricken",
            column: "pref_hide_stricken",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_highlight_last_count",
            column: "pref_highlight_last_count",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "pref_highlight_last_count_color",
            column: "pref_highlight_last_count_color",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_sound_on_stricken",
            column: "pref_sound_on_stricken",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_hide_thread_picker",
            column: "pref_hide_thread_picker",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "pref_stricken_count_opacity",
            column: "pref_stricken_count_opacity",
            kind: Kind::DecStr,
            hidden: false,
        },
        Column {
            prop: "pref_timestamp_display",
            column: "pref_timestamp_display",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_show_latency",
            column: "pref_show_latency",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "pref_pace_mode",
            column: "pref_pace_mode",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_pace_counts",
            column: "pref_pace_counts",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "pref_pace_target_ms",
            column: "pref_pace_target_ms",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "pref_chat_default",
            column: "pref_chat_default",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "pref_milestones",
            column: "pref_milestones",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "pref_post_font_size",
            column: "pref_post_font_size",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "money",
            column: "money",
            kind: Kind::BigStr,
            hidden: false,
        },
        Column {
            prop: "timeOnline",
            column: "timeOnline",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "inventory",
            column: "inventory",
            kind: Kind::Json,
            hidden: false,
        },
        Column {
            prop: "reddit",
            column: "reddit",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "redditAccess",
            column: "redditAccess",
            kind: Kind::Str,
            hidden: false,
        },
        Column {
            prop: "redditRefresh",
            column: "redditRefresh",
            kind: Kind::Str,
            hidden: false,
        },
    ],
};

pub static USER_ITEM: Entity = Entity {
    columns: &[
        Column {
            prop: "id",
            column: "id",
            kind: Kind::Num,
            hidden: false,
        },
        Column {
            prop: "unlocked",
            column: "unlocked",
            kind: Kind::Bool,
            hidden: false,
        },
        Column {
            prop: "consumedAt",
            column: "consumedAt",
            kind: Kind::BigStr,
            hidden: false,
        },
    ],
};
