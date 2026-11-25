//! Removes the user's home directory and OS user name from text that goes
//! into the log, a crash file or a problem report.

use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// What to remove. [`Scrubber::current`] is this machine's; tests build
/// their own.
#[derive(Clone, Debug, Default)]
pub struct Scrubber {
    /// Home directory spellings, longest first (`\` and `/` variants).
    homes: Vec<String>,
    user: Option<String>,
}

impl Scrubber {
    pub fn new(home: Option<&str>, user: Option<&str>) -> Scrubber {
        let mut homes = Vec::new();
        if let Some(h) = home {
            let h = h.trim_end_matches(['/', '\\']);
            // A home of "/" or "C:" would turn every path into "~".
            if h.chars().filter(|c| c.is_alphanumeric()).count() >= 2 {
                for v in [h.to_string(), h.replace('\\', "/"), h.replace('/', "\\")] {
                    if !homes.contains(&v) {
                        homes.push(v);
                    }
                }
            }
        }
        homes.sort_by_key(|h| std::cmp::Reverse(h.len()));
        let user = user
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .map(str::to_string);
        Scrubber { homes, user }
    }

    /// This machine's home directory and user name (in tests, the test
    /// identity once [`use_test_identity`] ran on the thread).
    pub fn current() -> &'static Scrubber {
        if let Some(s) = test_identity() {
            return s;
        }
        static S: OnceLock<Scrubber> = OnceLock::new();
        S.get_or_init(|| {
            let home = directories::BaseDirs::new()
                .map(|d| d.home_dir().to_string_lossy().into_owned())
                .or_else(|| std::env::var("HOME").ok());
            let user = ["USER", "USERNAME", "LOGNAME"]
                .iter()
                .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
                .or_else(|| {
                    // The last part of the home directory is the user name
                    // on every desktop OS we ship for.
                    home.as_deref().and_then(|h| {
                        h.trim_end_matches(['/', '\\'])
                            .rsplit(['/', '\\'])
                            .next()
                            .map(str::to_string)
                    })
                });
            Scrubber::new(home.as_deref(), user.as_deref())
        })
    }

    /// `text` with the home directory replaced by `~` and the user name by
    /// `<user>` (both case-insensitive).
    pub fn scrub(&self, text: &str) -> String {
        let mut s = text.to_string();
        for h in &self.homes {
            s = replace_ci(&s, h, "~", false);
        }
        if let Some(u) = &self.user {
            // Very short names would match inside ordinary words; replace
            // those only where they stand alone.
            let whole_word = u.chars().count() < 3;
            s = replace_ci(&s, u, "<user>", whole_word);
        }
        s
    }

    /// Text that leaves the machine: known folders removed (see
    /// [`know_dir`]), then home and user name, then the folders of any
    /// other path.
    pub fn clean(&self, text: &str) -> String {
        redact_dirs(&self.scrub(&strip_known(text)))
    }
}

#[cfg(test)]
thread_local! {
    static TEST_IDENTITY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Home directory of the test identity: no real machine has it.
#[cfg(test)]
pub const TEST_HOME: &str = "/home/zqtester-home-5c7a";
/// User name of the test identity.
#[cfg(test)]
pub const TEST_USER: &str = "zqtester5c7a";

/// From now on this test (its thread) scrubs [`TEST_HOME`] and
/// [`TEST_USER`] instead of the machine's, so assertions about what leaves
/// the machine do not depend on who runs the tests.
#[cfg(test)]
pub fn use_test_identity() {
    TEST_IDENTITY.with(|t| t.set(true));
}

#[cfg(test)]
fn test_identity() -> Option<&'static Scrubber> {
    static T: OnceLock<Scrubber> = OnceLock::new();
    TEST_IDENTITY
        .with(std::cell::Cell::get)
        .then(|| T.get_or_init(|| Scrubber::new(Some(TEST_HOME), Some(TEST_USER))))
}

#[cfg(not(test))]
fn test_identity() -> Option<&'static Scrubber> {
    None
}

/// Shorthand for the machine scrubber.
pub fn scrub(text: &str) -> String {
    Scrubber::current().scrub(text)
}

