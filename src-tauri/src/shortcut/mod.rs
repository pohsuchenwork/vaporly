//! Keyboard shortcut management module
//!
//! This module provides a unified interface for keyboard shortcuts with
//! multiple backend implementations:
//!
//! - `tauri`: Uses Tauri's built-in global-shortcut plugin
//! - `vaporly_native`: native key capture (vaporly_keys library) for more control
//!
//! The active implementation is determined by the `keyboard_implementation`
//! setting and can be changed at runtime.

mod handler;
pub mod native_keys;
mod tauri_impl;

use log::{error, info, warn};
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_autostart::ManagerExt;

use crate::settings::{
    self, get_settings, AccentPreset, AutoLearnMode, ContextAwarenessSettings, FeatureLevel,
    KeyboardImplementation, OverlayPosition, OverlayStyle, ShortcutBinding, SoundTheme,
    StageEngine, ThemeMode, WhisperStrength,
};

// Note: Commands are accessed via shortcut::native_keys:: in lib.rs

/// Initialize shortcuts using the configured implementation
pub fn init_shortcuts(app: &AppHandle) {
    let user_settings = settings::load_or_create_app_settings(app);

    // Check which implementation to use
    match user_settings.keyboard_implementation {
        KeyboardImplementation::Tauri => {
            tauri_impl::init_shortcuts(app);
        }
        KeyboardImplementation::VaporlyNative => {
            if let Err(e) = native_keys::init_shortcuts(app) {
                error!("Failed to initialize native-keys shortcuts: {}", e);
                // Fall back to Tauri implementation and persist this fallback
                warn!("Falling back to Tauri global shortcut implementation and saving fallback to settings");

                // Update settings to persist the fallback so we don't retry native keys on next launch
                let mut settings = settings::get_settings(app);
                settings.keyboard_implementation = KeyboardImplementation::Tauri;
                // The fn family cannot register under Tauri: rewrite any fn
                // binding to its registrable fallback so the dictation keys
                // stay alive on this backend.
                rewrite_fn_bindings_for_tauri(&mut settings);
                settings::write_settings(app, settings);

                tauri_impl::init_shortcuts(app);
            }
        }
    }
}

/// Register the cancel shortcut (called when recording starts)
pub fn register_cancel_shortcut(app: &AppHandle) {
    let settings = get_settings(app);
    match settings.keyboard_implementation {
        KeyboardImplementation::Tauri => tauri_impl::register_cancel_shortcut(app),
        KeyboardImplementation::VaporlyNative => native_keys::register_cancel_shortcut(app),
    }
}

/// Unregister the cancel shortcut (called when recording stops)
pub fn unregister_cancel_shortcut(app: &AppHandle) {
    let settings = get_settings(app);
    match settings.keyboard_implementation {
        KeyboardImplementation::Tauri => tauri_impl::unregister_cancel_shortcut(app),
        KeyboardImplementation::VaporlyNative => native_keys::unregister_cancel_shortcut(app),
    }
}

/// Register a shortcut using the appropriate implementation
pub fn register_shortcut(app: &AppHandle, binding: ShortcutBinding) -> Result<(), String> {
    let settings = get_settings(app);
    match settings.keyboard_implementation {
        KeyboardImplementation::Tauri => tauri_impl::register_shortcut(app, binding),
        KeyboardImplementation::VaporlyNative => native_keys::register_shortcut(app, binding),
    }
}

/// Unregister a shortcut using the appropriate implementation
pub fn unregister_shortcut(app: &AppHandle, binding: ShortcutBinding) -> Result<(), String> {
    let settings = get_settings(app);
    match settings.keyboard_implementation {
        KeyboardImplementation::Tauri => tauri_impl::unregister_shortcut(app, binding),
        KeyboardImplementation::VaporlyNative => native_keys::unregister_shortcut(app, binding),
    }
}

// ============================================================================
// Binding Management Commands
// ============================================================================

