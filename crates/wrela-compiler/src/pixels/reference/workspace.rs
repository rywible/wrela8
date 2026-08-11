//! Fixed-capacity renderer workspace and count-only reset protocol.

use super::telemetry::CertificateTelemetry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceError {
    CapacityExceeded,
    ProgramIndex,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixedStore<T: Copy + Default, const N: usize> {
    values: [T; N],
    count: usize,
}

impl<T: Copy + Default, const N: usize> Default for FixedStore<T, N> {
    fn default() -> Self {
        Self {
            values: [T::default(); N],
            count: 0,
        }
    }
}

impl<T: Copy + Default, const N: usize> FixedStore<T, N> {
    pub fn reset(&mut self) {
        self.count = 0;
    }

    pub fn push(&mut self, value: T) -> Result<usize, WorkspaceError> {
        let Some(slot) = self.values.get_mut(self.count) else {
            return Err(WorkspaceError::CapacityExceeded);
        };
        *slot = value;
        let index = self.count;
        self.count += 1;
        Ok(index)
    }

    pub fn get(&self, index: usize) -> Result<T, WorkspaceError> {
        if index >= self.count {
            return Err(WorkspaceError::ProgramIndex);
        }
        Ok(self.values[index])
    }

    pub fn as_slice(&self) -> &[T] {
        &self.values[..self.count]
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.values[..self.count]
    }

    pub const fn capacity(&self) -> usize {
        N
    }

    pub const fn len(&self) -> usize {
        self.count
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerWorkspace<
    Root: Copy + Default,
    Event: Copy + Default,
    Run: Copy + Default,
    Rebuild: Copy + Default,
    const ROOTS: usize,
    const EVENTS: usize,
    const RUNS: usize,
    const REBUILDS: usize,
> {
    pub roots: FixedStore<Root, ROOTS>,
    pub roots_tmp: FixedStore<Root, ROOTS>,
    pub events: FixedStore<Event, EVENTS>,
    pub runs: FixedStore<Run, RUNS>,
    pub rebuild: FixedStore<Rebuild, REBUILDS>,
    pub telemetry: CertificateTelemetry,
}

impl<
    Root: Copy + Default,
    Event: Copy + Default,
    Run: Copy + Default,
    Rebuild: Copy + Default,
    const ROOTS: usize,
    const EVENTS: usize,
    const RUNS: usize,
    const REBUILDS: usize,
> Default for WorkerWorkspace<Root, Event, Run, Rebuild, ROOTS, EVENTS, RUNS, REBUILDS>
{
    fn default() -> Self {
        Self {
            roots: FixedStore::default(),
            roots_tmp: FixedStore::default(),
            events: FixedStore::default(),
            runs: FixedStore::default(),
            rebuild: FixedStore::default(),
            telemetry: CertificateTelemetry::default(),
        }
    }
}

impl<
    Root: Copy + Default,
    Event: Copy + Default,
    Run: Copy + Default,
    Rebuild: Copy + Default,
    const ROOTS: usize,
    const EVENTS: usize,
    const RUNS: usize,
    const REBUILDS: usize,
> WorkerWorkspace<Root, Event, Run, Rebuild, ROOTS, EVENTS, RUNS, REBUILDS>
{
    pub fn reset_for_tile(&mut self) {
        self.roots.reset();
        self.roots_tmp.reset();
        self.events.reset();
        self.runs.reset();
        self.rebuild.reset();
        self.telemetry = CertificateTelemetry::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Workspace = WorkerWorkspace<u32, u16, u64, u8, 2, 1, 2, 1>;

    #[test]
    fn every_push_is_checked_and_reset_hides_old_records() {
        let mut workspace = Workspace::default();
        workspace.roots.push(7).unwrap();
        workspace.roots.push(9).unwrap();
        assert_eq!(
            workspace.roots.push(11),
            Err(WorkspaceError::CapacityExceeded)
        );
        workspace.reset_for_tile();
        assert_eq!(workspace.roots.len(), 0);
        assert_eq!(workspace.roots.get(0), Err(WorkspaceError::ProgramIndex));
        workspace.roots.push(13).unwrap();
        assert_eq!(workspace.roots.as_slice(), &[13]);
    }

    #[test]
    fn independent_workspaces_do_not_share_storage() {
        let mut first = Workspace::default();
        let mut second = Workspace::default();
        first.roots.push(1).unwrap();
        second.roots.push(2).unwrap();
        assert_eq!(first.roots.as_slice(), &[1]);
        assert_eq!(second.roots.as_slice(), &[2]);
    }
}