/// Folders removed from absolute paths, for text that leaves the machine:
/// `/mnt/clients/acme/part.step` becomes `…/part.step`. Source locations
/// (`….rs`, optionally with `:line:col`) are kept, since they point into
/// the code, not the user's files.
///
/// Folder names may hold spaces (several in a row too), brackets, quotes
/// and other punctuation, so when in doubt this removes too much rather
/// than too little: a path runs on through the following words up to the
/// last one that still has a separator.
pub fn redact_dirs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut prev: Option<char> = None;
    while i < text.len() {
        let rest = &text[i..];
        let boundary = prev.is_none_or(|c| c.is_whitespace() || "\"'([{<=:,;`".contains(c));
        if boundary && let Some(len) = path_len(rest) {
            let path = &rest[..len];
            out.push_str(&redact_one(path));
            prev = path.chars().last();
            i += len;
            continue;
        }
        let c = rest.chars().next().expect("non-empty");
        out.push(c);
        prev = Some(c);
        i += c.len_utf8();
    }
    out
}

fn starts_path(s: &str) -> bool {
    // A folder already replaced by `…` (see [`strip_known`]) may still be
    // followed by subfolders.
    if s.starts_with("…/") || s.starts_with("…\\") {
        return true;
    }
    let b = s.as_bytes();
    match b {
        [b'/', c, ..] => *c != b'/' && !c.is_ascii_whitespace(),
        [b'~', b'/' | b'\\', ..] => true,
        [b'\\', b'\\', c, ..] => c.is_ascii_alphanumeric(),
        [d, b':', b'\\' | b'/', ..] => d.is_ascii_alphabetic(),
        _ => false,
    }
}

/// Words after a path that are still looked at for a later separator.
const MAX_PATH_WORDS: usize = 8;

/// Byte length of the absolute path at the start of `s`, if one starts
/// there. Words are separated by runs of spaces; the path runs on to the
/// last of the next [`MAX_PATH_WORDS`] words that holds a separator
/// (`/mnt/Acme Corp (NDA)/x.step`), and ends with the file name after that
/// separator, which stops at a quote, closing bracket, comma or semicolon,
/// and drops trailing sentence punctuation.
fn path_len(s: &str) -> Option<usize> {
    if !starts_path(s) {
        return None;
    }
    let word_len = |t: &str| {
        t.find(|c: char| c.is_whitespace() || "\"`|".contains(c))
            .unwrap_or(t.len())
    };
    // The last word with a separator: (start, end).
    let mut last = (0, word_len(s));
    let mut probe = last.1;
    for _ in 0..MAX_PATH_WORDS {
        let after = &s[probe..];
        let spaces = after.len() - after.trim_start_matches(' ').len();
        if spaces == 0 {
            break;
        }
        let start = probe + spaces;
        let len = word_len(&s[start..]);
        if len == 0 {
            break;
        }
        let word = &s[start..start + len];
        // Another path or a URL starts: this one ended before it.
        let bare = word.trim_start_matches(['\'', '"', '(', '[', '{', '<', '`']);
        if starts_path(bare) || word.contains("://") {
            break;
        }
        probe = start + len;
        if word.contains(['/', '\\']) {
            last = (start, probe);
        }
    }
    let (start, end) = last;
    let sep = s[start..end].rfind(['/', '\\'])?;
    let name_start = start + sep + 1;
    let name = &s[name_start..end];
    let mut end = name_start
        + name
            .find(|c: char| "'\")]},;".contains(c))
            .unwrap_or(name.len());
    // Sentence punctuation after a path is not part of it.
    while end > name_start && s[..end].ends_with(['.', ':', '!', '?']) {
        end -= 1;
    }
    Some(end)
}

fn redact_one(path: &str) -> String {
    // Only a source location on its own is kept: a candidate glued from
    // several words, or holding a quote, bracket, comma or semicolon, may
    // start with another path (`/mnt/acme/x.step,src/main.rs`).
    let code = {
        let p = path.trim_end_matches(|c: char| c.is_ascii_digit() || c == ':');
        p.ends_with(".rs") && !p.contains(|c: char| c.is_whitespace() || "'\"`()[]{},;".contains(c))
    };
    if code {
        return path.to_string();
    }
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    match parts.last() {
        Some(last) if parts.len() >= 2 => format!("…/{last}"),
        _ => path.to_string(),
    }
}

// ------------------------------------------------------------ known folders

/// Spellings of folders the app works with (data and config folders, the
/// folders of opened, saved, imported and exported files), longest first.
fn known() -> &'static Mutex<Vec<String>> {
    static K: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    K.get_or_init(Default::default)
}

