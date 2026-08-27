#![forbid(unsafe_code)]

#[path = "../generated/rust/env.rs"]
mod env;
#[path = "../generated/rust/runtime.rs"]
mod env_runtime;

use std::{collections::HashMap, net::IpAddr, path::PathBuf, time::Duration};

use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use flags2env::BundledFlags2Env;
use futures_util::StreamExt;
use happy_wakey_interfaces::{
    Alarm, AlarmTransitionEvent, CreateAlarmRequest, TransitionAlarmRequest,
    TransitionAlarmResponse,
};
use reqwest::{redirect::Policy, Client, Response, StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

const CONFIG_NAME: &str = ".cli-flags.toml";
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_AUTH_RESPONSE_BYTES: usize = 64 * 1024;
const MAX_TOKEN_BYTES: usize = 16 * 1024;

const USAGE: &str = r#"happy-wakey [global options] <command>

Commands:
  capabilities
  verify
  alarms list
  alarms create --label LABEL --local-time HH:MM --time-zone ZONE --weekdays '[0,1,2,3,4]'
  occurrences transition --occurrence-id UUID --expected-generation N --event EVENT

Global options:
  --shared-auth-base HTTPS_URL
  --api-base HTTPS_URL
  --pretty

Credentials are environment-only: set HAPPY_WAKEY_ACCESS_TOKEN.
"#;

#[derive(Debug)]
struct Invocation {
    command: String,
    values: HashMap<String, String>,
    pretty: bool,
}

#[derive(Clone)]
struct HappyWakeyClient {
    http: Client,
    shared_auth_base: Url,
    api_base: Url,
}

#[derive(Debug, Deserialize, Serialize)]
struct Capabilities {
    mfa_enabled: bool,
    #[serde(default)]
    methods: Vec<String>,
    #[serde(default)]
    threefa_import_scheme: Option<String>,
    #[serde(default)]
    biometric_model: Option<String>,
}

impl HappyWakeyClient {
    fn new(shared_auth_base: &str, api_base: &str) -> Result<Self> {
        Ok(Self {
            http: Client::builder()
                .redirect(Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(15))
                .build()
                .context("build bounded HTTP client")?,
            shared_auth_base: validate_base_url(shared_auth_base, "Shared Auth")?,
            api_base: validate_base_url(api_base, "Happy Wakey API")?,
        })
    }

    async fn capabilities(&self) -> Result<Capabilities> {
        let response = self
            .http
            .get(self.shared_auth_base.join("auth/capabilities")?)
            .send()
            .await
            .context("Shared Auth capability request failed")?;
        decode_json(
            response,
            MAX_AUTH_RESPONSE_BYTES,
            "Shared Auth capabilities",
        )
        .await
    }

    async fn verify(&self, token: &str) -> Result<bool> {
        let response = self
            .http
            .get(self.shared_auth_base.join("auth/verify")?)
            .bearer_auth(token)
            .send()
            .await
            .context("Shared Auth verification request failed")?;
        match response.status() {
            StatusCode::OK => Ok(true),
            StatusCode::UNAUTHORIZED => Ok(false),
            status => bail!("Shared Auth verification returned status {status}"),
        }
    }

    async fn list_alarms(&self, token: &str) -> Result<Vec<Alarm>> {
        let response = self
            .http
            .get(self.api_base.join("v1/alarms")?)
            .bearer_auth(token)
            .send()
            .await
            .context("alarm list request failed")?;
        decode_json(response, MAX_RESPONSE_BYTES, "alarm list").await
    }

    async fn create_alarm(&self, token: &str, request: &CreateAlarmRequest) -> Result<Alarm> {
        let response = self
            .http
            .post(self.api_base.join("v1/alarms")?)
            .bearer_auth(token)
            .json(request)
            .send()
            .await
            .context("alarm creation request failed")?;
        decode_json(response, MAX_RESPONSE_BYTES, "alarm creation").await
    }

    async fn transition_occurrence(
        &self,
        token: &str,
        occurrence_id: &str,
        request: &TransitionAlarmRequest,
    ) -> Result<TransitionAlarmResponse> {
        validate_uuid(occurrence_id, "occurrence ID")?;
        let response = self
            .http
            .post(
                self.api_base
                    .join(&format!("v1/occurrences/{occurrence_id}/transitions"))?,
            )
            .bearer_auth(token)
            .json(request)
            .send()
            .await
            .context("occurrence transition request failed")?;
        decode_json(response, MAX_RESPONSE_BYTES, "occurrence transition").await
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let env_values = env_runtime::load_from_os();
    let _ = &env_values;
    let raw_args = std::env::args().collect::<Vec<_>>();
    if raw_args.len() == 1 || raw_args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{USAGE}");
        return Ok(());
    }
    if raw_args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("happy-wakey {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let invocation = parse_invocation(&raw_args)?;
    let shared_auth_base = required(&invocation.values, "HAPPY_WAKEY_SHARED_AUTH_BASE")?;
    let api_base = required(&invocation.values, "HAPPY_WAKEY_API_BASE")?;
    let client = HappyWakeyClient::new(shared_auth_base, api_base)?;

    let output = match invocation.command.as_str() {
        "capabilities" => serde_json::to_value(client.capabilities().await?)?,
        "verify" => {
            let token = access_token()?;
            let valid = client.verify(&token).await?;
            if !valid {
                bail!("credential is not valid for the configured Shared Auth realm");
            }
            json!({ "valid": true })
        }
        "alarms list" => {
            let token = access_token()?;
            serde_json::to_value(client.list_alarms(&token).await?)?
        }
        "alarms create" => {
            let token = access_token()?;
            let request = create_alarm_request(&invocation.values)?;
            serde_json::to_value(client.create_alarm(&token, &request).await?)?
        }
        "occurrences transition" => {
            let token = access_token()?;
            let (occurrence_id, request) = transition_request(&invocation.values)?;
            serde_json::to_value(
                client
                    .transition_occurrence(&token, occurrence_id, &request)
                    .await?,
            )?
        }
        _ => bail!("unknown or incomplete command; run happy-wakey --help"),
    };
    print_json(&output, invocation.pretty)?;
    Ok(())
}

fn parse_invocation(argv: &[String]) -> Result<Invocation> {
    let config_path = find_config()?;
    let parser = BundledFlags2Env::new();
    parser
        .audit_config(config_path.to_str())
        .map_err(|_| anyhow!("flags2env rejected the CLI schema"))?;
    let parsed = parser
        .parse_structured(argv, config_path.to_str())
        .map_err(|_| anyhow!("flags2env could not parse the CLI arguments"))?;
    if !parsed.unknown_options.is_empty() || !parsed.errors.is_empty() {
        bail!(
            "flags2env rejected {} unknown option(s) and {} invalid value(s)",
            parsed.unknown_options.len(),
            parsed.errors.len()
        );
    }
    let pretty = parsed
        .flags
        .get("HAPPY_WAKEY_PRETTY")
        .is_some_and(|value| value == "true");
    Ok(Invocation {
        command: parsed.command,
        values: parsed.flags,
        pretty,
    })
}

fn create_alarm_request(values: &HashMap<String, String>) -> Result<CreateAlarmRequest> {
    let transition_id = values
        .get("HAPPY_WAKEY_TRANSITION_ID")
        .cloned()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    validate_uuid(&transition_id, "transition ID")?;
    let label = bounded_text(required(values, "HAPPY_WAKEY_ALARM_LABEL")?, 120, "label")?;
    let local_time = required(values, "HAPPY_WAKEY_ALARM_LOCAL_TIME")?.to_owned();
    if !valid_local_time(&local_time) {
        bail!("local time must be HH:MM or HH:MM:SS");
    }
    let time_zone = bounded_text(
        required(values, "HAPPY_WAKEY_ALARM_TIME_ZONE")?,
        64,
        "time zone",
    )?;
    if time_zone.len() < 3 {
        bail!("time zone must contain at least three characters");
    }
    let mut weekdays: Vec<u8> = parse_json_array(values, "HAPPY_WAKEY_ALARM_WEEKDAYS")?;
    weekdays.sort_unstable();
    weekdays.dedup();
    if weekdays.is_empty() || weekdays.len() > 7 || weekdays.iter().any(|day| *day > 6) {
        bail!("weekdays must contain one to seven unique values from 0 through 6");
    }
    let enabled = parse_bool(values, "HAPPY_WAKEY_ALARM_ENABLED")?;
    let sound = bounded_text(required(values, "HAPPY_WAKEY_ALARM_SOUND")?, 128, "sound")?;
    let volume = parse_number::<f32>(values, "HAPPY_WAKEY_ALARM_VOLUME")?;
    if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
        bail!("volume must be between 0 and 1");
    }
    let gradual_seconds = parse_number::<u32>(values, "HAPPY_WAKEY_ALARM_GRADUAL_SECONDS")?;
    if gradual_seconds > 1800 {
        bail!("gradual seconds must not exceed 1800");
    }
    let mut tags: Vec<String> = parse_json_array(values, "HAPPY_WAKEY_ALARM_TAGS")?;
    for tag in &mut tags {
        *tag = bounded_text(tag, 40, "tag")?;
    }
    tags.sort();
    tags.dedup();
    if tags.len() > 20 {
        bail!("at most 20 unique tags are allowed");
    }
    Ok(CreateAlarmRequest {
        transition_id,
        label,
        local_time,
        time_zone,
        weekdays,
        enabled,
        sound,
        volume,
        gradual_seconds,
        tags,
    })
}

fn transition_request(values: &HashMap<String, String>) -> Result<(&str, TransitionAlarmRequest)> {
    let occurrence_id = required(values, "HAPPY_WAKEY_OCCURRENCE_ID")?;
    validate_uuid(occurrence_id, "occurrence ID")?;
    let transition_id = values
        .get("HAPPY_WAKEY_TRANSITION_ID")
        .cloned()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    validate_uuid(&transition_id, "transition ID")?;
    let expected_generation = parse_number::<u64>(values, "HAPPY_WAKEY_EXPECTED_GENERATION")?;
    let event = match required(values, "HAPPY_WAKEY_TRANSITION_EVENT")? {
        "fire" => AlarmTransitionEvent::Fire,
        "acknowledge" => AlarmTransitionEvent::Acknowledge,
        "snooze" => AlarmTransitionEvent::Snooze,
        "complete" => AlarmTransitionEvent::Complete,
        "mark_missed" => AlarmTransitionEvent::MarkMissed,
        "cancel" => AlarmTransitionEvent::Cancel,
        _ => bail!("event must be fire, acknowledge, snooze, complete, mark_missed, or cancel"),
    };
    let snooze_until = values.get("HAPPY_WAKEY_SNOOZE_UNTIL").cloned();
    if event == AlarmTransitionEvent::Snooze && snooze_until.is_none() {
        bail!("snooze requires --snooze-until");
    }
    if event != AlarmTransitionEvent::Snooze && snooze_until.is_some() {
        bail!("--snooze-until is valid only for the snooze event");
    }
    let client_time = values
        .get("HAPPY_WAKEY_CLIENT_TIME")
        .cloned()
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    Ok((
        occurrence_id,
        TransitionAlarmRequest {
            transition_id,
            expected_generation,
            event,
            snooze_until,
            client_time,
        },
    ))
}

fn find_config() -> Result<PathBuf> {
    if let Some(explicit) = std::env::var_os("FLAGS2ENV_CONFIG") {
        let path = PathBuf::from(explicit);
        if path.is_file() {
            return Ok(path);
        }
        bail!("FLAGS2ENV_CONFIG does not name a readable file");
    }
    let mut candidates = Vec::new();
    if let Ok(current) = std::env::current_dir() {
        candidates.push(current.join(CONFIG_NAME));
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join(CONFIG_NAME));
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(CONFIG_NAME));
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .context(".cli-flags.toml was not found beside the executable or in the working directory")
}

