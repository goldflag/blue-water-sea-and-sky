//! The clouds as the transparents drawn after their composite read them. The composite lays the clouds over the scene
//! in the `Sky` phase, before the transparents, and a plume, a puff or a glow drawn after it is not covered by a cloud
//! that stands in front of it: from above a deck of cumulus a funnel's smoke showed through. naval-clouds publishes here
//! each frame what its composite draws (`CloudLayer::cover`), and a layer drawn after it (the combat effects) fades
//! what the clouds hide. It lives here because both crates depend on this one.

use std::sync::{Arc, Mutex};

/// What the clouds' composite draws this frame.
#[derive(Clone, Debug, Default)]
pub struct CloudCoverFrame {
    /// The renderer core's frame it was published for: a reader holds anything else as no clouds (the sky not
    /// prepared this frame draws none).
    pub frame: Option<u64>,
    /// The reconstruction's two histories (premultiplied radiance, opacity in alpha; `Rgba16Float`, the drawing
    /// buffer's size), which the composite alternates between.
    pub histories: Option<[wgpu::TextureView; 2]>,
    /// Bumps whenever `histories` are made again (a resize), so a reader knows to rebind them.
    pub version: u64,
    /// Which history the composite draws this frame; `None` while it draws nothing.
    pub current: Option<usize>,
    /// The bottom of the lowest layer and the top of the highest (m above the sea), over a sea sphere of
    /// `planet_radius` m centred under the camera, as the clouds' march takes it.
    pub bottom: f32,
    pub top: f32,
    pub planet_radius: f32,
}

/// A handle on the clouds' published [`CloudCoverFrame`], shared by the layer that writes it and the layers that read it.
#[derive(Clone, Debug, Default)]
pub struct CloudCover(Arc<Mutex<CloudCoverFrame>>);

impl CloudCover {
    /// Changes the published frame in place.
    pub fn publish(&self, change: impl FnOnce(&mut CloudCoverFrame)) {
        let mut frame = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        change(&mut frame);
    }

    /// The published frame.
    pub fn read(&self) -> CloudCoverFrame {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}
