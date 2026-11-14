use worker::{Fetch, Headers, Method, Request, RequestInit, Result};

use super::DeviceCredentials;
use crate::domain::OtaFields;

const HUAWEI_ENDPOINT: &str = "https://update-drcn.platform.hicloud.com/BL/v1/VersionCheck";

pub(crate) async fn version_check(f: &OtaFields<'_>, credentials: &DeviceCredentials) -> Result<(u16, String)> {
    let mut rules = vec![
        serde_json::json!({ "versionNumber": f.base,    "versionPackageType": "2" }),
        serde_json::json!({ "versionNumber": f.cust,    "versionPackageType": "3" }),
        serde_json::json!({ "versionNumber": f.preload, "versionPackageType": "4" }),
    ];
    if let Some(patch) = f.patch {
        rules.push(serde_json::json!({ "versionNumber": patch, "versionPackageType": "8" }));
    }
    let body = serde_json::json!({
        "commonRules": {
            "boardID": "9465",
            "checkMode": "NORMAL",
            "checkType": "1",
            "clientType": "HwOUC",
            "clientVersion": "e.2.0",
            "collAbility": "1",
            "customType": "",
            "dVersion": "D000",
            "devModel": f.dev,
            "deviceType": "PHONE",
            "fingerPrint": "",
            "language": "zh-cn",
            "os": "",
            "plmn": "46001",
            "saleInfo": "",
            "securityPatchLevel": "",
            "softwarePlatform": "",
            "subGroup": "",
            "updateAction": f.update_action.as_str(),
            "usageInfo": "",
            "vendorCountry": f.vendor,
            "verGroup": "",
            "verifyInfo": ""
        },
        "authInfo": {
            "pkiAuthInfo": {
                "deviceCertificate": credentials.certificate,
                "keyAttestation": credentials.key_attestation
            },
            "authType": "1"
        },
        "deviceCertificate": credentials.certificate,
        "deviceInfo": {
            "deviceId": credentials.device_id,
            "udid": ""
        },
        "extraInfo": "",
        "keyAttestation": credentials.key_attestation,
        "versionPackageRules": rules
    }).to_string();
    let headers = Headers::new();
    headers.set("Content-Type", "application/json; charset=UTF-8")?;
    headers.set("x-devModel", f.dev)?;
    headers.set("x-requestId", "-")?;
    headers.set("x-deviceDescId", &credentials.device_id)?;
    headers.set("x-productId", "SmartPhone")?;
    headers.set(
        "User-Agent",
        "Dalvik/2.1.0 (Linux; U; Android 12; TAS-AN00 Build/HUAWEITAS-AN00)",
    )?;
    let request = Request::new_with_init(HUAWEI_ENDPOINT, &RequestInit {
        method: Method::Post, headers,
        body: Some(wasm_bindgen::JsValue::from_str(&body)),
        ..Default::default()
    })?;
    let mut response = Fetch::Request(request).send().await?;
    Ok((response.status_code(), response.text().await?))
}
