// Cloudflare's API, for the Worker's logs alone (docs/observability.md): a query of Workers
// Observability's telemetry with the owner's read-only token, `cloudflareLogsToken` in
// credentials.json. The token goes to api.cloudflare.com and nowhere else: not to output, a
// log or an error.

use serde_json::{Value, json};
use ureq::Agent;

use crate::error::Error;
use crate::http::post_json;
use crate::plain::Id;

const API: &str = "https://api.cloudflare.com/client/v4";

// Events per query, Cloudflare's own cap.
pub const LIMIT: usize = 2000;

// One field's value, as the Worker logs it (src/worker/log.ts).
pub struct Filter<'a> {
    pub key: &'a str,
    pub value: &'a str,
}

// The query's body: the Worker's events from `from_ms` to `to_ms` with any of `filters`.
pub fn query(filters: &[Filter], from_ms: i64, to_ms: i64) -> Value {
    let filters: Vec<Value> = filters.iter().map(|f| json!({ "key": f.key, "operation": "eq", "type": "string", "value": f.value })).collect();
    json!({
        "queryId": "tolkster-logs",
        "view": "events",
        "limit": LIMIT,
        "timeframe": { "from": from_ms, "to": to_ms },
        "parameters": { "filters": filters, "filterCombination": "or" },
    })
}

// The events `body` finds, as Cloudflare returns them. Its error text is left out: it can
// echo the request.
pub fn events(agent: &Agent, account: &Id, token: &str, body: &Value) -> Result<Vec<Value>, Error> {
    let url = format!("{API}/accounts/{account}/workers/observability/telemetry/query");
    let (status, text) = post_json(agent, &url, token, &[], &body.to_string()).map_err(|_| Error::Unavailable("couldn't reach Cloudflare's API".into()))?;
    match status {
        200 => {}
        401 | 403 => return Err(Error::Failed("Cloudflare refused cloudflareLogsToken: it needs Workers Observability read on cloudflareAccountId's account".into())),
        500.. => return Err(Error::Unavailable(format!("Cloudflare's API answered {status}"))),
        _ => return Err(Error::Failed(format!("Cloudflare's API answered {status}"))),
    }
    let mut answer: Value = serde_json::from_str(&text).map_err(|_| shape())?;
    match answer.pointer_mut("/result/events/events").map(Value::take) {
        Some(Value::Array(events)) => Ok(events),
        // No events at all comes back without the list.
        None | Some(Value::Null) if answer["success"] == true => Ok(Vec::new()),
        _ => Err(shape()),
    }
}

fn shape() -> Error {
    Error::Failed("Cloudflare's API answered in another shape".into())
}
