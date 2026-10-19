use std::path::PathBuf;

use clap::Parser;

/// Watch a Rust project. Rebuild and restart it when a file changes.
///
/// Type `rs` and Enter to restart manually.
#[derive(Parser, Debug)]
#[command(name = "rwatch", version)]
pub struct Cli {
    /// Run this shell command instead of `cargo build` and the binary.
    /// Example: -x "cargo test"
    #[arg(short = 'x', long, value_name = "COMMAND")]
    pub exec: Option<String>,

    /// The package to build (`cargo build -p`).
    #[arg(short, long, conflicts_with = "exec")]
    pub package: Option<String>,

    /// The binary to build and run (`cargo build --bin`).
    #[arg(long, conflicts_with_all = ["exec", "example"])]
    pub bin: Option<String>,

    /// The example to build and run (`cargo build --example`).
    #[arg(long, conflicts_with = "exec")]
    pub example: Option<String>,

    /// Build with the release profile.
    #[arg(short, long, conflicts_with = "exec")]
    pub release: bool,

    /// The features to turn on (`cargo build --features`).
    #[arg(short = 'F', long, conflicts_with = "exec")]
    pub features: Option<String>,

    /// The paths to watch. The default is the current directory.
    #[arg(short, long = "watch", value_name = "PATH")]
    pub watch: Vec<PathBuf>,

    /// The file extensions to watch. An empty value watches all files.
    #[arg(short, long, value_delimiter = ',', default_value = "rs,toml")]
    pub ext: Vec<String>,

    /// Ignore paths that match this gitignore pattern. You can use it more than one time.
    #[arg(short, long = "ignore", value_name = "PATTERN")]
    pub ignore: Vec<String>,

    /// The time in milliseconds to wait for more changes before a restart.
    #[arg(short, long, default_value_t = 100, value_name = "MS")]
    pub delay: u64,

    /// The time in milliseconds that the app has to stop after SIGTERM, before SIGKILL.
    #[arg(long, default_value_t = 3000, value_name = "MS")]
    pub grace: u64,

    /// Clear the screen before each restart.
    #[arg(short, long)]
    pub clear: bool,

    /// The arguments for the app. Put them after `--`.
    #[arg(last = true, value_name = "ARGS")]
    pub args: Vec<String>,
}

impl Cli {
    /// The arguments that go after `cargo build`.
    pub fn cargo_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(package) = &self.package {
            args.extend(["-p".into(), package.clone()]);
        }
        if let Some(bin) = &self.bin {
            args.extend(["--bin".into(), bin.clone()]);
        }
        if let Some(example) = &self.example {
            args.extend(["--example".into(), example.clone()]);
        }
        if self.release {
            args.push("--release".into());
        }
        if let Some(features) = &self.features {
            args.extend(["--features".into(), features.clone()]);
        }
        args
    }
}
