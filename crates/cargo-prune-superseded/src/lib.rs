//! Delete superseded Cargo build output using modification time, never access time.
#![doc = include_str!("../README.md")]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, Metadata};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;

/// Retention policy; callers must choose their own window.
#[derive(Debug)]
pub struct Options {
    /// Keep every build modified within this window, as well as each unit's newest.
    // llmlint: ignore[invalid_states_unrepresentable] The fixed SDK contract requires Duration here; prune_target_measured rejects zero at the boundary and parse_span validates CLI input.
    pub keep_within: Duration,
    /// Calculate removals without changing the filesystem.
    // llmlint: ignore[invalid_states_unrepresentable] The fixed SDK contract requires this bool; both values are valid execution policies, so no invalid state is expressible.
    pub dry_run: bool,
}

/// The result of one target-directory pass. Sizes are allocated bytes.
#[derive(Debug, Serialize)]
pub struct TargetReport {
    /// The supplied target directory.
    #[serde(serialize_with = "serialize_path")]
    pub path: PathBuf,
    /// Profile directories discovered without following symlinks.
    pub profiles: u32,
    /// Superseded unit/hash builds removed (incremental directories are not builds).
    pub builds_removed: u64,
    /// Blocks freed, excluding inodes with surviving hard links.
    pub bytes_freed: u64,
    /// Blocks belonging to retained recognized entries, counting each inode once.
    pub bytes_retained: u64,
    /// Entries refused or inaccessible; other entries continue to be processed.
    pub skipped: Vec<Skipped>,
}

/// An entry the pass could not process.
#[derive(Debug, Serialize)]
pub struct Skipped {
    /// The entry's path.
    #[serde(serialize_with = "serialize_path")]
    pub path: PathBuf,
    /// Why the entry was skipped.
    pub reason: String,
}

fn serialize_path<S: serde::Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}

/// Invalid retention input or an unreadable target directory.
#[derive(Debug)]
pub enum Error {
    /// A duration outside the positive integer + s/m/h/d grammar.
    InvalidSpan(String),
    /// The target directory could not be read.
    Target {
        /// The target whose read failed.
        path: PathBuf,
        /// The filesystem error.
        source: io::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSpan(text) => write!(
                f,
                "invalid span '{text}': expected a positive integer followed by s, m, h or d"
            ),
            Self::Target { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Target { source, .. } => Some(source),
            Self::InvalidSpan(_) => None,
        }
    }
}

/// Parse a positive integer followed by `s`, `m`, `h` or `d`.
///
/// # Errors
/// Returns [`Error::InvalidSpan`] for invalid or overflowing input.
pub fn parse_span(text: &str) -> Result<Duration, Error> {
    let invalid = || Error::InvalidSpan(text.to_owned());
    let unit = text.as_bytes().last().ok_or_else(invalid)?;
    let factor = match unit {
        b's' => 1,
        b'm' => 60,
        b'h' => 3600,
        b'd' => 86400,
        _ => return Err(invalid()),
    };
    let digits = &text[..text.len() - 1];
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let seconds = digits
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .and_then(|n| n.checked_mul(factor))
        .ok_or_else(invalid)?;
    Ok(Duration::from_secs(seconds))
}

/// Timings and counts for a pass, for budget telemetry rather than retention decisions.
#[derive(Debug, Default)]
pub struct Metrics {
    /// Files and directories inspected (symlinks are never descended into).
    pub entries_walked: u64,
    /// Wall-clock seconds spent discovering and measuring entries.
    pub walking_seconds: f64,
    /// Wall-clock seconds spent removing selected entries.
    pub removing_seconds: f64,
}

/// Prune a target directory with a caller-supplied time.
///
/// # Errors
/// Returns [`Error::Target`] if the target cannot be read. Missing targets and symlinked
/// targets produce a report with a skipped entry instead.
pub fn prune_target(
    target: &Path,
    options: &Options,
    now: SystemTime,
) -> Result<TargetReport, Error> {
    prune_target_measured(target, options, now).map(|(report, _)| report)
}

