//! Launch details for packaged installs: command-line parsing (file
//! associations pass the document as the first argument) and locating the
//! `newtopo-import` helper next to the real executable.

use std::path::{Path, PathBuf};

/// What the command line asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaunchArgs {
    /// `--shot <dir>` (+ `--models <dir>`): headless screenshots.
    pub shot: Option<PathBuf>,
    pub models: Option<PathBuf>,
    /// `--window-shot <png>`: capture the real window and quit.
    pub window_shot: Option<PathBuf>,
    pub sample: bool,
    /// A `.ntopo` scene or CAD file to open/import (first free argument).
    pub open: Option<PathBuf>,
}

/// Flags that take a value.
const VALUE_FLAGS: [&str; 3] = ["--shot", "--models", "--window-shot"];

pub fn parse_args(args: &[String]) -> LaunchArgs {
    let mut out = LaunchArgs::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let value = || args.get(i + 1).map(PathBuf::from);
        match a.as_str() {
            "--shot" => out.shot = value().or_else(|| Some(PathBuf::from("screenshots"))),
            "--models" => out.models = value(),
            "--window-shot" => out.window_shot = value(),
            "--sample" => out.sample = true,
            // macOS Finder may add a process serial number argument.
            s if s.starts_with("-psn_") => {}
            s if s.starts_with('-') => {}
            s if out.open.is_none() => out.open = Some(document_path(s)),
            _ => {}
        }
        i += if VALUE_FLAGS.contains(&a.as_str()) {
            2
        } else {
            1
        };
    }
    out
}

/// A document argument as a path: accepts `file:` URLs (some desktop
/// launchers pass them) and makes relative paths absolute.
pub fn document_path(s: &str) -> PathBuf {
    let p = match file_url_to_path(s, cfg!(windows)) {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(s),
    };
    if p.is_relative()
        && let Ok(cwd) = std::env::current_dir()
    {
        return cwd.join(p);
    }
    p
}

