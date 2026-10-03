//! `.oxphpdeny`: request paths, in gitignore syntax, that the server must not
//! serve directly.
//!
//! The file sits at the top of `DOCUMENT_ROOT` and is read once, at startup.
//! A plain rule denies the request, answered with `PHP_DENY_FALLBACK`; a rule
//! prefixed with `>` hands the request to the application's entry script
//! instead. Rules match the sanitized request URI, not the file the router
//! would resolve it to.
use std::fs::File;
use std::io::{ErrorKind, Read};
use std::ops::ControlFlow;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use super::ignore_rules::{Rule, RuleSet};
use super::RoutingModeKind;
use crate::types::BoxError;

/// Looked up at the top of `DOCUMENT_ROOT`.
pub const DENY_FILE_NAME: &str = ".oxphpdeny";

/// Deepest request path matched against the rules, in segments. Matching
/// re-checks every parent directory, so its cost grows with the square of
/// the depth: real paths stay far below this, a hostile URI need not.
pub const MAX_DEPTH: usize = 64;

/// Budget for depth × length, in bytes. A rule re-checks every parent,
/// scanning its prefix, so its work grows with both; this caps one rule's
/// at what [`MAX_DEPTH`] segments of 2 KiB in all take to match.
/// [`MAX_RULES`](super::ignore_rules::MAX_RULES) and
/// [`MAX_PATTERN_LEN`](super::ignore_rules::MAX_PATTERN_LEN) cap the rules.
pub const MAX_MATCH_COST: usize = MAX_DEPTH * 2048;

/// Largest file read, in bytes. A rule file runs to a few KiB; the cap
/// bounds what startup reads from whatever the name points at.
const MAX_FILE_SIZE: u64 = 1 << 20;

/// What a matched rule does with the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyAction {
    /// Answer with `PHP_DENY_FALLBACK`.
    Deny,
    /// Hand the request to the entry script — a `>` rule.
    Entry,
}

/// A parsed `.oxphpdeny`.
#[derive(Debug)]
pub struct DenyFile {
    rules: RuleSet<DenyAction>,
    path: PathBuf,
    /// `PHP_DENY_FALLBACK` as `/config` shows it: a status code, or `script`.
    fallback: String,
}

