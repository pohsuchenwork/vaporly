//! Bundled llama.cpp engine manager ("Vaporly Engine").
//!
//! Owns the packaged `llama-server` payload end-to-end so LLM cleanup works
//! with zero external installs:
//!
//!   resources/llama/ (staged by scripts/ci/fetch-llama-server.sh, bundled via
//!   the resources glob) --install-on-first-run--> <app_data>/engine/<tag>/
//!   --spawn--> llama-server on 127.0.0.1:<ephemeral port> --health poll-->
//!   Ready --> ENGINE_PORT published --> llm_client requests resolve to it.
//!
//! Installing out of the sealed .app solves exec-bit stripping, Gatekeeper
//! quarantine, and hardened-runtime immutability in one move, and gives us a
//! writable home for the pidfile. The engine runs only while the bundled
//! provider is selected and post-processing is enabled; the paste path never
//! blocks on it (see the warming gate in `actions.rs`).

use anyhow::{anyhow, Result};
use hf_hub::api::tokio::{ApiBuilder, CancellationToken, Progress};
use hf_hub::{Cache, Repo, RepoType};
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

use crate::managers::llm_catalog::{self, LlmModelInfo};
use crate::settings::{get_settings, write_settings};

pub const VAPORLY_ENGINE_PROVIDER_ID: &str = "vaporly_engine";

/// The one cleanup endpoint Vaporly talks to. Not persisted anywhere: the
/// bundled llama-server is the only provider, so this is plain runtime data
/// (the v1 provider list in settings is gone).
#[derive(Debug, Clone)]
pub struct PostProcessProvider {
    pub id: String,
    pub base_url: String,
}

/// The baked-in bundled-engine provider. The port is a placeholder resolved at
/// request time from the live engine (see [`resolve_provider`]).
pub fn engine_provider() -> PostProcessProvider {
    PostProcessProvider {
        id: VAPORLY_ENGINE_PROVIDER_ID.to_string(),
        base_url: "http://127.0.0.1:0/v1".to_string(),
    }
}

/// Resolve the cleanup model id: an explicit `llm_model_id` wins, empty means
/// the hardware ladder's recommendation ("" again on machines below the 1.5B
/// tier, which disables the model pass).
pub fn cleanup_model_id(settings: &crate::settings::AppSettings) -> String {
    let explicit = settings.llm_model_id.trim();
    if explicit.is_empty() {
        crate::managers::hardware::recommended_model_id()
    } else {
        explicit.to_string()
    }
}

/// Port the bundled engine is serving on; 0 = not ready. A static atomic so
/// the request path (`llm_complete` holds only `&AppSettings`) can resolve the
/// runtime base_url without an AppHandle.
pub static ENGINE_PORT: AtomicU16 = AtomicU16::new(0);