async fn decode_json<T: DeserializeOwned>(
    response: Response,
    limit: usize,
    operation: &str,
) -> Result<T> {
    let status = response.status();
    if !status.is_success() {
        bail!("{operation} returned status {status}");
    }
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        bail!("{operation} response exceeded its byte limit");
    }
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.with_context(|| format!("{operation} response stream failed"))?;
        if body.len().saturating_add(chunk.len()) > limit {
            bail!("{operation} response exceeded its byte limit");
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).with_context(|| format!("{operation} response was malformed"))
}

fn validate_base_url(raw: &str, label: &str) -> Result<Url> {
    let url = Url::parse(raw).with_context(|| format!("{label} base URL is invalid"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        bail!("{label} base URL must be a credential-free HTTPS authority");
    }
    if public_numeric_host(url.host_str()) {
        bail!("{label} base URL must use a DNS name, not a public IP");
    }
    Ok(url)
}

fn public_numeric_host(host: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    if matches!(host, "127.0.0.1" | "localhost" | "::1") {
        return false;
    }
    host.parse::<IpAddr>().is_ok()
}

fn access_token() -> Result<String> {
    let token = std::env::var("HAPPY_WAKEY_ACCESS_TOKEN")
        .context("HAPPY_WAKEY_ACCESS_TOKEN is required")?;
    if token.is_empty()
        || token.len() > MAX_TOKEN_BYTES
        || token
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        bail!("HAPPY_WAKEY_ACCESS_TOKEN is malformed");
    }
    Ok(token)
}