#[derive(Serialize, Type)]
pub struct BindingResponse {
    success: bool,
    binding: Option<ShortcutBinding>,
    error: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub fn change_binding(
    app: AppHandle,
    id: String,
    binding: String,
) -> Result<BindingResponse, String> {
    let mut settings = settings::get_settings(&app);

    // An empty binding means "unbound": unregister whatever was there and
    // persist the empty value (every registrar skips empty bindings). This is
    // also how reset works for optional keys whose default is unbound.
    if binding.trim().is_empty() {
        let existing = settings
            .bindings
            .get(&id)
            .cloned()
            .or_else(|| settings::get_default_settings().bindings.get(&id).cloned());
        let Some(mut b) = existing else {
            return Err(format!("Binding with id '{}' not found", id));
        };
        if id != "cancel" && !b.current_binding.is_empty() {
            if let Err(e) = unregister_shortcut(&app, b.clone()) {
                error!("change_binding unbind: failed to unregister: {}", e);
            }
        }
        b.current_binding = String::new();
        settings.bindings.insert(id, b.clone());
        settings::write_settings(&app, settings);
        return Ok(BindingResponse {
            success: true,
            binding: Some(b),
            error: None,
        });
    }

    // Get the binding to modify, or create it from defaults if it doesn't exist
    let binding_to_modify = match settings.bindings.get(&id) {
        Some(binding) => binding.clone(),
        None => {
            // Try to get the default binding for this id
            let default_settings = settings::get_default_settings();
            match default_settings.bindings.get(&id) {
                Some(default_binding) => {
                    warn!(
                        "Binding '{}' not found in settings, creating from defaults",
                        id
                    );
                    default_binding.clone()
                }
                None => {
                    let error_msg = format!("Binding with id '{}' not found in defaults", id);
                    warn!("change_binding error: {}", error_msg);
                    return Ok(BindingResponse {
                        success: false,
                        binding: None,
                        error: Some(error_msg),
                    });
                }
            }
        }
    };

    // If this is the cancel binding, just update the settings and return
    // It's managed dynamically, so we don't register/unregister here
    if id == "cancel" {
        if let Some(mut b) = settings.bindings.get(&id).cloned() {
            b.current_binding = binding;
            settings.bindings.insert(id.clone(), b.clone());
            settings::write_settings(&app, settings);
            return Ok(BindingResponse {
                success: true,
                binding: Some(b.clone()),
                error: None,
            });
        }
    }

    // Unregister the existing binding (nothing to do when it was unbound)
    if !binding_to_modify.current_binding.is_empty() {
        if let Err(e) = unregister_shortcut(&app, binding_to_modify.clone()) {
            let error_msg = format!("Failed to unregister shortcut: {}", e);
            error!("change_binding error: {}", error_msg);
        }
    }

    // Validate the new shortcut for the current keyboard implementation
    if let Err(e) = validate_shortcut_for_implementation(&binding, settings.keyboard_implementation)
    {
        warn!("change_binding validation error: {}", e);
        return Err(e);
    }

    // Create an updated binding
    let mut updated_binding = binding_to_modify;
    updated_binding.current_binding = binding;

    // Register the new binding
    if let Err(e) = register_shortcut(&app, updated_binding.clone()) {
        let error_msg = format!("Failed to register shortcut: {}", e);
        error!("change_binding error: {}", error_msg);
        return Ok(BindingResponse {
            success: false,
            binding: None,
            error: Some(error_msg),
        });
    }

    // Update the binding in the settings
    settings.bindings.insert(id, updated_binding.clone());

    // Save the settings
    settings::write_settings(&app, settings);

    // Return the updated binding
    Ok(BindingResponse {
        success: true,
        binding: Some(updated_binding),
        error: None,
    })
}

#[tauri::command]
#[specta::specta]
pub fn reset_binding(app: AppHandle, id: String) -> Result<BindingResponse, String> {
    let binding = settings::get_stored_binding(&app, &id);
    change_binding(app, id, binding.default_binding)
}

/// Temporarily unregister a binding while the user is editing it in the UI.
/// This avoids firing the action while keys are being recorded.
#[tauri::command]
#[specta::specta]
pub fn suspend_binding(app: AppHandle, id: String) -> Result<(), String> {
    if let Some(b) = settings::get_bindings(&app).get(&id).cloned() {
        if b.current_binding.is_empty() {
            return Ok(()); // unbound: nothing registered to suspend
        }
        if let Err(e) = unregister_shortcut(&app, b) {
            error!("suspend_binding error for id '{}': {}", id, e);
            return Err(e);
        }
    }
    Ok(())
}

/// Re-register the binding after the user has finished editing.
#[tauri::command]
#[specta::specta]
pub fn resume_binding(app: AppHandle, id: String) -> Result<(), String> {
    if let Some(b) = settings::get_bindings(&app).get(&id).cloned() {
        if b.current_binding.is_empty() {
            return Ok(()); // unbound: nothing to re-register
        }
        if let Err(e) = register_shortcut(&app, b) {
            error!("resume_binding error for id '{}': {}", id, e);
            return Err(e);
        }
    }
    Ok(())
}

// ============================================================================
// Keyboard Implementation Switching
// ============================================================================

/// Result of changing keyboard implementation
#[derive(Serialize, Type)]
pub struct ImplementationChangeResult {
    pub success: bool,
    /// List of binding IDs that were reset to defaults due to incompatibility
    pub reset_bindings: Vec<String>,
}

/// Change the keyboard implementation with runtime switching.
/// This will unregister all shortcuts from the old implementation,
/// validate shortcuts for the new implementation (resetting invalid ones to defaults),
/// and register them with the new implementation.
#[tauri::command]
#[specta::specta]
pub fn change_keyboard_implementation_setting(
    app: AppHandle,
    implementation: String,
) -> Result<ImplementationChangeResult, String> {
    let current_settings = settings::get_settings(&app);
    let current_impl = current_settings.keyboard_implementation;
    let new_impl = parse_keyboard_implementation(&implementation);

    // If same implementation, nothing to do
    if current_impl == new_impl {
        return Ok(ImplementationChangeResult {
            success: true,
            reset_bindings: vec![],
        });
    }

    info!(
        "Switching keyboard implementation from {:?} to {:?}",
        current_impl, new_impl
    );

    // Unregister all shortcuts from the current implementation
    unregister_all_shortcuts(&app, current_impl);

    // Update the setting
    let mut settings = settings::get_settings(&app);
    settings.keyboard_implementation = new_impl;
    settings::write_settings(&app, settings);

    // Initialize new implementation if needed (native keys needs state)
    if new_impl == KeyboardImplementation::VaporlyNative
        && initialize_native_keys_with_rollback(&app)?
    {
        // Shortcuts already registered during init
        return Ok(ImplementationChangeResult {
            success: true,
            reset_bindings: vec![],
        });
    }

    // Register all shortcuts with new implementation, resetting invalid ones
    let reset_bindings = register_all_shortcuts_for_implementation(&app, new_impl);

    // Emit event to notify frontend of the change
    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "keyboard_implementation",
            "value": implementation,
            "reset_bindings": reset_bindings
        }),
    );

    info!("Keyboard implementation switched to {:?}", new_impl);

    Ok(ImplementationChangeResult {
        success: true,
        reset_bindings,
    })
}