/// Per-session bearer token the bundled llama-server requires on every request
/// (delivered via the LLAMA_API_KEY env var, not argv). Regenerated at each
/// spawn, never persisted: without it, any local process could use the engine
/// port. `llm_client` injects it for the vaporly_engine provider.
pub static ENGINE_TOKEN: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// The current engine session token ("" while the engine is down).
pub fn engine_token() -> String {
    ENGINE_TOKEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// 32 bytes of OS entropy as lowercase hex.
fn generate_engine_token() -> String {
    let mut buf = [0u8; 32];
    // Fail closed: a predictable (time/pid-derived) token would leave the local
    // engine effectively unauthenticated, which is worse than not starting. OS
    // entropy is available on every supported platform.
    getrandom::fill(&mut buf)
        .expect("OS entropy unavailable; refusing to start the engine with a guessable token");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Bumped on every deliberate stop/restart so stale monitor tasks know to
/// stand down instead of fighting the next generation's child.
static GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum EngineState {
    /// Bundled provider not selected / post-processing off, intentionally down.
    Disabled,
    /// No payload staged for this platform (dev build without the fetch script).
    NotInstalled,
    /// Copying the payload from resources into app-data.
    Installing,
    /// Selected cleanup model's GGUF shards are not all in the HF cache.
    ModelMissing,
    /// Process launched; /health not answering yet.
    Spawning,
    /// /health says the model is still loading (HTTP 503).
    LoadingModel,
    /// /health 200, ENGINE_PORT is live.
    Ready,
    /// Crashed; backoff timer running before the next respawn attempt.
    Restarting,
    /// Gave up after repeated crashes, Repair button resets.
    Crashed,
    /// Deliberately stopped (app exit, model swap, provider switch).
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct EngineStatus {
    pub state: EngineState,
    pub port: u16,
    pub model_id: String,
    pub engine_version: String,
    /// Human-oriented detail (last error, crash count, ...). Not localized, /// diagnostic text only.
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct LlmModelStatus {
    pub info: LlmModelInfo,
    pub is_downloaded: bool,
    pub is_downloading: bool,
    pub is_selected: bool,
}

#[derive(Debug, Clone, Serialize)]
struct LlmDownloadProgressEvent {
    model_id: String,
    downloaded: u64,
    total: u64,
    percentage: f64,
}

struct Inner {
    state: EngineState,
    detail: String,
    child: Option<tokio::process::Child>,
    /// Pid of the live server. The `Child` HANDLE moves into monitor_exit's
    /// wait() as soon as the engine is Ready, so stop paths cannot rely on
    /// `child`; they signal this pid instead (with an identity check).
    child_pid: Option<u32>,
    /// Restart timestamps within the crash window (for the give-up rule).
    crashes: Vec<Instant>,
}

pub struct LlmEngineManager {
    app: AppHandle,
    inner: Arc<Mutex<Inner>>,
    cancel_flags: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

const HEALTH_TIMEOUT: Duration = Duration::from_secs(180);
const CRASH_WINDOW: Duration = Duration::from_secs(600);
const MAX_CRASHES_IN_WINDOW: usize = 5;

impl LlmEngineManager {
    pub fn new(app: AppHandle) -> Self {
        Self {
            app,
            inner: Arc::new(Mutex::new(Inner {
                state: EngineState::Stopped,
                detail: String::new(),
                child: None,
                child_pid: None,
                crashes: Vec::new(),
            })),
            cancel_flags: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    // ---------- status ----------

    pub fn status(&self) -> EngineStatus {
        let inner = self.inner.lock().unwrap();
        EngineStatus {
            state: inner.state,
            port: ENGINE_PORT.load(Ordering::Acquire),
            model_id: self.selected_model_id(),
            engine_version: self.installed_version().unwrap_or_default(),
            detail: inner.detail.clone(),
        }
    }

    pub fn state(&self) -> EngineState {
        self.inner.lock().unwrap().state
    }

    fn set_state(&self, state: EngineState, detail: impl Into<String>) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.state = state;
            inner.detail = detail.into();
            info!("llm-engine: state -> {:?} ({})", state, inner.detail);
        }
        let _ = self.app.emit("llm-engine-status", &self.status());
    }

    /// Catalog id of the cleanup model the engine should serve.
    pub fn selected_model_id(&self) -> String {
        cleanup_model_id(&get_settings(&self.app))
    }

    // ---------- payload install ----------

    fn resources_payload_dir(&self) -> Option<PathBuf> {
        if let Ok(dir) = std::env::var("VAPORLY_LLAMA_DIR") {
            let p = PathBuf::from(dir);
            if p.join(server_binary_name()).exists() {
                return Some(p);
            }
        }
        let p = self
            .app
            .path()
            .resolve("resources/llama", tauri::path::BaseDirectory::Resource)
            .ok()?;
        p.join(server_binary_name()).exists().then_some(p)
    }

    fn engine_root(&self) -> Result<PathBuf> {
        Ok(crate::portable::app_data_dir(&self.app)?.join("engine"))
    }

    fn installed_dir(&self) -> Option<PathBuf> {
        let root = self.engine_root().ok()?;
        let tag = std::fs::read_to_string(root.join("current")).ok()?;
        let dir = root.join(tag.trim());
        dir.join(server_binary_name()).exists().then_some(dir)
    }

    fn installed_version(&self) -> Option<String> {
        let dir = self.installed_dir()?;
        std::fs::read_to_string(dir.join("engine-version.txt"))
            .ok()
            .map(|s| s.trim().to_string())
    }

    /// Ensure the payload from resources is installed under app-data (copying
    /// only when the staged version differs). Returns the runnable dir.
    fn ensure_payload_installed(&self) -> Result<Option<PathBuf>> {
        let Some(src) = self.resources_payload_dir() else {
            return Ok(None); // NotInstalled
        };
        let staged_tag = std::fs::read_to_string(src.join("engine-version.txt"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unknown".into());

        if let Some(dir) = self.installed_dir() {
            if self.installed_version().as_deref() == Some(staged_tag.as_str()) {
                return Ok(Some(dir));
            }
        }

        self.set_state(EngineState::Installing, format!("installing {staged_tag}"));
        let root = self.engine_root()?;
        let dest = root.join(&staged_tag);
        let _ = std::fs::remove_dir_all(&dest);
        copy_dir_dereferencing(&src, &dest)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let server = dest.join(server_binary_name());
            let mut perms = std::fs::metadata(&server)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&server, perms)?;
        }
        #[cfg(target_os = "macos")]
        {
            // Downloaded-app quarantine propagates to copied files; strip it so
            // the child exec isn't blocked by Gatekeeper.
            let _ = std::process::Command::new("xattr")
                .args(["-dr", "com.apple.quarantine"])
                .arg(&dest)
                .output();
        }

        std::fs::write(root.join("current"), &staged_tag)?;
        // Prune older versions.
        if let Ok(entries) = std::fs::read_dir(&root) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if e.path().is_dir() && name != staged_tag {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        info!("llm-engine: installed payload {staged_tag} -> {dest:?}");
        Ok(Some(dest))
    }

    // ---------- model files ----------

    /// All shards cached? Returns the path of shard 0 (what `-m` points at).
    /// Falls back to a locally-installed Ollama blob carrying the same weights
    /// (Ollama's qwen2.5 tags ARE the official Q4_K_M GGUFs) so users migrating
    /// from Ollama get a working engine with zero re-download.
    pub fn model_path(&self, model: &LlmModelInfo) -> Option<PathBuf> {
        let cache = Cache::from_env().repo(Repo::with_revision(
            model.repo.clone(),
            RepoType::Model,
            "main".to_string(),
        ));
        let mut first: Option<PathBuf> = None;
        for (i, f) in model.files.iter().enumerate() {
            match cache.get(f) {
                Some(p) => {
                    if i == 0 {
                        first = Some(p);
                    }
                }
                None => {
                    first = None;
                    break;
                }
            }
        }
        if first.is_some() {
            return first;
        }
        ollama_blob_for(&model.id)
    }

    pub fn model_infos(&self) -> Vec<LlmModelStatus> {
        let selected = self.selected_model_id();
        llm_catalog::catalog()
            .into_iter()
            .map(|m| {
                let is_downloaded = self.model_path(&m).is_some();
                let is_downloading = self.cancel_flags.lock().unwrap().contains_key(&m.id);
                let is_selected = m.id == selected;
                LlmModelStatus {
                    info: m,
                    is_downloaded,
                    is_downloading,
                    is_selected,
                }
            })
            .collect()
    }

    /// Download every shard of a catalog model into the shared HF cache with
    /// aggregated progress events. Mirrors the STT path (resume via .sync.part,
    /// ~10 events/s, prompt cancellation).
    pub async fn download_model(&self, model_id: &str) -> Result<()> {
        let model =
            llm_catalog::find(model_id).ok_or_else(|| anyhow!("unknown LLM model {model_id}"))?;

        if self.model_path(&model).is_some() {
            let _ = self.app.emit("llm-model-download-complete", &model.id);
            return Ok(());
        }

        let token = CancellationToken::new();
        {
            let mut flags = self.cancel_flags.lock().unwrap();
            if flags.contains_key(model_id) {
                return Ok(()); // already downloading
            }
            flags.insert(model_id.to_string(), token.clone());
        }

        let result = self.download_model_inner(&model, token).await;
        self.cancel_flags.lock().unwrap().remove(model_id);

        match &result {
            Ok(()) => {
                let _ = self.app.emit("llm-model-download-complete", &model.id);
            }
            Err(e) if e.to_string().contains("cancelled") => {
                let _ = self.app.emit("llm-model-download-cancelled", &model.id);
            }
            Err(e) => {
                let _ = self.app.emit(
                    "llm-model-download-failed",
                    serde_json::json!({ "model_id": model.id, "error": e.to_string() }),
                );
            }
        }
        result
    }

    async fn download_model_inner(
        &self,
        model: &LlmModelInfo,
        token: CancellationToken,
    ) -> Result<()> {
        let api = ApiBuilder::from_env()
            .with_progress(false)
            .with_max_files(8)
            .build()
            .map_err(|e| anyhow!("HF api init: {e}"))?;
        let repo = api.repo(Repo::with_revision(
            model.repo.clone(),
            RepoType::Model,
            "main".to_string(),
        ));

        // Aggregate progress across shards: base = bytes of completed shards.
        let mut completed_bytes: u64 = 0;
        for f in &model.files {
            let progress = AggregatedProgress {
                app: self.app.clone(),
                model_id: model.id.clone(),
                grand_total: model.total_bytes,
                base: completed_bytes,
                shard_total: Arc::new(Mutex::new(0)),
                downloaded: Arc::new(Mutex::new(0)),
                last_emit: Arc::new(Mutex::new(Instant::now())),
            };
            match repo
                .download_with_progress_cancellable(f, progress, token.clone())
                .await
            {
                Ok(_) => {}
                Err(hf_hub::api::tokio::ApiError::Cancelled) => {
                    return Err(anyhow!("cancelled"));
                }
                Err(e) => return Err(anyhow!("download {f}: {e}")),
            }
            // Shard done, use its real cached size for the aggregate base.
            let cache = Cache::from_env().repo(Repo::with_revision(
                model.repo.clone(),
                RepoType::Model,
                "main".to_string(),
            ));
            if let Some(p) = cache.get(f) {
                completed_bytes += std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            }
        }
        Ok(())
    }

    pub fn cancel_model_download(&self, model_id: &str) {
        if let Some(token) = self.cancel_flags.lock().unwrap().get(model_id) {
            token.cancel();
        }
    }

    pub fn delete_model(&self, model_id: &str) -> Result<()> {
        let model =
            llm_catalog::find(model_id).ok_or_else(|| anyhow!("unknown LLM model {model_id}"))?;
        let cache = Cache::from_env().repo(Repo::with_revision(
            model.repo.clone(),
            RepoType::Model,
            "main".to_string(),
        ));
        for f in &model.files {
            if let Some(p) = cache.get(f) {
                let _ = std::fs::remove_file(p);
            }
        }
        Ok(())
    }

    // ---------- lifecycle ----------

    /// Should the engine be running at all, per current settings? True when
    /// any cleanup stage is set to the Model engine (see
    /// [`AppSettings::model_pass_needed`](crate::settings::AppSettings::model_pass_needed)).
    fn wanted(&self) -> bool {
        get_settings(&self.app).model_pass_needed()
    }

    /// Idempotent: bring the engine to Ready if settings want it, walking
    /// through install/model checks. Safe to call from anywhere.
    pub fn ensure_running(self: &Arc<Self>) {
        let this = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            this.ensure_running_inner().await;
        });
    }

    /// Boxed so the spawn_and_monitor -> monitor_exit -> ensure_running_inner
    /// recursion has a finite future type (async recursion needs erasure).
    fn ensure_running_inner(self: &Arc<Self>) -> futures_util::future::BoxFuture<'static, ()> {
        let this = Arc::clone(self);
        Box::pin(async move { this.ensure_running_boxed_body().await })
    }

    async fn ensure_running_boxed_body(self: &Arc<Self>) {
        if !self.wanted() {
            self.stop_internal(EngineState::Disabled, "provider not selected");
            return;
        }
        {
            let inner = self.inner.lock().unwrap();
            if matches!(
                inner.state,
                EngineState::Ready
                    | EngineState::Spawning
                    | EngineState::LoadingModel
                    | EngineState::Installing
            ) {
                return; // already up or on the way
            }
        }

        let payload = match self.ensure_payload_installed() {
            Ok(Some(dir)) => dir,
            Ok(None) => {
                self.set_state(
                    EngineState::NotInstalled,
                    "no engine payload for this platform",
                );
                return;
            }
            Err(e) => {
                self.set_state(EngineState::Crashed, format!("install failed: {e}"));
                return;
            }
        };

        let model_id = self.selected_model_id();
        if model_id.is_empty() {
            self.set_state(EngineState::ModelMissing, "no cleanup model selected");
            return;
        }
        let Some(model) = llm_catalog::find(&model_id) else {
            self.set_state(
                EngineState::ModelMissing,
                format!("unknown model {model_id}"),
            );
            return;
        };
        let Some(gguf) = self.model_path(&model) else {
            self.set_state(
                EngineState::ModelMissing,
                format!("{} not downloaded", model.display_name),
            );
            return;
        };

        self.spawn_and_monitor(payload, gguf).await;
    }

    async fn spawn_and_monitor(self: &Arc<Self>, payload: PathBuf, gguf: PathBuf) {
        let generation = GENERATION.fetch_add(1, Ordering::AcqRel) + 1;

        // Ephemeral port: bind :0, take it, release. A lost race just means one
        // failed spawn and a retry with a fresh port.
        let port = match std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
        {
            Ok(p) => p,
            Err(e) => {
                self.set_state(EngineState::Crashed, format!("port pick failed: {e}"));
                return;
            }
        };
        let port = std::env::var("VAPORLY_LLAMA_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(port);

        let ngl = engine_gpu_layers(&self.app);
        // Thread cap: the engine must never starve the live STT ticks (the
        // documented freeze mechanism was llama-server on ALL cores while a
        // pseudo-stream decode ran). Cores minus two, env-overridable.
        let threads = std::env::var("VAPORLY_LLAMA_THREADS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or_else(crate::managers::hardware::engine_cpu_threads);
        let server = payload.join(server_binary_name());
        info!(
            "llm-engine: spawning {server:?} -m {gguf:?} port={port} ngl={ngl} threads={threads} prio=low (gen {generation})"
        );
        self.set_state(EngineState::Spawning, format!("port {port}"));

        // Fresh auth token per spawn: the server only answers requests
        // carrying it (llama-server exempts /health, which the poll below
        // relies on).
        let token = generate_engine_token();
        *ENGINE_TOKEN.lock().unwrap_or_else(|e| e.into_inner()) = token.clone();

        let mut cmd = tokio::process::Command::new(&server);
        cmd.arg("-m")
            .arg(&gguf)
            .args(["--host", "127.0.0.1"])
            .args(["--port", &port.to_string()])
            .args(["-c", "8192"])
            .arg("--no-webui")
            // Deliver the auth token via the environment, not argv: a token on
            // the command line is readable by any same-user process via `ps` /
            // /proc/<pid>/cmdline. llama-server reads LLAMA_API_KEY, equivalent
            // to --api-key.
            .env("LLAMA_API_KEY", &token)
            .args(["-ngl", &ngl.to_string()])
            .args(["-t", &threads.to_string()])
            // llama.cpp-native low priority for the compute threads; covers
            // Windows too, so no creation_flags are needed there.
            .args(["--prio", "-1"])
            // Let a later cleanup chunk reuse cached KV even when an earlier
            // self-correction rewrote already-committed text. Cheap, and the
            // constant instruction prefix is already reused via default caching.
            .args(["--cache-reuse", "256"])
            .current_dir(&payload)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        // Flash attention: the bundled b9912 already negotiates '--flash-attn
        // auto' (enables it on Metal, skips it on CPU), so we do not force it.
        // Expose an explicit override for GPU builds only; never touch this
        // CPU path (ngl == 0 here), where flash attention is marginal-to-
        // negative. Flag form confirmed against the binary: --flash-attn on|off|auto.
        if ngl > 0 {
            if let Some(mode) = std::env::var("VAPORLY_LLAMA_FLASH_ATTN")
                .ok()
                .filter(|v| !v.is_empty())
            {
                cmd.args(["--flash-attn", &mode]);
            }
        }

        #[cfg(target_os = "linux")]
        unsafe {
            // Reap the child even if this process dies by SIGKILL, and drop
            // its scheduling priority so STT decodes preempt LLM compute.
            cmd.pre_exec(|| {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
                libc::setpriority(libc::PRIO_PROCESS, 0, 10);
                Ok(())
            });
        }
        #[cfg(target_os = "macos")]
        unsafe {
            // Whole-process nice as the guaranteed floor under --prio -1.
            cmd.pre_exec(|| {
                libc::setpriority(libc::PRIO_PROCESS, 0, 10);
                Ok(())
            });
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                self.set_state(EngineState::Crashed, format!("spawn failed: {e}"));
                return;
            }
        };

        // Forward server output to our log at debug level.
        if let Some(out) = child.stdout.take() {
            tauri::async_runtime::spawn(pipe_to_log(out, "llama_server"));
        }
        if let Some(err) = child.stderr.take() {
            tauri::async_runtime::spawn(pipe_to_log(err, "llama_server"));
        }

        let pid = child.id();
        self.write_pidfile(pid, port);
        {
            let mut inner = self.inner.lock().unwrap();
            inner.child = Some(child);
            inner.child_pid = pid;
        }

        // Health poll until Ready (or give up).
        let health_ok = self.poll_health(port, generation).await;
        if !health_ok {
            // poll_health already set a state; make sure the child is gone.
            self.kill_child().await;
            return;
        }

        ENGINE_PORT.store(port, Ordering::Release);
        self.set_state(EngineState::Ready, String::new());

        // Monitor: wait for exit; restart with backoff unless deliberate.
        let this = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            this.monitor_exit(generation).await;
        });
    }

    async fn poll_health(&self, port: u16, generation: u64) -> bool {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_millis(500))
            .timeout(Duration::from_secs(2))
            .build()
            .expect("health client");
        let url = format!("http://127.0.0.1:{port}/health");
        let start = Instant::now();
        let mut announced_loading = false;
        while start.elapsed() < HEALTH_TIMEOUT {
            if GENERATION.load(Ordering::Acquire) != generation {
                return false; // superseded
            }
            // Child died during startup?
            {
                let mut inner = self.inner.lock().unwrap();
                if let Some(child) = inner.child.as_mut() {
                    if let Ok(Some(status)) = child.try_wait() {
                        drop(inner);
                        self.set_state(
                            EngineState::Crashed,
                            format!("server exited during startup: {status}"),
                        );
                        return false;
                    }
                }
            }
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => return true,
                Ok(resp) if resp.status().as_u16() == 503 => {
                    if !announced_loading {
                        announced_loading = true;
                        self.set_state(EngineState::LoadingModel, String::new());
                    }
                }
                _ => {}
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        self.set_state(EngineState::Crashed, "health check timed out".to_string());
        false
    }

    async fn monitor_exit(self: &Arc<Self>, generation: u64) {
        // Take the child handle to wait on it (holding no lock across await).
        let child = {
            let mut inner = self.inner.lock().unwrap();
            inner.child.take()
        };
        let Some(mut child) = child else { return };
        let status = child.wait().await;

        if GENERATION.load(Ordering::Acquire) != generation {
            return; // deliberate stop/restart superseded us
        }

        // The pid is dead; drop it so no later stop signals a recycled pid.
        self.inner.lock().unwrap().child_pid = None;
        ENGINE_PORT.store(0, Ordering::Release);
        ENGINE_TOKEN
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.remove_pidfile();
        warn!("llm-engine: server exited unexpectedly: {status:?}");

        // Crash bookkeeping + give-up rule.
        let crash_count = {
            let mut inner = self.inner.lock().unwrap();
            let now = Instant::now();
            inner
                .crashes
                .retain(|t| now.duration_since(*t) < CRASH_WINDOW);
            inner.crashes.push(now);
            inner.crashes.len()
        };
        if crash_count > MAX_CRASHES_IN_WINDOW {
            self.set_state(
                EngineState::Crashed,
                format!("{crash_count} crashes in 10 min, gave up"),
            );
            return;
        }
        let backoff = Duration::from_secs(1 << (crash_count.saturating_sub(1)).min(4));
        self.set_state(
            EngineState::Restarting,
            format!("attempt {crash_count}/{MAX_CRASHES_IN_WINDOW} in {backoff:?}"),
        );
        tokio::time::sleep(backoff).await;
        if GENERATION.load(Ordering::Acquire) != generation {
            return;
        }
        self.ensure_running_inner().await;
    }

    async fn kill_child(&self) {
        let (child, pid) = {
            let mut inner = self.inner.lock().unwrap();
            (inner.child.take(), inner.child_pid.take())
        };
        if let Some(mut child) = child {
            let _ = child.kill().await;
        }
        if let Some(pid) = pid {
            Self::kill_server_pid(pid);
        }
        self.remove_pidfile();
    }

    fn stop_internal(&self, state: EngineState, detail: &str) {
        GENERATION.fetch_add(1, Ordering::AcqRel);
        ENGINE_PORT.store(0, Ordering::Release);
        let (child, pid) = {
            let mut inner = self.inner.lock().unwrap();
            (inner.child.take(), inner.child_pid.take())
        };
        if let Some(mut child) = child {
            let _ = child.start_kill();
        }
        // The handle almost never lives in `inner` (monitor_exit owns it while
        // waiting), so the pid is the authoritative kill path.
        if let Some(pid) = pid {
            Self::kill_server_pid(pid);
        }
        self.remove_pidfile();
        self.set_state(state, detail.to_string());
    }

    /// Deliberate stop (app exit, provider switch, model swap).
    pub fn stop(&self) {
        self.stop_internal(EngineState::Stopped, "");
    }

    /// Repair: reset crash bookkeeping and try again from scratch.
    pub fn repair(self: &Arc<Self>) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.crashes.clear();
        }
        self.stop_internal(EngineState::Stopped, "repairing");
        self.ensure_running();
    }

    /// Swap the cleanup model: persist, restart the engine on it.
    pub fn set_model(self: &Arc<Self>, model_id: &str) -> Result<()> {
        llm_catalog::find(model_id).ok_or_else(|| anyhow!("unknown LLM model {model_id}"))?;
        let mut settings = get_settings(&self.app);
        settings.llm_model_id = model_id.to_string();
        write_settings(&self.app, settings);
        self.stop_internal(EngineState::Stopped, "model swap");
        self.ensure_running();
        Ok(())
    }

    // ---------- pidfile (SIGKILL orphan safety) ----------

    /// Kill the server by PID after verifying the process is still ours
    /// (guards against pid reuse). SIGTERM for a clean log line, SIGKILL
    /// immediately after: llama-server holds no persistent state, so a hard
    /// kill is always safe and never waits on a busy inference loop.
    fn kill_server_pid(pid: u32) {
        #[cfg(unix)]
        {
            let ours = std::process::Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "comm="])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).contains("llama-server"))
                .unwrap_or(false);
            if !ours {
                return;
            }
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        {
            let ours = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).contains("llama-server"))
                .unwrap_or(false);
            if !ours {
                return;
            }
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/F"])
                .output();
        }
    }

    fn pidfile(&self) -> Option<PathBuf> {
        self.engine_root().ok().map(|r| r.join("llm-engine.pid"))
    }

    fn write_pidfile(&self, pid: Option<u32>, port: u16) {
        let (Some(pf), Some(pid)) = (self.pidfile(), pid) else {
            return;
        };
        let _ = std::fs::create_dir_all(pf.parent().unwrap());
        let _ = std::fs::write(pf, format!("{pid} {port}"));
    }

    fn remove_pidfile(&self) {
        if let Some(pf) = self.pidfile() {
            let _ = std::fs::remove_file(pf);
        }
    }

    /// Reap a llama-server orphaned by a hard kill of the previous app run.
    /// Only kills a live pid whose process name actually looks like our server.
    pub fn reap_orphan(&self) {
        let Some(pf) = self.pidfile() else { return };
        let Ok(content) = std::fs::read_to_string(&pf) else {
            return;
        };
        let mut parts = content.split_whitespace();
        let Some(pid) = parts.next().and_then(|p| p.parse::<i32>().ok()) else {
            let _ = std::fs::remove_file(&pf);
            return;
        };
        #[cfg(unix)]
        {
            let name_matches = std::process::Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "comm="])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).contains("llama-server"))
                .unwrap_or(false);
            if name_matches {
                warn!("llm-engine: reaping orphaned llama-server pid {pid}");
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
        #[cfg(windows)]
        {
            let name_matches = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).contains("llama-server"))
                .unwrap_or(false);
            if name_matches {
                warn!("llm-engine: reaping orphaned llama-server pid {pid}");
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/F"])
                    .output();
            }
        }
        let _ = std::fs::remove_file(&pf);
    }

    /// Belt-and-braces orphan sweep: kill ANY llama-server whose command line
    /// references our engine payload directory. The pidfile above only covers
    /// the most recent child; a crash-respawn cycle ended by SIGKILL can leave
    /// an older sibling running (observed in the wild holding gigabytes of
    /// model RAM). Runs at startup, before this session spawns anything, so
    /// every match is an orphan by construction. The path filter means user
    /// llama-servers elsewhere are never touched.
    pub fn reap_strays(&self) {
        let Ok(root) = self.engine_root() else { return };
        let needle = root.to_string_lossy().to_string();
        #[cfg(unix)]
        {
            let Ok(out) = std::process::Command::new("pgrep")
                .args(["-f", &needle])
                .output()
            else {
                return;
            };
            let own = std::process::id();
            for pid in String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .filter_map(|p| p.parse::<i32>().ok())
            {
                if pid as u32 == own {
                    continue;
                }
                warn!("llm-engine: reaping stray llama-server pid {pid} under {needle}");
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
        #[cfg(windows)]
        {
            // PowerShell CIM query: kill processes whose executable lives in
            // our engine dir. Single quotes doubled for PS literal strings.
            let pattern = format!("{}*", needle).replace('\'', "''");
            let script = format!(
                "Get-CimInstance Win32_Process | Where-Object {{ $_.ExecutablePath -like '{pattern}' }} | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force }}"
            );
            let _ = std::process::Command::new("powershell")
                .args(["-NoProfile", "-Command", &script])
                .output();
        }
    }
}

/// Aggregated multi-shard progress: emits `llm-model-download-progress` with
/// bytes relative to the model's grand total. hf-hub clones the reporter, so
/// shared counters live behind Arcs.
#[derive(Clone)]
struct AggregatedProgress {
    app: AppHandle,
    model_id: String,
    grand_total: u64,
    base: u64,
    shard_total: Arc<Mutex<u64>>,
    downloaded: Arc<Mutex<u64>>,
    last_emit: Arc<Mutex<Instant>>,
}

impl AggregatedProgress {
    fn emit(&self, force: bool) {
        let downloaded = *self.downloaded.lock().unwrap();
        {
            let mut last = self.last_emit.lock().unwrap();
            let now = Instant::now();
            if !force && now.duration_since(*last) < Duration::from_millis(100) {
                return;
            }
            *last = now;
        }
        let overall = self.base + downloaded;
        let percentage = if self.grand_total > 0 {
            (overall as f64 / self.grand_total as f64) * 100.0
        } else {
            0.0
        };
        let _ = self.app.emit(
            "llm-model-download-progress",
            &LlmDownloadProgressEvent {
                model_id: self.model_id.clone(),
                downloaded: overall,
                total: self.grand_total,
                percentage: percentage.min(100.0),
            },
        );
    }
}

impl Progress for AggregatedProgress {
    async fn init(&mut self, size: usize, _filename: &str) {
        *self.shard_total.lock().unwrap() = size as u64;
        *self.downloaded.lock().unwrap() = 0;
        self.emit(true);
    }
    async fn update(&mut self, size: usize) {
        {
            let mut d = self.downloaded.lock().unwrap();
            *d = d.saturating_add(size as u64);
        }
        self.emit(false);
    }
    async fn finish(&mut self) {
        let total = *self.shard_total.lock().unwrap();
        *self.downloaded.lock().unwrap() = total;
        self.emit(true);
    }
}

/// Resolve an equivalent GGUF from a local Ollama install, manifest-verified.
/// Only exact-weight matches: Ollama's `qwen2.5:7b`/`1.5b` tags ship the same
/// official Q4_K_M quants our catalog points at.
fn ollama_blob_for(catalog_id: &str) -> Option<PathBuf> {
    let tag_prefix = match catalog_id {
        "qwen2.5-7b-instruct-q4_k_m" => "7b",
        "qwen2.5-1.5b-instruct-q4_k_m" => "1.5b",
        _ => return None,
    };
    let home = std::env::var("HOME").ok()?;
    let base = PathBuf::from(home).join(".ollama/models");
    let manifest_dir = base.join("manifests/registry.ollama.ai/library/qwen2.5");
    for entry in std::fs::read_dir(&manifest_dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // "7b" must not match "1.5b"'s prefix logic, exact tag or tag-variant.
        if name != tag_prefix && !name.starts_with(&format!("{tag_prefix}-")) {
            continue;
        }
        let Ok(manifest) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&manifest) else {
            continue;
        };
        let Some(layers) = json.get("layers").and_then(|l| l.as_array()) else {
            continue;
        };
        for layer in layers {
            let media = layer
                .get("mediaType")
                .and_then(|m| m.as_str())
                .unwrap_or("");
            if !media.ends_with("image.model") {
                continue;
            }
            let Some(digest) = layer.get("digest").and_then(|d| d.as_str()) else {
                continue;
            };
            let expected_size = layer.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
            let blob = base.join("blobs").join(digest.replace(':', "-"));
            if let Ok(meta) = std::fs::metadata(&blob) {
                if expected_size == 0 || meta.len() == expected_size {
                    info!(
                        "llm-engine: using local Ollama weights for {catalog_id}: {blob:?} ({} bytes)",
                        meta.len()
                    );
                    return Some(blob);
                }
            }
        }
    }
    None
}

