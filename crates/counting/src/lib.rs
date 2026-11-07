pub mod helpers;
pub mod js;
pub mod parse;
pub mod seedrandom;
pub mod validate;

pub use parse::{Parsed, parse_input, parse_spaces};
pub use validate::{Env, Input, PrevCount, ThreadRules, Verdict, validate};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValidationType {
    Main,
    Slow,
    Bars,
    Parity,
    Yoco,
    Roulette,
    Tslc,
    Randomhour,
    Randomminute,
    Waitx,
    OneInX,
    Countdown,
    Tugofwar,
    Repdigits,
    NoRepdigits,
    RepeatingDigits,
    MostlyRepeatingDigits,
    Factoradic,
    Username,
    FastTslc,
    Wait2,
    IncrementalOdds,
    By3,
    Coordinates,
    CoordinatesTow,
    Letters,
    Binary,
    Ternary,
    Quaternary,
    Quinary,
    Senary,
    Septenary,
    Octonary,
    Nonary,
    Hexadecimal,
    Alphanumerics,
    Base62,
    Base64,
    Wave,
    Mayan,
    Permutations,
    Dates,
    Military,
    Time,
    Rationals,
    Roman,
    Words,
    Ordinals,
    G2048,
    Other,
}

impl ValidationType {
    pub fn from_name(s: &str) -> Self {
        use ValidationType::*;
        match s {
            "main" => Main,
            "slow" => Slow,
            "bars" => Bars,
            "parity" => Parity,
            "yoco" => Yoco,
            "roulette" => Roulette,
            "tslc" => Tslc,
            "randomhour" => Randomhour,
            "randomminute" => Randomminute,
            "waitx" => Waitx,
            "1inx" => OneInX,
            "countdown" => Countdown,
            "tugofwar" => Tugofwar,
            "repdigits" => Repdigits,
            "no_repdigits" => NoRepdigits,
            "repeating_digits" => RepeatingDigits,
            "mostly_repeating_digits" => MostlyRepeatingDigits,
            "factoradic" => Factoradic,
            "username" => Username,
            "fast_tslc" => FastTslc,
            "wait_2" => Wait2,
            "incremental_odds" => IncrementalOdds,
            "by3" => By3,
            "coordinates" => Coordinates,
            "coordinates_tow" => CoordinatesTow,
            "letters" => Letters,
            "binary" => Binary,
            "ternary" => Ternary,
            "quaternary" => Quaternary,
            "quinary" => Quinary,
            "senary" => Senary,
            "septenary" => Septenary,
            "octonary" => Octonary,
            "nonary" => Nonary,
            "hexadecimal" => Hexadecimal,
            "alphanumerics" => Alphanumerics,
            "base62" => Base62,
            "base64" => Base64,
            "wave" => Wave,
            "mayan" => Mayan,
            "permutations" => Permutations,
            "dates" => Dates,
            "military" => Military,
            "time" => Time,
            "rationals" => Rationals,
            "roman" => Roman,
            "words" => Words,
            "ordinals" => Ordinals,
            "2048" => G2048,
            _ => Other,
        }
    }

    pub fn is_numeric(self) -> bool {
        use ValidationType::*;
        matches!(
            self,
            Main | Slow
                | Bars
                | Parity
                | Yoco
                | Roulette
                | Tslc
                | Randomhour
                | Randomminute
                | Waitx
                | OneInX
                | Countdown
                | Tugofwar
                | Repdigits
                | NoRepdigits
                | RepeatingDigits
                | MostlyRepeatingDigits
                | Factoradic
                | Username
                | FastTslc
                | Wait2
                | IncrementalOdds
                | By3
        )
    }

    pub fn base_n_radix(self) -> Option<u32> {
        use ValidationType::*;
        Some(match self {
            Binary => 2,
            Ternary => 3,
            Quaternary => 4,
            Quinary => 5,
            Senary => 6,
            Septenary => 7,
            Octonary => 8,
            Nonary => 9,
            _ => return None,
        })
    }

    pub fn is_nondeterministic(self) -> bool {
        use ValidationType::*;
        matches!(self, OneInX | IncrementalOdds | Randomhour | Randomminute | Waitx)
    }
}

#[cfg(test)]
mod tests {
    use super::seedrandom::SeedRandom;

    #[test]
    fn seedrandom_matches_node() {
        let mut r = SeedRandom::new("hello");
        assert_eq!(r.next_f64(), 0.5463663768140734);
        assert_eq!(r.next_f64(), 0.4397379377059223);
        assert_eq!(r.next_f64(), 0.554769432473455);
        let mut r = SeedRandom::new("random_minute_2026-10-06");
        assert_eq!(r.next_f64(), 0.12384247940308211);
        assert_eq!(SeedRandom::new("").next_f64(), 0.23144008215179881);
    }
}