impl DenyFile {
    /// Read `<document_root>/.oxphpdeny`. `Ok(None)` when there is none. An
    /// unreadable file, a malformed line, or a `>` rule in a mode with no
    /// entry script to hand the request to is an error.
    pub fn load(document_root: &Path, mode: RoutingModeKind) -> Result<Option<Self>, BoxError> {
        let path = document_root.join(DENY_FILE_NAME);
        // Without O_NONBLOCK, opening a FIFO waits for a writer and startup
        // hangs; with it, the FIFO opens and `read_regular` refuses it.
        let opened = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path);
        let bytes = match opened.and_then(read_regular) {
            Ok(bytes) => bytes,
            // A missing DOCUMENT_ROOT lands here too; `config --check`
            // reports it, and `serve` stops when routing resolves the root.
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                // A symlink to nothing is a broken deploy, not an absent
                // file: going on without the rules would serve what they deny.
                if std::fs::symlink_metadata(&path).is_ok() {
                    return Err(format!("{}: dangling symlink ({e})", path.display()).into());
                }
                return Ok(None);
            }
            Err(e) => return Err(format!("{}: {e}", path.display()).into()),
        };
        let src = String::from_utf8(bytes)
            .map_err(|_| -> BoxError { format!("{}: not valid UTF-8", path.display()).into() })?;
        let fallback = std::env::var("PHP_DENY_FALLBACK").ok();
        let file = Self::parse(&src, path, mode, fallback.as_deref())?;
        file.log_loaded();
        Ok(Some(file))
    }

    fn parse(
        src: &str,
        path: PathBuf,
        mode: RoutingModeKind,
        fallback: Option<&str>,
    ) -> Result<Self, BoxError> {
        let rules = RuleSet::parse(src, classify)
            .map_err(|e| -> BoxError { format!("{}: {e}", path.display()).into() })?;
        if matches!(mode, RoutingModeKind::Traditional | RoutingModeKind::Spa) {
            if let Some(rule) = rules.iter().find(|r| r.action == DenyAction::Entry) {
                return Err(format!(
                    "{}: line {}: {:?} hands the request to the entry script, but this routing \
                     mode has none — `>` rules need a .php ENTRY_FILE (front controller or worker)",
                    path.display(),
                    rule.line,
                    rule.original
                )
                .into());
            }
        }
        Ok(Self {
            rules,
            path,
            fallback: fallback_label(fallback),
        })
    }

    /// The rule that applies to `path` (sanitized URI, no leading `/`), if
    /// any. `is_dir`: the request URI ended with `/`. `path_info`: routing
    /// may run a `.php` segment of `path` as the script, with the rest as
    /// PATH_INFO.
    ///
    /// Two other readings of the request are judged as well, and the
    /// stricter answer wins — a deny over a `>`, a `>` over none. Routing
    /// drops a trailing slash and may serve `path` as a file, so a directory
    /// request is also judged as a file; with `path_info`, a `.php` segment
    /// followed by more path is also judged as the script it names. Judged
    /// as a directory alone, `!*/` would let `/secret.txt/` and
    /// `/secret.php/x.js` through. Nothing else the router resolves a path
    /// to — a directory index, the entry script — is judged.
    ///
    /// An excluded directory decides for everything below it, as in git,
    /// with one exception: a `>` directory hands its contents to the entry
    /// script without shielding them, so a deny below it still wins. A `!`
    /// that lifts that deny leaves the file to the `>` directory, as in git.
    pub fn check(&self, path: &str, is_dir: bool, path_info: bool) -> Option<&Rule<DenyAction>> {
        let script = |ancestor: &str| path_info && names_a_script(ancestor);
        let mut entry = None;
        let denied = self.rules.excluded_ancestors(path, script, |rule| {
            if rule.action == DenyAction::Deny {
                return ControlFlow::Break(rule);
            }
            entry.get_or_insert(rule);
            ControlFlow::Continue(())
        });
        if denied.is_some() {
            return denied;
        }
        let as_file = self.rules.matched_last(path, false);
        let as_dir = is_dir
            .then(|| self.rules.matched_last(path, true))
            .flatten();
        let deny = |r: &&Rule<DenyAction>| r.action == DenyAction::Deny;
        // Below a `>` directory a deny still wins; failing one, the `>`
        // decides — `!` re-includes nothing there, as below any excluded
        // directory.
        as_dir
            .filter(deny)
            .or(as_file.filter(deny))
            .or(entry)
            .or(as_dir)
            .or(as_file)
    }

    /// Whether matching `path` would cost too much: more than [`MAX_DEPTH`]
    /// segments, or depth × length past [`MAX_MATCH_COST`]. Such a request is
    /// refused without being matched.
    pub fn too_costly(&self, path: &str) -> bool {
        let slashes = path.bytes().filter(|&b| b == b'/').count();
        slashes >= MAX_DEPTH || (slashes + 1) * path.len() > MAX_MATCH_COST
    }

    /// Whether any rule can deny a request or hand it to the entry script.
    /// Without one the file changes no answer.
    pub fn excludes_anything(&self) -> bool {
        self.rules.iter().any(|r| !r.negated())
    }

    /// Whether any rule can answer with `PHP_DENY_FALLBACK`.
    pub fn has_deny_rules(&self) -> bool {
        self.rules
            .iter()
            .any(|r| !r.negated() && r.action == DenyAction::Deny)
    }

    /// The `deny_file` entry of `/config`: counts only. The rules stay out —
    /// they would turn the endpoint into a map of the protected paths.
    pub fn summary_json(&self) -> serde_json::Value {
        let (deny, entry, allow) = self.counts();
        serde_json::json!({
            "loaded": true,
            "rules": deny + entry + allow,
            "deny": deny,
            "entry": entry,
            "allow": allow,
            "fallback": self.fallback,
        })
    }

    /// `(deny, entry, allow)` rule counts.
    fn counts(&self) -> (usize, usize, usize) {
        let (mut deny, mut entry, mut allow) = (0, 0, 0);
        for rule in self.rules.iter() {
            match (rule.negated(), rule.action) {
                (true, _) => allow += 1,
                (false, DenyAction::Deny) => deny += 1,
                (false, DenyAction::Entry) => entry += 1,
            }
        }
        (deny, entry, allow)
    }

    fn log_loaded(&self) {
        let (deny, entry, allow) = self.counts();
        tracing::info!(
            "loaded {} rules ({deny} deny, {entry} entry, {allow} allow) from {}",
            deny + entry + allow,
            self.path.display()
        );
        for rule in self.rules.iter() {
            tracing::debug!(
                line = rule.line,
                action = action_label(rule),
                pattern = %rule.original,
                glob = %rule.glob,
                dir_only = rule.dir_only(),
                "oxphpdeny rule"
            );
        }
    }
}

