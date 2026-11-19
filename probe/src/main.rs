use std::process::{Command, ExitCode, Stdio};
use std::time::Instant;

fn stats(name: &str, mut ms: Vec<f64>) {
    ms.sort_by(|a, b| a.total_cmp(b));
    let q = |p: f64| ms[((ms.len() - 1) as f64 * p).round() as usize];
    println!(
        "{{\"case\":{name:?},\"n\":{},\"min_ms\":{:.2},\"p50_ms\":{:.2},\"p90_ms\":{:.2},\"max_ms\":{:.2}}}",
        ms.len(), ms[0], q(0.5), q(0.9), ms[ms.len() - 1]
    );
}

fn read_repo(path: &str) -> Result<(String, bool, usize), Box<dyn std::error::Error>> {
    let repo = gix::open(path)?;
    let head = repo.head_id()?.to_string();
    let dirty = repo.is_dirty()?;
    let worktrees = repo.worktrees()?.len();
    Ok((head, dirty, worktrees))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("noop") => ExitCode::SUCCESS,
        Some("spawn") => {
            let (name, n, cmd) = (&args[1], args[2].parse::<usize>().unwrap(), &args[3..]);
            let mut ms = Vec::with_capacity(n);
            for _ in 0..n {
                let t = Instant::now();
                let status = Command::new(&cmd[0]).args(&cmd[1..])
                    .stdout(Stdio::null()).stderr(Stdio::null()).status()
                    .expect("spawn failed");
                ms.push(t.elapsed().as_secs_f64() * 1e3);
                assert!(status.code().is_some(), "{name} killed");
            }
            stats(name, ms);
            ExitCode::SUCCESS
        }
        Some("gix") => {
            let (path, n) = (&args[1], args[2].parse::<usize>().unwrap());
            let t = Instant::now();
            let (head, dirty, wt) = read_repo(path).expect("gix read failed");
            eprintln!("head={head} dirty={dirty} worktrees={wt} first_ms={:.2}", t.elapsed().as_secs_f64() * 1e3);
            let ms = (0..n).map(|_| {
                let t = Instant::now();
                read_repo(path).expect("gix read failed");
                t.elapsed().as_secs_f64() * 1e3
            }).collect();
            stats("gix_open_head_dirty_worktrees_in_process", ms);
            ExitCode::SUCCESS
        }
        Some("gix-once") => match read_repo(&args[1]) {
            Ok(_) => ExitCode::SUCCESS,
            Err(e) => { eprintln!("{e}"); ExitCode::FAILURE }
        },
        _ => { eprintln!("usage: probe noop|spawn NAME N CMD..|gix REPO N|gix-once REPO"); ExitCode::from(2) }
    }
}
