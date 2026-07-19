use crate::audio_feedback;
use crate::audio_toolkit::audio::{list_input_devices, list_output_devices};
use crate::managers::audio::AudioRecordingManager;
use crate::settings::{get_settings, write_settings};
use log::warn;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

#[cfg(target_os = "windows")]
use winreg::{
    enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
    RegKey, HKEY,
};

#[derive(Serialize, Type)]
pub struct CustomSounds {
    start: bool,
    stop: bool,
}

fn custom_sound_exists(app: &AppHandle, sound_type: &str) -> bool {
    crate::portable::resolve_app_data(app, &format!("custom_{}.wav", sound_type))
        .is_ok_and(|path| path.exists())
}

#[tauri::command]
#[specta::specta]
pub fn check_custom_sounds(app: AppHandle) -> CustomSounds {
    CustomSounds {
        start: custom_sound_exists(&app, "start"),
        stop: custom_sound_exists(&app, "stop"),
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct AudioDevice {
    pub index: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum PermissionAccess {
    Allowed,
    Denied,
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct WindowsMicrophonePermissionStatus {
    pub supported: bool,
    pub overall_access: PermissionAccess,
    pub device_access: PermissionAccess,
    pub app_access: PermissionAccess,
    pub desktop_app_access: PermissionAccess,
}

#[cfg(target_os = "windows")]
fn read_registry_permission_access(root_hkey: HKEY, path: &str) -> PermissionAccess {
    let root = RegKey::predef(root_hkey);
    let Ok(key) = root.open_subkey(path) else {
        return PermissionAccess::Unknown;
    };

    let Ok(value) = key.get_value::<String, _>("Value") else {
        return PermissionAccess::Unknown;
    };

    match value.to_ascii_lowercase().as_str() {
        "allow" => PermissionAccess::Allowed,
        "deny" => PermissionAccess::Denied,
        _ => PermissionAccess::Unknown,
    }
}

#[cfg(target_os = "windows")]
fn get_windows_microphone_permission_status_impl() -> WindowsMicrophonePermissionStatus {
    const MICROPHONE_PATH: &str =
        "Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone";
    const DESKTOP_APPS_PATH: &str =
        "Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone\\NonPackaged";

    let device_access = read_registry_permission_access(HKEY_LOCAL_MACHINE, MICROPHONE_PATH);
    let app_access = read_registry_permission_access(HKEY_CURRENT_USER, MICROPHONE_PATH);
    let desktop_app_access = read_registry_permission_access(HKEY_CURRENT_USER, DESKTOP_APPS_PATH);

    let overall_access = if [device_access, app_access, desktop_app_access]
        .into_iter()
        .any(|access| access == PermissionAccess::Denied)
    {
        PermissionAccess::Denied
    } else if [device_access, app_access, desktop_app_access]
        .into_iter()
        .all(|access| access == PermissionAccess::Allowed)
    {
        PermissionAccess::Allowed
    } else {
        PermissionAccess::Unknown
    };

    WindowsMicrophonePermissionStatus {
        supported: true,
        overall_access,
        device_access,
        app_access,
        desktop_app_access,
    }
}

#[tauri::command]
#[specta::specta]
pub fn get_windows_microphone_permission_status() -> WindowsMicrophonePermissionStatus {
    #[cfg(target_os = "windows")]
    {
        get_windows_microphone_permission_status_impl()
    }

    #[cfg(not(target_os = "windows"))]
    {
        WindowsMicrophonePermissionStatus {
            supported: false,
            overall_access: PermissionAccess::Unknown,
            device_access: PermissionAccess::Unknown,
            app_access: PermissionAccess::Unknown,
            desktop_app_access: PermissionAccess::Unknown,
        }
    }
}

#[tauri::command]
#[specta::specta]
pub fn open_microphone_privacy_settings() -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        Command::new("cmd")
            .args(["/C", "start", "", "ms-settings:privacy-microphone"])
            .spawn()
            .map_err(|e| format!("Failed to open Windows microphone privacy settings: {}", e))?;
        return Ok(());
    }

    #[cfg(not(target_os = "windows"))]
    {
        Err("Opening microphone privacy settings is only supported on Windows".to_string())
    }
}

#[tauri::command]
#[specta::specta]
pub fn get_available_microphones() -> Result<Vec<AudioDevice>, String> {
    let devices =
        list_input_devices().map_err(|e| format!("Failed to list audio devices: {}", e))?;

    let mut result = vec![AudioDevice {
        index: "default".to_string(),
        name: "Default".to_string(),
        is_default: true,
    }];

    result.extend(devices.into_iter().map(|d| AudioDevice {
        index: d.index,
        name: d.name,
        is_default: false, // The explicit default is handled separately
    }));

    Ok(result)
}

#[tauri::command]
#[specta::specta]
pub fn set_selected_microphone(app: AppHandle, device_name: String) -> Result<(), String> {
    let mut settings = get_settings(&app);
    settings.selected_microphone = if device_name == "default" {
        None
    } else {
        Some(device_name)
    };
    write_settings(&app, settings);

    // Update the audio manager to use the new device
    let rm = app.state::<Arc<AudioRecordingManager>>();
    rm.update_selected_device()
        .map_err(|e| format!("Failed to update selected device: {}", e))?;

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn get_selected_microphone(app: AppHandle) -> Result<String, String> {
    let settings = get_settings(&app);
    Ok(settings
        .selected_microphone
        .unwrap_or_else(|| "default".to_string()))
}

#[tauri::command]
#[specta::specta]
pub fn get_available_output_devices() -> Result<Vec<AudioDevice>, String> {
    let devices =
        list_output_devices().map_err(|e| format!("Failed to list output devices: {}", e))?;

    let mut result = vec![AudioDevice {
        index: "default".to_string(),
        name: "Default".to_string(),
        is_default: true,
    }];

    result.extend(devices.into_iter().map(|d| AudioDevice {
        index: d.index,
        name: d.name,
        is_default: false, // The explicit default is handled separately
    }));

    Ok(result)
}

#[tauri::command]
#[specta::specta]
pub fn set_selected_output_device(app: AppHandle, device_name: String) -> Result<(), String> {
    let mut settings = get_settings(&app);
    settings.selected_output_device = if device_name == "default" {
        None
    } else {
        Some(device_name)
    };
    write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn get_selected_output_device(app: AppHandle) -> Result<String, String> {
    let settings = get_settings(&app);
    Ok(settings
        .selected_output_device
        .unwrap_or_else(|| "default".to_string()))
}

#[tauri::command]
#[specta::specta]
pub async fn play_test_sound(app: AppHandle, sound_type: String) {
    let sound = match sound_type.as_str() {
        "start" => audio_feedback::SoundType::Start,
        "stop" => audio_feedback::SoundType::Stop,
        _ => {
            warn!("Unknown sound type: {}", sound_type);
            return;
        }
    };
    audio_feedback::play_test_sound(&app, sound);
}

#[tauri::command]
#[specta::specta]
pub fn is_recording(app: AppHandle) -> bool {
    let audio_manager = app.state::<Arc<AudioRecordingManager>>();
    audio_manager.is_recording()
}

/// Per-frame rms stats returned when a wizard phase ends. The wizard uses
/// `mean_rms` for the ambient phase and `p75_rms` for the voice phases (see
/// `whisper_calibrate::phase_stats` for why the percentile).
#[derive(Serialize, Type)]
pub struct CalibrationPhaseStats {
    pub mean_rms: f32,
    pub p75_rms: f32,
}

/// Begin one RAW capture phase for the whisper calibration wizard (wizard
/// v2): the mic opens with the gate, booster, and VAD all bypassed and
/// records until `whisper_calibration_phase_stop`. The wizard runs three
/// phases (ambient, normal voice, whisper), each ended by the user pressing
/// Continue after its 5 second unlock. Momentary by design: nothing keeps
/// running outside an active phase.
#[tauri::command]
#[specta::specta]
pub fn whisper_calibration_phase_start(app: AppHandle) -> Result<(), String> {
    use crate::audio_toolkit::{AgcParams, CaptureTuning, VadPolicy, WhisperVetoes};

    let rm = app.state::<Arc<AudioRecordingManager>>().inner().clone();
    let raw = CaptureTuning {
        agc: AgcParams {
            enabled: false,
            ..AgcParams::NORMAL
        },
        vad_threshold: crate::defaults::VAD_BASE_THRESHOLD,
        loudness_ceiling: None,
        vetoes: WhisperVetoes::OFF,
    };
    rm.try_start_with_tuning("whisper_calibration", VadPolicy::Disabled, Some(raw))
}

/// End the current wizard phase and return its level stats. Doubles as the
/// cancel path: the wizard calls this and discards the result.
#[tauri::command]
#[specta::specta]
pub fn whisper_calibration_phase_stop(app: AppHandle) -> Result<CalibrationPhaseStats, String> {
    let rm = app.state::<Arc<AudioRecordingManager>>().inner().clone();
    let generation = rm.cancel_generation();
    let samples = rm
        .stop_recording("whisper_calibration", generation)
        .ok_or_else(|| "calibration capture was interrupted".to_string())?;
    if samples.is_empty() {
        return Err("no audio captured; check the microphone".to_string());
    }
    let (mean_rms, p75_rms) = crate::whisper_calibrate::phase_stats(&samples);
    Ok(CalibrationPhaseStats { mean_rms, p75_rms })
}

/// Turn the three measured wizard levels into a stored per-mic calibration
/// and return it for display. Overwrites any previous calibration.
#[tauri::command]
#[specta::specta]
pub fn whisper_calibration_finish(
    app: AppHandle,
    ambient: f32,
    normal: f32,
    whisper: f32,
) -> Result<crate::settings::WhisperCalibration, String> {
    let mut settings = get_settings(&app);
    let device = settings
        .selected_microphone
        .clone()
        .unwrap_or_else(|| "default".to_string());
    let cal = crate::whisper_calibrate::recommend(device, ambient, normal, whisper);
    log::info!(
        "whisper calibration saved for '{}': ambient {:.4} normal {:.4} whisper {:.4} -> ceilings {:.4}/{:.4}/{:.4} floor {:.4} ({})",
        cal.device_name,
        cal.ambient_rms,
        cal.normal_rms,
        cal.whisper_rms,
        cal.light_ceiling,
        cal.medium_ceiling,
        cal.high_ceiling,
        cal.energy_floor,
        cal.separation
    );
    settings.whisper_calibration = Some(cal.clone());
    write_settings(&app, settings);
    Ok(cal)
}

/// Remove the stored calibration: the gate returns to the strength defaults.
#[tauri::command]
#[specta::specta]
pub fn whisper_calibration_clear(app: AppHandle) -> Result<(), String> {
    let mut settings = get_settings(&app);
    settings.whisper_calibration = None;
    write_settings(&app, settings);
    Ok(())
}