/// The one addition to gitignore syntax: a leading `>`, optionally followed
/// by spaces or tabs, marks an entry rule. `\>` stays a literal `>` — the
/// glob compiler resolves the escape.
fn classify(negated: bool, rest: &str) -> Result<(DenyAction, &str), String> {
    let Some(pattern) = rest.strip_prefix('>') else {
        return Ok((DenyAction::Deny, rest));
    };
    if negated {
        return Err(
            "`!` cannot be combined with `>`: a re-include rule takes no action".to_string(),
        );
    }
    let pattern = pattern.trim_start_matches([' ', '\t']);
    if pattern.is_empty() {
        return Err("`>` must be followed by a pattern".to_string());
    }
    Ok((DenyAction::Entry, pattern))
}

fn action_label(rule: &Rule<DenyAction>) -> &'static str {
    match (rule.negated(), rule.action) {
        (true, _) => "allow",
        (false, DenyAction::Deny) => "deny",
        (false, DenyAction::Entry) => "entry",
    }
}

/// The whole of `file`, if it is a regular file of at most [`MAX_FILE_SIZE`]
/// bytes. A device or a FIFO is refused unread.
fn read_regular(file: File) -> std::io::Result<Vec<u8>> {
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_SIZE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_SIZE {
        return Err(std::io::Error::other(format!(
            "larger than {} MiB",
            MAX_FILE_SIZE >> 20
        )));
    }
    Ok(bytes)
}

/// `PHP_DENY_FALLBACK` as `/config` shows it — a script's path is left out,
/// as `/config` leaves out other filesystem layout. The value is validated
/// when routing is built, and only if a rule can deny: a bad value then stops
/// startup. Without deny rules it is never used, and shown unvalidated.
fn fallback_label(raw: Option<&str>) -> String {
    match raw.map(str::trim) {
        None => "404".to_string(),
        Some(v) => match v.parse::<u16>() {
            Ok(code) => code.to_string(),
            Err(_) => "script".to_string(),
        },
    }
}

/// Whether `path` ends in a `.php` segment, matched without regard to case
/// as routing matches it.
fn names_a_script(path: &str) -> bool {
    path.len() >= 4 && path.as_bytes()[path.len() - 4..].eq_ignore_ascii_case(b".php")
}

#[cfg(test)]
mod tests {
    use super::super::ignore_rules::MAX_RULES;
    use super::*;

    fn parse(src: &str, mode: RoutingModeKind) -> Result<DenyFile, BoxError> {
        DenyFile::parse(src, PathBuf::from("/srv/.oxphpdeny"), mode, None)
    }

    fn action_at(file: &DenyFile, path: &str, is_dir: bool) -> Option<DenyAction> {
        file.check(path, is_dir, false).map(|r| r.action)
    }

    #[test]
    fn plain_rules_deny_and_gt_rules_go_to_the_entry_script() {
        let file = parse(
            "vendor/\n>storage/\n> *.map\n>\t/private\n\\>literal",
            RoutingModeKind::Framework,
        )
        .unwrap();
        assert_eq!(action_at(&file, "vendor/x", false), Some(DenyAction::Deny));
        assert_eq!(
            action_at(&file, "storage/a.pdf", false),
            Some(DenyAction::Entry)
        );
        assert_eq!(
            action_at(&file, "js/app.js.map", false),
            Some(DenyAction::Entry)
        );
        assert_eq!(action_at(&file, "private", false), Some(DenyAction::Entry));
        // `\>` is a literal `>` in a plain rule.
        assert_eq!(action_at(&file, ">literal", false), Some(DenyAction::Deny));
        assert_eq!(action_at(&file, "app.js", false), None);
    }

    #[test]
    fn gt_rule_patterns_are_recorded_without_the_prefix() {
        let file = parse("> storage/", RoutingModeKind::Worker).unwrap();
        let rule = file.check("storage/x", false, false).unwrap();
        assert_eq!(rule.original, "> storage/");
        assert_eq!(rule.glob, "**/storage");
    }

    #[test]
    fn negated_gt_and_bare_gt_are_errors() {
        for (src, needle) in [
            ("/ok\n!>x", "line 2: `!` cannot be combined with `>`"),
            (">", "line 1: `>` must be followed by a pattern"),
            (">   ", "line 1: `>` must be followed by a pattern"),
            ("/a[", "line 1: invalid pattern"),
        ] {
            let err = parse(src, RoutingModeKind::Framework)
                .unwrap_err()
                .to_string();
            assert!(err.starts_with("/srv/.oxphpdeny: "), "{src:?}: {err}");
            assert!(err.contains(needle), "{src:?}: {err}");
        }
    }

