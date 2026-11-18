use std::net::IpAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Inactive,
    Active,
    ProtectionRetained,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    #[default]
    Unknown,
    Confirmed,
    Negative,
    Inconclusive,
}

impl Verification {
    fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "confirmed" => Self::Confirmed,
            "negative" => Self::Negative,
            "inconclusive" | "unconfirmed" => Self::Inconclusive,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TtpStatus {
    pub phase: Phase,
    pub exit_ip: Option<IpAddr>,
    pub started_at: Option<i64>,
    pub country: Option<String>,
    pub verification: Verification,
    pub issue: Option<&'static str>,
    #[serde(skip)]
    pub recorded_verification: bool,
    #[serde(skip)]
    pub verification_observed_at: Option<serde_json::Number>,
    #[serde(skip)]
    pub session_id: Option<String>,
}

impl TtpStatus {
    pub fn new(phase: Phase) -> Self {
        Self {
            phase,
            exit_ip: None,
            started_at: None,
            country: None,
            verification: Verification::Unknown,
            issue: None,
            recorded_verification: false,
            verification_observed_at: None,
            session_id: None,
        }
    }

    pub fn same_session(&self, other: &Self) -> bool {
        self.phase == Phase::Active
            && other.phase == Phase::Active
            && self.session_id == other.session_id
            && (self.started_at.is_none()
                || other.started_at.is_none()
                || self.started_at == other.started_at)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TtpCheckResult {
    pub verification: Verification,
    pub exit_ip: Option<IpAddr>,
    pub latency_ms: Option<u64>,
    pub country: Option<String>,
}

#[derive(Deserialize)]
struct Report {
    phase: String,
    #[serde(default)]
    health: String,
    started: Option<String>,
    session_id: Option<String>,
    verification: Option<RecordedVerification>,
    #[serde(default)]
    session_exit_code: i32,
}

#[derive(Deserialize)]
struct RecordedVerification {
    state: String,
    ip: Option<String>,
    observed_at: Option<serde_json::Number>,
}

pub fn clean(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&strip_ansi_escapes::strip(bytes))
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
        .collect()
}

fn field<'a>(text: &'a str, label: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let line = line.trim().trim_start_matches("[TTP]").trim_start();
        let line = line.trim_start_matches(['-', '•']).trim_start();
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case(label)
            .then_some(value.trim())
    })
}

fn timestamp(value: &str) -> Option<i64> {
    OffsetDateTime::parse(value.trim(), &Rfc3339)
        .ok()
        .map(OffsetDateTime::unix_timestamp)
}

fn uptime_anchor(value: &str) -> Option<i64> {
    let mut parts = value.split(':');
    let hours: i64 = parts.next()?.parse().ok()?;
    let minutes: i64 = parts.next()?.parse().ok()?;
    let seconds: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some()
        || hours < 0
        || !(0..60).contains(&minutes)
        || !(0..60).contains(&seconds)
    {
        return None;
    }
    let uptime = hours
        .checked_mul(3600)?
        .checked_add(minutes * 60 + seconds)?;
    let clock = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    i64::try_from(clock).ok()?.checked_sub(uptime)
}

fn country(text: &str) -> Option<String> {
    let value = field(text, "country")?;
    (value.len() == 2 && value.bytes().all(|byte| byte.is_ascii_alphabetic()))
        .then(|| value.to_ascii_uppercase())
}

fn ip(value: &str) -> Option<IpAddr> {
    value.split_whitespace().next()?.parse().ok()
}

pub fn status(text: &str, json: bool) -> Option<TtpStatus> {
    if json {
        let report: Report = serde_json::from_str(text.trim()).ok()?;
        let phase = match report.phase.as_str() {
            "absent" => Phase::Inactive,
            "running" | "recorded" => Phase::Active,
            "protection_retained" => Phase::ProtectionRetained,
            _ => return None,
        };
        let mut status = TtpStatus::new(phase);
        status.recorded_verification = true;
        status.session_id = report.session_id;
        status.started_at = report.started.as_deref().and_then(timestamp);
        if let Some(record) = report.verification {
            status.verification = Verification::parse(&record.state);
            status.exit_ip = record.ip.as_deref().and_then(ip);
            status.verification_observed_at = record.observed_at;
        }
        status.issue = match (phase, report.health.as_str(), report.session_exit_code) {
            (Phase::ProtectionRetained, _, _) => Some("Startup protection retained"),
            (_, "emergency_verified", _) => Some("Emergency protection active"),
            (_, "containment_unconfirmed", _) => Some("Containment unconfirmed"),
            (Phase::Active, _, 0 | 3) => None,
            (Phase::Active, _, 5) => Some("Required watchdog unavailable"),
            (Phase::Active, _, 9) => Some("TTP notification failed"),
            (Phase::Active, _, _) => Some("TTP reports a protection issue"),
            _ => None,
        };
        return Some(status);
    }

    let phase = match field(text, "status")?.split_whitespace().next()? {
        value if value.eq_ignore_ascii_case("active") => Phase::Active,
        value if value.eq_ignore_ascii_case("inactive") => Phase::Inactive,
        _ => return None,
    };
    let mut status = TtpStatus::new(phase);
    if phase == Phase::Active {
        status.exit_ip = field(text, "exit ip").and_then(ip);
        status.started_at = field(text, "session started")
            .and_then(timestamp)
            .or_else(|| field(text, "uptime").and_then(uptime_anchor));
        status.country = country(text);
    }
    Some(status)
}

