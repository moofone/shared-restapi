use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use sonic_rs::JsonValueTrait;

use crate::{RestError, RestRequest, RestResult};

const SHARED_RESTAPI_FIXTURE_CAPTURE_MODE_ENV: &str = "SHARED_RESTAPI_FIXTURE_CAPTURE_MODE";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestFixtureRequirement {
    pub contract_id: String,
    pub success_path: PathBuf,
    pub error_path: PathBuf,
}

#[derive(Default)]
struct RestFixtureRegistry {
    requirements: Vec<RestFixtureRequirement>,
}

fn registry() -> &'static Mutex<RestFixtureRegistry> {
    static REGISTRY: OnceLock<Mutex<RestFixtureRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(RestFixtureRegistry::default()))
}

pub fn register_required_rest_contracts(
    requirements: impl IntoIterator<Item = RestFixtureRequirement>,
) {
    let mut guard = registry().lock().expect("rest fixture registry poisoned");
    guard.requirements = requirements.into_iter().collect();
}

pub fn required_rest_contracts() -> Vec<RestFixtureRequirement> {
    registry()
        .lock()
        .expect("rest fixture registry poisoned")
        .requirements
        .clone()
}

pub fn clear_required_rest_contracts_for_tests() {
    registry()
        .lock()
        .expect("rest fixture registry poisoned")
        .requirements
        .clear();
}

