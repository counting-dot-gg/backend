use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::time::Instant;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

const N: usize = 2000;
const REC: usize = 600;

fn report(name: &str, mut v: Vec<f64>) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |p: f64| v[((v.len() as f64 * p) as usize).min(v.len() - 1)];
    println!("{name:<34} p50 {:>7.1}us  p90 {:>7.1}us  p99 {:>7.1}us  max {:>8.1}us", q(0.5), q(0.9), q(0.99), v[v.len() - 1]);
}

fn run(name: &str, mut f: File, sync: bool) {
    let buf = vec![7u8; REC];
    let mut v = Vec::with_capacity(N);
    for _ in 0..N {
        let t = Instant::now();
        f.write_all(&buf).unwrap();
        if sync { f.sync_data().unwrap(); }
        v.push(t.elapsed().as_secs_f64() * 1e6);
    }
    report(name, v);
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or(".".into());
    let p = |n: &str| format!("{dir}/fsb-{n}");
    let _ = std::fs::remove_file(p("a")); let _ = std::fs::remove_file(p("b")); let _ = std::fs::remove_file(p("c")); let _ = std::fs::remove_file(p("d"));
    run("append + sync_data", OpenOptions::new().create(true).append(true).open(p("a")).unwrap(), true);
    {
        let mut f = OpenOptions::new().create(true).read(true).write(true).open(p("b")).unwrap();
        f.write_all(&vec![0u8; N * REC + 4096]).unwrap();
        f.sync_all().unwrap();
        f.seek(SeekFrom::Start(0)).unwrap();
        run("prealloc overwrite + sync_data", f, true);
    }
    #[cfg(windows)]
    {
        let mut f = OpenOptions::new().create(true).read(true).write(true).open(p("c")).unwrap();
        f.write_all(&vec![0u8; N * REC + 4096]).unwrap();
        f.sync_all().unwrap();
        drop(f);
        const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
        let mut f = OpenOptions::new().read(true).write(true).custom_flags(FILE_FLAG_WRITE_THROUGH).open(p("c")).unwrap();
        f.seek(SeekFrom::Start(0)).unwrap();
        run("prealloc + WRITE_THROUGH", f, false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = OpenOptions::new().create(true).read(true).write(true).open(p("d")).unwrap();
        f.write_all(&vec![0u8; N * 4096 + 4096]).unwrap();
        f.sync_all().unwrap();
        drop(f);
        let f = OpenOptions::new().read(true).write(true).custom_flags(libc::O_DSYNC).open(p("d")).unwrap();
        run("prealloc + O_DSYNC", f, false);
        let f = OpenOptions::new().read(true).write(true).custom_flags(libc::O_DIRECT | libc::O_DSYNC).open(p("d")).unwrap();
        let layout = std::alloc::Layout::from_size_align(4096, 4096).unwrap();
        let block = unsafe { std::slice::from_raw_parts_mut(std::alloc::alloc_zeroed(layout), 4096) };
        let mut v = Vec::with_capacity(N);
        let mut off = 0u64; let mut fill = 0usize;
        use std::os::unix::fs::FileExt;
        for _ in 0..N {
            let t = Instant::now();
            if fill + REC > 4096 { off += 4096; fill = 0; block.fill(0); }
            block[fill..fill + REC].fill(7); fill += REC;
            f.write_all_at(block, off).unwrap();
            v.push(t.elapsed().as_secs_f64() * 1e6);
        }
        report("prealloc + O_DIRECT|O_DSYNC 4K", v);
    }
    run("append, no sync", OpenOptions::new().create(true).append(true).open(p("a")).unwrap(), false);
    for n in ["a", "b", "c", "d"] {
        let _ = std::fs::remove_file(p(n));
    }
}