fn required<'a>(values: &'a HashMap<String, String>, key: &str) -> Result<&'a str> {
    values
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("required setting {key} is missing"))
}

fn parse_bool(values: &HashMap<String, String>, key: &str) -> Result<bool> {
    match required(values, key)? {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => bail!("{key} must be a boolean"),
    }
}

fn parse_number<T>(values: &HashMap<String, String>, key: &str) -> Result<T>
where
    T: std::str::FromStr,
{
    required(values, key)?
        .parse()
        .map_err(|_| anyhow!("{key} has an invalid numeric value"))
}

fn parse_json_array<T: DeserializeOwned>(
    values: &HashMap<String, String>,
    key: &str,
) -> Result<Vec<T>> {
    serde_json::from_str(required(values, key)?).map_err(|_| anyhow!("{key} must be a JSON array"))
}

fn bounded_text(value: &str, max: usize, label: &str) -> Result<String> {
    if value.is_empty()
        || value.len() > max
        || value.chars().any(|character| character.is_control())
    {
        bail!("{label} is empty, too long, or contains controls");
    }
    Ok(value.to_owned())
}

fn valid_local_time(value: &str) -> bool {
    let parts = value.split(':').collect::<Vec<_>>();
    if !matches!(parts.len(), 2 | 3) || parts.iter().any(|part| part.len() != 2) {
        return false;
    }
    let parsed = parts
        .iter()
        .map(|part| part.parse::<u8>())
        .collect::<std::result::Result<Vec<_>, _>>();
    parsed.is_ok_and(|parts| {
        parts[0] <= 23 && parts[1] <= 59 && parts.get(2).is_none_or(|second| *second <= 59)
    })
}

