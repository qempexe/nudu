mod scan;
mod units;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use scan::{Kind, ScanOptions};
use units::human_size;

/// nudu - disk usage analyzer in the spirit of ncdu
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Directory to scan (defaults to the current directory)
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Also descend into other mounted filesystems
    #[arg(long)]
    cross_fs: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let opts = ScanOptions {
        one_filesystem: !args.cross_fs,
    };

    let root = match scan::scan(&args.path, opts) {
        Ok(root) => root,
        Err(e) => {
            eprintln!("nudu: {}: {e}", args.path.display());
            return ExitCode::FAILURE;
        }
    };

    // Temporary text output until the TUI lands.
    println!(
        "{}  total {}  ({} items)",
        args.path.display(),
        human_size(root.disk),
        root.items
    );
    for child in root.children.iter().take(15) {
        let slash = if child.kind == Kind::Dir { "/" } else { "" };
        println!(
            "{:>10}  {}{slash}",
            human_size(child.disk),
            child.name.to_string_lossy()
        );
    }

    ExitCode::SUCCESS
}
