use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use worker::{console_log, Delay, Env, Error, MessageBatch, MessageExt, Result};

use crate::domain::{CheckedFirmware, FirmwareInput, RefreshSeed, UpdateAction, VersionMatch};
use crate::service::{self, SaveError};
use crate::{firmware, notifications, storage::Repository};

pub const CRON: &str = "0 17 * * *";


#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    dev: String,
    vendor: String,
    base: String,
    cust: String,
    preload: String,
}

impl Job {
    fn input(&self, action: UpdateAction) -> FirmwareInput {
        FirmwareInput {
            brand: "huawei", dev: self.dev.clone(), vendor: self.vendor.clone(),
            base: self.base.clone(), cust: self.cust.clone(), preload: self.preload.clone(),
            patch: Some("patch00".into()), upgrade_or_erec: action,
        }
    }
}


#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Version {
    parts: [u64; 4],
    len: u8,
}

#[derive(Debug)]
struct Triple {
    base: String,
    cust: String,
    preload: String,
    base_prefix: String,
    lgrp: String,
    region: String,
    region_revision: u64,
    base_version: Version,
    sp: u64,
    cust_version: Version,
    preload_version: Version,
}

fn digits(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) { return None; }
    s.parse().ok()
}

fn numbers(s: &str) -> Option<Version> {
    let mut parts = [0; 4];
    let mut len = 0;
    for part in s.split('.') {
        if len == parts.len() { return None; }
        parts[len] = digits(part)?;
        len += 1;
    }
    (3..=4).contains(&len).then_some(Version { parts, len: len as u8 })
}

fn base_parts(s: &str) -> Option<(&str, &str, Version, u64)> {
    let (prefix, version) = s.split_once(' ')?;
    if prefix.chars().any(char::is_whitespace) { return None; }
    let mut lgrp = None;
    let mut parts = prefix.split('-').peekable();
    while let Some(part) = parts.next() {
        if let Some(n) = part.strip_prefix("LGRP") {
            if digits(n).is_some() && parts.peek().is_some() && lgrp.is_none() {
                lgrp = Some(part);
            }
        }
    }
    let (version, sp) = if let Some((v, rest)) = version.split_once("(SP") {
        (v, digits(rest.strip_suffix(')')?)?)
    } else { (version, 0) };
    Some((prefix, lgrp?, numbers(version)?, sp))
}

fn regional_parts<'a>(s: &'a str, kind: &str) -> Option<(Version, &'a str)> {
    let (prefix, rest) = s.split_once(' ')?;
    if prefix.chars().any(char::is_whitespace) || !prefix.ends_with(kind) || prefix.len() == kind.len() { return None; }
    let (version, region) = rest.split_once('(')?;
    Some((numbers(version)?, region.strip_suffix(')')?))
}

fn region(s: &str) -> bool {
    let s = s.strip_prefix("DEM").unwrap_or(s);
    s.strip_prefix('C').and_then(digits).is_some()
}

fn triple<'a>(versions: impl IntoIterator<Item = &'a str>) -> Option<Triple> {
    let mut base = None;
    let mut cust = None;
    let mut preload = None;
    for value in versions {
        if let Some(parts) = base_parts(value) {
            if base.replace((value, parts)).is_some() { return None; }
        }
        if let Some((v, c)) = regional_parts(value, "-CUST") {
            if region(c) && cust.replace((value, v, c)).is_some() { return None; }
        }
        if let Some((v, cr)) = regional_parts(value, "-PRELOAD") {
            if let Some((c, r)) = cr.rsplit_once('R') {
                if region(c) {
                    if let Some(r) = digits(r) {
                        if preload.replace((value, v, c, r)).is_some() { return None; }
                    }
                }
            }
        }
    }
    let (base, (prefix, lgrp, base_version, sp)) = base?;
    let (cust, cust_version, c) = cust?;
    let (preload, preload_version, pc, r) = preload?;
    if c != pc { return None; }
    Some(Triple {
        base: base.into(), cust: cust.into(), preload: preload.into(),
        base_prefix: prefix.into(), lgrp: lgrp.into(), region: c.into(), region_revision: r,
        base_version, sp, cust_version, preload_version,
    })
}

