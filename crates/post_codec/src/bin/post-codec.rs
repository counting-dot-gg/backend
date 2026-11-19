use post_codec::{Context, Flag, decode, encode_checked, tsv};
use std::collections::BTreeMap;
use std::io::{BufRead, BufWriter, Write};

#[derive(Default)]
struct Report {
    rows: u64,
    mismatches: u64,
    shown: u32,
    flags: BTreeMap<String, u64>,
    field_mismatches: BTreeMap<&'static str, u64>,
    repairs: BTreeMap<&'static str, u64>,
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let emit = args.iter().any(|a| a == "--emit");
    let mut repairs = match args.iter().position(|a| a == "--repairs") {
        Some(i) => Some(BufWriter::new(std::fs::File::create(&args[i + 1])?)),
        None => None,
    };
    let stdin = std::io::stdin().lock();
    let mut out = BufWriter::with_capacity(1 << 20, std::io::stdout().lock());
    let mut report = Report::default();
    let mut ctx = Context::default();
    let mut thread = None;
    let mut buf = String::new();
    for line in stdin.lines() {
        let line = line?;
        let line = line.strip_suffix('\r').unwrap_or(&line);
        let plain = match tsv::parse_plain(line) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("unparsable line: {}", e.0);
                std::process::exit(2);
            }
        };
        if thread != Some(plain.thread_id) {
            ctx = Context::default();
            thread = Some(plain.thread_id);
        }
        let checked = encode_checked(&ctx, &plain);
        let row = checked.row;
        report.rows += 1;
        for r in &checked.repairs {
            *report.repairs.entry(r.field).or_default() += 1;
            if let Some(w) = repairs.as_mut() {
                let (old, new) = (r.old.as_deref().unwrap_or(""), r.new.as_deref().unwrap_or(""));
                writeln!(w, "{}	{}	{old}	{new}", r.id, r.field)?;
            }
        }
        if !checked.mismatches.is_empty() {
            report.mismatches += 1;
            for f in &checked.mismatches {
                *report.field_mismatches.entry(f).or_default() += 1;
            }
            if report.shown < 10 {
                report.shown += 1;
                eprintln!(
                    "mismatch id {} fields {:?}
  original {plain:?}
  decoded  {:?}",
                    plain.id,
                    checked.mismatches,
                    decode(&ctx, &row)
                );
            }
        }
        for f in Flag::ALL {
            if row.has(f) {
                *report.flags.entry(format!("{f:?}")).or_default() += 1;
            }
        }
        if emit {
            buf.clear();
            tsv::write_row(&row, &mut buf);
            buf.push('\n');
            out.write_all(buf.as_bytes())?;
        }
        ctx.advance(&plain);
    }
    out.flush()?;
    if let Some(w) = repairs.as_mut() {
        w.flush()?;
    }
    eprintln!("rows {} mismatches {}", report.rows, report.mismatches);
    for (f, n) in &report.repairs {
        eprintln!("  repaired {f}: {n}");
    }
    for (f, n) in &report.field_mismatches {
        eprintln!("  mismatch {f}: {n}");
    }
    for (f, n) in &report.flags {
        eprintln!(
            "  flag {f}: {n} ({:.3}%)",
            *n as f64 * 100.0 / report.rows.max(1) as f64
        );
    }
    if report.mismatches > 0 {
        std::process::exit(1);
    }
    Ok(())
}
