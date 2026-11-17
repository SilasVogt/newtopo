//! Every user-facing string in one table (SPEC §1 non-goals: English only,
//! but keep UI strings in one place). Command labels live in
//! [`crate::commands`] next to their ids and also come from here.

pub const APP_NAME: &str = "newtopo";
pub const UNTITLED: &str = "Untitled";
pub const UNSAVED: &str = "unsaved";

// Menus
pub const MENU_FILE: &str = "File";
pub const MENU_EDIT: &str = "Edit";
pub const MENU_VIEW: &str = "View";
pub const MENU_TOOLS: &str = "Tools";
pub const MENU_HELP: &str = "Help";
pub const MENU_RECENT: &str = "Open Recent";
pub const MENU_NO_RECENT: &str = "No recent scenes";
pub const MENU_CLEAR_RECENT: &str = "Clear list";
pub const MENU_PANELS: &str = "Panels";
pub const MENU_CAMERA: &str = "Camera";

// Tabs
pub const TAB_OUTLINER: &str = "Outliner";
pub const TAB_VIEWPORT: &str = "Viewport";
pub const TAB_TOOL: &str = "Tool";
pub const TAB_STATS: &str = "Statistics";
pub const TAB_HISTORY: &str = "History";
pub const TAB_RENAME: &str = "Rename";
pub const TAB_BRIDGE: &str = "Bridge";
pub const TAB_UV: &str = "UV";

// Modes
pub const MODE_VERTEX: &str = "Vertex";
pub const MODE_EDGE: &str = "Edge";
pub const MODE_FACE: &str = "Face";
pub const MODE_OBJECT: &str = "Object";

// Toggles
pub const TOGGLE_PREVIEW: &str = "Preview";
pub const TOGGLE_AUTOFILL: &str = "Auto-fill";
pub const TOGGLE_LINK: &str = "Link";
pub const TOGGLE_SYMMETRY: &str = "Symmetry";

// Empty states
pub const EMPTY_TITLE: &str = "Bring in a CAD model";
pub const EMPTY_BODY: &str = "Drop a STEP, IGES or BREP file here, or import one from disk.";
pub const EMPTY_IMPORT: &str = "Import CAD…";
pub const EMPTY_OPEN: &str = "Open scene…";
pub const EMPTY_DEMO: &str = "Explore the sample";
pub const EMPTY_HINT: &str = "Ctrl+K searches every command";
pub const EMPTY_OUTLINER: &str = "No objects yet.";
pub const EMPTY_HISTORY: &str = "Nothing here yet. Rules and edits you add show up in order.";
pub const EMPTY_TOOL: &str =
    "Pick a tool: Connect (C), Grid (G), Slice (S), Iso (I), Cut (K) or Weld (W).";
pub const NO_SCENE: &str = "No scene loaded";
pub const DEMO_BADGE: &str = "Sample model (read-only)";

// Outliner
pub const OUTLINER_SEARCH: &str = "Filter objects  (Ctrl+F)";
pub const OUTLINER_NEW_LAYER: &str = "New layer";
pub const OUTLINER_DELETE_LAYER: &str = "Delete layer";
pub const OUTLINER_RENAME: &str = "Rename";
pub const OUTLINER_EXPAND_ALL: &str = "Expand all";
pub const OUTLINER_COLLAPSE_ALL: &str = "Collapse all";
pub const OUTLINER_ACTIVE_TIP: &str = "Active layer: imported objects without a layer land here";
pub const OUTLINER_VISIBLE: &str = "Show / hide (drag down to set many)";
pub const OUTLINER_LOCK: &str = "Lock / unlock (locked objects can't be selected)";
pub const OUTLINER_LAYER_NOT_EMPTY: &str = "Objects on a deleted layer move to the active layer.";

// Statistics
pub const STAT_VERTS: &str = "Vertices";
pub const STAT_EDGES: &str = "Edges";
pub const STAT_FACES: &str = "Faces";
pub const STAT_TRIS: &str = "Triangles";
pub const STAT_QUADS: &str = "Quads";
pub const STAT_NGONS: &str = "N-gons";
pub const STAT_DORMANT: &str = "Dormant edits";
pub const STAT_TOTAL: &str = "Total";
pub const STAT_SELECTED: &str = "Selected";
pub const STAT_SOURCE_PREVIEW: &str = "Output mesh";

// History
pub const HIST_RULES: &str = "Rules";
pub const HIST_EDITS: &str = "Edits";
pub const HIST_PHASE: &str = "Phase overrides";
pub const HIST_DORMANT: &str = "dormant";
pub const HIST_PURGE: &str = "Purge dormant";
pub const HIST_DELETE_RULE: &str = "Delete rule";

