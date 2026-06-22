# Tick Platform-Identity Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Scope:** **Backend only.** This plan covers tap-trading's Rust backend. The client (tap-trading UI) and play-shell integration are handled by a **separate frontend track** — see "Frontend contract (separate track)" below for the wire contract the backend exposes to them; no frontend files are touched here.

**Goal:** Replace tap-trading's unauthenticated `X-Account-Id` header with verified platform-identity JWTs, keying accounts on the platform `sub` and gating the signup bonus to registered users.

**Architecture:** Per ADR-0013. Tick's backend (a standalone Cargo workspace) verifies the platform Ed25519 JWT itself against the identity service's published JWKS (`GET /.well-known/jwks.json`), then produces the **same** `AccountCtx { id, external_id }` request extension that ADR-0009's middleware produced — so no handler changes. The `/stream` WebSocket stays public (market data only). No DB migration: the `accounts` table already has `external_id`/`zklogin_sub`/`zklogin_iss`.

**Tech Stack:** Rust (axum 0.7, sqlx, `jsonwebtoken` for EdDSA, `reqwest` for JWKS fetch, `ed25519-dalek` already in workspace via deps).

**Prerequisite:** This branch must be rebased onto `origin/dev` first — the `identity` service and gateway live there. Do not start work until `cargo build` sees the dev tree.

### Frontend contract (separate track)

The backend's only requirement of the client is: **send `Authorization: Bearer <platform access JWT>`** on every REST request. The token is the same platform JWT the identity service issues (obtained client-side via `@dopamint/sdk` / the play-shell `?token=` hand-off — out of scope here). Backend behavior the frontend can rely on:

