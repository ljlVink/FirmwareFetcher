use worker::{console_log, Env};

use crate::domain::{FirmwareInput, FirmwarePackage, SubmissionSource};
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
    baseline_legacy(&repository, input.brand, target).await?;
    Ok(repository.insert_firmware(input, target, packages, &key, source).await?)
}

async fn baseline_legacy(repository: &Repository, brand: &str, target: &str) -> worker::Result<()> {
    for record in repository.legacy_firmware(brand, target).await? {
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
