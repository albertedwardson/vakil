use std::path::PathBuf;

use clap::{Parser, Subcommand};
use log::info;
use vakil_runtime::{Runtime, RuntimeConfig};

#[derive(Parser)]
#[command(name = "vakil-cli", version, about = "Vakil runtime CLI runner")]
struct Cli {
    /// Path to config file (overrides env)
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Increase log verbosity (-v, -vv)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the runtime (default)
    Run,
}

#[tokio::main]
async fn main() -> anyhow::Result<(), anyhow::Error> {
    let Cli {
        config,
        verbose,
        command,
    } = Cli::parse();

    let log_level = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };

    let _ =
        env_logger::Builder::from_env(env_logger::Env::default().filter_or("RUST_LOG", log_level))
            .format_timestamp_millis()
            .try_init();

    if let Some(path) = config.as_ref() {
        unsafe {
            std::env::set_var("VAKIL_CONFIG", path.to_string_lossy().to_string());
        }
    }

    info!(
        "starting vakil-cli with verbosity={} config={}",
        verbose,
        config
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<env>".to_string())
    );

    match command.unwrap_or(Commands::Run) {
        Commands::Run => {
            let config = RuntimeConfig::from_env()?;
            let runtime = Runtime::build(config)?;
            runtime.run().await?;
            Ok(())
        }
    }
}
