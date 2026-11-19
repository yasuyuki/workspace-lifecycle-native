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
    read_with(gix::open(path)?)
}

fn read_repo_no_git_binary(path: &str) -> Result<(String, bool, usize), Box<dyn std::error::Error>> {
    let mut permissions = gix::open::Permissions::default();
    permissions.config.git_binary = false;
    read_with(gix::open_opts(path, gix::open::Options::default().permissions(permissions))?)
}

fn read_with(repo: gix::Repository) -> Result<(String, bool, usize), Box<dyn std::error::Error>> {
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
            for i in 0..n {
                let argv: Vec<String> = cmd.iter().map(|a| a.replace("{i}", &i.to_string())).collect();
                let t = Instant::now();
                let status = Command::new(&argv[0]).args(&argv[1..])
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
        Some("gix-once-no-git-binary") => match read_repo_no_git_binary(&args[1]) {
            Ok(_) => ExitCode::SUCCESS,
            Err(e) => { eprintln!("{e}"); ExitCode::FAILURE }
        },
        Some("gix-phases") => {
            let isolated = args.get(2).map(String::as_str) == Some("isolated");
            for round in 0..2 {
                let t = Instant::now();
                let repo = if isolated {
                    gix::open_opts(&args[1], gix::open::Options::isolated()).unwrap()
                } else {
                    gix::open(&args[1]).unwrap()
                };
                let open = t.elapsed().as_secs_f64() * 1e3;
                let t = Instant::now();
                repo.head_id().unwrap();
                let head = t.elapsed().as_secs_f64() * 1e3;
                let t = Instant::now();
                repo.is_dirty().unwrap();
                let dirty = t.elapsed().as_secs_f64() * 1e3;
                let t = Instant::now();
                repo.worktrees().unwrap();
                let wt = t.elapsed().as_secs_f64() * 1e3;
                println!("{{\"case\":\"gix_phases\",\"isolated\":{isolated},\"round\":{round},\"open_ms\":{open:.2},\"head_ms\":{head:.2},\"dirty_ms\":{dirty:.2},\"worktrees_ms\":{wt:.2}}}");
            }
            ExitCode::SUCCESS
        }
        Some("gix-perm") => {
            let on: Vec<&str> = args[2].split(',').collect();
            let mut permissions = gix::open::Permissions::default();
            permissions.config = gix::open::permissions::Config {
                git_binary: on.contains(&"git_binary"),
                system: on.contains(&"system"),
                git: on.contains(&"git"),
                user: on.contains(&"user"),
                env: on.contains(&"env"),
                includes: on.contains(&"includes"),
            };
            let t = Instant::now();
            let repo = gix::open_opts(&args[1], gix::open::Options::default().permissions(permissions)).unwrap();
            let open = t.elapsed().as_secs_f64() * 1e3;
            let t = Instant::now();
            repo.is_dirty().unwrap();
            let dirty = t.elapsed().as_secs_f64() * 1e3;
            let autocrlf = repo.config_snapshot().string("core.autocrlf").map(|v| v.to_string());
            println!("{{\"case\":\"gix_perm\",\"on\":{:?},\"open_ms\":{open:.2},\"dirty_ms\":{dirty:.2},\"autocrlf\":{:?}}}", args[2], format!("{autocrlf:?}"));
            ExitCode::SUCCESS
        }
        Some("gix-once") => match read_repo(&args[1]) {
            Ok(_) => ExitCode::SUCCESS,
            Err(e) => { eprintln!("{e}"); ExitCode::FAILURE }
        },
        _ => { eprintln!("usage: probe noop|spawn NAME N CMD..|gix REPO N|gix-once REPO"); ExitCode::from(2) }
    }
}