/// Remember a folder the app works with: [`strip_known`] replaces it with
/// `…` wherever it appears, whatever characters its name holds.
pub fn know_dir(dir: &Path) {
    let mut forms = vec![dir.to_path_buf()];
    if let Ok(abs) = std::path::absolute(dir)
        && abs != dir
    {
        forms.push(abs);
    }
    // A root ("/", "C:", "~") or a bare relative name ("parts") would
    // match ordinary text.
    let usable =
        |v: &str| v.chars().filter(|c| c.is_alphanumeric()).count() >= 2 && v.contains(['/', '\\']);
    let mut add = Vec::new();
    for f in forms {
        let raw = f.to_string_lossy();
        let raw = raw.trim_end_matches(['/', '\\']);
        for v in [
            raw.to_string(),
            raw.replace('\\', "/"),
            raw.replace('/', "\\"),
        ] {
            let scrubbed = scrub(&v);
            add.extend([v, scrubbed].into_iter().filter(|v| usable(v)));
        }
    }
    let mut k = known().lock().unwrap_or_else(|e| e.into_inner());
    for v in add {
        if !k.contains(&v) {
            k.push(v);
        }
    }
    k.sort_by_key(|v| std::cmp::Reverse(v.len()));
}

/// Remember the folder of a file the app works with.
pub fn know_file(path: &Path) {
    if let Some(dir) = path.parent() {
        know_dir(dir);
    }
}

/// `text` with every known folder (see [`know_dir`]) replaced by `…`, so
/// only the file name after it remains.
pub fn strip_known(text: &str) -> String {
    // Copied out so nothing runs while the lock is held, and never waited
    // for: the panic hook logs, and logging strips known folders. When the
    // list is busy, `redact_dirs` still removes the folders of the line.
    let dirs = match known().try_lock() {
        Ok(k) => k.clone(),
        Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner().clone(),
        Err(std::sync::TryLockError::WouldBlock) => Vec::new(),
    };
    let mut s = text.to_string();
    for d in &dirs {
        s = replace_dir(&s, d);
    }
    s
}

