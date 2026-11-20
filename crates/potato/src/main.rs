mod build;
mod commands;
mod config;
mod ctx;
mod fetch;
mod git;
mod hashcache;
mod index;
mod java_mirror;
mod launcher;
mod modrinth;
mod pack;
mod progress;
mod publish;
mod storage;
mod sync;
mod target;
mod upload;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use config::TargetType;
use log::LevelFilter;

pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(format!("potato-launcher/{VERSION}"))
        .build()
        .expect("http client")
}

const VERSION: &str = match launcher_build_config::version() {
    Some(version) => version,
    None => "dev",
};

#[derive(Parser)]
#[command(name = "potato", about = "Pack repos for Potato Launcher", version = VERSION)]
struct Cli {
    /// Pack repo to work in.
    #[arg(short = 'C', long = "repo", default_value = ".", global = true)]
    repo: PathBuf,
    /// List every uploaded file.
    #[arg(short, long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create potato.toml in a new or existing repo.
    Init {
        #[arg(long)]
        id: Option<String>,
        /// Prism instance folder to sync with.
        #[arg(long)]
        prism: Option<PathBuf>,
    },
    /// Bring changes from a Prism target into the repo.
    Pull {
        target: String,
        /// Make the repo match Prism exactly.
        #[arg(long)]
        force: bool,
    },
    /// Write the committed repo to a target.
    Push {
        target: String,
        /// Skip the checks and overwrite the target.
        #[arg(long)]
        force: bool,
    },
    /// Show what pull and push would change for each target.
    Status,
    /// Manage the Prism instances and servers to sync with.
    Target {
        #[command(subcommand)]
        action: TargetAction,
    },
    /// Add and inspect mods and packs.
    Mod {
        #[command(subcommand)]
        action: ModAction,
    },
    /// Build the last commit without publishing.
    Build,
    /// Build the last commit and publish it.
    Publish {
        /// Skip the checks.
        #[arg(long)]
        force: bool,
    },
    /// Remove this instance from the instance list.
    Unpublish,
    /// Delete stored files no published version needs.
    Gc {
        /// Hours to keep a replaced version's files.
        #[arg(long, default_value_t = publish::DEFAULT_GRACE_HOURS)]
        grace_hours: u64,
    },
    /// Upload launcher builds and regenerate the downloads page.
    PublishLauncher {
        /// Site root, e.g. rsync://user@host/srv/potato/site
        #[arg(long)]
        target: String,
        #[arg(long)]
        name: String,
        #[arg(long)]
        version: String,
        /// Image shown on the downloads page.
        #[arg(long)]
        icon: Option<PathBuf>,
        /// Repository URL for the downloads page.
        #[arg(long)]
        source: Option<String>,
        files: Vec<PathBuf>,
    },
    /// Manage the download and build cache.
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
}

#[derive(Subcommand)]
enum TargetAction {
    /// Add or change a target.
    ///
    /// Without --prism or --server, a folder with mmc-pack.json is a Prism instance.
    #[command(group(clap::ArgGroup::new("kind")))]
    Add {
        name: String,
        path: PathBuf,
        /// A Prism instance folder.
        #[arg(long, group = "kind")]
        prism: bool,
        /// A server folder.
        #[arg(long, group = "kind")]
        server: bool,
    },
    /// List targets.
    List,
    /// Forget a target, leaving its folder as it is.
    Remove { name: String },
}

#[derive(Subcommand)]
enum ModAction {
    /// Add a mod or pack.
    Add {
        /// A file, a URL or `modrinth:<slug>[@version]`.
        source: String,
        /// Only for these OSes (windows, macos, linux).
        #[arg(long)]
        os: Vec<String>,
        /// client, server or both.
        #[arg(long)]
        side: Option<String>,
        #[arg(long, default_value = "mods")]
        dir: String,
        /// Index file name, without .pw.toml.
        #[arg(long)]
        name: Option<String>,
    },
    /// Remove a mod or pack, with its committed file.
    Remove {
        /// Index name, as `mod list` shows it.
        name: String,
    },
    /// List mods and packs in the repo.
    List,
    /// Print the mod id of a jar or `modrinth:<slug>`.
    Id { source: String },
    /// Change which OSes or side a mod or pack is for.
    Set {
        /// Index name, as `mod list` shows it.
        name: String,
        /// windows, macos, linux or all; repeat for several.
        #[arg(long)]
        os: Vec<String>,
        /// client, server or both.
        #[arg(long)]
        side: Option<String>,
    },
}