// Tool panel
pub const SEG_TITLE: &str = "Segments";
pub const SEG_A: &str = "A";
pub const SEG_B: &str = "B";
pub const SEG_U: &str = "U";
pub const SEG_V: &str = "V";
pub const SEG_COUNT: &str = "Count";
pub const SEG_TOTAL: &str = "Lines added";
pub const SEG_START_ON_SEAM: &str = "Start on seam";
pub const SEG_SPACING: &str = "Spacing";
pub const SEG_ARC: &str = "Arc length";
pub const SEG_PARAM: &str = "Parametric";
pub const SEG_THROUGH: &str = "Through (A)";
pub const SEG_GUIDE: &str = "Guide edge";
pub const SEG_GUIDE_PICK: &str = "Pick guide edge";
pub const SEG_GUIDE_AUTO: &str = "Auto orientation";
pub const SEG_CLEAR: &str = "Clear (count 0)";
pub const SEG_HINT_WHEEL: &str = "Shift+wheel / Ctrl+wheel over the viewport change the counts.";
pub const ISO_DIR: &str = "Direction";
pub const ISO_SPLIT: &str = "Split mode (X)";
pub const ISO_SNAP: &str = "Snap (N)";
pub const CUT_POINTS: &str = "Points";
pub const CUT_FINISH: &str = "Finish (Enter)";
pub const CUT_UNDO_POINT: &str = "Remove last (Backspace)";

// Status bar hints (SPEC §5.9)
pub const HINT_SELECT: &str = "Click to select · Shift add · Ctrl remove · drag for a box · right-drag for a lasso · double-click for loops";
pub const HINT_CONNECT: &str = "Connect: select patches · Shift+wheel = A · Ctrl+wheel = B · Tab flips A/B on the hovered patch";
pub const HINT_GRID: &str =
    "Surface Grid: select patches · Shift+wheel = U · Ctrl+wheel = V · Tab swaps U and V";
pub const HINT_SLICE: &str =
    "Slice: select objects or patches · Shift+wheel = count · A toggles through";
pub const HINT_ISO: &str =
    "Iso: click to place · Tab direction · X split mode · A through · N snap";
pub const HINT_CUT: &str =
    "Cut: click points · Enter finish · Backspace remove last · click the first point to close";
pub const HINT_WELD: &str =
    "Target Weld: click the source vertex, then the target (or drag between them)";
pub const HINT_DEMO: &str =
    "Sample model: navigation and selection work; editing needs an imported model";

// Toasts and refusals
pub const TOAST_NO_SCENE: &str = "Import a CAD model first.";
pub const TOAST_DEMO_READONLY: &str =
    "The sample model is for looking around. Import a CAD file to edit.";
pub const TOAST_SELECT_PATCHES: &str = "Select one or more faces first (face mode, 3).";
pub const TOAST_SELECT_EDGES: &str = "Select edges first (edge mode, 2).";
pub const TOAST_SELECT_OBJECTS: &str = "Select an object first.";
pub const TOAST_CAD_EDGE_ARMED: &str =
    "That's a CAD edge. Press Delete again within a second to remove it from the output.";
pub const TOAST_FILL_EDGE: &str =
    "Fill edges can't be removed. Add a line (Iso or Cut) or weld instead.";
pub const TOAST_SAVED: &str = "Saved";
pub const TOAST_EXPORTED: &str = "Exported";
pub const TOAST_ENGINE_MISSING: &str = "This part of the engine isn't available yet";
pub const TOAST_NO_CAD_IMPORT: &str = "This build has no CAD import (built without OCCT).";
pub const TOAST_KERNEL_MISSING: &str =
    "No CAD kernel in this build, so imported geometry can't be loaded.";
pub const TOAST_NOT_YET: &str = "Not available yet in this build";
pub const TOAST_COPIED: &str = "Copied";
pub const TOAST_REPLACE_UNSUPPORTED: &str =
    "Replace isn't supported by the engine yet; imported as a new object instead.";

// Dialogs
pub const DLG_CANCEL: &str = "Cancel";
pub const DLG_CONFIRM: &str = "Confirm";
pub const DLG_CLOSE: &str = "Close";
pub const DLG_APPLY: &str = "Apply";
pub const DLG_DISCARD: &str = "Discard";
pub const DLG_SAVE: &str = "Save";
pub const DLG_CONTINUE: &str = "Continue anyway";
pub const DLG_UNSAVED_TITLE: &str = "Unsaved changes";
pub const DLG_UNSAVED_BODY: &str = "The scene has changes that aren't saved.";
pub const DLG_CAD_EDGE_TITLE: &str = "Remove CAD edge?";
pub const DLG_CAD_EDGE_BODY: &str = "This merges the two output polygons on either side. The CAD model itself never changes, but the output can depart from the CAD shape.";
pub const DLG_RECOVER_TITLE: &str = "Recover autosaved work?";
pub const DLG_RECOVER_BODY: &str =
    "newtopo found autosaves newer than their scenes. It may have closed unexpectedly.";
