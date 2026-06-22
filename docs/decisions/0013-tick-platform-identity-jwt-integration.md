# ADR-0013 — Tick adopts platform identity via JWT verification

**Date:** 2026-06-08
**Status:** Accepted
**Workstream:** Tick (tap-trading)
**Supersedes:** ADR-0009 §1 (the `X-Account-Id` dev stand-in)
**Superseded by:** —
**Implementation:** split — the **backend** (JWT verification + account keying)
is built in `docs/superpowers/plans/2026-06-08-tick-platform-identity-integration.md`;
the **client** (SDK sign-in, play-shell launch) is a separate **frontend track**.
The "Client side" paragraph below is the contract between them, not backend work.

## Context

ADR-0009 §1 shipped a deliberate placeholder for player identity: every API
request carries `X-Account-Id: <text>`, the middleware lazy-creates an
`accounts` row keyed on that header (`external_id`), and a first sighting
grants a one-time **+10,000-point SIGNUP** ledger entry. ADR-0009 named this a
footgun and prescribed the fix verbatim: *"Swap-in for real auth must delete
the lazy-create branch entirely … so the middleware fails closed,"* and *"the
JWT verifier replaces the middleware and writes real values"* into the
`zklogin_sub` / `zklogin_iss` columns. The `AccountCtx { id, external_id }`
request-extension contract was designed to be stable across that swap so
handlers never change.

That swap is now unblocked. `origin/dev` landed the platform `identity`
service: Google OAuth → Ed25519 (`EdDSA`) JWTs, guest tokens, and a public
**`GET /.well-known/jwks.json`** (RFC 8037 OKP JWK) intended for external
services to verify tokens themselves. The play shell launches games in an
iframe and hands the access token in via a `?token=<jwt>` query param.

Today's state is not merely "unintegrated" — it is **unauthenticated**: any
client can POST as any `external_id`, draining or inflating another account's
balance, and can mint unlimited +10k bonuses. Closing this is the motivation.

Three facts shaped the decision:

- **Tick's backend is a standalone Cargo workspace**, not a member of the root
  workspace. It cannot share the gateway's in-process `SessionClaims` type or
  middleware. It must verify the JWT itself against the published JWKS — which
  is exactly the external-integrator path the JWKS endpoint exists to serve.
- **Tick's `/stream` WebSocket is a public oracle price feed** (`stream.rs`
  takes no account context; it rebroadcasts ETH ticks to anyone). Identity
  rides only the REST surface. So this integration touches REST auth only;
  there is no long-lived authenticated socket to keep alive.
- **The `accounts` schema already has `external_id`, `zklogin_sub`,
  `zklogin_iss`** (ADR-0009 added them as `NOT NULL`, sentinel `'dev'`). The
  swap writes real values into existing columns — **no migration**.

## Decision

Replace the `X-Account-Id` middleware with a **platform-JWT middleware** that
fails closed, keeping the downstream `AccountCtx` contract unchanged.

1. **Verify, don't trust.** Read `Authorization: Bearer <jwt>`. Verify the
   Ed25519 signature against the identity service's JWKS (fetched once and
   cached, refreshed on unknown `kid`), and validate `exp`, `iss`, and `aud`.
   Reject anything else with `401`. There is no anonymous lazy-create path.
2. **Key accounts on the verified `sub`.** The platform user/guest UUID
   becomes `accounts.external_id`; `zklogin_sub = sub`, `zklogin_iss =` the
   token issuer. Handlers continue to read `AccountCtx.id` and never see the
   token. Account lookup stays a single `SELECT … WHERE external_id = $1` —
   the same access pattern as today, now on a verified key.
3. **Gate the SIGNUP bonus to registered users.** The +10k is granted only
   when `kind = user`. Guests (`kind = guest`) start at 0 balance. This closes
   the bonus-farming vector — a guest token is cheap to mint, a Google account
   is not — and avoids double-granting across the guest→user upgrade (see
   Consequences).