type Branch = (String, String, String, String, u64, u64);
type Rank = (Version, u64, Version, Version, i64);

fn select_seeds(rows: Vec<RefreshSeed>) -> (Vec<Job>, usize, usize) {
    let mut latest: BTreeMap<Branch, (Rank, Job)> = BTreeMap::new();
    let mut skipped = 0;
    let mut upgrade_models = BTreeSet::new();
    let mut recovery_models = BTreeSet::new();
    for row in rows {
        if row.action != UpdateAction::Recovery {
            upgrade_models.insert(row.dev);
            continue;
        }
        recovery_models.insert(row.dev.clone());
        let Some(t) = triple(row.packages.as_deref().unwrap_or_default().iter().map(String::as_str)) else {
            skipped += 1;
            continue;
        };
        if row.lgrp.as_deref() != Some(t.lgrp.as_str()) {
            skipped += 1;
            continue;
        }
        let branch = (row.dev.clone(), row.vendor.clone(), t.base_prefix, t.region, t.region_revision, t.base_version.parts[0]);
        let rank = (t.base_version, t.sp, t.cust_version, t.preload_version, row.id);
        let job = Job { dev: row.dev, vendor: row.vendor, base: t.base, cust: t.cust, preload: t.preload };
        match latest.get_mut(&branch) {
            Some(previous) if rank > previous.0 => *previous = (rank, job),
            None => { latest.insert(branch, (rank, job)); }
            _ => {}
        }
    }
    let without_recovery = upgrade_models.difference(&recovery_models).count();
    (latest.into_values().map(|(_, job)| job).collect(), skipped, without_recovery)
}

pub async fn enqueue(env: &Env) -> Result<()> {
    let rows = Repository::connect(env).await?.refresh_seeds().await?;
    let (jobs, skipped, upgrade_only) = select_seeds(rows);
    console_log!("OTA refresh: {} branches, {} invalid recovery rows, {} upgrade-only models", jobs.len(), skipped, upgrade_only);
    let queue = env.queue("OTA_REFRESH")?;
    for chunk in jobs.chunks(100) {
        queue.send_batch::<&Job, _>(chunk.iter()).await?;
    }
    Ok(())
}

fn complete_target(matches: &[VersionMatch], dev: &str, expected_lgrp: &str) -> Result<Option<Triple>> {
    for entry in matches {
        let target = entry.info.as_ref().and_then(|info| info.target.as_deref()).unwrap_or("");
        if !target.strip_prefix(dev).is_some_and(|s| s.starts_with(' ')) { continue; }
        let mut values: [Option<&str>; 3] = [None, None, None];
        let mut invalid = false;
        for component in &entry.components {
            let kind = component.get("versionPackageType");
            let index = match kind {
                Some(v) if v.as_u64() == Some(2) || v.as_str() == Some("2") => Some(0),
                Some(v) if v.as_u64() == Some(3) || v.as_str() == Some("3") => Some(1),
                Some(v) if v.as_u64() == Some(4) || v.as_str() == Some("4") => Some(2),
                _ => None,
            };
            if let Some(i) = index {
                let value = component.get("versionNumber").and_then(|v| v.as_str());
                if values[i].is_some() || value.is_none() { invalid = true; }
                values[i] = value;
            }
        }
        if !invalid {
            if let [Some(base), Some(cust), Some(preload)] = values {
                if base_parts(base).is_some()
                    && regional_parts(cust, "-CUST").is_some()
                    && regional_parts(preload, "-PRELOAD").is_some()
                {
                    if let Some(t) = triple([base, cust, preload]) {
                        if t.lgrp == expected_lgrp { return Ok(Some(t)); }
                    }
                }
            }
        }
        return Err(Error::RustError("OTA refresh: invalid complete target components".into()));
    }
    Ok(None)
}

fn log_target(target: &str) -> String {
    target.chars().take(160).map(|c| {
        if c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '-' | '_' | '(' | ')') { c }
        else { '?' }
    }).collect()
}

