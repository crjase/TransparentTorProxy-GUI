use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::OnceCell;
use tokio::time::timeout;

use crate::observations::{self, TtpStatus};

const READ_TIMEOUT: Duration = Duration::from_secs(5);
const SUDO: &str = "/usr/bin/sudo";
const PASSWORD_PROMPT: &str = "[ttp-desktop:sudo] Password: ";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TtpOptions {
    pub verbose: bool,
    pub quiet: bool,
    pub log_format: String,
    pub require_vpn: String,
    pub extra_start_flags: String,
    pub live_watchdog: bool,
}

impl Default for TtpOptions {
    fn default() -> Self {
        Self {
            verbose: false,
            quiet: false,
            log_format: "text".into(),
            require_vpn: String::new(),
            extra_start_flags: String::new(),
            live_watchdog: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Toggle,
    Start,
    Stop,
    Restart,
    Refresh,
    Check,
    CheckLeak,
    WatchdogStart,
    WatchdogStop,
    Diagnose,
    Logs,
}

impl Operation {
    pub fn verb(self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Refresh => "refresh",
            Self::Check => "check",
            Self::CheckLeak => "check-leak",
            Self::WatchdogStart => "watchdog start",
            Self::WatchdogStop => "watchdog stop",
            Self::Diagnose => "diagnose",
            Self::Logs => "logs",
        }
    }
}

#[derive(Clone, Copy)]
pub enum Invocation {
    Status { json: bool },
    Start,
    Stop { restore_only: bool },
    Restart,
    Refresh,
    Check,
    CheckLeak,
    Watchdog { subcommand: &'static str },
    Diagnose,
    Logs,
}

impl Invocation {
    fn args(self) -> &'static [&'static str] {
        match self {
            Self::Status { json: true } => &["status", "--json"],
            Self::Status { json: false } => &["status"],
            Self::Start => &["start"],
            Self::Stop {
                restore_only: false,
            } => &["stop"],
            Self::Stop { restore_only: true } => &["stop", "--restore-only"],
            Self::Restart => &["restart"],
            Self::Refresh => &["refresh"],
            Self::Check => &["check"],
            Self::CheckLeak => &["check-leak"],
            Self::Watchdog { subcommand: "start" } => &["watchdog", "start"],
            Self::Watchdog { subcommand: "stop" } => &["watchdog", "stop"],
            Self::Watchdog { .. } => &["watchdog", "status"],
            Self::Diagnose => &["diagnose"],
            Self::Logs => &["logs"],
        }
    }

    pub fn build_args(self, opts: &TtpOptions) -> Vec<String> {
        let mut out = Vec::new();
        if !matches!(self, Self::Status { .. }) {
            if opts.verbose {
                out.push("--verbose".into());
            } else if opts.quiet {
                out.push("--quiet".into());
            }
            if opts.log_format == "json" {
                out.push("--log-format".into());
                out.push("json".into());
            }
        }
        out.extend(self.args().iter().map(|s| (*s).to_owned()));
        if matches!(self, Self::Start) {
            let vpn = opts.require_vpn.trim();
            if !vpn.is_empty() {
                out.push("--require-vpn".into());
                out.push(vpn.to_owned());
            }
            for flag in opts.extra_start_flags.split_whitespace() {
                out.push(flag.to_owned());
            }
        }
        out
    }

    pub fn progress_after(self) -> Duration {
        match self {
            Self::Status { .. } | Self::Check | Self::Watchdog { .. } => READ_TIMEOUT,
            _ => Duration::from_secs(10),
        }
    }
}

#[derive(Clone, Debug)]
pub enum StatusError {
    AuthenticationRequired,
    Failed(String),
}

impl From<String> for StatusError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

pub struct CommandOutput {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
    pub sent_password: bool,
}

impl CommandOutput {
    pub fn requires_root(&self) -> bool {
        !self.status.success()
            && format!("{}\n{}", self.stdout, self.stderr)
                .to_ascii_lowercase()
                .contains("must be run as root")
    }

    pub fn requires_password(&self) -> bool {
        !self.status.success()
            && self.stderr.lines().any(|line| {
                line.trim_start().starts_with("sudo:") && line.contains("a password is required")
            })
    }

    pub fn rejected_password(&self) -> bool {
        !self.status.success()
            && self.sent_password
            && (self.stderr.contains("Sorry, try again.")
                || self.stderr.contains("incorrect password attempt"))
    }

