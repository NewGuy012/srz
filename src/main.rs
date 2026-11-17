#![forbid(unsafe_code)]

use anyhow::Result;
use clap::{CommandFactory, FromArgMatches};
use srz::commands::{dispatch_async, dispatch_sync, print_execution_error};
use srz::config::{Cli, Config};
use srz::ui::Ui;

fn run() -> Result<()> {
    let cmd = Cli::command();
    let matches = cmd.get_matches();
    let mut args = match Cli::from_arg_matches(&matches) {
        Ok(args) => args,
        Err(err) => err.exit(),
    };

    let (config, config_path) = match &args.config {
        Some(p) => (Config::load_from_path(p)?, Some(p.clone())),
        None => Config::load_default()?,
    };
    args.apply_config(&config, Some(&matches))?;

    if args.quiet || args.json {
        Ui::set_quiet(true);
    }
    if args.verbose {
        Ui::set_verbose(true);
    }

    if dispatch_sync(&args, &config, config_path.as_deref())? {
        return Ok(());
    }

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(dispatch_async(args, config, config_path))
}

fn main() {
    if let Err(err) = run() {
        print_execution_error("Command failed", &err);
        std::process::exit(1);
    }
}