- No/invalid/expired token → `401`. A fresh registered-user token → account auto-created with the +10k bonus; a guest token (`kind:guest`) → account at balance 0 (can view, can't stake until signed in).
- `/stream` (WebSocket) needs **no** auth — public price feed.
- The frontend track also owns: the `@dopamint/sdk` sign-in wiring, the play-shell catalog entry (`status: 'live'`, `playUrl`), and the `VITE_*` frontend env vars.

---

## File Structure

**Backend (`games/tap-trading/backend/`)**
- Create: `api/src/auth/mod.rs` — module root re-exporting the verifier + claims.
- Create: `api/src/auth/claims.rs` — local `PlatformClaims` deserialization struct (Tick cannot import `platform-lib-types`; separate workspace).
- Create: `api/src/auth/jwks.rs` — JWKS fetch + cache + Ed25519 `DecodingKey` construction.
- Create: `api/src/auth/verify.rs` — `PlatformJwtVerifier` (verify a bearer token → `PlatformClaims`).
- Create: `api/src/middleware/platform_jwt.rs` — middleware producing `AccountCtx`, bonus gated to `kind=user`.
- Modify: `api/src/account_ctx.rs` — add `kind` + `sui_address` to `AccountCtx` (handlers still read `.id`).
- Modify: `api/src/error.rs` — add JWT error variants (`MissingBearer`, `InvalidToken`, `JwksUnavailable`).
- Modify: `api/src/state.rs` — hold an `Arc<PlatformJwtVerifier>`.
- Modify: `api/src/lib.rs` — swap `account_id_middleware` → `platform_jwt_middleware` in `router()` and the two test routers.
- Modify: `api/src/main.rs` — read identity env vars, build the verifier into `AppState`.
- Modify: `api/Cargo.toml` — add `jsonwebtoken`.
- Modify: `backend/Cargo.toml` — add `jsonwebtoken` to `[workspace.dependencies]`.
- Delete: `api/src/middleware/account_id.rs` and `api/tests/middleware_account_id.rs` (replaced).
- Create: `api/tests/middleware_platform_jwt.rs` — new middleware integration tests.
- Modify: every test that sent `X-Account-Id` (`tests/common/mod.rs`, `get_me.rs`, `post_positions_*.rs`, `idempotency.rs`, `concurrency.rs`, `lock_at_tap.rs`, `rate_limit.rs`, `get_history.rs`, `notify.rs`) — send a signed test JWT instead.

**Backend runtime config (dev env)**
- Modify: `api/.env.example` — document `TAP_IDENTITY_JWKS_URL`, `TAP_IDENTITY_JWT_ISS`, `TAP_IDENTITY_JWT_AUD`.
- Modify: `scripts/worktree-env.sh`, `scripts/sync-service-envs.sh` — export those three vars into the Tick API's env so the backend boots (per the `cmk-worktree-dev-env` skill).

**Out of scope (frontend track):** `games/tap-trading/ui/**`, the play-shell catalog (`platform/ui/play/src/data/games.ts`), and any `VITE_*` frontend env wiring. The backend exposes only the bearer-token contract described in the header.

---

## Task 1: Add the `jsonwebtoken` dependency

**Files:**
- Modify: `games/tap-trading/backend/Cargo.toml` (`[workspace.dependencies]`)
- Modify: `games/tap-trading/backend/api/Cargo.toml` (`[dependencies]`)

- [ ] **Step 1: Add to workspace deps**

In `games/tap-trading/backend/Cargo.toml` under `[workspace.dependencies]`, after the `reqwest` line:

```toml
jsonwebtoken = "9"
base64 = "0.22"
```

- [ ] **Step 2: Reference from the api crate**

In `games/tap-trading/backend/api/Cargo.toml` under `[dependencies]`:

```toml
jsonwebtoken = { workspace = true }
base64 = { workspace = true }
```

- [ ] **Step 3: Verify it resolves**

Run: `cd games/tap-trading/backend && cargo build -p tap-trading-api`
Expected: builds (deps download, no code change yet).

- [ ] **Step 4: Commit**

```bash
git add games/tap-trading/backend/Cargo.toml games/tap-trading/backend/api/Cargo.toml games/tap-trading/backend/Cargo.lock
git commit -m "build(tick): add jsonwebtoken for platform jwt verify"
```

---

## Task 2: Local claims struct

Tick is a separate workspace and cannot import `platform_lib_types::SessionClaims`. Mirror only the fields we read. Field names and `serde` defaults must match the platform's wire format (`sub`, `kind` defaulting to `user`, `sui_address` optional).

> **Gotcha (verified against `origin/dev:platform/backend/api/identity/src/jwt.rs:4-6`):** `iss` and `aud` are **not** fields of `SessionClaims` — issuance wraps the claims in an internal `ClaimsWithIssAud` and encodes *that*, so the real tokens *do* carry `iss`/`aud`. Don't be misled by `SessionClaims` lacking them. Our `Validation::set_issuer`/`set_audience` (Task 4) is therefore correct; jsonwebtoken validates `aud`/`iss` against the raw token independent of our struct. We keep `iss` in `PlatformClaims` only to persist it as `zklogin_iss`; `aud`/`iat`/`game_id` are intentionally omitted (serde ignores unknown claims).

**Files:**
- Create: `games/tap-trading/backend/api/src/auth/mod.rs`
- Create: `games/tap-trading/backend/api/src/auth/claims.rs`
- Test: inline `#[cfg(test)]` in `claims.rs`

- [ ] **Step 1: Write the failing test**

Create `games/tap-trading/backend/api/src/auth/claims.rs`:

```rust
//! Platform JWT payload, mirrored from `platform_lib_types::SessionClaims`.
//! Tick is a separate Cargo workspace, so the type is duplicated rather than
//! shared; only the fields Tick reads are present. Wire compatibility (field
//! names, `kind` default) is load-bearing — see ADR-0013.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    #[default]
    User,
    Guest,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlatformClaims {
    /// Platform user/guest UUID. Becomes `accounts.external_id`.
    pub sub: String,
    #[serde(default)]
    pub kind: SessionKind,
    #[serde(default)]
    pub sui_address: Option<String>,
    pub iss: String,
    pub exp: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_defaults_to_user_when_absent() {
        let json = r#"{"sub":"u-1","iss":"dopamint-identity","exp":9999999999}"#;
        let c: PlatformClaims = serde_json::from_str(json).unwrap();
        assert_eq!(c.kind, SessionKind::User);
        assert_eq!(c.sub, "u-1");
    }

    #[test]
    fn guest_kind_parses() {
        let json = r#"{"sub":"g-1","kind":"guest","iss":"dopamint-identity","exp":1}"#;
        let c: PlatformClaims = serde_json::from_str(json).unwrap();
        assert_eq!(c.kind, SessionKind::Guest);
    }
}
```

Create `games/tap-trading/backend/api/src/auth/mod.rs`:

```rust
pub mod claims;
pub use claims::{PlatformClaims, SessionKind};
```

- [ ] **Step 2: Register the module**

In `games/tap-trading/backend/api/src/lib.rs`, add to the `pub mod` block (after `pub mod aggregator_client;`):

```rust
pub mod auth;
```

- [ ] **Step 3: Run the tests**

Run: `cd games/tap-trading/backend && cargo test -p tap-trading-api auth::claims`
Expected: PASS (2 tests).

- [ ] **Step 4: Commit**

```bash
git add games/tap-trading/backend/api/src/auth games/tap-trading/backend/api/src/lib.rs
git commit -m "feat(tick): add platform jwt claims type"
```

---

## Task 3: JWKS fetch + Ed25519 decoding key

The identity JWKS is an RFC 8037 OKP set: `{"keys":[{"kty":"OKP","crv":"Ed25519","alg":"EdDSA","use":"sig","kid":"...","x":"<base64url 32-byte pubkey>"}]}`. `jsonwebtoken::DecodingKey::from_ed_der` wants DER (SubjectPublicKeyInfo). Wrap the raw 32-byte key with the fixed 12-byte Ed25519 SPKI prefix.

**Files:**
- Create: `games/tap-trading/backend/api/src/auth/jwks.rs`
- Test: inline `#[cfg(test)]` in `jwks.rs`

- [ ] **Step 1: Write the failing test**

Create `games/tap-trading/backend/api/src/auth/jwks.rs`:

```rust
//! Fetches the platform's Ed25519 JWKS and builds a `jsonwebtoken` decoding
//! key. The verifying key rotates rarely; we cache it and refetch only when a
//! token presents an unknown `kid`. ADR-0013.

use std::collections::HashMap;
use std::sync::RwLock;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use jsonwebtoken::DecodingKey;
use serde::Deserialize;

use crate::error::ApiError;

/// Fixed DER SubjectPublicKeyInfo prefix for an Ed25519 public key (RFC 8410).
const ED25519_SPKI_PREFIX: [u8; 12] =
    [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];

#[derive(Debug, Deserialize)]
struct Jwk {
    kid: String,
    x: String,
}

#[derive(Debug, Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

/// Convert an RFC 8037 OKP `x` (base64url, raw 32-byte key) into a
/// `jsonwebtoken` `DecodingKey`.
fn decoding_key_from_x(x: &str) -> Result<DecodingKey, ApiError> {
    let raw = URL_SAFE_NO_PAD.decode(x).map_err(|_| ApiError::JwksUnavailable)?;
    if raw.len() != 32 {
        return Err(ApiError::JwksUnavailable);
    }
    let mut der = Vec::with_capacity(44);
    der.extend_from_slice(&ED25519_SPKI_PREFIX);
    der.extend_from_slice(&raw);
    Ok(DecodingKey::from_ed_der(&der))
}

/// Parse a JWKS JSON body into `kid → DecodingKey`.
pub fn parse_jwks(body: &str) -> Result<HashMap<String, DecodingKey>, ApiError> {
    let jwks: Jwks = serde_json::from_str(body).map_err(|_| ApiError::JwksUnavailable)?;
    let mut out = HashMap::new();
    for jwk in jwks.keys {
        out.insert(jwk.kid.clone(), decoding_key_from_x(&jwk.x)?);
    }
    Ok(out)
}

/// In-memory cache of `kid → DecodingKey`, refilled from `jwks_url` on miss.
pub struct JwksCache {
    jwks_url: String,
    http: reqwest::Client,
    keys: RwLock<HashMap<String, DecodingKey>>,
}

impl JwksCache {
    pub fn new(jwks_url: String, http: reqwest::Client) -> Self {
        Self { jwks_url, http, keys: RwLock::new(HashMap::new()) }
    }

    /// Return the decoding key for `kid`, fetching the JWKS once if absent.
    pub async fn key_for(&self, kid: &str) -> Result<DecodingKey, ApiError> {
        if let Some(k) = self.keys.read().unwrap().get(kid).cloned() {
            return Ok(k);
        }
        let body = self
            .http
            .get(&self.jwks_url)
            .send()
            .await
            .map_err(|_| ApiError::JwksUnavailable)?
            .text()
            .await
            .map_err(|_| ApiError::JwksUnavailable)?;
        let fresh = parse_jwks(&body)?;
        let key = fresh.get(kid).cloned().ok_or(ApiError::InvalidToken)?;
        *self.keys.write().unwrap() = fresh;
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A known-good Ed25519 JWKS (kid "k1") generated for tests.
    const SAMPLE: &str = r#"{"keys":[{"kty":"OKP","crv":"Ed25519","alg":"EdDSA","use":"sig","kid":"k1","x":"11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo"}]}"#;

    #[test]
    fn parses_okp_jwks_into_one_key() {
        let keys = parse_jwks(SAMPLE).unwrap();
        assert!(keys.contains_key("k1"));
    }

    #[test]
    fn rejects_malformed_x() {
        let bad = r#"{"keys":[{"kty":"OKP","crv":"Ed25519","alg":"EdDSA","use":"sig","kid":"k1","x":"@@@"}]}"#;
        assert!(parse_jwks(bad).is_err());
    }
}
```

> Note: the `x` in `SAMPLE` is the RFC 8037 test vector (32 bytes). If `from_ed_der` rejects it at parse time in a future `jsonwebtoken`, regenerate with `ed25519-dalek` in the test and base64url-encode the verifying key bytes.

- [ ] **Step 2: Add the module + error variants (Task 4 adds the variants; stub here)**

Add `pub mod jwks;` to `games/tap-trading/backend/api/src/auth/mod.rs`. The `ApiError::JwksUnavailable` / `InvalidToken` variants are added in Task 4 — if running Task 3 first, add them now (see Task 4 Step 1).

- [ ] **Step 3: Run the tests**

Run: `cd games/tap-trading/backend && cargo test -p tap-trading-api auth::jwks`
Expected: PASS (2 tests).

- [ ] **Step 4: Commit**

```bash
git add games/tap-trading/backend/api/src/auth/jwks.rs games/tap-trading/backend/api/src/auth/mod.rs
git commit -m "feat(tick): fetch and cache platform jwks"
```

---

## Task 4: Error variants + the verifier

**Files:**
- Modify: `games/tap-trading/backend/api/src/error.rs`
- Create: `games/tap-trading/backend/api/src/auth/verify.rs`
- Test: inline `#[cfg(test)]` in `verify.rs`

- [ ] **Step 1: Add error variants**

In `games/tap-trading/backend/api/src/error.rs`, add to the `ApiError` enum (match the existing `thiserror` style and the `IntoResponse` impl — mirror how `MissingAccountId`/`InvalidAccountId` map to 401/400):

```rust
#[error("missing bearer token")]
MissingBearer,
#[error("invalid token")]
InvalidToken,
#[error("identity jwks unavailable")]
JwksUnavailable,
```

In the `IntoResponse` (or status-mapping) impl, map:
- `MissingBearer` → `401` code `"missing_token"`
- `InvalidToken` → `401` code `"invalid_token"`
- `JwksUnavailable` → `503` code `"identity_unavailable"`

- [ ] **Step 2: Write the failing test + verifier**

Create `games/tap-trading/backend/api/src/auth/verify.rs`:

```rust
//! Verifies a platform bearer token end-to-end: pulls the `kid` from the
//! header, fetches the matching Ed25519 key, validates signature + `exp` +
//! `iss` + `aud`. ADR-0013.

use std::sync::Arc;

use jsonwebtoken::{decode, decode_header, Algorithm, Validation};

use crate::auth::claims::PlatformClaims;
use crate::auth::jwks::JwksCache;
use crate::error::ApiError;

pub struct PlatformJwtVerifier {
    jwks: Arc<JwksCache>,
    expected_iss: String,
    expected_aud: String,
}

impl PlatformJwtVerifier {
    pub fn new(jwks: Arc<JwksCache>, expected_iss: String, expected_aud: String) -> Self {
        Self { jwks, expected_iss, expected_aud }
    }

    /// Verify a raw bearer token (no `Bearer ` prefix) and return its claims.
    pub async fn verify(&self, token: &str) -> Result<PlatformClaims, ApiError> {
        let header = decode_header(token).map_err(|_| ApiError::InvalidToken)?;
        let kid = header.kid.ok_or(ApiError::InvalidToken)?;
        let key = self.jwks.key_for(&kid).await?;

        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.set_issuer(&[self.expected_iss.as_str()]);
        validation.set_audience(&[self.expected_aud.as_str()]);
        validation.leeway = 30;

        let data = decode::<PlatformClaims>(token, &key, &validation)
            .map_err(|_| ApiError::InvalidToken)?;
        Ok(data.claims)
    }
}

#[cfg(test)]
mod tests {
    // Sign a token with a generated Ed25519 key, expose the matching JWKS via
    // a `wiremock` server, and assert: valid token → claims; expired → err;
    // wrong iss → err; unknown kid → err. See tests/common for the keygen
    // helper added in Task 6.
}
```

Add `pub mod verify;` and `pub use verify::PlatformJwtVerifier;` to `auth/mod.rs`.

- [ ] **Step 3: Run**

Run: `cd games/tap-trading/backend && cargo build -p tap-trading-api`
Expected: builds. (Verifier unit tests are exercised via the middleware integration tests in Task 6, which have the keygen helper.)

- [ ] **Step 4: Commit**

```bash
git add games/tap-trading/backend/api/src/error.rs games/tap-trading/backend/api/src/auth
git commit -m "feat(tick): platform jwt verifier"
```

---

## Task 5: `AccountCtx` carries identity; state holds the verifier

**Files:**
- Modify: `games/tap-trading/backend/api/src/account_ctx.rs`
- Modify: `games/tap-trading/backend/api/src/state.rs`

- [ ] **Step 1: Extend `AccountCtx`**

`games/tap-trading/backend/api/src/account_ctx.rs` — add identity fields handlers may later use (handlers keep reading `.id`):

```rust
//! Authenticated request context. Attached by the platform-jwt middleware.

use crate::auth::SessionKind;

#[derive(Debug, Clone)]
pub struct AccountCtx {
    pub id: i64,
    /// Platform `sub` (user/guest UUID). Stored as `accounts.external_id`.
    pub external_id: String,
    pub kind: SessionKind,
    pub sui_address: Option<String>,
}
```

- [ ] **Step 2: Hold the verifier in `AppState`**

In `games/tap-trading/backend/api/src/state.rs`, add to `AppState` (match the existing field/clone style — `AppState` is `Clone`):

```rust
pub verifier: std::sync::Arc<crate::auth::PlatformJwtVerifier>,
```

- [ ] **Step 3: Build**

Run: `cd games/tap-trading/backend && cargo build -p tap-trading-api`
Expected: fails only where `AppState` is constructed (main.rs, test common) — fixed in Tasks 7 & 6. The `account_ctx.rs` change compiles.

- [ ] **Step 4: Commit**

```bash
git add games/tap-trading/backend/api/src/account_ctx.rs games/tap-trading/backend/api/src/state.rs
git commit -m "feat(tick): carry platform identity on account ctx"
```

---

## Task 6: The platform-JWT middleware (+ replace ADR-0009 tests)

This is the core swap. The middleware reads `Authorization: Bearer`, verifies, then runs the same lookup-or-create as ADR-0009 — except the bonus branch is gated to `kind=user` and there is no anonymous path.

**Files:**
- Create: `games/tap-trading/backend/api/src/middleware/platform_jwt.rs`
- Modify: `games/tap-trading/backend/api/src/middleware/mod.rs`
- Delete: `games/tap-trading/backend/api/src/middleware/account_id.rs`
- Modify: `games/tap-trading/backend/api/tests/common/mod.rs` (keygen + signed-token helper + JWKS wiremock)
- Delete: `games/tap-trading/backend/api/tests/middleware_account_id.rs`
- Create: `games/tap-trading/backend/api/tests/middleware_platform_jwt.rs`

- [ ] **Step 1: Write the failing middleware tests**

Create `games/tap-trading/backend/api/tests/middleware_platform_jwt.rs`. These encode ADR-0013's intent — *why*, not just *what*:

```rust
//! Platform-JWT middleware (ADR-0013). The middleware exists to make Tick
//! fail closed: only a token the identity service signed grants an account,
//! and the +10k bonus is reserved for registered users so guest tokens can't
//! farm it.

mod common;
use common::{signed_token, TestApp};

#[tokio::test]
async fn missing_bearer_returns_401() {
    let app = TestApp::spawn().await;
    let res = app.get_me_raw(None).await;
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn invalid_signature_returns_401() {
    let app = TestApp::spawn().await;
    let forged = signed_token(&app.wrong_key, "u-1", "user"); // signed by a key not in JWKS
    let res = app.get_me_raw(Some(&forged)).await;
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn valid_user_token_lazy_creates_account_with_signup_bonus() {
    let app = TestApp::spawn().await;
    let token = signed_token(&app.key, "user-aaa", "user");
    let me = app.get_me(&token).await;
    assert_eq!(me.balance, 10_000, "registered users get the SIGNUP bonus once");
    // ledger has exactly one SIGNUP row
    assert_eq!(app.signup_rows("user-aaa").await, 1);
}

#[tokio::test]
async fn guest_token_creates_account_without_bonus() {
    let app = TestApp::spawn().await;
    let token = signed_token(&app.key, "guest-bbb", "guest");
    let me = app.get_me(&token).await;
    assert_eq!(me.balance, 0, "guests must not receive the bonus (farming guard)");
    assert_eq!(app.signup_rows("guest-bbb").await, 0);
}

#[tokio::test]
async fn second_call_reuses_account_and_does_not_re_grant() {
    let app = TestApp::spawn().await;
    let token = signed_token(&app.key, "user-ccc", "user");
    let _ = app.get_me(&token).await;
    let me2 = app.get_me(&token).await;
    assert_eq!(me2.balance, 10_000);
    assert_eq!(app.signup_rows("user-ccc").await, 1, "bonus is one-time");
}
```

- [ ] **Step 2: Add test helpers**

In `games/tap-trading/backend/api/tests/common/mod.rs`, add (alongside the existing Postgres/Redis container setup):
- An Ed25519 keypair generated via `ed25519-dalek`, plus a second "wrong" key.
- A `wiremock` server serving `GET /.well-known/jwks.json` with the OKP JWK for the good key (`kid="test"`, `x=` base64url of the verifying key bytes).
- `signed_token(signing_key, sub, kind)` → a JWT with header `{alg:"EdDSA", kid:"test"}` and claims `{sub, kind, iss:"dopamint-identity", aud:"dopamint-platform", exp: now+3600}`, signed via `jsonwebtoken::encode` with `EncodingKey::from_ed_der`.
- Build `AppState.verifier` pointing at the wiremock JWKS URL with iss `"dopamint-identity"`, aud `"dopamint-platform"`.
- `get_me(token)` / `get_me_raw(Option<token>)` send `Authorization: Bearer` instead of `X-Account-Id`.
- `signup_rows(sub)` → `SELECT count(*) FROM points_ledger pl JOIN accounts a ON a.id = pl.account_id WHERE a.external_id = $1 AND pl.kind = 'SIGNUP'`.

- [ ] **Step 3: Run the tests (verify they fail)**

Run: `cd games/tap-trading/backend && cargo test -p tap-trading-api --test middleware_platform_jwt`
Expected: FAIL to compile (no `platform_jwt` middleware yet).

- [ ] **Step 4: Implement the middleware**

Create `games/tap-trading/backend/api/src/middleware/platform_jwt.rs`:

```rust
//! Platform-JWT middleware. ADR-0013 — replaces the `X-Account-Id` stand-in
//! (ADR-0009 §1). Verifies the bearer token against the identity JWKS, then
//! lazy-creates the Tick account keyed on the verified `sub`. The +10k SIGNUP
//! bonus is granted only to `kind=user`; there is no anonymous path, so the
//! middleware fails closed (ADR-0009 named the old lazy-create a footgun).

use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::Response;

use crate::account_ctx::AccountCtx;
use crate::auth::SessionKind;
use crate::error::ApiError;
use crate::state::AppState;

const SIGNUP_BONUS: i64 = 10_000;

pub async fn platform_jwt_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(ApiError::MissingBearer)?;

    let claims = state.verifier.verify(token).await?;
    let ctx = lookup_or_create(&state, &claims.sub, claims.kind, &claims.iss).await?;
    let ctx = AccountCtx { sui_address: claims.sui_address.clone(), ..ctx };
    req.extensions_mut().insert(ctx);
    Ok(next.run(req).await)
}

/// Fast path: existing account by `external_id` (the verified `sub`). Slow
/// path: insert + (for registered users only) the one-time SIGNUP ledger row.
async fn lookup_or_create(
    state: &AppState,
    sub: &str,
    kind: SessionKind,
    iss: &str,
) -> Result<AccountCtx, ApiError> {
    if let Some((id,)) =
        sqlx::query_as::<_, (i64,)>("SELECT id FROM accounts WHERE external_id = $1")
            .bind(sub)
            .fetch_optional(&state.pg)
            .await?
    {
        return Ok(AccountCtx { id, external_id: sub.to_string(), kind, sui_address: None });
    }

    let now = state.clock.now_ms();
    let mut tx = state.pg.begin().await?;

    let inserted: Option<(i64,)> = sqlx::query_as(
        r#"
        INSERT INTO accounts
          (external_id, zklogin_sub, zklogin_iss, balance, lifetime_points_won,
           signup_bonus_at_ms, created_at_ms, last_active_ms)
        VALUES ($1, $1, $2, $3, 0, $4, $4, $4)
        ON CONFLICT (external_id) DO NOTHING
        RETURNING id
        "#,
    )
    .bind(sub)
    .bind(iss)
    .bind(if kind == SessionKind::User { SIGNUP_BONUS } else { 0 })
    .bind(now)
    .fetch_optional(&mut *tx)
    .await?;

    let id = if let Some((id,)) = inserted {
        if kind == SessionKind::User {
            sqlx::query(
                r#"INSERT INTO points_ledger (account_id, kind, delta, ref_id, created_at_ms)
                   VALUES ($1, 'SIGNUP', $2, NULL, $3)"#,
            )
            .bind(id)
            .bind(SIGNUP_BONUS)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        id
    } else {
        let (id,): (i64,) =
            sqlx::query_as("SELECT id FROM accounts WHERE external_id = $1")
                .bind(sub)
                .fetch_one(&mut *tx)
                .await?;
        id
    };
    tx.commit().await?;
    Ok(AccountCtx { id, external_id: sub.to_string(), kind, sui_address: None })
}
```

In `middleware/mod.rs`: replace `pub mod account_id;` with `pub mod platform_jwt;`. Delete `account_id.rs`.

- [ ] **Step 5: Run the tests (verify they pass)**

Run: `cd games/tap-trading/backend && cargo test -p tap-trading-api --test middleware_platform_jwt`
Expected: PASS (5 tests). Then delete `tests/middleware_account_id.rs`.

- [ ] **Step 6: Commit**

```bash
git add games/tap-trading/backend/api/src/middleware games/tap-trading/backend/api/tests/middleware_platform_jwt.rs games/tap-trading/backend/api/tests/common/mod.rs
git rm games/tap-trading/backend/api/src/middleware/account_id.rs games/tap-trading/backend/api/tests/middleware_account_id.rs
git commit -m "feat(tick): verify platform jwt, gate signup bonus to users"
```

---

## Task 7: Wire the middleware into the router + main

**Files:**
- Modify: `games/tap-trading/backend/api/src/lib.rs:26-99` (3 routers)
- Modify: `games/tap-trading/backend/api/src/main.rs`

- [ ] **Step 1: Swap the middleware in all three routers**

In `lib.rs`, replace every `middleware::account_id::account_id_middleware` with `middleware::platform_jwt::platform_jwt_middleware` (in `router_with_rate_limit_probe`, `router_without_rate_limit`, and `router`). The `/stream` route stays in the `public` router with no auth layer (ADR-0013 — public market data).

- [ ] **Step 2: Build the verifier in main**

In `main.rs`, read env and construct the verifier before building `AppState`:

```rust
let jwks_url = std::env::var("TAP_IDENTITY_JWKS_URL")
    .expect("TAP_IDENTITY_JWKS_URL is required");
let iss = std::env::var("TAP_IDENTITY_JWT_ISS").expect("TAP_IDENTITY_JWT_ISS is required");
let aud = std::env::var("TAP_IDENTITY_JWT_AUD").expect("TAP_IDENTITY_JWT_AUD is required");
let jwks = std::sync::Arc::new(crate::auth::jwks::JwksCache::new(
    jwks_url,
    reqwest::Client::new(),
));
let verifier = std::sync::Arc::new(crate::auth::PlatformJwtVerifier::new(jwks, iss, aud));
```

Pass `verifier` into the `AppState { … }` constructor.

- [ ] **Step 3: Run the full backend suite**

Run: `cd games/tap-trading/backend && cargo test -p tap-trading-api`
Expected: PASS. (Tasks 6's helper already migrated `tests/common`; any test still sending `X-Account-Id` must be switched to `Bearer signed_token(...)` — fix each compile error by replacing the header.)

- [ ] **Step 4: Commit**

```bash
git add games/tap-trading/backend/api/src/lib.rs games/tap-trading/backend/api/src/main.rs games/tap-trading/backend/api/tests
git commit -m "feat(tick): mount platform-jwt middleware, drop x-account-id"
```

---

## Task 8: Backend runtime config + smoke test

`main.rs` (Task 7) reads three identity env vars. Wire them into the dev-env
scripts so the backend boots, document them, then smoke-test against a real
token. (Frontend/SDK wiring is a separate track — see the header contract.)

**Files:**
- Modify: `games/tap-trading/backend/api/.env.example`
- Modify: `scripts/worktree-env.sh`, `scripts/sync-service-envs.sh`

- [ ] **Step 1: Document the vars**

In `games/tap-trading/backend/api/.env.example`, add:

```
# Platform identity (ADR-0013). JWKS is the identity service's public-key
# endpoint; iss/aud must match what the identity service mints.
TAP_IDENTITY_JWKS_URL=http://localhost:3023/.well-known/jwks.json
TAP_IDENTITY_JWT_ISS=dopamint-identity
TAP_IDENTITY_JWT_AUD=dopamint-platform
```

- [ ] **Step 2: Export them per-worktree**

Following the `cmk-worktree-dev-env` skill, add the three exports for the Tick
API in `scripts/worktree-env.sh` (derive the JWKS host/port from the identity
service's worktree-calculated port — never a literal), and write them into the
Tick API's `.env` in `scripts/sync-service-envs.sh`.

- [ ] **Step 3: Smoke test against a real token**

Run `./scripts/init-worktree-dev.sh`, start the identity service + Tick API.
Mint a token and call Tick:

```bash
# guest token → account at balance 0 (can view, can't stake)
curl -s -H "Authorization: Bearer $GUEST_JWT" http://localhost:$TAP_API_PORT/v1/me
# no token → 401
curl -s -o /dev/null -w '%{http_code}\n' http://localhost:$TAP_API_PORT/v1/me
```

Expected: bearer → `200` with the account; no bearer → `401`; a first-time
registered-user token → balance `10000`.

- [ ] **Step 4: Commit**

```bash
git add games/tap-trading/backend/api/.env.example scripts/worktree-env.sh scripts/sync-service-envs.sh
git commit -m "build(tick): wire identity jwks env for the api"
```

---

## Task 9: Cross-reference ADR-0009

**Files:**
- Modify: `docs/decisions/0009-tick-api-cross-service-contracts.md` (§1 + Forecast)

- [ ] **Step 1:** Add a note at the top of §1 and the Forecast "Real auth" bullet: *"Superseded by ADR-0013 — `X-Account-Id` is replaced by platform-JWT verification; the `AccountCtx` contract is unchanged as forecast."*

- [ ] **Step 2: Commit**

```bash
git add docs/decisions/0009-tick-api-cross-service-contracts.md
git commit -m "docs(tick): note ADR-0013 supersedes the x-account-id stand-in"
```

---

## Self-Review

- **Backend scope only.** Client/SDK sign-in, the play-shell catalog entry, and `VITE_*` env are the **frontend track's** job — the backend just exposes the bearer-token contract (see header). No `games/tap-trading/ui/**` or `platform/ui/**` files are touched here.
- **Spec coverage (ADR-0013, backend half):** verify JWT (Tasks 3,4,6) ✓; key on `sub` (Task 6) ✓; gate bonus to users (Task 6) ✓; economies stay separate (no points-service code touched) ✓; `/stream` stays public (Task 7 Step 1) ✓; no migration (Task 6 uses existing columns) ✓; runtime env + smoke test (Task 8) ✓; supersede note (Task 9) ✓.
- **Type consistency:** `AccountCtx { id, external_id, kind, sui_address }` defined in Task 5, constructed in Task 6, read by handlers via `.id` (unchanged). `PlatformClaims { sub, kind, sui_address, iss, exp }` (Task 2) consumed by `verify` (Task 4) and the middleware (Task 6). `JwksCache`/`PlatformJwtVerifier` names consistent across Tasks 3–8.
- **Login-to-trade is intended (ADR-0013).** Guests get a verified identity at balance 0 → `POST /v1/positions` returns `insufficient_balance` until they sign in. The backend enforces this; the watch-only UX (chart visible to guests via the public WS) is the frontend track's concern.
- **Known follow-ups (out of scope, flagged):** seamless mid-session token refresh in a cross-origin embed (frontend track); guest→user account link/merge (blocked on identity exposing the linkage); optional platform-points bridge on settlement (ADR-0006 HMAC path).
