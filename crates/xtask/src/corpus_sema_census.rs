pub use wrela_compiler::census::CorpusSemaPin;

pub fn pins() -> &'static [CorpusSemaPin] {
    &wrela_compiler::census::data().corpus_sema_pins
}
