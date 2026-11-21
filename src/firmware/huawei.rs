use worker::{Fetch, Headers, Method, Request, RequestInit, Result};

use super::{DeviceCredentials, QueryEndpoint};
use crate::domain::OtaFields;

const HUAWEI_OTAUPDATE_ENDPOINT: &str = "https://query.hicloud.com/BL/v1/VersionCheck";
const HUAWEI_ERECOVERY_ENDPOINT: &str = "https://query.hicloud.com/BL/v1/CheckAndAuthorize";

pub(crate) async fn version_check(
    f: &OtaFields<'_>,
    credentials: &DeviceCredentials,
    endpoint: QueryEndpoint,
) -> Result<(u16, String)> {
    let mut rules = vec![
        serde_json::json!({ "versionNumber": f.base,    "versionPackageType": "2" }),
        serde_json::json!({ "versionNumber": f.cust,    "versionPackageType": "3" }),
        serde_json::json!({ "versionNumber": f.preload, "versionPackageType": "4" }),
    ];
    if let Some(patch) = f.patch {
        rules.push(serde_json::json!({ "versionNumber": patch, "versionPackageType": "8" }));
    }
    let mut body = serde_json::json!({
        "commonRules": {
            "boardID": "9465",
            "checkMode": "NORMAL",
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
        "deviceToken": "-",
        "updateToken": "-",
        "cotaInfo": {
            "country": "DEFAULT",
            "vendorCota": "DEFAULT",
            "vendorExpiredTime": "0"
        },
        "deviceCertificate": credentials.certificate,
        "deviceInfo": {
            "deviceId": credentials.device_id,
            "udid": ""
        },
        "extraInfo": "",
        "keyAttestation": credentials.key_attestation,
        "ver": "2",
        "versionPackageRules": rules
    });
    if endpoint == QueryEndpoint::OtaUpdate {
        body["commonRules"]["checkType"] = "1".into();
    }
    let body = body.to_string();
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
    let url = match endpoint {
        QueryEndpoint::OtaUpdate => HUAWEI_OTAUPDATE_ENDPOINT,
        QueryEndpoint::ERecovery => HUAWEI_ERECOVERY_ENDPOINT,
    };
    let request = Request::new_with_init(url, &RequestInit {
        method: Method::Post, headers,
        body: Some(wasm_bindgen::JsValue::from_str(&body)),
        ..Default::default()
    })?;
    let mut response = Fetch::Request(request).send().await?;
    Ok((response.status_code(), response.text().await?))
}
