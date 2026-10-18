mod age;
mod app;
mod delete;
mod filetype;
mod filter;
mod fsinfo;
mod mounts;
mod owners;
mod privilege;
mod scan;
mod scanning;
mod sort;
mod tree;
mod ui;
mod units;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

use app::{App, Settings};
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

    /// Turn off deleting
    #[arg(long, short = 'r')]
    read_only: bool,

    /// Number of scanning threads (default: one per CPU core)
    #[arg(long, short = 't', value_name = "N")]
    threads: Option<usize>,
}

fn main() -> ExitCode {
    let args = Args::parse();

    // Work with the absolute path, so `.` shows up as a real location.
    // Checking it here, before the terminal switches screens, means a typo
    // in the path prints a normal error message.
    let path = match args.path.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("nudu: {}: {e}", args.path.display());
            return ExitCode::FAILURE;
        }
    };

    if let Some(n) = args.threads {
        // Configures the thread pool rayon uses for the whole scan.
        rayon::ThreadPoolBuilder::new()
            .num_threads(n.max(1))
            .build_global()
            .expect("thread pool is only configured once");
    }

    // init() switches to the alternate screen and raw mode, and installs a
    // panic hook that restores the terminal if anything crashes.
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &args, path);
    ratatui::restore();

    match result {
        Ok(Next::Exit) => ExitCode::SUCCESS,
        Ok(Next::RelaunchWithSudo) => {
            // Same arguments as this run, now with root rights.
            // relaunch_with_sudo only returns if sudo could not be started.
            let err = privilege::relaunch_with_sudo(std::env::args_os().skip(1));
            eprintln!("nudu: could not start sudo: {err}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("nudu: {e}");
            ExitCode::FAILURE
        }
    }
}

/// What to do after the interface closes.
enum Next {
    Exit,
    RelaunchWithSudo,
}

/// Scan with live progress, then browse the result.
fn run(
    terminal: &mut ratatui::DefaultTerminal,
    args: &Args,
    path: PathBuf,
) -> std::io::Result<Next> {
    let opts = ScanOptions {
        one_filesystem: !args.cross_fs,
        include: Vec::new(),
    };
    let root = match scanning::run(terminal, &path, opts.clone())? {
        scanning::Outcome::Done(root) => root,
        scanning::Outcome::Cancelled => return Ok(Next::Exit),
        // Say which path failed, not just "No such file or directory".
        scanning::Outcome::Failed(e) => {
            return Err(std::io::Error::new(
                e.kind(),
                format!("{}: {e}", path.display()),
            ));
        }
    };
    let fs = FsInfo::for_path(&path);
    let settings = Settings {
        icons: !args.no_icons,
        read_only: args.read_only,
        scan: opts,
    };
    let mut app = App::new(root, path, fs, settings);
    app.run(terminal)?;
    Ok(if app.relaunch_sudo {
        Next::RelaunchWithSudo
    } else {
        Next::Exit
    })
}
