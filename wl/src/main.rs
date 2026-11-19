mod commands;
mod error;
mod model;
mod repo;
mod store;

use error::{Error, Result};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage:
  wl [--repo PATH] begin TASK --request URL [--branch NAME] [--base REV]
  wl [--repo PATH] status [TASK]
  wl [--repo PATH] hold TASK --reason TEXT
  wl [--repo PATH] release-hold TASK --decision REF";

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

    fn required(&mut self, name: &str) -> Result<String> {
        self.option(name)?.ok_or_else(|| Error::new(format!("--{name} is required\n{USAGE}")))
    }

    fn finish(self) -> Result<()> {
        if self.rest.is_empty() { Ok(()) } else { Err(Error::new(format!("unexpected arguments: {}\n{USAGE}", self.rest.join(" ")))) }
    }
}

fn run() -> Result<serde_json::Value> {
    let mut args = Args { rest: std::env::args().skip(1).collect() };
    let repo = args.option("repo")?.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let command = args.positional("command")?;
    match command.as_str() {
        "begin" => commands::begin(&repo, args),
        "status" => commands::status(&repo, args),
        "hold" => commands::hold(&repo, args),
        "release-hold" => commands::release_hold(&repo, args),
        "__prepare-slot" => commands::prepare_slot(&repo, args),
        "--help" | "-h" | "help" => {
            println!("{USAGE}");
            std::process::exit(0)
        }
        other => Err(Error::new(format!("unknown command {other:?}\n{USAGE}"))),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(mut value) => {
            value["ok"] = true.into();
            println!("{value}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            println!("{}", serde_json::json!({ "ok": false, "error": error.to_string() }));
            ExitCode::from(2)
        }
    }
}
