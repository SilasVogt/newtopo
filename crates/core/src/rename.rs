//! Batch rename (SPEC §9.1). OWNER: export.
//!
//! Pure functions over a list of names: [`preview`] applies one operation
//! and returns old -> new with collision flags; the app shows it live and
//! applies it as one undo step. Lengths and positions count Unicode scalar
//! values (`char`s), not bytes.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Start,
    End,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Placement {
    Prefix,
    Suffix,
}

/// One rename operation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenameOp {
    /// Every selected object gets `name` (use [`RenameOp::copy_active`] to
    /// fill it from the active object).
    Set { name: String },
    /// Add `text` as a prefix or suffix.
    Add { text: String, at: Placement },
    /// Remove `count` characters from the start, the end, or both ends.
    Remove { count: usize, from: Side },
    /// Replace every occurrence of `find` with `with`. With `regex`, `find`
    /// is a regular expression and `with` may use `$1` / `${name}` groups.
    Replace {
        find: String,
        with: String,
        regex: bool,
        case_sensitive: bool,
    },
    /// Append `separator` + a number: `start`, `start + step`, ... in list
    /// order, zero-padded to `pad` digits.
    Number {
        separator: String,
        start: i64,
        step: i64,
        pad: usize,
    },
    /// Strip a trailing `separator` + digits (as added by `Number`). With an
    /// empty separator, strips trailing digits only.
    RemoveNumbering { separator: String },
}

impl RenameOp {
    /// "Copy active name" button: a `Set` with the active object's name.
    pub fn copy_active(names: &[String], active: usize) -> Option<RenameOp> {
        names.get(active).map(|n| RenameOp::Set { name: n.clone() })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RenameError {
    #[error("invalid regular expression: {0}")]
    BadRegex(String),
    #[error("find text is empty")]
    EmptyFind,
}

/// One row of the live preview.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameEntry {
    pub old: String,
    pub new: String,
    /// True if `new` equals another new name in the batch or a name in
    /// `others` (objects not being renamed), or is empty.
    pub collision: bool,
}

impl RenameEntry {
    pub fn changed(&self) -> bool {
        self.old != self.new
    }
}

fn take_chars(s: &str, skip: usize, keep: usize) -> String {
    s.chars().skip(skip).take(keep).collect()
}

fn remove(name: &str, count: usize, from: Side) -> String {
    let n = name.chars().count();
    match from {
        Side::Start => take_chars(name, count, n),
        Side::End => take_chars(name, 0, n.saturating_sub(count)),
        Side::Both => take_chars(name, count, n.saturating_sub(count.saturating_mul(2))),
    }
}

fn number(n: i64, pad: usize) -> String {
    if n < 0 {
        format!("-{:0pad$}", n.unsigned_abs(), pad = pad)
    } else {
        format!("{n:0pad$}")
    }
}

fn remove_numbering(name: &str, sep: &str) -> String {
    let digits = name
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .count();
    if digits == 0 {
        return name.to_string();
    }
    let stem = &name[..name.len() - digits]; // ASCII digits: 1 byte each
    if sep.is_empty() {
        return stem.to_string();
    }
    match stem.strip_suffix(sep) {
        // Don't strip a name down to nothing ("_01" stays).
        Some(base) if !base.is_empty() => base.to_string(),
        _ => name.to_string(),
    }
}

/// Compile the matcher for `Replace` once per batch.
enum Matcher {
    Plain { find: String, case_sensitive: bool },
    Regex(Regex),
}

impl Matcher {
    fn new(find: &str, regex: bool, case_sensitive: bool) -> Result<Matcher, RenameError> {
        if find.is_empty() {
            return Err(RenameError::EmptyFind);
        }
        if regex {
            let re = regex::RegexBuilder::new(find)
                .case_insensitive(!case_sensitive)
                .build()
                .map_err(|e| RenameError::BadRegex(e.to_string()))?;
            Ok(Matcher::Regex(re))
        } else if case_sensitive {
            Ok(Matcher::Plain {
                find: find.to_string(),
                case_sensitive,
            })
        } else {
            // Case-insensitive plain text: an escaped regex does the work.
            let re = regex::RegexBuilder::new(&regex::escape(find))
                .case_insensitive(true)
                .build()
                .map_err(|e| RenameError::BadRegex(e.to_string()))?;
            Ok(Matcher::Regex(re))
        }
    }
    fn apply(&self, name: &str, with: &str, regex_syntax: bool) -> String {
        match self {
            Matcher::Plain { find, .. } => name.replace(find.as_str(), with),
            Matcher::Regex(re) if regex_syntax => re.replace_all(name, with).into_owned(),
            Matcher::Regex(re) => re.replace_all(name, regex::NoExpand(with)).into_owned(),
        }
    }
}

/// Apply `op` to every name (in list order).
pub fn apply(names: &[String], op: &RenameOp) -> Result<Vec<String>, RenameError> {
    Ok(match op {
        RenameOp::Set { name } => vec![name.clone(); names.len()],
        RenameOp::Add { text, at } => names
            .iter()
            .map(|n| match at {
                Placement::Prefix => format!("{text}{n}"),
                Placement::Suffix => format!("{n}{text}"),
            })
            .collect(),
        RenameOp::Remove { count, from } => {
            names.iter().map(|n| remove(n, *count, *from)).collect()
        }
        RenameOp::Replace {
            find,
            with,
            regex,
            case_sensitive,
        } => {
            let m = Matcher::new(find, *regex, *case_sensitive)?;
            names.iter().map(|n| m.apply(n, with, *regex)).collect()
        }
        RenameOp::Number {
            separator,
            start,
            step,
            pad,
        } => names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let k = start.saturating_add(step.saturating_mul(i as i64));
                format!("{n}{separator}{}", number(k, *pad))
            })
            .collect(),
        RenameOp::RemoveNumbering { separator } => names
            .iter()
            .map(|n| remove_numbering(n, separator))
            .collect(),
    })
}

