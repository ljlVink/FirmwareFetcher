mod device_models;
pub(crate) mod firmware;

use worker::{Env, Error, Result};

pub(crate) async fn scheduled(cron: &str, env: &Env) -> Result<()> {
    match cron {
        firmware::CRON => firmware::enqueue(env).await,
        device_models::CRON => device_models::sync(env).await,
        _ => Err(Error::RustError(format!("Unsupported cron: {cron}"))),
    }
}
