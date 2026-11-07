pub const STANDARDIZE_FORMAT: [&str; 5] = ["Disabled", "No Separator", "Commas", "Periods", "Spaces"];
pub const NIGHT_MODE: [&str; 3] = ["System", "On", "Off"];
pub const SUBMIT_SHORTCUT: [&str; 3] = ["CtrlEnter", "Enter", "Off"];
pub const CUSTOM_STRICKEN: [&str; 3] = ["Disabled", "Enabled", "Inverse"];
pub const CHAT_DEFAULT: [&str; 3] = ["Hidden", "Collapsed", "Open"];
pub const PACE_MODE: [&str; 3] = ["Split", "Get", "Counts"];
pub const SOUND_ON_STRICKEN: [&str; 3] = ["Disabled", "All Stricken", "Only My Counts"];
pub const POST_STYLE: [&str; 4] = ["Default", "Classic", "LC", "Minimal"];
pub const CLEAR: [&str; 4] = ["Clear", "No Clear", "Clipboard", "Custom"];
pub const NIGHT_MODE_COLORS: [&str; 3] = ["Default", "Light", "Dark"];
pub const POST_POSITION: [&str; 2] = ["Left", "Right"];
pub const HIDE_STRICKEN: [&str; 3] = ["Disabled", "Minimize", "Hide"];

pub const PRONOUNS: [[&str; 4]; 3] = [
    ["he", "him", "his", "his"],
    ["she", "her", "her", "hers"],
    ["they", "them", "their", "theirs"],
];

pub struct Level {
    pub level: &'static str,
    pub min_xp: i64,
}

pub const LEVELS: [Level; 65] = [
    Level { level: "1", min_xp: 0 },
    Level {
        level: "2",
        min_xp: 100,
    },
    Level {
        level: "3",
        min_xp: 250,
    },
    Level {
        level: "4",
        min_xp: 500,
    },
    Level {
        level: "5",
        min_xp: 800,
    },
    Level {
        level: "6",
        min_xp: 1200,
    },
    Level {
        level: "7",
        min_xp: 1700,
    },
    Level {
        level: "8",
        min_xp: 2500,
    },
    Level {
        level: "9",
        min_xp: 3500,
    },
    Level {
        level: "10",
        min_xp: 5000,
    },
    Level {
        level: "11",
        min_xp: 6666,
    },
    Level {
        level: "12",
        min_xp: 8333,
    },
    Level {
        level: "13",
        min_xp: 10000,
    },
    Level {
        level: "14",
        min_xp: 12345,
    },
    Level {
        level: "15",
        min_xp: 15000,
    },
    Level {
        level: "16",
        min_xp: 20000,
    },
    Level {
        level: "17",
        min_xp: 26000,
    },
    Level {
        level: "18",
        min_xp: 33000,
    },
    Level {
        level: "19",
        min_xp: 42069,
    },
    Level {
        level: "20",
        min_xp: 54321,
    },
    Level {
        level: "21",
        min_xp: 69420,
    },
    Level {
        level: "22",
        min_xp: 84999,
    },
    Level {
        level: "23",
        min_xp: 100000,
    },
    Level {
        level: "24",
        min_xp: 118000,
    },
    Level {
        level: "25",
        min_xp: 138000,
    },
    Level {
        level: "26",
        min_xp: 160000,
    },
    Level {
        level: "27",
        min_xp: 183000,
    },
    Level {
        level: "28",
        min_xp: 208000,
    },
    Level {
        level: "29",
        min_xp: 235000,
    },
    Level {
        level: "30",
        min_xp: 265000,
    },
    Level {
        level: "31",
        min_xp: 300000,
    },
    Level {
        level: "32",
        min_xp: 337000,
    },
    Level {
        level: "33",
        min_xp: 377000,
    },
    Level {
        level: "34",
        min_xp: 420069,
    },
    Level {
        level: "35",
        min_xp: 465000,
    },
    Level {
        level: "36",
        min_xp: 509000,
    },
    Level {
        level: "37",
        min_xp: 555555,
    },
    Level {
        level: "38",
        min_xp: 605000,
    },
    Level {
        level: "39",
        min_xp: 666666,
    },
    Level {
        level: "40",
        min_xp: 725000,
    },
    Level {
        level: "41",
        min_xp: 800000,
    },
    Level {
        level: "42",
        min_xp: 888888,
    },
    Level {
        level: "43",
        min_xp: 1000000,
    },
    Level {
        level: "44",
        min_xp: 1234567,
    },
    Level {
        level: "45",
        min_xp: 1500000,
    },
    Level {
        level: "46",
        min_xp: 1750000,
    },
    Level {
        level: "47",
        min_xp: 2020202,
    },
    Level {
        level: "48",
        min_xp: 2270000,
    },
    Level {
        level: "49",
        min_xp: 2600000,
    },
    Level {
        level: "50",
        min_xp: 3000000,
    },
    Level {
        level: "51",
        min_xp: 3500000,
    },
    Level {
        level: "52",
        min_xp: 4000000,
    },
    Level {
        level: "53",
        min_xp: 4500000,
    },
    Level {
        level: "54",
        min_xp: 5000000,
    },
    Level {
        level: "55",
        min_xp: 5500000,
    },
    Level {
        level: "56",
        min_xp: 6000000,
    },
    Level {
        level: "57",
        min_xp: 6500000,
    },
    Level {
        level: "58",
        min_xp: 7000000,
    },
    Level {
        level: "59",
        min_xp: 7500000,
    },
    Level {
        level: "60",
        min_xp: 8000000,
    },
    Level {
        level: "61",
        min_xp: 8500000,
    },
    Level {
        level: "62",
        min_xp: 9000000,
    },
    Level {
        level: "63",
        min_xp: 9500000,
    },
    Level {
        level: "64",
        min_xp: 10000000,
    },
    Level {
        level: "65",
        min_xp: 11111111,
    },
];

pub fn level_for(xp: f64) -> &'static Level {
    LEVELS
        .iter()
        .rev()
        .find(|l| xp >= l.min_xp as f64)
        .unwrap_or(&LEVELS[0])
}

pub fn is_hex6(s: &str) -> bool {
    s.len() == 6 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod level_tests {
    use super::*;

    #[test]
    fn levels_follow_xp_like_node() {
        assert_eq!(level_for(0.0).level, "1");
        assert_eq!(level_for(250.0).level, "3");
        assert_eq!(level_for(100_000.0).level, "23");
        assert_eq!(level_for(11_111_111.0).level, "65");
        assert_eq!(level_for(f64::NAN).level, "1");
    }
}
