mod args;
mod osc;
mod state;
mod terminal;
mod tool;
mod wrapper;

use std::process;

use args::{ParseOutcome, parse_env_args};

fn main() {
    let exit_code = match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("agent-status-cli: {err}");
            2
        }
    };
    process::exit(exit_code);
}

fn run() -> anyhow::Result<i32> {
    match parse_env_args().map_err(anyhow::Error::msg)? {
        ParseOutcome::Help(help) => {
            println!("{help}");
            Ok(0)
        }
        ParseOutcome::Run(args) => wrapper::run(args),
    }
}
