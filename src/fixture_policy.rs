use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Method;

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
}
