//! Tauri commands for the bundled LLM engine (status, models, lifecycle).

use crate::managers::hardware::{self, HardwareProfile};
use crate::managers::llm_engine::{EngineStatus, LlmEngineManager, LlmModelStatus};
use std::sync::Arc;
use tauri::State;

#[tauri::command]
#[specta::specta]
pub fn get_llm_engine_status(engine: State<'_, Arc<LlmEngineManager>>) -> EngineStatus {
    engine.status()
}

#[tauri::command]
#[specta::specta]
pub fn restart_llm_engine(engine: State<'_, Arc<LlmEngineManager>>) {
    engine.repair();
}

#[tauri::command]
#[specta::specta]
pub fn get_hardware_profile() -> HardwareProfile {
    hardware::profile().clone()
}

#[tauri::command]
#[specta::specta]
pub fn get_llm_models(engine: State<'_, Arc<LlmEngineManager>>) -> Vec<LlmModelStatus> {
    engine.model_infos()
}

#[tauri::command]
#[specta::specta]
pub async fn download_llm_model(
    engine: State<'_, Arc<LlmEngineManager>>,
    model_id: String,
) -> Result<(), String> {
    let engine = Arc::clone(&engine);
    engine
        .download_model(&model_id)
        .await
        .map_err(|e| e.to_string())?;
    // A freshly downloaded model may be exactly what the engine was waiting on.
    engine.ensure_running();
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn cancel_llm_model_download(engine: State<'_, Arc<LlmEngineManager>>, model_id: String) {
    engine.cancel_model_download(&model_id);
}

#[tauri::command]
#[specta::specta]
pub fn delete_llm_model(
    engine: State<'_, Arc<LlmEngineManager>>,
    model_id: String,
) -> Result<(), String> {
    engine.delete_model(&model_id).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub fn set_llm_model(
    engine: State<'_, Arc<LlmEngineManager>>,
    model_id: String,
) -> Result<(), String> {
    engine.set_model(&model_id).map_err(|e| e.to_string())
}

/// One tiny completion through the live engine, the settings "Test" button
/// and the onboarding finish check.
#[tauri::command]
#[specta::specta]
pub async fn llm_engine_selftest(app: tauri::AppHandle) -> Result<String, String> {
    let settings = crate::settings::get_settings(&app);
    let text = "um so lets meet at three no wait four pm";
    let prompt = format!(
        "Clean this dictation (remove fillers, apply self-corrections), reply with ONLY the cleaned text:\n{text}"
    );
    match crate::actions::llm_complete(&settings, prompt).await {
        Some(out) if !out.trim().is_empty() => Ok(out),
        _ => Err("engine returned no text (is it Ready?)".to_string()),
    }
}