pub const DLG_RECOVER: &str = "Recover";
pub const DLG_ABOUT_BODY: &str = "Turns CAD solids into clean, game-ready polygon meshes.";

// Import
pub const IMPORT_TITLE: &str = "Import";
pub const IMPORT_LIST_TITLE: &str = "Choose objects to import";
pub const IMPORT_KEEP: &str = "Keep";
pub const IMPORT_EXCLUDE: &str = "Exclude";
pub const IMPORT_REPLACE: &str = "Replace";
pub const IMPORT_ALL: &str = "All";
pub const IMPORT_NONE: &str = "None";
pub const IMPORT_SEARCH: &str = "Search objects";
pub const IMPORT_RUN: &str = "Import";
pub const IMPORT_REPORT_TITLE: &str = "Import report";
pub const IMPORT_FAILED: &str = "Import failed";
pub const IMPORT_CANCELLED: &str = "Import cancelled";
pub const IMPORT_DAMAGED: &str = "Some faces are missing after healing. The file itself was probably damaged when it was exported. Try exporting it again from the CAD tool; if that keeps happening, try IGES or a different STEP schema.";
pub const IMPORT_OPTIONS: &str = "Import options";
pub const IMPORT_HEAL: &str = "Heal geometry";
pub const IMPORT_SEWING: &str = "Sewing tolerance";
pub const IMPORT_UNITS: &str = "Units";
pub const IMPORT_FILTER: &str = "CAD files";

// Export
pub const EXPORT_TITLE: &str = "Export";
pub const EXPORT_FORMAT: &str = "Format";
pub const EXPORT_TRIANGULATE: &str = "Triangulate";
pub const EXPORT_GLB_TRI: &str = "glTF is always triangulated";
pub const EXPORT_SCALE: &str = "Scale";
pub const EXPORT_UNITS: &str = "Units";
pub const EXPORT_UP: &str = "Up axis";
pub const EXPORT_LAYERS: &str = "Layers as hierarchy";
pub const EXPORT_MATERIALS: &str = "Materials from CAD colours";
pub const EXPORT_NORMALS: &str = "Normals";
pub const EXPORT_NORMALS_CAD: &str = "Exact CAD normals";
pub const EXPORT_NORMALS_WEIGHTED: &str = "Face-weighted";
pub const EXPORT_NGON_NOTE: &str =
    "Auto-fill is off: n-gons are written as n-gons (FBX/OBJ) or triangulated (glTF).";
pub const EXPORT_FAN_TITLE: &str = "Polygons filled by the fallback fan";
pub const EXPORT_SCOPE_SEL: &str = "Exports the selected objects.";
pub const EXPORT_SCOPE_ALL: &str = "Nothing selected: exports the whole scene.";
pub const EXPORT_RUN: &str = "Export…";

// Studio panels
pub const RENAME_SET: &str = "Set name";
pub const RENAME_COPY_ACTIVE: &str = "Use active";
pub const RENAME_ADD: &str = "Add text";
pub const RENAME_PREFIX: &str = "Prefix";
pub const RENAME_SUFFIX: &str = "Suffix";
pub const RENAME_REMOVE: &str = "Remove characters";
pub const RENAME_FROM_START: &str = "From start";
pub const RENAME_FROM_END: &str = "From end";
pub const RENAME_REPLACE: &str = "Replace";
pub const RENAME_FIND: &str = "Find";
pub const RENAME_WITH: &str = "With";
pub const RENAME_NUMBER: &str = "Number";
pub const RENAME_SEPARATOR: &str = "Separator";
pub const RENAME_START: &str = "Start";
pub const RENAME_STEP: &str = "Step";
pub const RENAME_PADDING: &str = "Padding";
pub const RENAME_STRIP_NUMBER: &str = "Remove numbering";
pub const RENAME_REGEX: &str = "Regular expression";
pub const RENAME_REGEX_HINT: &str = "Use $1 or ${name} in the replacement for captured groups";
pub const RENAME_CASE: &str = "Match case";
pub const RENAME_PREVIEW: &str = "Preview";
pub const RENAME_COLLISION: &str = "Name collision";
pub const RENAME_NOTHING: &str = "Select objects to rename them.";
pub const BRIDGE_TOKEN: &str = "Pairing token";
pub const BRIDGE_TOKEN_HINT: &str = "Paste this once into each addon's settings.";
pub const BRIDGE_SEND: &str = "Send";
pub const BRIDGE_PORT: &str = "Port";
pub const BRIDGE_BAD_TOKEN: &str = "Token rejected: paste the token below into the add-on";
pub const BRIDGE_DISCONNECTED: &str = "Not connected";
pub const BRIDGE_KEEP_BOTH: &str = "Keep both when names match";
pub const BRIDGE_SCOPE: &str =
    "Sends the selection, or the whole scene (hidden layers included) when nothing is selected.";
