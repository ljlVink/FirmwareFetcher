use std::collections::BTreeMap;

use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use worker::{console_log, Env, Error, Result};

use crate::domain::{CheckedFirmware, FirmwareInput, RefreshSeeds, SubmissionSource, UpdateAction, VersionMatch};
use crate::firmware::fingerprint;
use crate::service::{self, SaveError};
use crate::{firmware, storage::Repository};

use super::{NotificationKind, PendingNotification};

pub const CRON: &str = "0 16 * * *";
const OTA_QUERY_CONCURRENCY: usize = 5;


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

fn select_seeds(data: RefreshSeeds) -> (Vec<Job>, usize, usize) {
    let mut latest: BTreeMap<Branch, (Rank, Job)> = BTreeMap::new();
    let mut skipped = data.invalid_recovery;
    for row in data.seeds {
        let Some(t) = triple(row.packages.as_deref().unwrap_or_default().iter().map(String::as_str)) else {
            skipped += 1;
            continue;
        };
        if row.lgrp != t.lgrp {
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
    (latest.into_values().map(|(_, job)| job).collect(), skipped, data.upgrade_only)
}

pub async fn enqueue(env: &Env) -> Result<()> {
    let data = Repository::connect(env).await?.refresh_seeds().await?;
    let (jobs, skipped, upgrade_only) = select_seeds(data);
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

pub(super) fn log_target(target: &str) -> String {
    target.chars().take(160).map(|c| {
        if c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '-' | '_' | '(' | ')') { c }
        else { '?' }
    }).collect()
}

async fn save(repository: &Repository, input: &FirmwareInput, firmware: &CheckedFirmware) -> Result<bool> {
    service::save_with_repository(repository, input, &firmware.target, &firmware.packages, SubmissionSource::AutoRefresh)
        .await
        .map(|saved| saved.is_some())
        .map_err(|error| match error {
            SaveError::Fingerprint(message) => {
                Error::RustError(format!("OTA refresh: XML fingerprint failed: {message}"))
            }
            SaveError::Storage(error) => error,
        })
}

pub(super) async fn process_batch(
    jobs: &[&Job],
    env: &Env,
    repository: &mut Option<Repository>,
) -> (Vec<Result<()>>, Vec<PendingNotification>) {
    let mut outcomes: Vec<Result<()>> = (0..jobs.len()).map(|_| Ok(())).collect();
    let mut notifications = Vec::new();
    if jobs.is_empty() {
        return (outcomes, notifications);
    }

    let mut upgrade_responses = stream::iter(jobs.iter().enumerate())
        .map(|(index, job)| async move {
            let input = job.input(UpdateAction::Upgrade);
            let result = firmware::query(env, &input, firmware::QueryEndpoint::OtaUpdate).await;
            (index, input, result)
        })
        .buffer_unordered(OTA_QUERY_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    upgrade_responses.sort_by_key(|(index, _, _)| *index);

    let mut upgrade_hits = Vec::new();
    for (index, input, result) in upgrade_responses {
        match result {
            Ok(diff) => {
                let url_key = fingerprint::url_set_key(&diff.packages);
                upgrade_hits.push((index, input, diff, url_key));
            }
            Err(failure) if failure.status == 422 => {
                console_log!("OTA refresh: no upgrade package for {}", jobs[index].dev);
            }
            Err(failure) => {
                outcomes[index] = Err(Error::RustError(format!(
                    "OTA refresh: upgrade query failed ({})",
                    failure.status
                )));
            }
        }
    }
    if upgrade_hits.is_empty() {
        return (outcomes, notifications);
    }

    let repo = match repository {
        Some(repo) => repo,
        None => match Repository::connect(env).await {
            Ok(repo) => repository.insert(repo),
            Err(error) => {
                let message = error.to_string();
                for (index, _, _, _) in upgrade_hits {
                    outcomes[index] = Err(Error::RustError(message.clone()));
                }
                return (outcomes, notifications);
            }
        },
    };

    let upgrade_keys: Vec<String> = upgrade_hits
        .iter()
        .filter_map(|(_, _, _, key)| key.clone())
        .collect();
    let mut existing_keys = match repo.existing_url_set_keys("huawei", &upgrade_keys).await {
        Ok(keys) => keys,
        Err(error) => {
            let message = error.to_string();
            for (index, _, _, _) in upgrade_hits {
                outcomes[index] = Err(Error::RustError(message.clone()));
            }
            return (outcomes, notifications);
        }
    };

    let mut recovery_tasks = Vec::new();
    for (index, upgrade, diff, url_key) in upgrade_hits {
        let already_saved = url_key.as_deref().is_some_and(|key| existing_keys.contains(key));
        if already_saved {
            console_log!("OTA refresh: duplicate action=upgrade target={}", log_target(&diff.target));
        } else {
            match save(repo, &upgrade, &diff).await {
                Ok(true) => {
                    console_log!("OTA refresh: inserted action=upgrade target={}", log_target(&diff.target));
                    if let Some(key) = url_key {
                        existing_keys.insert(key);
                    }
                    notifications.push(PendingNotification {
                        kind: NotificationKind::Ota,
                        input: upgrade,
                        target: diff.target,
                    });
                }
                Ok(false) => {
                    console_log!("OTA refresh: duplicate action=upgrade target={}", log_target(&diff.target));
                    if let Some(key) = url_key {
                        existing_keys.insert(key);
                    }
                }
                Err(error) => {
                    outcomes[index] = Err(error);
                    continue;
                }
            }
        }
        let Some((_, lgrp, _, _)) = base_parts(&jobs[index].base) else {
            outcomes[index] = Err(Error::RustError("OTA refresh: invalid seed base".into()));
            continue;
        };
        let target = match complete_target(&diff.matches, &jobs[index].dev, lgrp) {
            Ok(Some(target)) => target,
            Ok(None) => {
                console_log!("OTA refresh: no complete target for {}", jobs[index].dev);
                continue;
            }
            Err(error) => {
                outcomes[index] = Err(error);
                continue;
            }
        };
        let recovery_job = Job {
            dev: jobs[index].dev.clone(),
            vendor: jobs[index].vendor.clone(),
            base: target.base,
            cust: target.cust,
            preload: target.preload,
        };
        recovery_tasks.push((index, lgrp.to_owned(), recovery_job.input(UpdateAction::Recovery)));
    }

    if recovery_tasks.is_empty() {
        return (outcomes, notifications);
    }

    let mut recovery_responses = stream::iter(recovery_tasks)
        .map(|(index, lgrp, input)| async move {
            let result = firmware::query(env, &input, firmware::QueryEndpoint::OtaUpdate).await;
            (index, lgrp, input, result)
        })
        .buffer_unordered(OTA_QUERY_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
    recovery_responses.sort_by_key(|(index, _, _, _)| *index);

    let mut recovery_hits = Vec::new();
    for (index, lgrp, recovery, result) in recovery_responses {
        let full = match result {
            Ok(full) => full,
            Err(failure) if failure.status == 422 => {
                console_log!("OTA refresh: no recovery package for {}", jobs[index].dev);
                continue;
            }
            Err(failure) => {
                outcomes[index] = Err(Error::RustError(format!(
                    "OTA refresh: recovery query failed ({})",
                    failure.status
                )));
                continue;
            }
        };
        let Some(actual) = triple(full.packages.iter().map(|p| p.version_number.as_str())) else {
            outcomes[index] = Err(Error::RustError("OTA refresh: incomplete recovery packages".into()));
            continue;
        };
        if actual.lgrp != lgrp {
            outcomes[index] = Err(Error::RustError("OTA refresh: recovery response LGRP mismatch".into()));
            continue;
        }
        let url_key = fingerprint::url_set_key(&full.packages);
        recovery_hits.push((index, recovery, full, url_key));
    }

    if recovery_hits.is_empty() {
        return (outcomes, notifications);
    }

    let pending_recovery_keys: Vec<String> = recovery_hits
        .iter()
        .filter_map(|(_, _, _, key)| key.as_ref().filter(|k| !existing_keys.contains(*k)).cloned())
        .collect();
    if !pending_recovery_keys.is_empty() {
        match repo.existing_url_set_keys("huawei", &pending_recovery_keys).await {
            Ok(found) => existing_keys.extend(found),
            Err(error) => {
                let message = error.to_string();
                for (index, _, _, _) in recovery_hits {
                    outcomes[index] = Err(Error::RustError(message.clone()));
                }
                return (outcomes, notifications);
            }
        }
    }

    for (index, recovery, full, url_key) in recovery_hits {
        let already_saved = url_key.as_deref().is_some_and(|key| existing_keys.contains(key));
        if already_saved {
            console_log!("OTA refresh: duplicate action=recovery target={}", log_target(&full.target));
            continue;
        }
        match save(repo, &recovery, &full).await {
            Ok(true) => {
                console_log!("OTA refresh: inserted action=recovery target={}", log_target(&full.target));
                if let Some(key) = url_key {
                    existing_keys.insert(key);
                }
                notifications.push(PendingNotification {
                    kind: NotificationKind::Ota,
                    input: recovery,
                    target: full.target,
                });
            }
            Ok(false) => {
                console_log!("OTA refresh: duplicate action=recovery target={}", log_target(&full.target));
                if let Some(key) = url_key {
                    existing_keys.insert(key);
                }
            }
            Err(error) => {
                outcomes[index] = Err(error);
            }
        }
    }

    (outcomes, notifications)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::RefreshSeed;

    #[test]
    fn select_seeds_picks_highest_version_per_branch_and_tracks_counts() {
        let seed = |id, base: &str, cust: &str, preload: &str, lgrp: &str| RefreshSeed {
            id,
            dev: "MNA-AL00".into(),
            vendor: "all_cn".into(),
            lgrp: lgrp.into(),
            packages: Some(vec![base.into(), cust.into(), preload.into()]),
        };
        let data = RefreshSeeds {
            upgrade_only: 3,
            invalid_recovery: 2,
            seeds: vec![
                seed(1, "MNA-LGRP1-CHN 4.0.0.100", "MNA-AL00-CUST 4.0.0.1(C00)", "MNA-AL00-PRELOAD 4.0.0.1(C00R1)", "LGRP1"),
                seed(2, "MNA-LGRP1-CHN 4.0.0.120(SP2)", "MNA-AL00-CUST 4.0.0.2(C00)", "MNA-AL00-PRELOAD 4.0.0.1(C00R1)", "LGRP1"),
                seed(3, "MNA-LGRP1-CHN 4.0.0.120(SP1)", "MNA-AL00-CUST 4.0.0.2(C00)", "MNA-AL00-PRELOAD 4.0.0.1(C00R1)", "LGRP1"),
                seed(4, "MNA-LGRP1-CHN 4.0.0.130", "MNA-AL00-CUST 4.0.0.1(C00)", "MNA-AL00-PRELOAD 4.0.0.1(C00R1)", "LGRP2"),
            ],
        };
        let (jobs, skipped, upgrade_only) = select_seeds(data);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].base, "MNA-LGRP1-CHN 4.0.0.120(SP2)");
        assert_eq!(skipped, 3);
        assert_eq!(upgrade_only, 3);
    }
}

