use std::path::Path;

use sonic_rs::JsonValueTrait;

use crate::adapter::{RestError, RestResult};
use crate::mock::{FixtureResponse, MockResponse};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestFixture {
    pub source: String,
    pub captured_at_ms: u64,
    pub capture_command: String,
    pub exchange_env: String,
    pub url: String,
    pub status: u16,
    pub body: String,
}

impl RestFixture {
    pub fn read(path: &Path) -> RestResult<Self> {
        let bytes = std::fs::read(path).map_err(|err| {
            RestError::internal(format!("failed to read fixture {}: {err}", path.display()))
        })?;
        let root = sonic_rs::from_slice::<sonic_rs::Value>(&bytes).map_err(RestError::from)?;
        let url = root
            .get("url")
            .and_then(|value| value.as_str())
            .ok_or_else(|| RestError::internal(format!("fixture {} missing url", path.display())))?
            .to_string();
        let status = root
            .get("status")
            .and_then(|value| value.as_u64())
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| {
                RestError::internal(format!("fixture {} missing status", path.display()))
            })?;
        let body = root
            .get("body")
            .ok_or_else(|| RestError::internal(format!("fixture {} missing body", path.display())))?;
        Ok(Self {
            source: root
                .get("source")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown")
                .to_string(),
            captured_at_ms: root
                .get("captured_at_ms")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
            capture_command: root
                .get("capture_command")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
            exchange_env: root
                .get("exchange_env")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
            url,
            status,
            body: decode_embedded_json_value(body, path)?,
        })
    }

    pub fn write(&self, path: &Path) -> RestResult<()> {
        let body = encode_embedded_json_value(self.body.as_str()).map_err(RestError::internal)?;
        let bytes = serde_json::to_vec_pretty(&serde_json::json!({
            "body": body,
            "capture_command": self.capture_command,
            "captured_at_ms": self.captured_at_ms,
            "exchange_env": self.exchange_env,
            "source": self.source,
            "status": self.status,
            "url": self.url,
        }))
        .map_err(|err| RestError::internal(format!("failed to encode fixture json: {err}")))?;
        std::fs::write(path, bytes).map_err(|err| {
            RestError::internal(format!("failed to write fixture {}: {err}", path.display()))
        })
    }

    pub fn into_response(self) -> FixtureResponse {
        FixtureResponse {
            url: self.url,
            response: MockResponse::text(self.status, self.body),
        }
    }
}

fn decode_embedded_json_value(value: &sonic_rs::Value, path: &Path) -> RestResult<String> {
    let mut body = if value.is_array() || value.is_object() {
        sonic_rs::to_string(value).map_err(RestError::from)?
    } else {
        value
            .as_str()
            .ok_or_else(|| {
                RestError::internal(format!(
                    "fixture {} body must be a JSON string, object, or array",
                    path.display()
                ))
            })?
            .to_string()
    };
    for _ in 0..4 {
        if let Ok(decoded) = sonic_rs::from_str::<sonic_rs::Value>(body.as_str()) {
            if decoded.is_array() || decoded.is_object() {
                break;
            }
            if let Some(inner) = decoded.as_str() {
                if inner == body {
                    break;
                }
                body = inner.to_string();
                continue;
            }
        }
        let wrapped = format!("\"{body}\"");
        let Ok(decoded) = sonic_rs::from_str::<String>(wrapped.as_str()) else {
            break;
        };
        if decoded == body {
            break;
        }
        body = decoded;
    }
    Ok(body)
}

fn encode_embedded_json_value(raw: &str) -> Result<serde_json::Value, String> {
    let normalized = decode_embedded_json_string(raw)?;
    let json = serde_json::from_str::<serde_json::Value>(normalized.as_str())
        .map_err(|err| err.to_string())?;
    match json {
        serde_json::Value::Object(_) | serde_json::Value::Array(_) => Ok(json),
        _ => Ok(serde_json::Value::String(normalized)),
    }
}

fn decode_embedded_json_string(raw: &str) -> Result<String, String> {
    let mut current = raw.to_string();
    for _ in 0..4 {
        match serde_json::from_str::<serde_json::Value>(current.as_str()) {
            Ok(serde_json::Value::Object(_)) | Ok(serde_json::Value::Array(_)) => return Ok(current),
            Ok(serde_json::Value::String(inner)) => {
                if inner == current {
                    return Ok(current);
                }
                current = inner;
            }
            Ok(_) => return Ok(current),
            Err(_) => return Ok(current),
        }
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::RestFixture;

    fn temp_fixture_path(name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("shared-restapi-{name}-{}.json", std::process::id()));
        path
    }

    #[test]
    fn rest_fixture_round_trips_nested_object_body() {
        let path = temp_fixture_path("object");
        let fixture = RestFixture {
            source: "live_capture".to_string(),
            captured_at_ms: 1,
            capture_command: "capture".to_string(),
            exchange_env: "deribit_testnet".to_string(),
            url: "https://example.invalid".to_string(),
            status: 200,
            body: "{\"jsonrpc\":\"2.0\",\"ok\":true}".to_string(),
        };
        fixture.write(path.as_path()).expect("fixture should write");
        let written = std::fs::read_to_string(path.as_path()).expect("fixture should read");
        assert!(written.contains("\n  \"body\": {\n    \"jsonrpc\": \"2.0\",\n    \"ok\": true\n  }"));
        let decoded = RestFixture::read(path.as_path()).expect("fixture should decode");
        assert_eq!(decoded.body, fixture.body);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rest_fixture_reads_double_encoded_body() {
        let path = temp_fixture_path("double");
        std::fs::write(
            path.as_path(),
            r#"{"url":"https://example.invalid","status":200,"body":"\"{\\\"jsonrpc\\\":\\\"2.0\\\"}\""}"#,
        )
        .expect("fixture should write");
        let decoded = RestFixture::read(path.as_path()).expect("fixture should decode");
        assert_eq!(decoded.body, r#"{"jsonrpc":"2.0"}"#);
        let _ = std::fs::remove_file(path);
    }
}
