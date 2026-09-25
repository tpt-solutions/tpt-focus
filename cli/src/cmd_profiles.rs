//! `tpt-focus profiles` — manage focus profiles in the TOML configuration.

use anyhow::{Context as _, Result};
use clap::{Subcommand, ValueEnum};

use crate::paths;
use crate::print;
use crate::Ctx;

use tpt_focus_core::Decision;

#[derive(Subcommand)]
pub enum ProfilesArgs {
    /// List configured focus profiles.
    List {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Add or update a focus profile.
    Add {
        /// Profile name (matched case-insensitively by rules).
        name: String,
        /// Free-text description.
        #[arg(short, long)]
        description: Option<String>,
        /// Decision applied while active when no rule matches.
        #[arg(long, value_enum)]
        default: Option<DefaultArg>,
    },
    /// Remove a focus profile.
    Remove { name: String },
}

#[derive(Copy, Clone, ValueEnum)]
pub enum DefaultArg {
    Allow,
    Mute,
    Batch,
}

impl From<DefaultArg> for Decision {
    fn from(value: DefaultArg) -> Self {
        match value {
            DefaultArg::Allow => Decision::Allow,
            DefaultArg::Mute => Decision::Mute,
            DefaultArg::Batch => Decision::Batch,
        }
    }
}

pub fn run(ctx: &Ctx, args: ProfilesArgs) -> Result<()> {
    let path = paths::config_path(ctx.config.as_deref());
    let mut config = load_or_default(&path)?;

    match args {
        ProfilesArgs::List { json } => {
            config.validate()?;
            if json {
                print::print_json(&config.profiles)?;
            } else if config.profiles.is_empty() {
                println!("no focus profiles configured ({})", path.display());
            } else {
                for profile in &config.profiles {
                    print::print_profile(profile);
                }
            }
        }

        ProfilesArgs::Add {
            name,
            description,
            default,
        } => {
            let mut profile = tpt_focus_core::FocusProfile::new(&name);
            if let Some(description) = description {
                profile.description = Some(description);
            }
            if let Some(default) = default {
                profile.default_decision = Some(default.into());
            }

            match config
                .profiles
                .iter_mut()
                .find(|p| p.name.eq_ignore_ascii_case(&name))
            {
                Some(existing) => {
                    *existing = profile;
                    println!("profile `{name}` updated");
                }
                None => {
                    config.profiles.push(profile);
                    println!("profile `{name}` added");
                }
            }

            paths::ensure_parent(&path)?;
            paths::ensure_parent(&path)?;
            config.save(&path).context("failed to save config")?;
        }

        ProfilesArgs::Remove { name } => {
            let before = config.profiles.len();
            config
                .profiles
                .retain(|profile| !profile.name.eq_ignore_ascii_case(&name));
            if config.profiles.len() == before {
                anyhow::bail!("no profile named `{name}`");
            }

            // Rules referencing the removed profile would fail validation.
            let referenced: Vec<String> = config
                .rules
                .iter()
                .filter(|rule| {
                    rule.conditions.iter().any(|condition| {
                        matches!(condition, tpt_focus_core::Condition::Profile { profiles }
                            if profiles.iter().any(|p| p.eq_ignore_ascii_case(&name)))
                    })
                })
                .map(|rule| rule.id.clone())
                .collect();

            paths::ensure_parent(&path)?;
            paths::ensure_parent(&path)?;
            config.save(&path).context("failed to save config")?;
            println!("profile `{name}` removed");
            if !referenced.is_empty() {
                println!(
                    "warning: rules {} still reference `{name}` and will fail validation",
                    referenced.join(", ")
                );
            }
        }
    }
    Ok(())
}

fn load_or_default(path: &std::path::Path) -> Result<tpt_focus_core::Config> {
    if path.exists() {
        tpt_focus_core::Config::load(path)
            .with_context(|| format!("failed to load {}", path.display()))
    } else {
        Ok(tpt_focus_core::Config::default())
    }
}
