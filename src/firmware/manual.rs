use serde::Deserialize;
use worker::Url;

use crate::domain::{FirmwareInput, FirmwarePackage, UpdateAction};

pub(crate) const MAX_BODY_SIZE: usize = 16 * 1024;
const URL_PREFIX: &str = "http://update.dbankcdn.com/";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManualRequest {
    urls: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Base,
    Cust,
    Preload,
    Updater,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Base => "BASE package",
            Self::Cust => "CUST package",
            Self::Preload => "PRELOAD package",
            Self::Updater => "updater.zip",
        }
    }
}

struct PackageUrl {
    url: String,
    kind: Kind,
    dev: String,
    vendor: String,
    revision: Option<String>,
}

pub(crate) struct Prepared {
    urls: Vec<PackageUrl>,
}

fn digits(raw: &str) -> bool {
    !raw.is_empty() && raw.bytes().all(|byte| byte.is_ascii_digit())
}

fn identifier(raw: &str) -> bool {
    !raw.is_empty() && raw.bytes().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn model(raw: &str) -> bool {
    raw.len() <= 64 && raw.split_once('-').is_some_and(|(series, model)| identifier(series) && identifier(model))
}

fn package_url(raw: String) -> Result<PackageUrl, String> {
    let raw = raw.trim();
    // Literal prefix validation is mandatory, independently of frontend validation.
    if !raw.starts_with(URL_PREFIX) {
        return Err(format!("Package URL must start with {URL_PREFIX}"));
    }
    if raw.len() > 2048 || !raw.bytes().all(|byte| byte.is_ascii_alphanumeric() || b":/._-".contains(&byte)) {
        return Err("Package URL is too long or contains invalid characters; escapes, whitespace, query strings and fragments are not allowed".into());
    }
    let url = Url::parse(raw).map_err(|_| "Package URL is not a valid URL")?;
    if url.scheme() != "http" || url.host_str() != Some("update.dbankcdn.com")
        || !url.username().is_empty() || url.password().is_some() || url.port().is_some()
        || url.query().is_some() || url.fragment().is_some() || url.as_str() != raw
    {
        return Err("Package URL must use the designated HTTP host without credentials, port, query string or fragment".into());
    }
    let path = &raw[URL_PREFIX.len()..];
    let (directory, filename) = path.rsplit_once('/').ok_or("Package URL is missing the full directory")?;
    if path.split('/').any(|part| part.is_empty() || matches!(part, "." | ".."))
        || directory.rsplit('/').next() != Some("full")
    {
        return Err("Invalid package URL path; it must be a direct ZIP link in the full directory".into());
    }
    let mut package = PackageUrl {
        url: raw.to_owned(), kind: Kind::Base,
        dev: String::new(), vendor: String::new(), revision: None,
    };
    match filename {
        "update_full_base.zip" => return Ok(package),
        "updater.zip" => { package.kind = Kind::Updater; return Ok(package); }
        _ => {}
    }
    let name = filename.strip_suffix(".zip").ok_or("Package URL must point to a ZIP file")?;
    let identity = if let Some(identity) = name.strip_prefix("update_full_cust_") {
        package.kind = Kind::Cust;
        identity
    } else if let Some(identity) = name.strip_prefix("update_full_preload_") {
        package.kind = Kind::Preload;
        let (identity, revision) = identity.rsplit_once("_R").ok_or("PRELOAD package file name is missing the R version")?;
        if !digits(revision) { return Err("Invalid R version in PRELOAD package file name".into()); }
        package.revision = Some(revision.to_owned());
        identity
    } else {
        return Err("Only update_full_base.zip, update_full_cust_<model>_<channel>.zip, update_full_preload_<model>_<channel>_R<version>.zip and an optional updater.zip are accepted".into());
    };
    let (dev, vendor) = identity.split_once('_').ok_or("CUST or PRELOAD package file name is missing the model or channel")?;
    if !model(dev) || vendor.is_empty() || vendor.len() > 32
        || !vendor.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err("Invalid model or channel in package file name".into());
    }
    package.dev = dev.to_owned();
    package.vendor = vendor.to_owned();
    Ok(package)
}

pub(crate) fn prepare(input: ManualRequest) -> Result<Prepared, String> {
    if !(3..=4).contains(&input.urls.len()) {
        return Err("BASE, CUST and PRELOAD packages are required, plus an optional updater.zip (3 or 4 URLs in total)".into());
    }
    let mut urls: Vec<PackageUrl> = Vec::with_capacity(input.urls.len());
    for (index, raw) in input.urls.into_iter().enumerate() {
        let package = package_url(raw).map_err(|message| format!("URL {}: {message}", index + 1))?;
        if urls.iter().any(|entry| entry.kind == package.kind) {
            return Err(format!("Duplicate {}; only one of each package type may be submitted", package.kind.label()));
        }
        urls.push(package);
    }
    for kind in [Kind::Base, Kind::Cust, Kind::Preload] {
        if !urls.iter().any(|entry| entry.kind == kind) {
            return Err(format!("Missing {}", kind.label()));
        }
    }
    urls.sort_unstable_by_key(|entry| match entry.kind { Kind::Base => 0, Kind::Cust => 1, Kind::Preload => 2, Kind::Updater => 3 });
    if urls[1].dev != urls[2].dev || urls[1].vendor != urls[2].vendor {
        return Err("CUST and PRELOAD package file names must have the same model and channel".into());
    }
    Ok(Prepared { urls })
}

struct Version<'a> {
    identity: &'a str,
    number: &'a str,
    suffix: &'a str,
}

