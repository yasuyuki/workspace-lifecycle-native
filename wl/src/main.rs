mod commands;
mod config;
mod error;
mod finish;
mod model;
mod push;
mod repo;
mod store;
mod trace;

use error::{Error, Result};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage:
  wl [--repo PATH] begin TASK --request URL [--branch NAME] [--target BRANCH] [--base REV]
  wl [--repo PATH] status [TASK] [--finish ID]
  wl [--repo PATH] hold TASK --reason TEXT
  wl [--repo PATH] release-hold TASK --decision REF
  wl [--repo PATH] finish TASK [--retire]
  wl [--repo PATH] retire TASK [--abandon --decision REF]
  wl preflight [REPO] [--user-intent auto|push|hold] [--temporary] [--oss auto|yes|no] [--policy PATH]";

pub struct Args {
    rest: Vec<String>,
}

impl Args {
    fn positional(&mut self, what: &str) -> Result<String> {
        match self.rest.iter().position(|a| !a.starts_with("--")) {
            Some(i) => Ok(self.rest.remove(i)),
            None => Err(Error::new(format!("missing {what}\n{USAGE}"))),
        }
    }

    fn optional_positional(&mut self) -> Option<String> {
        self.rest.iter().position(|a| !a.starts_with("--")).map(|i| self.rest.remove(i))
    }

    fn option(&mut self, name: &str) -> Result<Option<String>> {
        let flag = format!("--{name}");
        match self.rest.iter().position(|a| *a == flag) {
            None => Ok(None),
            Some(i) if i + 1 < self.rest.len() => {
                let value = self.rest.remove(i + 1);
                self.rest.remove(i);
                Ok(Some(value))
            }
            Some(_) => Err(Error::new(format!("{flag} needs a value"))),
        }
    }

    fn flag(&mut self, name: &str) -> bool {
        let flag = format!("--{name}");
        match self.rest.iter().position(|a| *a == flag) {
            Some(i) => {
                self.rest.remove(i);
                true
            }
            None => false,
        }
    }

    fn required(&mut self, name: &str) -> Result<String> {
        self.option(name)?.ok_or_else(|| Error::new(format!("--{name} is required\n{USAGE}")))
    }

    fn finish(self) -> Result<()> {
        if self.rest.is_empty() { Ok(()) } else { Err(Error::new(format!("unexpected arguments: {}\n{USAGE}", self.rest.join(" ")))) }
    }
}

fn run(repo: &std::path::Path, command: &str, args: Args) -> Result<serde_json::Value> {
    let repo = repo.to_path_buf();
    match command {
        "begin" => commands::begin(&repo, args),
        "status" => commands::status(&repo, args),
        "hold" => commands::hold(&repo, args),
        "release-hold" => commands::release_hold(&repo, args),
        "retire" => commands::retire(&repo, args),
        "finish" => finish::finish(&repo, args),
        "__work" => finish::work(&repo, args),
        "__prepare-slot" => commands::prepare_slot(&repo, args),
        "--help" | "-h" | "help" => {
            println!("{USAGE}");
            std::process::exit(0)
        }
        other => Err(Error::new(format!("unknown command {other:?}\n{USAGE}"))),
    }
}

fn main() -> ExitCode {
    trace::init();
    repo::prepare_environment();
    trace::point("environment");
    let mut args = Args { rest: std::env::args().skip(1).collect() };
    let parsed = args.option("repo").and_then(|repo| {
        Ok((repo.map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")), args.positional("command")?))
    });
    if let Ok((repo, command)) = &parsed {
        if command == "preflight" {
            // Keeps the legacy preflight's output and exit status: push 0, hold 10, ask 11, usage 2.
            return push::cli(repo, args);
        }
    }
    let result = parsed.and_then(|(repo, command)| run(&repo, &command, args));
    trace::point("done");
    match result {
        Ok(mut value) => {
            value["ok"] = true.into();
            let notices = finish::take_notices();
            if !notices.is_empty() {
                value["notices"] = serde_json::to_value(notices).expect("notices serialize");
            }
            println!("{value}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            let mut value = serde_json::json!({ "ok": false, "error": error.to_string() });
            let notices = finish::take_notices();
            if !notices.is_empty() {
                value["notices"] = serde_json::to_value(notices).expect("notices serialize");
            }
            println!("{value}");
            ExitCode::from(2)
        }
    }
}