/// Like [`prune_target`], also returning the measurements used by the binary's budget report.
///
/// # Errors
/// Returns the same errors as [`prune_target`].
pub fn prune_target_measured(
    target: &Path,
    options: &Options,
    now: SystemTime,
) -> Result<(TargetReport, Metrics), Error> {
    if options.keep_within.is_zero() {
        return Err(Error::InvalidSpan("0s".to_owned()));
    }
    let start = Instant::now();
    let mut pass = Pass {
        report: TargetReport {
            path: target.to_owned(),
            profiles: 0,
            builds_removed: 0,
            bytes_freed: 0,
            bytes_retained: 0,
            skipped: Vec::new(),
        },
        metrics: Metrics::default(),
        retained: BTreeSet::new(),
        removals: Vec::new(),
    };
    let meta = match target_metadata(target) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            pass.skip(target, "target directory is absent");
            return Ok((pass.report, pass.metrics));
        }
        Err(source) => {
            return Err(Error::Target {
                path: target.to_owned(),
                source,
            });
        }
    };
    if meta.is_symlink() {
        pass.skip(target, "symlink in target directory path refused");
        return Ok((pass.report, pass.metrics));
    }
    let children = pass.children(target).map_err(|source| Error::Target {
        path: target.to_owned(),
        source,
    })?;
    pass.discover(target, children, options, now);
    pass.metrics.walking_seconds = start.elapsed().as_secs_f64();
    let removing = Instant::now();
    if options.dry_run {
        pass.predict_freed();
    } else {
        pass.remove_selected();
    }
    pass.metrics.removing_seconds = removing.elapsed().as_secs_f64();
    Ok((pass.report, pass.metrics))
}

#[derive(Default)]
struct Build {
    paths: Vec<PathBuf>,
    entries: Vec<(PathBuf, EntryMetadata)>,
    newest: Option<SystemTime>,
    incomplete: bool,
    kind: BuildKind,
}

struct Pass {
    report: TargetReport,
    metrics: Metrics,
    retained: BTreeSet<(u64, u64)>,
    removals: Vec<Build>,
}

impl Pass {
    fn skip(&mut self, path: &Path, reason: &(impl ToString + ?Sized)) {
        self.report.skipped.push(Skipped {
            path: path.to_owned(),
            reason: reason.to_string(),
        });
    }

    fn discover(
        &mut self,
        path: &Path,
        children: Vec<PathBuf>,
        options: &Options,
        now: SystemTime,
    ) {
        if children.iter().any(|child| {
            child.file_name().is_some_and(|name| name == ".fingerprint")
                && fs::symlink_metadata(child).is_ok_and(|m| m.is_dir() || m.is_symlink())
        }) {
            self.report.profiles += 1;
            self.profile(path, options, now);
            return;
        }
        for child in children {
            if child
                .file_name()
                .is_some_and(|name| name == "doc" || name == "tmp")
            {
                continue;
            }
            match fs::symlink_metadata(&child) {
                Ok(meta) if meta.is_symlink() => {
                    self.skip(&child, "symlink refused during profile discovery");
                }
                Ok(meta) if meta.is_dir() => match self.children(&child) {
                    Ok(children) => self.discover(&child, children, options, now),
                    Err(e) => self.skip(&child, &e),
                },
                Ok(_) => (),
                Err(e) => self.skip(&child, &e),
            }
        }
    }

    fn children(&mut self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let mut children = Vec::new();
        for entry in fs::read_dir(path)? {
            match entry {
                Ok(e) => {
                    self.metrics.entries_walked += 1;
                    children.push(e.path());
                }
                Err(e) => self.skip(path, &e),
            }
        }
        children.sort();
        Ok(children)
    }

