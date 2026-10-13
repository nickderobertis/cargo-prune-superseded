//! Real Cargo layouts, file mtimes and inventories for SDK, subprocess and release journeys.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use filetime::FileTime;

/// A filesystem entry’s type in a before/after inventory.
#[derive(Debug, Eq, PartialEq)]
pub enum EntryKind {
    /// An ordinary file.
    File,
    /// An ordinary directory.
    Directory,
    /// A link recorded without following its referent.
    Symlink,
}
/// Bytes, optional mtime and kind of one inventoried entry.
#[derive(Debug, Eq, PartialEq)]
pub struct InventoryEntry {
    /// File contents or a symlink’s encoded referent path; empty for directories.
    pub bytes: Vec<u8>,
    /// Modification time.
    pub modified: SystemTime,
    /// The entry’s filesystem type.
    pub kind: EntryKind,
}
/// An inventory keyed by relative path (or joined paths in a multi-build inventory).
pub type Inventory = BTreeMap<PathBuf, InventoryEntry>;

/// Expired older build age in seconds.
pub const OLD: u64 = 6 * 86400;
/// Expired newest build age in seconds.
pub const MIDDLE: u64 = 4 * 86400;
/// Within-window build age in seconds.
pub const WINDOW: u64 = 86400;
/// Recent newest build age in seconds.
pub const RECENT: u64 = 3600;
/// First deterministic metadata hash.
pub const A: &str = "0000000000000001";
/// Second deterministic metadata hash.
pub const B: &str = "0000000000000002";
/// Third deterministic metadata hash.
pub const C: &str = "0000000000000003";

/// Create an isolated directory with ordinary (canonical) ancestors.
///
/// # Panics
/// Panics if the real filesystem or subprocess operation fails or an assertion is unmet.
#[must_use]
pub fn tempdir() -> tempfile::TempDir {
    // macOS commonly exposes its temporary root through the /var symlink; the
    // fixtures themselves must have ordinary ancestors, with escapes added explicitly.
    tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
}

/// Write non-empty allocated output, creating its parent directories.
///
/// # Panics
/// Panics if the real filesystem or subprocess operation fails or an assertion is unmet.
pub fn write(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![b'x'; 8192]).unwrap();
}

/// List the five roots belonging to a unit/hash fixture build.
///
/// # Panics
/// Panics if the real filesystem or subprocess operation fails or an assertion is unmet.
#[must_use]
pub fn paths(profile: &Path, unit: &str, hash: &str) -> Vec<PathBuf> {
    let crate_name = unit.replace('-', "_");
    vec![
        profile.join(format!("deps/lib{crate_name}-{hash}.rlib")),
        profile.join(format!("deps/{crate_name}-{hash}.d")),
        profile.join(format!(".fingerprint/{unit}-{hash}")),
        profile.join(format!("build/{unit}-{hash}")),
        profile.join(format!("incremental/{crate_name}-{hash}")),
    ]
}

/// Write and age a complete build in all four Cargo output containers.
///
/// # Panics
/// Panics if the real filesystem or subprocess operation fails or an assertion is unmet.
pub fn build(profile: &Path, unit: &str, hash: &str, age: u64) {
    let paths = paths(profile, unit, hash);
    write(&paths[0]);
    write(&paths[1]);
    for path in &paths[2..] {
        write(&path.join("sibling"));
        write(&path.join("one/two/leaf"));
    }
    for path in paths {
        age_tree(&path, age);
    }
}

/// Backdate every file and directory, setting directories after their contents.
///
/// # Panics
/// Panics if the real filesystem or subprocess operation fails or an assertion is unmet.
pub fn age_tree(path: &Path, age: u64) {
    let meta = fs::symlink_metadata(path).unwrap();
    if meta.is_symlink() {
        return;
    }
    if meta.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            age_tree(&entry.unwrap().path(), age);
        }
    }
    let time = FileTime::from_system_time(SystemTime::now() - Duration::from_secs(age));
    filetime::set_file_times(path, time, time).unwrap();
}

