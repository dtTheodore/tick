//! Per-account token-bucket rate limiter. ADR-0009 §6.
//!
//! Bucket: hash `tap:rl:{account_id}` with fields `tokens` (f64) and `ts_ms`
//! (i64). Capacity 10. Refill 10 tokens/sec linear (1 token / 100 ms).
//! On each call: refill = (now_ms - ts_ms) * 10 / 1000; new = min(cap, old + refill);
//! if new >= 1 → consume 1, write back, allow; else → reject with the
//! milliseconds-until-1 as `Retry-After` (ceil to seconds, minimum 1).

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

use crate::account_ctx::AccountCtx;
use crate::error::ApiError;
use crate::state::AppState;

const CAP: f64 = 10.0;
const REFILL_PER_SEC: f64 = 10.0;
const TTL_SECS: u64 = 2;

// KEYS[1] = bucket key, ARGV[1] = now_ms, ARGV[2] = cap, ARGV[3] = refill_per_sec, ARGV[4] = ttl
// Returns: { allowed (0|1), retry_after_ms }
const SCRIPT: &str = r#"
local key = KEYS[1]
local now_ms = tonumber(ARGV[1])
local cap = tonumber(ARGV[2])
local refill_per_sec = tonumber(ARGV[3])
local ttl = tonumber(ARGV[4])

local data = redis.call('HMGET', key, 'tokens', 'ts_ms')
local tokens = tonumber(data[1])
local ts_ms = tonumber(data[2])
if tokens == nil then
  tokens = cap
  ts_ms = now_ms
end
local elapsed = math.max(0, now_ms - ts_ms)
local refilled = math.min(cap, tokens + (elapsed * refill_per_sec) / 1000.0)
local allowed = 0
local retry_after_ms = 0
if refilled >= 1.0 then
  refilled = refilled - 1.0
  allowed = 1
else
  retry_after_ms = math.ceil((1.0 - refilled) * 1000.0 / refill_per_sec)
end
redis.call('HSET', key, 'tokens', tostring(refilled), 'ts_ms', tostring(now_ms))
redis.call('EXPIRE', key, ttl)
return { allowed, retry_after_ms }
"#;

pub async fn rate_limit_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let ctx = req
        .extensions()
        .get::<AccountCtx>()
        .cloned()
        .ok_or(ApiError::MissingAccountId)?;
    let mut redis = state.redis.clone();
    let key = format!("tap:rl:{}", ctx.id);
    let res: Vec<i64> = redis::Script::new(SCRIPT)
        .key(key)
        .arg(state.clock.now_ms())
        .arg(CAP)
        .arg(REFILL_PER_SEC)
        .arg(TTL_SECS)
        .invoke_async(&mut redis)
        .await?;
    let allowed = res.first().copied().unwrap_or(0);
    if allowed == 0 {
        let retry_after_ms = res.get(1).copied().unwrap_or(1000);
        let secs = ((retry_after_ms as u64).saturating_add(999)) / 1000;
        return Err(ApiError::RateLimited {
            retry_after_secs: secs.max(1),
        });
    }
    Ok(next.run(req).await)
}
