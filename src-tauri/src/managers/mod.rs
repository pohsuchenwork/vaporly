pub mod audio;
// Header-level GGUF parsing. Production consumers (the local-model discovery
// scans) were removed with the fixed-model strip-down; the parser and its
// tests stay for the next metadata consumer.
#[allow(dead_code)]
pub mod gguf_meta;
pub mod hardware;
pub mod history;
pub mod llm_catalog;
pub mod llm_engine;
pub mod model;
pub mod model_capabilities;
pub mod transcription;
