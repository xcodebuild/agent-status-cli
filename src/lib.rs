mod args;
mod osc;
mod state;
mod terminal;
mod tool;
mod wrapper;

use std::ffi::OsString;
use std::process;

use args::{ParseOutcome, parse_args, parse_env_args};

pub fn main_entry() {
    let exit_code = match run_from_env() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("agent-status-cli: {err}");
            2
        }
    };
    process::exit(exit_code);
}

pub fn run_from_env() -> anyhow::Result<i32> {
    match parse_env_args().map_err(anyhow::Error::msg)? {
        ParseOutcome::Help(help) => {
            println!("{help}");
            Ok(0)
        }
        ParseOutcome::Run(args) => wrapper::run(args),
    }
}

#[allow(dead_code)]
pub fn run_from_args<I>(iter: I) -> anyhow::Result<i32>
where
    I: IntoIterator<Item = OsString>,
{
    match parse_args(iter).map_err(anyhow::Error::msg)? {
        ParseOutcome::Help(help) => {
            println!("{help}");
            Ok(0)
        }
        ParseOutcome::Run(args) => wrapper::run(args),
    }
}
