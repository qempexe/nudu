//! Who a file belongs to: a package, an app, a git repo, Steam, Docker...
//!
//! Owners are worked out once, right after a scan, in one top-down pass:
//!
//! 1. Path rules: `~/.cache/<x>` is `cache:x`, `~/.var/app/<id>` is a
//!    Flatpak app, `steamapps/common/<game>` is that Steam game, and so on.
//!    A folder with `.git` inside is a git repo. Everything inside a
//!    matched folder inherits its owner.
//! 2. Package databases (pacman on Arch, dpkg on Debian/Ubuntu): every
//!    file a package installed is listed there. System files no package
//!    claims are marked `unowned`.
//!
//! Each entry stores its owner as a small `Owner` value. Names (package,
//! repo, app) live once in a shared table and are referred to by number.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use rayon::prelude::*;

use crate::scan::{Entry, Kind};

/// Who owns an entry. Small and `Copy`: names are numbers into `names()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Owner {
    /// Nothing known: usually your own files.
    #[default]
    None,
    /// A system file that no installed package lists.
    Unowned,
    /// Installed by this package.
    Package(Name),
    /// A system folder that several packages put files in.
    Packages(u32),
    Git(Name),
    Flatpak(Name),
    /// Steam itself (`None`) or one game.
    Steam(Option<Name>),
    Docker,
    Containers,
    Cache(Name),
    Config(Name),
    Data(Name),
    Trash,
}

/// A name in the shared table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Name(u32);

impl Owner {
    /// Owners that cover everything inside the folder they were found on.
    fn covers_contents(self) -> bool {
        !matches!(
            self,
            Owner::None | Owner::Unowned | Owner::Package(_) | Owner::Packages(_)
        )
    }

    /// `pacman:firefox`, `git:nudu`, `cache:mozilla`, `12 packages`...
    pub fn label(self) -> String {
        match self {
            Owner::None => "other".into(),
            Owner::Unowned => "unowned".into(),
            Owner::Package(n) => format!("{}:{}", manager_name(), text(n)),
            Owner::Packages(count) => format!("{count} packages"),
            Owner::Git(n) => format!("git:{}", text(n)),
            Owner::Flatpak(n) => format!("flatpak:{}", text(n)),
            Owner::Steam(None) => "steam".into(),
            Owner::Steam(Some(n)) => format!("steam:{}", text(n)),
            Owner::Docker => "docker".into(),
            Owner::Containers => "containers".into(),
            Owner::Cache(n) => format!("cache:{}", text(n)),
            Owner::Config(n) => format!("config:{}", text(n)),
            Owner::Data(n) => format!("data:{}", text(n)),
            Owner::Trash => "trash".into(),
        }
    }

    /// The package name, when a package owns this.
    pub fn package(self) -> Option<String> {
        match self {
            Owner::Package(n) => Some(text(n)),
            _ => None,
        }
    }
}

// ── The shared name table ───────────────────────────────────────────────────

#[derive(Default)]
struct Names {
    list: Vec<String>,
    index: HashMap<String, u32>,
}

fn names() -> &'static Mutex<Names> {
    static NAMES: OnceLock<Mutex<Names>> = OnceLock::new();
    NAMES.get_or_init(Default::default)
}

/// The number for `name`, adding it to the table the first time.
fn intern(name: &str) -> Name {
    let mut t = names().lock().unwrap();
    if let Some(&i) = t.index.get(name) {
        return Name(i);
    }
    let i = t.list.len() as u32;
    t.list.push(name.to_string());
    t.index.insert(name.to_string(), i);
    Name(i)
}

fn text(name: Name) -> String {
    names().lock().unwrap().list[name.0 as usize].clone()
}

// ── Package databases ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    Pacman,
    Dpkg,
}

/// What the package database says about one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Claim {
    One(Name),
    /// Listed by this many packages (shared folders like /usr/lib).
    Many(u32),
}

/// Every path any installed package lists, and who lists it.
#[derive(Default)]
pub struct PackageDb {
    pub manager: Option<Manager>,
    claims: HashMap<PathBuf, Claim>,
}

