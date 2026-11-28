mod device_models;
mod ffusersubmission;
mod firmware;

use std::time::Duration;

use serde::{Deserialize, Serialize};
use worker::{console_log, Delay, Env, Error, MessageBatch, MessageExt, Result};

use crate::domain::FirmwareInput;
use crate::notifications;

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
pub(crate) enum Job {
    Ffusersubmission(ffusersubmission::Job),
    Ota(firmware::Job),
}

pub(super) enum NotificationKind {
    Ota,
    Ffusersubmission,
}

pub(super) struct PendingNotification {
    pub(super) kind: NotificationKind,
    pub(super) input: FirmwareInput,
    pub(super) target: String,
}

pub(crate) async fn scheduled(cron: &str, env: &Env) -> Result<()> {
    match cron {
        firmware::CRON => firmware::enqueue(env).await,
        ffusersubmission::CRON => ffusersubmission::enqueue(env).await,
        device_models::CRON => device_models::sync(env).await,
        _ => Err(Error::RustError(format!("Unsupported cron: {cron}"))),
    }
}

async fn deliver_notifications(env: &Env, pending: Vec<PendingNotification>) {
    if pending.is_empty() {
        return;
    }
    let configured = notifications::is_configured(env);
    for item in pending {
        match item.kind {
            NotificationKind::Ota => {
                if configured {
                    match notifications::send(env, &item.input, &item.target).await {
                        Ok(()) => console_log!(
                            "OTA refresh: Telegram delivered dev={} action={} target={}",
                            firmware::log_target(&item.input.dev),
                            item.input.upgrade_or_erec.as_str(),
                            firmware::log_target(&item.target)
                        ),
                        Err(_) => console_log!(
                            "OTA refresh: Telegram failed dev={} action={} target={}",
                            firmware::log_target(&item.input.dev),
                            item.input.upgrade_or_erec.as_str(),
                            firmware::log_target(&item.target)
                        ),
                    }
                    Delay::from(Duration::from_millis(3100)).await;
                } else {
                    console_log!(
                        "OTA refresh: Telegram failed dev={} action={} target={}",
                        firmware::log_target(&item.input.dev),
                        item.input.upgrade_or_erec.as_str(),
                        firmware::log_target(&item.target)
                    );
                }
            }
            NotificationKind::Ffusersubmission => {
                if configured {
                    if notifications::send(env, &item.input, &item.target).await.is_err() {
                        console_log!("ffusersubmission: Telegram failed");
                    }
                    Delay::from(Duration::from_millis(3100)).await;
                }
            }
        }
    }
}

pub(crate) async fn consume(batch: MessageBatch<Job>, env: Env) -> Result<()> {
    let mut malformed = false;
    let mut messages = Vec::new();
    for message in batch.iter() {
        match message {
            Ok(message) => messages.push(message),
            Err(_) => malformed = true,
        }
    }

    let mut ota_indices = Vec::new();
    let mut ff_indices = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        match message.body() {
            Job::Ota(_) => ota_indices.push(index),
            Job::Ffusersubmission(_) => ff_indices.push(index),
        }
    }

    let mut repository = None;
    let mut pending_notifications = Vec::new();

    if !ota_indices.is_empty() {
        let jobs: Vec<&firmware::Job> = ota_indices
            .iter()
            .filter_map(|&index| match messages[index].body() {
                Job::Ota(job) => Some(job),
                Job::Ffusersubmission(_) => None,
            })
            .collect();
        let (outcomes, notifications) = firmware::process_batch(&jobs, &env, &mut repository).await;
        for (&index, result) in ota_indices.iter().zip(outcomes) {
            match result {
                Ok(()) => messages[index].ack(),
                Err(_) => {
                    console_log!("OTA refresh: job failed; queue retry requested");
                    messages[index].retry();
                }
            }
        }
        pending_notifications.extend(notifications);
    }

    if !ff_indices.is_empty() {
        let jobs: Vec<&ffusersubmission::Job> = ff_indices
            .iter()
            .filter_map(|&index| match messages[index].body() {
                Job::Ffusersubmission(job) => Some(job),
                Job::Ota(_) => None,
            })
            .collect();
        let (outcomes, notifications) =
            ffusersubmission::process_batch(&jobs, &env, &mut repository).await;
        for (&index, result) in ff_indices.iter().zip(outcomes) {
            match result {
                Ok(()) => messages[index].ack(),
                Err(_) => {
                    console_log!("ffusersubmission: job failed; queue retry requested");
                    messages[index].retry();
                }
            }
        }
        pending_notifications.extend(notifications);
    }

    deliver_notifications(&env, pending_notifications).await;

    if malformed {
        return Err(Error::RustError("Firmware queue: invalid message".into()));
    }
    Ok(())
}
