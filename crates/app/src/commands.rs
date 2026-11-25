//! Every command the UI can run: id (stable, used in JSON), label,
//! category, icon and default shortcuts. The default keymap is SPEC §5.8's
//! table exactly; file commands and a few extras use free combos.

use crate::icons::Icon;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    File,
    Edit,
    Select,
    View,
    Tools,
    Toggles,
    Panels,
    Help,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::File => "File",
            Category::Edit => "Edit",
            Category::Select => "Select",
            Category::View => "View",
            Category::Tools => "Tools",
            Category::Toggles => "Toggles",
            Category::Panels => "Panels",
            Category::Help => "Help",
        }
    }
}

macro_rules! commands {
    ($( $v:ident => $id:literal, $label:literal, $cat:ident, $icon:expr, [$($bind:literal),*] );* $(;)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Cmd { $($v),* }

        pub const ALL: &[Cmd] = &[$(Cmd::$v),*];

        impl Cmd {
            pub fn id(self) -> &'static str {
                match self { $(Cmd::$v => $id),* }
            }
            pub fn label(self) -> &'static str {
                match self { $(Cmd::$v => $label),* }
            }
            pub fn category(self) -> Category {
                match self { $(Cmd::$v => Category::$cat),* }
            }
            pub fn icon(self) -> Option<Icon> {
                match self { $(Cmd::$v => $icon),* }
            }
            /// Default shortcuts as strings (see [`crate::keymap::Binding::parse`]).
            pub fn default_bindings(self) -> &'static [&'static str] {
                match self { $(Cmd::$v => &[$($bind),*]),* }
            }
        }
    };
}

