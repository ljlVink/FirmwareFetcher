use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UpdateAction {
    Upgrade,
    Recovery,
    Rollback,
}

impl UpdateAction {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Upgrade => "upgrade",
            Self::Recovery => "recovery",
            Self::Rollback => "rollback",
        }
    }
}

impl FromStr for UpdateAction {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "upgrade" => Ok(Self::Upgrade),
            "recovery" => Ok(Self::Recovery),
            "rollback" => Ok(Self::Rollback),
            _ => Err(()),
        }
    }
}

pub(crate) const PER_PAGE: u32 = 50;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SubmissionSource {
    ManualVersion,
    ErecThreePackage,
    AutoRefresh,
    Ffusersubmission,
    AdminManual,
}

impl SubmissionSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ManualVersion => "manual_version",
            Self::ErecThreePackage => "erec_three_package",
            Self::AutoRefresh => "auto_refresh",
            Self::Ffusersubmission => "ffusersubmission",
            Self::AdminManual => "admin_manual",
        }
    }
}

pub(crate) struct FirmwareInput {
    pub(crate) brand: &'static str,
    pub(crate) dev: String,
    pub(crate) base: String,
    pub(crate) cust: String,
    pub(crate) preload: String,
    pub(crate) patch: Option<String>,
    pub(crate) vendor: String,
    pub(crate) upgrade_or_erec: UpdateAction,
}

pub(crate) struct CheckedFirmware {
    pub(crate) target: String,
    pub(crate) packages: Vec<FirmwarePackage>,
    pub(crate) matches: Vec<VersionMatch>,
}

pub(crate) struct CheckFailure {
    pub(crate) status: u16,
    pub(crate) message: String,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct FirmwarePackage {
    #[serde(rename = "versionNumber")]
    pub(crate) version_number: String,
    pub(crate) url: String,
}

#[derive(Deserialize)]
pub(crate) struct VersionCheckResult {
    pub(crate) status: String,
    #[serde(rename = "versionList", default)]
    pub(crate) packages: Vec<FirmwarePackage>,
    #[serde(rename = "blVersionCheckResults", default)]
    pub(crate) matches: Vec<VersionMatch>,
}

#[derive(Deserialize)]
pub(crate) struct VersionMatch {
    #[serde(rename = "blVersionInfo")]
    pub(crate) info: Option<TargetVersionInfo>,
    #[serde(rename = "targetBlComponents", default)]
    pub(crate) components: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
pub(crate) struct TargetVersionInfo {
    #[serde(rename = "targetBlVersionNumber", default)]
    pub(crate) target: Option<String>,
}

pub(crate) struct OtaFields<'a> {
    pub(crate) dev: &'a str,
    pub(crate) base: &'a str,
    pub(crate) cust: &'a str,
    pub(crate) preload: &'a str,
    pub(crate) patch: Option<&'a str>,
    pub(crate) vendor: &'a str,
    pub(crate) update_action: UpdateAction,
}

#[derive(Deserialize)]
pub(crate) struct LegacyFirmware {
    pub(crate) id: String,
    pub(crate) packages: Vec<FirmwarePackage>,
}

pub(crate) fn is_numeric_changelog_version(version: &str) -> bool {
    let version = version.trim();
    version.len() == 9 && version.bytes().all(|byte| byte.is_ascii_digit())
}

#[derive(Deserialize)]
pub(crate) struct RefreshSeed {
    pub(crate) id: i64,
    pub(crate) dev: String,
    pub(crate) vendor: String,
    pub(crate) lgrp: String,
    pub(crate) packages: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub(crate) struct RefreshSeeds {
    #[serde(default)]
    pub(crate) seeds: Vec<RefreshSeed>,
    #[serde(rename = "upgradeOnly", default)]
    pub(crate) upgrade_only: usize,
    #[serde(rename = "invalidRecovery", default)]
    pub(crate) invalid_recovery: usize,
}

#[derive(Deserialize)]
pub(crate) struct InsertAttempt {
    #[serde(default)]
    pub(crate) legacy: Vec<LegacyFirmware>,
    pub(crate) saved: Option<serde_json::Value>,
}

pub(crate) struct ModelSyncSummary {
    pub(crate) added: u64,
    pub(crate) removed: u64,
    pub(crate) sources: Vec<(String, u64)>,
}
