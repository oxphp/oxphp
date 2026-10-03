//! Gitignore-style rule lists compiled onto `globset`.
//!
//! A [`RuleSet`] holds the rules of one file in source order and answers
//! "which rule excludes this path" with git's semantics: the last matching
//! rule wins, a `!` rule re-includes, and nothing inside an excluded
//! directory can be re-included. Each rule carries an action chosen per line
//! by the `classify` hook given to [`RuleSet::parse`] — the one place a
//! caller extends the syntax.
//!
//! Deliberate departures from git: a malformed line is an error instead of
//! being skipped, and so is a character class that names `/` (git accepts
//! one and never matches `/` with it) or holds a POSIX class, a backslash
//! escape, a `-` right after a range (globset would read each differently)
//! or a range that runs backwards (git reads `[z-a]` as `z`, globset
//! refuses it) or a character outside ASCII (a class matches one byte, in
//! both), and so is an escaped `/` (git reads `**\/x` as `**/x` short of
//! the top level), whitespace at the start of a pattern or an unescaped tab
//! at its end (git makes it part of the name, so the rule covers other than
//! it reads), a NUL byte (git ends the pattern there), `//` (no request
//! path has it), a rule past the [`MAX_RULES`]th and a pattern longer
//! than [`MAX_PATTERN_LEN`]. A line of spaces and tabs is blank, where git
//! reads a tab as a pattern. `{` and `}` are literal (globset would read
//! them as alternation).
//! Excluding rules match without regard to ASCII case — globset folds no
//! other letter, and nothing normalizes Unicode — and read the few letters
//! outside ASCII that a case-insensitive filesystem reads as ASCII (`ſ` as
//! `s`, `ß` as `ss`, …) as that ASCII too, in the path and in the rule.
//! `!` rules match exactly, so case only ever widens what is excluded. That
//! is why a negated class in an excluding rule may not name an ASCII letter:
//! folding would add its other case to what the class leaves out.
use std::fmt;
use std::ops::ControlFlow;

use globset::{Candidate, GlobBuilder, GlobSet, GlobSetBuilder};

/// Most rules a file may hold. A path meets the rules one by one, once for
/// each of its parents, so their count bounds what a request costs.
pub const MAX_RULES: usize = 256;

/// Longest pattern a rule may have, in bytes. What matching a rule costs —
/// the time, and the memory each thread keeps for it — grows with its
/// length.
pub const MAX_PATTERN_LEN: usize = 128;

/// Compiled rules of one file, in source order.
#[derive(Debug)]
pub struct RuleSet<A> {
    rules: Vec<Rule<A>>,
}

/// One non-blank, non-comment line.
#[derive(Debug)]
pub struct Rule<A> {
    /// 1-based line number in the source.
    pub line: usize,
    /// The line as written, minus trailing unescaped spaces.
    pub original: String,
    /// What the line compiled to, for diagnostics.
    pub glob: String,
    pub action: A,
    negated: bool,
    dir_only: bool,
    /// `glob`, compiled — less a leading `**/` when one name follows it,
    /// and then matched against the last segment of a path alone.
    matcher: GlobSet,
    /// `matcher` with its letters [`fold`]ed, when that changes it; for an
    /// excluding rule only.
    folded: Option<GlobSet>,
    name_only: bool,
}

impl<A> Rule<A> {
    /// A `!` rule: re-includes what an earlier rule excluded.
    pub fn negated(&self) -> bool {
        self.negated
    }

    /// A rule written with a trailing `/`: applies to directories only.
    pub fn dir_only(&self) -> bool {
        self.dir_only
    }

    /// Whether the rule matches `path` or, an excluding rule, `folded` —
    /// `path` with its letters [`fold`]ed, `None` when that changes nothing.
    /// The rule as written still meets the path as written, so folding only
    /// adds matches.
    fn matches(&self, path: &Target<'_>, folded: Option<&Target<'_>>) -> bool {
        let meets = |set: &GlobSet, target: &Target<'_>| {
            set.is_match_candidate(if self.name_only {
                &target.name
            } else {
                &target.path
            })
        };
        if meets(&self.matcher, path) {
            return true;
        }
        if self.negated || (self.folded.is_none() && folded.is_none()) {
            return false;
        }
        meets(
            self.folded.as_ref().unwrap_or(&self.matcher),
            folded.unwrap_or(path),
        )
    }
}

/// A path as rules meet it: whole, and its last segment alone.
struct Target<'p> {
    path: Candidate<'p>,
    name: Candidate<'p>,
}

impl<'p> Target<'p> {
    fn new(path: &'p str) -> Self {
        let name = path.rsplit_once('/').map_or(path, |(_, name)| name);
        Self {
            path: Candidate::new(path),
            name: Candidate::new(name),
        }
    }
}