/// Set one entry’s real modification time to the stated age.
///
/// # Panics
/// Panics if the real filesystem or subprocess operation fails or an assertion is unmet.
pub fn recent(path: &Path, age: u64) {
    let time = FileTime::from_system_time(SystemTime::now() - Duration::from_secs(age));
    filetime::set_file_mtime(path, time).unwrap();
}

/// Read every ordinary file and directory without traversing symlinks.
///
/// # Panics
/// Panics if the real filesystem or subprocess operation fails or an assertion is unmet.
pub fn refresh_atime(path: &Path) {
    let meta = fs::symlink_metadata(path).unwrap();
    if meta.is_symlink() {
        return;
    }
    if meta.is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            refresh_atime(&entry.unwrap().path());
        }
    } else {
        fs::read(path).unwrap();
    }
}

// Content, mtime and file type, but deliberately no atime.
/// Record paths, bytes, types and real mtimes, directories included.
///
/// # Panics
/// Panics if the real filesystem or subprocess operation fails or an assertion is unmet.
#[must_use]
pub fn inventory(path: &Path) -> Inventory {
    fn walk(root: &Path, path: &Path, result: &mut Inventory) {
        let meta = fs::symlink_metadata(path)
            .unwrap_or_else(|error| panic!("inventory {}: {error}", path.display()));
        let (bytes, kind) = if meta.is_symlink() {
            (
                fs::read_link(path)
                    .unwrap()
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec(),
                EntryKind::Symlink,
            )
        } else if meta.is_dir() {
            (Vec::new(), EntryKind::Directory)
        } else {
            (fs::read(path).unwrap(), EntryKind::File)
        };
        result.insert(
            path.strip_prefix(root).unwrap().to_owned(),
            InventoryEntry {
                bytes,
                modified: meta.modified().unwrap(),
                kind,
            },
        );
        if meta.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                walk(root, &entry.unwrap().path(), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    walk(path, path, &mut result);
    result
}

/// Measure allocated bytes for a set of roots, deduplicating hard links via real du.
///
/// # Panics
/// Panics if the real filesystem or subprocess operation fails or an assertion is unmet.
#[must_use]
pub fn du(paths: &[PathBuf]) -> u64 {
    let out = std::process::Command::new("du")
        .arg("-sk")
        .args(paths)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|line| {
            line.split_whitespace()
                .next()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                * 1024
        })
        .sum()
}

/// Write the alpha/beta retention journey and refresh every atime.
///
/// # Panics
/// Panics if the real filesystem or subprocess operation fails or an assertion is unmet.
pub fn basic(target: &Path) {
    let profile = target.join("debug");
    build(&profile, "alpha", A, OLD);
    build(&profile, "alpha", B, WINDOW);
    build(&profile, "alpha", C, RECENT);
    build(&profile, "beta", A, OLD);
    build(&profile, "beta", B, MIDDLE);
    refresh_atime(target);
}

/// Assert that every root of a unit/hash build is absent.
///
/// # Panics
/// Panics if the real filesystem or subprocess operation fails or an assertion is unmet.
pub fn expect_removed(profile: &Path, unit: &str, hash: &str) {
    for path in paths(profile, unit, hash) {
        assert!(!path.try_exists().unwrap(), "{} survived", path.display());
    }
}

/// Inventory all roots belonging to the given kept unit/hash builds.
///
/// # Panics
/// Panics if the real filesystem or subprocess operation fails or an assertion is unmet.
#[must_use]
pub fn kept(profile: &Path, units: &[(&str, &str)]) -> Inventory {
    let mut result = BTreeMap::new();
    for (unit, hash) in units {
        for path in paths(profile, unit, hash) {
            for (relative, value) in inventory(&path) {
                result.insert(path.join(relative), value);
            }
        }
    }
    result
}
