//! Explicit limits for finite symbolic compilation.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolicQuota {
    pub max_steps: u64,
    pub max_nodes: usize,
    pub max_call_depth: usize,
    pub max_unrolled_statements: u64,
    pub max_aggregate_elements: usize,
    pub steps: u64,
    pub unrolled_statements: u64,
    pub aggregate_elements: usize,
    pub peak_call_depth: usize,
    pub peak_nodes: usize,
}

impl Default for SymbolicQuota {
    fn default() -> Self {
        Self {
            max_steps: 1_000_000,
            max_nodes: 250_000,
            max_call_depth: 128,
            max_unrolled_statements: 250_000,
            max_aggregate_elements: 250_000,
            steps: 0,
            unrolled_statements: 0,
            aggregate_elements: 0,
            peak_call_depth: 0,
            peak_nodes: 0,
        }
    }
}

impl SymbolicQuota {
    pub fn step(&mut self, stack: &[String]) -> Result<(), String> {
        self.steps = self
            .steps
            .checked_add(1)
            .ok_or_else(|| self.error(stack, "step counter"))?;
        if self.steps > self.max_steps {
            return Err(self.error(stack, "symbolic step quota"));
        }
        Ok(())
    }

    pub fn call(&mut self, stack: &[String]) -> Result<(), String> {
        if stack.len() > self.max_call_depth {
            return Err(self.error(stack, "call-depth quota"));
        }
        self.peak_call_depth = self.peak_call_depth.max(stack.len());
        Ok(())
    }

    pub fn node(&mut self, count: usize, stack: &[String]) -> Result<(), String> {
        if count >= self.max_nodes {
            return Err(self.error(stack, "symbolic node quota"));
        }
        self.peak_nodes = self.peak_nodes.max(count + 1);
        Ok(())
    }

    pub fn unroll(&mut self, count: u64, stack: &[String]) -> Result<(), String> {
        self.unrolled_statements = self
            .unrolled_statements
            .checked_add(count)
            .ok_or_else(|| self.error(stack, "loop-expansion counter"))?;
        if self.unrolled_statements > self.max_unrolled_statements {
            return Err(self.error(stack, "loop-expansion quota"));
        }
        Ok(())
    }

    pub fn aggregate(&mut self, count: usize, stack: &[String]) -> Result<(), String> {
        self.aggregate_elements = self
            .aggregate_elements
            .checked_add(count)
            .ok_or_else(|| self.error(stack, "aggregate counter"))?;
        if self.aggregate_elements > self.max_aggregate_elements {
            return Err(self.error(stack, "symbolic-memory quota"));
        }
        Ok(())
    }

    fn error(&self, stack: &[String], kind: &str) -> String {
        format!(
            "P014: Pixels symbolic compilation exhausted {kind}\n  renderer call chain: {}",
            stack.join(" -> ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack() -> Vec<String> {
        vec!["scene::world".to_string(), "scene::helper".to_string()]
    }

    #[test]
    fn every_symbolic_ceiling_fails_closed_with_the_call_chain() {
        let stack = stack();

        let mut steps = SymbolicQuota {
            max_steps: 0,
            ..Default::default()
        };
        assert!(steps.step(&stack).unwrap_err().contains("scene::helper"));

        let mut nodes = SymbolicQuota {
            max_nodes: 1,
            ..Default::default()
        };
        assert!(nodes.node(1, &stack).unwrap_err().contains("node quota"));

        let mut calls = SymbolicQuota {
            max_call_depth: 1,
            ..Default::default()
        };
        assert!(calls.call(&stack).unwrap_err().contains("call-depth quota"));

        let mut loops = SymbolicQuota {
            max_unrolled_statements: 1,
            ..Default::default()
        };
        assert!(
            loops
                .unroll(2, &stack)
                .unwrap_err()
                .contains("loop-expansion quota")
        );

        let mut memory = SymbolicQuota {
            max_aggregate_elements: 1,
            ..Default::default()
        };
        assert!(
            memory
                .aggregate(2, &stack)
                .unwrap_err()
                .contains("symbolic-memory quota")
        );
    }

    #[test]
    fn call_quota_records_the_observed_peak() {
        let mut quota = SymbolicQuota::default();
        quota.call(&["scene::world".to_string()]).unwrap();
        quota.call(&stack()).unwrap();
        assert_eq!(quota.peak_call_depth, 2);
    }

    #[test]
    fn node_quota_records_pre_canonicalization_peak() {
        let mut quota = SymbolicQuota::default();
        quota.node(0, &stack()).unwrap();
        quota.node(7, &stack()).unwrap();
        assert_eq!(quota.peak_nodes, 8);
    }
}
