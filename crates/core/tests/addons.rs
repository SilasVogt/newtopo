//! Static checks on the DCC add-ons (they can't all run here): the shared
//! protocol module is byte-identical in every add-on, and every Python file
//! compiles (if `python3` is available).

use std::path::Path;
use std::process::Command;

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

#[test]
fn protocol_copies_are_identical() {
    let read = |host: &str| {
        std::fs::read(root().join(format!("addons/{host}/newtopo_bridge/protocol.py"))).unwrap()
    };
    let blender = read("blender");
    assert_eq!(
        blender,
        read("maya"),
        "addons/maya protocol.py differs from Blender's"
    );
    assert_eq!(
        blender,
        read("max"),
        "addons/max protocol.py differs from Blender's"
    );
}

#[test]
fn python_sources_compile() {
    let Ok(out) = Command::new("python3").arg("--version").output() else {
        eprintln!("skipped: no python3");
        return;
    };
    assert!(out.status.success());
    let mut files = Vec::new();
    for dir in [
        "addons/blender/newtopo_bridge",
        "addons/maya/newtopo_bridge",
        "addons/max/newtopo_bridge",
        "addons/max",
        "tests/blender",
    ] {
        for e in std::fs::read_dir(root().join(dir)).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x == "py") {
                files.push(p);
            }
        }
    }
    assert!(files.len() >= 10);
    // Parse only (no bytecode written into the source tree).
    let code = "import ast,sys\nfor f in sys.argv[1:]:\n    ast.parse(open(f,encoding='utf-8').read(), f)\n";
    let st = Command::new("python3")
        .arg("-c")
        .arg(code)
        .args(&files)
        .status()
        .unwrap();
    assert!(st.success(), "a Python source failed to parse");
}

#[test]
fn python_protocol_decodes_rust_frames() {
    // Run the shared protocol module against a frame encoded by Rust.
    if Command::new("python3").arg("--version").output().is_err() {
        return;
    }
    use newtopo_core::bridge::payload::build;
    use newtopo_core::bridge::{BridgeObject, Header, MsgType, codec};
    use newtopo_core::export::samples::SampleScene;
    let scene = SampleScene::new();
    let objs: Vec<BridgeObject> = scene
        .objects()
        .into_iter()
        .map(BridgeObject::from)
        .collect();
    let (layout, payload) = build(&objs, 1.0, false);
    let mut h = Header::new(MsgType::Push, "tok");
    h.layout = Some(serde_json::to_value(&layout).unwrap());
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("frame.bin");
    std::fs::write(&f, codec::encode(&h, &payload)).unwrap();
    let code = r#"
import sys
sys.path.insert(0, sys.argv[1])
import protocol as p
buf = bytearray(open(sys.argv[2], 'rb').read())
h, payload, used = p.try_decode(buf)
assert used == len(buf) and h['type'] == 'push' and p.check(h, 'tok') is None
assert p.check(h, 'nope') is not None
o = h['layout']['objects'][0]
pos = p.array(payload, o['arrays']['positions'])
assert o['name'] == 'Cube' and len(pos) == 24 and pos[3] == 2.0
assert list(p.array(payload, o['arrays']['poly_sizes'])) == [4] * 6
assert p.try_decode(buf[:-1]) is None
print('ok')
"#;
    let out = Command::new("python3")
        .arg("-c")
        .arg(code)
        .arg(root().join("addons/blender/newtopo_bridge"))
        .arg(&f)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn maya_server_and_max_smoothing_groups_without_hosts() {
    // Host-free parts of the Maya and Max add-ons, with stubbed host modules:
    // the Maya background-thread server (dispatch replaced) and the Max
    // smoothing-group assignment.
    if Command::new("python3").arg("--version").output().is_err() {
        return;
    }
    let code = r#"
import sys, types, socket, importlib
root = sys.argv[1]
for m in ['pymxs', 'maya', 'maya.cmds', 'maya.utils', 'maya.api', 'maya.api.OpenMaya']:
    sys.modules[m] = types.ModuleType(m)
sys.modules['pymxs'].runtime = types.SimpleNamespace()
sys.modules['pymxs'].undo = None

sys.path.insert(0, root + '/addons/maya')
maya_bridge = importlib.import_module('newtopo_bridge')
p = maya_bridge.protocol
srv = maya_bridge.ThreadedServer(0, 'tok', dispatch=lambda h, pl: p.hello_frame('fake maya'))
# bind to an ephemeral port
srv.start(); port = srv._sock.getsockname()[1]
def ask(tok):
    s = socket.create_connection(('127.0.0.1', port), timeout=5)
    s.sendall(p.encode({'type': 'hello', 'token': tok}))
    buf = bytearray()
    while True:
        c = s.recv(65536)
        if not c: break
        buf += c
        r = p.try_decode(buf)
        if r: return r[0]
assert ask('tok')['info']['host'] == 'fake maya'
assert ask('bad')['info']['code'] == 'bad_token'
srv.stop()
del sys.modules['newtopo_bridge'], sys.modules['newtopo_bridge.protocol']

sys.path.insert(0, root + '/addons/max')
mb = importlib.import_module('newtopo_bridge.build')
# Cube (6 quads, all edges hard): faceted.
cube = [0,3,2,1, 4,5,6,7, 0,1,5,4, 2,3,7,6, 0,4,7,3, 1,2,6,5]
hard = [a for e in [(0,1),(1,2),(2,3),(3,0),(4,5),(5,6),(6,7),(7,4),(0,4),(1,5),(2,6),(3,7)] for a in e]
assert mb.smoothing_groups([4]*6, cube, hard) == [0]*6
# Two smooth strips meeting at one hard seam: two groups, different bits.
#  0-1-2      quads (0,1,4,3) and (1,2,5,4) share edge 1-4 (hard)
#  3-4-5      plus (3,4,7,6) and (4,5,8,7) below share 4-7 (hard)
#  6-7-8      left column smooth across 3-4, right across 4-5
v = [0,1,4,3, 1,2,5,4, 3,4,7,6, 4,5,8,7]
g = mb.smoothing_groups([4]*4, v, [1,4, 4,7])
assert g[0] == g[2] and g[1] == g[3] and g[0] != g[1] and 0 not in g, g
print('ok')
"#;
    let out = Command::new("python3")
        .arg("-c")
        .arg(code)
        .arg(root())
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn bridge_server_hardening_python_tests() {
    // tests/blender/test_protocol.py: header-first auth, size caps,
    // connection cap, timeouts, buffer bound (plain Python, stub bpy).
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("skipped: no python3");
        return;
    }
    let out = Command::new("python3")
        .arg(root().join("tests/blender/test_protocol.py"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
