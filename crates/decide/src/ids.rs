pub fn uuid_for(ns: u64, r: f64) -> String {
    let s = format!("{ns:019}{}", (r * 9e4) as u32 + 10000);
    let v = ['8', '9', 'a'][((r * 3.0) as usize).min(2)];
    format!(
        "{}-{}-4{}-{}{}-{}{}",
        &s[0..8],
        &s[8..12],
        &s[12..15],
        v,
        &s[15..18],
        &s[18..24],
        &s[12..18]
    )
}

pub fn timestamp_for(ns: u64) -> String {
    counting::js::num_to_string((ns / 1000) as f64 / 1000.0)
}

fn timestamp_ns(t: &str) -> Option<u64> {
    let (ms, fraction) = t.split_once('.').unwrap_or((t, ""));
    let fraction: String = fraction.chars().chain(std::iter::repeat('0')).take(6).collect();
    Some(ms.parse::<u64>().ok()? * 1_000_000 + fraction.parse::<u64>().ok()?)
}

pub fn ns_after(last_timestamp: Option<&str>, wanted: u64) -> u64 {
    let after_last = last_timestamp.and_then(timestamp_ns).map_or(0, |ns| ns + 1_000);
    wanted.max(after_last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape() {
        let u = uuid_for(1791321226163219123, 0.5);
        assert_eq!(u.len(), 36);
        assert!(u.starts_with("17913212-2616-4321-"));
        assert_eq!(&u[30..36], format!("{}{}", &u[15..18], &u[20..23]));
        assert!(matches!(&u[19..20], "8" | "9" | "a"));
        assert_eq!(timestamp_for(1791321226163219123), "1791321226163.219");
        assert_eq!(timestamp_for(1791321226163200000), "1791321226163.2");
    }

    #[test]
    fn a_post_is_never_stamped_before_the_previous_one() {
        let last = "1791321226163.219";
        let early = timestamp_for(ns_after(Some(last), 1791321226000000000));
        assert!(counting::js::parse_float(&early) > counting::js::parse_float(last));
        assert_eq!(ns_after(Some(last), 1791321227000000000), 1791321227000000000);
        assert_eq!(ns_after(None, 5), 5);
    }
}
