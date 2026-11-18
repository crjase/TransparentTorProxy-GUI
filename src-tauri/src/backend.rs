use std::net::IpAddr;
use std::sync::Mutex;

use gio::prelude::ApplicationExt;
use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, WindowEvent,
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tokio::sync::{Mutex as AsyncMutex, OnceCell, oneshot};
use tokio::time::timeout;
use zeroize::Zeroizing;

use crate::cli::{self, CommandOutput, Invocation, Operation, StatusError, TtpOptions};
use crate::observations::{self, Phase, TtpCheckResult, TtpStatus, Verification};

const SNAPSHOT_EVENT: &str = "ttp://snapshot";
const WINDOW_LABEL: &str = "main";

type Password = Zeroizing<String>;

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthPrompt {
    id: u64,
    rejected: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct WatchdogSnapshot {
    state: String,
    label: String,
    detail: String,
    daemon_running: bool,
}

impl Default for WatchdogSnapshot {
    fn default() -> Self {
        Self {
            state: "idle".into(),
            label: "WATCHDOG · STANDBY".into(),
            detail: "No anomalies detected".into(),
            daemon_running: false,
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    revision: u64,
    status: Option<TtpStatus>,
    status_error: Option<String>,
    status_needs_auth: bool,
    tor_version: Option<String>,
    check: Option<TtpCheckResult>,
    watchdog: WatchdogSnapshot,
    options: TtpOptions,
    operation: Option<Operation>,
    slow: bool,
    auth: Option<AuthPrompt>,
    notice: Option<String>,
}

#[derive(Default)]
struct Model {
    view: Snapshot,
    generation: u64,
    challenge: u64,
    quitting: bool,
    auth_reply: Option<oneshot::Sender<Option<Password>>>,
    password: Option<Password>,
    rotation: Option<Rotation>,
}

struct Rotation {
    ip: Option<IpAddr>,
}

impl Model {
    fn observe(&mut self, result: Result<TtpStatus, StatusError>) {
        self.view.status_needs_auth = matches!(result, Err(StatusError::AuthenticationRequired));
        match result {
            Ok(mut status) => {
                if let Some(previous) = self
                    .view
                    .status
                    .as_ref()
                    .filter(|old| old.same_session(&status))
                {
                    if status.recorded_verification {
                        if status.verification_observed_at != previous.verification_observed_at {
                            self.rotation = None;
                            self.view.check = None;
                        } else if let Some(rotation) = &self.rotation {
                            status.exit_ip = rotation.ip.or(status.exit_ip);
                            status.verification = Verification::Unknown;
                        }
                    } else {
                        self.rotation = None;
                        status.verification = previous.verification;
                        status.exit_ip = status.exit_ip.or(previous.exit_ip);
                        status.country = status.country.or_else(|| previous.country.clone());
                    }
                } else {
                    self.view.check = None;
                    self.rotation = None;
                }
                self.view.status = Some(status);
                self.view.status_error = None;
            }
            Err(StatusError::AuthenticationRequired) => self.view.status_error = None,
            Err(StatusError::Failed(error)) => self.view.status_error = Some(error),
        }
    }
}

#[derive(Default)]
struct Backend {
    model: Mutex<Model>,
    observation: AsyncMutex<()>,
    json_status: OnceCell<bool>,
}

impl Backend {
    fn snapshot(&self) -> Snapshot {
        self.model.lock().unwrap().view.clone()
    }

    fn change<T>(&self, app: &AppHandle, update: impl FnOnce(&mut Model) -> T) -> T {
        let mut model = self.model.lock().unwrap();
        let prior = model.view.clone();
        let result = update(&mut model);
        if prior != model.view {
            model.view.revision += 1;
            let snapshot = model.view.clone();
            drop(model);
            let _ = app.emit_to(WINDOW_LABEL, SNAPSHOT_EVENT, snapshot);
        }
        result
    }
}

#[tauri::command]
async fn ttp_status(app: AppHandle) -> Snapshot {
    let backend = app.state::<Backend>();
    let Ok(_guard) = backend.observation.try_lock() else {
        return backend.snapshot();
    };
    let generation = {
        let model = backend.model.lock().unwrap();
        if model.view.operation.is_some() {
            return model.view.clone();
        }
        model.generation
    };
    let status = read_session(&app, &backend, false).await;
    let poll_wd = backend.model.lock().unwrap().view.options.live_watchdog;
    let wd_out = if poll_wd {
        cli::watchdog_status().await.ok()
    } else {
        None
    };
    backend.change(&app, |model| {
        if generation == model.generation && model.view.operation.is_none() {
            model.observe(status);
            model.view.watchdog = compute_watchdog(model.view.status.as_ref(), wd_out.as_ref());
        }
    });
    backend.snapshot()
}

#[tauri::command]
async fn ttp_toggle(app: AppHandle) -> Result<(), String> {
    operate(app, Operation::Toggle).await.map(|_| ())
}

#[tauri::command]
async fn ttp_start(app: AppHandle) -> Result<(), String> {
    operate(app, Operation::Start).await.map(|_| ())
}

#[tauri::command]
async fn ttp_stop(app: AppHandle) -> Result<(), String> {
    operate(app, Operation::Stop).await.map(|_| ())
}

#[tauri::command]
async fn ttp_refresh(app: AppHandle) -> Result<Option<IpAddr>, String> {
    operate(app, Operation::Refresh)
        .await
        .map(|result| result.ip)
}

#[tauri::command]
async fn ttp_check(app: AppHandle) -> Result<Option<TtpCheckResult>, String> {
    operate(app, Operation::Check)
        .await
        .map(|result| result.check)
}

#[tauri::command]
async fn ttp_restart(app: AppHandle) -> Result<(), String> {
    operate(app, Operation::Restart).await.map(|_| ())
}

#[tauri::command]
async fn ttp_check_leak(app: AppHandle) -> Result<(), String> {
    operate(app, Operation::CheckLeak).await.map(|_| ())
}

#[tauri::command]
async fn ttp_watchdog_start(app: AppHandle) -> Result<(), String> {
    operate(app, Operation::WatchdogStart).await.map(|_| ())
}

#[tauri::command]
async fn ttp_watchdog_stop(app: AppHandle) -> Result<(), String> {
    operate(app, Operation::WatchdogStop).await.map(|_| ())
}

#[tauri::command]
async fn ttp_diagnose(app: AppHandle) -> Result<(), String> {
    operate(app, Operation::Diagnose).await.map(|_| ())
}

#[tauri::command]
async fn ttp_logs(app: AppHandle) -> Result<(), String> {
    operate(app, Operation::Logs).await.map(|_| ())
}

#[tauri::command]
fn ttp_save_options(app: AppHandle, options: TtpOptions) -> Snapshot {
    let backend = app.state::<Backend>();
    backend.change(&app, |model| {
        model.view.options = options;
    });
    backend.snapshot()
}

#[tauri::command]
fn ttp_authenticate(app: AppHandle, id: u64, password: String) -> Result<(), String> {
    let password = Zeroizing::new(password);
    if password.is_empty() || password.len() > 1024 || password.contains(['\n', '\r', '\0']) {
        return Err("Enter a sudo password without line breaks.".into());
    }
    reply_to_prompt(&app, id, Some(password))
}

#[tauri::command]
fn ttp_cancel_authentication(app: AppHandle, id: u64) -> Result<(), String> {
    reply_to_prompt(&app, id, None)
}

fn reply_to_prompt(app: &AppHandle, id: u64, password: Option<Password>) -> Result<(), String> {
    let sender = app.state::<Backend>().change(app, |model| {
        if model.view.auth.as_ref().map(|prompt| prompt.id) != Some(id) {
            return None;
        }
        model.view.auth = None;
        model.auth_reply.take()
    });
    sender
        .ok_or("The password prompt has expired.")?
        .send(password)
        .map_err(|_| "The password prompt has expired.".into())
}

async fn ask(app: &AppHandle, backend: &Backend, rejected: bool) -> Option<Password> {
    let (sender, receiver) = oneshot::channel();
    backend.change(app, |model| {
        model.challenge += 1;
        model.view.auth = Some(AuthPrompt {
            id: model.challenge,
            rejected,
        });
        model.view.slow = false;
        model.auth_reply = Some(sender);
    });
    show_window(app);
    receiver.await.ok().flatten()
}

async fn run_sudo(
    app: &AppHandle,
    backend: &Backend,
    request: Invocation,
    password: Option<&str>,
) -> Result<CommandOutput, String> {
    let opts = backend.model.lock().unwrap().view.options.clone();
    let process = cli::elevated_with_options(request, password, &opts);
    if matches!(request, Invocation::Status { .. }) {
        return process.await;
    }
    tokio::pin!(process);
    match timeout(request.progress_after(), process.as_mut()).await {
        Ok(result) => result,
        Err(_) => {
            backend.change(app, |model| model.view.slow = true);
            process.await
        }
    }
}

async fn authorized(
    app: &AppHandle,
    backend: &Backend,
    request: Invocation,
    interactive: bool,
) -> Result<Option<CommandOutput>, String> {
    let initial = run_sudo(app, backend, request, None).await?;
    if !initial.requires_password() {
        return Ok(Some(initial));
    }

    let mut cached = backend.model.lock().unwrap().password.clone();
    let mut rejected = false;
    loop {
        let password = match cached.take() {
            Some(password) => password,
            None if !interactive => return Ok(None),
            None => match ask(app, backend, rejected).await {
                Some(password) => password,
                None => return Ok(None),
            },
        };
        let result = run_sudo(app, backend, request, Some(&password)).await?;
        if result.rejected_password() {
            let mut model = backend.model.lock().unwrap();
            if model.password.as_ref() == Some(&password) {
                model.password = None;
            }
            rejected = true;
        } else {
            if result.sent_password && result.status.success() {
                backend.model.lock().unwrap().password = Some(password);
            }
            return Ok(Some(result));
        }
    }
}

async fn read_session(
    app: &AppHandle,
    backend: &Backend,
    interactive: bool,
) -> Result<TtpStatus, StatusError> {
    match cli::public_status(&backend.json_status).await {
        Err(StatusError::AuthenticationRequired) => {
            let json = backend.json_status.get().copied().unwrap_or(false);
            let output = authorized(app, backend, Invocation::Status { json }, interactive)
                .await?
                .ok_or(StatusError::AuthenticationRequired)?;
            cli::parse_status(&output, json).map_err(|error| {
                StatusError::Failed(format!("Authenticated status read failed. {error}"))
            })
        }
        other => other,
    }
}

#[derive(Default)]
struct Outcome {
    ip: Option<IpAddr>,
    check: Option<TtpCheckResult>,
}

fn complete(
    model: &mut Model,
    operation: Operation,
    run: Result<Option<CommandOutput>, String>,
) -> Result<Outcome, String> {
    let output = match run {
        Ok(Some(output)) => output,
        Ok(None) => {
            model.view.notice = Some("Authentication cancelled.".into());
            return Ok(Outcome::default());
        }
        Err(error) => {
            if operation == Operation::Check
                && let Some(status) = &mut model.view.status
            {
                status.verification = Verification::Unknown;
            }
            return Err(error);
        }
    };
    let success = output.status.success();
    let mut outcome = Outcome::default();
    match operation {
        Operation::Toggle => return Err("The proxy state could not be resolved.".into()),
        Operation::Start => {
            let address = observations::start_confirmation(&output.stdout);
            let configured = success
                || output
                    .stdout
                    .contains("Session active but Tor verification failed.");
            if configured && (model.view.status_error.is_some() || model.view.status_needs_auth) {
                model.view.status = Some(TtpStatus::new(Phase::Active));
                model.view.check = None;
                model.rotation = None;
            }
            if success
                && let (Some(status), Some(address)) = (&mut model.view.status, address)
                && status.phase == Phase::Active
                && !status.recorded_verification
            {
                status.exit_ip = Some(address);
                status.verification = Verification::Confirmed;
            }
        }
        Operation::Stop => {
            if success && (model.view.status_error.is_some() || model.view.status_needs_auth) {
                model.view.status = Some(TtpStatus::new(Phase::Inactive));
                model.view.check = None;
                model.rotation = None;
            }
        }
        Operation::Refresh => {
            if success {
                let (address, changed) = observations::refreshed_ip(&output.stdout);
                outcome.ip = address;
                if let Some(status) = model
                    .view
                    .status
                    .as_mut()
                    .filter(|status| status.phase == Phase::Active)
                {
                    model.view.check = None;
                    model.rotation = Some(Rotation { ip: address });
                    status.exit_ip = address.or(status.exit_ip);
                    status.verification = Verification::Unknown;
                }
                model.view.notice = Some(match (address, changed) {
                    (Some(_), true) => "Exit IP updated.".into(),
                    (Some(_), false) => "Circuit rotated. Exit IP may stay the same.".into(),
                    (None, _) => "Circuit rotation finished. Exit IP unavailable.".into(),
                });
            }
        }
        Operation::Check => {
            outcome.check = observations::check(&output.stdout);
            if let Some(status) = model
                .view
                .status
                .as_mut()
                .filter(|status| status.phase == Phase::Active)
            {
                status.verification = Verification::Unknown;
                if let Some(check) = &outcome.check {
                    model.rotation = None;
                    status.verification = check.verification;
                    status.exit_ip = check.exit_ip.or(status.exit_ip);
                    status.country = check.country.clone().or(status.country.take());
                }
            }
            model.view.check = outcome.check.clone();
            if success && outcome.check.is_none() {
                return Err("TTP check did not report a routing verdict.".into());
            }
        }
        Operation::Restart => {
            if success {
                model.view.notice = Some("Proxy session restarted.".into());
            }
        }
        Operation::CheckLeak => {
            if success {
                let text = output.stdout.trim();
                let summary = text.lines().last().unwrap_or("Leak check completed.");
                model.view.notice = Some(summary.chars().take(180).collect());
            }
        }
        Operation::WatchdogStart | Operation::WatchdogStop => {
            if success {
                let running = operation == Operation::WatchdogStart;
                model.view.watchdog.daemon_running = running;
                model.view.notice = Some(if running {
                    "Background watchdog started.".into()
                } else {
                    "Background watchdog stopped.".into()
                });
            }
        }
        Operation::Diagnose | Operation::Logs => {
            if success {
                let lines: Vec<&str> = output
                    .stdout
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .collect();
                let tail = lines.iter().rev().take(3).rev().copied().collect::<Vec<_>>().join(" | ");
                model.view.notice = Some(if tail.is_empty() {
                    format!("ttp {} completed.", operation.verb())
                } else {
                    tail.chars().take(220).collect()
                });
            }
        }
    }
    if success {
        Ok(outcome)
    } else {
        Err(output.error(operation.verb()))
    }
}

fn toggle_direction(phase: Phase) -> Operation {
    match phase {
        Phase::Inactive => Operation::Start,
        Phase::Active | Phase::ProtectionRetained => Operation::Stop,
    }
}

async fn perform(
    app: &AppHandle,
    backend: &Backend,
    requested: Operation,
) -> Result<Outcome, String> {
    // Decide an unknown toggle from a fresh observation under the same operation lock.
    // Explicit Start and Stop buttons keep the direction shown when they were clicked.
    let mut phase = backend.snapshot().status.map(|s| s.phase);
    let operation = if requested == Operation::Toggle {
        let observation = {
            let _guard = backend.observation.lock().await;
            read_session(app, backend, true).await
        };
        backend.change(app, |model| model.observe(observation.clone()));
        match observation {
            Ok(status) => {
                phase = Some(status.phase);
                let action = toggle_direction(status.phase);
                backend.change(app, |model| model.view.operation = Some(action));
                action
            }
            Err(StatusError::AuthenticationRequired) => {
                backend.change(app, |model| {
                    model.view.notice = Some("Authentication cancelled.".into())
                });
                return Ok(Outcome::default());
            }
            Err(StatusError::Failed(error)) => return Err(error),
        }
    } else {
        requested
    };
    let request = match operation {
        Operation::Start if phase == Some(Phase::ProtectionRetained) => {
            return Err("Restore the retained protection before starting TTP.".into());
        }
        Operation::Start => Invocation::Start,
        Operation::Stop => Invocation::Stop {
            restore_only: phase == Some(Phase::ProtectionRetained),
        },
        Operation::Restart => Invocation::Restart,
        Operation::Refresh => Invocation::Refresh,
        Operation::Check => Invocation::Check,
        Operation::CheckLeak => Invocation::CheckLeak,
        Operation::WatchdogStart => Invocation::Watchdog { subcommand: "start" },
        Operation::WatchdogStop => Invocation::Watchdog { subcommand: "stop" },
        Operation::Diagnose => Invocation::Diagnose,
        Operation::Logs => Invocation::Logs,
        Operation::Toggle => return Err("The proxy state could not be resolved.".into()),
    };
    let run = if operation == Operation::Check {
        match cli::check().await {
            Ok(check) if !check.requires_root() => Ok(Some(check)),
            Ok(_) => authorized(app, backend, request, true).await,
            Err(error) => Err(error),
        }
    } else {
        authorized(app, backend, request, true).await
    };
    let observation = if matches!(run, Ok(Some(_)))
        && (operation != Operation::Check || backend.json_status.get().copied().unwrap_or(false))
    {
        let _guard = backend.observation.lock().await;
        Some(read_session(app, backend, false).await)
    } else {
        None
    };
    backend.change(app, |model| {
        if let Some(observation) = observation {
            model.observe(observation);
        }
        complete(model, operation, run)
    })
}

async fn operate(app: AppHandle, operation: Operation) -> Result<Outcome, String> {
    let backend = app.state::<Backend>();
    backend.change(&app, |model| {
        if model.quitting {
            return Err("TTP Desktop is closing.");
        }
        if model.view.operation.is_some() {
            return Err("A TTP command is already running.");
        }
        model.generation += 1;
        model.view.operation = Some(operation);
        model.view.slow = false;
        model.view.notice = None;
        if operation == Operation::Check {
            model.view.check = None;
        }
        Ok(())
    })?;
    let result = perform(&app, &backend, operation).await;
    backend.change(&app, |model| {
        model.view.operation = None;
        model.view.slow = false;
        model.view.auth = None;
        model.auth_reply = None;
        if let Err(error) = &result {
            model.view.notice = Some(error.clone());
        }
    });
    result
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn toggle_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        if window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false) {
            let _ = window.hide();
        } else {
            show_window(app);
        }
    }
}

#[tauri::command]
fn quit_desktop(app: AppHandle) -> Result<(), String> {
    let backend = app.state::<Backend>();
    {
        let mut model = backend.model.lock().unwrap();
        if model.view.operation.is_some() {
            return Err("Wait for the TTP command to finish before quitting.".into());
        }
        model.quitting = true;
    }
    app.exit(0);
    Ok(())
}

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let window = MenuItem::with_id(app, "window", "Show / Hide TTP Desktop", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&window])?;
    TrayIconBuilder::with_id("ttp")
        .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .menu(&menu)
        .tooltip("TTP Desktop")
        .on_menu_event(|app, event| {
            if event.id().as_ref() == "window" {
                toggle_window(app);
            }
        })
        .build(app)?;
    Ok(())
}

