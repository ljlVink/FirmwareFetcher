use worker::{console_error, Env, Error, Result, Url};

use crate::domain::{CheckFailure, CheckedFirmware, FirmwareInput, OtaFields, UpdateAction, VersionCheckResult};

pub(crate) mod fingerprint;
mod honor;
mod huawei;
pub(crate) mod manual;
mod remote_zip;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueryEndpoint {
    OtaUpdate,
    ERecovery,
}

pub(crate) struct DeviceCredentials {
    device_id: String,
    certificate: String,
    key_attestation: String,
}

impl DeviceCredentials {
    fn from_env(env: &Env, prefix: &str) -> Result<Self> {
        let binding = |name: &str| -> Result<String> {
            let key = format!("{prefix}_{name}");
            let value = env.secret(&key)
                .map_err(|_| Error::RustError(format!("{key} is not configured")))?
                .to_string();
            let value = value.trim();
            if value.is_empty() {
                return Err(Error::RustError(format!("{key} is empty")));
            }
            Ok(value.to_owned())
        };
        Ok(Self {
            device_id: binding("DEVICE_ID")?,
            certificate: binding("DEV_CERT")?,
            key_attestation: binding("KEY_ATT")?,
        })
    }
}

fn filelist_url(raw: &str) -> Result<String> {
    let mut url = Url::parse(raw)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(Error::RustError("invalid firmware URL".into()));
    }
    let path = url.path().trim_end_matches('/');
    if !path.ends_with("/full/filelist.xml") {
        let path = format!("{path}/full/filelist.xml");
        url.set_path(&path);
    }
    Ok(url.into())
}

pub(crate) async fn query(
    env: &Env,
    input: &FirmwareInput,
    endpoint: QueryEndpoint,
) -> std::result::Result<CheckedFirmware, CheckFailure> {
    let failure = |status, message: String| CheckFailure { status, message };
    let fields = OtaFields {
        dev: &input.dev, base: &input.base, cust: &input.cust,
        preload: &input.preload, patch: input.patch.as_deref(), vendor: &input.vendor,
        update_action: input.upgrade_or_erec,
    };
    let honor = input.brand == "honor";
    if honor && endpoint == QueryEndpoint::ERecovery {
        return Err(failure(400, "eRecovery is only available for Huawei devices".into()));
    }
    let brand_name = if honor { "Honor" } else { "Huawei" };
    let credentials = match DeviceCredentials::from_env(env, if honor { "HONOR" } else { "HUAWEI" }) {
        Ok(credentials) => credentials,
        Err(error) => {
            console_error!("{brand_name} device credentials unavailable: {error}");
            return Err(failure(500, format!("Server has no {brand_name} device credentials configured; record not saved")));
        }
    };
    let response = if honor {
        honor::version_check(&fields, &credentials).await
    } else {
        huawei::version_check(&fields, &credentials, endpoint).await
    };
    let (status, body) = match response {
        Ok(response) => response,
        Err(_) => return Err(failure(502, format!("Unable to reach {brand_name} server; record not saved"))),
    };
    if status != 200 {
        return Err(failure(502, format!("{brand_name} server verification failed; record not saved")));
    }
    let parsed = if honor {
        honor::parse_response(&body)
    } else {
        serde_json::from_str(&body)
    };
    let mut result: VersionCheckResult = match parsed {
        Ok(result) => result,
        Err(error) => {
            if endpoint == QueryEndpoint::ERecovery {
                return Err(failure(502, "Huawei eRecovery returned an unexpected format; record not saved".into()));
            }
            let excerpt: String = body.chars().take(160).collect();
            return Err(failure(502, format!("{brand_name} server returned an unexpected format ({error}): {excerpt}")));
        }
    };
    if result.status != "0" || result.packages.is_empty() {
        let kind = match input.upgrade_or_erec {
            UpdateAction::Upgrade => "incremental package",
            UpdateAction::Recovery => "full package",
            UpdateAction::Rollback => "rollback package",
        };
        if !honor && result.status == "1" && input.upgrade_or_erec != UpdateAction::Rollback {
            return Err(failure(422,
                "Huawei returned no available firmware (status=1): this package may still be in Huawei Club beta; try again in a few days, it becomes downloadable once officially released".into(),
            ));
        }
        return Err(failure(422, format!(
            "{brand_name} returned no available firmware (status={}): no {} is available for this version combination; record not saved",
            result.status, kind
        )));
    }
    let target = match result.matches.iter().filter_map(|entry| entry.info.as_ref())
        .filter_map(|info| info.target.as_ref()).find(|target| !target.trim().is_empty()) {
        Some(target) => target.clone(),
        None => return Err(failure(502, format!("{brand_name} server returned no target version; record not saved"))),
    };
    for package in &mut result.packages {
        if package.version_number.trim().is_empty() {
            return Err(failure(502, format!("{brand_name} server returned an incomplete firmware version; record not saved")));
        }
        package.url = match filelist_url(&package.url) {
            Ok(url) => url,
            Err(_) => return Err(failure(502, format!("{brand_name} server returned an invalid firmware URL; record not saved"))),
        };
    }
    Ok(CheckedFirmware { target, packages: result.packages, matches: result.matches })
}