/// Get the current keyboard implementation
#[tauri::command]
#[specta::specta]
pub fn get_keyboard_implementation(app: AppHandle) -> String {
    let settings = settings::get_settings(&app);
    match settings.keyboard_implementation {
        KeyboardImplementation::Tauri => "tauri".to_string(),
        KeyboardImplementation::VaporlyNative => "vaporly_native".to_string(),
    }
}

// ============================================================================
// Validation Helpers
// ============================================================================

/// Validate a shortcut for a specific implementation
fn validate_shortcut_for_implementation(
    raw: &str,
    implementation: KeyboardImplementation,
) -> Result<(), String> {
    match implementation {
        KeyboardImplementation::Tauri => tauri_impl::validate_shortcut(raw),
        KeyboardImplementation::VaporlyNative => native_keys::validate_shortcut(raw),
    }
}

/// Parse a keyboard implementation string into the enum
fn parse_keyboard_implementation(s: &str) -> KeyboardImplementation {
    match s {
        "tauri" => KeyboardImplementation::Tauri,
        // "handy_keys" accepted for pre-rename stores/UI payloads.
        "vaporly_native" | "handy_keys" => KeyboardImplementation::VaporlyNative,
        other => {
            warn!(
                "Invalid keyboard implementation '{}', defaulting to tauri",
                other
            );
            KeyboardImplementation::Tauri
        }
    }
}

