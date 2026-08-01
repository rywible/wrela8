pub const MAX_STEPS: u64 = 20_000;

pub const MAX_MEMORY: u64 = 10_000_000;

pub const MAX_CALL_DEPTH: usize = 1_000;

pub const MAX_EXHAUSTIVE_CASES: u128 = 65_536;

#[derive(Debug, Clone, Default)]
pub struct Quota {
    steps: u64,
    memory: u64,
}

impl Quota {
    pub fn new() -> Quota {
        Quota::default()
    }

    pub fn tick_step(&mut self) -> Result<(), String> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            Err(format!("step quota exceeded ({MAX_STEPS} steps)"))
        } else {
            Ok(())
        }
    }

    pub fn charge_memory(&mut self, n: u64) -> Result<(), String> {
        self.memory += n;
        if self.memory > MAX_MEMORY {
            Err(format!("memory quota exceeded ({MAX_MEMORY} elements)"))
        } else {
            Ok(())
        }
    }
}