    pub fn error(&self, command: &str) -> String {
        let details = [&self.stderr, &self.stdout]
            .iter()
            .flat_map(|text| text.lines())
            .map(|line| {
                line.trim()
                    .trim_matches(['│', '─', '╭', '╰', '╮', '╯'])
                    .trim()
            })
            .filter(|line| !line.is_empty())
            .take(4)
            .collect::<Vec<_>>()
            .join(" ");
        let outcome = self.status.code().map_or_else(
            || "was interrupted".to_owned(),
            |code| format!("exited with code {code}"),
        );
        if details.is_empty() {
            format!("ttp {command} {outcome}.")
        } else {
            let details: String = details.chars().take(380).collect();
            format!("ttp {command} {outcome}. {details}")
        }
    }
}

fn trusted_ttp() -> Result<&'static str, String> {
    for name in ["/usr/bin/ttp", "/usr/local/bin/ttp"] {
        let path = Path::new(name);
        let Ok(canonical) = path.canonicalize() else {
            continue;
        };
        let Ok(executable) = canonical.metadata() else {
            continue;
        };
        let trusted = |part: &Path| {
            part.metadata()
                .is_ok_and(|metadata| metadata.uid() == 0 && metadata.mode() & 0o022 == 0)
        };
        if executable.is_file()
            && executable.mode() & 0o111 != 0
            && canonical.ancestors().all(trusted)
            && path
                .parent()
                .is_some_and(|parent| parent.ancestors().all(trusted))
        {
            return Ok(name);
        }
        return Err(format!(
            "{name} must be root-owned and protected against writes by other users."
        ));
    }
    Err("TTP is missing. Install the upstream Arch package to provide /usr/bin/ttp.".into())
}

fn process(program: &str) -> Command {
    let mut command = Command::new(program);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .envs([
            ("LC_ALL", "C"),
            ("LANG", "C"),
            ("TERM", "dumb"),
            ("NO_COLOR", "1"),
            ("COLUMNS", "240"),
            ("PYTHONUNBUFFERED", "1"),
        ]);
    command
}

fn output(status: ExitStatus, stdout: &[u8], stderr: &[u8], sent_password: bool) -> CommandOutput {
    CommandOutput {
        status,
        stdout: observations::clean(stdout),
        stderr: observations::clean(stderr).replace(PASSWORD_PROMPT, ""),
        sent_password,
    }
}

async fn local(program: &str, args: &[&str], deadline: Duration) -> Result<CommandOutput, String> {
    let label = args.first().copied().unwrap_or("");
    let result = timeout(
        deadline,
        process(program).args(args).kill_on_drop(true).output(),
    )
    .await
    .map_err(|_| {
        format!(
            "{} {label} timed out after {} seconds.",
            program.rsplit('/').next().unwrap_or(program),
            deadline.as_secs()
        )
    })?
    .map_err(|error| format!("Could not run {program} {label}: {error}"))?;
    Ok(output(result.status, &result.stdout, &result.stderr, false))
}

pub fn parse_status(output: &CommandOutput, json: bool) -> Result<TtpStatus, String> {
    if !output.status.success() {
        return Err(output.error("status"));
    }
    observations::status(&output.stdout, json)
        .ok_or_else(|| "TTP returned an unrecognized session status.".into())
}

pub async fn public_status(format: &OnceCell<bool>) -> Result<TtpStatus, StatusError> {
    let ttp = trusted_ttp()?;
    let json = *format
        .get_or_try_init(|| async {
            let help = local(ttp, &["status", "--help"], READ_TIMEOUT).await?;
            if !help.status.success() {
                return Err(help.error("status --help"));
            }
            Ok(help.stdout.contains("--json"))
        })
        .await?;
    let result = local(ttp, Invocation::Status { json }.args(), READ_TIMEOUT).await?;
    if !result.status.success() {
        let diagnostic = format!("{} {}", result.stdout, result.stderr).to_ascii_lowercase();
        if result.requires_root()
            || diagnostic.contains("cannot read the session intent")
            || diagnostic.contains("permission denied")
        {
            return Err(StatusError::AuthenticationRequired);
        }
    }
    let status = parse_status(&result, json)?;
    // Older TTP releases may report INACTIVE when the private directory is inaccessible.
    if !json
        && status.phase == observations::Phase::Inactive
        && std::fs::File::open("/run/ttp/ttp.lock")
            .is_err_and(|error| error.kind() == io::ErrorKind::PermissionDenied)
    {
        return Err(StatusError::AuthenticationRequired);
    }
    Ok(status)
}

pub async fn check() -> Result<CommandOutput, String> {
    local(trusted_ttp()?, &["check"], READ_TIMEOUT).await
}

pub async fn watchdog_status() -> Result<CommandOutput, String> {
    local(trusted_ttp()?, &["watchdog", "status"], READ_TIMEOUT).await
}

