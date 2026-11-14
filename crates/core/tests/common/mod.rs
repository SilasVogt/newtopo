//! Helpers for the opt-in Blender tests.
#![allow(dead_code)]

use serde_json::Value;
use std::process::{Command, Stdio};

/// Blender executable: `NEWTOPO_BLENDER_CMD` or `blender`.
pub fn blender_cmd() -> String {
    std::env::var("NEWTOPO_BLENDER_CMD").unwrap_or_else(|_| "blender".to_string())
}

/// Run Blender headless (`-b --factory-startup`) with `pre` arguments and
/// `post` script arguments, and parse the `NEWTOPO_JSON` line it prints.
pub fn blender_json(pre: &[&str], post: &[String]) -> Value {
    let out = Command::new(blender_cmd())
        .args(["-b", "--factory-startup"])
        .args(pre)
        .args(post)
        .stdin(Stdio::null())
        .output()
        .expect("failed to start Blender (set NEWTOPO_BLENDER_CMD)");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find_map(|l| l.strip_prefix("NEWTOPO_JSON "))
        .unwrap_or_else(|| {
            panic!(
                "no NEWTOPO_JSON line; status {:?}\nstdout:\n{stdout}\nstderr:\n{}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            )
        });
    serde_json::from_str(line).expect("bad JSON from Blender")
}

pub fn num(v: &Value) -> u64 {
    v.as_u64().unwrap_or_else(|| panic!("not a number: {v}"))
}