/// Case-insensitive replace of the folder `dir` by `…` where it stands as
/// a whole folder: not inside a longer name on either side. Where a space
/// follows and the path runs on to a later separator, `dir` may be the
/// start of a longer folder name (`/srv/Acme Corp/x.step` with `/srv/Acme`
/// known): that whole path is redacted instead (see [`redact_dirs`]).
fn replace_dir(hay: &str, dir: &str) -> String {
    let mut out = String::with_capacity(hay.len());
    let mut i = 0;
    let mut prev: Option<char> = None;
    while i < hay.len() {
        let rest = &hay[i..];
        if !prev.is_some_and(is_word)
            && let Some(len) = match_len(rest, dir)
            && !rest[len..]
                .chars()
                .next()
                .is_some_and(|c| is_word(c) || c == '.' || c == '-')
        {
            let longer = rest[len..]
                .starts_with(char::is_whitespace)
                .then(|| path_len(rest))
                .flatten()
                .filter(|&p| p > len);
            match longer {
                Some(p) => {
                    let r = redact_one(&rest[..p]);
                    prev = r.chars().last();
                    out.push_str(&r);
                    i += p;
                }
                None => {
                    out.push('…');
                    prev = Some('…');
                    i += len;
                }
            }
            continue;
        }
        let c = rest.chars().next().expect("non-empty");
        out.push(c);
        prev = Some(c);
        i += c.len_utf8();
    }
    out
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Case-insensitive replace of every `needle` in `hay`.
fn replace_ci(hay: &str, needle: &str, with: &str, whole_word: bool) -> String {
    if needle.is_empty() {
        return hay.to_string();
    }
    let mut out = String::with_capacity(hay.len());
    let mut i = 0;
    let mut prev: Option<char> = None;
    while i < hay.len() {
        let rest = &hay[i..];
        if let Some(len) = match_len(rest, needle) {
            let next = rest[len..].chars().next();
            let bounded = !prev.is_some_and(is_word) && !next.is_some_and(is_word);
            if !whole_word || bounded {
                out.push_str(with);
                prev = needle.chars().last();
                i += len;
                continue;
            }
        }
        let c = rest.chars().next().expect("non-empty");
        out.push(c);
        prev = Some(c);
        i += c.len_utf8();
    }
    out
}

/// Byte length of `needle` at the start of `s`, ignoring case.
fn match_len(s: &str, needle: &str) -> Option<usize> {
    let mut chars = s.char_indices();
    let mut end = 0;
    for n in needle.chars() {
        let (i, c) = chars.next()?;
        if c != n && !c.to_lowercase().eq(n.to_lowercase()) {
            return None;
        }
        end = i + c.len_utf8();
    }
    Some(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_and_user_are_removed() {
        let s = Scrubber::new(Some("/home/alice"), Some("alice"));
        assert_eq!(
            s.scrub("opened /home/alice/parts/tube.step for ALICE"),
            "opened ~/parts/tube.step for <user>"
        );
        // A user name inside another path is still removed.
        assert_eq!(s.scrub("/mnt/alice-disk/x"), "/mnt/<user>-disk/x");
        assert!(
            !s.scrub("Alice wrote /HOME/ALICE/a")
                .to_lowercase()
                .contains("alice")
        );
    }

    #[test]
    fn windows_paths_in_both_spellings() {
        let s = Scrubber::new(Some(r"C:\Users\Bob Smith"), Some("Bob Smith"));
        assert_eq!(
            s.scrub(r"C:\Users\Bob Smith\Desktop\a.step and C:/Users/bob smith/b.step"),
            r"~\Desktop\a.step and ~/b.step"
        );
    }

    #[test]
    fn short_names_only_as_whole_words() {
        let s = Scrubber::new(Some("/home/al"), Some("al"));
        assert_eq!(
            s.scrub("all normal; al did it"),
            "all normal; <user> did it"
        );
        assert_eq!(s.scrub("/home/al/x"), "~/x");
    }

    #[test]
    fn root_like_homes_are_ignored() {
        let s = Scrubber::new(Some("/"), None);
        assert_eq!(s.scrub("/usr/lib"), "/usr/lib");
        let s = Scrubber::new(None, None);
        assert_eq!(s.scrub("unchanged ✓"), "unchanged ✓");
    }

    #[test]
    fn folders_are_removed_from_paths() {
        assert_eq!(
            redact_dirs("Couldn't open /mnt/clients/acme/part.ntopo: not found"),
            "Couldn't open …/part.ntopo: not found"
        );
        assert_eq!(redact_dirs("saved to ~/parts/x.zip."), "saved to …/x.zip.");
        assert_eq!(
            redact_dirs(r"read C:\Work\Acme Corp\bracket v2.step ok"),
            r"read …/bracket v2.step ok"
        );
        assert_eq!(
            redact_dirs("in '/srv/My Parts/x.step' now"),
            "in '…/x.step' now"
        );
        // Several words in one folder name, as in an open error.
        assert_eq!(
            redact_dirs("failed opening /mnt/Top Secret Client/part.step"),
            "failed opening …/part.step"
        );
        assert_eq!(
            redact_dirs("read /home/a/My Big Old Parts/sub dir/x.step: not found"),
            "read …/x.step: not found"
        );
        assert_eq!(
            redact_dirs(r"C:\Users\x\Client Work Files\a b.step failed"),
            r"…/a b.step failed"
        );
        // Sentence punctuation after the file name is kept outside it.
        assert_eq!(
            redact_dirs("saved /a/b.step. Then it went on"),
            "saved …/b.step. Then it went on"
        );
        // A later word with a separator may still be a folder of the same
        // path, so it is removed too (too much rather than too little).
        assert_eq!(redact_dirs("saved /a/b.step. Then c/d"), "saved …/d");
        assert_eq!(
            redact_dirs("opened /a/b.step because it was there"),
            "opened …/b.step because it was there"
        );
        // Code locations, URLs, fractions and relative paths stay.
        for keep in [
            "at /rustc/abc/library/std/src/panicking.rs:697:5",
            "at ~/.cargo/registry/src/egui-0.36.0/src/ui.rs:12",
            "crates/core/src/eval/assemble.rs:368:12",
            "https://github.com/SilasVogt/newtopo/issues",
            "1/2 of 3/4",
            "a / b",
            "/",
        ] {
            assert_eq!(redact_dirs(keep), keep);
        }
    }

    #[test]
    fn folders_with_punctuation_and_spacing_are_removed() {
        for (text, want) in [
            (
                "autosaved to /home/bob/Clients/Acme Corp (NDA)/bracket.ntopo.autosave",
                "autosaved to …/bracket.ntopo.autosave",
            ),
            ("/Volumes/Jane's SSD/Clients/Acme/x", "…/x"),
            (
                "Couldn't open /srv/[Acme] Secret/x.ntopo: not found",
                "Couldn't open …/x.ntopo: not found",
            ),
            (
                r"C:\Users\bob\Documents\New folder (2)\part.step",
                "…/part.step",
            ),
            ("path:/srv/acme/y.step", "path:…/y.step"),
            ("read /srv/Acme  Corp/x.step ok", "read …/x.step ok"),
            ("(/srv/Acme, Inc/x.step)", "(…/x.step)"),
            (
                "copied '/a/Acme/b' to '/c/Other/d'",
                "copied '…/b' to '…/d'",
            ),
        ] {
            assert_eq!(redact_dirs(text), want, "{text}");
            for secret in ["Acme", "Jane", "NDA", "Documents", "srv"] {
                assert!(!redact_dirs(text).contains(secret), "{secret} in {text}");
            }
        }
    }

    #[test]
    fn known_folders_are_removed_as_written() {
        let dir = tempfile::tempdir().unwrap();
        // Characters no path heuristic should have to guess about.
        let odd = dir.path().join("Known \"Client\"   [x] (2)");
        know_file(&odd.join("part.ntopo"));
        let text = format!("Couldn't open {}/part.ntopo\nnext", odd.display());
        let got = Scrubber::new(None, None).clean(&text);
        assert_eq!(got, "Couldn't open …/part.ntopo\nnext");
        // Subfolders of a known folder are removed as well.
        let got = Scrubber::new(None, None).clean(&format!("{}/Sub Dir/x.step", odd.display()));
        assert_eq!(got, "…/x.step");
        // Only whole folder names: a longer sibling is left to redact_dirs.
        let sibling = format!("{}x/y.step", odd.display());
        assert!(!strip_known(&sibling).starts_with('…'));
        // Roots and bare names are never remembered.
        know_dir(Path::new("/"));
        know_dir(Path::new("parts"));
        assert_eq!(strip_known("/usr/parts/x"), "/usr/parts/x");
    }

    #[test]
    fn source_locations_never_shield_other_paths() {
        for (text, want) in [
            // Glued over later words: the folders of the first path go.
            (
                "Opened /mnt/clients/acme/part.step then checked src/main.rs",
                "Opened …/main.rs",
            ),
            // Glued without a space.
            (
                "Opened /mnt/clients/acme/part.step,src/main.rs",
                "Opened …/main.rs",
            ),
            (
                "Opened /mnt/clients/acme/x.step;crates/app/src/jobs.rs:709:5",
                "Opened …/jobs.rs:709:5",
            ),
            // A private path and a source location on one line.
            (
                "panicked at /work/newtopo/crates/app/src/jobs.rs:709:5 while opening /mnt/clients/acme/part.step",
                "panicked at /work/newtopo/crates/app/src/jobs.rs:709:5 while opening …/part.step",
            ),
            (
                "panicked at crates/app/src/jobs.rs:709:5: can't read /mnt/clients/acme/part.step",
                "panicked at crates/app/src/jobs.rs:709:5: can't read …/part.step",
            ),
        ] {
            let got = redact_dirs(text);
            assert_eq!(got, want, "{text}");
            for secret in ["clients", "acme", "/mnt"] {
                assert!(!got.contains(secret), "{secret} in {got}");
            }
        }
        // A source location on its own is still kept.
        for keep in [
            "crates/app/src/jobs.rs:709:5",
            "at /work/newtopo/crates/app/src/jobs.rs:709:5",
        ] {
            assert_eq!(redact_dirs(keep), keep);
        }
    }

    #[test]
    fn a_known_folder_that_starts_a_longer_name_is_not_cut_off() {
        for (known, text) in [
            ("/srv/Acme", "open /srv/Acme Corp/x.step"),
            ("/srv/Acme Corp", "open /srv/Acme Corp (NDA)/x.step"),
        ] {
            assert_eq!(replace_dir(text, known), "open …/x.step", "{known}");
            assert_eq!(
                redact_dirs(&replace_dir(text, known)),
                "open …/x.step",
                "{known}"
            );
        }
        // Where the known folder ends, it is still replaced as written.
        for (text, want) in [
            ("open /srv/Acme/x.step", "open …/x.step"),
            ("folder /srv/Acme is read-only", "folder … is read-only"),
            ("in '/srv/Acme': gone", "in '…': gone"),
            ("/srv/Acme", "…"),
        ] {
            assert_eq!(replace_dir(text, "/srv/Acme"), want);
        }
    }

    #[test]
    fn unicode_is_kept_intact() {
        let s = Scrubber::new(Some("/home/jörg"), Some("jörg"));
        assert_eq!(s.scrub("Größe /home/JÖRG/ä.step"), "Größe ~/ä.step");
    }
}