/// Clone-with-live-port: the bundled provider carries a placeholder base_url
/// (port 0); the real port exists only at runtime in [`ENGINE_PORT`]. Pure,
/// no waiting, no events.
pub fn resolve_provider(provider: &PostProcessProvider) -> PostProcessProvider {
    let mut p = provider.clone();
    if p.id == VAPORLY_ENGINE_PROVIDER_ID {
        let port = ENGINE_PORT.load(Ordering::Acquire);
        p.base_url = format!("http://127.0.0.1:{port}/v1");
    }
    p
}

/// Bounded wait for the engine to publish its port (the "dictated right after
/// login while the model is still loading" case). Returns true when ready.
pub async fn wait_ready_bounded(timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if ENGINE_PORT.load(Ordering::Acquire) != 0 {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn server_binary_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/// `-ngl` for the spawn. The accelerator is a fixed default in v2 (Auto =
/// hardware probe: Metal on real Apple Silicon, CPU in VMs and elsewhere).
fn engine_gpu_layers(_app: &AppHandle) -> u32 {
    use crate::defaults::LlmAcceleratorSetting;
    match crate::defaults::LLM_ENGINE_ACCELERATOR {
        LlmAcceleratorSetting::Cpu => 0,
        LlmAcceleratorSetting::Gpu => 99,
        LlmAcceleratorSetting::Auto => crate::managers::hardware::auto_gpu_layers(),
    }
}

/// Copy a payload directory, following symlinks (each soname alias becomes a
/// real file, the app-data copy must be runnable without link support).
fn copy_dir_dereferencing(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let meta = std::fs::metadata(&src)?; // follows symlinks
        if meta.is_dir() {
            copy_dir_dereferencing(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?; // follows symlinks
        }
    }
    Ok(())
}

async fn pipe_to_log<R: tokio::io::AsyncRead + Unpin>(reader: R, target: &'static str) {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        debug!(target: "llama_server", "[{target}] {line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_binary_name_matches_platform() {
        if cfg!(windows) {
            assert_eq!(server_binary_name(), "llama-server.exe");
        } else {
            assert_eq!(server_binary_name(), "llama-server");
        }
    }

    #[test]
    fn pidfile_parse_shape() {
        // "pid port" split logic used by reap_orphan
        let content = "12345 18099";
        let mut parts = content.split_whitespace();
        assert_eq!(
            parts.next().and_then(|p| p.parse::<i32>().ok()),
            Some(12345)
        );
    }

    #[test]
    fn copy_dir_dereferences_symlinks() {
        let tmp = std::env::temp_dir().join(format!("vaporly_copy_test_{}", std::process::id()));
        let src = tmp.join("src");
        let dst = tmp.join("dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("real.dylib"), b"content").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(src.join("real.dylib"), src.join("alias.dylib")).unwrap();
        copy_dir_dereferencing(&src, &dst).unwrap();
        assert!(dst.join("real.dylib").is_file());
        #[cfg(unix)]
        {
            let alias = dst.join("alias.dylib");
            assert!(alias.is_file());
            assert!(!std::fs::symlink_metadata(&alias)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(std::fs::read(&alias).unwrap(), b"content");
        }
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