/// A line that could not be compiled.
#[derive(Debug, PartialEq, Eq)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl<A> RuleSet<A> {
    /// Compile `src`. `classify(negated, rest)` receives each rule with any
    /// leading `!` removed and returns the rule's action and the pattern that
    /// follows whatever prefix it consumed — or an error for the line.
    pub fn parse(
        src: &str,
        classify: impl Fn(bool, &str) -> Result<(A, &str), String>,
    ) -> Result<Self, ParseError> {
        let src = src.strip_prefix('\u{feff}').unwrap_or(src);
        let mut rules = Vec::new();
        // git's split: one `\r` before each `\n` — or the end — is dropped.
        for (idx, raw) in src.split('\n').enumerate() {
            let line = idx + 1;
            let err = move |message: String| ParseError { line, message };
            let text = trim_trailing_spaces(raw.strip_suffix('\r').unwrap_or(raw));
            // git ends the pattern there; a UTF-16 file has one after each
            // ASCII character and passes for UTF-8.
            if text.contains('\0') {
                return Err(err("a NUL byte, which git reads as the end of the \
                     pattern — remove it, or save a UTF-16 file as UTF-8"
                    .to_string()));
            }
            // git keeps a tab here, as a pattern for a name of whitespace; the
            // line looks blank, and is read as it looks.
            if text.trim_matches([' ', '\t']).is_empty() || text.starts_with('#') {
                continue;
            }
            let (negated, rest) = match text.strip_prefix('!') {
                Some(rest) => (true, rest),
                None => (false, text),
            };
            if rules.len() == MAX_RULES {
                return Err(err(format!(
                    "more than {MAX_RULES} rules — every request is matched against each; \
                     cover a directory rather than the files in it"
                )));
            }
            let (action, pattern) = classify(negated, rest).map_err(err)?;
            if pattern.len() > MAX_PATTERN_LEN {
                return Err(err(format!(
                    "the pattern is {} bytes, more than the {MAX_PATTERN_LEN} a rule may have — \
                     shorten it with `*` or `**`",
                    pattern.len()
                )));
            }
            let (glob, dir_only) = translate(pattern, !negated).map_err(err)?;
            let (body, name_only) = match glob.strip_prefix("**/") {
                Some(name) if !name.contains('/') => (name, true),
                _ => (glob.as_str(), false),
            };
            let compile = |glob: &str| -> Result<GlobSet, ParseError> {
                GlobBuilder::new(glob)
                    .literal_separator(true)
                    .backslash_escape(true)
                    .case_insensitive(!negated)
                    .build()
                    .and_then(|glob| GlobSetBuilder::new().add(glob).build())
                    .map_err(|e| err(format!("invalid pattern {text:?}: {e}")))
            };
            let matcher = compile(body)?;
            let folded = match fold(body) {
                Some(body) if !negated => Some(compile(&body)?),
                _ => None,
            };
            rules.push(Rule {
                line,
                original: text.to_string(),
                glob,
                action,
                negated,
                dir_only,
                matcher,
                folded,
                name_only,
            });
        }
        Ok(Self { rules })
    }

    /// Rules in source order.
    pub fn iter(&self) -> impl Iterator<Item = &Rule<A>> {
        self.rules.iter()
    }

    /// Calls `f` with the rule excluding each ancestor of `path` (no leading
    /// `/`), top-down, until `f` breaks, and returns what it broke with. Each
    /// ancestor is judged as a directory and then, when `as_file` holds for
    /// it, as a file.
    ///
    /// In git the first excluded ancestor decides — it excludes everything
    /// below it, whatever later `!` rules say — so breaking on the first call
    /// gives git's answer, and [`matched_last`](Self::matched_last) gives it
    /// when no ancestor is excluded.
    pub fn excluded_ancestors<'a, B>(
        &'a self,
        path: &str,
        as_file: impl Fn(&str) -> bool,
        mut f: impl FnMut(&'a Rule<A>) -> ControlFlow<B>,
    ) -> Option<B> {
        if self.rules.is_empty() {
            return None;
        }
        for (i, _) in path.match_indices('/') {
            let ancestor = &path[..i];
            let folded = fold(ancestor);
            let folded = folded.as_deref().map(Target::new);
            let exact = Target::new(ancestor);
            if let Some(rule) = self.last_match(&exact, folded.as_ref(), true) {
                if !rule.negated {
                    if let ControlFlow::Break(out) = f(rule) {
                        return Some(out);
                    }
                }
            }
            if as_file(ancestor) {
                if let Some(rule) = self.last_match(&exact, folded.as_ref(), false) {
                    if !rule.negated {
                        if let ControlFlow::Break(out) = f(rule) {
                            return Some(out);
                        }
                    }
                }
            }
        }
        None
    }

    /// The rule that excludes `path` itself, its ancestors aside. `is_dir`
    /// says whether its last segment names a directory.
    pub fn matched_last(&self, path: &str, is_dir: bool) -> Option<&Rule<A>> {
        if path.is_empty() || self.rules.is_empty() {
            return None;
        }
        let folded = fold(path);
        let folded = folded.as_deref().map(Target::new);
        self.last_match(&Target::new(path), folded.as_ref(), is_dir)
            .filter(|rule| !rule.negated)
    }

    /// The last rule matching `path` (`folded`: see [`Rule::matches`]),
    /// skipping directory-only rules when `path` is not a directory. Each
    /// rule is tried on its own, last first: one set of all of them has
    /// to track every rule at once, which costs seconds a request — and
    /// gigabytes — once a few hundred rules can match.
    fn last_match(
        &self,
        path: &Target<'_>,
        folded: Option<&Target<'_>>,
        is_dir: bool,
    ) -> Option<&Rule<A>> {
        self.rules
            .iter()
            .rev()
            .filter(|rule| is_dir || !rule.dir_only)
            .find(|rule| rule.matches(path, folded))
    }
}

