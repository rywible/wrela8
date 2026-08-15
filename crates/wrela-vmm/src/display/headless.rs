use wrela_machine::pixels::{DisplayQueue, PresentedFrame};

use super::{BackendKind, DisplayDevice, HostPresentError, PresentationBackend};

#[derive(Debug, Default)]
pub struct HeadlessBackend {
    presented: Vec<[u8; 32]>,
}

impl HeadlessBackend {
    pub fn digests(&self) -> &[[u8; 32]] {
        &self.presented
    }
}

impl PresentationBackend for HeadlessBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Headless
    }

    fn present(&mut self, frame: &PresentedFrame) -> Result<(), HostPresentError> {
        self.presented.push(frame.visible_digest);
        Ok(())
    }

    fn presented_digest(&self) -> Option<[u8; 32]> {
        self.presented.last().copied()
    }
}

pub type HeadlessDisplay = DisplayDevice<HeadlessBackend>;

impl Default for HeadlessDisplay {
    fn default() -> Self {
        Self::new(
            DisplayQueue::negotiate_first_mode(),
            HeadlessBackend::default(),
        )
    }
}
