const CONTEXT: usize = 4;
const NO_NEWLINE: &str = "\\ No newline at end of file";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Same,
    Removed,
    Added,
}

fn split_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> = text.split('\n').map(|l| format!("{l}\n")).collect();
    let last = out.pop().unwrap_or_default();
    if !text.ends_with('\n') {
        out.push(last[..last.len() - 1].to_owned());
    }
    out
}

fn myers(a: &[String], b: &[String]) -> Vec<(Op, usize)> {
    let (n, m) = (a.len(), b.len());
    let max = n + m;
    let off = max as isize;
    let mut v = vec![0isize; 2 * max + 2];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    'search: for d in 0..=max as isize {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let idx = (k + off) as usize;
            let mut x = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) {
                v[idx + 1]
            } else {
                v[idx - 1] + 1
            };
            let mut y = x - k;
            while (x as usize) < n && (y as usize) < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x as usize >= n && y as usize >= m {
                trace.push(v.clone());
                break 'search;
            }
            k += 2;
        }
    }
    let mut ops = Vec::new();
    let (mut x, mut y) = (n as isize, m as isize);
    for d in (1..trace.len() as isize - 1).rev() {
        let v = &trace[d as usize];
        let k = x - y;
        let idx = (k + off) as usize;
        let prev_k = if k == -d || (k != d && v[idx - 1] < v[idx + 1]) {
            k + 1
        } else {
            k - 1
        };
        let prev_x = v[(prev_k + off) as usize];
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            ops.push(Op::Same);
            x -= 1;
            y -= 1;
        }
        ops.push(if x == prev_x { Op::Added } else { Op::Removed });
        x = prev_x;
        y = prev_y;
    }
    while x > 0 && y > 0 {
        ops.push(Op::Same);
        x -= 1;
        y -= 1;
    }
    ops.reverse();
    let mut runs: Vec<(Op, usize)> = Vec::new();
    for op in ops {
        match runs.last_mut() {
            Some((o, n)) if *o == op => *n += 1,
            _ => runs.push((op, 1)),
        }
    }
    let mut fixed: Vec<(Op, usize)> = Vec::new();
    let mut i = 0;
    while i < runs.len() {
        if runs[i].0 == Op::Added && runs.get(i + 1).is_some_and(|r| r.0 == Op::Removed) {
            fixed.push(runs[i + 1]);
            fixed.push(runs[i]);
            i += 2;
        } else {
            fixed.push(runs[i]);
            i += 1;
        }
    }
    fixed
}

struct Change {
    op: Op,
    lines: Vec<String>,
}

fn changes(a: &[String], b: &[String]) -> Vec<Change> {
    let (mut ia, mut ib) = (0, 0);
    myers(a, b)
        .into_iter()
        .map(|(op, n)| {
            let lines = match op {
                Op::Same | Op::Removed => {
                    let l = a[ia..ia + n].to_vec();
                    ia += n;
                    if op == Op::Same {
                        ib += n;
                    }
                    l
                }
                Op::Added => {
                    let l = b[ib..ib + n].to_vec();
                    ib += n;
                    l
                }
            };
            Change { op, lines }
        })
        .collect()
}

struct Hunk {
    old_start: usize,
    old_lines: usize,
    new_start: usize,
    new_lines: usize,
    lines: Vec<String>,
}

fn hunks(mut diff: Vec<Change>) -> Vec<Hunk> {
    diff.push(Change {
        op: Op::Same,
        lines: Vec::new(),
    });
    let ctx = |ls: &[String]| ls.iter().map(|l| format!(" {l}")).collect::<Vec<_>>();
    let mut out = Vec::new();
    let (mut old_range, mut new_range) = (0usize, 0usize);
    let mut cur: Vec<String> = Vec::new();
    let (mut old_line, mut new_line) = (1usize, 1usize);
    for i in 0..diff.len() {
        let lines = &diff[i].lines;
        if diff[i].op != Op::Same {
            if old_range == 0 {
                old_range = old_line;
                new_range = new_line;
                if i > 0 {
                    let prev = &diff[i - 1].lines;
                    cur = ctx(&prev[prev.len().saturating_sub(CONTEXT)..]);
                    old_range -= cur.len();
                    new_range -= cur.len();
                }
            }
            let sign = if diff[i].op == Op::Added { '+' } else { '-' };
            cur.extend(lines.iter().map(|l| format!("{sign}{l}")));
            if diff[i].op == Op::Added {
                new_line += lines.len();
            } else {
                old_line += lines.len();
            }
        } else {
            if old_range != 0 {
                if lines.len() <= CONTEXT * 2 && i + 2 < diff.len() {
                    cur.extend(ctx(lines));
                } else {
                    let size = lines.len().min(CONTEXT);
                    cur.extend(ctx(&lines[..size]));
                    out.push(Hunk {
                        old_start: old_range,
                        old_lines: old_line - old_range + size,
                        new_start: new_range,
                        new_lines: new_line - new_range + size,
                        lines: std::mem::take(&mut cur),
                    });
                    old_range = 0;
                    new_range = 0;
                }
            }
            old_line += lines.len();
            new_line += lines.len();
        }
    }
    for h in &mut out {
        let mut fixed = Vec::with_capacity(h.lines.len());
        for l in h.lines.drain(..) {
            match l.strip_suffix('\n') {
                Some(t) => fixed.push(t.to_owned()),
                None => {
                    fixed.push(l);
                    fixed.push(NO_NEWLINE.to_owned());
                }
            }
        }
        h.lines = fixed;
    }
    out
}

pub fn two_files_patch(
    old_name: &str,
    new_name: &str,
    old: &str,
    new: &str,
    old_header: &str,
    new_header: &str,
) -> String {
    let diff = changes(&split_lines(old), &split_lines(new));
    let mut out = Vec::new();
    if old_name == new_name {
        out.push(format!("Index: {old_name}"));
    }
    out.push("===================================================================".to_owned());
    out.push(format!("--- {old_name}\t{old_header}"));
    out.push(format!("+++ {new_name}\t{new_header}"));
    for h in hunks(diff) {
        let old_start = if h.old_lines == 0 { h.old_start - 1 } else { h.old_start };
        let new_start = if h.new_lines == 0 { h.new_start - 1 } else { h.new_start };
        out.push(format!(
            "@@ -{old_start},{} +{new_start},{} @@",
            h.old_lines, h.new_lines
        ));
        out.extend(h.lines);
    }
    out.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    const NODE_OUTPUTS: &str = include_str!("unified_diff_cases.json");

    #[test]
    fn matches_the_node_diff_package_byte_for_byte() {
        let cases: Vec<(String, String, String)> = serde_json::from_str(NODE_OUTPUTS).unwrap();
        for (old, new, want) in cases {
            assert_eq!(
                two_files_patch("old.txt", "new.txt", &old, &new, "old", "new"),
                want,
                "{old:?} -> {new:?}"
            );
        }
    }
}
