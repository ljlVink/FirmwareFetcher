use worker::{console_log, Env};

use crate::domain::{FirmwareInput, FirmwarePackage, LegacyFirmware, SubmissionSource};
use crate::firmware::fingerprint;
use crate::storage::Repository;

pub(crate) enum SaveError {
    Fingerprint(String),
    Storage(worker::Error),
}

impl From<worker::Error> for SaveError {
    fn from(error: worker::Error) -> Self {
        Self::Storage(error)
    }
}

pub(crate) async fn save(
    env: &Env,
    input: &FirmwareInput,
    target: &str,
    packages: &[FirmwarePackage],
    source: SubmissionSource,
) -> Result<Option<String>, SaveError> {
    let key = fingerprint::content_key(input.brand, packages)
        .await
        .map_err(SaveError::Fingerprint)?;
    let repository = Repository::connect(env).await?;
    save_with_key(&repository, input, target, packages, &key, source).await
}

pub(crate) async fn save_with_repository(
    repository: &Repository,
    input: &FirmwareInput,
    target: &str,
    packages: &[FirmwarePackage],
    source: SubmissionSource,
) -> Result<Option<String>, SaveError> {
    let key = fingerprint::content_key(input.brand, packages)
        .await
        .map_err(SaveError::Fingerprint)?;
    save_with_key(repository, input, target, packages, &key, source).await
}

async fn save_with_key(
    repository: &Repository,
    input: &FirmwareInput,
    target: &str,
    packages: &[FirmwarePackage],
    key: &str,
    source: SubmissionSource,
) -> Result<Option<String>, SaveError> {
    let attempt = repository.try_insert_firmware(input, target, packages, key, source).await?;
    if attempt.legacy.is_empty() {
        return Ok(attempt.saved.map(|value| value.to_string()));
    }
    baseline_legacy(repository, input.brand, attempt.legacy).await?;
    Ok(repository.insert_firmware_after_baseline(input, target, packages, key, source).await?)
}

async fn baseline_legacy(
    repository: &Repository,
    brand: &str,
    legacy: Vec<LegacyFirmware>,
) -> worker::Result<()> {
    for record in legacy {
        let key = match fingerprint::content_key(brand, &record.packages).await {
            Ok(key) => key,
            Err(_) => {
                console_log!("XML baseline unavailable submission_id={}", record.id);
                continue;
            }
        };
        repository.set_legacy_fingerprint(&record.id, &key, brand).await?;
    }
    Ok(())
}
