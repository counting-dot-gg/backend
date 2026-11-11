use std::collections::HashMap;

pub const VPS: &str = "vps";
const MIN_GAIN_MS: f64 = 25.0;
const MIN_GAIN_SHARE: f64 = 0.2;
const COVERAGE: f64 = 0.8;

pub type Latencies = HashMap<String, HashMap<String, f64>>;

pub fn shares<'a>(authors: impl IntoIterator<Item = &'a str>) -> Vec<(String, f64)> {
    let mut counts: HashMap<&str, f64> = HashMap::new();
    let mut total = 0.0;
    for a in authors {
        *counts.entry(a).or_default() += 1.0;
        total += 1.0;
    }
    let mut out: Vec<(String, f64)> = counts.into_iter().map(|(a, n)| (a.to_owned(), n / total)).collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

pub fn costs(shares: &[(String, f64)], latency: &Latencies, places: &[&str]) -> HashMap<String, f64> {
    let mut out = HashMap::new();
    for place in places {
        let (mut covered, mut weighted) = (0.0, 0.0);
        for (author, share) in shares {
            if let Some(ms) = latency.get(author).and_then(|l| l.get(*place)) {
                covered += share;
                weighted += share * ms;
            }
        }
        if covered >= COVERAGE {
            out.insert((*place).to_owned(), weighted / covered);
        }
    }
    out
}

pub fn choose(current: &str, costs: &HashMap<String, f64>) -> Option<String> {
    let here = *costs.get(current)?;
    let (best, cost) = costs
        .iter()
        .min_by(|a, b| a.1.total_cmp(b.1).then_with(|| a.0.cmp(b.0)))?;
    (best != current && here - cost >= MIN_GAIN_MS.max(MIN_GAIN_SHARE * here)).then(|| best.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn latencies(rows: &[(&str, &[(&str, f64)])]) -> Latencies {
        rows.iter()
            .map(|(who, places)| {
                (
                    (*who).to_owned(),
                    places.iter().map(|(p, ms)| ((*p).to_owned(), *ms)).collect(),
                )
            })
            .collect()
    }

    const PLACES: [&str; 4] = ["vps", "weur", "enam", "apac"];

    #[test]
    fn a_solo_runner_gets_their_closest_place() {
        let l = latencies(&[("eu", &[("vps", 90.0), ("weur", 15.0), ("enam", 85.0), ("apac", 250.0)])]);
        let c = costs(&shares(["eu"; 10]), &l, &PLACES);
        assert_eq!(choose("vps", &c).as_deref(), Some("weur"));
        assert_eq!(choose("weur", &c), None);
    }

    #[test]
    fn a_duo_minimises_their_summed_round_trips() {
        let l = latencies(&[
            ("eu", &[("vps", 90.0), ("weur", 15.0), ("enam", 85.0), ("apac", 250.0)]),
            ("us", &[("vps", 20.0), ("weur", 90.0), ("enam", 10.0), ("apac", 200.0)]),
        ]);
        let c = costs(&shares(["eu", "us", "eu", "us"]), &l, &PLACES);
        assert!((c["weur"] - 52.5).abs() < 1e-9);
        assert!((c["enam"] - 47.5).abs() < 1e-9);
        assert_eq!(choose("vps", &c), None);
        assert_eq!(choose("apac", &c).as_deref(), Some("enam"));
        let l = latencies(&[
            ("eu", &[("vps", 90.0), ("weur", 15.0)]),
            ("eu2", &[("vps", 100.0), ("weur", 30.0)]),
        ]);
        let c = costs(&shares(["eu", "eu2", "eu", "eu2"]), &l, &PLACES);
        assert_eq!(choose("vps", &c).as_deref(), Some("weur"));
    }

    #[test]
    fn someone_dropping_one_count_does_not_move_a_run() {
        let l = latencies(&[
            ("eu", &[("vps", 90.0), ("weur", 15.0)]),
            ("asia", &[("vps", 220.0), ("weur", 300.0), ("apac", 10.0)]),
        ]);
        let mut run = vec!["eu"; 29];
        run.push("asia");
        let c = costs(&shares(run), &l, &PLACES);
        assert_eq!(choose("weur", &c), None);
        assert!(!c.contains_key("apac"));
    }

    #[test]
    fn small_gains_are_not_worth_a_move() {
        let l = latencies(&[("us", &[("vps", 40.0), ("enam", 25.0)])]);
        let c = costs(&shares(["us"]), &l, &PLACES);
        assert_eq!(choose("vps", &c), None);
    }

    #[test]
    fn without_a_measure_of_where_it_is_now_nothing_moves() {
        let l = latencies(&[("eu", &[("weur", 15.0)])]);
        let c = costs(&shares(["eu"]), &l, &PLACES);
        assert_eq!(choose("vps", &c), None);
    }
}