/// The ASCII a case-insensitive filesystem reads `c` as, for the letters
/// outside ASCII that it reads so: APFS serves `license.txt` for
/// `licenſe.txt`, and `straße` for `strasse`.
fn ascii_form(c: char) -> Option<&'static str> {
    Some(match c {
        '\u{17F}' => "s",              // long s
        '\u{DF}' | '\u{1E9E}' => "ss", // sharp s, small and capital
        '\u{37E}' => ";",              // Greek question mark
        '\u{1FEF}' => "`",             // Greek varia
        '\u{212A}' => "k",             // Kelvin sign
        '\u{FB00}' => "ff",            // the Latin ligatures
        '\u{FB01}' => "fi",
        '\u{FB02}' => "fl",
        '\u{FB03}' => "ffi",
        '\u{FB04}' => "ffl",
        '\u{FB05}' | '\u{FB06}' => "st",
        _ => return None,
    })
}

/// `s` with each letter [`ascii_form`] knows replaced by its ASCII, or
/// `None` when it has none.
fn fold(s: &str) -> Option<String> {
    if !s.chars().any(|c| ascii_form(c).is_some()) {
        return None;
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match ascii_form(c) {
            Some(ascii) => out.push_str(ascii),
            None => out.push(c),
        }
    }
    Some(out)
}

/// Drop trailing spaces, keeping one escaped as `\ `.
fn trim_trailing_spaces(s: &str) -> &str {
    let bytes = s.as_bytes();
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1] == b' ' {
        let space = end - 1;
        if is_escaped(s, space) {
            break;
        }
        end = space;
    }
    &s[..end]
}

/// Whether the byte at `at` follows an odd run of backslashes.
fn is_escaped(s: &str, at: usize) -> bool {
    s.as_bytes()[..at]
        .iter()
        .rev()
        .take_while(|&&b| b == b'\\')
        .count()
        % 2
        == 1
}

/// Translate a gitignore pattern into a globset glob, returning it with the
/// directory-only flag. `ignore_case` says how the glob will be compiled.
fn translate(pattern: &str, ignore_case: bool) -> Result<(String, bool), String> {
    // git accepts these silently. Whitespace at either end becomes part of
    // the name, so the rule covers other than it reads — an indented `> x`
    // denies ` > x` — and no request path has `//`.
    if pattern.starts_with([' ', '\t']) {
        return Err(format!(
            "pattern {pattern:?} starts with whitespace, which would be part of the name — \
             start the rule in the first column, or escape it with `\\` if the name starts \
             with it"
        ));
    }
    if pattern.ends_with('\t') && !is_escaped(pattern, pattern.len() - 1) {
        return Err(format!(
            "pattern {pattern:?} ends in a tab, which would be part of the name — \
             remove it, or escape it with `\\` if the name ends with it"
        ));
    }
    if pattern.contains("//") {
        return Err(format!(
            "pattern {pattern:?} has `//`, which no request path does"
        ));
    }
    // git reads `**\/x` as `**/x` short of the top level, globset as two `*`
    // and `/x`; elsewhere the escape only hides a `/` from the checks above
    // and from anchoring.
    if pattern
        .match_indices('/')
        .any(|(at, _)| is_escaped(pattern, at))
    {
        return Err(format!(
            "pattern {pattern:?} escapes a `/`, which never needs it — write `/`"
        ));
    }
    let (body, dir_only) = match pattern.strip_suffix('/') {
        Some(body) => (body, true),
        None => (pattern, false),
    };
    // A separator anywhere but at the end anchors the pattern to the root;
    // without one it matches at any depth.
    let anchored = body.contains('/');
    let body = if anchored {
        body.strip_prefix('/').unwrap_or(body)
    } else {
        body
    };
    if body.is_empty() {
        return Err(format!("pattern {pattern:?} names no path"));
    }
    let mut glob = String::with_capacity(body.len() + 4);
    if !anchored {
        glob.push_str("**/");
    }
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                glob.push(c);
                glob.extend(chars.next());
            }
            '{' | '}' => {
                glob.push('\\');
                glob.push(c);
            }
            '[' => copy_class(&mut chars, &mut glob, ignore_case)?,
            // git reads any run of `*` as `**`; globset reads each pair.
            '*' if chars.as_str().starts_with("**") => {
                glob.push_str("**");
                while chars.as_str().starts_with('*') {
                    chars.next();
                }
            }
            _ => glob.push(c),
        }
    }
    Ok((glob, dir_only))
}

