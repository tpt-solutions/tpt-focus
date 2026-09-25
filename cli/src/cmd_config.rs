//! `tpt-focus config` — inspect, create or validate the TOML configuration.

use std::path::PathBuf;

use anyhow::{Context as _, Result};
use clap::Subcommand;

use crate::paths;
use crate::Ctx;

#[derive(Subcommand)]
pub enum ConfigArgs {
    /// Write the starter configuration if no config file exists yet.
    Init {
        /// Overwrite an existing configuration file.
        #[arg(long)]
        force: bool,
    },
    /// Print the configuration file path.
    Path,
    /// Print the effective configuration as TOML.
    Show,
    /// Validate a configuration file (default: the active one).
    Validate {
        /// Alternative file to validate instead of the active configuration.
        path: Option<PathBuf>,
    },
}

pub fn run(ctx: &Ctx, args: ConfigArgs) -> Result<()> {
    let path = paths::config_path(ctx.config.as_deref());
    match args {
        ConfigArgs::Path => {
            println!("{}", path.display());
        }
        ConfigArgs::Init { force } => {
            if path.exists() && !force {
                anyhow::bail!(
                    "{} already exists (use --force to overwrite)",
                    path.display()
                );
            }
            paths::ensure_parent(&path)?;
            let starter = tpt_focus_core::Config::starter();
            starter.save(&path).context("failed to write config")?;
            println!("starter configuration written to {}", path.display());
        }
        ConfigArgs::Show => {
            let config = tpt_focus_core::Config::load(&path)
                .with_context(|| format!("failed to load {}", path.display()))?;
            print!("{}", config.to_toml()?);
        }
        ConfigArgs::Validate { path: alt } => {
            let target = alt.unwrap_or_else(|| path.clone());
            let config = tpt_focus_core::Config::load(&target)
                .with_context(|| format!("failed to load {}", target.display()))?;
            config.validate()?;
            println!(
                "{} is valid: {} rules, {} profiles",
                target.display(),
                config.rules.len(),
                config.profiles.len()
            );
        }
    }
    Ok(())
}
