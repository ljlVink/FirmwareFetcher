use worker::*;

mod api;
mod domain;
mod firmware;
mod notifications;
mod service;
mod storage;
mod tasks;

#[event(fetch)]
async fn main(req: Request, env: Env, ctx: Context) -> Result<Response> {
    api::handle(req, env, ctx).await
}

#[event(queue)]
async fn queue(batch: MessageBatch<tasks::Job>, env: Env, _ctx: Context) -> Result<()> {
    tasks::consume(batch, env).await
}

mod scheduled_export {
    use worker::wasm_bindgen::prelude::wasm_bindgen;
    use worker::wasm_bindgen::JsValue;

    #[wasm_bindgen(
        wasm_bindgen = ::worker::wasm_bindgen,
        wasm_bindgen_futures = ::worker::wasm_bindgen_futures,
        js_sys = ::worker::js_sys
    )]
    pub async fn scheduled(
        event: worker::worker_sys::ScheduledEvent,
        env: worker::Env,
        _ctx: worker::worker_sys::ScheduleContext,
    ) -> std::result::Result<JsValue, JsValue> {
        std::panic::AssertUnwindSafe(async move {
            let cron = event.cron()?;
            super::tasks::scheduled(&cron, &env)
                .await
                .map_err(JsValue::from)?;
            Ok(JsValue::UNDEFINED)
        })
        .await
    }
}