fn validate_uuid(value: &str, label: &str) -> Result<()> {
    Uuid::parse_str(value)
        .map(|_| ())
        .with_context(|| format!("{label} must be an RFC 4122 UUID"))
}

fn print_json(value: &Value, pretty: bool) -> Result<()> {
    if pretty {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{}", serde_json::to_string(value)?);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn flags2env_resolves_nested_alarm_command() {
        let invocation = parse_invocation(&strings(&[
            "happy-wakey",
            "alarms",
            "create",
            "--label",
            "Weekday",
            "--local-time",
            "07:30",
            "--time-zone",
            "America/Chicago",
            "--weekdays",
            "[1,2,3,4,5]",
        ]))
        .unwrap();
        assert_eq!(invocation.command, "alarms create");
        let request = create_alarm_request(&invocation.values).unwrap();
        assert_eq!(request.weekdays, [1, 2, 3, 4, 5]);
        assert_eq!(request.label, "Weekday");
    }

    #[test]
    fn secret_shaped_option_fails_without_echoing_value() {
        let error = parse_invocation(&strings(&[
            "happy-wakey",
            "verify",
            "--access-token=do-not-print-this",
        ]))
        .unwrap_err()
        .to_string();
        assert!(error.contains("1 unknown option"));
        assert!(!error.contains("do-not-print-this"));
    }

    #[test]
    fn url_and_token_boundaries_are_strict() {
        assert!(validate_base_url("http://api.example.test", "API").is_err());
        assert!(validate_base_url("https://user@api.example.test", "API").is_err());
        assert!(validate_base_url("https://api.example.test/path", "API").is_err());
        assert!(validate_base_url("https://api.example.test", "API").is_ok());
        assert!(validate_base_url("https://98.90.186.114", "API").is_err());
        assert!(validate_base_url("https://[2001:db8::1]/", "API").is_err());
    }

    #[test]
    fn local_time_parser_is_total_and_bounded() {
        assert!(valid_local_time("07:30"));
        assert!(valid_local_time("23:59:59"));
        assert!(!valid_local_time("24:00"));
        assert!(!valid_local_time("7:30"));
        assert!(!valid_local_time("noon"));
    }

    #[test]
    fn snooze_is_the_only_transition_with_a_deadline() {
        let mut values = HashMap::from([
            (
                "HAPPY_WAKEY_OCCURRENCE_ID".into(),
                Uuid::new_v4().to_string(),
            ),
            ("HAPPY_WAKEY_EXPECTED_GENERATION".into(), "4".into()),
            ("HAPPY_WAKEY_TRANSITION_EVENT".into(), "snooze".into()),
        ]);
        assert!(transition_request(&values).is_err());
        values.insert(
            "HAPPY_WAKEY_SNOOZE_UNTIL".into(),
            "2026-08-25T12:15:00Z".into(),
        );
        assert!(transition_request(&values).is_ok());
        values.insert("HAPPY_WAKEY_TRANSITION_EVENT".into(), "complete".into());
        assert!(transition_request(&values).is_err());
    }
}
