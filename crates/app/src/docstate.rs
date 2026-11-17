//! The open document: an optional core `Scene` plus app-side bookkeeping
//! (path, revision counter for cache invalidation, saved marker).
//!
//! The core facade may still be partly unimplemented (`todo!()`); every
//! call goes through [`guard`] so an unimplemented engine method shows a
//! toast instead of taking the app down. Remove the guard once the engine
//! is complete.

use newtopo_core::ops::{Op, OpOutcome, Refusal};
use newtopo_core::scene::Scene;
use newtopo_kernel::CadKernel;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Run engine code, turning a panic into an error message.
pub fn guard<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|e| {
        let msg = e
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "engine error".into());
        if msg.contains("not yet implemented") || msg.contains("not implemented") {
            format!("{} ({msg})", crate::strings::TOAST_ENGINE_MISSING)
        } else {
            msg
        }
    })
}

/// The CAD kernel used for new scenes. Wired by the lead once a kernel
/// constructor exists (API request: `newtopo_kernel::default_kernel()`).
pub fn make_kernel() -> Option<Arc<dyn CadKernel>> {
    Some(newtopo_kernel::default_kernel())
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApplyError {
    NoScene,
    Refused(Refusal),
    Engine(String),
}

impl ApplyError {
    pub fn message(&self) -> String {
        match self {
            ApplyError::NoScene => crate::strings::TOAST_NO_SCENE.into(),
            ApplyError::Refused(r) => r.reason.clone(),
            ApplyError::Engine(e) => e.clone(),
        }
    }
}

#[derive(Default)]
pub struct DocState {
    pub scene: Option<Scene>,
    pub path: Option<PathBuf>,
    /// Bumped on every change; view caches compare against it.
    pub revision: u64,
    saved_revision: u64,
}

impl DocState {
    pub fn has_scene(&self) -> bool {
        self.scene.is_some()
    }

    pub fn is_dirty(&self) -> bool {
        self.scene.is_some() && self.revision != self.saved_revision
    }

    pub fn mark_saved(&mut self) {
        self.saved_revision = self.revision;
    }

    pub fn bump(&mut self) {
        self.revision += 1;
    }

    pub fn title(&self) -> String {
        match &self.path {
            Some(p) => p
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string()),
            None => crate::strings::UNTITLED.to_string(),
        }
    }

    /// Replace the scene (new, opened or first import).
    pub fn set_scene(&mut self, scene: Scene, path: Option<PathBuf>) {
        self.scene = Some(scene);
        self.path = path;
        self.bump();
        self.saved_revision = self.revision;
    }

    pub fn close(&mut self) {
        self.scene = None;
        self.path = None;
        self.bump();
        self.saved_revision = self.revision;
    }

    /// Create an empty scene if none exists (first import).
    pub fn ensure_scene(&mut self) -> Result<&mut Scene, String> {
        if self.scene.is_none() {
            let kernel =
                make_kernel().ok_or_else(|| crate::strings::TOAST_KERNEL_MISSING.to_string())?;
            let scene = guard(|| Scene::new(kernel))?;
            self.scene = Some(scene);
            self.path = None;
            self.bump();
            // A brand-new scene counts as modified once something is added.
            self.saved_revision = self.revision;
        }
        Ok(self.scene.as_mut().expect("scene"))
    }

    /// Apply one op as one undo step.
    pub fn apply(&mut self, op: Op) -> Result<OpOutcome, ApplyError> {
        let Some(scene) = self.scene.as_mut() else {
            return Err(ApplyError::NoScene);
        };
        match guard(|| scene.apply(op)) {
            Ok(Ok(o)) => {
                self.bump();
                Ok(o)
            }
            Ok(Err(r)) => Err(ApplyError::Refused(r)),
            Err(e) => {
                // The scene may be half-modified: mark it changed and stop
                // calling into the engine until a new scene is loaded.
                self.bump();
                Err(ApplyError::Engine(e))
            }
        }
    }

    /// Apply as part of the current gesture (one undo step for the whole
    /// gesture, SPEC §4.7); call [`DocState::end_gesture`] when it ends.
    pub fn apply_coalesced(&mut self, op: Op) -> Result<OpOutcome, ApplyError> {
        let Some(scene) = self.scene.as_mut() else {
            return Err(ApplyError::NoScene);
        };
        match guard(|| scene.apply_coalesced(op)) {
            Ok(Ok(o)) => {
                self.bump();
                Ok(o)
            }
            Ok(Err(r)) => Err(ApplyError::Refused(r)),
            Err(e) => {
                self.bump();
                Err(ApplyError::Engine(e))
            }
        }
    }

    pub fn end_gesture(&mut self) {
        if let Some(scene) = self.scene.as_mut() {
            let _ = guard(|| scene.end_coalesce());
        }
    }

    /// Several ops as one undo step, all or nothing. With `coalesced`, the
    /// step joins the open gesture.
    pub fn apply_all(
        &mut self,
        ops: Vec<Op>,
        coalesced: bool,
    ) -> Result<Vec<OpOutcome>, ApplyError> {
        let Some(scene) = self.scene.as_mut() else {
            return Err(ApplyError::NoScene);
        };
        match guard(|| {
            if coalesced {
                scene.begin_coalesce();
            }
            scene.apply_all(ops)
        }) {
            Ok(Ok(o)) => {
                self.bump();
                Ok(o)
            }
            Ok(Err(r)) => Err(ApplyError::Refused(r)),
            Err(e) => {
                self.bump();
                Err(ApplyError::Engine(e))
            }
        }
    }

    pub fn undo(&mut self) -> Result<bool, String> {
        let Some(scene) = self.scene.as_mut() else {
            return Ok(false);
        };
        let r = guard(|| scene.undo())?;
        if r {
            self.bump();
        }
        Ok(r)
    }

    pub fn redo(&mut self) -> Result<bool, String> {
        let Some(scene) = self.scene.as_mut() else {
            return Ok(false);
        };
        let r = guard(|| scene.redo())?;
        if r {
            self.bump();
        }
        Ok(r)
    }

    pub fn save(&mut self, path: &Path) -> Result<(), String> {
        let Some(scene) = self.scene.as_mut() else {
            return Err(crate::strings::TOAST_NO_SCENE.into());
        };
        guard(|| scene.save(path))?.map_err(|e| format!("{e:#}"))?;
        self.path = Some(path.to_path_buf());
        self.mark_saved();
        Ok(())
    }

    /// Save a copy without changing the document's path or saved marker.
    pub fn save_copy(&mut self, path: &Path) -> Result<(), String> {
        let Some(scene) = self.scene.as_mut() else {
            return Err(crate::strings::TOAST_NO_SCENE.into());
        };
        guard(|| scene.save_copy(path))?.map_err(|e| format!("{e:#}"))
    }

    /// Store the viewport camera in the document (not an undo step).
    pub fn set_camera(&mut self, c: newtopo_core::doc::Camera) {
        if let Some(scene) = self.scene.as_mut() {
            let _ = guard(|| scene.set_camera(c));
        }
    }

    pub fn open(&mut self, path: &Path) -> Result<(), String> {
        let kernel =
            make_kernel().ok_or_else(|| crate::strings::TOAST_KERNEL_MISSING.to_string())?;
        let scene = guard(|| Scene::load(kernel, path))?.map_err(|e| format!("{e:#}"))?;
        self.set_scene(scene, Some(path.to_path_buf()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_catches_todo() {
        let r: Result<(), String> = guard(|| todo!("engine"));
        assert!(
            r.unwrap_err()
                .contains(crate::strings::TOAST_ENGINE_MISSING)
        );
        assert_eq!(guard(|| 3), Ok(3));
    }

    #[test]
    fn no_scene_degrades() {
        let mut d = DocState::default();
        assert!(!d.is_dirty());
        assert_eq!(d.apply(Op::PurgeDormant), Err(ApplyError::NoScene));
        assert_eq!(d.undo(), Ok(false));
        assert_eq!(d.title(), crate::strings::UNTITLED);
        assert!(d.ensure_scene().is_ok(), "default kernel available");
    }
}