/// Copy a character class whose `[` was just read, as globset parses it: a
/// `!` or `^` first negates it, and a `]` or `-` first is literal. git's
/// classes never match the separator; globset's do, so a negated class gains
/// `/` and one that names `/` is refused. So is what git reads inside a class
/// and globset does not — a POSIX class, a backslash escape, or a `-` right
/// after a range, which git takes literally and globset adds to the range —
/// and a character outside ASCII, since both match a class against one byte
/// and would take such a character's bytes as members of their own. Where
/// case is ignored a negated class may not name an ASCII letter: folding
/// case would narrow it, `(?i)[^i]` missing `I` too. An unclosed class is
/// copied as is for globset to reject.
fn copy_class(
    chars: &mut std::str::Chars<'_>,
    glob: &mut String,
    ignore_case: bool,
) -> Result<(), String> {
    let start = glob.len();
    glob.push('[');
    let mut rest = chars.clone();
    let negated = matches!(rest.next(), Some('!' | '^'));
    if negated {
        glob.extend(chars.next());
    }
    let mut first = true;
    // A `-` after a character opens a range; one left open at the close is
    // a literal `-`, and the added `/` goes before it, not into the range.
    let mut open_range = false;
    // The last character ended a range.
    let mut closed_range = false;
    // The last character, and whether any so far is an ASCII letter.
    let mut last = '\0';
    let mut cased = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                return Err("a backslash inside a character class is not supported — \
                     put `]` first and `-` last to match them"
                    .to_string());
            }
            '[' if opens_posix_class(chars.as_str()) => {
                return Err("POSIX character classes such as `[:digit:]` are not \
                     supported — write the range, e.g. `[0-9]`"
                    .to_string());
            }
            ']' if !first => {
                if negated {
                    let at = glob.len() - usize::from(open_range);
                    glob.insert(at, '/');
                }
                glob.push(']');
                let class = &glob[start..];
                // globset's own reading of the class, so the two cannot differ.
                let names_separator = GlobBuilder::new(class)
                    .build()
                    .is_ok_and(|g| g.compile_matcher().is_match("/"));
                if names_separator {
                    return Err(format!(
                        "{class}: a character class cannot match `/` — write the separator outside it"
                    ));
                }
                if cased {
                    return Err(format!(
                        "{class}: a negated class cannot name an ASCII letter in a rule that \
                         ignores case — match any character there and re-include the \
                         exception with a `!` rule, which matches case exactly"
                    ));
                }
                return Ok(());
            }
            '-' if !first => {
                if closed_range && !chars.as_str().starts_with(']') {
                    return Err("a `-` right after a range is read differently by git \
                         and globset — move the literal `-` to the start or the end"
                        .to_string());
                }
                if open_range && last > '-' {
                    return Err(backwards(last, '-'));
                }
                closed_range = open_range;
                open_range = !open_range;
            }
            _ => {
                if !c.is_ascii() {
                    return Err(format!(
                        "a character class matches a single byte and `{c}` takes {} — \
                         put `*` in place of a character outside ASCII",
                        c.len_utf8()
                    ));
                }
                if open_range && last > c {
                    return Err(backwards(last, c));
                }
                if negated && ignore_case {
                    cased |= names_ascii_letter(if open_range { last } else { c }, c);
                }
                last = c;
                closed_range = open_range;
                open_range = false;
            }
        }
        glob.push(c);
        first = false;
    }
    Ok(())
}

/// The error for a range `lo-hi` with `lo` above `hi`.
fn backwards(lo: char, hi: char) -> String {
    format!(
        "range `{lo}-{hi}` runs backwards, which git reads as `{lo}` alone — write that, or \
         put the lower end first"
    )
}

/// Whether what follows a `[` inside a class opens a POSIX class, as git
/// reads one: `[:` closed by `:]` before the next `]`. Otherwise the `[` is
/// an ordinary member.
fn opens_posix_class(rest: &str) -> bool {
    rest.strip_prefix(':')
        .and_then(|name| name.find(']').map(|end| &name[..end]))
        .is_some_and(|name| name.ends_with(':'))
}