pub fn fixture_capture_mode_enabled() -> bool {
    std::env::var(SHARED_RESTAPI_FIXTURE_CAPTURE_MODE_ENV)
        .ok()
        .map(|raw| matches!(raw.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn ensure_live_capture_fixture(path: &PathBuf) -> RestResult<()> {
    let bytes = std::fs::read(path).map_err(|err| {
        RestError::internal(format!(
            "live REST request blocked: failed to read fixture {}: {err}",
            path.display()
        ))
    })?;
    let root = sonic_rs::from_slice::<sonic_rs::Value>(&bytes).map_err(|err| {
        RestError::internal(format!(
            "live REST request blocked: failed to parse fixture {}: {err}",
            path.display()
        ))
    })?;
    let source = root.get("source").and_then(|value| value.as_str()).unwrap_or("");
    let captured_at_ms = root
        .get("captured_at_ms")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let capture_command = root
        .get("capture_command")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let exchange_env = root
        .get("exchange_env")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if source != "live_capture"
        || captured_at_ms == 0
        || capture_command.trim().is_empty()
        || exchange_env.trim().is_empty()
    {
        return Err(RestError::internal(format!(
            "live REST request blocked: fixture {} is not compliant live-capture provenance",
            path.display()
        )));
    }
    Ok(())
}

pub fn ensure_live_request_allowed(request: &RestRequest) -> RestResult<()> {
    if fixture_capture_mode_enabled() {
        return Ok(());
    }
    let contract_id = request.fixture_contract.as_deref().ok_or_else(|| {
        RestError::internal("live REST request missing required fixture contract metadata")
    })?;
    let guard = registry().lock().expect("rest fixture registry poisoned");
    if guard.requirements.is_empty() {
        return Err(RestError::internal(
            "live REST request blocked: no required fixture contracts registered",
        ));
    }
    let Some(requirement) = guard
        .requirements
        .iter()
        .find(|item| item.contract_id == contract_id)
    else {
        return Err(RestError::internal(format!(
            "live REST request blocked: unregistered fixture contract {contract_id}"
        )));
    };
    if !requirement.success_path.exists() || !requirement.error_path.exists() {
        return Err(RestError::internal(format!(
            "live REST request blocked: missing fixture files for contract={} success={} error={}",
            requirement.contract_id,
            requirement.success_path.display(),
            requirement.error_path.display()
        )));
    }
    ensure_live_capture_fixture(&requirement.success_path)?;
    ensure_live_capture_fixture(&requirement.error_path)?;
    Ok(())
}

pub fn validate_required_rest_contracts(
    requirements: &[RestFixtureRequirement],
) -> RestResult<()> {
    if requirements.is_empty() {
        return Err(RestError::internal(
            "required REST fixture validation failed: no fixture contracts registered",
        ));
    }

    for requirement in requirements {
        if !requirement.success_path.exists() || !requirement.error_path.exists() {
            return Err(RestError::internal(format!(
                "required REST fixture validation failed: missing fixture files for contract={} success={} error={}",
                requirement.contract_id,
                requirement.success_path.display(),
                requirement.error_path.display()
            )));
        }
        ensure_live_capture_fixture(&requirement.success_path)?;
        ensure_live_capture_fixture(&requirement.error_path)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Method;
    use std::path::Path;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "shared-restapi-fixture-policy-{}-{}",
            std::process::id(),
            name
        ))
    }

    fn write_fixture(path: &Path, source: &str) {
        let body = sonic_rs::to_vec(&sonic_rs::json!({
            "source": source,
            "captured_at_ms": if source == "live_capture" { 1_u64 } else { 0_u64 },
            "capture_command": if source == "live_capture" { "capture" } else { "" },
            "exchange_env": if source == "live_capture" { "deribit_testnet" } else { "" },
            "url": "https://example.invalid",
            "status": 200,
            "body": "{}"
        }))
        .expect("serialize fixture");
        std::fs::write(path, body).expect("write fixture");
    }

    #[test]
    fn live_request_requires_registered_contract_and_files() {
        clear_required_rest_contracts_for_tests();
        let request = RestRequest::new(Method::GET, "https://example.invalid")
            .with_fixture_contract("contract-a");
        let err = ensure_live_request_allowed(&request).expect_err("missing registry should fail");
        assert!(err.to_string().contains("no required fixture contracts registered"));
    }

    #[test]
    fn live_request_requires_fixture_contract_metadata() {
        clear_required_rest_contracts_for_tests();
        let request = RestRequest::new(Method::GET, "https://example.invalid");
        let err =
            ensure_live_request_allowed(&request).expect_err("missing fixture metadata should fail");
        assert!(err.to_string().contains("missing required fixture contract metadata"));
    }

    #[test]
    fn live_request_rejects_non_live_capture_fixtures() {
        clear_required_rest_contracts_for_tests();
        let success = temp_path("success.json");
        let error = temp_path("error.json");
        let _ = std::fs::remove_file(&success);
        let _ = std::fs::remove_file(&error);
        write_fixture(success.as_path(), "synthesized");
        write_fixture(error.as_path(), "live_capture");
        register_required_rest_contracts([RestFixtureRequirement {
            contract_id: "contract-a".to_string(),
            success_path: success.clone(),
            error_path: error.clone(),
        }]);
        let request = RestRequest::new(Method::GET, "https://example.invalid")
            .with_fixture_contract("contract-a");
        let err =
            ensure_live_request_allowed(&request).expect_err("non-live provenance should fail");
        assert!(err.to_string().contains("not compliant live-capture provenance"));
        let _ = std::fs::remove_file(success);
        let _ = std::fs::remove_file(error);
    }

    #[test]
    fn live_request_accepts_live_capture_fixtures() {
        clear_required_rest_contracts_for_tests();
        let success = temp_path("good-success.json");
        let error = temp_path("good-error.json");
        let _ = std::fs::remove_file(&success);
        let _ = std::fs::remove_file(&error);
        write_fixture(success.as_path(), "live_capture");
        write_fixture(error.as_path(), "live_capture");
        register_required_rest_contracts([RestFixtureRequirement {
            contract_id: "contract-a".to_string(),
            success_path: success.clone(),
            error_path: error.clone(),
        }]);
        let request = RestRequest::new(Method::GET, "https://example.invalid")
            .with_fixture_contract("contract-a");
        ensure_live_request_allowed(&request).expect("live-captured fixtures should pass");
        let _ = std::fs::remove_file(success);
        let _ = std::fs::remove_file(error);
    }

    #[test]
    fn validator_requires_registered_contracts() {
        let err = validate_required_rest_contracts(&[])
            .expect_err("empty requirement list should fail validation");
        assert!(err.to_string().contains("no fixture contracts registered"));
    }

    #[test]
    fn validator_rejects_missing_fixture_files() {
        let success = temp_path("validator-missing-success.json");
        let error = temp_path("validator-missing-error.json");
        let _ = std::fs::remove_file(&success);
        let _ = std::fs::remove_file(&error);

        let err = validate_required_rest_contracts(&[RestFixtureRequirement {
            contract_id: "contract-a".to_string(),
            success_path: success.clone(),
            error_path: error.clone(),
        }])
        .expect_err("missing files should fail validation");
        assert!(err.to_string().contains("missing fixture files"));
    }

    #[test]
    fn validator_accepts_live_capture_fixtures() {
        let success = temp_path("validator-good-success.json");
        let error = temp_path("validator-good-error.json");
        let _ = std::fs::remove_file(&success);
        let _ = std::fs::remove_file(&error);
        write_fixture(success.as_path(), "live_capture");
        write_fixture(error.as_path(), "live_capture");

        validate_required_rest_contracts(&[RestFixtureRequirement {
            contract_id: "contract-a".to_string(),
            success_path: success.clone(),
            error_path: error.clone(),
        }])
        .expect("live-captured fixtures should pass validation");

        let _ = std::fs::remove_file(success);
        let _ = std::fs::remove_file(error);
    }
}
