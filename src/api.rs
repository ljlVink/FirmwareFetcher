use std::collections::HashMap;

use futures_util::StreamExt;
use serde::Deserialize;
use worker::*;

use crate::domain::{FirmwareInput, FirmwarePackage, UpdateAction, PER_PAGE};
use crate::firmware::{self, manual, QueryEndpoint};
use crate::service::{self, SaveError};
use crate::{notifications, storage::Repository};

#[derive(Deserialize)]
struct NewRequest {
    brand: Option<String>,
    dev: String,
    base: String,
    cust: String,
    preload: String,
    patch: Option<String>,
    vendor: Option<String>,
    upgrade_or_erec: String,
    #[serde(default)]
    erecovery: bool,
}

fn check_text(s: String, max: usize, name: &str) -> Result<String, String> {
    if s.len() > max {
        return Err(format!("{name} is too long (max {max} bytes)"));
    }
    if !s.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
        return Err(format!("{name} contains invalid characters (printable ASCII only)"));
    }
    Ok(s)
}

fn clean(raw: Option<String>, default: &str, max: usize, name: &str) -> Result<String, String> {
    let mut s = raw.unwrap_or_default().trim().to_string();
    if s.is_empty() {
        if default.is_empty() {
            return Err(format!("{name} must not be empty"));
        }
        s = default.to_string();
    }
    check_text(s, max, name)
}

fn clean_opt(raw: Option<String>, max: usize, name: &str) -> Result<Option<String>, String> {
    let s = raw.unwrap_or_default().trim().to_string();
    if s.is_empty() {
        return Ok(None);
    }
    Ok(Some(check_text(s, max, name)?))
}

fn validate(v: NewRequest) -> Result<FirmwareInput, String> {
    Ok(FirmwareInput {
        brand: match v.brand.as_deref().unwrap_or("huawei") {
            "huawei" => "huawei",
            "honor" => "honor",
            _ => return Err("brand must be huawei or honor".into()),
        },
        dev: clean(Some(v.dev), "", 64, "dev")?,
        base: clean(Some(v.base), "", 160, "base")?,
        cust: clean(Some(v.cust), "", 160, "cust")?,
        preload: clean(Some(v.preload), "", 160, "preload")?,
        patch: clean_opt(v.patch, 32, "patch")?,
        vendor: clean(v.vendor, "all_cn", 32, "vendor")?,
        upgrade_or_erec: match v.upgrade_or_erec.trim() {
            "emulater" => return Err("Emulator firmware can only be imported by administrators; submissions are not accepted".into()),
            action => action.parse().map_err(|_| "Package type must be upgrade (incremental), recovery (full) or rollback (beta)".to_owned())?,
        },
    })
}

async fn parse_input(req: &mut Request) -> Result<NewRequest> {
    let ct = req.headers().get("content-type")?.unwrap_or_default();
    if ct.starts_with("application/json") {
        return req.json().await;
    }
    let form = req.form_data().await?;
    let field = |k: &str| {
        form.get(k).and_then(|entry| match entry {
            FormEntry::Field(value) => Some(value),
            _ => None,
        })
    };
    Ok(NewRequest {
        brand: field("brand"),
        dev: field("dev").unwrap_or_default(),
        base: field("base").unwrap_or_default(),
        cust: field("cust").unwrap_or_default(),
        preload: field("preload").unwrap_or_default(),
        patch: field("patch"),
        vendor: field("vendor"),
        upgrade_or_erec: field("upgrade_or_erec").unwrap_or_default(),
        erecovery: match field("erecovery").as_deref() {
            Some("true") => true,
            Some("false") | None => false,
            _ => return Err(Error::RustError("erecovery must be true or false".into())),
        },
    })
}

fn api_error(message: &str, status: u16) -> Result<Response> {
    Ok(Response::from_json(&serde_json::json!({ "error": message }))?.with_status(status))
}

fn json_response(body: String, status: u16) -> Result<Response> {
    let headers = Headers::new();
    headers.set("Content-Type", "application/json; charset=utf-8")?;
    headers.set("Cache-Control", "no-store")?;
    Ok(Response::ok(body)?.with_status(status).with_headers(headers))
}

