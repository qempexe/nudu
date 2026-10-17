mod app;
mod scan;
mod ui;
mod units;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use app::App;
use scan::ScanOptions;

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

    // Scan before touching the terminal, so errors print normally.
    eprintln!("Scanning {} ...", args.path.display());
    let root = match scan::scan(&args.path, opts) {
        Ok(root) => root,
        Err(e) => {
            eprintln!("nudu: {}: {e}", args.path.display());
            return ExitCode::FAILURE;
        }
    };

    // init() switches to the alternate screen and raw mode, and installs a
    // panic hook that restores the terminal if anything crashes.
    let mut terminal = ratatui::init();
    let mut app = App::new(root, args.path);
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