    #[test]
    fn gt_rules_need_a_mode_with_an_entry_script() {
        for mode in [RoutingModeKind::Traditional, RoutingModeKind::Spa] {
            let err = parse("vendor/\n> storage/", mode).unwrap_err().to_string();
            assert!(err.contains("line 2"), "{mode:?}: {err}");
            assert!(err.contains("entry script"), "{mode:?}: {err}");
        }
        for mode in [RoutingModeKind::Framework, RoutingModeKind::Worker] {
            assert!(parse("> storage/", mode).is_ok(), "{mode:?}");
        }
        // Plain rules are fine everywhere.
        assert!(parse("vendor/", RoutingModeKind::Traditional).is_ok());
    }

    #[test]
    fn too_costly_caps_depth_and_depth_times_length() {
        let file = parse("vendor/", RoutingModeKind::Traditional).unwrap();
        let at_cap = vec!["a"; MAX_DEPTH].join("/");
        assert!(!file.too_costly(&at_cap));
        assert!(file.too_costly(&format!("{at_cap}/b")));
        assert!(!file.too_costly(""));
        // MAX_DEPTH segments of MAX_MATCH_COST / MAX_DEPTH bytes in all: the
        // most matching may cost. One byte more is refused.
        let filler = "a".repeat(MAX_MATCH_COST / MAX_DEPTH - 2 * (MAX_DEPTH - 1));
        let at_budget = format!("{filler}/{}", vec!["a"; MAX_DEPTH - 1].join("/"));
        assert_eq!(at_budget.len() * MAX_DEPTH, MAX_MATCH_COST);
        assert!(!file.too_costly(&at_budget));
        assert!(file.too_costly(&format!("a{at_budget}")));
        // Long but shallow stays cheap: one segment of the longest URI.
        assert!(!file.too_costly(&"a".repeat(65_534)));
        // Deep and long does not: 64 segments of 1000 bytes.
        let seg = "a".repeat(1000);
        assert!(file.too_costly(&vec![seg.as_str(); MAX_DEPTH].join("/")));
    }

    #[test]
    fn a_trailing_slash_cannot_lift_a_denial() {
        // Routing drops the slash and serves `/secret.txt/` as the file, so a
        // directory-only re-include must not let it through.
        let file = parse("*\n!*/\n!*.js\n", RoutingModeKind::Traditional).unwrap();
        let rule = file.check("secret.txt", true, false).expect("denied");
        assert_eq!(
            (rule.original.as_str(), rule.action),
            ("*", DenyAction::Deny)
        );
        assert!(file.check("app.js", true, false).is_none());
        // A deny reached either way beats a `>` reached the other way.
        let file = parse("x\n> x/\n", RoutingModeKind::Framework).unwrap();
        assert_eq!(
            file.check("x", true, false).unwrap().action,
            DenyAction::Deny
        );
        // A `>` directory rule still hands the directory over.
        let file = parse("> storage/\n", RoutingModeKind::Framework).unwrap();
        assert_eq!(
            file.check("storage", true, false).unwrap().action,
            DenyAction::Entry
        );
        assert!(file.check("storage", false, false).is_none());
    }

    #[test]
    fn a_deny_below_an_entry_directory_still_denies() {
        for src in [
            "*.sql\n> /storage/invoices/\n",
            "> /storage/invoices/\n*.sql\n",
        ] {
            let file = parse(src, RoutingModeKind::Framework).unwrap();
            let at = |path| action_at(&file, path, false);
            assert_eq!(
                at("storage/invoices/dump.sql"),
                Some(DenyAction::Deny),
                "{src:?}"
            );
            assert_eq!(
                at("storage/invoices/2026.pdf"),
                Some(DenyAction::Entry),
                "{src:?}"
            );
        }
        // A denied directory below a `>` one, and the directory itself.
        let file = parse(
            "> /storage/\n/storage/private/\n",
            RoutingModeKind::Framework,
        )
        .unwrap();
        assert_eq!(
            action_at(&file, "storage/private/key.pem", false),
            Some(DenyAction::Deny)
        );
        assert_eq!(
            action_at(&file, "storage/private", true),
            Some(DenyAction::Deny)
        );
        // `!` still cannot lift a `>` directory, as below any excluded one.
        let file = parse(
            "> /storage/\n!/storage/public.txt\n",
            RoutingModeKind::Framework,
        )
        .unwrap();
        assert_eq!(
            action_at(&file, "storage/public.txt", false),
            Some(DenyAction::Entry)
        );
        // So a `!` that lifts a deny below one leaves the file to it, as in
        // git: the entry script gets the request.
        let file = parse(
            "*.sql\n> /storage/invoices/\n!/storage/invoices/dump.sql\n",
            RoutingModeKind::Framework,
        )
        .unwrap();
        assert_eq!(
            action_at(&file, "storage/invoices/dump.sql", false),
            Some(DenyAction::Entry)
        );
    }