async fn create(mut req: Request, ctx: RouteContext<Context>) -> Result<Response> {
    let parsed = match parse_input(&mut req).await {
        Ok(input) => input,
        Err(_) => return api_error("Invalid submission format", 400),
    };
    let endpoint = if parsed.erecovery {
        QueryEndpoint::ERecovery
    } else {
        QueryEndpoint::OtaUpdate
    };
    let input = match validate(parsed) {
        Ok(input) => input,
        Err(message) => return api_error(&message, 400),
    };
    let checked = match firmware::query(&ctx.env, &input, endpoint).await {
        Ok(checked) => checked,
        Err(failure) => return api_error(&failure.message, failure.status),
    };
    save(input, checked.target, checked.packages, &ctx.env, &ctx.data).await
}

async fn create_manual(mut req: Request, ctx: RouteContext<Context>) -> Result<Response> {
    let content_type = req.headers().get("content-type")?.unwrap_or_default();
    if !content_type.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("application/json") {
        return api_error("Manual submission only accepts application/json in the form {\"urls\":[\"package URL\",…]}", 415);
    }
    if let Some(length) = req.headers().get("content-length")? {
        if length.parse::<usize>().map_or(true, |length| length > manual::MAX_BODY_SIZE) {
            return api_error("Manual submission must not exceed 16 KiB", 413);
        }
    }
    let mut stream = match req.stream() {
        Ok(stream) => stream,
        Err(_) => return api_error("Submission must not be empty", 400),
    };
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(_) => return api_error("Unable to read submission", 400),
        };
        if body.len() + chunk.len() > manual::MAX_BODY_SIZE {
            return api_error("Manual submission must not exceed 16 KiB", 413);
        }
        body.extend_from_slice(&chunk);
    }
    let parsed = match serde_json::from_slice::<manual::ManualRequest>(&body) {
        Ok(parsed) => parsed,
        Err(_) => return api_error("Invalid submission format; body must contain only a urls string array", 400),
    };
    let prepared = match manual::prepare(parsed) {
        Ok(prepared) => prepared,
        Err(message) => return api_error(&message, 400),
    };
    let (input, target, packages) = match manual::verify(prepared).await {
        Ok(verified) => verified,
        Err(message) => return api_error(&message, 422),
    };
    save(input, target, packages, &ctx.env, &ctx.data).await
}

async fn save(input: FirmwareInput, target: String, packages: Vec<FirmwarePackage>, env: &Env, context: &Context) -> Result<Response> {
    let result = match service::save(env, &input, &target, &packages).await {
        Ok(Some(body)) => body,
        Ok(None) => return api_error("An identical package already exists", 409),
        Err(SaveError::Fingerprint(message)) => {
            return api_error(&format!("Unable to verify XML package fingerprint: {message}; record not saved"), 502);
        }
        Err(SaveError::Storage(error)) => return Err(error),
    };
    let response = json_response(result, 201)?;
    notifications::schedule(env, context, input, target);
    Ok(response)
}

fn like_pattern(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut escaped = String::with_capacity(trimmed.len() + 2);
    for c in trimmed.chars().take(200) {
        if matches!(c, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    format!("%{escaped}%")
}

async fn list(req: Request, ctx: RouteContext<Context>) -> Result<Response> {
    let url = req.url()?;
    let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
    let pattern = like_pattern(query.get("q").map(String::as_str).unwrap_or(""));
    let action = match query.get("action").map(String::as_str) {
        Some("emulater") => "emulater",
        Some(action) => action.parse::<UpdateAction>().map(UpdateAction::as_str).unwrap_or(""),
        None => "",
    };
    let include_emulators = query.get("include_emulators").map(String::as_str).unwrap_or("");
    let page = query.get("page")
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(1)
        .max(1);
    let offset = page.saturating_sub(1).saturating_mul(PER_PAGE);
    let repository = Repository::connect(&ctx.env).await?;
    let body = repository.list(&pattern, action, PER_PAGE, offset, include_emulators).await?;
    json_response(body, 200)
}

pub(crate) async fn handle(req: Request, env: Env, ctx: Context) -> Result<Response> {
    Router::with_data(ctx)
        .post_async("/requests", create)
        .post_async("/requests/manual", create_manual)
        .get_async("/requests", list)
        .run(req, env)
        .await
}
