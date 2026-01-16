mod config;
mod commands;
mod error;
mod porcupine_sys;
mod porcupine;
mod audio;
mod vad;
mod intent;
mod ml;
mod ui;
mod tts;
mod search;
mod net;
mod executor;
mod llm;
mod decision;
mod manager;

use error::{BtwError, Result};
use std::{fs, time::Instant};
use xdg::BaseDirectories;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;
use std::path::PathBuf;

fn normalize_short(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch.is_whitespace() {
            out.push(ch.to_ascii_lowercase());
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

// NOTE: web-search gating is handled by the strict
// LLM knowledge-check → Tavily → LLM workflow in `search`.

fn handle_transcript(
    text: &str,
    cfg: &config::Config,
    exec: &mut executor::Executor,
    intent_router: &intent::IntentRouter,
    llm_client: &Arc<dyn llm::LlmClient>,
) {
    let norm = normalize_short(text);

    // 1) Confirmation/cancellation ONLY if a command is pending.
    if exec.has_pending() {
        let proceed = matches!(norm.as_str(), "yes" | "confirm" | "do it");
        let cancel = matches!(norm.as_str(), "no" | "cancel" | "stop");

        if !proceed && !cancel {
            // Must ignore everything else while pending.
            return;
        }

        let status = exec.handle_confirmation_text(&norm);
        eprintln!("exec: confirmation text -> {:?}", status);
        return;
    }

    // 2) Command detection (ALLOW-LIST ONLY).
    // NOTE: IntentRouter currently includes LLM fallback; we must not guess commands.
    // We enforce allow-list + deterministic score gate, and treat anything else as a question.
    let routed = intent_router.route(text);
    let det_score = routed.deterministic_score.unwrap_or(0.0);
    let is_valid_allowlisted = routed.command_id.is_some();
    let passed_threshold = det_score >= cfg.intent.deterministic_threshold;

    if is_valid_allowlisted && passed_threshold {
        if routed.dangerous {
            let status = exec.handle_intent(&intent::IntentResult {
                requires_confirmation: true,
                ..routed
            });
            eprintln!("exec: dangerous command -> {:?}", status);
            return;
        }

        // Non-dangerous executes immediately.
        let status = exec.handle_intent(&routed);
        eprintln!("exec: command -> {:?}", status);
        return;
    }

    // 3) Non-command -> Question routing.
    // If below threshold, treat as question (never command).
    let question = text.trim();
    if question.is_empty() {
        return;
    }

    // Strict workflow: ask LLM first with a knowledge-check. Only if it explicitly
    // returns the sentinel string do we call Tavily and then re-ask.
    // No UI notifications are shown until the final answer is ready.
    if cfg.search.enabled {
        eprintln!("assistant: question; strict LLM→Tavily gating");
        search::search_and_summarize_async(
            question.to_string(),
            cfg.search.clone(),
            cfg.ui.osd,
            cfg.ui.osd_timeout_ms,
            cfg.speech_output.clone(),
            llm_client.clone(),
        );
        return;
    }

    // If search is disabled, fall back to direct LLM answer.
    eprintln!("assistant: question; asking LLM (search disabled)");
    let ans = llm_client.answer_short(question).unwrap_or_else(|e| {
        eprintln!("assistant: LLM answer error: {}", e);
        "I don’t know.".to_string()
    });
    ui::notify_text(cfg.ui.osd, cfg.ui.osd_timeout_ms, "Btw", &ans);
    if cfg.speech_output.enabled {
        tts::speak_async(ans, cfg.speech_output.clone());
    }
}
fn main() {
    if let Err(e) = run() {
        eprintln!("btwd startup error: {}", e);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let xdg = BaseDirectories::with_prefix("btw")
        .map_err(|e| BtwError::XdgError { message: e.to_string() })?;

    let config_path = xdg.find_config_file("config.toml")
        .ok_or_else(|| expected_missing(&xdg, "config.toml", "config"))?;
    let commands_path = xdg.find_config_file("commands.json")
        .ok_or_else(|| expected_missing(&xdg, "commands.json", "commands"))?;
    let env_path = xdg.find_config_file(".env")
        .ok_or_else(|| expected_missing(&xdg, ".env", "env"))?;

    dotenvy::from_path(&env_path)
        .map_err(|e| BtwError::EnvLoadError { path: env_path.clone(), source: e })?;

    let cfg_str = fs::read_to_string(&config_path)
        .map_err(|e| BtwError::ReadError { path: config_path.clone(), source: e })?;
    let cfg = config::Config::from_toml_str(&cfg_str)
        .map_err(|msg| BtwError::ParseError { path: config_path.clone(), kind: "toml", message: msg })?;

    let commands_str = fs::read_to_string(&commands_path)
        .map_err(|e| BtwError::ReadError { path: commands_path.clone(), source: e })?;
    let _commands = commands::parse_commands_json(&commands_str)
        .map_err(|msg| BtwError::ParseError { path: commands_path.clone(), kind: "json", message: msg })?;

    eprintln!("btwd started successfully");
    eprintln!("Loaded config from {}", config_path.display());
    eprintln!("Loaded commands from {}", commands_path.display());
    eprintln!("Environment loaded from {}", env_path.display());

    // ---- Porcupine init (CORRECT PLACE)
    let porcupine = porcupine::Porcupine::new(
        cfg.wake_word.model_path.as_ref(),
        &cfg.wake_word.device,
        cfg.wake_word.ppn_path.as_ref(),
        cfg.wake_word.sensitivity,
    )?;

    eprintln!("Porcupine version: {}", porcupine::Porcupine::version());
    eprintln!("Porcupine sample rate: {}", porcupine.sample_rate());
    eprintln!("Porcupine frame length: {}", porcupine.frame_length());
    eprintln!("Porcupine device: {}", porcupine.device());

    // ---- Audio thread
    let (_audio_handle, rx): (std::thread::JoinHandle<()>, Receiver<Vec<i16>>) =
        audio::start_listening(&porcupine)?;

    eprintln!("Listening for wake word...");

    let mut worker = ml::MLWorker::new()?;
    let mut vad = vad::Vad::new(cfg.speech.vad_mode)?;

    let sample_rate = porcupine.sample_rate();
    let frame_length = porcupine.frame_length();
    let frame_ms = (frame_length as f64) * 1000.0 / sample_rate as f64;

    let llm_client: Arc<dyn llm::LlmClient> = match cfg.llm.provider.as_str() {
        "groq" => {
            std::env::var("GROQ_API_KEY").map_err(|e| {
                BtwError::ParseError {
                    path: config_path.clone(),
                    kind: "env",
                    message: format!("missing GROQ_API_KEY: {}", e),
                }
            })?;
            Arc::new(llm::GroqClient::new(std::env::var("GROQ_API_KEY").unwrap()))
        }
        "mistral" => {
            std::env::var("MISTRAL_API_KEY").map_err(|e| {
                BtwError::ParseError {
                    path: config_path.clone(),
                    kind: "env",
                    message: format!("missing MISTRAL_API_KEY: {}", e),
                }
            })?;
            Arc::new(llm::MistralClient::new(std::env::var("MISTRAL_API_KEY").unwrap()))
        }
        p => {
            return Err(BtwError::ParseError {
                path: config_path.clone(),
                kind: "llm",
                message: format!("unknown provider '{}'", p),
            })
        }
    };

    let intent_router = intent::IntentRouter::from_file(
        &commands_path,
        intent::IntentConfig {
            deterministic_threshold: cfg.intent.deterministic_threshold,
            llm_fallback_threshold: cfg.intent.llm_fallback_threshold,
        },
        llm_client.clone(),
    )?;

    let decision_manager = decision::DecisionManager::new(decision::DecisionConfig {
        deterministic_threshold: cfg.intent.deterministic_threshold,
    });

    let mut exec = executor::Executor::new_from_path(
        &commands_path,
        executor::ExecutionCfg {
            confirmation_timeout_seconds: cfg.execution.confirmation_timeout_seconds,
            dry_run: cfg.execution.dry_run,
        },
    )?;

    // NOTE: The legacy `Manager` state machine is retained for unit tests and
    // module compatibility, but runtime behavior is centralized in
    // `handle_transcript` + `Executor` pending confirmation.
    let mut _mgr = manager::Manager::new(decision_manager);

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ListenState {
        Idle,
        Listening,
        Recording,
    }

    let mut state = ListenState::Idle;
    let mut samples: Vec<i16> = Vec::new();
    let mut silence_ms = 0.0;
    let mut start_time: Option<Instant> = None;
    let mut saw_post_wake_speech = false;

    let mut porcupine = porcupine;
    let mut last_heartbeat = Instant::now();
    let mut last_listening_debug = Instant::now();
    let mut pending_confirm_request_id: Option<String> = None;

    // Optional: dump recorded audio for debugging, controlled by env var.
    // Example: export BTWD_DEBUG_AUDIO_DIR=/tmp/btwd-audio
    let debug_audio_dir: Option<PathBuf> = std::env::var("BTWD_DEBUG_AUDIO_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from);
    if let Some(dir) = &debug_audio_dir {
        eprintln!("debug: BTWD_DEBUG_AUDIO_DIR enabled: {}", dir.display());
    }

    loop {
        // Confirmation polling happens ONLY when the Executor has a pending command.
        // The UI helper writes 'yes'/'no' into $XDG_RUNTIME_DIR/btwd-confirm-<request_id>.
        if let Some(req_id) = exec.pending_request_id().map(|s| s.to_string()) {
            let runtime_dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
            let path = std::path::Path::new(&runtime_dir).join(format!("btwd-confirm-{}", req_id));
            if let Ok(action) = std::fs::read_to_string(&path) {
                let _ = std::fs::remove_file(&path);
                let action = action.trim().to_ascii_lowercase();
                if action == "no" {
                    eprintln!("exec: cancel via notification");
                    let _ = exec.cancel_pending("user canceled");
                    // Best-effort: ensure no stale spool survives.
                    let _ = std::fs::remove_file(&path);
                    pending_confirm_request_id = None;
                } else if action == "yes" {
                    eprintln!("exec: confirm via notification");
                    let _ = exec.confirm_pending();
                    pending_confirm_request_id = None;
                }
            } else {
                let should_notify = pending_confirm_request_id.as_deref() != Some(&req_id);
                if should_notify {
                    pending_confirm_request_id = Some(req_id.clone());
                    ui::notify_confirm_actions(cfg.ui.osd, &req_id, "btwd", "Confirm command");
                }
            }
        } else {
            pending_confirm_request_id = None;
        }

        let frame = rx.recv().map_err(|_| {
            BtwError::ParseError {
                path: config_path.clone(),
                kind: "audio",
                message: "audio stream ended".into(),
            }
        })?;

        // Ticks should be serviced regardless of audio state.
        exec.handle_tick(Instant::now());

        // Periodic heartbeat so it's obvious we're alive while idle.
        if matches!(state, ListenState::Idle) && last_heartbeat.elapsed() >= Duration::from_secs(30) {
            eprintln!("Listening for wake word...");
            last_heartbeat = Instant::now();
        }

        match state {
            ListenState::Idle => {
                // Wake word detection.
                if porcupine.process(&frame)? {
                    eprintln!("wake: detected (porcupine)");
                    // Single source of truth: notification only on Idle -> Listening.
                    ui::notify_listening(cfg.ui.osd, cfg.ui.osd_timeout_ms);

                    // Do NOT reuse this frame as user speech.
                    state = ListenState::Listening;
                    // Legacy manager wake handling removed from runtime path.
                    samples.clear();
                    silence_ms = 0.0;
                    start_time = None;
                    saw_post_wake_speech = false;
                    eprintln!("state: Idle -> Listening (armed, waiting for speech)");
                }
                continue;
            }
            ListenState::Listening => {
                // We're "armed" after wake word. We start recording only once we see actual speech.
                // This prevents the wake-word tail from being fed to ASR/UI/routing.

                // Allow re-wake while armed (useful if we got stuck waiting for speech).
                if porcupine.process(&frame)? {
                    eprintln!("wake: detected again while Listening (re-arming)");
                    ui::notify_listening(cfg.ui.osd, cfg.ui.osd_timeout_ms);
                    samples.clear();
                    silence_ms = 0.0;
                    start_time = None;
                    saw_post_wake_speech = false;
                    last_listening_debug = Instant::now();
                    continue;
                }

                let sum_sq: f64 = frame.iter().map(|&s| {
                    let v = s as f64;
                    v * v
                }).sum();
                let rms = (sum_sq / frame_length as f64).sqrt() / i16::MAX as f64;

                let vad_speech = vad.is_speech(&frame);
                // Fallback: treat sufficiently loud audio as speech onset.
                // This uses the existing configured silence threshold.
                let rms_speech = rms >= cfg.speech.silence_threshold as f64;
                let speech = vad_speech || rms_speech;

                // Debug every ~2s while waiting for speech so we can confirm if VAD is firing.
                if last_listening_debug.elapsed() >= Duration::from_secs(2) {
                    eprintln!(
                        "listening: awaiting speech (vad_speech={}, rms_speech={}, rms={:.4}, vad_mode={})",
                        vad_speech,
                        rms_speech,
                        rms,
                        cfg.speech.vad_mode
                    );
                    last_listening_debug = Instant::now();
                }

                if speech {
                    state = ListenState::Recording;
                    // Legacy manager deciding state removed from runtime path.
                    samples.clear();
                    silence_ms = 0.0;
                    start_time = Some(Instant::now());
                    saw_post_wake_speech = true;
                    samples.extend_from_slice(&frame);
                    eprintln!("speech: detected (vad) -> start recording");
                    eprintln!("state: Listening -> Recording");
                }
                continue;
            }
            ListenState::Recording => {
                // Keep buffering audio during recording.
                samples.extend_from_slice(&frame);
            }
        }

        // RMS (existing logic)
        let sum_sq: f64 = frame.iter().map(|&s| {
            let v = s as f64;
            v * v
        }).sum();
        let rms = (sum_sq / frame_length as f64).sqrt() / i16::MAX as f64;

        let speech = vad.is_speech(&frame);

        if !speech && rms < cfg.speech.silence_threshold as f64 {
            silence_ms += frame_ms;
        } else {
            silence_ms = 0.0;
        }

        let elapsed = start_time.map(|t| t.elapsed().as_secs_f64()).unwrap_or(0.0);

        if silence_ms >= cfg.speech.silence_duration_ms as f64 ||
           elapsed >= cfg.speech.max_utterance_seconds as f64 {

            eprintln!(
                "recording: stop (samples={}, elapsed_sec={:.2}, silence_ms={:.0})",
                samples.len(),
                elapsed,
                silence_ms
            );

            // Optionally dump captured audio to disk for debugging.
            if let Some(dir) = &debug_audio_dir {
                if let Err(e) = std::fs::create_dir_all(dir) {
                    eprintln!("debug: failed to create BTWD_DEBUG_AUDIO_DIR: {}", e);
                } else {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis();
                    let path = dir.join(format!("btwd-{}.pcm16", ts));
                    let bytes: Vec<u8> = samples
                        .iter()
                        .flat_map(|s| s.to_le_bytes())
                        .collect();
                    match std::fs::write(&path, bytes) {
                        Ok(_) => eprintln!("debug: audio saved: {}", path.display()),
                        Err(e) => eprintln!("debug: failed to save audio: {}", e),
                    }
                }
            }

            // Only attempt ASR if we actually transitioned to Recording because we saw speech.
            // (This should always be true in Recording state, but keep the invariant explicit.)
            if saw_post_wake_speech && !samples.is_empty() {
                eprintln!("asr: sending audio to worker");
                match worker.transcribe(samples.clone(), sample_rate) {
                    Ok(resp) => {
                        if let Some(err) = resp.error.as_deref() {
                            if !err.is_empty() {
                                eprintln!("asr: worker returned error: {}", err);
                            }
                        }
                        let raw_text = resp.text;
                        let text = raw_text.trim();
                        eprintln!("asr: text='{}'", raw_text);

                        // Never show a transcript for the wake word alone; this is post-wake speech only.
                        ui::notify_text(cfg.ui.osd, cfg.ui.osd_timeout_ms, "You", text);

                        // Centralized strict decision logic: exactly one path.
                        handle_transcript(text, &cfg, &mut exec, &intent_router, &llm_client);
                    }
                    Err(e) => eprintln!("ASR error: {}", e),
                }
            } else {
                eprintln!("asr: skipped (no post-wake speech captured)");
            }

            state = ListenState::Idle;
            samples.clear();
            silence_ms = 0.0;
            start_time = None;
            saw_post_wake_speech = false;
            eprintln!("state: -> Idle");
        }
    }
}

fn expected_missing(xdg: &BaseDirectories, filename: &str, kind: &'static str) -> BtwError {
    let expected = xdg.get_config_home().join(filename);
    BtwError::MissingFile { path: expected, kind }
}