/// Live preview: old -> new with collision flags. `others` are the names of
/// objects outside the selection (they keep their names).
pub fn preview(
    names: &[String],
    op: &RenameOp,
    others: &[String],
) -> Result<Vec<RenameEntry>, RenameError> {
    let new = apply(names, op)?;
    let mut count: HashMap<&str, usize> = HashMap::new();
    for n in &new {
        *count.entry(n.as_str()).or_default() += 1;
    }
    let others: std::collections::HashSet<&str> = others.iter().map(String::as_str).collect();
    Ok(names
        .iter()
        .zip(&new)
        .map(|(old, new)| RenameEntry {
            old: old.clone(),
            new: new.clone(),
            collision: new.is_empty() || count[new.as_str()] > 1 || others.contains(new.as_str()),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn news(names: &[&str], op: RenameOp) -> Vec<String> {
        apply(&v(names), &op).unwrap()
    }

    #[test]
    fn set_and_copy_active() {
        let names = v(&["Bolt", "Nut"]);
        let op = RenameOp::copy_active(&names, 1).unwrap();
        assert_eq!(op, RenameOp::Set { name: "Nut".into() });
        let p = preview(&names, &op, &[]).unwrap();
        assert_eq!(
            p.iter().map(|e| e.new.as_str()).collect::<Vec<_>>(),
            ["Nut", "Nut"]
        );
        assert!(p.iter().all(|e| e.collision));
        assert!(!p[1].changed());
        assert!(RenameOp::copy_active(&names, 5).is_none());
    }

    #[test]
    fn add_prefix_and_suffix() {
        assert_eq!(
            news(
                &["a", "b"],
                RenameOp::Add {
                    text: "SM_".into(),
                    at: Placement::Prefix
                }
            ),
            ["SM_a", "SM_b"]
        );
        assert_eq!(
            news(
                &["a"],
                RenameOp::Add {
                    text: "_low".into(),
                    at: Placement::Suffix
                }
            ),
            ["a_low"]
        );
    }

    #[test]
    fn remove_chars() {
        let op = |count, from| RenameOp::Remove { count, from };
        assert_eq!(news(&["Body_001"], op(5, Side::Start)), ["001"]);
        assert_eq!(news(&["Body_001"], op(4, Side::End)), ["Body"]);
        assert_eq!(news(&["xBodyx"], op(1, Side::Both)), ["Body"]);
        assert_eq!(news(&["ab"], op(5, Side::End)), [""]);
        assert_eq!(news(&["abc"], op(2, Side::Both)), [""]);
        // Characters, not bytes.
        assert_eq!(news(&["Ärmel"], op(1, Side::Start)), ["rmel"]);
        let p = preview(&v(&["ab"]), &op(2, Side::Start), &[]).unwrap();
        assert!(p[0].collision, "empty names are flagged");
    }

    #[test]
    fn replace_plain_and_regex() {
        let r = |find: &str, with: &str, regex, cs| RenameOp::Replace {
            find: find.into(),
            with: with.into(),
            regex,
            case_sensitive: cs,
        };
        assert_eq!(
            news(&["Body.001", "Body.002"], r(".", "_", false, true)),
            ["Body_001", "Body_002"]
        );
        assert_eq!(
            news(&["BODY body"], r("body", "Part", false, false)),
            ["Part Part"]
        );
        assert_eq!(
            news(&["BODY body"], r("body", "Part", false, true)),
            ["BODY Part"]
        );
        assert_eq!(
            news(&["Body.012"], r(r"\.(\d+)$", "_v$1", true, true)),
            ["Body_v012"]
        );
        // Plain mode never expands `$`.
        assert_eq!(news(&["a.b"], r(".", "$0", false, false)), ["a$0b"]);
        assert_eq!(
            apply(&v(&["a"]), &r("(", "", true, true))
                .unwrap_err()
                .to_string()
                .split(':')
                .next(),
            Some("invalid regular expression")
        );
        assert_eq!(
            apply(&v(&["a"]), &r("", "x", false, true)),
            Err(RenameError::EmptyFind)
        );
    }

    #[test]
    fn numbering_and_removal() {
        let op = RenameOp::Number {
            separator: "_".into(),
            start: 1,
            step: 1,
            pad: 2,
        };
        assert_eq!(
            news(&["Bolt", "Bolt", "Nut"], op),
            ["Bolt_01", "Bolt_02", "Nut_03"]
        );
        let op = RenameOp::Number {
            separator: "".into(),
            start: 10,
            step: -5,
            pad: 3,
        };
        assert_eq!(
            news(&["a", "b", "c", "d"], op),
            ["a010", "b005", "c000", "d-005"]
        );
        let rm = |sep: &str| RenameOp::RemoveNumbering {
            separator: sep.into(),
        };
        assert_eq!(
            news(&["Bolt_01", "Nut", "M8_x_12", "_07", "Part7"], rm("_")),
            ["Bolt", "Nut", "M8_x", "_07", "Part7"]
        );
        assert_eq!(news(&["Part7", "Part"], rm("")), ["Part", "Part"]);
        // Round trip.
        let names = v(&["A", "B"]);
        let numbered = apply(
            &names,
            &RenameOp::Number {
                separator: ".".into(),
                start: 1,
                step: 1,
                pad: 3,
            },
        )
        .unwrap();
        assert_eq!(apply(&numbered, &rm(".")).unwrap(), names);
    }

    #[test]
    fn collisions_with_batch_and_others() {
        let names = v(&["a", "b", "c"]);
        let op = RenameOp::Replace {
            find: "b".into(),
            with: "a".into(),
            regex: false,
            case_sensitive: true,
        };
        let p = preview(&names, &op, &v(&["c"])).unwrap();
        assert_eq!(
            p.iter().map(|e| e.collision).collect::<Vec<_>>(),
            [true, true, true]
        );
        let p = preview(
            &names,
            &RenameOp::Add {
                text: "x".into(),
                at: Placement::Suffix,
            },
            &v(&["ax"]),
        )
        .unwrap();
        assert_eq!(
            p.iter().map(|e| e.collision).collect::<Vec<_>>(),
            [true, false, false]
        );
    }
}