pub fn run() {
    let context = tauri::generate_context!();
    // GTK 3 also uses the GLib program name for xdg_toplevel.set_app_id.
    gio::glib::set_prgname(Some(context.config().identifier.as_str()));
    tauri::Builder::default()
        .manage(Backend::default())
        .invoke_handler(tauri::generate_handler![
            quit_desktop,
            ttp_status,
            ttp_toggle,
            ttp_start,
            ttp_stop,
            ttp_restart,
            ttp_refresh,
            ttp_check,
            ttp_check_leak,
            ttp_watchdog_start,
            ttp_watchdog_stop,
            ttp_diagnose,
            ttp_logs,
            ttp_save_options,
            ttp_authenticate,
            ttp_cancel_authentication,
        ])
        .setup(|app| {
            // GTK may activate an already running app more than once.
            // Create the window only in this one-time setup callback.
            let mut window = tauri::WebviewWindowBuilder::from_config(
                app.handle(),
                &app.config().app.windows[0],
            )?;
            // X11 needs a compositor to repaint a transparent webview correctly.
            if gdk::Screen::default().is_some_and(|screen| !screen.is_composited()) {
                window = window
                    .transparent(false)
                    .background_color(tauri::window::Color(0, 0, 0, 255));
            }
            window.build()?;
            setup_tray(app.handle())?;
            // A second menu or taskbar launch activates the existing GTK application.
            if let Some(gtk_app) = gio::Application::default() {
                let handle = app.handle().clone();
                gtk_app.connect_activate(move |_| show_window(&handle));
            }
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let version = cli::tor_version().await;
                handle
                    .state::<Backend>()
                    .change(&handle, |model| model.view.tor_version = version);
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(context)
        .expect("could not launch TTP Desktop");
}

fn compute_watchdog(status: Option<&TtpStatus>, wd_out: Option<&CommandOutput>) -> WatchdogSnapshot {
    let mut daemon_running = false;
    let mut wd_issue: Option<String> = None;

    if let Some(out) = wd_out {
        let combined = format!("{}\n{}", out.stdout, out.stderr);
        let low = combined.to_ascii_lowercase();
        daemon_running = (low.contains("running") || low.contains("active") || low.contains("watching"))
            && !low.contains("inactive")
            && !low.contains("stopped");
        if low.contains("anomaly") || low.contains("leak") || low.contains("drift") || low.contains("mismatch") {
            wd_issue = combined.lines().find(|l| !l.trim().is_empty()).map(|l| l.trim().chars().take(120).collect());
        }
    }

    if let Some(issue) = wd_issue {
        return WatchdogSnapshot {
            state: "anomaly".into(),
            label: "WATCHDOG · ANOMALY".into(),
            detail: issue,
            daemon_running,
        };
    }
    if let Some(st) = status {
        if st.phase == Phase::ProtectionRetained {
            return WatchdogSnapshot {
                state: "anomaly".into(),
                label: "WATCHDOG · LOCKDOWN".into(),
                detail: "Emergency protection retained — restore network".into(),
                daemon_running,
            };
        }
        if let Some(issue) = &st.issue {
            return WatchdogSnapshot {
                state: "anomaly".into(),
                label: "WATCHDOG · ANOMALY".into(),
                detail: issue.clone(),
                daemon_running,
            };
        }
        if st.phase == Phase::Active && st.verification == Verification::Negative {
            return WatchdogSnapshot {
                state: "anomaly".into(),
                label: "WATCHDOG · ROUTE ALERT".into(),
                detail: "Tor routing check failed while proxy active".into(),
                daemon_running,
            };
        }
        if st.phase == Phase::Active && !daemon_running {
            return WatchdogSnapshot {
                state: "warning".into(),
                label: "WATCHDOG · INACTIVE".into(),
                detail: "Proxy active, background watchdog not running".into(),
                daemon_running: false,
            };
        }
        if st.phase == Phase::Active && daemon_running {
            return WatchdogSnapshot {
                state: "healthy".into(),
                label: "WATCHDOG · NOMINAL".into(),
                detail: "0 anomalies · session & nftables guarded".into(),
                daemon_running: true,
            };
        }
    }
    if daemon_running {
        return WatchdogSnapshot {
            state: "healthy".into(),
            label: "WATCHDOG · ARMED".into(),
            detail: "Background watchdog running · 0 anomalies".into(),
            daemon_running: true,
        };
    }
    WatchdogSnapshot::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_uses_the_observed_session_direction() {
        assert_eq!(toggle_direction(Phase::Inactive), Operation::Start);
        assert_eq!(toggle_direction(Phase::Active), Operation::Stop);
        assert_eq!(toggle_direction(Phase::ProtectionRetained), Operation::Stop);
    }

    #[test]
    fn private_and_damaged_records_do_not_mean_inactive() {
        let mut model = Model::default();
        model.observe(Err(StatusError::AuthenticationRequired));
        assert!(model.view.status.is_none());
        assert!(model.view.status_needs_auth);
        model.observe(Err(StatusError::Failed(
            "Cannot read the session intent".into(),
        )));
        assert!(model.view.status.is_none());
        assert!(!model.view.status_needs_auth);
        assert_eq!(
            model.view.status_error.as_deref(),
            Some("Cannot read the session intent")
        );
        model.observe(Ok(TtpStatus::new(Phase::Active)));
        assert!(model.view.status_error.is_none());
        assert_eq!(model.view.status.unwrap().phase, Phase::Active);
    }

    #[test]
    fn a_rotation_yields_to_a_new_recorded_verification() {
        let mut model = Model::default();
        let first = r#"{"phase":"running","session_id":"a","verification":{"state":"confirmed","ip":"192.0.2.2","observed_at":12.0}}"#;
        let rotated = "192.0.2.3".parse().unwrap();
        let last = r#"{"phase":"running","session_id":"a","verification":{"state":"confirmed","ip":"192.0.2.3","observed_at":32.0}}"#;

        model.observe(Ok(observations::status(first, true).unwrap()));
        model.rotation = Some(Rotation { ip: Some(rotated) });
        model.observe(Ok(observations::status(first, true).unwrap()));
        let view = model.view.status.as_ref().unwrap();
        assert_eq!(view.exit_ip, Some(rotated));
        assert_eq!(view.verification, Verification::Unknown);

        model.observe(Ok(observations::status(last, true).unwrap()));
        assert!(model.rotation.is_none());
        assert_eq!(
            model.view.status.unwrap().verification,
            Verification::Confirmed
        );
    }
}