async fn notify(env: &Env, input: &FirmwareInput, target: &str) {
    match notifications::send(env, input, target).await {
        Ok(()) => console_log!("OTA refresh: Telegram delivered dev={} action={} target={}", log_target(&input.dev), input.upgrade_or_erec.as_str(), log_target(target)),
        Err(_) => console_log!("OTA refresh: Telegram failed dev={} action={} target={}", log_target(&input.dev), input.upgrade_or_erec.as_str(), log_target(target)),
    }
    Delay::from(Duration::from_millis(3100)).await;
}

async fn save(env: &Env, input: &FirmwareInput, firmware: &CheckedFirmware) -> Result<bool> {
    service::save(env, input, &firmware.target, &firmware.packages)
        .await
        .map(|saved| saved.is_some())
        .map_err(|error| match error {
            SaveError::Fingerprint(message) => {
                Error::RustError(format!("OTA refresh: XML fingerprint failed: {message}"))
            }
            SaveError::Storage(error) => error,
        })
}

async fn process(job: &Job, env: &Env) -> Result<()> {
    let upgrade = job.input(UpdateAction::Upgrade);
    let diff = match firmware::query(env, &upgrade).await {
        Ok(result) => result,
        Err(failure) if failure.status == 422 => {
            console_log!("OTA refresh: no upgrade package for {}", job.dev);
            return Ok(());
        }
        Err(failure) => return Err(Error::RustError(format!("OTA refresh: upgrade query failed ({})", failure.status))),
    };
    if save(env, &upgrade, &diff).await? {
        console_log!("OTA refresh: inserted action=upgrade target={}", log_target(&diff.target));
        notify(env, &upgrade, &diff.target).await;
    } else {
        console_log!("OTA refresh: duplicate action=upgrade target={}", log_target(&diff.target));
    }
    let Some((_, lgrp, _, _)) = base_parts(&job.base) else {
        return Err(Error::RustError("OTA refresh: invalid seed base".into()));
    };
    let Some(target) = complete_target(&diff.matches, &job.dev, lgrp)? else {
        console_log!("OTA refresh: no complete target for {}", job.dev);
        return Ok(());
    };
    let recovery_job = Job { dev: job.dev.clone(), vendor: job.vendor.clone(),
        base: target.base, cust: target.cust, preload: target.preload };
    let recovery = recovery_job.input(UpdateAction::Recovery);
    let full = match firmware::query(env, &recovery).await {
        Ok(result) => result,
        Err(failure) if failure.status == 422 => {
            console_log!("OTA refresh: no recovery package for {}", job.dev);
            return Ok(());
        }
        Err(failure) => return Err(Error::RustError(format!("OTA refresh: recovery query failed ({})", failure.status))),
    };
    let actual = triple(full.packages.iter().map(|p| p.version_number.as_str()))
        .ok_or_else(|| Error::RustError("OTA refresh: incomplete recovery packages".into()))?;
    if actual.lgrp != lgrp {
        return Err(Error::RustError("OTA refresh: recovery response LGRP mismatch".into()));
    }
    if save(env, &recovery, &full).await? {
        console_log!("OTA refresh: inserted action=recovery target={}", log_target(&full.target));
        notify(env, &recovery, &full.target).await;
    } else {
        console_log!("OTA refresh: duplicate action=recovery target={}", log_target(&full.target));
    }
    Ok(())
}

pub async fn consume(batch: MessageBatch<Job>, env: Env) -> Result<()> {
    let mut malformed = false;
    for message in batch.iter() {
        let message = match message {
            Ok(message) => message,
            Err(_) => { malformed = true; continue; }
        };
        match process(message.body(), &env).await {
            Ok(()) => message.ack(),
            Err(_) => {
                console_log!("OTA refresh: job failed dev={}; queue retry requested", log_target(&message.body().dev));
                message.retry();
            }
        }
    }
    if malformed { return Err(Error::RustError("OTA refresh: invalid queue message".into())); }
    Ok(())
}
