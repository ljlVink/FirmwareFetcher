use serde::Deserialize;
use worker::{Fetch, Headers, Method, Request, RequestInit, Result};

use super::DeviceCredentials;
use crate::domain::{FirmwarePackage, OtaFields, VersionCheckResult, VersionMatch};

const HONOR_ENDPOINT: &str = "https://update.platform.hihonorcloud.com/blversion/v1/version/check";

pub(crate) async fn version_check(f: &OtaFields<'_>, credentials: &DeviceCredentials) -> Result<(u16, String)> {
    let mut rules = vec![
        serde_json::json!({ "versionNumber": f.base, "versionPackageType": 2 }),
        serde_json::json!({ "versionNumber": f.cust, "versionPackageType": 3 }),
        serde_json::json!({ "versionNumber": f.preload, "versionPackageType": 4 }),
    ];
    if let Some(patch) = f.patch {
        rules.push(serde_json::json!({ "versionNumber": patch, "versionPackageType": 8 }));
    }
    let body = serde_json::json!({
        "commonRules": {
            "devModel": f.dev, "plmn": "-", "subGroup": "", "updateAction": f.update_action.as_str(),
            "vendorCountry": f.vendor, "verGroup": ""
        },
        "cotaInfo": { "country": "DEFAULT", "vendorCota": "", "vendorExpiredTime": "" },
        "deviceCertificate": credentials.certificate,
        "deviceInfo": { "deviceId": credentials.device_id },
        "keyAttestation": credentials.key_attestation,
        "versionPackageRules": rules
    }).to_string();
    let headers = Headers::new();
    headers.set("Content-Type", "application/json;charset=UTF-8")?;
    headers.set("Accept", "*/*")?;
    headers.set("Cache-Control", "no-cache")?;
    headers.set("Pragma", "no-cache")?;
    headers.set("x-requestId", "-")?;
    let request = Request::new_with_init(HONOR_ENDPOINT, &RequestInit {
        method: Method::Post, headers,
        body: Some(wasm_bindgen::JsValue::from_str(&body)),
        ..Default::default()
    })?;
    let mut response = Fetch::Request(request).send().await?;
    Ok((response.status_code(), response.text().await?))
}

#[derive(Deserialize)]
struct HonorResponse {
    status: u32,
    result: Option<HonorResult>,
}

#[derive(Deserialize)]
struct HonorResult {
    #[serde(rename = "checkResult")]
    check_result: u32,
    #[serde(rename = "versionList", default)]
    packages: Vec<FirmwarePackage>,
    #[serde(rename = "blVersionCheckResults", default)]
    matches: Vec<VersionMatch>,
}

pub(crate) fn parse_response(body: &str) -> std::result::Result<VersionCheckResult, serde_json::Error> {
    let response: HonorResponse = serde_json::from_str(body)?;
    let mut result = VersionCheckResult {
        status: response.status.to_string(), packages: Vec::new(), matches: Vec::new(),
    };
    if response.status == 0 {
        if let Some(mut inner) = response.result {
            result.status = inner.check_result.to_string();
            // Numeric versions describe changelog.xml, not downloadable firmware.
            inner.packages.retain(|package| package.version_number.is_empty()
                || !package.version_number.bytes().all(|b| b.is_ascii_digit()));
            result.packages = inner.packages;
            result.matches = inner.matches;
        }
    }
    Ok(result)
}