pub fn check(text: &str) -> Option<TtpCheckResult> {
    let verification = if let Some(value) = field(text, "tor verification") {
        Verification::parse(value.split(';').next()?)
    } else {
        match field(text, "tor network")?
            .split_whitespace()
            .next()?
            .to_ascii_lowercase()
            .as_str()
        {
            "yes" => Verification::Confirmed,
            "no" => Verification::Inconclusive,
            _ => Verification::Unknown,
        }
    };
    let latency_ms = field(text, "api latency")
        .or_else(|| field(text, "latency"))
        .and_then(|value| value.split_whitespace().next()?.parse().ok());
    Some(TtpCheckResult {
        verification,
        exit_ip: field(text, "observed ip")
            .or_else(|| field(text, "current ip"))
            .or_else(|| field(text, "exit ip"))
            .and_then(ip),
        latency_ms,
        country: country(text),
    })
}

pub fn start_confirmation(text: &str) -> Option<IpAddr> {
    text.lines().find_map(|line| {
        line.split_once("Tor routing confirmed at this observation. Exit IP:")
            .or_else(|| line.split_once("Session active. Exit IP:"))
            .and_then(|(_, value)| ip(value))
    })
}

pub fn refreshed_ip(text: &str) -> (Option<IpAddr>, bool) {
    if let Some(address) = field(text, "new exit ip").and_then(ip) {
        return (Some(address), true);
    }
    let address = field(text, "current ip")
        .or_else(|| text.split_once("Current IP:").map(|(_, value)| value))
        .and_then(ip);
    (address, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_status_distinguishes_active_from_inactive_and_cleartext_ip() {
        let inactive = "[TTP] Status: \x1b[31mINACTIVE\x1b[0m\n[TTP] Current IP: 192.0.2.1";
        assert_eq!(
            status(&clean(inactive.as_bytes()), false),
            Some(TtpStatus::new(Phase::Inactive))
        );
        let active = "[TTP] Status: ACTIVE\n[TTP] Watchdog: Inactive\n[TTP] Exit IP: 2001:db8::8\n[TTP] Session started: 2026-11-18T00:00:00.123456+00:00";
        let result = status(active, false).unwrap();
        assert_eq!(result.phase, Phase::Active);
        assert_eq!(result.exit_ip, Some("2001:db8::8".parse().unwrap()));
        assert_eq!(result.started_at, Some(1_794_960_000));
        assert_eq!(result.verification, Verification::Unknown);
        assert_eq!(result.country, None);
    }

    #[test]
    fn structured_status_preserves_emergency_and_retained_protection() {
        let report = r#"{"phase":"running","health":"emergency_verified","session_exit_code":7,"verification":{"state":"confirmed","ip":"192.0.2.2"}}"#;
        let result = status(report, true).unwrap();
        assert_eq!(result.phase, Phase::Active);
        assert!(result.recorded_verification);
        assert_eq!(result.issue, Some("Emergency protection active"));
        assert_eq!(
            status(r#"{"phase":"protection_retained"}"#, true)
                .unwrap()
                .phase,
            Phase::ProtectionRetained
        );
        assert!(status(r#"{"phase":"unexpected"}"#, true).is_none());
        let slow_check =
            r#"{"phase":"running","session_exit_code":3,"verification":{"state":"inconclusive"}}"#;
        assert_eq!(status(slow_check, true).unwrap().issue, None);
        let missing_watchdog =
            r#"{"phase":"running","session_exit_code":5,"verification":{"state":"confirmed"}}"#;
        assert_eq!(
            status(missing_watchdog, true).unwrap().issue,
            Some("Required watchdog unavailable")
        );
    }

    #[test]
    fn check_and_rotation_require_evidence() {
        let legacy = "  - Current IP: 192.0.2.2\n  - Tor Network: No\n  - API Latency: 842 ms";
        let result = check(legacy).unwrap();
        assert_eq!(result.verification, Verification::Inconclusive);
        assert_eq!(result.latency_ms, Some(842));
        let report = "[TTP] Tor verification: confirmed; reason: tor.valid_positive\n[TTP] Observed IP: 2001:db8::9";
        assert_eq!(check(report).unwrap().verification, Verification::Confirmed);
        assert_eq!(check(report).unwrap().latency_ms, None);
        assert!(start_confirmation("Session active but Tor verification failed.").is_none());
        assert_eq!(
            refreshed_ip(
                "[TTP] Circuit rotated but IP may not have changed yet. Current IP:\n2001:db8::9"
            ),
            (Some("2001:db8::9".parse().unwrap()), false)
        );
    }
}