impl PackageDb {
    /// Loads the system's database: pacman if present, else dpkg.
    pub fn load() -> PackageDb {
        if Path::new("/var/lib/pacman/local").is_dir() {
            Self::load_pacman(Path::new("/var/lib/pacman/local"))
        } else if Path::new("/var/lib/dpkg/info").is_dir() {
            Self::load_dpkg(Path::new("/var/lib/dpkg/info"))
        } else {
            PackageDb::default()
        }
    }

    /// pacman keeps one folder per package, like `firefox-131.0-1/`, with a
    /// `files` list: a `%FILES%` header, then paths relative to `/`.
    pub fn load_pacman(dir: &Path) -> PackageDb {
        let mut db = PackageDb {
            manager: Some(Manager::Pacman),
            ..Default::default()
        };
        for pkg in fs::read_dir(dir).into_iter().flatten().flatten() {
            let Some(name) = pacman_name(&pkg.path()) else { continue };
            let Ok(files) = fs::read_to_string(pkg.path().join("files")) else { continue };
            let name = intern(&name);
            let listed = files
                .lines()
                .skip_while(|l| *l != "%FILES%")
                .skip(1)
                .take_while(|l| !l.is_empty());
            for rel in listed {
                db.claim(Path::new("/").join(rel), name);
            }
        }
        db
    }

    /// dpkg keeps `<package>[:arch].list` files with absolute paths.
    pub fn load_dpkg(dir: &Path) -> PackageDb {
        let mut db = PackageDb {
            manager: Some(Manager::Dpkg),
            ..Default::default()
        };
        for item in fs::read_dir(dir).into_iter().flatten().flatten() {
            let file_name = item.file_name().to_string_lossy().into_owned();
            let Some(stem) = file_name.strip_suffix(".list") else { continue };
            let package = stem.split(':').next().unwrap_or(stem);
            let Ok(list) = fs::read_to_string(item.path()) else { continue };
            let name = intern(package);
            for line in list.lines().filter(|l| l.starts_with('/') && *l != "/.") {
                db.claim(PathBuf::from(line), name);
            }
        }
        db
    }

    fn claim(&mut self, path: PathBuf, package: Name) {
        let path = normalize(&path);
        self.claims
            .entry(path)
            .and_modify(|claim| {
                *claim = match *claim {
                    Claim::One(other) if other == package => Claim::One(other),
                    Claim::One(_) => Claim::Many(2),
                    Claim::Many(n) => Claim::Many(n + 1),
                }
            })
            .or_insert(Claim::One(package));
    }

    fn lookup(&self, path: &Path) -> Option<Claim> {
        self.claims.get(&normalize(path)).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.claims.is_empty()
    }
}

/// `firefox-131.0.3-1` -> `firefox`: pacman folders end in `-version-release`.
fn pacman_name(dir: &Path) -> Option<String> {
    let full = dir.file_name()?.to_string_lossy().into_owned();
    let mut parts = full.rsplitn(3, '-');
    let (_release, _version, name) = (parts.next()?, parts.next()?, parts.next()?);
    Some(name.to_string())
}

/// On most systems `/bin`, `/sbin` and `/lib` are symlinks into `/usr`,
/// but older packages still list the short form. Use the `/usr` form for
/// both, so `/bin/ls` and `/usr/bin/ls` are the same file.
fn normalize(path: &Path) -> PathBuf {
    for (short, long) in [("/bin", "/usr/bin"), ("/sbin", "/usr/sbin"), ("/lib64", "/usr/lib64"), ("/lib", "/usr/lib")] {
        if let Ok(rest) = path.strip_prefix(short) {
            if merged_usr() {
                return Path::new(long).join(rest);
            }
        }
    }
    path.to_path_buf()
}

fn merged_usr() -> bool {
    static MERGED: OnceLock<bool> = OnceLock::new();
    *MERGED.get_or_init(|| fs::symlink_metadata("/bin").is_ok_and(|m| m.file_type().is_symlink()))
}

