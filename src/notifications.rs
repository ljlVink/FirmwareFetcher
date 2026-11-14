use serde::Deserialize;
use worker::*;

use crate::domain::{FirmwareInput, UpdateAction};

const MESSAGE_LIMIT: usize = 4096;

struct Config {
    token: String,
    chat_id: String,
}

#[derive(Deserialize)]
struct ApiResponse {
    ok: bool,
    error_code: Option<u16>,
}

impl Config {
    fn from_env(env: &Env) -> Result<Self> {
        let binding = |key| -> Result<String> {
            let value = env.secret(key)
                .map_err(|_| Error::RustError(format!("{key} is not configured")))?
                .to_string();
            let value = value.trim();
            if value.is_empty() {
                return Err(Error::RustError(format!("{key} is empty")));
            }
            Ok(value.to_owned())
        };
        Ok(Self {
            token: binding("TG_BOT_KEY")?,
            chat_id: chat_id(&binding("TG_GROUP")?),
        })
    }
}

fn chat_id(value: &str) -> String {
    let value = value.trim();
    if value.starts_with('@') || value.parse::<i64>().is_ok() {
        value.to_owned()
    } else {
        format!("@{value}")
    }
}

pub(crate) fn schedule(env: &Env, context: &Context, input: FirmwareInput, target: String) {
    let config = match Config::from_env(env) {
        Ok(config) => config,
        Err(error) => {
            console_error!("Telegram notification not sent: {error}");
            return;
        }
    };
    context.wait_until(async move {
        let text = message(&input, &target);
        if let Err(error) = send_message(&config, &text).await {
            // Never log fetch errors or response bodies: they can contain the token URL.
            console_error!("Telegram notification failed (model {}): {error}", input.dev);
        }
    });
}

pub(crate) async fn send(env: &Env, input: &FirmwareInput, target: &str) -> Result<()> {
    let config = Config::from_env(env)?;
    let text = message(input, target);
    send_message(&config, &text).await
        .map_err(|_| Error::RustError("Telegram notification failed".into()))
}

fn message(input: &FirmwareInput, target: &str) -> String {
    let brand = if input.brand == "honor" { "HONOR" } else { "HUAWEI" };
    let kind = match input.upgrade_or_erec {
        UpdateAction::Upgrade => "Incremental package",
        UpdateAction::Recovery => "Full package",
    };
    format!(
        "New firmware added\nBrand: {brand}\nModel: {}\nChannel: {}\nVersion: {target}\nType: {kind}",
        input.dev, input.vendor,
    )
}

fn message_end(text: &str) -> usize {
    let mut units = 0;
    for (index, character) in text.char_indices() {
        units += character.len_utf16();
        if units > MESSAGE_LIMIT {
            return text[..index].rfind('\n')
                .filter(|index| *index > 0)
                .map_or(index, |index| index + 1);
        }
    }
    text.len()
}

async fn send_message(config: &Config, mut text: &str) -> Result<()> {
    let endpoint = format!("https://api.telegram.org/bot{}/sendMessage", config.token);
    while !text.is_empty() {
        let end = message_end(text);
        let body = serde_json::json!({
            "chat_id": config.chat_id,
            "text": &text[..end],
            "link_preview_options": { "is_disabled": true },
        }).to_string();
        let headers = Headers::new();
        headers.set("Content-Type", "application/json")?;
        let request = Request::new_with_init(&endpoint, &RequestInit {
            method: Method::Post,
            headers,
            body: Some(wasm_bindgen::JsValue::from_str(&body)),
            redirect: RequestRedirect::Manual,
            ..Default::default()
        }).map_err(|_| Error::RustError("Failed to build Telegram request".into()))?;
        let mut response = Fetch::Request(request).send().await
            .map_err(|_| Error::RustError("Telegram network request failed".into()))?;
        if response.status_code() != 200 {
            return Err(Error::RustError(format!("Telegram HTTP {}", response.status_code())));
        }
        let result: ApiResponse = response.json().await
            .map_err(|_| Error::RustError("Invalid Telegram response format".into()))?;
        if !result.ok {
            return Err(Error::RustError(format!("Telegram API rejected message (error code {:?})", result.error_code)));
        }
        text = &text[end..];
    }
    Ok(())
}