/// Unregister all shortcuts for the current implementation
fn unregister_all_shortcuts(app: &AppHandle, implementation: KeyboardImplementation) {
    let bindings = settings::get_bindings(app);

    for (id, binding) in bindings {
        // Skip cancel shortcut as it's dynamically registered
        if id == "cancel" {
            continue;
        }

        let result = match implementation {
            KeyboardImplementation::Tauri => tauri_impl::unregister_shortcut(app, binding),
            KeyboardImplementation::VaporlyNative => native_keys::unregister_shortcut(app, binding),
        };

        if let Err(e) = result {
            warn!(
                "Failed to unregister shortcut '{}' during switch: {}",
                id, e
            );
        }
    }
}

/// Register all shortcuts for a specific implementation, validating and resetting invalid ones
fn register_all_shortcuts_for_implementation(
    app: &AppHandle,
    implementation: KeyboardImplementation,
) -> Vec<String> {
    let mut reset_bindings = Vec::new();
    let default_bindings = settings::get_default_settings().bindings;
    let mut current_settings = settings::get_settings(app);

    for (id, default_binding) in &default_bindings {
        // Skip cancel shortcut as it's dynamically registered
        if id == "cancel" {
            continue;
        }

        let mut binding = current_settings
            .bindings
            .get(id)
            .cloned()
            .unwrap_or_else(|| default_binding.clone());

        // Unbound (empty) bindings stay unregistered on every backend.
        if binding.current_binding.is_empty() {
            continue;
        }

        // Validate the shortcut for the target implementation
        if let Err(e) =
            validate_shortcut_for_implementation(&binding.current_binding, implementation)
        {
            info!(
                "Shortcut '{}' ({}) is invalid for {:?}: {}. Resetting to default.",
                id, binding.current_binding, implementation, e
            );

            // Reset to the default; when the default itself cannot register on
            // this backend (the macOS fn family under Tauri), use the
            // registrable fallback and rewrite the stored default so
            // Reset-to-default stays functional too.
            let mut target = default_binding.current_binding.clone();
            if validate_shortcut_for_implementation(&target, implementation).is_err() {
                if let Some(fallback) = settings::tauri_safe_fallback(id) {
                    target = fallback.to_string();
                    binding.default_binding = fallback.to_string();
                }
            }
            binding.current_binding = target;
            current_settings
                .bindings
                .insert(id.clone(), binding.clone());
            reset_bindings.push(id.clone());
        }

        // Register with the appropriate implementation
        let result = match implementation {
            KeyboardImplementation::Tauri => tauri_impl::register_shortcut(app, binding),
            KeyboardImplementation::VaporlyNative => native_keys::register_shortcut(app, binding),
        };

        if let Err(e) = result {
            error!(
                "Failed to register shortcut '{}' for {:?}: {}",
                id, implementation, e
            );
        }
    }

    // Save settings if any bindings were reset
    if !reset_bindings.is_empty() {
        settings::write_settings(app, current_settings);
    }

    reset_bindings
}

/// Initialize HandyKeys if not already initialized, with rollback on failure
fn initialize_native_keys_with_rollback(app: &AppHandle) -> Result<bool, String> {
    if app.try_state::<native_keys::NativeKeysState>().is_some() {
        return Ok(false); // Already initialized, caller should continue
    }

    if let Err(e) = native_keys::init_shortcuts(app) {
        error!("Failed to initialize HandyKeys: {}", e);
        // Rollback to Tauri
        let mut settings = settings::get_settings(app);
        settings.keyboard_implementation = KeyboardImplementation::Tauri;
        rewrite_fn_bindings_for_tauri(&mut settings);
        settings::write_settings(app, settings);
        tauri_impl::init_shortcuts(app);
        return Err(format!(
            "Failed to initialize HandyKeys: {}. Reverted to Tauri.",
            e
        ));
    }

    // init_shortcuts already registered shortcuts
    Ok(true)
}