    #[test]
    fn a_full_rule_file_matches_the_costliest_path_quickly() {
        // Matched as one set, a few hundred `>` rules for `*name*` took
        // seconds a request on a few long segments made of their names: a
        // `>` parent does not end the walk, so each parent was matched in
        // full. Rule by rule the cost grows with the rules, no faster.
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut name = || -> String {
            (0..12)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    b"abcdefghijklmnopqrstuvwxyz0123456789"[(seed % 36) as usize] as char
                })
                .collect()
        };
        let names: Vec<String> = (0..MAX_RULES).map(|_| name()).collect();
        let src: String = names.iter().map(|n| format!("> *{n}*\n")).collect();
        let file = parse(&src, RoutingModeKind::Framework).unwrap();
        // 8 segments, as long as the budget lets them be.
        let segment = |i: usize| {
            let mut s = String::new();
            while s.len() < MAX_MATCH_COST / 8 / 8 - 1 {
                s.push_str(&names[(i * 31 + s.len()) % names.len()]);
            }
            s.truncate(MAX_MATCH_COST / 8 / 8 - 1);
            s
        };
        let path = (0..8).map(segment).collect::<Vec<_>>().join("/");
        assert!(!file.too_costly(&path));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(file.check(&path, true, true).map(|r| r.line));
        });
        let hit = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("matching took over 5 s");
        assert!(hit.is_some());
    }

    #[test]
    fn a_path_info_script_is_judged_as_a_file() {
        // Traditional routing runs `secret.php` for `secret.php/x.js`, so the
        // `.php` segment is judged as the script, not only as a directory.
        let file = parse("*\n!*/\n!*.js\n", RoutingModeKind::Traditional).unwrap();
        for path in ["secret.php/x.js", "secret.PHP/x.js", "a/b.php/c.php/x.js"] {
            assert_eq!(file.check(path, false, true).expect(path).original, "*");
        }
        // Without PATH_INFO the segment is a directory, which `!*/` keeps.
        assert!(file.check("secret.php/x.js", false, false).is_none());
        // Only a whole `.php` segment names a script.
        assert!(file.check("x.phpx/y.js", false, true).is_none());
        // A script the file re-includes keeps its PATH_INFO URIs.
        let file = parse("*\n!*/\n!*.js\n!/index.php\n", RoutingModeKind::Traditional).unwrap();
        assert!(file.check("index.php/x.js", false, true).is_none());
    }

    #[test]
    fn has_deny_rules_ignores_entry_and_allow_rules() {
        let only_entry = parse("> storage/\n!keep", RoutingModeKind::Framework).unwrap();
        assert!(!only_entry.has_deny_rules());
        let with_deny = parse("> storage/\nvendor/", RoutingModeKind::Framework).unwrap();
        assert!(with_deny.has_deny_rules());
    }

    #[test]
    fn summary_counts_rules_by_action() {
        let file = DenyFile::parse(
            "# comment\nvendor/\n*.sql\n/composer.*\n> storage/\n!/composer.json\n",
            PathBuf::from("/srv/.oxphpdeny"),
            RoutingModeKind::Framework,
            Some("/_security/denied.php"),
        )
        .unwrap();
        assert_eq!(
            file.summary_json(),
            serde_json::json!({
                "loaded": true,
                "rules": 5,
                "deny": 3,
                "entry": 1,
                "allow": 1,
                "fallback": "script",
            })
        );
    }

    #[test]
    fn fallback_label_shows_a_status_or_script_never_a_path() {
        assert_eq!(fallback_label(None), "404");
        assert_eq!(fallback_label(Some("403")), "403");
        assert_eq!(fallback_label(Some(" 410 ")), "410");
        // Shown as the status it answers with, not as written.
        assert_eq!(fallback_label(Some("+404")), "404");
        assert_eq!(fallback_label(Some("0403")), "403");
        assert_eq!(fallback_label(Some("/_security/denied.php")), "script");
    }

    /// `load` reads `PHP_DENY_FALLBACK` for the `/config` label: keep an
    /// inherited value out of these tests.
    fn load(root: &Path, mode: RoutingModeKind) -> Result<Option<DenyFile>, BoxError> {
        crate::config::test_env::with_env(&[("PHP_DENY_FALLBACK", None)], || {
            DenyFile::load(root, mode)
        })
    }

    #[test]
    fn load_reads_the_file_at_the_top_of_the_document_root() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join(DENY_FILE_NAME), "vendor/\n").unwrap();
        let file = load(dir.path(), RoutingModeKind::Traditional)
            .unwrap()
            .expect("file present");
        assert_eq!(
            action_at(&file, "vendor/autoload.php", false),
            Some(DenyAction::Deny)
        );
        assert_eq!(file.summary_json()["fallback"], "404");
    }

    #[test]
    fn load_without_a_file_or_document_root_is_none() {
        let dir = tempfile::TempDir::new().unwrap();
        let none = load(dir.path(), RoutingModeKind::Traditional);
        assert!(none.unwrap().is_none());
        let missing = dir.path().join("no-such-root");
        assert!(load(&missing, RoutingModeKind::Traditional)
            .unwrap()
            .is_none());
        // DOCUMENT_ROOT pointing at a file.
        let file_root = dir.path().join("file");
        std::fs::write(&file_root, "").unwrap();
        assert!(load(&file_root, RoutingModeKind::Traditional)
            .unwrap()
            .is_none());
    }

    #[test]
    fn load_rejects_an_unreadable_or_non_utf8_file() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join(DENY_FILE_NAME), b"vendor/\n\xff\n").unwrap();
        let err = load(dir.path(), RoutingModeKind::Traditional)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not valid UTF-8"), "{err}");

        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(DENY_FILE_NAME)).unwrap();
        assert!(load(dir.path(), RoutingModeKind::Traditional).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_a_dangling_symlink() {
        // A symlink whose target is gone is a broken deploy, not an absent
        // file: running without the rules would serve what they deny.
        let dir = tempfile::TempDir::new().unwrap();
        let link = dir.path().join(DENY_FILE_NAME);
        std::os::unix::fs::symlink(dir.path().join("gone"), &link).unwrap();
        let err = load(dir.path(), RoutingModeKind::Traditional)
            .unwrap_err()
            .to_string();
        assert!(err.contains(&link.display().to_string()), "{err}");
    }

    #[test]
    fn load_refuses_what_is_not_a_regular_file() {
        // Code that can write to DOCUMENT_ROOT can plant either, and the file
        // is read before privileges drop: a FIFO would hang startup in `open`
        // until something wrote to it, and a link to /dev/zero would be read
        // until memory ran out (/dev/null stands in for it here).
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::TempDir::new().unwrap();
        let fifo = dir.path().join(DENY_FILE_NAME);
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c_path` is a NUL-terminated path that outlives the call.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let root = dir.path().to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = load(&root, RoutingModeKind::Traditional);
            let _ = tx.send(result.map(|f| f.is_some()).map_err(|e| e.to_string()));
        });
        let err = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("load blocked on a FIFO")
            .unwrap_err();
        assert!(err.contains("not a regular file"), "{err}");

        let dir = tempfile::TempDir::new().unwrap();
        std::os::unix::fs::symlink("/dev/null", dir.path().join(DENY_FILE_NAME)).unwrap();
        let err = load(dir.path(), RoutingModeKind::Traditional)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not a regular file"), "{err}");
    }

    #[test]
    fn load_refuses_a_file_past_the_size_cap() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(DENY_FILE_NAME);
        // One comment line, `len` bytes long with its newline.
        let comment = |len: usize| format!("{}\n", "#".repeat(len - 1));
        std::fs::write(&path, comment(MAX_FILE_SIZE as usize)).unwrap();
        assert!(load(dir.path(), RoutingModeKind::Traditional)
            .unwrap()
            .is_some());
        std::fs::write(&path, comment(MAX_FILE_SIZE as usize + 1)).unwrap();
        let err = load(dir.path(), RoutingModeKind::Traditional)
            .unwrap_err()
            .to_string();
        assert!(err.contains("larger than"), "{err}");
    }
}
