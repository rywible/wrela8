pub fn classify_owner(key: &str) -> &'static str {
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