pub async fn elevated_with_options(
    request: Invocation,
    password: Option<&str>,
    opts: &TtpOptions,
) -> Result<CommandOutput, String> {
    let ttp = trusted_ttp()?;
    let dyn_args = request.build_args(opts);
    let mut command = process(SUDO);
    command.current_dir("/");
    match password {
        Some(_) => command
            .args(["-S", "-p", PASSWORD_PROMPT])
            .stdin(Stdio::piped()),
        None => command.arg("-n"),
    };
    command.args(["--", ttp]).args(&dyn_args);
    let read_only = matches!(request, Invocation::Status { .. });
    command.kill_on_drop(read_only);

    let run = async {
        let Some(password) = password else {
            let result = command.output().await?;
            return Ok(output(result.status, &result.stdout, &result.stderr, false));
        };
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("piped sudo input");
        let mut stdout = child.stdout.take().expect("piped sudo output");
        let stderr = child.stderr.take().expect("piped sudo errors");
        let mut stdout_bytes = Vec::new();
        let (status, _, (stderr_bytes, sent)) = tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut stdout_bytes),
            supply_password(stderr, stdin, password),
        )?;
        Ok::<_, io::Error>(output(status, &stdout_bytes, &stderr_bytes, sent))
    };
    let result = if read_only {
        timeout(READ_TIMEOUT, run)
            .await
            .map_err(|_| "The authenticated TTP status read timed out.")?
    } else {
        run.await
    };
    result.map_err(|error| format!("Could not run sudo ttp {}: {error}", request.args()[0]))
}

pub async fn tor_version() -> Option<String> {
    let result = local("/usr/bin/tor", &["--version"], Duration::from_secs(2))
        .await
        .ok()?;
    if !result.status.success() {
        return None;
    }
    let version = result
        .stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("Tor version "))?
        .split_whitespace()
        .next()?
        .trim_end_matches('.');
    Some(version.to_owned())
}

async fn supply_password<R, W>(
    mut stderr: R,
    mut stdin: W,
    password: &str,
) -> io::Result<(Vec<u8>, bool)>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut bytes = Vec::new();
    let prompt = PASSWORD_PROMPT.as_bytes();
    let prompted = timeout(READ_TIMEOUT, async {
        let mut chunk = [0; 1024];
        loop {
            let count = stderr.read(&mut chunk).await?;
            if count == 0 {
                return Ok::<_, io::Error>(false);
            }
            bytes.extend_from_slice(&chunk[..count]);
            if bytes.windows(prompt.len()).any(|part| part == prompt) {
                return Ok(true);
            }
        }
    })
    .await
    .unwrap_or(Ok(false))?;
    let sent = prompted
        && stdin.write_all(password.as_bytes()).await.is_ok()
        && stdin.write_all(b"\n").await.is_ok();
    drop(stdin);
    stderr.read_to_end(&mut bytes).await?;
    Ok((bytes, sent))
}

pub async fn elevated(
    request: Invocation,
    password: Option<&str>,
) -> Result<CommandOutput, String> {
    let ttp = trusted_ttp()?;
    let mut command = process(SUDO);
    command.current_dir("/");
    match password {
        Some(_) => command
            .args(["-S", "-p", PASSWORD_PROMPT])
            .stdin(Stdio::piped()),
        None => command.arg("-n"),
    };
    command.args(["--", ttp]).args(request.args());
    let read_only = matches!(request, Invocation::Status { .. });
    command.kill_on_drop(read_only);

    let run = async {
        let Some(password) = password else {
            let result = command.output().await?;
            return Ok(output(result.status, &result.stdout, &result.stderr, false));
        };
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("piped sudo input");
        let mut stdout = child.stdout.take().expect("piped sudo output");
        let stderr = child.stderr.take().expect("piped sudo errors");
        let mut stdout_bytes = Vec::new();
        let (status, _, (stderr_bytes, sent)) = tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut stdout_bytes),
            supply_password(stderr, stdin, password),
        )?;
        Ok::<_, io::Error>(output(status, &stdout_bytes, &stderr_bytes, sent))
    };
    let result = if read_only {
        timeout(READ_TIMEOUT, run)
            .await
            .map_err(|_| "The authenticated TTP status read timed out.")?
    } else {
        run.await
    };
    result.map_err(|error| format!("Could not run sudo ttp {}: {error}", request.args()[0]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn password_is_sent_only_after_sudos_prompt() {
        let (stderr, mut fake_stderr) = duplex(256);
        let (stdin, mut fake_stdin) = duplex(256);
        let sudo = async move {
            fake_stderr
                .write_all(PASSWORD_PROMPT.as_bytes())
                .await
                .unwrap();
            let mut password = String::new();
            fake_stdin.read_to_string(&mut password).await.unwrap();
            assert_eq!(password, "test password\n");
        };
        let (result, ()) = tokio::join!(supply_password(stderr, stdin, "test password"), sudo);
        assert!(result.unwrap().1);

        let (stderr, mut fake_stderr) = duplex(256);
        let (stdin, mut fake_stdin) = duplex(256);
        let command = async {
            fake_stderr
                .write_all(b"command diagnostic\n")
                .await
                .unwrap();
            drop(fake_stderr);
            let mut received = Vec::new();
            fake_stdin.read_to_end(&mut received).await.unwrap();
            assert!(received.is_empty());
        };
        let (result, ()) = tokio::join!(supply_password(stderr, stdin, "test password"), command);
        assert!(!result.unwrap().1);
    }
}