/// Whether `lo..=hi` holds an ASCII letter, the only kind globset folds.
fn names_ascii_letter(lo: char, hi: char) -> bool {
    (lo <= 'Z' && hi >= 'A') || (lo <= 'z' && hi >= 'a')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every rule gets the same unit action; `!` passes through.
    fn plain(_negated: bool, rest: &str) -> Result<((), &str), String> {
        Ok(((), rest))
    }

    fn rules(src: &str) -> RuleSet<()> {
        RuleSet::parse(src, plain).unwrap()
    }

    /// The rule that excludes `path`, judged as git judges it: ancestors as
    /// directories, top-down, then `path` itself.
    fn matched<'a>(set: &'a RuleSet<()>, path: &str, is_dir: bool) -> Option<&'a Rule<()>> {
        set.excluded_ancestors(path, |_| false, ControlFlow::Break)
            .or_else(|| set.matched_last(path, is_dir))
    }

    /// Line number of the rule excluding `path`, or `None`.
    fn hit(set: &RuleSet<()>, path: &str, is_dir: bool) -> Option<usize> {
        matched(set, path, is_dir).map(|r| r.line)
    }

    /// `(path, is_dir, excluded?)` rows against one rule file.
    fn assert_table(src: &str, rows: &[(&str, bool, bool)]) {
        let set = rules(src);
        for &(path, is_dir, excluded) in rows {
            assert_eq!(
                matched(&set, path, is_dir).is_some(),
                excluded,
                "rules {src:?}, path {path:?}, is_dir {is_dir}"
            );
        }
    }

    #[test]
    fn unanchored_name_matches_at_any_depth() {
        assert_table(
            "vendor",
            &[
                ("vendor", false, true),
                ("vendor", true, true),
                ("vendor/autoload.php", false, true),
                ("lib/vendor/x", false, true),
                ("vendors", false, false),
            ],
        );
    }

    #[test]
    fn leading_or_middle_slash_anchors_to_root() {
        assert_table(
            "/vendor\nlib/cache",
            &[
                ("vendor", false, true),
                ("lib/vendor", false, false),
                ("lib/cache", false, true),
                ("lib/cache/x", false, true),
                ("src/lib/cache", false, false),
            ],
        );
    }

    #[test]
    fn trailing_slash_and_the_admin_table() {
        // The table in the public documentation: `admin/` misses the bare
        // `/admin`, `admin` hits all four, `admin/*` and `admin/x` only what
        // lies below.
        let paths = [
            ("admin", false),
            ("admin", true),
            ("admin/x", false),
            ("admin/x/y", false),
        ];
        for (src, expected) in [
            ("admin/", [false, true, true, true]),
            ("admin", [true, true, true, true]),
            ("/admin", [true, true, true, true]),
            ("admin/*", [false, false, true, true]),
            ("admin/x", [false, false, true, true]),
        ] {
            let set = rules(src);
            for ((path, is_dir), want) in paths.iter().zip(expected) {
                assert_eq!(
                    matched(&set, path, *is_dir).is_some(),
                    want,
                    "rule {src:?}, path {path:?}, is_dir {is_dir}"
                );
            }
        }
        // Unanchored vs anchored below the root.
        assert!(matched(&rules("admin"), "foo/admin", false).is_some());
        assert!(matched(&rules("/admin"), "foo/admin", false).is_none());
    }

    #[test]
    fn double_star_forms() {
        assert_table(
            "**/logs\nbuild/**\na/**/b\n**/c/d",
            &[
                ("logs", false, true),
                ("x/y/logs", false, true),
                ("build", true, false),
                ("build/out.js", false, true),
                ("a/b", false, true),
                ("a/x/y/b", false, true),
                ("c/d", false, true),
                ("x/y/c/d", false, true),
                ("x/d", false, false),
            ],
        );
        // `**` inside a segment is two `*`: it stays within the segment.
        assert_table(
            "/a**b",
            &[
                ("ab", false, true),
                ("axxb", false, true),
                ("a/b", false, false),
            ],
        );
    }

    #[test]
    fn a_longer_run_of_stars_is_a_double_star() {
        // git reads any run of `*` as `**`; globset would read `***` as three
        // `*`, each within one segment.
        assert_table(
            "***/secret.txt",
            &[
                ("secret.txt", false, true),
                ("a/secret.txt", false, true),
                ("a/b/secret.txt", false, true),
            ],
        );
        assert_table(
            "/a/****/b\n/x***y",
            &[
                ("a/b", false, true),
                ("a/x/b", false, true),
                ("a/x/y/b", false, true),
                ("xzy", false, true),
                ("x/y", false, false),
            ],
        );
    }

    #[test]
    fn stray_whitespace_and_a_double_slash_are_errors() {
        // git accepts each. Whitespace at either end becomes part of the name,
        // so the rule would cover other than it reads, and `//` matches no
        // path — globset would even read `**//` as every directory.
        for (src, needle) in [
            (" > /storage/invoices/", "whitespace"),
            ("  vendor/", "whitespace"),
            ("\tvendor", "whitespace"),
            ("! vendor", "whitespace"),
            ("vendor/\t", "tab"),
            ("vendor \t", "tab"),
            ("//vendor", "`//`"),
            ("vendor//lib/", "`//`"),
            ("vendor//", "`//`"),
            ("**//", "`//`"),
        ] {
            let err = RuleSet::parse(src, plain).unwrap_err();
            assert!(err.message.contains(needle), "{src:?}: {}", err.message);
        }
        // An escaped tab is meant; a line of spaces and tabs is blank.
        let set = rules("vendor\\\t\n \t \n/a");
        assert_eq!(set.iter().count(), 2);
    }

    #[test]
    fn a_nul_byte_is_an_error() {
        // git ends a pattern at a NUL, so `vendor\0xyz` covers `vendor`; kept,
        // the rule would cover only a name with a NUL in it. A UTF-16 file is
        // valid UTF-8 with a NUL after each ASCII character, and none of its
        // rules would cover anything.
        for (src, line) in [
            ("vendor\0xyz", 1),
            ("/a\n# x\0", 2),
            ("v\0e\0n\0d\0o\0r\0\n\0", 1),
        ] {
            let err = RuleSet::parse(src, plain).unwrap_err();
            assert_eq!(err.line, line, "{src:?}");
            assert!(err.message.contains("NUL"), "{src:?}: {}", err.message);
        }
    }

    #[test]
    fn an_escaped_slash_is_an_error() {
        // git reads `**\/x` as `**/x` short of the top level, globset as two
        // `*` and `/x`; `\/vendor` and `/a/\/b` match no request path in
        // either. A separator never needs the escape.
        for src in [
            "**\\/secret.txt",
            "\\/vendor",
            "/a/\\/b",
            "a\\/b",
            "vendor\\/",
            "!a\\/b",
        ] {
            let err = RuleSet::parse(src, plain).unwrap_err();
            assert!(
                err.message.contains("write `/`"),
                "{src:?}: {}",
                err.message
            );
        }
        // An escaped backslash leaves the separator after it alone.
        assert_table("/a\\\\/b", &[("a\\/b", false, true)]);
    }

    #[test]
    fn wildcards_stay_inside_one_segment() {
        assert_table(
            "/x/*.sql\n/y/?.txt",
            &[
                ("x/dump.sql", false, true),
                ("x/sub/dump.sql", false, false),
                ("y/a.txt", false, true),
                ("y/ab.txt", false, false),
                ("y/a/.txt", false, false),
            ],
        );
    }

    #[test]
    fn character_classes() {
        assert_table(
            "/[a-c].txt\n/[!0]1\n/[^9]2",
            &[
                ("b.txt", false, true),
                ("d.txt", false, false),
                ("a1", false, true),
                ("01", false, false),
                ("a2", false, true),
                ("92", false, false),
            ],
        );
    }

    #[test]
    fn last_matching_rule_wins_and_negation_reincludes() {
        let set = rules("*.log\n!keep.log\n/keep.log");
        assert_eq!(hit(&set, "a.log", false), Some(1));
        // Line 3 re-excludes what line 2 re-included.
        assert_eq!(hit(&set, "keep.log", false), Some(3));
        assert_eq!(hit(&set, "sub/keep.log", false), None);
    }

    #[test]
    fn negation_cannot_reach_inside_an_excluded_directory() {
        // git: "It is not possible to re-include a file if a parent directory
        // of that file is excluded."
        let set = rules("private/\n!private/ok.txt");
        assert_eq!(hit(&set, "private/ok.txt", false), Some(1));
        // Excluding the contents instead of the directory leaves room for `!`.
        let set = rules("/private/*\n!/private/ok.txt");
        assert_eq!(hit(&set, "private/ok.txt", false), None);
        assert_eq!(hit(&set, "private/other.txt", false), Some(1));
    }

    #[test]
    fn allowlist_style_file() {
        // "Deny everything but": `!dir/` alone re-includes the directory,
        // not what is in it — as in git, `!dir/**` is needed as well.
        let set = rules("*\n!/index.php\n!/assets/\n!/assets/**");
        assert_eq!(hit(&set, "index.php", false), None);
        assert_eq!(hit(&set, "assets/app.js", false), None);
        assert_eq!(hit(&set, "x.php", false), Some(1));
        assert_eq!(hit(&set, "lib/x.php", false), Some(1));
        let partial = rules("*\n!/assets/");
        assert_eq!(hit(&partial, "assets/app.js", false), Some(1));
    }

    #[test]
    fn escapes_and_trailing_spaces() {
        let set = rules("\\#hash\n\\!bang\nspace\\ \ntrail   ");
        assert_eq!(hit(&set, "#hash", false), Some(1));
        assert_eq!(hit(&set, "!bang", false), Some(2));
        assert_eq!(hit(&set, "space ", false), Some(3));
        assert_eq!(hit(&set, "space", false), None);
        assert_eq!(hit(&set, "trail", false), Some(4));
        assert_eq!(set.iter().nth(3).unwrap().original, "trail");
    }

    #[test]
    fn negated_classes_never_match_a_separator() {
        // As in git: a class stands for one character of a name, never `/`.
        assert_table(
            "/release[!-]*.zip\n/backup[^_]*\n/x[!0-]y\n/r[!0-2]s\n/q[!]]z",
            &[
                ("release/secret.zip", false, false),
                ("release1.zip", false, true),
                ("backup/readme.txt", false, false),
                ("backup1", false, true),
                // A trailing `-` stays literal next to the added `/`.
                ("xby", false, true),
                ("x-y", false, false),
                ("x/y", false, false),
                ("r3s", false, true),
                ("r1s", false, false),
                ("r/s", false, false),
                // A leading `]` stays literal.
                ("qaz", false, true),
                ("q]z", false, false),
                ("q/z", false, false),
            ],
        );
    }

    #[test]
    fn a_class_that_names_the_separator_is_an_error() {
        // git's classes never match `/`; one that names it would match it
        // here, so the line is refused instead of silently diverging.
        for src in ["/release[-/]*.zip", "/x[+-0]y", "/y[/]"] {
            let err = RuleSet::parse(src, plain).unwrap_err();
            assert!(
                err.message.contains("cannot match `/`"),
                "{src:?}: {}",
                err.message
            );
        }
        // A negated class gets the separator excluded instead.
        assert!(RuleSet::parse("/z[!/]", plain).is_ok());
    }

    #[test]
    fn a_class_globset_would_read_differently_is_an_error() {
        // globset has neither POSIX classes nor escapes inside a class: it
        // reads `[[:digit:]]` as the set `[:digt` and then a literal `]`, and
        // `[\]]` as `\` and then `]`. git reads each as one class.
        for (src, needle) in [
            ("*.log.[[:digit:]]", "POSIX"),
            ("/x[![:space:]]", "POSIX"),
            ("/x[[::]y", "POSIX"),
            ("/a[\\]]b", "backslash"),
            ("/a[x\\-]b", "backslash"),
        ] {
            let err = RuleSet::parse(src, plain).unwrap_err();
            assert!(err.message.contains(needle), "{src:?}: {}", err.message);
        }
        // `[` or `:` alone is an ordinary member, and so is a `[:` that no
        // `:]` closes before the next `]`, as in git.
        assert_table(
            "/a[[]b\n/c[:]d\n/e[[:]f\n/g[x[:]h\n/i[[:x]j",
            &[
                ("a[b", false, true),
                ("c:d", false, true),
                ("e[f", false, true),
                ("e:f", false, true),
                ("gxh", false, true),
                ("g[h", false, true),
                ("i[j", false, true),
                ("ixj", false, true),
            ],
        );
    }

    #[test]
    fn a_negated_class_naming_an_ascii_letter_is_an_error_where_case_is_ignored() {
        // Folding case narrows a negated class — `(?i)[^i]` misses `I` too —
        // so `/[!i]*.php` would run `/Info.php`. The exception belongs in a
        // `!` rule, which matches exactly.
        for src in [
            "/[!i]*.php",
            "/x[^a]",
            "/[!a-z]*",
            "/[!0-9A]x",
            "/[!0-~]x",
            "/[!@-A]x",
            "/[!Z-_]x",
            "/[!z-~]x",
        ] {
            let err = RuleSet::parse(src, plain).unwrap_err();
            assert!(err.message.contains("`!` rule"), "{src:?}: {}", err.message);
        }
        // Case cannot touch a class with no letter, nor a `!` rule.
        for src in ["/[!0-9_]x", "/[![-`]x", "/[!{-~]x", "!/[!i]*"] {
            assert!(RuleSet::parse(src, plain).is_ok(), "{src:?}");
        }
        assert_table(
            "/*.php\n!/info.php",
            &[
                ("Info.php", false, true),
                ("Info.PHP", false, true),
                ("info.php", false, false),
            ],
        );
    }

    #[test]
    fn a_class_cannot_hold_a_character_outside_ascii() {
        // A class matches one byte, as in git, so `[äÄ]` is the set of the
        // bytes of both letters: `/[äÄ]dmin` covers neither `ädmin` nor
        // `Ädmin`, and `/[äÄ]*` covers `öffnungszeiten`, which starts with the
        // same byte.
        for src in [
            "/[äÄ]dmin",
            "/[äÄ]*",
            "/x[!é]",
            "/x[!0-é]",
            "/x[!{-é]",
            "/x[!é-ö]",
            "/x[!ß]",
            "/x[!\u{212A}]",
            "!/x[ä]",
        ] {
            let err = RuleSet::parse(src, plain).unwrap_err();
            assert!(
                err.message.contains("outside ASCII"),
                "{src:?}: {}",
                err.message
            );
        }
        // `?` matches one byte too; only `*` stands for such a letter.
        assert_table(
            "/release?.zip\n/caf*.txt",
            &[
                ("releasex.zip", false, true),
                ("releaseé.zip", false, false),
                ("café.txt", false, true),
                ("cafe\u{301}.txt", false, true),
            ],
        );
    }

    #[test]
    fn a_dash_right_after_a_range_is_an_error() {
        // git reads it as a literal `-`; globset extends the range with it,
        // so `[a-b-c]` would be `[a-c]`.
        for src in ["/x[a-b-c]", "/x[!a-b-c]", "/x[+---0]"] {
            let err = RuleSet::parse(src, plain).unwrap_err();
            assert!(
                err.message.contains("right after a range"),
                "{src:?}: {}",
                err.message
            );
        }
        // Before the closing `]` it is literal in both.
        assert_table(
            "/y[a-b-]\n/z[!0-1-]",
            &[
                ("y-", false, true),
                ("yb", false, true),
                ("yc", false, false),
                ("z-", false, false),
                ("z2", false, true),
                ("z/", false, false),
            ],
        );
    }

    #[test]
    fn a_backwards_range_is_an_error() {
        // git reads `[z-a]` as `z` alone and `[a--]` as `a`; globset refuses
        // both.
        for src in ["/x[z-a]", "/x[a--]", "/x[!z-a]", "!/x[9-0]"] {
            let err = RuleSet::parse(src, plain).unwrap_err();
            assert!(
                err.message.contains("backwards"),
                "{src:?}: {}",
                err.message
            );
        }
        // A one-character range, and a `-` closing one from below, are fine.
        assert_table(
            "/x[a-a]\n/y[+--]",
            &[
                ("xa", false, true),
                ("xb", false, false),
                ("y,", false, true),
                ("y-", false, true),
            ],
        );
    }

    #[test]
    fn a_rule_past_the_cap_is_an_error() {
        // Comments and blank lines do not count.
        let at_cap = "/a\n".repeat(MAX_RULES);
        let set = rules(&format!("# x\n\n{at_cap}# y\n"));
        assert_eq!(set.iter().count(), MAX_RULES);
        let err = RuleSet::parse(&format!("# x\n{at_cap}/b"), plain).unwrap_err();
        assert_eq!(err.line, MAX_RULES + 2);
        assert!(
            err.message.contains(&MAX_RULES.to_string()),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_pattern_past_the_length_cap_is_an_error() {
        let at_cap = format!("/{}", "a".repeat(MAX_PATTERN_LEN - 1));
        // The `!` before it does not count.
        assert_eq!(rules(&format!("{at_cap}\n!{at_cap}")).iter().count(), 2);
        let err = RuleSet::parse(&format!("/ok\n{at_cap}b"), plain).unwrap_err();
        assert_eq!(err.line, 2);
        assert!(
            err.message.contains(&MAX_PATTERN_LEN.to_string()),
            "{}",
            err.message
        );
    }

    #[test]
    fn excluding_rules_read_letters_a_filesystem_folds_to_ascii_as_ascii() {
        // A case-insensitive filesystem reads each of these as ASCII — APFS
        // serves `license.txt` for `licenſe.txt` — so excluding rules do too.
        assert_table(
            "/license.txt\n/strasse\n/x;y\n/a`b\n*.bak\n/stuff\n/fi\n/fl\n/ffi\n/ffl\n/st\n\
             /assets/\nsecret.txt",
            &[
                ("licen\u{17F}e.txt", false, true),
                ("stra\u{DF}e", false, true),
                ("STRA\u{1E9E}E", false, true),
                ("x\u{37E}y", false, true),
                ("a\u{1FEF}b", false, true),
                ("backup.ba\u{212A}", false, true),
                ("stu\u{FB00}", false, true),
                ("\u{FB01}", false, true),
                ("\u{FB02}", false, true),
                ("\u{FB03}", false, true),
                ("\u{FB04}", false, true),
                ("\u{FB05}", false, true),
                ("\u{FB06}", false, true),
                // Below an excluded directory, and by name at any depth.
                ("a\u{17F}\u{17F}ets/app.js", false, true),
                ("x/\u{17F}ecret.txt", false, true),
                // Any other letter is still matched only as written.
                ("licen\u{15F}e.txt", false, false),
                ("\u{17F}", false, false),
            ],
        );
        // The letters fold in a rule as well, escaped or not.
        assert_table(
            "/stra\u{DF}e/\n/\\\u{212A}ey",
            &[
                ("strasse/x", false, true),
                ("STRASSE/x", false, true),
                ("stra\u{DF}e/x", false, true),
                ("stra\u{1E9E}e/x", false, true),
                ("key", false, true),
            ],
        );
        // What a rule matched as written it still matches: `?` takes one byte
        // of `ſ`, which folds to one.
        assert_table(
            "/a??b\n/\u{DF}??",
            &[("a\u{17F}b", false, true), ("\u{DF}\u{17F}", false, true)],
        );
        // `!` rules match exactly, so folding only ever widens a denial.
        assert_table(
            "*.txt\n!/license.txt\n!/stra\u{DF}e.txt",
            &[
                ("license.txt", false, false),
                ("licen\u{17F}e.txt", false, true),
                ("stra\u{DF}e.txt", false, false),
                ("strasse.txt", false, true),
            ],
        );
    }

    #[test]
    fn braces_are_literal() {
        assert_table(
            "/{a,b}.txt\n/[{]x",
            &[
                ("{a,b}.txt", false, true),
                ("a.txt", false, false),
                // Inside a class a brace is already literal; escaping it
                // there would add `\` to the class.
                ("{x", false, true),
                ("\\x", false, false),
            ],
        );
    }

    #[test]
    fn bom_crlf_comments_blank_lines() {
        // As in git, a last line without a newline loses its `\r` too.
        let set = rules("\u{feff}# comment\r\n\r\n/a\r\n  \r\n/b\r\n/c\r");
        assert_eq!(hit(&set, "a", false), Some(3));
        assert_eq!(hit(&set, "b", false), Some(5));
        assert_eq!(hit(&set, "c", false), Some(6));
        assert_eq!(set.iter().count(), 3);
    }

    #[test]
    fn case_is_ignored_for_ascii_letters_alone() {
        // globset folds ASCII case only, and nothing normalizes Unicode: any
        // other letter, and a decomposed `é`, match only as written.
        assert_table(
            "/ädmin\n/café.txt",
            &[
                ("ädmin", false, true),
                ("äDMIN", false, true),
                ("Ädmin", false, false),
                ("café.txt", false, true),
                ("cafe\u{301}.txt", false, false),
            ],
        );
    }

    #[test]
    fn excluding_rules_ignore_case_and_re_including_rules_do_not() {
        // Case only ever widens what is excluded: `x.PHP` runs as PHP, and a
        // case-insensitive filesystem serves `VENDOR/x` as `vendor/x`.
        assert_table(
            "vendor/\n*.php\n!/ok.php",
            &[
                ("vendor/x", false, true),
                ("VENDOR/x", false, true),
                ("x.PHP", false, true),
                ("ok.php", false, false),
                ("OK.php", false, true),
                ("ok.PHP", false, true),
            ],
        );
    }

    #[test]
    fn empty_path_and_empty_set_match_nothing() {
        assert_eq!(hit(&rules("*"), "", true), None);
        assert_eq!(hit(&rules("# only a comment"), "a", false), None);
    }

    #[test]
    fn errors_carry_the_line_number() {
        let err = RuleSet::parse("/ok\n/bad[a-", plain).unwrap_err();
        assert_eq!(err.line, 2);
        assert!(err.message.contains("/bad[a-"), "{}", err.message);

        for src in ["/", "!", "!/"] {
            let err = RuleSet::parse(src, plain).unwrap_err();
            assert_eq!(err.line, 1, "{src:?}");
        }

        // A `fn`, not a closure: `classify` must be generic over the input
        // lifetime, and closures cannot be.
        fn refuse_nope(_: bool, rest: &str) -> Result<((), &str), String> {
            match rest {
                "nope" => Err("refused".to_string()),
                _ => Ok(((), rest)),
            }
        }
        let err = RuleSet::parse("/a\n\nnope", refuse_nope).unwrap_err();
        assert_eq!(
            err,
            ParseError {
                line: 3,
                message: "refused".to_string()
            }
        );
    }

    #[test]
    fn rule_reports_its_translation() {
        let set = rules("vendor/\n/composer.*\n!x");
        let got: Vec<_> = set
            .iter()
            .map(|r| (r.line, r.glob.as_str(), r.dir_only(), r.negated()))
            .collect();
        assert_eq!(
            got,
            [
                (1, "**/vendor", true, false),
                (2, "composer.*", false, false),
                (3, "**/x", false, true),
            ]
        );
    }
}
