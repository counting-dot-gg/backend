CREATE TABLE `achievements` (
  `id` int NOT NULL AUTO_INCREMENT,
  `name` text NOT NULL,
  `icon` text,
  `isPublic` tinyint NOT NULL,
  `description` text NOT NULL,
  `countersEarned` int NOT NULL DEFAULT '0',
  `maxProgress` int NOT NULL,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `alliance` (
  `id` int NOT NULL AUTO_INCREMENT,
  `name` varchar(255) NOT NULL,
  `level` int NOT NULL,
  `progress` int NOT NULL,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `bingoclaim` (
  `id` int NOT NULL AUTO_INCREMENT,
  `gameId` int NOT NULL,
  `squareId` int NOT NULL,
  `squareIndex` tinyint NOT NULL,
  `rank` varchar(32) NOT NULL,
  `seasonId` int DEFAULT NULL,
  `userUuid` varchar(255) DEFAULT NULL,
  `teamId` int NOT NULL,
  `prevTeamId` int DEFAULT NULL,
  `kind` varchar(8) NOT NULL,
  `at` bigint NOT NULL,
  PRIMARY KEY (`id`),
  KEY `IDX_e65f8c150468b396eb33bc607f` (`gameId`),
  KEY `IDX_23fbffb68817999a37b8edb80d` (`squareId`),
  KEY `IDX_d8a523dae35809319f23175191` (`userUuid`,`kind`),
  KEY `IDX_4937f4c55cd69a0974779d10e3` (`seasonId`,`kind`,`rank`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `bingogame` (
  `id` int NOT NULL AUTO_INCREMENT,
  `hostUserUuid` varchar(255) NOT NULL,
  `status` varchar(255) NOT NULL DEFAULT 'lobby',
  `mode` varchar(255) NOT NULL DEFAULT 'bingo',
  `rank` varchar(255) NOT NULL,
  `seasonId` int NOT NULL,
  `allowJoin` tinyint NOT NULL DEFAULT '1',
  `joinMode` varchar(255) NOT NULL DEFAULT 'closed',
  `teamSwapMode` varchar(255) NOT NULL DEFAULT 'locked',
  `joinCode` varchar(255) DEFAULT NULL,
  `timeLimitSeconds` int NOT NULL DEFAULT '86400',
  `createdAt` bigint NOT NULL DEFAULT '0',
  `startedAt` bigint NOT NULL DEFAULT '0',
  `endsAt` bigint NOT NULL DEFAULT '0',
  `graceEndsAt` bigint NOT NULL DEFAULT '0',
  `finishedAt` bigint NOT NULL DEFAULT '0',
  `winnerUserUuid` varchar(255) DEFAULT NULL,
  `winnerTeamId` int DEFAULT NULL,
  `winnerAt` bigint NOT NULL DEFAULT '0',
  `winnerTeamSize` int NOT NULL DEFAULT '0',
  `boardSeed` varchar(255) DEFAULT NULL,
  `minTimeSeconds` int DEFAULT NULL,
  `lineAt` bigint NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `IDX_2f58b1dbb9c7b98fa5d93c630e` (`hostUserUuid`),
  KEY `IDX_c824ed21503ad4201c8b44ddc2` (`status`),
  KEY `IDX_99f3b999587b2f41c9a7aba955` (`rank`),
  KEY `IDX_dccdc6e490540d310452225f5b` (`seasonId`),
  KEY `IDX_88d2c8887bf6e6a91cc24899b9` (`createdAt`),
  KEY `IDX_9fcdc4539c7fe93d6b82d22edf` (`startedAt`),
  KEY `IDX_e1ca077324b88e4f0fde03947b` (`endsAt`),
  KEY `IDX_0c34e90dfb91e95dd5571f66bc` (`graceEndsAt`),
  KEY `IDX_7950c6920d1d8454b66c9da898` (`finishedAt`),
  KEY `IDX_8e7222471ad01c522c1c2cf3ba` (`winnerUserUuid`),
  KEY `IDX_51b5c3da2a6340385d2a442e46` (`winnerTeamId`),
  KEY `IDX_d8b8ce376723a329619a154a9c` (`winnerAt`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `bingogamemember` (
  `id` int NOT NULL AUTO_INCREMENT,
  `gameId` int NOT NULL,
  `teamId` int NOT NULL,
  `userUuid` varchar(255) NOT NULL,
  `role` varchar(255) NOT NULL DEFAULT 'player',
  `status` varchar(255) NOT NULL DEFAULT 'active',
  `joinedAt` bigint NOT NULL DEFAULT '0',
  `leftAt` bigint NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  UNIQUE KEY `IDX_29184a47283276f0f190d2b4b8` (`gameId`,`userUuid`),
  KEY `IDX_658277a94f5d35f7379cb0a03b` (`gameId`),
  KEY `IDX_d1e3357affacb5c473076faf07` (`teamId`),
  KEY `IDX_13e9776ad9de2658dcd34c86a6` (`userUuid`),
  KEY `IDX_1bb97f6d2fb5e6654a307a13f1` (`status`),
  KEY `IDX_da6cc76e01b3f0568e2789a7a0` (`joinedAt`),
  KEY `IDX_9d706b8571bc1f33f003a427cd` (`leftAt`),
  CONSTRAINT `FK_13e9776ad9de2658dcd34c86a62` FOREIGN KEY (`userUuid`) REFERENCES `counters` (`uuid`) ON DELETE CASCADE,
  CONSTRAINT `FK_658277a94f5d35f7379cb0a03b8` FOREIGN KEY (`gameId`) REFERENCES `bingogame` (`id`) ON DELETE CASCADE,
  CONSTRAINT `FK_d1e3357affacb5c473076faf07e` FOREIGN KEY (`teamId`) REFERENCES `bingogameteam` (`id`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `bingogamesquare` (
  `id` int NOT NULL AUTO_INCREMENT,
  `gameId` int NOT NULL,
  `index` int NOT NULL,
  `challengeType` varchar(255) NOT NULL,
  `rank` varchar(255) NOT NULL,
  `threadUuid` varchar(255) DEFAULT NULL,
  `rankChallengeId` varchar(255) DEFAULT NULL,
  `target` int NOT NULL DEFAULT '1',
  `params` json DEFAULT NULL,
  `progressValue` int NOT NULL DEFAULT '0',
  `isUnlocked` tinyint NOT NULL DEFAULT '0',
  `mostRecentUnlockerUserUuid` varchar(255) DEFAULT NULL,
  `mostRecentUnlockerTeamId` int DEFAULT NULL,
  `mostRecentUnlockAt` bigint NOT NULL DEFAULT '0',
  `lastProgressAt` bigint NOT NULL DEFAULT '0',
  `createdAt` bigint NOT NULL DEFAULT '0',
  `threadName` varchar(255) DEFAULT NULL,
  `threadTitle` varchar(255) DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `IDX_987872df99789456924571a580` (`gameId`,`index`),
  KEY `IDX_6804cc24219336d783795332f7` (`gameId`),
  KEY `IDX_9a2128544309eaae891c113605` (`threadUuid`),
  CONSTRAINT `FK_6804cc24219336d783795332f7a` FOREIGN KEY (`gameId`) REFERENCES `bingogame` (`id`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `bingogameteam` (
  `id` int NOT NULL AUTO_INCREMENT,
  `gameId` int NOT NULL,
  `name` varchar(255) NOT NULL DEFAULT 'Team 1',
  `isDefault` tinyint NOT NULL DEFAULT '1',
  `locked` tinyint NOT NULL DEFAULT '0',
  `createdAt` bigint NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `IDX_e3d90923489b1d5b4f001ddde3` (`gameId`),
  KEY `IDX_392e0f3e86d268859a7755213a` (`createdAt`),
  CONSTRAINT `FK_e3d90923489b1d5b4f001ddde35` FOREIGN KEY (`gameId`) REFERENCES `bingogame` (`id`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `bingosquareuserprogress` (
  `id` int NOT NULL AUTO_INCREMENT,
  `squareId` int NOT NULL,
  `userUuid` varchar(255) NOT NULL,
  `progressValue` int NOT NULL DEFAULT '0',
  `lastProgressAt` bigint NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  UNIQUE KEY `IDX_b8653385406cac3455bc4f267e` (`squareId`,`userUuid`),
  KEY `IDX_9953e8daa0bf0d8b90fdebb741` (`squareId`),
  KEY `IDX_62ac71adfa5d165070b92a2704` (`userUuid`),
  KEY `IDX_2731f4cd69c542915be2d19638` (`lastProgressAt`),
  CONSTRAINT `FK_62ac71adfa5d165070b92a27045` FOREIGN KEY (`userUuid`) REFERENCES `counters` (`uuid`) ON DELETE CASCADE,
  CONSTRAINT `FK_9953e8daa0bf0d8b90fdebb741d` FOREIGN KEY (`squareId`) REFERENCES `bingogamesquare` (`id`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `blog` (
  `id` int NOT NULL AUTO_INCREMENT,
  `uuid` varchar(255) NOT NULL,
  `timestamp` varchar(255) NOT NULL,
  `edited_timestamp` varchar(255) DEFAULT NULL,
  `title` varchar(255) NOT NULL,
  `body` text NOT NULL,
  `tags` json NOT NULL,
  `views` int NOT NULL DEFAULT '1',
  `authorUuid` varchar(255) DEFAULT NULL,
  `reactions` json DEFAULT NULL,
  `hidden` tinyint NOT NULL DEFAULT '0',
  PRIMARY KEY (`uuid`),
  KEY `IDX_85c6532ad065a448e9de763857` (`id`),
  KEY `IDX_88b60a3fb66620118c8d971278` (`timestamp`),
  KEY `FK_1114576b5dab6394965958163df` (`authorUuid`),
  CONSTRAINT `FK_1114576b5dab6394965958163df` FOREIGN KEY (`authorUuid`) REFERENCES `counters` (`uuid`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `bloglike` (
  `id` int NOT NULL AUTO_INCREMENT,
  `vote_type` int NOT NULL,
  `blogUuid` varchar(255) DEFAULT NULL,
  `voterUuid` varchar(255) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `FK_7deaf951239e8a62991352917fd` (`blogUuid`),
  KEY `FK_fba79775cdd7a51dd14cc972bfd` (`voterUuid`),
  CONSTRAINT `FK_7deaf951239e8a62991352917fd` FOREIGN KEY (`blogUuid`) REFERENCES `blog` (`uuid`),
  CONSTRAINT `FK_fba79775cdd7a51dd14cc972bfd` FOREIGN KEY (`voterUuid`) REFERENCES `counters` (`uuid`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `cgg_wal_checkpoint` (
  `id` tinyint NOT NULL,
  `seq` bigint unsigned NOT NULL,
  `updatedAt` timestamp(3) NOT NULL DEFAULT CURRENT_TIMESTAMP(3) ON UPDATE CURRENT_TIMESTAMP(3),
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `challenge` (
  `id` int NOT NULL AUTO_INCREMENT,
  `day` varchar(255) NOT NULL,
  `timestamp` varchar(255) NOT NULL,
  `thread` varchar(255) DEFAULT NULL,
  `progress` int NOT NULL DEFAULT '0',
  `maxProgress` int NOT NULL DEFAULT '100',
  `reward` int NOT NULL DEFAULT '100',
  `rewardType` varchar(255) NOT NULL DEFAULT 'xp',
  `difficulty` varchar(255) NOT NULL DEFAULT 'Easy',
  `status` varchar(255) NOT NULL DEFAULT 'inactive',
  `category` varchar(255) NOT NULL,
  `challengeNumber` int NOT NULL,
  `title` varchar(255) NOT NULL,
  `description` varchar(255) NOT NULL,
  `misc` json NOT NULL,
  `userUuid` varchar(255) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `IDX_1ffd38154da90e9bd1c6954c52` (`day`),
  KEY `IDX_5640e4cbd7e729061a27e292c7` (`timestamp`),
  KEY `FK_8197c535fead4b15c0d6f224491` (`userUuid`),
  CONSTRAINT `FK_8197c535fead4b15c0d6f224491` FOREIGN KEY (`userUuid`) REFERENCES `users` (`uuid`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `challengelog` (
  `id` int NOT NULL AUTO_INCREMENT,
  `counterUuid` varchar(255) NOT NULL,
  `challengeId` varchar(255) NOT NULL,
  `seasonId` int DEFAULT NULL,
  `context` varchar(255) NOT NULL DEFAULT 'rank',
  `bingoGameId` int DEFAULT NULL,
  `progress` int NOT NULL DEFAULT '0',
  `target` int NOT NULL,
  `accuracyWindow` text,
  `completedAt` bigint DEFAULT NULL,
  `cancelledAt` bigint DEFAULT NULL,
  `ggReward` int NOT NULL DEFAULT '0',
  `ggAwarded` int NOT NULL DEFAULT '0',
  `createdAt` bigint NOT NULL,
  `updatedAt` bigint NOT NULL,
  `seenAt` bigint DEFAULT NULL,
  `entranceSeenAt` bigint DEFAULT NULL,
  `availableAt` bigint DEFAULT NULL,
  `assignmentOrdinal` int NOT NULL DEFAULT '0',
  `lastCreditedSecond` bigint DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `IDX_d36f7aeaa25b13e2ce936d27c5` (`counterUuid`),
  KEY `IDX_d7a29cea554ee0df356a47c892` (`challengeId`),
  KEY `IDX_3964652b9cf827b0b8cfb55c37` (`seasonId`),
  KEY `IDX_56a2e984ffef32598418086c13` (`context`),
  KEY `IDX_0fb5451455f944e37d6026abc7` (`completedAt`),
  KEY `IDX_2df208a65d7060664c353ae499` (`cancelledAt`),
  KEY `IDX_09ace25376ca2981442ca4a554` (`createdAt`),
  KEY `IDX_464f196b2146ab0e0b72fa5a00` (`updatedAt`),
  KEY `IDX_9ca5e7507b226856553f691e73` (`seenAt`),
  KEY `n_cl_open` (`counterUuid`,`seasonId`,`completedAt`,`cancelledAt`,`challengeId`),
  KEY `n_cl_counter_challenge_season` (`counterUuid`,`challengeId`,`seasonId`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `counter_achievements` (
  `id` int NOT NULL AUTO_INCREMENT,
  `counterUUID` varchar(255) NOT NULL,
  `achievementId` int NOT NULL,
  `timestamp` text,
  `progress` int NOT NULL,
  `isComplete` tinyint NOT NULL,
  `lastChecked` tinyint NOT NULL,
  PRIMARY KEY (`id`),
  KEY `n_ca_achievement` (`achievementId`),
  KEY `n_ca_counter_achievement` (`counterUUID`,`achievementId`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `counter_profile_pages` (
  `counterUUID` varchar(255) NOT NULL,
  `bio` text,
  `socialLinks` json DEFAULT NULL,
  PRIMARY KEY (`counterUUID`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `counters` (
  `id` int NOT NULL AUTO_INCREMENT,
  `uuid` varchar(255) NOT NULL,
  `discordId` text NOT NULL,
  `name` text NOT NULL,
  `color` varchar(255) NOT NULL,
  `roles` json NOT NULL,
  `avatar` varchar(255) NOT NULL,
  `cardStyle` varchar(255) NOT NULL,
  `cardBorderStyle` varchar(255) NOT NULL,
  `pronouns` json NOT NULL,
  `title` varchar(255) NOT NULL,
  `lastRob` varchar(255) DEFAULT NULL,
  `xp` bigint NOT NULL DEFAULT '0',
  `username` varchar(255) NOT NULL,
  `emoji` varchar(255) DEFAULT NULL,
  `allianceId` int DEFAULT NULL,
  `xp2` bigint NOT NULL DEFAULT '0',
  `rainbow` int NOT NULL DEFAULT '1',
  PRIMARY KEY (`uuid`),
  KEY `IDX_910bfcbadea9cde6397e0daf99` (`id`),
  KEY `FK_d0908111584c5807923f3693b40` (`allianceId`),
  CONSTRAINT `FK_d0908111584c5807923f3693b40` FOREIGN KEY (`allianceId`) REFERENCES `alliance` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `counters_servers_servers` (
  `countersUuid` varchar(255) NOT NULL,
  `serversId` int NOT NULL,
  `serversDiscordID` varchar(255) NOT NULL,
  PRIMARY KEY (`countersUuid`,`serversId`,`serversDiscordID`),
  KEY `IDX_5dac26d287fe478f9850f1485f` (`countersUuid`),
  KEY `IDX_221fff3e850c9aa90c1d5bb83b` (`serversId`,`serversDiscordID`),
  CONSTRAINT `FK_221fff3e850c9aa90c1d5bb83b0` FOREIGN KEY (`serversId`, `serversDiscordID`) REFERENCES `servers` (`id`, `discordID`),
  CONSTRAINT `FK_5dac26d287fe478f9850f1485f5` FOREIGN KEY (`countersUuid`) REFERENCES `counters` (`uuid`) ON DELETE CASCADE ON UPDATE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `counterserver` (
  `id` int NOT NULL AUTO_INCREMENT,
  `counterID` varchar(255) NOT NULL,
  `serverID` varchar(255) NOT NULL,
  PRIMARY KEY (`id`,`counterID`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `emote` (
  `id` int NOT NULL AUTO_INCREMENT,
  `name` varchar(255) NOT NULL,
  `filename` varchar(255) NOT NULL,
  `ownerUUID` varchar(255) DEFAULT NULL,
  `approved` tinyint NOT NULL DEFAULT '0',
  `createdByUUID` varchar(255) DEFAULT NULL,
  `slotUserItemId` int DEFAULT NULL,
  `createdAt` bigint NOT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `IDX_707b2db5fddf27d55cb93e9d56` (`name`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `feature_flag` (
  `id` int NOT NULL AUTO_INCREMENT,
  `flagKey` varchar(64) NOT NULL,
  `enabled` tinyint NOT NULL DEFAULT '0',
  `description` varchar(255) DEFAULT NULL,
  `updatedAt` bigint DEFAULT NULL,
  `updatedBy` varchar(255) DEFAULT NULL,
  `config` text,
  PRIMARY KEY (`id`),
  UNIQUE KEY `IDX_feature_flag_key` (`flagKey`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `feedback` (
  `id` int NOT NULL AUTO_INCREMENT,
  `authorUuid` varchar(255) NOT NULL,
  `title` varchar(200) NOT NULL,
  `body` text NOT NULL,
  `kind` varchar(255) NOT NULL DEFAULT 'bug',
  `status` varchar(255) NOT NULL DEFAULT 'open',
  `isPrivate` tinyint NOT NULL DEFAULT '0',
  `score` int NOT NULL DEFAULT '0',
  `createdAt` bigint NOT NULL DEFAULT '0',
  `updatedAt` bigint NOT NULL DEFAULT '0',
  `resolvedAt` bigint NOT NULL DEFAULT '0',
  `staffNote` text,
  PRIMARY KEY (`id`),
  KEY `IDX_9e1e7be391793e8db78d6462e6` (`authorUuid`),
  KEY `IDX_e6d53208abf3a4d0dcaba3c8cb` (`kind`),
  KEY `IDX_b58070b64dd68964f5a71ff021` (`status`),
  KEY `IDX_30b9086eec2c53caa156855004` (`isPrivate`),
  KEY `IDX_b072e0610d55793714b48a1309` (`score`),
  KEY `IDX_59a7946f56cddfd6f80fafda48` (`createdAt`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `feedback_comment` (
  `id` int NOT NULL AUTO_INCREMENT,
  `feedbackId` int NOT NULL,
  `authorUuid` varchar(255) NOT NULL,
  `body` text NOT NULL,
  `deleted` tinyint NOT NULL DEFAULT '0',
  `createdAt` bigint NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  KEY `IDX_1225904e864a89186f42f7fd5e` (`feedbackId`),
  KEY `IDX_43f5b9bc91411e5d4d840b3e1d` (`authorUuid`),
  KEY `IDX_c2c6a460b2ccd78e37d4435ac7` (`createdAt`),
  CONSTRAINT `FK_1225904e864a89186f42f7fd5e5` FOREIGN KEY (`feedbackId`) REFERENCES `feedback` (`id`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `feedback_vote` (
  `id` int NOT NULL AUTO_INCREMENT,
  `feedbackId` int NOT NULL,
  `voterUuid` varchar(255) NOT NULL,
  `createdAt` bigint NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`),
  UNIQUE KEY `IDX_22f08851d789fc03dd41c1fddc` (`feedbackId`,`voterUuid`),
  KEY `IDX_379190cb1856842cd6c2d914dd` (`feedbackId`),
  KEY `IDX_4d214fce1251458c1a4723daa7` (`voterUuid`),
  CONSTRAINT `FK_379190cb1856842cd6c2d914dd5` FOREIGN KEY (`feedbackId`) REFERENCES `feedback` (`id`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `item` (
  `id` int NOT NULL AUTO_INCREMENT,
  `name` varchar(255) NOT NULL,
  `description` text,
  `category` varchar(255) NOT NULL,
  `unlockMethod` varchar(255) NOT NULL,
  `unlockDescription` text,
  `price` bigint NOT NULL,
  `quantity` int DEFAULT NULL,
  `levelToUnlock` int DEFAULT NULL,
  `achievementId` int DEFAULT NULL,
  `internal_name` varchar(255) NOT NULL,
  `uuid` varchar(36) NOT NULL,
  `season` int DEFAULT NULL,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `macro_entry` (
  `id` int NOT NULL AUTO_INCREMENT,
  `triggerKey` varchar(64) NOT NULL,
  `macroType` enum('CHAR_INSERT','ACTION','SUBMIT','SUBMIT_ACTION','TOGGLE','COMBO') NOT NULL,
  `payloadJson` json NOT NULL,
  `createdAt` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updatedAt` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  `versionId` int DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `macro_entry_version_triggerKey_uq` (`versionId`,`triggerKey`),
  CONSTRAINT `FK_9dcd25ec03bd073524edf2c9be3` FOREIGN KEY (`versionId`) REFERENCES `macro_preset_version` (`id`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `macro_preset` (
  `id` int NOT NULL AUTO_INCREMENT,
  `name` varchar(64) NOT NULL,
  `handle` varchar(64) NOT NULL,
  `description` varchar(280) NOT NULL DEFAULT '',
  `visibility` enum('PUBLIC') NOT NULL DEFAULT 'PUBLIC',
  `isDeleted` tinyint NOT NULL DEFAULT '0',
  `createdAt` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `updatedAt` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  `ownerCounterUuid` varchar(255) DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `macro_preset_handle_uq` (`handle`),
  KEY `macro_preset_createdAt_idx` (`createdAt`),
  KEY `macro_preset_updatedAt_idx` (`updatedAt`),
  KEY `FK_694909fd706b3ffc297277ec45d` (`ownerCounterUuid`),
  CONSTRAINT `FK_694909fd706b3ffc297277ec45d` FOREIGN KEY (`ownerCounterUuid`) REFERENCES `counters` (`uuid`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `macro_preset_version` (
  `id` int NOT NULL AUTO_INCREMENT,
  `versionNumber` int NOT NULL,
  `changeNote` varchar(280) DEFAULT NULL,
  `createdAt` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `macroPresetId` int DEFAULT NULL,
  `createdByCounterUuid` varchar(255) DEFAULT NULL,
  `keyboardLayout` json DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `macro_preset_version_macroPreset_versionNumber_uq` (`macroPresetId`,`versionNumber`),
  KEY `FK_e6c9577098d93d5d7d71cfc678e` (`createdByCounterUuid`),
  CONSTRAINT `FK_66eb663b05609a43479a87ff070` FOREIGN KEY (`macroPresetId`) REFERENCES `macro_preset` (`id`) ON DELETE CASCADE,
  CONSTRAINT `FK_e6c9577098d93d5d7d71cfc678e` FOREIGN KEY (`createdByCounterUuid`) REFERENCES `counters` (`uuid`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `macro_update_queue` (
  `id` int NOT NULL AUTO_INCREMENT,
  `requestPayloadJson` json NOT NULL,
  `status` enum('PENDING','PROCESSING','SUCCEEDED','FAILED','REJECTED_DAILY_LIMIT') NOT NULL DEFAULT 'PENDING',
  `errorMessage` varchar(500) DEFAULT NULL,
  `createdAt` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `processedAt` datetime DEFAULT NULL,
  `macroPresetId` int DEFAULT NULL,
  `requestedByCounterUuid` varchar(255) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `macro_update_queue_createdAt_idx` (`createdAt`),
  KEY `FK_c84620dd6282de31e7b5dd707bd` (`macroPresetId`),
  KEY `FK_b5bc29850a10c46e21a6fbbcfc8` (`requestedByCounterUuid`),
  CONSTRAINT `FK_b5bc29850a10c46e21a6fbbcfc8` FOREIGN KEY (`requestedByCounterUuid`) REFERENCES `counters` (`uuid`) ON DELETE CASCADE,
  CONSTRAINT `FK_c84620dd6282de31e7b5dd707bd` FOREIGN KEY (`macroPresetId`) REFERENCES `macro_preset` (`id`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `mentions` (
  `id` int NOT NULL AUTO_INCREMENT,
  `readStatus` tinyint NOT NULL DEFAULT '0',
  `postUUID` varchar(255) NOT NULL,
  `counterUuid` varchar(255) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `FK_7d3f979805136f81f2d40daf230` (`counterUuid`),
  CONSTRAINT `FK_7d3f979805136f81f2d40daf230` FOREIGN KEY (`counterUuid`) REFERENCES `counters` (`uuid`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `misc` (
  `id` int NOT NULL AUTO_INCREMENT,
  `name` varchar(255) NOT NULL,
  `val` json NOT NULL,
  PRIMARY KEY (`id`,`name`),
  KEY `IDX_f449e85e02123137d44ecdce81` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `miscsettings` (
  `id` int NOT NULL AUTO_INCREMENT,
  `categories` json NOT NULL,
  `lastUpdated` bigint NOT NULL,
  `userUuid` varchar(255) DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `REL_9cbc5707d270169f545f4cfd51` (`userUuid`),
  CONSTRAINT `FK_9cbc5707d270169f545f4cfd515` FOREIGN KEY (`userUuid`) REFERENCES `users` (`uuid`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `numbershuffle` (
  `id` int NOT NULL AUTO_INCREMENT,
  `client_id` varchar(255) DEFAULT NULL,
  `user_uuid` varchar(255) DEFAULT NULL,
  `seed` varchar(255) NOT NULL,
  `fakeSeed` varchar(255) NOT NULL,
  `size` int NOT NULL DEFAULT '10',
  `moves` json NOT NULL,
  `gameStatus` varchar(255) NOT NULL DEFAULT 'in_progress',
  `score` int NOT NULL DEFAULT '0',
  `isPreseeded` tinyint NOT NULL DEFAULT '0',
  `day` varchar(255) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `IDX_dcbd571e5b68b765fec086301c` (`client_id`),
  KEY `IDX_df3432b278dbd6f3174b361fd0` (`user_uuid`),
  KEY `IDX_186fe5bfc544aefe53b4c992b1` (`seed`),
  KEY `IDX_ce5a59c25d8a53e5faf3abc220` (`fakeSeed`),
  KEY `IDX_fcee346a6601d52c50eae370be` (`gameStatus`),
  KEY `IDX_5b6b68c5f94a7f20d9392fe8b1` (`day`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `post` (
  `id` int NOT NULL AUTO_INCREMENT,
  `uuid` varchar(255) NOT NULL,
  `timestamp` varchar(255) NOT NULL,
  `timeSinceLastCount` varchar(255) DEFAULT NULL,
  `timeSinceLastPost` varchar(255) DEFAULT NULL,
  `rawText` text NOT NULL,
  `isCount` tinyint NOT NULL,
  `isValidCount` tinyint DEFAULT NULL,
  `countContent` text,
  `rawCount` text,
  `stricken` tinyint NOT NULL,
  `thread` varchar(255) NOT NULL,
  `hasComment` tinyint NOT NULL,
  `comment` text,
  `authorUUID` varchar(255) NOT NULL,
  `isDeleted` tinyint NOT NULL,
  `isCommentDeleted` tinyint NOT NULL,
  `reactions` json NOT NULL,
  `validCountNumber` int DEFAULT NULL,
  `hasThreeCharComment` tinyint NOT NULL DEFAULT '0',
  `macroPresetId` int DEFAULT NULL,
  `macroPresetVersionId` int DEFAULT NULL,
  `threadId` smallint unsigned DEFAULT NULL,
  `authorId` mediumint unsigned DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `rawCountIdx` (`rawCount`(10)),
  KEY `IDX_1f469063a68c898f81186c0c11` (`timestamp`),
  KEY `uuid_prefix` (`uuid`(8)),
  KEY `n_thread_valid_id` (`threadId`,`isValidCount`,`id`),
  KEY `n_thread_id` (`threadId`,`id`),
  KEY `n_thread_validcountnum` (`threadId`,`validCountNumber`),
  KEY `n_thread_3char_id` (`threadId`,`hasThreeCharComment`,`id`),
  KEY `n_author_id_valid` (`authorId`,`id`,`isValidCount`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `postscroll` (
  `id` int NOT NULL AUTO_INCREMENT,
  `thread` varchar(255) NOT NULL,
  `postUUID` varchar(255) NOT NULL,
  `authorUUID` varchar(255) NOT NULL,
  `postScroll` json NOT NULL,
  PRIMARY KEY (`postUUID`),
  KEY `IDX_8ff209238e2da16b0efd60d003` (`id`),
  KEY `IDX_535a448d7a577b7b5825e80864` (`thread`),
  KEY `IDX_2177b35cd9eefbfd9bda8034a9` (`authorUUID`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `queuereqs` (
  `id` int NOT NULL AUTO_INCREMENT,
  `counter_uuid` varchar(255) NOT NULL,
  `createdAt` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
  `endedAt` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  `status` varchar(255) NOT NULL,
  `type` varchar(255) NOT NULL,
  `params` text NOT NULL,
  PRIMARY KEY (`id`),
  KEY `IDX_0bbd97335af0453a75c88392d8` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `rankchallenge` (
  `id` varchar(36) NOT NULL,
  `type` varchar(255) NOT NULL,
  `rank` varchar(255) NOT NULL,
  `sequence` int NOT NULL,
  `ggReward` int NOT NULL,
  `rewardType` varchar(255) NOT NULL DEFAULT 'gg',
  `target` int NOT NULL,
  `params` json DEFAULT NULL,
  `threadUuid` varchar(255) DEFAULT NULL,
  `bingoCategory` varchar(255) DEFAULT NULL,
  `seasonId` int NOT NULL,
  `clonedFromId` varchar(255) DEFAULT NULL,
  `createdAt` bigint NOT NULL,
  `baseReassignmentWaitMs` bigint DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `IDX_6e3d34b2a5719ae382022a26ef` (`type`),
  KEY `IDX_0dc5a09e0a28e683beb5792a59` (`rank`),
  KEY `IDX_cd4b8ee264d2023161e402442d` (`threadUuid`),
  KEY `IDX_29fd82bdffe64a45d50e77a1bb` (`seasonId`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `rankingcriteria` (
  `id` varchar(36) NOT NULL,
  `type` varchar(255) NOT NULL,
  `label` varchar(255) NOT NULL,
  `threadUuid` varchar(255) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `FK_e2d5b2b6910e308946fae585431` (`threadUuid`),
  CONSTRAINT `FK_e2d5b2b6910e308946fae585431` FOREIGN KEY (`threadUuid`) REFERENCES `thread` (`uuid`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `rankingthreshold` (
  `id` varchar(36) NOT NULL,
  `thresholdValue` int NOT NULL,
  `points` int NOT NULL DEFAULT '1',
  `rankingCriteriaId` varchar(36) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `FK_be6522c94ee2a83ab36595c20ae` (`rankingCriteriaId`),
  CONSTRAINT `FK_be6522c94ee2a83ab36595c20ae` FOREIGN KEY (`rankingCriteriaId`) REFERENCES `rankingcriteria` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `rankseason` (
  `id` int NOT NULL AUTO_INCREMENT,
  `name` varchar(255) NOT NULL,
  `startedAt` bigint NOT NULL,
  `endedAt` bigint DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `IDX_e57da8c3bcf92a871d041c8499` (`startedAt`),
  KEY `IDX_c9bc4ed4b15b37d50c9d977b82` (`endedAt`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `rankupevent` (
  `id` int NOT NULL AUTO_INCREMENT,
  `counterUuid` varchar(255) NOT NULL,
  `threadUuid` varchar(255) DEFAULT NULL,
  `seasonId` int DEFAULT NULL,
  `fromRank` varchar(255) NOT NULL,
  `fromDivision` int NOT NULL,
  `toRank` varchar(255) NOT NULL,
  `toDivision` int NOT NULL,
  `createdAt` bigint NOT NULL,
  `seenAt` bigint DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `IDX_33f7e2efeb66aad7620784c473` (`counterUuid`),
  KEY `IDX_cfcdcea9e339b0ab0766c4b516` (`threadUuid`),
  KEY `IDX_58692833b45dbf181db2f7a3fd` (`seasonId`),
  KEY `IDX_95a9165f2a0e3806cda4e12574` (`createdAt`),
  KEY `IDX_f44c8afca59abf423c97ca3f40` (`seenAt`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `rob` (
  `id` int NOT NULL AUTO_INCREMENT,
  `counterUUID` varchar(255) NOT NULL,
  `postUUID` varchar(255) NOT NULL,
  `moneyRobbed` bigint NOT NULL,
  `timestamp` varchar(255) NOT NULL,
  PRIMARY KEY (`id`),
  KEY `IDX_53ff3e36606a552fd4b31818fa` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `rpsgame` (
  `id` int NOT NULL AUTO_INCREMENT,
  `reason` varchar(255) DEFAULT NULL,
  `hostUserUuid` varchar(255) DEFAULT NULL,
  `challengerUserUuid` varchar(255) DEFAULT NULL,
  `winnerUuid` varchar(255) DEFAULT NULL,
  `loserUuid` varchar(255) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `FK_be800db5d33845ced3db35cd6cb` (`hostUserUuid`),
  KEY `FK_e63b26af612d998740e62e5998e` (`challengerUserUuid`),
  KEY `FK_76046bd0c5241372a6983bcb34e` (`winnerUuid`),
  KEY `FK_9102c91a8828d4ca53ff66b4d3f` (`loserUuid`),
  CONSTRAINT `FK_76046bd0c5241372a6983bcb34e` FOREIGN KEY (`winnerUuid`) REFERENCES `users` (`uuid`),
  CONSTRAINT `FK_9102c91a8828d4ca53ff66b4d3f` FOREIGN KEY (`loserUuid`) REFERENCES `users` (`uuid`),
  CONSTRAINT `FK_be800db5d33845ced3db35cd6cb` FOREIGN KEY (`hostUserUuid`) REFERENCES `users` (`uuid`),
  CONSTRAINT `FK_e63b26af612d998740e62e5998e` FOREIGN KEY (`challengerUserUuid`) REFERENCES `users` (`uuid`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `rpsmove` (
  `id` int NOT NULL AUTO_INCREMENT,
  `hostMove` tinyint unsigned NOT NULL,
  `challengerMove` tinyint unsigned NOT NULL,
  `gameId` int DEFAULT NULL,
  `challengerUserUuid` varchar(255) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `FK_e1cb7858e9c3d5a136a239ba9ad` (`gameId`),
  KEY `FK_0ab3e022f342bf232d1c01ffbad` (`challengerUserUuid`),
  CONSTRAINT `FK_0ab3e022f342bf232d1c01ffbad` FOREIGN KEY (`challengerUserUuid`) REFERENCES `users` (`uuid`),
  CONSTRAINT `FK_e1cb7858e9c3d5a136a239ba9ad` FOREIGN KEY (`gameId`) REFERENCES `rpsgame` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `server` (
  `id` int NOT NULL AUTO_INCREMENT,
  `serverID` varchar(255) NOT NULL,
  `name` varchar(255) NOT NULL,
  `avatar` varchar(255) NOT NULL,
  `counts` int NOT NULL,
  `admins` json NOT NULL,
  PRIMARY KEY (`id`),
  KEY `IDX_f8b8af38bdc23b447c0a57c793` (`id`),
  KEY `IDX_13287a451af4923e676bc19bc5` (`serverID`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `servers` (
  `id` int NOT NULL AUTO_INCREMENT,
  `name` varchar(255) NOT NULL,
  `avatar` varchar(255) DEFAULT NULL,
  `counts` int NOT NULL DEFAULT '0',
  `nsfw` tinyint NOT NULL DEFAULT '0',
  `isActive` tinyint NOT NULL DEFAULT '1',
  `canUsersJoinWithoutApproval` tinyint NOT NULL DEFAULT '1',
  `discordID` varchar(255) NOT NULL,
  `botMods` json DEFAULT NULL,
  `canUsersNotInTheServerJoin` tinyint NOT NULL DEFAULT '0',
  `kickUsersAfterTheyLeaveTheDiscordServer` tinyint NOT NULL DEFAULT '1',
  `isBanned` tinyint NOT NULL DEFAULT '0',
  PRIMARY KEY (`id`,`discordID`),
  UNIQUE KEY `IDX_42487edef17a6a132abe1c3669` (`id`,`discordID`),
  KEY `IDX_bdaafb02c4fa6fd03fbf579cd4` (`isActive`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `servers_members_counters` (
  `serversId` int NOT NULL,
  `serversDiscordID` varchar(255) NOT NULL,
  `countersUuid` varchar(255) NOT NULL,
  PRIMARY KEY (`serversId`,`serversDiscordID`,`countersUuid`),
  KEY `IDX_30ad75df8292aea149654d622d` (`serversId`,`serversDiscordID`),
  KEY `IDX_875fbac0a1a9d2cdcabee648c0` (`countersUuid`),
  CONSTRAINT `FK_30ad75df8292aea149654d622df` FOREIGN KEY (`serversId`, `serversDiscordID`) REFERENCES `servers` (`id`, `discordID`) ON DELETE CASCADE ON UPDATE CASCADE,
  CONSTRAINT `FK_875fbac0a1a9d2cdcabee648c0a` FOREIGN KEY (`countersUuid`) REFERENCES `counters` (`uuid`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `servers_users_users` (
  `serversId` int NOT NULL,
  `serversDiscordID` varchar(255) NOT NULL,
  `usersUuid` varchar(255) NOT NULL,
  PRIMARY KEY (`serversId`,`serversDiscordID`,`usersUuid`),
  KEY `IDX_321f08da28fb04949fcd941021` (`serversId`,`serversDiscordID`),
  KEY `IDX_2df93a03a77a8b4bee19465cd6` (`usersUuid`),
  CONSTRAINT `FK_2df93a03a77a8b4bee19465cd6f` FOREIGN KEY (`usersUuid`) REFERENCES `users` (`uuid`),
  CONSTRAINT `FK_321f08da28fb04949fcd941021f` FOREIGN KEY (`serversId`, `serversDiscordID`) REFERENCES `servers` (`id`, `discordID`) ON DELETE CASCADE ON UPDATE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `session` (
  `expiredAt` bigint NOT NULL,
  `id` varchar(255) NOT NULL,
  `json` text NOT NULL,
  PRIMARY KEY (`id`),
  KEY `IDX_28c5d1d16da7908c97c9bc2f74` (`expiredAt`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `site_goal` (
  `id` int NOT NULL AUTO_INCREMENT,
  `goalKey` varchar(64) NOT NULL,
  `name` varchar(120) NOT NULL,
  `description` text,
  `funding` varchar(16) NOT NULL DEFAULT 'community',
  `targetCost` bigint NOT NULL,
  `raised` bigint NOT NULL DEFAULT '0',
  `unlockedAt` bigint DEFAULT NULL,
  `unlockedBy` varchar(255) DEFAULT NULL,
  `active` tinyint NOT NULL DEFAULT '1',
  `sortOrder` int NOT NULL DEFAULT '0',
  `createdAt` bigint DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `IDX_c6e41166a29ed23e76d7b9a0fe` (`goalKey`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `site_goal_contribution` (
  `id` int NOT NULL AUTO_INCREMENT,
  `goalKey` varchar(64) NOT NULL,
  `counterUuid` varchar(255) NOT NULL,
  `amount` bigint NOT NULL,
  `createdAt` bigint NOT NULL,
  PRIMARY KEY (`id`),
  KEY `n_sgc_goal` (`goalKey`),
  KEY `n_sgc_counter` (`counterUuid`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `thread` (
  `id` int NOT NULL AUTO_INCREMENT,
  `uuid` varchar(255) NOT NULL,
  `name` varchar(255) NOT NULL,
  `title` varchar(255) NOT NULL,
  `description` text NOT NULL,
  `rules` text NOT NULL,
  `firstValidCount` varchar(255) NOT NULL,
  `validationType` varchar(255) NOT NULL,
  `resetOnMistakes` tinyint NOT NULL,
  `allowDoublePosts` tinyint NOT NULL,
  `visibleTo` json NOT NULL,
  `updatableBy` json NOT NULL,
  `locked` tinyint NOT NULL,
  `autoValidated` tinyint NOT NULL,
  `moderators` json NOT NULL,
  `verifiers` json NOT NULL,
  `countBans` json NOT NULL,
  `postBans` json NOT NULL,
  `shortDescription` varchar(255) NOT NULL,
  `color1` varchar(255) NOT NULL,
  `color2` varchar(255) NOT NULL,
  `category` varchar(255) NOT NULL,
  `countsPerSplit` int NOT NULL DEFAULT '100',
  `splitsPerGet` int NOT NULL DEFAULT '10',
  `splitOffset` int NOT NULL DEFAULT '0',
  `serverId` int DEFAULT NULL,
  `serverDiscordID` varchar(255) DEFAULT NULL,
  `threadOfTheDay` tinyint NOT NULL DEFAULT '0',
  `type` varchar(255) NOT NULL DEFAULT 'global',
  PRIMARY KEY (`uuid`),
  UNIQUE KEY `IDX_bf03d98fc03d4d9c1b98ca130c` (`name`),
  KEY `IDX_cabc0f3f27d7b1c70cf64623e0` (`id`),
  KEY `FK_b4f6128e95d962863f0c9aaaadf` (`serverId`,`serverDiscordID`),
  CONSTRAINT `FK_b4f6128e95d962863f0c9aaaadf` FOREIGN KEY (`serverId`, `serverDiscordID`) REFERENCES `servers` (`id`, `discordID`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `thread_macro_preset_preference` (
  `id` int NOT NULL AUTO_INCREMENT,
  `enabled` tinyint NOT NULL DEFAULT '0',
  `updatedAt` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  `userUuid` varchar(255) DEFAULT NULL,
  `threadUuid` varchar(255) DEFAULT NULL,
  `macroPresetId` int DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `thread_macro_preset_preference_user_thread_uq` (`userUuid`,`threadUuid`),
  KEY `thread_macro_preset_preference_thread_enabled_macro_idx` (`threadUuid`,`enabled`,`macroPresetId`),
  KEY `thread_macro_preset_preference_macro_enabled_thread_idx` (`macroPresetId`,`enabled`,`threadUuid`),
  KEY `thread_macro_preset_preference_macro_idx` (`macroPresetId`),
  KEY `thread_macro_preset_preference_thread_idx` (`threadUuid`),
  KEY `thread_macro_preset_preference_user_idx` (`userUuid`),
  CONSTRAINT `FK_20ee929fb3adcaac2e1aac3364b` FOREIGN KEY (`macroPresetId`) REFERENCES `macro_preset` (`id`) ON DELETE SET NULL,
  CONSTRAINT `FK_47113052661786bc32340eaa643` FOREIGN KEY (`userUuid`) REFERENCES `users` (`uuid`) ON DELETE CASCADE,
  CONSTRAINT `FK_75ed0ca11b35029b0571824c108` FOREIGN KEY (`threadUuid`) REFERENCES `thread` (`uuid`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `thread_macro_preset_usage` (
  `id` int NOT NULL AUTO_INCREMENT,
  `appliesCount` int NOT NULL DEFAULT '0',
  `lastAppliedAt` datetime(6) DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  `threadUuid` varchar(255) DEFAULT NULL,
  `macroPresetId` int DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `thread_macro_preset_usage_thread_macro_uq` (`threadUuid`,`macroPresetId`),
  KEY `FK_c4668dc4fc1b7b769ba8a8b3e3f` (`macroPresetId`),
  CONSTRAINT `FK_4ba3d5e9588ec4ef6a8668f7ac1` FOREIGN KEY (`threadUuid`) REFERENCES `thread` (`uuid`) ON DELETE CASCADE,
  CONSTRAINT `FK_c4668dc4fc1b7b769ba8a8b3e3f` FOREIGN KEY (`macroPresetId`) REFERENCES `macro_preset` (`id`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `thread_preferences` (
  `id` int NOT NULL AUTO_INCREMENT,
  `enabled` tinyint NOT NULL DEFAULT '0',
  `pref_online` tinyint DEFAULT NULL,
  `pref_discord_pings` tinyint DEFAULT NULL,
  `pref_load_from_bottom` tinyint DEFAULT NULL,
  `pref_strike_color` varchar(255) DEFAULT NULL,
  `pref_submit_shortcut` varchar(255) DEFAULT NULL,
  `pref_clear` varchar(255) DEFAULT NULL,
  `pref_nightMode` varchar(255) DEFAULT NULL,
  `pref_standardize_format` varchar(255) DEFAULT NULL,
  `pref_time_since_last_count` tinyint DEFAULT NULL,
  `pref_custom_stricken` varchar(255) DEFAULT NULL,
  `pref_post_style` varchar(255) DEFAULT NULL,
  `pref_post_style_mobile` varchar(255) DEFAULT NULL,
  `pref_reply_time_interval` int DEFAULT NULL,
  `pref_night_mode_colors` varchar(255) DEFAULT NULL,
  `pref_post_position` varchar(255) DEFAULT NULL,
  `pref_hide_stricken` varchar(255) DEFAULT NULL,
  `pref_highlight_last_count` tinyint DEFAULT NULL,
  `pref_highlight_last_count_color` varchar(255) DEFAULT NULL,
  `pref_sound_on_stricken` varchar(255) DEFAULT NULL,
  `pref_hide_thread_picker` tinyint DEFAULT NULL,
  `pref_stricken_count_opacity` decimal(3,2) DEFAULT NULL,
  `pref_timestamp_display` varchar(255) DEFAULT NULL,
  `pref_show_latency` tinyint DEFAULT NULL,
  `userUuid` varchar(255) DEFAULT NULL,
  `threadUuid` varchar(255) DEFAULT NULL,
  `pref_pace_mode` varchar(255) DEFAULT NULL,
  `pref_pace_counts` int DEFAULT NULL,
  `pref_pace_target_ms` int DEFAULT NULL,
  `pref_post_font_size` smallint DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `FK_9c7a9fdcdee16af8e46468cbdce` (`userUuid`),
  KEY `FK_88b8797331a96ca878b0faed050` (`threadUuid`),
  CONSTRAINT `FK_88b8797331a96ca878b0faed050` FOREIGN KEY (`threadUuid`) REFERENCES `thread` (`uuid`) ON DELETE CASCADE,
  CONSTRAINT `FK_9c7a9fdcdee16af8e46468cbdce` FOREIGN KEY (`userUuid`) REFERENCES `users` (`uuid`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `threadrank` (
  `counterUuid` varchar(255) NOT NULL,
  `threadUuid` varchar(255) DEFAULT NULL,
  `seasonId` int DEFAULT NULL,
  `rank` varchar(255) NOT NULL DEFAULT 'bronze',
  `division` int NOT NULL DEFAULT '1',
  `gg` int NOT NULL DEFAULT '0',
  `ggTotal` int NOT NULL DEFAULT '0',
  `id` int NOT NULL AUTO_INCREMENT,
  PRIMARY KEY (`id`),
  UNIQUE KEY `IDX_e015e77e5fa7fe662716f60ffb` (`counterUuid`,`threadUuid`,`seasonId`),
  KEY `IDX_7efdf862fa55217875240acd77` (`counterUuid`),
  KEY `IDX_2fba1749468b02ead3f178bc56` (`threadUuid`),
  KEY `IDX_6d10ad884b6925f197a44281b9` (`seasonId`),
  KEY `IDX_0ad1c6e9080cb080a21d877242` (`rank`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `threadrankthreshold` (
  `id` varchar(36) NOT NULL,
  `rankName` varchar(255) NOT NULL,
  `pointsThreshold` int NOT NULL,
  `threadUuid` varchar(255) DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `FK_db4e676298ce411fc59d4838480` (`threadUuid`),
  CONSTRAINT `FK_db4e676298ce411fc59d4838480` FOREIGN KEY (`threadUuid`) REFERENCES `thread` (`uuid`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `user_macro_preset_preference` (
  `id` int NOT NULL AUTO_INCREMENT,
  `updatedAt` datetime(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6) ON UPDATE CURRENT_TIMESTAMP(6),
  `userUuid` varchar(255) DEFAULT NULL,
  `macroPresetId` int DEFAULT NULL,
  PRIMARY KEY (`id`),
  UNIQUE KEY `user_macro_preset_preference_user_uq` (`userUuid`),
  KEY `FK_b75112fc3a06544c88c3911c3ef` (`macroPresetId`),
  CONSTRAINT `FK_b75112fc3a06544c88c3911c3ef` FOREIGN KEY (`macroPresetId`) REFERENCES `macro_preset` (`id`) ON DELETE SET NULL,
  CONSTRAINT `FK_eb125f55dca4c2accd32e951a5f` FOREIGN KEY (`userUuid`) REFERENCES `users` (`uuid`) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `useritem` (
  `id` int NOT NULL AUTO_INCREMENT,
  `unlocked` tinyint NOT NULL DEFAULT '0',
  `userUuid` varchar(255) DEFAULT NULL,
  `itemId` int DEFAULT NULL,
  `consumedAt` bigint DEFAULT NULL,
  PRIMARY KEY (`id`),
  KEY `FK_2f153bc51ca4d78fd90eed93e3d` (`userUuid`),
  KEY `FK_e28622a688fba1f16b31f1d065a` (`itemId`),
  CONSTRAINT `FK_2f153bc51ca4d78fd90eed93e3d` FOREIGN KEY (`userUuid`) REFERENCES `users` (`uuid`),
  CONSTRAINT `FK_e28622a688fba1f16b31f1d065a` FOREIGN KEY (`itemId`) REFERENCES `item` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `usermiscsettings` (
  `id` int NOT NULL AUTO_INCREMENT,
  PRIMARY KEY (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `users` (
  `id` int NOT NULL AUTO_INCREMENT,
  `uuid` varchar(255) NOT NULL,
  `discord_id` varchar(255) NOT NULL,
  `access_token` varchar(255) NOT NULL,
  `refresh_token` varchar(255) NOT NULL,
  `username` varchar(255) NOT NULL,
  `discriminator` varchar(255) NOT NULL,
  `pref_online` tinyint NOT NULL,
  `pref_discord_pings` tinyint NOT NULL,
  `pref_load_from_bottom` tinyint NOT NULL,
  `pref_strike_color` varchar(255) NOT NULL,
  `pref_submit_shortcut` varchar(255) NOT NULL,
  `pref_nightMode` varchar(255) NOT NULL,
  `pref_standardize_format` varchar(255) NOT NULL,
  `pref_time_since_last_count` tinyint NOT NULL,
  `pref_custom_stricken` varchar(255) NOT NULL,
  `timeOnline` varchar(255) NOT NULL,
  `pref_post_style` varchar(255) NOT NULL,
  `inventory` json NOT NULL,
  `money` bigint NOT NULL DEFAULT '2000',
  `pref_clear` varchar(255) NOT NULL,
  `pref_reply_time_interval` int NOT NULL,
  `pref_night_mode_colors` varchar(255) NOT NULL,
  `pref_post_position` varchar(255) NOT NULL,
  `pref_hide_stricken` varchar(255) NOT NULL,
  `pref_highlight_last_count` tinyint NOT NULL DEFAULT '0',
  `pref_highlight_last_count_color` varchar(255) NOT NULL DEFAULT '#00b2ff',
  `pref_sound_on_stricken` varchar(255) NOT NULL DEFAULT 'Disabled',
  `pref_hide_thread_picker` tinyint NOT NULL DEFAULT '0',
  `pref_stricken_count_opacity` decimal(3,2) NOT NULL DEFAULT '1.00',
  `reddit` varchar(255) DEFAULT NULL,
  `redditAccess` text,
  `redditRefresh` text,
  `pref_post_style_mobile` varchar(255) NOT NULL DEFAULT 'Default',
  `pref_timestamp_display` varchar(255) NOT NULL DEFAULT 'Milliseconds',
  `pref_show_latency` tinyint NOT NULL DEFAULT '1',
  `miscSettingsId` int DEFAULT NULL,
  `pref_pace_mode` varchar(255) NOT NULL DEFAULT 'Split',
  `pref_pace_counts` int NOT NULL DEFAULT '100',
  `pref_pace_target_ms` int NOT NULL DEFAULT '0',
  `pref_chat_default` varchar(255) NOT NULL DEFAULT 'Hidden',
  `pref_post_font_size` smallint NOT NULL DEFAULT '16',
  `pref_milestones` tinyint NOT NULL DEFAULT '1',
  PRIMARY KEY (`uuid`),
  UNIQUE KEY `IDX_ecb6461da358b6d8a4f83d611a` (`discord_id`),
  UNIQUE KEY `REL_3cb554d5a531085b9042570104` (`miscSettingsId`),
  KEY `IDX_a3ffb1c0c8416b9fc6f907b743` (`id`),
  CONSTRAINT `FK_3cb554d5a531085b9042570104c` FOREIGN KEY (`miscSettingsId`) REFERENCES `miscsettings` (`id`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `users_servers_servers` (
  `usersUuid` varchar(255) NOT NULL,
  `serversId` int NOT NULL,
  `serversDiscordID` varchar(255) NOT NULL,
  PRIMARY KEY (`usersUuid`,`serversId`,`serversDiscordID`),
  KEY `IDX_e05d88690d572eca16edf4e3b9` (`usersUuid`),
  KEY `IDX_6c79bf283822a2b53857817f17` (`serversId`,`serversDiscordID`),
  CONSTRAINT `FK_6c79bf283822a2b53857817f171` FOREIGN KEY (`serversId`, `serversDiscordID`) REFERENCES `servers` (`id`, `discordID`),
  CONSTRAINT `FK_e05d88690d572eca16edf4e3b97` FOREIGN KEY (`usersUuid`) REFERENCES `users` (`uuid`) ON DELETE CASCADE ON UPDATE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

CREATE TABLE `userserver` (
  `id` int NOT NULL AUTO_INCREMENT,
  `userID` varchar(255) NOT NULL,
  `serverID` varchar(255) NOT NULL,
  PRIMARY KEY (`id`,`userID`)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;

