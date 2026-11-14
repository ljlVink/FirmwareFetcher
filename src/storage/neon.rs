use serde::{de::DeserializeOwned, Deserialize, Serialize};
use worker::{Env, Error, Fetch, Headers, Method, Request, RequestInit, Url};

pub struct NeonDb {
    endpoint: String,
    conn_string: String,
}

#[derive(Deserialize, Debug)]
pub struct SqlResult {
    #[serde(default)]
    pub rows: Vec<Vec<Option<String>>>,
}

#[derive(Serialize)]
pub struct SqlQuery<'a> {
    pub query: &'a str,
    pub params: &'a [&'a str],
}

#[derive(Deserialize)]
struct BatchResult {
    results: Vec<SqlResult>,
}

impl NeonDb {
    pub fn from_env(env: &Env) -> Result<Self, Error> {
        let conn_string = env.secret("NEON_CONN_STRING")?.to_string();
        let url = Url::parse(&conn_string)?;
        let host = url
            .host_str()
            .ok_or_else(|| Error::RustError("NEON_CONN_STRING has no host".into()))?;
        let endpoint = match host.split_once('.') {
            Some((_, rest)) => format!("https://api.{rest}/sql"),
            None => return Err(Error::RustError(format!("bad NEON host: {host}"))),
        };
        Ok(Self {
            endpoint,
            conn_string,
        })
    }

    pub async fn query(&self, sql: &str, params: &[&str]) -> Result<SqlResult, Error> {
        self.send(&SqlQuery { query: sql, params }).await
    }

    pub async fn batch(&self, queries: &[SqlQuery<'_>]) -> Result<Vec<SqlResult>, Error> {
        #[derive(Serialize)]
        struct Batch<'a> {
            queries: &'a [SqlQuery<'a>],
        }
        let result: BatchResult = self.send(&Batch { queries }).await?;
        Ok(result.results)
    }

    async fn send<T: Serialize + ?Sized, R: DeserializeOwned>(&self, payload: &T) -> Result<R, Error> {
        let body = serde_json::to_string(payload)
            .map_err(|e| Error::RustError(format!("neon bad json: {e}")))?;
        let headers = Headers::new();
        headers.set("Content-Type", "application/json")?;
        headers.set("Neon-Connection-String", &self.conn_string)?;
        headers.set("Neon-Raw-Text-Output", "true")?;
        headers.set("Neon-Array-Mode", "true")?;
        let init = RequestInit {
            method: Method::Post,
            headers,
            body: Some(wasm_bindgen::JsValue::from_str(&body)),
            ..Default::default()
        };
        let req = Request::new_with_init(&self.endpoint, &init)?;
        let mut resp = Fetch::Request(req).send().await?;
        let status = resp.status_code();
        let text = resp.text().await?;
        if status != 200 {
            let msg = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| {
                    v.get("message")
                        .and_then(|m| m.as_str())
                        .map(str::to_owned)
                })
                .unwrap_or_else(|| text.chars().take(300).collect());
            return Err(Error::RustError(format!("neon http {status}: {msg}")));
        }
        serde_json::from_str(&text).map_err(|e| Error::RustError(format!("neon bad json: {e}")))
    }
}