static MANAGER: OnceLock<Option<Manager>> = OnceLock::new();

fn manager_name() -> &'static str {
    match MANAGER.get().copied().flatten() {
        Some(Manager::Pacman) => "pacman",
        Some(Manager::Dpkg) => "dpkg",
        None => "pkg",
    }
}

/// The package database, loaded once on first use.
pub fn package_db() -> &'static PackageDb {
    static DB: OnceLock<PackageDb> = OnceLock::new();
    DB.get_or_init(|| {
        let db = PackageDb::load();
        let _ = MANAGER.set(db.manager);
        db
    })
}

// ── Path rules ──────────────────────────────────────────────────────────────

/// Folders where files are expected to belong to a package.
fn is_system_path(path: &Path) -> bool {
    ["/usr", "/etc", "/opt", "/boot", "/srv"]
        .iter()
        .any(|p| path.starts_with(p))
}

/// The owner a folder gets from where it is, if any.
fn rule(path: &Path, home: Option<&Path>) -> Option<Owner> {
    let parts: Vec<&str> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();

    // A Steam game, wherever the library is.
    if let [.., "steamapps", "common", game] = parts[..] {
        return Some(Owner::Steam(Some(intern(game))));
    }

    if let Some(home) = home
        && let Ok(rest) = path.strip_prefix(home)
    {
        let rest: Vec<&str> = rest.components().filter_map(|c| c.as_os_str().to_str()).collect();
        return match rest[..] {
            [".cache", x] => Some(Owner::Cache(intern(x))),
            [".config", x] => Some(Owner::Config(intern(x))),
            [".steam"] | [".local", "share", "Steam"] => Some(Owner::Steam(None)),
            [".local", "share", "flatpak", "app" | "runtime", id] => Some(Owner::Flatpak(intern(id))),
            [".var", "app", id] => Some(Owner::Flatpak(intern(id))),
            [".local", "share", "Trash"] => Some(Owner::Trash),
            [".local", "share", "containers"] => Some(Owner::Containers),
            [".local", "share", x] => Some(Owner::Data(intern(x))),
            [".docker"] => Some(Owner::Docker),
            // Other dot-folders in home belong to the app named after them.
            [x] if x.starts_with('.') && ![".local", ".config", ".cache", ".var"].contains(&x) => {
                Some(Owner::Data(intern(&x[1..])))
            }
            _ => None,
        };
    }

    match parts[..] {
        ["var", "lib", "docker"] => Some(Owner::Docker),
        ["var", "lib", "containers"] => Some(Owner::Containers),
        ["var", "lib", "flatpak", "app" | "runtime", id] => Some(Owner::Flatpak(intern(id))),
        ["var", "cache", "pacman"] => Some(Owner::Cache(intern("pacman"))),
        ["var", "cache", "apt"] => Some(Owner::Cache(intern("apt"))),
        _ => None,
    }
}

// ── Assigning owners after a scan ───────────────────────────────────────────

struct Context<'a> {
    home: Option<&'a Path>,
    db: &'a PackageDb,
}

/// True when a scan of `path` can reach files a package installed.
/// Packages never install into a home folder, so scanning inside one
/// skips the package database entirely.
pub fn needs_packages(path: &Path) -> bool {
    !crate::privilege::user_home().is_some_and(|home| path.starts_with(home))
}

/// Starts reading the package database, so it is ready when the scan
/// ends. Meant to run on its own thread next to the scan.
pub fn preload(path: &Path) {
    if needs_packages(path) {
        package_db();
    }
}

/// Sets `owner` on `root` (which lives at `path`) and everything below it.
pub fn assign(root: &mut Entry, path: &Path) {
    static NO_PACKAGES: OnceLock<PackageDb> = OnceLock::new();
    let ctx = Context {
        home: crate::privilege::user_home().map(PathBuf::as_path),
        db: if needs_packages(path) {
            package_db()
        } else {
            NO_PACKAGES.get_or_init(PackageDb::default)
        },
    };
    // What a folder at `path` inherits from the folders above it.
    let inherited = path.parent().map(|p| inherited_at(p, &ctx)).unwrap_or_default();
    visit(root, path, inherited, &ctx);
}