// ============================================================================
// General Settings Commands
// ============================================================================

/// Rewrite every binding that uses the fn modifier to its Tauri-registrable
/// fallback (both current and default). No-op for bindings without fn.
fn rewrite_fn_bindings_for_tauri(settings: &mut crate::settings::AppSettings) {
    for (id, binding) in settings.bindings.iter_mut() {
        if !settings::binding_uses_fn(&binding.current_binding)
            && !settings::binding_uses_fn(&binding.default_binding)
        {
            continue;
        }
        if let Some(fallback) = settings::tauri_safe_fallback(id) {
            info!(
                "Rewriting fn binding '{}' ({}) to Tauri-safe '{}'",
                id, binding.current_binding, fallback
            );
            if settings::binding_uses_fn(&binding.current_binding) {
                binding.current_binding = fallback.to_string();
            }
            if settings::binding_uses_fn(&binding.default_binding) {
                binding.default_binding = fallback.to_string();
            }
        }
    }
}

/// Stage-engine settings gate the bundled LLM engine: after any of them
/// changes, ask the engine manager to reconcile (it starts when a stage wants
/// the model and stops when nothing does).
fn reconcile_llm_engine(app: &AppHandle) {
    if let Some(engine) =
        app.try_state::<std::sync::Arc<crate::managers::llm_engine::LlmEngineManager>>()
    {
        engine.ensure_running();
    }
}

#[tauri::command]
#[specta::specta]
pub fn change_globe_key_notice_dismissed_setting(
    app: AppHandle,
    dismissed: bool,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.globe_key_notice_dismissed = dismissed;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_audio_feedback_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.audio_feedback = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_audio_feedback_volume_setting(app: AppHandle, volume: f32) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.audio_feedback_volume = volume;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_sound_theme_setting(app: AppHandle, theme: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let parsed = match theme.as_str() {
        "marimba" => SoundTheme::Marimba,
        "pop" => SoundTheme::Pop,
        "chime" => SoundTheme::Chime,
        "bubble" => SoundTheme::Bubble,
        "breeze" => SoundTheme::Breeze,
        "custom" => SoundTheme::Custom,
        other => {
            warn!("Invalid sound theme '{}', defaulting to marimba", other);
            SoundTheme::Marimba
        }
    };
    settings.sound_theme = parsed;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_overlay_position_setting(app: AppHandle, position: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let parsed = match position.as_str() {
        "bottom" => OverlayPosition::Bottom,
        "top" => OverlayPosition::Top,
        other => {
            warn!("Invalid overlay position '{}', defaulting to bottom", other);
            OverlayPosition::Bottom
        }
    };
    settings.overlay_position = parsed;
    settings::write_settings(&app, settings);

    // Whether the overlay shows at all is owned by overlay_style; position
    // only ever toggles Top/Bottom, so the enabled cache is untouched here.
    crate::utils::update_overlay_position(&app);

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_overlay_style_setting(app: AppHandle, style: String) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    let parsed = match style.as_str() {
        "none" => OverlayStyle::None,
        "bar" => OverlayStyle::Bar,
        "bar_live" => OverlayStyle::BarLive,
        "textbox_raw" => OverlayStyle::TextboxRaw,
        "textbox_clean" => OverlayStyle::TextboxClean,
        "inline" | "wispr" => OverlayStyle::Inline,
        other => {
            warn!("Invalid overlay style '{}', defaulting to bar", other);
            OverlayStyle::Bar
        }
    };
    settings.overlay_style = parsed;
    settings::write_settings(&app, settings);

    // Keep the cached overlay-enabled flag in sync so emit_levels stops (or
    // resumes) emitting on the next audio callback.
    crate::overlay::update_overlay_enabled_cache(parsed != OverlayStyle::None);

    // Reposition in case the window needs to re-center for the new style.
    crate::utils::update_overlay_position(&app);

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_autostart_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.autostart_enabled = enabled;
    settings::write_settings(&app, settings);

    // Apply the autostart setting immediately
    let autostart_manager = app.autolaunch();
    if enabled {
        let _ = autostart_manager.enable();
    } else {
        let _ = autostart_manager.disable();
    }

    // Notify frontend
    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "autostart_enabled",
            "value": enabled
        }),
    );

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_update_checks_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.update_checks_enabled = enabled;
    settings::write_settings(&app, settings);

    let _ = app.emit(
        "settings-changed",
        serde_json::json!({
            "setting": "update_checks_enabled",
            "value": enabled
        }),
    );

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn update_custom_words(app: AppHandle, words: Vec<String>) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.custom_words = words;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn update_custom_phrases(
    app: AppHandle,
    phrases: Vec<settings::CustomPhrase>,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.custom_phrases = phrases
        .into_iter()
        .map(|p| settings::CustomPhrase {
            say: sanitize_phrase_text(&p.say, 100, false),
            write: sanitize_phrase_text(&p.write, 10_000, true),
        })
        .filter(|p| !p.say.trim().is_empty() && !p.write.trim().is_empty())
        .collect();
    settings::write_settings(&app, settings);
    Ok(())
}

