use std::path::PathBuf;

use clap::Parser;

/// nudu - disk usage analyzer in the spirit of ncdu
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Directory to scan (defaults to the current directory)
    #[arg(default_value = ".")]
    path: PathBuf,
}

fn main() {
    let args = Args::parse();
    println!("nudu will scan: {}", args.path.display());
}