#[derive(Subcommand)]
enum CacheAction {
    /// Delete the cache.
    Clean,
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    if let Command::Init { id, prism } = cli.command {
        return commands::init(&cli.repo, id, prism);
    }
    if let Command::Target { action } = &cli.command {
        return match action {
            TargetAction::Add {
                name,
                path,
                prism,
                server,
            } => {
                let kind = match (prism, server) {
                    (true, _) => Some(TargetType::Prism),
                    (_, true) => Some(TargetType::Server),
                    _ => None,
                };
                commands::target_add(&cli.repo, name, kind, path)
            }
            TargetAction::List => commands::target_list(&cli.repo),
            TargetAction::Remove { name } => commands::target_remove(&cli.repo, name),
        };
    }
    if let Command::PublishLauncher {
        target,
        name,
        version,
        icon,
        source,
        files,
    } = &cli.command
    {
        let release = launcher::LauncherRelease {
            name,
            version,
            files,
            icon: icon.as_deref(),
            source: source.as_deref(),
        };
        return launcher::publish_launcher(target, release).await;
    }
    let mut ctx = ctx::Ctx::new(&cli.repo)?;
    ctx.verbose = cli.verbose;
    utils::logging::setup_logger(&ctx.cache_dir.join("potato.log"), true, LevelFilter::Warn)?;
    match cli.command {
        Command::Init { .. } | Command::Target { .. } | Command::PublishLauncher { .. } => {
            unreachable!()
        }
        Command::Pull { target, force } => sync::pull(&mut ctx, &target, force).await,
        Command::Push { target, force } => sync::push(&mut ctx, &target, force).await,
        Command::Status => sync::status(&mut ctx).await,
        Command::Mod {
            action:
                ModAction::Add {
                    source,
                    os,
                    side,
                    dir,
                    name,
                },
        } => {
            commands::add(
                &mut ctx,
                &source,
                commands::AddOptions {
                    os,
                    side,
                    dir,
                    name,
                },
            )
            .await
        }
        Command::Mod {
            action: ModAction::Remove { name },
        } => commands::mod_remove(&ctx, &name),
        Command::Mod {
            action: ModAction::List,
        } => commands::mods(&mut ctx).await,
        Command::Mod {
            action: ModAction::Id { source },
        } => commands::mod_id(&mut ctx, &source).await,
        Command::Mod {
            action: ModAction::Set { name, os, side },
        } => commands::mod_set(&ctx, &name, &os, side.as_deref()),
        Command::Build => {
            let head = ctx.git.head()?;
            let output = Box::pin(build::build(&mut ctx, &head)).await?;
            println!(
                "built {} ({} files to upload), see .potato/build/",
                output.config.id,
                output.uploads.len()
            );
            Ok(())
        }
        Command::Publish { force } => publish::publish(&mut ctx, force).await,
        Command::Unpublish => publish::unpublish(&mut ctx).await,
        Command::Gc { grace_hours } => publish::gc(&mut ctx, grace_hours).await,
        Command::Cache {
            action: CacheAction::Clean,
        } => commands::cache_clean(&ctx),
    }
}

/// The generator's futures are large in debug builds.
pub fn block_on_big_stack<T: Send + 'static>(job: impl Future<Output = T> + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(64 << 20)
                .build()
                .expect("tokio runtime")
                .block_on(job)
        })
        .expect("main thread")
        .join()
        .expect("main thread panicked")
}

fn main() {
    let cli = Cli::parse();
    if let Err(err) = block_on_big_stack(run(cli)) {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
