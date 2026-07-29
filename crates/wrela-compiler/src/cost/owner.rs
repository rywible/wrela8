//! Owner buckets for proxy-cycle ranking (plans/M18.md item G, freeze 1365).

/// Classify a fn key into `app` / `runtime` / `driver`.
pub fn classify_owner(key: &str) -> &'static str {
    // `core.runtime` stays local to this bucket on purpose: it is a real
    // source module, not glue, and folding it into the shared rule would
    // move `layout.rs`'s cross-core redirect decision (and the cost
    // goldens with it).
    if crate::codegen::is_compiler_glue_symbol(key) || key.contains("core.runtime") {
        return "runtime";
    }
    if key.contains(".on_") {
        return "driver";
    }
    "app"
}

#[cfg(test)]
mod tests {
    use super::classify_owner;

    #[test]
    fn synthetic_key_with_space_is_runtime() {
        assert_eq!(classify_owner("rt_run_one 0"), "runtime");
    }

    #[test]
    fn wrela_abort_is_runtime() {
        assert_eq!(classify_owner("__wrela_abort"), "runtime");
    }

    #[test]
    fn ordinary_checked_add_is_app() {
        assert_eq!(classify_owner("checked_add"), "app");
    }

    #[test]
    fn driver_on_turn_is_driver() {
        assert_eq!(classify_owner("BlkDriver.on_turn"), "driver");
    }

    #[test]
    fn driver_on_irq_is_driver() {
        assert_eq!(classify_owner("Foo.on_irq"), "driver");
    }
}