4. **Keep Tick's economy separate from platform points.** Tick's
   `balance` / `points_ledger` remain its own gameplay currency; the platform
   `points` service is the distinct cross-game reward currency. No bridge in
   this slice. (A future ADR may award platform points on settlement via the
   ADR-0006 service-auth HMAC `points.award` path.)
5. **Leave `/stream` unauthenticated.** It serves public market data only.

Client side (separate frontend track — stated here as the contract): the Tick
UI signs in **through `@dopamint/sdk`**
(`createSessionBoot` + `DopamintClient`), which walks the standard token chain
(play-shell `?token=` → masterkey-cookie exchange → cached token → guest mint).
It takes `boot.token` (the access JWT) and sends it as `Authorization: Bearer`
to Tick's *own* backend. On a `401`, it re-runs the SDK boot to obtain a fresh
token via the SDK's refresh/masterkey path and retries once. (Seamless
mid-session refresh inside a cross-origin embed — where the iframe only ever
sees the shell's `?token=` and the masterkey cookie may not ride — is a flagged
follow-up; the worst case is re-entry from the shell on reload.)

## Consequences

- **Security:** the spoof/drain and bonus-farm vectors of ADR-0009 §1 are
  closed; the middleware fails closed on a missing/invalid token.
- **Tick is login-to-trade (intended).** The minimum stake is 50 points
  (`STAKE_TIERS_V1`) and `POST /v1/positions` rejects a stake when
  `balance < stake` (`positions.rs:134`). A guest starts at balance 0, so a
  guest **cannot place a trade** — they can watch the public price feed and
  hold a verified account, but must sign in (via the SDK; a Google account for
  a real balance) to play. This is the deliberate anti-farming stance: a guest
  token is cheap to mint, so granting it stake-able points would reopen the
  farm. The SDK is the sign-in path, per the product decision on this slice.
- **Guest→user upgrade is lossy (accepted for now).** Guest JWTs carry
  `sub = guest_uuid`, user JWTs `sub = user_uuid`, and the OAuth callback
  deletes the guest record; the identity guest-upgrade spec explicitly carries
  **no balance/inventory migration** (deferred to a future "Workstream C").
  Because we key on `sub`, a guest who logs in gets a fresh Tick account — but
  since guests have no stake-able balance, there is nothing to lose and no
  double-bonus. If guest balances ever become load-bearing, a
  `guest_uuid → user_uuid` link/merge step is required and depends on identity
  exposing that linkage (it does not yet).
- **No schema change.** Existing columns absorb the real values; ADR-0009's
  "genesis migration is closed — every further change is a new file" rule is
  not triggered.
- **Coupling:** Tick gains a build dependency on `jsonwebtoken` and a runtime
  dependency on the identity service's JWKS URL (new env config). It does
  **not** join the root workspace.
- **Tests:** the ADR-0009 middleware tests
  (`missing_header_returns_401`, `unknown_header_lazy_creates_account_and_signup_ledger`,
  …) are replaced by JWT-middleware tests: rejects missing/expired/wrong-`iss`
  tokens, accepts a valid token and attaches `AccountCtx`, grants the bonus to
  `user` once, withholds it from `guest`.

## Alternatives considered

- **Client derives `X-Account-Id` from the platform `sub`** (call `auth.me()`,
  send the UUID as the header). Zero backend change, but still trusts a
  client-supplied identity — the spoof vector stays open. Unacceptable for an
  account with a points economy.
- **Join the root workspace and reuse a shared verifier crate.** Tighter
  coupling and a larger blast radius for a game that is otherwise a
  self-contained external backend; the platform's verify logic is server-side,
  not packaged as a reusable library. Rejected in favor of standalone JWKS
  verification, which is what the public JWKS endpoint is for.
- **Bridge Tick's economy onto platform points now.** Out of scope; the two
  currencies are conceptually distinct and bridging is a separable product
  decision (see Decision §4).