/// Convert a `file:` URL to a local path string (RFC 8089). `windows`
/// selects the target conventions so both are testable on any host:
/// * `file:///home/a/b.ntopo`, `file://localhost/home/a/b.ntopo` → `/home/a/b.ntopo`
/// * `file:///C:/Users/a.ntopo`, `file:///C|/Users/a.ntopo` → `C:\Users\a.ntopo`
/// * `file://server/share/a.ntopo` → `\\server\share\a.ntopo` (UNC, Windows only)
///
/// Percent-escapes are decoded. Returns `None` for non-file URLs and for
/// remote hosts on non-Windows systems.
pub fn file_url_to_path(s: &str, windows: bool) -> Option<String> {
    let rest = s
        .get(..5)
        .filter(|p| p.eq_ignore_ascii_case("file:"))
        .map(|_| &s[5..])?;
    // Strip any query or fragment.
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let (host, path) = match rest.strip_prefix("//") {
        Some(r) => match r.find('/') {
            Some(i) => (&r[..i], &r[i..]),
            None => (r, ""),
        },
        None => ("", rest),
    };
    let path = percent_decode(path);
    let local = host.is_empty() || host.eq_ignore_ascii_case("localhost");
    if !windows {
        return (local && path.starts_with('/')).then_some(path);
    }
    if !local {
        // UNC: \\server\share\…
        let tail = path.trim_start_matches('/').replace('/', "\\");
        return Some(format!("\\\\{host}\\{tail}"));
    }
    // "/C:/x" or "/C|/x" → "C:\x".
    let p = path.trim_start_matches('/');
    let b = p.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && (b[1] == b':' || b[1] == b'|') {
        let rest = &p[2..];
        return Some(format!(
            "{}:{}",
            &p[..1],
            if rest.is_empty() {
                "\\".to_string()
            } else {
                rest.replace('/', "\\")
            }
        ));
    }
    Some(path.replace('/', "\\"))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn helper_name() -> String {
    format!("newtopo-import{}", std::env::consts::EXE_SUFFIX)
}

/// Directories to look in for the helper, best first: next to the
/// canonical executable (resolves `/usr/bin` symlinks, AppImage mounts,
/// `.app/Contents/MacOS`, the Windows install dir), then next to the path
/// we were started as.
pub fn helper_dirs(exe: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(c) = exe.canonicalize()
        && let Some(d) = c.parent()
    {
        dirs.push(d.to_path_buf());
    }
    if let Some(d) = exe.parent()
        && !dirs.iter().any(|x| x == d)
    {
        dirs.push(d.to_path_buf());
    }
    dirs
}

/// The import helper to run, or `None` to let the kernel search
/// (`$NEWTOPO_IMPORT_HELPER`, dev build directories).
pub fn helper_path() -> Option<PathBuf> {
    if std::env::var_os("NEWTOPO_IMPORT_HELPER").is_some() {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    helper_dirs(&exe)
        .into_iter()
        .map(|d| d.join(helper_name()))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn first_argument_is_the_document() {
        // A platform-absolute root: "/x/…" isn't absolute on Windows (it
        // resolves against the current drive).
        let root = std::env::temp_dir();
        let p = |name: &str| root.join("x").join(name);
        let s = |name: &str| p(name).to_string_lossy().into_owned();
        let l = parse_args(&a(&[&s("part.ntopo")]));
        assert_eq!(l.open, Some(p("part.ntopo")));
        let l = parse_args(&a(&["--sample", "-psn_0_1234", &s("a.step"), &s("b.step")]));
        assert!(l.sample);
        assert_eq!(l.open, Some(p("a.step")));
        let l = parse_args(&a(&["--window-shot", &s("w.png"), &s("s.ntopo")]));
        assert_eq!(l.window_shot, Some(p("w.png")));
        assert_eq!(l.open, Some(p("s.ntopo")));
        let l = parse_args(&a(&["--shot", "out", "--models", "m"]));
        assert_eq!(
            (l.shot, l.models, l.open),
            (Some(PathBuf::from("out")), Some(PathBuf::from("m")), None)
        );
    }

    #[test]
    fn file_urls_and_relative_paths() {
        assert!(document_path("rel.ntopo").is_absolute());
        #[cfg(unix)]
        assert_eq!(
            document_path("file:///home/u/My%20Part.ntopo"),
            PathBuf::from("/home/u/My Part.ntopo")
        );
        #[cfg(windows)]
        assert_eq!(
            document_path("file:///C:/Users/u/My%20Part.ntopo"),
            PathBuf::from(r"C:\Users\u\My Part.ntopo")
        );
    }

    #[test]
    fn unix_file_urls() {
        let u = |s| file_url_to_path(s, false);
        assert_eq!(
            u("file:///home/u/a.ntopo").as_deref(),
            Some("/home/u/a.ntopo")
        );
        assert_eq!(
            u("file://localhost/home/u/a.ntopo").as_deref(),
            Some("/home/u/a.ntopo")
        );
        assert_eq!(
            u("FILE://LOCALHOST/tmp/x.step").as_deref(),
            Some("/tmp/x.step")
        );
        assert_eq!(
            u("file:/tmp/one%20slash.step").as_deref(),
            Some("/tmp/one slash.step")
        );
        assert_eq!(
            u("file:///tmp/%C3%BCber.ntopo").as_deref(),
            Some("/tmp/über.ntopo")
        );
        assert_eq!(
            u("file:///tmp/a.ntopo#frag").as_deref(),
            Some("/tmp/a.ntopo")
        );
        assert_eq!(
            u("file://server/share/a.ntopo"),
            None,
            "remote host on Unix"
        );
        assert_eq!(u("https://x/a.ntopo"), None);
    }

    #[test]
    fn windows_file_urls() {
        let w = |s| file_url_to_path(s, true);
        assert_eq!(
            w("file:///C:/Users/a.ntopo").as_deref(),
            Some(r"C:\Users\a.ntopo")
        );
        assert_eq!(
            w("file:///c|/Users/My%20Docs/a.ntopo").as_deref(),
            Some(r"c:\Users\My Docs\a.ntopo")
        );
        assert_eq!(
            w("file://localhost/C:/a.ntopo").as_deref(),
            Some(r"C:\a.ntopo")
        );
        assert_eq!(w("file:///D:").as_deref(), Some(r"D:\"));
        assert_eq!(
            w("file://server/share/dir/a.ntopo").as_deref(),
            Some(r"\\server\share\dir\a.ntopo")
        );
        assert_eq!(
            w("file://server/share/x%23y.step").as_deref(),
            Some(r"\\server\share\x#y.step")
        );
    }

    #[test]
    fn helper_found_through_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("opt/newtopo/bin");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("newtopo"), b"").unwrap();
        std::fs::write(real.join(helper_name()), b"").unwrap();
        let bin = dir.path().join("usr/bin");
        std::fs::create_dir_all(&bin).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(real.join("newtopo"), bin.join("newtopo")).unwrap();
            let dirs = helper_dirs(&bin.join("newtopo"));
            assert_eq!(dirs[0], real.canonicalize().unwrap());
            assert!(dirs.iter().any(|d| d.join(helper_name()).is_file()));
        }
    }
}
