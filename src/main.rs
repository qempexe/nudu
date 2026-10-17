mod app;
mod fsinfo;
mod scan;
mod ui;
mod units;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use app::App;
use fsinfo::FsInfo;
use scan::ScanOptions;

/// nudu (new du) - see what is eating your disk
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Directory to scan (defaults to the current directory)
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Also descend into other mounted filesystems
    #[arg(long)]
    cross_fs: bool,

    /// Plain characters instead of Nerd Font icons
    #[arg(long)]
    no_icons: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let opts = ScanOptions {
        one_filesystem: !args.cross_fs,
    };

    // Work with the absolute path, so `.` shows up as a real location.
    let path = match args.path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("nudu: {}: {e}", args.path.display());
            return ExitCode::FAILURE;
        }
    };

    // Scan before touching the terminal, so errors print normally.
    eprintln!("Scanning {} ...", path.display());
    let root = match scan::scan(&path, opts) {
        Ok(root) => root,
        Err(e) => {
            eprintln!("nudu: {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let fs = FsInfo::for_path(&path);

    // init() switches to the alternate screen and raw mode, and installs a
    // panic hook that restores the terminal if anything crashes.
    let mut terminal = ratatui::init();
    let mut app = App::new(root, path, fs, !args.no_icons);
    let result = app.run(&mut terminal);
    ratatui::restore();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("nudu: {e}");
            ExitCode::FAILURE
        }
    }
}
