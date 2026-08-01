use crate::sema::stdlib_enums::AUTO_VISIBLE;

pub const FIXED_PRELUDE: &[&str] = &[
    "Option",
    "Some",
    "None",
    "Result",
    "Ok",
    "Err",
    "panic",
    "CallError",
    "Admission",
];

pub const TIME_PRELUDE_NAMES: &[&str] = &[
    "Duration", "Instant", "ns", "us", "ms", "seconds", "minutes", "hours",
];

pub const STDLIB_AUTO_VISIBLE: &[&str] = AUTO_VISIBLE;

pub fn is_fixed_prelude_name(name: &str) -> bool {
    FIXED_PRELUDE.contains(&name)
}