/// Works out the owner a path inherits by applying the rules to each of
/// its ancestors, top to bottom (used when rescanning a subfolder).
fn inherited_at(path: &Path, ctx: &Context) -> Owner {
    let mut owner = Owner::None;
    let ancestors: Vec<&Path> = path.ancestors().collect();
    for dir in ancestors.into_iter().rev() {
        if dir.join(".git").exists() {
            owner = Owner::Git(intern(&display_name(dir)));
        } else if let Some(found) = rule(dir, ctx.home) {
            owner = found;
        }
    }
    if owner.covers_contents() { owner } else { Owner::None }
}

fn visit(entry: &mut Entry, path: &Path, inherited: Owner, ctx: &Context) {
    if entry.kind != Kind::Dir {
        entry.owner = if inherited.covers_contents() {
            inherited
        } else {
            package_owner(path, ctx)
        };
        return;
    }

    entry.owner = folder_owner(entry, path, inherited, ctx);
    let pass_down = if entry.owner.covers_contents() {
        entry.owner
    } else {
        inherited
    };
    // Files only need their full path for a package lookup. Skip building
    // it (one allocation per file) when they simply inherit an owner or
    // there is no package database to ask.
    let files_inherit = pass_down.covers_contents() || ctx.db.is_empty();
    entry.children.par_iter_mut().for_each(|child| {
        if child.kind != Kind::Dir && files_inherit {
            child.owner = if pass_down.covers_contents() { pass_down } else { Owner::None };
        } else {
            visit(child, &path.join(&child.name), pass_down, ctx);
        }
    });
}

fn folder_owner(dir: &Entry, path: &Path, inherited: Owner, ctx: &Context) -> Owner {
    // The innermost git repo wins, even inside a config folder.
    if dir.children.iter().any(|c| c.name == ".git") {
        return Owner::Git(intern(&display_name(path)));
    }
    if let Some(found) = rule(path, ctx.home) {
        return found;
    }
    if inherited.covers_contents() {
        return inherited;
    }
    package_owner(path, ctx)
}