/// Strip control characters (keeping newline and tab only for multi-line
/// write texts) and cap the length at a char boundary. Defense in depth
/// behind the UI caps; phrase text lands inside LLM prompts.
fn sanitize_phrase_text(raw: &str, max_chars: usize, keep_newlines: bool) -> String {
    raw.chars()
        .filter(|c| {
            if c.is_control() {
                keep_newlines && (*c == '\n' || *c == '\t')
            } else {
                true
            }
        })
        .take(max_chars)
        .collect()
}

#[tauri::command]
#[specta::specta]
pub fn change_append_trailing_space_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.append_trailing_space = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_keep_result_on_clipboard_setting(
    app: AppHandle,
    enabled: bool,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.keep_result_on_clipboard = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_onboarding_completed_setting(app: AppHandle, completed: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.onboarding_completed = completed;
    settings::write_settings(&app, settings);
    Ok(())
}

// ============================================================================
// Stage-dial Commands (custom words, filler fix up, mind-change, context)
// ============================================================================

#[tauri::command]
#[specta::specta]
pub fn change_custom_words_level_setting(
    app: AppHandle,
    level: FeatureLevel,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.custom_words_level = level;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_theme_mode_setting(app: AppHandle, mode: ThemeMode) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.theme_mode = mode;
    settings::write_settings(&app, settings);
    // Both webview windows repaint from this event (the overlay has no
    // settings store of its own).
    let _ = app.emit("appearance-changed", ());
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_accent_preset_setting(app: AppHandle, preset: AccentPreset) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.accent_preset = preset;
    settings::write_settings(&app, settings);
    let _ = app.emit("appearance-changed", ());
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_whisper_mode_setting(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.whisper_mode = enabled;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_whisper_strength_setting(
    app: AppHandle,
    strength: WhisperStrength,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.whisper_strength = strength;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_custom_phrases_level_setting(
    app: AppHandle,
    level: FeatureLevel,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.custom_phrases_level = level;
    settings::write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_filler_level_setting(app: AppHandle, level: FeatureLevel) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.filler_level = level;
    settings::write_settings(&app, settings);
    reconcile_llm_engine(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_filler_engine_setting(app: AppHandle, engine: StageEngine) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.filler_engine = engine;
    settings::write_settings(&app, settings);
    reconcile_llm_engine(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_mind_change_level_setting(app: AppHandle, level: FeatureLevel) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.mind_change_level = level;
    settings::write_settings(&app, settings);
    reconcile_llm_engine(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_mind_change_engine_setting(
    app: AppHandle,
    engine: StageEngine,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.mind_change_engine = engine;
    settings::write_settings(&app, settings);
    reconcile_llm_engine(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_context_awareness_setting(
    app: AppHandle,
    value: ContextAwarenessSettings,
) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.context_awareness = value;
    settings::write_settings(&app, settings);
    reconcile_llm_engine(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn change_auto_learn_mode_setting(app: AppHandle, mode: AutoLearnMode) -> Result<(), String> {
    let mut settings = settings::get_settings(&app);
    settings.auto_learn_mode = mode;
    settings::write_settings(&app, settings);
    Ok(())
}
