//! Running as root: detecting it, finding the real user's home, and
//! restarting nudu through sudo.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// True when nudu runs with root rights (for example through sudo).
pub fn is_root() -> bool {
    rustix::process::geteuid().is_root()
}

/// The home folder of the person at the keyboard.
///
/// Under sudo, `$HOME` usually points to `/root`. sudo also sets
/// `$SUDO_USER` to the real user, so we look their home up instead,
/// which keeps `~` meaning *your* home in the breadcrumb.
pub fn user_home() -> Option<&'static PathBuf> {
    static HOME: OnceLock<Option<PathBuf>> = OnceLock::new();
    HOME.get_or_init(|| {
        if is_root()
            && let Ok(user) = std::env::var("SUDO_USER")
            && user != "root"
            && let Some(home) = home_from_passwd(&user)
        {
            return Some(home);
        }
        std::env::var_os("HOME").map(PathBuf::from)
    })
    .as_ref()
}

/// Reads a user's home folder from /etc/passwd
/// (`name:x:uid:gid:info:home:shell`).
fn home_from_passwd(user: &str) -> Option<PathBuf> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        (fields.len() >= 6 && fields[0] == user).then(|| PathBuf::from(fields[5]))
    })
}

/// Replaces this process with `sudo <this binary> <args>`.
///
/// The binary is passed by its full path, so it works even when the
/// folder nudu lives in (like ~/.cargo/bin) is not on sudo's PATH.
/// Only returns if starting sudo failed.
pub fn relaunch_with_sudo(args: impl IntoIterator<Item = OsString>) -> io::Error {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return e,
    };
    Command::new("sudo").arg(exe).args(args).exec()
}