/// What the package database says, or `unowned` in system folders.
fn package_owner(path: &Path, ctx: &Context) -> Owner {
    if ctx.db.is_empty() {
        return Owner::None;
    }
    match ctx.db.lookup(path) {
        Some(Claim::One(name)) => Owner::Package(name),
        Some(Claim::Many(n)) => Owner::Packages(n),
        None if !ctx.db.is_empty() && is_system_path(path) => Owner::Unowned,
        None => Owner::None,
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

// ── Ranking owners for the owners view ──────────────────────────────────────

/// Bytes on disk per owner inside `dir`, biggest first. Counts files (and
/// other non-folders), so the numbers add up to what the files take.
pub fn ranking(dir: &Entry) -> Vec<(Owner, u64)> {
    let mut totals: HashMap<Owner, u64> = HashMap::new();
    fn walk(e: &Entry, totals: &mut HashMap<Owner, u64>) {
        if e.kind == Kind::Dir {
            for c in &e.children {
                walk(c, totals);
            }
        } else if e.disk > 0 {
            *totals.entry(e.owner).or_default() += e.disk;
        }
    }
    walk(dir, &mut totals);
    let mut list: Vec<(Owner, u64)> = totals.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1));
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nudu-owners-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_pacman_databases() {
        let db_dir = temp("pacman");
        let pkg = db_dir.join("firefox-131.0.3-1");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(
            pkg.join("files"),
            "%FILES%\nusr/\nusr/bin/\nusr/bin/firefox\nusr/lib/firefox/libxul.so\n\n%BACKUP%\netc/firefox.conf\tabc\n",
        )
        .unwrap();
        let other = db_dir.join("glibc-2.40-1");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("files"), "%FILES%\nusr/\nusr/bin/\nusr/bin/ldd\n").unwrap();

        let db = PackageDb::load_pacman(&db_dir);
        let firefox = intern("firefox");
        assert_eq!(db.lookup(Path::new("/usr/bin/firefox")), Some(Claim::One(firefox)));
        assert_eq!(db.lookup(Path::new("/usr/lib/firefox/libxul.so")), Some(Claim::One(firefox)));
        assert_eq!(db.lookup(Path::new("/usr/bin")), Some(Claim::Many(2)));
        // Backup entries are not part of %FILES%.
        assert_eq!(db.lookup(Path::new("/etc/firefox.conf")), None);
        fs::remove_dir_all(db_dir).unwrap();
    }

    #[test]
    fn reads_dpkg_databases() {
        let db_dir = temp("dpkg");
        fs::write(db_dir.join("coreutils.list"), "/.\n/usr\n/usr/bin\n/usr/bin/ls\n").unwrap();
        fs::write(db_dir.join("libc6:amd64.list"), "/.\n/usr\n/usr/lib/libc.so.6\n").unwrap();
        let db = PackageDb::load_dpkg(&db_dir);
        assert_eq!(db.lookup(Path::new("/usr/bin/ls")), Some(Claim::One(intern("coreutils"))));
        assert_eq!(db.lookup(Path::new("/usr/lib/libc.so.6")), Some(Claim::One(intern("libc6"))));
        assert_eq!(db.lookup(Path::new("/usr")), Some(Claim::Many(2)));
        fs::remove_dir_all(db_dir).unwrap();
    }

    #[test]
    fn pacman_folder_names() {
        assert_eq!(pacman_name(Path::new("/x/firefox-131.0.3-1")).as_deref(), Some("firefox"));
        assert_eq!(pacman_name(Path::new("/x/lib32-gcc-libs-14.2.1-1")).as_deref(), Some("lib32-gcc-libs"));
        assert_eq!(pacman_name(Path::new("/x/python-3.12.7-1")).as_deref(), Some("python"));
    }

    #[test]
    fn path_rules() {
        let home = Path::new("/home/me");
        let h = Some(home);
        let label = |p: &str| rule(Path::new(p), h).map(Owner::label);
        assert_eq!(label("/home/me/.cache/mozilla").as_deref(), Some("cache:mozilla"));
        assert_eq!(label("/home/me/.config/nvim").as_deref(), Some("config:nvim"));
        assert_eq!(label("/home/me/.local/share/Steam").as_deref(), Some("steam"));
        assert_eq!(
            label("/home/me/.local/share/Steam/steamapps/common/Portal 2").as_deref(),
            Some("steam:Portal 2")
        );
        assert_eq!(label("/home/me/.var/app/org.gimp.GIMP").as_deref(), Some("flatpak:org.gimp.GIMP"));
        assert_eq!(label("/home/me/.local/share/Trash").as_deref(), Some("trash"));
        assert_eq!(label("/home/me/.minecraft").as_deref(), Some("data:minecraft"));
        assert_eq!(label("/var/lib/docker").as_deref(), Some("docker"));
        assert_eq!(label("/home/me/Documents"), None);
        assert_eq!(label("/home/me/.config"), None);
    }

    #[test]
    fn assigns_git_and_inherits() {
        let root = temp("git");
        fs::create_dir_all(root.join("app/.git")).unwrap();
        fs::create_dir_all(root.join("app/src")).unwrap();
        fs::write(root.join("app/src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join("notes.txt"), "hi").unwrap();

        let opts = crate::scan::ScanOptions { one_filesystem: true, include: Vec::new() };
        let mut tree = crate::scan::scan(&root, opts, &Default::default()).unwrap().unwrap();
        assign(&mut tree, &root);

        let app = tree.children.iter().find(|c| c.name == "app").unwrap();
        assert_eq!(app.owner.label(), "git:app");
        let src = app.children.iter().find(|c| c.name == "src").unwrap();
        assert_eq!(src.children[0].owner.label(), "git:app");
        let notes = tree.children.iter().find(|c| c.name == "notes.txt").unwrap();
        assert!(!notes.owner.covers_contents());
        fs::remove_dir_all(root).unwrap();
    }
}
