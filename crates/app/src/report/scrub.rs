//! Removes the user's home directory and OS user name from text that goes
//! into the log, a crash file or a problem report.

use std::sync::OnceLock;

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

    /// This machine's home directory and user name.
    pub fn current() -> &'static Scrubber {
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
}

/// Shorthand for the machine scrubber.
pub fn scrub(text: &str) -> String {
    Scrubber::current().scrub(text)
}

/// Folders removed from absolute paths, for text that leaves the machine:
/// `/mnt/clients/acme/part.step` becomes `…/part.step`. Source locations
/// (`….rs`, optionally with `:line:col`) are kept, since they point into
/// the code, not the user's files.
pub fn redact_dirs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut prev: Option<char> = None;
    while i < text.len() {
        let rest = &text[i..];
        let boundary = prev.is_none_or(|c| c.is_whitespace() || "\"'([<=`".contains(c));
        if boundary && let Some(len) = path_len(rest) {
            let raw = &rest[..len];
            // Sentence punctuation after a path is not part of it.
            let path = raw.trim_end_matches(['.', ',', ';', ':', '!', '?']);
            out.push_str(&redact_one(path));
            out.push_str(&raw[path.len()..]);
            prev = raw.chars().last();
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
/// there. A path may contain folder and file names with single spaces:
/// it runs on through space-separated words up to the last one that
/// continues it with another separator (`/mnt/Top Secret Client/x.step`).
/// Sentence punctuation at the end of a word ends the path there.
fn path_len(s: &str) -> Option<usize> {
    if !starts_path(s) {
        return None;
    }
    // `<` and `>` stay inside: the scrubber's `<user>` can be part of a path.
    let stop = |c: char| c.is_whitespace() || "\"'|`()[]{}".contains(c);
    let ends_sentence = |w: &str| w.ends_with(['.', ',', ';', ':', '!', '?']);
    let mut end = s.find(stop).unwrap_or(s.len());
    if ends_sentence(&s[..end]) {
        return Some(end);
    }
    // Look ahead word by word; commit up to the last word with a separator.
    let mut probe = end;
    for _ in 0..MAX_PATH_WORDS {
        let Some(after) = s[probe..].strip_prefix(' ') else {
            break;
        };
        let word_end = after.find(stop).unwrap_or(after.len());
        let word = &after[..word_end];
        if word.is_empty() || starts_path(word) {
            break;
        }
        probe += 1 + word_end;
        if word.contains(['/', '\\']) {
            end = probe;
        }
        if ends_sentence(word) {
            break;
        }
    }
    Some(end)
}

fn redact_one(path: &str) -> String {
    let code = {
        let p = path.trim_end_matches(|c: char| c.is_ascii_digit() || c == ':');
        p.ends_with(".rs")
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
        // A path ends at the end of a sentence; later words are kept.
        assert_eq!(
            redact_dirs("saved /a/b.step. Then it went to c/d"),
            "saved …/b.step. Then it went to c/d"
        );
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
    fn unicode_is_kept_intact() {
        let s = Scrubber::new(Some("/home/jörg"), Some("jörg"));
        assert_eq!(s.scrub("Größe /home/JÖRG/ä.step"), "Größe ~/ä.step");
    }
}
