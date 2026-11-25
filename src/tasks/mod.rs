mod device_models;
mod ffusersubmission;
mod firmware;

use serde::{Deserialize, Serialize};
use worker::{console_log, Env, Error, MessageBatch, MessageExt, Result};

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum Job {
    Ffusersubmission(ffusersubmission::Job),
    Ota(firmware::Job),
}

pub(crate) async fn scheduled(cron: &str, env: &Env) -> Result<()> {
    match cron {
        firmware::CRON => firmware::enqueue(env).await,
        ffusersubmission::CRON => ffusersubmission::enqueue(env).await,
        device_models::CRON => device_models::sync(env).await,
        _ => Err(Error::RustError(format!("Unsupported cron: {cron}"))),
    }
}

pub(crate) async fn consume(batch: MessageBatch<Job>, env: Env) -> Result<()> {
    let mut malformed = false;
    for message in batch.iter() {
        let message = match message {
            Ok(message) => message,
            Err(_) => { malformed = true; continue; }
        };
        let (source, result) = match message.body() {
            Job::Ota(job) => ("OTA refresh", firmware::process(job, &env).await),
            Job::Ffusersubmission(job) => ("ffusersubmission", ffusersubmission::process(job, &env).await),
        };
        match result {
            Ok(()) => message.ack(),
            Err(_) => {
                console_log!("{}: job failed; queue retry requested", source);
                message.retry();
            }
        }
    }
    if malformed { return Err(Error::RustError("Firmware queue: invalid message".into())); }
    Ok(())
}
