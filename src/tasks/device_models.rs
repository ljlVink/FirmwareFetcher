use std::collections::{BTreeSet, HashSet};

use serde::Serialize;
use worker::{Env, Error, Fetch, Method, Request};

use crate::storage::Repository;

pub(super) const CRON: &str = "0 0 * * *";

const SOURCE_URL: &str = "https://raw.githubusercontent.com/KHwang9883/MobileModels-csv/main/models.csv";

#[derive(Debug, Serialize, Eq, PartialEq, Ord, PartialOrd)]
struct Model {
    model: String,
    model_name: String,
    ver_name: String,
    source_file: String,
}

fn parse_models(input: &[u8]) -> Result<BTreeSet<Model>, String> {
    let input = std::str::from_utf8(input).map_err(|e| format!("CSV is not valid UTF-8: {e}"))?;
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(input.as_bytes());
    let headers = reader.headers().map_err(|e| format!("Invalid CSV header: {e}"))?.clone();
    let mut seen = HashSet::new();
    if headers.iter().any(|header| !seen.insert(header)) {
        return Err("CSV contains duplicate column names".into());
    }
    let index = |name: &str| headers.iter().position(|header| header == name)
        .ok_or_else(|| format!("CSV is missing required column: {name}"));
    let brand_idx = index("brand")?;
    let model_idx = index("model")?;
    let name_idx = index("model_name")?;
    let version_idx = index("ver_name")?;
    let source_idx = index("source_file")?;
    let mut models = BTreeSet::new();
    let (mut huawei, mut honor) = (false, false);
    let mut row = csv::StringRecord::new();
    while reader.read_record(&mut row).map_err(|e| format!("Failed to read CSV: {e}"))? {
        let brand = match row.get(brand_idx) {
            Some("huawei") => "huawei",
            Some("honor") => "honor",
            _ => continue,
        };
        let line = row.position().map_or(0, |position| position.line());
        if row.len() != headers.len() {
            return Err(format!("CSV line {line} has a wrong column count"));
        }
        let model = row[model_idx].trim();
        let model_name = row[name_idx].trim();
        let ver_name = row[version_idx].trim();
        let source_file = row[source_idx].trim();
        let valid_source = source_file.strip_prefix(brand).is_some_and(|rest| rest.starts_with('_'));
        if model.is_empty() || model_name.is_empty() || !valid_source {
            return Err(format!("CSV line {line} has an invalid model, name or brand source"));
        }
        if [model, model_name, ver_name, source_file].iter().any(|value| value.contains('\0')) {
            return Err(format!("CSV line {line} contains a NUL character"));
        }
        models.insert(Model {
            model: model.to_owned(),
            model_name: model_name.to_owned(),
            ver_name: ver_name.to_owned(),
            source_file: source_file.to_owned(),
        });
        if brand == "huawei" { huawei = true; } else { honor = true; }
    }
    if !huawei || !honor {
        return Err("CSV must contain valid mappings for both Huawei and Honor; refusing to sync incomplete data".into());
    }
    Ok(models)
}


pub async fn sync(env: &Env) -> worker::Result<()> {
    let request = Request::new(SOURCE_URL, Method::Get)?;
    let mut response = Fetch::Request(request).send().await?;
    let status = response.status_code();
    if status != 200 {
        return Err(Error::RustError(format!("Device model CSV download failed: HTTP {status}")));
    }
    let bytes = response.bytes().await?;
    let models = parse_models(&bytes).map_err(Error::RustError)?;
    drop(bytes);
    let payload = serde_json::to_string(&models)
        .map_err(|e| Error::RustError(format!("Failed to serialize device model CSV: {e}")))?;
    let summary = Repository::connect(env).await?.sync_models(&payload).await?;
    worker::console_log!("Device model sync complete: {} valid mappings, {} added, {} removed", models.len(), summary.added, summary.removed);
    for (source, count) in summary.sources {
        worker::console_log!("  {}: {}", source, count);
    }
    Ok(())
}