commands! {
    ModeVertex => "mode.vertex", "Vertex mode", Select, Some(Icon::Vertex), ["1"];
    ModeEdge => "mode.edge", "Edge mode", Select, Some(Icon::Edge), ["2"];
    ModeFace => "mode.face", "Face mode", Select, Some(Icon::Face), ["3"];
    ModeObject => "mode.object", "Object mode", Select, Some(Icon::Object), ["4"];

    Palette => "app.palette", "Command palette", Help, Some(Icon::Command), ["Ctrl+K"];
    Preferences => "app.preferences", "Preferences", Help, Some(Icon::Gear), ["Ctrl+Comma"];
    KeymapEditor => "app.keymap", "Edit keymap", Help, Some(Icon::Keyboard), [];

    Preview => "view.preview", "Preview mode", View, Some(Icon::Preview), ["V"];
    Wireframe => "view.wireframe", "Wireframe in preview", View, None, ["Shift+V"];
    FlatShading => "view.flat_shading", "Flat shading in preview", View, None, [];
    AutoFill => "toggle.autofill", "Auto-fill on/off", Toggles, Some(Icon::AutoFill), ["Shift+A"];
    Link => "toggle.link", "Link on/off", Toggles, Some(Icon::Link), ["Shift+L"];
    Symmetry => "toggle.symmetry", "Symmetry on/off", Toggles, Some(Icon::Symmetry), ["Shift+M"];
    MirrorX => "symmetry.mirror_x", "Mirror X", Toggles, None, ["Alt+X"];
    MirrorY => "symmetry.mirror_y", "Mirror Y", Toggles, None, ["Alt+Y"];
    MirrorZ => "symmetry.mirror_z", "Mirror Z", Toggles, None, ["Alt+Z"];
    SymmetryPlanes => "view.symmetry_planes", "Show symmetry planes", View, None, [];

    Connect => "tool.connect", "Connect", Tools, Some(Icon::Connect), ["C"];
    Grid => "tool.grid", "Surface Grid", Tools, Some(Icon::Grid), ["G"];
    Slice => "tool.slice", "Slice", Tools, Some(Icon::Slice), ["S"];
    Iso => "tool.iso", "Iso", Tools, Some(Icon::Iso), ["I"];
    Cut => "tool.cut", "Cut", Tools, Some(Icon::Cut), ["K"];
    Weld => "tool.weld", "Target Weld", Tools, Some(Icon::Weld), ["W"];
    Collapse => "edit.collapse", "Collapse", Tools, Some(Icon::Collapse), ["Alt+C"];
    Remove => "edit.remove_edge", "Remove edge (twice on a CAD edge)", Tools, Some(Icon::Remove), ["Delete"];
    Snap => "tool.snap", "Snap on/off (Iso, Cut)", Tools, None, ["N"];

    Loop => "select.loop", "Select loop", Select, Some(Icon::Loop), ["L"];
    LoopAlternate => "select.loop_alternate", "Select every other in loop", Select, None, ["L x2"];
    Ring => "select.ring", "Select ring", Select, Some(Icon::Ring), ["R"];
    RingAlternate => "select.ring_alternate", "Select every other in ring", Select, None, ["R x2"];
    SelectAll => "select.all", "Select all", Select, None, ["Ctrl+A"];
    SelectNone => "select.none", "Select none", Select, None, ["Alt+A"];
    Invert => "select.invert", "Invert selection", Select, None, ["Ctrl+I"];
    VisibleOnly => "select.visible_only", "Visible-only selection", Select, None, ["Alt+V"];

    Frame => "view.frame", "Frame selection (or all)", View, Some(Icon::Frame), ["F"];
    Ortho => "view.ortho", "Orthographic / perspective", View, Some(Icon::Cube), ["O"];
    Hide => "view.hide", "Hide selected", View, Some(Icon::EyeOff), ["H"];
    UnhideAll => "view.unhide_all", "Unhide all", View, Some(Icon::Eye), ["Alt+H"];
    Isolate => "view.isolate", "Isolate / step to next", View, None, ["Slash"];
    ExitIsolate => "view.exit_isolate", "Exit isolate", View, None, [];
    ViewFront => "view.front", "View front", View, None, [];
    ViewBack => "view.back", "View back", View, None, [];
    ViewLeft => "view.left", "View left", View, None, [];
    ViewRight => "view.right", "View right", View, None, [];
    ViewTop => "view.top", "View top", View, None, [];
    ViewBottom => "view.bottom", "View bottom", View, None, [];
    ViewIso => "view.iso", "View isometric", View, Some(Icon::Cube), [];
    HidePanels => "view.hide_panels", "Hide panels", View, None, ["Ctrl+Backslash"];
    ResetLayout => "view.reset_layout", "Reset panel layout", View, None, [];
    OutlinerSearch => "outliner.search", "Search outliner", View, Some(Icon::Search), ["Ctrl+F"];

    CopyRules => "edit.copy_rules", "Copy rules", Edit, None, ["Ctrl+C"];
    PasteRules => "edit.paste_rules", "Paste rules", Edit, None, ["Ctrl+V"];
    Undo => "edit.undo", "Undo", Edit, Some(Icon::Undo), ["Ctrl+Z"];
    Redo => "edit.redo", "Redo", Edit, Some(Icon::Redo), ["Ctrl+Shift+Z", "Ctrl+Y"];
    PurgeDormant => "edit.purge_dormant", "Purge dormant edits", Edit, Some(Icon::Trash), [];
    CheckMesh => "edit.check_mesh", "Check output mesh", Edit, Some(Icon::Warning), [];

    RenamePanel => "panel.rename", "Batch rename panel", Panels, Some(Icon::Rename), ["F1"];
    BridgePanel => "panel.bridge", "Bridge panel", Panels, Some(Icon::Bridge), ["F2"];
    UvPanel => "panel.uv", "UV panel", Panels, Some(Icon::Uv), ["F3"];

    New => "file.new", "New scene", File, None, ["Ctrl+N"];
    Open => "file.open", "Open scene…", File, Some(Icon::Open), ["Ctrl+O"];
    Save => "file.save", "Save", File, Some(Icon::Save), ["Ctrl+S"];
    SaveAs => "file.save_as", "Save as…", File, Some(Icon::Save), ["Ctrl+Shift+S"];
    Import => "file.import", "Import CAD…", File, Some(Icon::Import), ["Ctrl+Shift+I"];
    Export => "file.export", "Export…", File, Some(Icon::Export), ["Ctrl+E"];
    LoadSample => "file.sample", "Open the sample model", File, None, [];
    Quit => "file.quit", "Quit", File, None, ["Ctrl+Q"];

    CheckUpdates => "help.check_updates", "Check for updates…", Help, None, [];
    About => "help.about", "About newtopo", Help, Some(Icon::Info), [];
}

impl Cmd {
    pub fn from_id(id: &str) -> Option<Cmd> {
        ALL.iter().copied().find(|c| c.id() == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_and_roundtrip() {
        let mut seen = HashSet::new();
        for c in ALL {
            assert!(seen.insert(c.id()), "duplicate id {}", c.id());
            assert_eq!(Cmd::from_id(c.id()), Some(*c));
        }
    }
}