pub const UV_TITLE: &str = "Quick UV unwrap";
pub const UV_COMING_SOON: &str = "Coming soon: the unwrapper lands in a later phase.";
pub const UV_PADDING: &str = "Padding (px)";
pub const UV_TEX_SIZE: &str = "Texture size";
pub const UV_ROTATION: &str = "Allow rotation";
pub const UV_TEXEL: &str = "Uniform texel density";
pub const UV_CHECKER: &str = "Checker preview";
pub const UV_TILES: &str = "Tiles";
pub const UV_RUN: &str = "Unwrap";

// Palette / radial / keymap
pub const PALETTE_HINT: &str = "Type a command…";
pub const PALETTE_FAVOURITES: &str = "Favourites";
pub const PALETTE_ALL: &str = "All commands";
pub const PALETTE_NO_MATCH: &str = "No command matches";
pub const PALETTE_CHANGE_SHORTCUT: &str = "Change shortcut…";
pub const PALETTE_ADD_FAV: &str = "Add to favourites";
pub const PALETTE_REMOVE_FAV: &str = "Remove from favourites";
pub const PALETTE_ADD_RADIAL: &str = "Add to radial menu";
pub const PALETTE_RESET_SHORTCUT: &str = "Reset shortcut";
pub const RADIAL_REPLACE: &str = "Replace…";
pub const RADIAL_REMOVE: &str = "Remove";
pub const RADIAL_MOVE: &str = "Move…";
pub const RADIAL_MOVE_HINT: &str = "Click the slot to move it to";
pub const RADIAL_FULL: &str = "The radial menu is full (12 slots). Remove one first.";
pub const KEY_CAPTURE_TITLE: &str = "Press the new shortcut";
pub const KEY_CAPTURE_DOUBLE: &str = "Double-press (same key twice within 300 ms)";
pub const KEY_CAPTURE_CONFLICT: &str = "Also used by";
pub const KEY_NONE: &str = "—";

// Preferences
pub const PREFS_TITLE: &str = "Preferences";
pub const PREFS_GENERAL: &str = "General";
pub const PREFS_APPEARANCE: &str = "Appearance";
pub const PREFS_STATS: &str = "Statistics";
pub const PREFS_KEYMAP: &str = "Keymap";
pub const PREFS_NAV: &str = "Navigation";
pub const PREFS_UNITS: &str = "Scene units";
pub const PREFS_UP: &str = "Up axis";
pub const PREFS_AUTOSAVE: &str = "Autosave every";
pub const PREFS_MINUTES: &str = "min";
pub const PREFS_THEME: &str = "UI theme";
pub const PREFS_RADIAL_THEME: &str = "Radial menu";
pub const PREFS_OVERLAY: &str = "Viewport colours";
pub const PREFS_RESET_COLOURS: &str = "Reset to theme";
pub const PREFS_UI_SCALE: &str = "Interface scale";
pub const PREFS_STATS_SHOW: &str = "Show";
pub const PREFS_STATS_ROWS: &str = "Rows";
pub const PREFS_SEL_UNDO: &str = "Selection changes are undo steps";
pub const PREFS_KEYMAP_SEARCH: &str = "Search commands";
pub const PREFS_RESET_ALL_KEYS: &str = "Reset all shortcuts";
pub const PREFS_SELECT_FILL: &str = "Allow selecting fill edges";
pub const NAV_BLENDER: &str = "Blender";
pub const NAV_MAYA: &str = "Maya";
pub const NAV_MAX: &str = "3ds Max";
pub const NAV_BLENDER_HELP: &str =
    "Middle-drag orbit · Shift+middle pan · Ctrl+middle zoom · wheel zoom";
pub const NAV_MAYA_HELP: &str = "Alt+left orbit · Alt+middle pan · Alt+right zoom · wheel zoom";
pub const NAV_MAX_HELP: &str = "Alt+middle orbit · middle pan · Ctrl+Alt+middle zoom · wheel zoom";
pub const STATS_TOTALS: &str = "Totals";
pub const STATS_SELECTION: &str = "Selection";
pub const STATS_BOTH: &str = "Both";

// Viewport
pub const VP_PERSP: &str = "Perspective";
pub const VP_ORTHO: &str = "Orthographic";
pub const VP_ISOLATED: &str = "Isolated";
pub const VP_VISIBLE_ONLY: &str = "Visible only";
pub const VP_DROP: &str = "Drop to import";
pub const VIEW_FRONT: &str = "Front";
pub const VIEW_BACK: &str = "Back";
pub const VIEW_LEFT: &str = "Left";
pub const VIEW_RIGHT: &str = "Right";
pub const VIEW_TOP: &str = "Top";
pub const VIEW_BOTTOM: &str = "Bottom";