fn version(raw: &str) -> Result<Version<'_>, String> {
    if raw.is_empty() || raw.len() > 160 || !raw.bytes().all(|byte| byte.is_ascii_graphic() || byte == b' ') {
        return Err("VERSION.mbn must be a printable ASCII version of 1–160 bytes".into());
    }
    let (identity, rest) = raw.split_once(' ').ok_or("VERSION.mbn is missing the version number")?;
    let (number, suffix) = if let Some((number, suffix)) = rest.split_once('(') {
        (number, suffix.strip_suffix(')').ok_or("VERSION.mbn version has an unclosed parenthesis")?)
    } else { (rest, "") };
    let mut count = 0;
    for component in number.split('.') {
        if !digits(component) { return Err("VERSION.mbn numeric version components must be digits".into()); }
        count += 1;
    }
    if !(3..=4).contains(&count) {
        return Err("VERSION.mbn numeric version must have 3 or 4 components".into());
    }
    Ok(Version { identity, number, suffix })
}

fn assemble(prepared: Prepared, texts: &[String]) -> Result<(FirmwareInput, String, Vec<FirmwarePackage>), String> {
    let base = version(&texts[0]).map_err(|message| format!("BASE package: {message}"))?;
    let cust = version(&texts[1]).map_err(|message| format!("CUST package: {message}"))?;
    let preload = version(&texts[2]).map_err(|message| format!("PRELOAD package: {message}"))?;
    let mut base_parts = base.identity.split('-');
    let (Some(series), Some(lgrp), Some(region), None) =
        (base_parts.next(), base_parts.next(), base_parts.next(), base_parts.next())
    else { return Err("BASE package VERSION.mbn has an invalid series, LGRP group or region".into()); };
    if !identifier(series) || !lgrp.strip_prefix("LGRP").is_some_and(digits)
        || !identifier(region) || !(base.suffix.is_empty() || base.suffix.strip_prefix("SP").is_some_and(digits))
    {
        return Err("BASE package VERSION.mbn must be <series>-LGRP<group>-<region> <numeric version> (optionally with SP version)".into());
    }
    let dev = &prepared.urls[1].dev;
    if cust.identity.strip_suffix("-CUST") != Some(dev.as_str()) || preload.identity.strip_suffix("-PRELOAD") != Some(dev.as_str())
        || dev.split_once('-').map(|parts| parts.0) != Some(series)
    {
        return Err("Series, model or package type in the VERSION.mbn of the three packages does not match the file names".into());
    }
    let cust_region = cust.suffix.strip_prefix('C').filter(|region| digits(region)).ok_or("CUST package VERSION.mbn is missing a valid C region code")?;
    let (preload_region, revision) = preload.suffix.strip_prefix('C').and_then(|suffix| suffix.split_once('R'))
        .filter(|(region, revision)| digits(region) && digits(revision)).ok_or("PRELOAD package VERSION.mbn is missing a valid C region code or R version")?;
    if cust_region != preload_region || prepared.urls[2].revision.as_deref() != Some(revision) {
        return Err("CUST and PRELOAD C region codes differ, or the PRELOAD R version does not match its file name".into());
    }
    if texts.len() == 4 {
        validate_updater(&texts[3])?;
    }
    // Reverse app.js's parser. CUST/PRELOAD number prefixes can legitimately differ
    // from BASE (e.g. TAS 104.2.0 vs PRELOAD 104.0.0); E/P are their last components.
    let target = format!("{dev}-{lgrp} {}({}C{cust_region}E{}R{revision}P{})", base.number, base.suffix,
        cust.number.rsplit('.').next().unwrap(), preload.number.rsplit('.').next().unwrap());
    let input = FirmwareInput {
        brand: "huawei",
        dev: dev.clone(), base: texts[0].clone(), cust: texts[1].clone(), preload: texts[2].clone(),
        // updater.zip does not declare a patchNN version. Never invent patch00.
        patch: None, vendor: prepared.urls[1].vendor.clone(),
        upgrade_or_erec: UpdateAction::Recovery,
    };
    let packages = prepared.urls.into_iter().enumerate().map(|(index, package)| FirmwarePackage {
        version_number: if package.kind == Kind::Updater { "updater.zip".into() } else { texts[index].clone() },
        url: package.url,
    }).collect();
    Ok((input, target, packages))
}

fn validate_updater(text: &str) -> Result<(), String> {
    let mut mode = None;
    let mut kind = None;
    let mut data = None;
    if text.len() > 4096 { return Err("updater.zip update.tag is too long".into()); }
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (key, value) = line.split_once('=').ok_or("Invalid updater.zip update.tag format")?;
        let slot = match key {
            "PKG_UPDATE_MODE" => &mut mode,
            "PKG_TYPE" => &mut kind,
            "PKG_DATA_TYPE" => &mut data,
            _ => return Err("updater.zip update.tag contains an unknown field".into()),
        };
        if slot.replace(value).is_some() { return Err("updater.zip update.tag contains a duplicate field".into()); }
    }
    if mode != Some("OTA") || kind != Some("PATCH") || data != Some("FULL") {
        return Err("updater.zip must pass update.tag checks: PKG_UPDATE_MODE=OTA, PKG_TYPE=PATCH, PKG_DATA_TYPE=FULL".into());
    }
    Ok(())
}

pub(crate) async fn verify(prepared: Prepared) -> Result<(FirmwareInput, String, Vec<FirmwarePackage>), String> {
    let mut texts = Vec::with_capacity(prepared.urls.len());
    for package in &prepared.urls {
        let entry = if package.kind == Kind::Updater { "update.tag" } else { "VERSION.mbn" };
        let text = super::remote_zip::fetch_entry(&package.url, entry).await
            .map_err(|message| format!("{} validation failed: {message}; record not saved", package.kind.label()))?;
        texts.push(text);
    }
    assemble(prepared, &texts)
}