    fn profile(&mut self, path: &Path, options: &Options, now: SystemTime) {
        let mut groups: BTreeMap<(BuildKind, UnitName), BTreeMap<BuildId, Build>> = BTreeMap::new();
        let mut unreadable_container = false;
        for container in ["deps", ".fingerprint", "build", "incremental"] {
            let dir = path.join(container);
            let skipped_before = self.report.skipped.len();
            let children = match fs::symlink_metadata(&dir) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Ok(meta) if meta.is_symlink() => {
                    self.skip(&dir, "symlinked container refused");
                    continue;
                }
                metadata => match metadata.and_then(|_| self.children(&dir)) {
                    Ok(children) => {
                        // An entry the listing could not name may belong to any build.
                        unreadable_container |= self.report.skipped.len() != skipped_before;
                        children
                    }
                    Err(e) => {
                        self.skip(&dir, &e);
                        unreadable_container = true;
                        continue;
                    }
                },
            };
            for entry in children {
                let kind = if container == "incremental" {
                    BuildKind::Incremental
                } else {
                    BuildKind::Compiled
                };
                if kind == BuildKind::Incremental
                    && !fs::symlink_metadata(&entry).is_ok_and(|m| m.is_dir() || m.is_symlink())
                {
                    continue;
                }
                let Some((unit, hash)) = entry.file_name().and_then(|s| s.to_str()).and_then(|s| {
                    split_name(
                        s,
                        kind,
                        if container == "deps" {
                            ArtifactOrigin::Dependency
                        } else {
                            ArtifactOrigin::Other
                        },
                    )
                }) else {
                    continue;
                };
                let build = groups
                    .entry((kind, unit))
                    .or_default()
                    .entry(hash)
                    .or_default();
                build.kind = kind;
                build.paths.push(entry.clone());
                self.measure(&entry, build);
            }
        }
        for builds in groups.into_values() {
            let newest = builds
                .iter()
                .filter_map(|(hash, b)| b.newest.map(|time| (time, hash)))
                .max()
                .map(|(_, hash)| hash.clone());
            for (hash, build) in builds {
                let recent = build.newest.is_some_and(|time| {
                    now.duration_since(time)
                        .map_or(true, |age| age <= options.keep_within)
                });
                if unreadable_container
                    || build.incomplete
                    || newest.as_ref() == Some(&hash)
                    || recent
                    || build.newest.is_none()
                {
                    for (_, meta) in &build.entries {
                        if self.retained.insert(identity(meta)) {
                            self.report.bytes_retained += allocated_bytes(meta);
                        }
                    }
                } else {
                    self.removals.push(build);
                }
            }
        }
    }

    fn measure(&mut self, path: &Path, build: &mut Build) {
        let meta = match entry_metadata(path) {
            Ok(meta) if meta.is_symlink() => return,
            Ok(meta) => meta,
            Err(e) => {
                self.skip(path, &e);
                build.incomplete = true;
                return;
            }
        };
        match meta.modified() {
            Ok(time) => build.newest = Some(build.newest.map_or(time, |old| old.max(time))),
            Err(e) => {
                self.skip(path, &e);
                build.incomplete = true;
            }
        }
        if meta.is_dir() {
            let skipped_before = self.report.skipped.len();
            match self.children(path) {
                Ok(children) => {
                    build.incomplete |= self.report.skipped.len() != skipped_before;
                    for child in children {
                        self.measure(&child, build);
                    }
                }
                Err(e) => {
                    self.skip(path, &e);
                    build.incomplete = true;
                }
            }
        }
        build.entries.push((path.to_owned(), meta));
    }

    fn predict_freed(&mut self) {
        let mut inodes: BTreeMap<(u64, u64), (u64, u64, u64, bool)> = BTreeMap::new();
        for build in &self.removals {
            if build.kind == BuildKind::Compiled {
                self.report.builds_removed += 1;
            }
            for (_, meta) in &build.entries {
                let item = inodes.entry(identity(meta)).or_insert((
                    0,
                    links(meta),
                    allocated_bytes(meta),
                    meta.is_dir(),
                ));
                item.0 += 1;
            }
        }
        self.report.bytes_freed = inodes
            .values()
            .filter(|(count, links, _, dir)| *dir || count >= links)
            .map(|(_, _, bytes, _)| bytes)
            .sum();
    }

    fn remove_selected(&mut self) {
        for build in std::mem::take(&mut self.removals) {
            let mut complete = true;
            for path in &build.paths {
                complete &= self.remove(path);
            }
            if complete && build.kind == BuildKind::Compiled {
                self.report.builds_removed += 1;
            }
            if !complete {
                for (path, _) in &build.entries {
                    if let Ok(meta) = entry_metadata(path)
                        && !meta.is_symlink()
                        && self.retained.insert(identity(&meta))
                    {
                        self.report.bytes_retained += allocated_bytes(&meta);
                    }
                }
            }
        }
    }

    fn remove(&mut self, path: &Path) -> bool {
        match self.remove_entry(path) {
            Ok(meta) => {
                if !meta.is_symlink() && (meta.is_dir() || links(&meta) == 1) {
                    self.report.bytes_freed += allocated_bytes(&meta);
                }
                true
            }
            Err(e) => {
                self.skip(path, &e);
                false
            }
        }
    }

    // A child that fails is skipped by its own call; the parent then fails as non-empty.
    fn remove_entry(&mut self, path: &Path) -> io::Result<EntryMetadata> {
        let meta = entry_metadata(path)?;
        if meta.is_dir() {
            for child in self.children(path)? {
                self.remove(&child);
            }
            fs::remove_dir(path)?;
        } else {
            fs::remove_file(path)?;
        }
        Ok(meta)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
enum BuildKind {
    #[default]
    Compiled,
    Incremental,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct UnitName(String);
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct MetadataHash(String);
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct IncrementalId(String);
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum BuildId {
    Metadata(MetadataHash),
    Incremental(IncrementalId),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ArtifactOrigin {
    Dependency,
    Other,
}

fn split_name(name: &str, kind: BuildKind, origin: ArtifactOrigin) -> Option<(UnitName, BuildId)> {
    let stem = name.split_once('.').map_or(name, |(stem, _)| stem);
    let (unit, hash) = stem.rsplit_once('-')?;
    let library_artifact = name.split_once('.').is_some_and(|(_, extension)| {
        matches!(
            extension.split('.').next(),
            Some("rlib" | "rmeta" | "so" | "dylib" | "a")
        )
    });
    let unit = if origin == ArtifactOrigin::Dependency && library_artifact {
        unit.strip_prefix("lib").unwrap_or(unit)
    } else {
        unit
    };
    if unit.is_empty() || hash.is_empty() {
        return None;
    }
    let id = match kind {
        BuildKind::Compiled if hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()) => {
            BuildId::Metadata(MetadataHash(hash.to_owned()))
        }
        BuildKind::Compiled => return None,
        BuildKind::Incremental => BuildId::Incremental(IncrementalId(hash.to_owned())),
    };
    Some((UnitName(unit.replace('-', "_")), id))
}

fn target_metadata(target: &Path) -> io::Result<Metadata> {
    let absolute = if target.is_absolute() {
        target.to_owned()
    } else {
        std::env::current_dir()?.join(target)
    };
    let mut latest = None;
    // Root first; ancestors never yields a bare Windows drive/UNC prefix, which is not a
    // complete path until its root follows.
    let mut ancestors: Vec<_> = absolute.ancestors().collect();
    ancestors.reverse();
    for prefix in ancestors {
        let meta = fs::symlink_metadata(prefix)?;
        if meta.is_symlink() {
            return Ok(meta);
        }
        latest = Some(meta);
    }
    latest.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty target path"))
}

struct EntryMetadata {
    metadata: Metadata,
    identity: (u64, u64),
    allocated_bytes: u64,
    links: u64,
}

impl std::ops::Deref for EntryMetadata {
    type Target = Metadata;
    fn deref(&self) -> &Metadata {
        &self.metadata
    }
}

fn identity(meta: &EntryMetadata) -> (u64, u64) {
    meta.identity
}
fn allocated_bytes(meta: &EntryMetadata) -> u64 {
    meta.allocated_bytes
}
fn links(meta: &EntryMetadata) -> u64 {
    meta.links
}

fn entry_metadata(path: &Path) -> io::Result<EntryMetadata> {
    let metadata = fs::symlink_metadata(path)?;
    // Links are removable entries but never contribute a referent's time or bytes.
    if metadata.is_symlink() {
        return Ok(EntryMetadata {
            metadata,
            identity: (0, 0),
            allocated_bytes: 0,
            links: 0,
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(EntryMetadata {
            identity: (metadata.dev(), metadata.ino()),
            allocated_bytes: metadata.blocks() * 512,
            links: metadata.nlink(),
            metadata,
        })
    }
    #[cfg(windows)]
    {
        use fs2::FileExt;
        use std::os::windows::fs::OpenOptionsExt;
        // BACKUP_SEMANTICS opens directories; OPEN_REPARSE_POINT prevents following
        // a final-component reparse point if it changed since symlink_metadata.
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(0x0200_0000 | 0x0020_0000)
            .open(path)?;
        let info = winapi_util::file::information(&file)?;
        Ok(EntryMetadata {
            identity: (info.volume_serial_number(), info.file_index()),
            allocated_bytes: file.allocated_size()?,
            links: info.number_of_links(),
            metadata,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prune_fixtures as fixture;
    use std::error::Error as _;

    fn set_mtime_tree(path: &Path, time: SystemTime) {
        if path.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                set_mtime_tree(&entry.unwrap().path(), time);
            }
        }
        filetime::set_file_mtime(path, filetime::FileTime::from_system_time(time)).unwrap();
    }

    #[test]
    fn sdk_exact_retention_boundary_and_equal_mtime_ties_use_real_file_times() {
        let temp = fixture::tempdir();
        let profile = temp.path().join("debug");
        let now = SystemTime::now();
        let window = Duration::from_hours(48);
        for (hash, time) in [
            (fixture::A, now - window - Duration::from_nanos(1)),
            (fixture::B, now - window),
            (fixture::C, now - Duration::from_secs(1)),
        ] {
            fixture::build(&profile, "boundary", hash, fixture::OLD);
            for path in fixture::paths(&profile, "boundary", hash) {
                set_mtime_tree(&path, time);
            }
        }
        let equal_time = now - Duration::from_secs(fixture::OLD);
        for hash in [fixture::A, fixture::B] {
            fixture::build(&profile, "tied", hash, fixture::OLD);
            for path in fixture::paths(&profile, "tied", hash) {
                set_mtime_tree(&path, equal_time);
            }
        }
        let boundary_kept: Vec<_> = [fixture::B, fixture::C]
            .iter()
            .flat_map(|hash| fixture::paths(&profile, "boundary", hash))
            .map(|path| {
                let value = fixture::inventory(&path);
                (path, value)
            })
            .collect();
        let tied_kept: Vec<_> = fixture::paths(&profile, "tied", fixture::B)
            .into_iter()
            .map(|path| {
                let value = fixture::inventory(&path);
                (path, value)
            })
            .collect();
        let report = prune_target(
            temp.path(),
            &Options {
                keep_within: window,
                dry_run: false,
            },
            now,
        )
        .unwrap();
        assert_eq!(report.builds_removed, 2);
        fixture::expect_removed(&profile, "boundary", fixture::A);
        fixture::expect_removed(&profile, "tied", fixture::A);
        for (path, value) in boundary_kept.into_iter().chain(tied_kept) {
            assert_eq!(fixture::inventory(&path), value);
        }
    }

    #[test]
    fn sdk_parses_spans_and_rejects_overflow() {
        for (text, seconds) in [("1s", 1), ("2m", 120), ("3h", 10800), ("4d", 345_600)] {
            assert_eq!(parse_span(text).unwrap(), Duration::from_secs(seconds));
        }
        for text in [
            "",
            "0s",
            "-1s",
            "+1s",
            "1.5d",
            "1S",
            "1 d",
            "18446744073709551615d",
            "é",
        ] {
            let error = parse_span(text).unwrap_err();
            assert!(error.to_string().contains(text));
            assert!(error.source().is_none());
        }
    }

    #[test]
    fn sdk_keeps_future_build_and_handles_missing_and_nondirectory_targets() {
        let temp = fixture::tempdir();
        let target = temp.path().join("target");
        let options = Options {
            keep_within: Duration::from_hours(24),
            dry_run: false,
        };
        let report = prune_target(&target, &options, SystemTime::now()).unwrap();
        assert_eq!(report.skipped.len(), 1);
        fixture::build(&target.join("debug"), "alpha", fixture::A, fixture::OLD);
        fixture::build(&target.join("debug"), "alpha", fixture::B, fixture::MIDDLE);
        fixture::build(&target.join("debug"), "alpha", fixture::C, fixture::RECENT);
        let future = SystemTime::now() + Duration::from_hours(24);
        filetime::set_file_mtime(
            &fixture::paths(&target.join("debug"), "alpha", fixture::A)[0],
            filetime::FileTime::from_system_time(future),
        )
        .unwrap();
        let report = prune_target(&target, &options, SystemTime::now()).unwrap();
        assert_eq!(report.builds_removed, 1);
        fixture::expect_removed(&target.join("debug"), "alpha", fixture::B);
        assert!(fixture::paths(&target.join("debug"), "alpha", fixture::A)[0].exists());
        let file = temp.path().join("file");
        fixture::write(&file);
        let error = prune_target(&file, &options, SystemTime::now()).unwrap_err();
        assert!(error.to_string().contains(file.to_str().unwrap()));
        assert!(error.source().is_some());
        // A target beneath a file cannot even be inspected: an error, not an absent skip.
        let beneath = file.join("target");
        let error = prune_target(&beneath, &options, SystemTime::now()).unwrap_err();
        assert!(matches!(&error, Error::Target { path, .. } if *path == beneath));
        assert_eq!(
            error
                .source()
                .and_then(|e| e.downcast_ref::<io::Error>())
                .map(io::Error::kind),
            Some(io::ErrorKind::NotADirectory)
        );
    }

    #[test]
    fn sdk_dry_run_deduplicates_internal_hardlinks() {
        let temp = fixture::tempdir();
        let profile = temp.path().join("debug");
        fixture::build(&profile, "alpha", fixture::A, fixture::OLD);
        fixture::build(&profile, "alpha", fixture::B, fixture::MIDDLE);
        let first = fixture::paths(&profile, "alpha", fixture::A)[0].clone();
        let second = fixture::paths(&profile, "alpha", fixture::A)[1].clone();
        fs::remove_file(&second).unwrap();
        fs::hard_link(&first, &second).unwrap();
        let before = fixture::inventory(temp.path());
        let dry = prune_target(
            temp.path(),
            &Options {
                keep_within: Duration::from_hours(24),
                dry_run: true,
            },
            SystemTime::now(),
        )
        .unwrap();
        assert_eq!(fixture::inventory(temp.path()), before);
        let real = prune_target(
            temp.path(),
            &Options {
                keep_within: Duration::from_hours(24),
                dry_run: false,
            },
            SystemTime::now(),
        )
        .unwrap();
        assert_eq!(dry.bytes_freed, real.bytes_freed);
        assert_eq!(dry.bytes_retained, real.bytes_retained);
        assert_eq!(real.builds_removed, 1);
        fixture::expect_removed(&profile, "alpha", fixture::A);
    }
}

#[cfg(test)]
mod option_tests {
    use super::*;
    #[test]
    fn sdk_zero_window_is_rejected_before_any_filesystem_change() {
        let temp = tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap();
        let sentinel = temp.path().join("sentinel");
        fs::write(&sentinel, b"untouched").unwrap();
        let error = prune_target(
            temp.path(),
            &Options {
                keep_within: Duration::ZERO,
                dry_run: false,
            },
            SystemTime::now(),
        )
        .unwrap_err();
        assert!(matches!(error, Error::InvalidSpan(ref text) if text == "0s"));
        assert_eq!(fs::read(sentinel).unwrap(), b"untouched");
    }
}
