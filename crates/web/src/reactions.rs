use std::collections::HashSet;
use std::sync::LazyLock;

static REACTIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut s: HashSet<&'static str> = include_str!("reactions.txt")
        .lines()
        .filter(|l| !l.is_empty())
        .collect();
    s.extend([
        "pog",
        "got_it",
        "dafek",
        "fr-emoji",
        "soaked",
        "PLEASE",
        "THANKS",
        "wholesome",
        "upvote",
        "downvote",
        "beenaday",
        "pogfish",
        "angery",
        "catcry",
        "cuteballgames",
        "gg",
        "goldpincus",
        "happyegg",
        "letsgo",
        "phazer",
        "pincushd",
        "s1bronze",
        "s1plat",
        "sadge",
        "sussy",
    ]);
    s
});

pub fn builtin(id: &str) -> bool {
    REACTIONS.contains(id)
}
