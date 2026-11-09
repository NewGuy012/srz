#![forbid(unsafe_code)]

use anyhow::Result;
use clap::{CommandFactory, FromArgMatches};
use srz::cli::Cli;
use srz::commands::dispatch_cli;
use srz::config::Config;
use srz::ui::Ui;

#[tokio::main]
async fn main() -> Result<()> {
    let cmd = Cli::command();
    let matches = cmd.get_matches();
    let mut args = match Cli::from_arg_matches(&matches) {
        Ok(args) => args,
        Err(err) => err.exit(),
    };

    let (config, config_path) = match &args.config {
        Some(p) => {
            let cfg = Config::load_from_path(p)?;
            (cfg, Some(p.clone()))
        }
        None => Config::load_default(),
    };
    args.apply_config(&config, Some(&matches));

    if args.quiet || args.json {
        Ui::set_quiet(true);
    }
    if args.verbose {
        Ui::set_verbose(true);
    }

    dispatch_cli(args, config, config_path).await
}
