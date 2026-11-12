# Product Requirements Document
## Topcoat To-Do (working name: "Recoat")

**Version:** 1.0
**Date:** 2026-11-09
**Author:** TBD
**Status:** Draft

> **Revision, 2026-11-12: the database is now Microsoft SQL Server, not SQLite + Toasty.** The
> data layer was rewritten as hand-written, parameterized T-SQL on Microsoft's `mssql-tds` driver
> (the `mssql-rs` repository, via the `mssql-tiberius-bridge` pooling wrapper), with SQL Server
> 2022 running in a Docker container. Toasty has no SQL Server driver, so it was dropped.
> Wherever the text below says SQLite, Toasty, `data.db` or `toasty migration`, read "SQL Server",
> the `db` module, the `todoapp` database and the numbered files in `migrations/`. The slice
> history below is kept as it was written. See `todo-app/README.md` and
> `todo-app/docs/DEPLOYMENT.md` for the current setup.

> **Assumptions made (no interview round was held; please correct):** single developer / showcase POC; local-first deployment; SQLite for the database; lightweight email+password auth; no AI features. Topcoat details come from the [GitHub repo](https://github.com/tokio-rs/topcoat) (README, `docs/getting_started.md`, `examples/toasty-todo`), the [official announcement](https://tokio.rs/blog/2026-07-22-announcing-topcoat) and [docs.rs](https://docs.rs/topcoat). Topcoat is "early-stage and experimental. Expect breaking changes" — pin exact versions and re-verify API details (macro names, session/auth helpers) against the repo docs before building.
>
> **Verified from the repo:** `topcoat new --recommended` scaffolds an app with module routing, Tailwind, Topcoat UI, icons, fonts, and a todo example on **Toasty + SQLite** (`data.db`). Dev server: `topcoat dev`. Migrations: `cargo run -- toasty migration generate` / `apply`. The repo's toolchain is plain `stable`. Topcoat documents "bring-your-own-storage" session auth (login/logout, sliding expiration, token rotation).

---

## 1. Executive Summary

A modern, fast, server-rendered to-do application with a relational database backend, written entirely in Rust on the Topcoat full-stack framework. It is a proof of concept whose real audience is developers evaluating Rust for web apps: it should show that a polished, interactive UI, type-safe data access and authentication can be built in one language, with no WebAssembly and no separate JavaScript codebase.

---

## 2. Problem Statement

Rust is well-regarded for performance and safety, but full-stack web development in Rust has a reputation for friction: split front-end/back-end codebases, WASM toolchains, or hand-wired server + templating + JS. Topcoat (from the Tokio team) claims to remove this with a batteries-included, server-rendered model where reactive behavior is written in type-checked Rust. There is no small, realistic reference app that demonstrates this end to end. A to-do app is familiar enough that viewers judge the *framework*, not the domain, and it exercises all the essentials: CRUD, auth, forms, live search, optimistic UI, and persistence.

---

## 3. Goals & Success Metrics

### Goals
- Demonstrate a complete full-stack app (UI, routing, DB, auth) in one Rust workspace.
- Showcase Topcoat's distinctive features: `#[page]`/`#[component]`, `signal` + `$(...)` client-side expressions, `#[shard]` partial re-renders, `#[memoize]`, `asset!`, and the Tailwind component library.
- Deliver a UI that feels modern (instant interactions, responsive, accessible, dark mode) so the demo is persuasive.
- Produce an honest record of developer experience (what was easy, what hit rough edges) to accompany the code.

### Success Metrics (MVP)
| Metric | Target |
|--------|--------|
| Time from `git clone` to running app | < 5 minutes, documented in README |
| Core flows implemented (register, login, add/edit/complete/delete task, filter) | 100% |
| Server response time for task list render (local, 1,000 tasks) | p95 < 50 ms |
| Interactions that need no full page reload | Toggle complete, add, delete, filter, search |
| Lighthouse (performance / accessibility / best practices) | ≥ 90 each |
| Lines of hand-written JavaScript | 0 (Topcoat-generated only; small HTMX/Alpine use only if unavoidable) |
| Automated test coverage on domain + data layer | ≥ 80% |
| DX write-up of Topcoat strengths and gaps | Delivered with POC |

---

## 4. User Personas

### Persona 1: Priya Raman — Rust-curious Backend Engineer (primary audience)
- **Background:** 6 years in Go/Java services; evaluating Rust for an internal tools team.
- **Goals:** See whether Rust can replace a React + API stack for small internal apps.
- **Pain Points:** Rust web examples are either API-only or WASM-heavy; unclear how auth and DB fit in.
- **Technical Level:** High

### Persona 2: Marcus Chen — Open-source Maintainer / Conference Speaker
- **Background:** Builds demos and talks about systems languages.
- **Goals:** A small, readable, runnable codebase to walk an audience through in 20 minutes.
- **Pain Points:** Demo apps that are too trivial (no DB/auth) or too large to explain.
- **Technical Level:** High

### Persona 3: Dana Whitfield — To-do End User (in-app)
- **Background:** Product manager who manages daily tasks across work and home.
- **Goals:** Capture tasks quickly, organize by project and due date, see what's due today.
- **Pain Points:** Slow, cluttered task apps; lost tasks; too many clicks.
- **Technical Level:** Low

---

## 5. User Stories

### Must Have (MVP)
- As Dana, I want to register and log in so that my tasks are private to me.
- As Dana, I want to add a task with a title so that I can capture it in seconds.
- As Dana, I want to mark a task done/undone with one click and see it update instantly so that the app feels fast.
- As Dana, I want to edit a task's title, notes, due date and priority so that I can keep it accurate.
- As Dana, I want to delete a task (with confirmation) so that I can remove mistakes.
- As Dana, I want to filter by All / Active / Completed so that I can focus.
- As Priya, I want a README with exact setup steps so that I can run the app quickly.

### Should Have
- As Dana, I want to group tasks into lists/projects so that work and personal stay separate.
- As Dana, I want live search by title so that I can find a task as I type.
- As Dana, I want due dates with overdue and "due today" highlighting.
- As Dana, I want dark mode following my system setting.
- As Marcus, I want a DX notes document so that I can discuss Topcoat honestly on stage.

### Nice to Have (Post-MVP)
- As Dana, I want tags and drag-to-reorder so that I can prioritize visually.
- As Dana, I want recurring tasks.
- As Priya, I want a Postgres option and a Docker image so that I can see a production-style deployment.
- As Dana, I want to export my tasks as JSON/CSV.

---

## 6. Feature Specifications

### Feature 1: Authentication & Sessions
- **Description:** Email + password registration/login/logout. Server-side sessions stored in the DB; random session ID in a cookie. Protection is applied per page/component via an explicit `require_auth(cx)`-style helper (Topcoat uses explicit context rather than global middleware).
- **User Benefit:** Per-user private task data; demonstrates Topcoat's auth pattern.
- **Acceptance Criteria:**
  - [ ] Passwords hashed with Argon2id; never logged or stored in plain text
  - [ ] Cookie is `HttpOnly`, `SameSite=Lax`, `Secure` when served over HTTPS; session rotated on login
  - [ ] Unauthenticated access to protected pages and shard endpoints redirects to login / returns 401
  - [ ] Logout revokes the session server-side
  - [ ] CSRF protection on all state-changing requests
  - [ ] Basic login rate limiting (e.g., 5 attempts/minute/IP)
- **Priority:** MVP
- **AI Component:** No
- **Status (built and verified 2026-11-09, `todo-app/`):** Done except where noted.
  - Argon2id (per-hash salt, off the async threads, length 12–128 chars); unknown-email logins verify against a dummy hash so timing doesn't reveal which emails exist.
  - Topcoat's `__Host-session` cookie (`Secure`, `HttpOnly`, `SameSite=Lax`) works over plain HTTP on 127.0.0.1; only the SHA-256 token hash is stored (32 bytes); 14-day lifetime; new token on every login.
  - Logout deletes the DB record; replaying the old cookie is redirected to login. Logout is POST-only (`GET` → 405).
  - CSRF: relies on Topcoat's default `OriginPolicy` (cross-origin POST → 403, verified) plus `SameSite=Lax`; no separate token.
  - Authorization: every `Todo` function takes the acting `user_id`. Verified with a second account: calling another user's toggle/remove procedure returns 200 but changes nothing; anonymous calls are redirected. **Not yet covered by an automated test** (needs a way to build a `Cx` in tests).
  - **Hardening (built and verified 2026-11-09):**
    - **Rate limiting** (in-memory sliding window, checked *before* any Argon2 work so a blocked client also can't burn CPU): 5 failed logins / 15 min per network+email, 20 failed logins / 15 min per network, 10 registration attempts / hour per network. Keying the account limit by network *and* email stops a stranger from locking the real owner out. IPv6 clients are grouped by /64; IPv4-mapped IPv6 is unwrapped. Verified over HTTP: the 6th attempt is refused even with the right password, another account is unaffected until the network limit, existing sessions keep working while a client is blocked.
    - **Expired-session cleanup:** a background task deletes expired `session_records` at startup and hourly (verified: 3 expired rows removed, live rows kept, real session still valid).
    - **Sliding expiry:** `session::refresh` runs on `/todos` page loads once less than half of the 14-day lifetime remains: new cookie (`Max-Age=1209600`) and extended row; no cookie rewrite on other requests (verified).
    - **Automated ownership tests:** real in-memory-SQLite tests (via `Cx::new().with(db)`) cover list/find/toggle/remove/edit across two users, with owner controls. Mutation-checked: removing the ownership check fails exactly those four tests; disabling the throttle check or shifting the purge boundary fails the throttle and purge tests. 51 tests total, clippy clean.
    - **Security headers (built and verified 2026-11-09):** a pathless layer (`src/security.rs`) sets `Content-Security-Policy`, `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: same-origin`, `Permissions-Policy` (camera, microphone, geolocation, payment, usb… all off), `Cross-Origin-Opener-Policy` and `Cross-Origin-Resource-Policy: same-origin`, plus `Cache-Control: no-store` on everything that doesn't choose its own caching (hashed assets and fonts keep `immutable`). Verified on 200, 303, 307, 400, 404 and 405 responses, assets, fonts, and the shard and procedure endpoints. `Strict-Transport-Security` is opt-in (`ENABLE_HSTS=1`), because browsers ignore it over plain HTTP and the TLS proxy may already send it. `CSP_REPORT_ONLY=1` reports violations without blocking, for trying a change safely.
      - **The CSP:** `default-src 'self'`; scripts `'self'` + `'unsafe-eval'`; no inline scripts or styles; `img-src 'self' data:`; `font-src 'self'`; `connect-src 'self'`; `object-src 'none'`; `base-uri 'none'`; `form-action 'self'`; `frame-ancestors 'none'`. **`'unsafe-eval'` is required by Topcoat's browser runtime** (it builds `$(...)` handlers with `new Function`); verified by injecting a policy without it, after which the first re-render left the UI silently unresponsive. So the CSP is a second line of defence behind output escaping, not a replacement for it.
      - **Fonts are now self-hosted** (`host: Asset`): previously every visitor's browser fetched Geist from jsDelivr at an unpinned `@latest`, a third-party request and a supply-chain exposure; the page now makes no third-party requests.
      - **Verified in the browser under enforcement:** third-party fetch, image, script, inline script and iframe are all blocked; add, toggle, live search, list create, and edit-save all work with zero violations; framing the app (even same-origin) is blocked; after logout the Back button refetches and shows the login form instead of old todos.
      - **Dev note:** `topcoat dev` allows its own origin and WebSocket via `TOPCOAT_DEV_URL`; its status indicator's font is blocked (cosmetic, dev only).
      - **Known gap:** Topcoat's cross-origin check (the 403) runs before any layer, so that one plain-text response has none of these headers; set the same headers at the reverse proxy to cover it. 10 unit tests cover the policy builder, including that a hostile `TOPCOAT_DEV_URL` can't inject into the CSP.
    - **Password change (built and verified 2026-11-10):** an Account page (`/account`) asks for the current password, the new one, and a confirmation. The current-password check is rate limited (5 wrong answers per 15 min per account, 20 per network) *before* any hashing, so a stolen session cannot be used to guess the password; form mistakes (too short, mismatch) don't count against it, and a success resets it. "Same as current" is only revealed after the current password is proven. On success every other session of the account is deleted and the current one gets a fresh token (Topcoat's `session::rotate`). Verified with two devices: the changing device stayed logged in on its new token, its old token and the other device were signed out, the old password stopped working and the new one worked; the database holds only an Argon2id hash and the server log held no password; passwords travel only in the POST body. Also verified refusals (mismatch, weak, same, wrong current, cross-origin 403, anonymous redirect, missing fields 400) and the throttle (5 guesses, then even the correct password is refused and nothing is changed). Mutation-checked: skipping the current-password check or deleting the wrong sessions fails six tests. 102 tests total.
    - **Password change limits:** no password reset for a forgotten password (there is no email to send a link to); no separate "log out other devices" button (a password change does it); no password-strength meter or breached-password check beyond the 12 to 128 character rule.
  - **Closing sign-ups (built and verified 2026-11-10):** `REGISTRATION=open|closed|first-user` (app default `open`, so nothing changes unless set; the compose file defaults to `closed`). A typo (`colsed`) stops the app at startup (exit 2) instead of leaving sign-ups open. In `closed` mode the sign-up page shows a notice instead of the form, the login page stops linking to it, and a POST is refused *before* anything is validated, hashed or stored (valid and garbage input get the same answer, so a closed site does not coach anyone). `first-user` is open only until an account exists, and a race between two simultaneous first sign-ups is settled deterministically (the lowest id stays, the other is deleted and told it is closed; tried live with two simultaneous POSTs: exactly one account). Closing sign-ups does not affect login, password change or existing accounts.
    - **`todo-app create-user <email>`** makes an account directly (the bootstrap for a closed site): the password is read from standard input, never an argument (no process listing or shell history), it is never echoed, only the line ending is stripped (trailing spaces are part of the password), and it applies the same rules as sign-up (valid email, 12 to 128 character password, email unused case-insensitively). Exit codes: 0 created, 1 refused (message says why), 2 misused (no stdin, wrong arguments). Verified on a brand-new database, against a running server, and inside the container (unprivileged user, read-only filesystem) followed by a login through the HTTPS proxy.
    - **Why `closed`, not `first-user`, for deployments:** on a fresh public server whoever reaches it first would own the first account, and automatic HTTPS makes a new site reachable within seconds. 109 tests (up from 102); mutation-checked: ignoring `closed` or not removing the race loser fails exactly the two tests that target them.
    - **Limits:** there is no invitation flow and no admin UI (accounts are made by the operator with `create-user`); existing accounts cannot be listed, disabled or deleted from the app yet; `first-user` still has the claim-the-site window described above.
  - **Delete account (built and verified 2026-11-10):** the Account page gets a separate, red "Delete account" card (`/account/delete`, POST only; a GET is 405). Deleting needs the **current password** (checked through the same function and the same rate-limit counter as password change, so five wrong answers split across the two forms block both) and the account's **email typed out** (matched like at sign-up; a mistyped one is reported before any hashing and does not count as a guess). The account to delete always comes from the session, never from the form (a bystander's session submitting the victim's email and password cannot touch the victim). Deletion removes sessions, todos, lists and the account in **one transaction**; on success the session cookie is cleared (`Max-Age=0`) and the browser goes to `/login?deleted=1`, which says so once.
    - **Verified live (two devices + a bystander):** before, victim 3 todos / 1 list / 2 sessions; after, account and all of it gone, bystander untouched, zero orphaned rows, no trace of the victim in any text column of any table, every old cookie (both devices, and the pre-response one) redirects to login, old credentials refused, the same email can sign up again with a new id and nothing carried over. Refusals checked: wrong password, wrong/blank/other-account confirmation, missing fields (400), cross-origin (403), anonymous (307), GET (405), no change to the data in any of them. Also done through the real browser form (wrong confirmation shows the error and keeps the session; the correct one lands on the login page; no CSP violations; no password in any URL).
    - **Operator command** `todo-app delete-user <email> [--yes]`: a **dry run by default** (prints what the account owns, deletes nothing, exit 2); `--yes` deletes (exit 0); unknown email exit 1; misuse exit 2; email matched ignoring case; flag order free. Verified natively against a running server. (Its container path is the same `docker compose exec` route verified for `create-user`; `delete-user` itself was not re-run inside a rebuilt image.)
    - **Tests:** 123 (up from 109), including one that proves atomicity: it builds a database from the real migrations *minus the `lists` table*, so the purge fails after already deleting sessions and todos, and asserts everything is back. Mutation-checked: removing the transaction fails exactly that test; ignoring a wrong password plus not deleting todos fails eight.
    - **Limits:** no grace period, undo or "export my data first" step; backups taken earlier still hold the data until rotated away; in `first-user` mode deleting the only account reopens sign-ups; the check and the delete are separate steps (an account changed in between is still deleted correctly, but a password changed in that instant would not stop it).
  - **Password reset (built and verified 2026-11-10), operator-delivered:** the site cannot send email (no SMTP configuration or mail dependency, and sign-ups are closed in deployments), so a forgotten password is reset with a **one-time link the operator makes and hands over**: `todo-app reset-link <email>` prints `https://site/reset?token=<64 hex>` (full URL when `PUBLIC_URL` is set, otherwise the path plus a note). There is deliberately **no web form that creates links**, so the site cannot be used to find out who has an account. Email delivery of the same link is a clean later addition that would not change the core.
    - **Properties, each covered by a test and most also verified live:** the token is 256 random bits (64 hex); only its **SHA-256 hash** is stored (verified: the raw token is in no table, and not in the server log); it works **once**, for **one hour** (verified by backdating the expiry in the database), and a newer link for the same account **cancels** the older one; a wrong, malformed, expired, used or cancelled link all read as "not valid"; opening the link (GET) changes nothing and a valid token is echoed into the page only if it is well-formed hex (hostile `<script>`/`<img onerror>`/`</title><svg onload>` values in `token` and `error` are never reflected); a refused password (too short, mismatch) leaves the link usable, and an invalid link is reported as invalid even if the two passwords also differ; using the link replaces the password, deletes the account's links and **every session of the account** in one transaction, and logs **nobody in** (verified: both sessions of the account signed out, a bystander's session and link untouched, old password refused, new accepted). Bad links are limited to 20 per 15 minutes per network, after which even the real link is refused (verified live: first refusal came on attempt 15 after ~6 earlier bad ones). Cross-origin POST is 403, missing fields 400. The page is served `Referrer-Policy: same-origin`, `no-store` and unframeable, so the URL secret does not leak onward. Deleting an account deletes its pending link; expired links are swept hourly.
    - **Verified in the real browser** (mismatch shows the error and keeps the link; the correct password lands on the login page with a notice, no token or password left in the address bar, no CSP violations) **and in the container** (new migration `0004` applied on a fresh volume; `reset-link` printed a complete `https://localhost:8443/...` link from `PUBLIC_URL`; the reset completed through the HTTPS proxy with HSTS present; reuse refused). `delete-user` was also exercised inside the container for the first time.
    - **Tests:** 142 (up from 123), including expiry to the exact second, one-use, cancellation, session revocation scope, the not-stored raw token, and the network limit. Mutation-checked: ignoring expiry, keeping sessions alive and not cancelling the old link fail exactly the three tests that target them.
    - **Limits:** the link is a secret in a URL, so it can appear in the operator's terminal history, chat logs, and a reverse proxy's access log if the proxy logs query strings (the app does not log it); anyone holding a live link can take over the account until it is used or expires; the operator is trusted (they can make a link for anyone); email delivery was added afterwards (next entry); two simultaneous uses of one link are serialized by the database transaction, but that exact race was not exercised.
  - **Emailed reset links (built and verified 2026-11-10):** optional. Setting `SMTP_URL`, `MAIL_FROM` and `PUBLIC_URL` turns on a **"Forgot your password?"** form (`/forgot`, linked from the login page only when email is configured) that emails the *same* one-time link as `reset-link`, using Topcoat's `mail` feature (`mail-smtp`, rustls with bundled roots, so the slim container image needs no extra packages). `todo-app send-test-email <address>` checks the settings with a message that contains no link. The operator-made link still works with or without email.
    - **Does not reveal who has an account:** every submission redirects to the same "if that address belongs to an account..." page. The lookup, token and SMTP conversation run in a **background task**, so timing matches too (verified live against a local SMTP server that took 2 s per message: known and unknown addresses both answered in about 3 ms, 2.2 to 2.8 ms each, and only the known address received mail). Rate limits are 3 per address per hour and 10 per network per 15 minutes, and unknown addresses are counted exactly like known ones (verified live: the 4th and 5th request for one address were silently ignored with the same reply; the 11th from one network got "too many requests").
    - **Safe by construction:** the link is built from `PUBLIC_URL`, never the `Host` header (a unit test would fail if it were not); email needs an **encrypted** SMTP connection (`smtps://`, or `smtp://...?tls=required`; plain `smtp://` only for localhost) and an `https://` `PUBLIC_URL` (http only for localhost), all checked at startup with messages that never echo the SMTP password (which is also redacted from `Debug` output); a mail failure is logged as `could not email a password reset link: could not deliver the mail` with neither address nor link (verified with the mail server stopped); the real emailed link was followed end to end (older link dead after a newer email, reset worked, link single-use, old password refused, new accepted), in the built-in browser as well.
    - **Tests:** 159 in all (up from 142). Mutation-checked: not counting per-address requests, allowing plain SMTP to a remote host, and building the link from another address each fail the tests that target them.
    - **Limits:** plain-text mail only, no queue, retries or bounce handling (the app learns only that the server *accepted* the message); delivery to the inbox (SPF/DKIM, spam) is the operator's job; anyone who knows an address can cancel its owner's latest link by asking for a new one (at most 3 times an hour; they cannot read it); no email verification, so a mistyped sign-up address is never caught; the real SMTP-over-TLS handshake was **not** exercised (only plain SMTP to localhost, the configuration parsing and the transport's own tests), so try `send-test-email` against your real provider before relying on it.
  - **Known limits:** the limiter is per-process, so it resets on restart and is not shared across instances (use the database or Redis before running several). Behind a reverse proxy, Topcoat's `TrustedProxies` must be configured or all users share one limit (noted in `app.rs`). Users behind one shared NAT address share the network limits. Refresh only happens on page loads, not shard/procedure calls (cookies can't be set once a response streams). No email verification or "log out other sessions" button yet. `register`/`log_in` success paths (which call Topcoat's `session::start`) are verified live but not in unit tests.
  - Differences from the plan: IDs are `u64` auto-increment, not UUID; login/register forms post to the same path as their page (`/login`, `/register`); mutations are `#[procedure]` calls whose cross-user failures are silent no-ops.

### Feature 2: Task CRUD
- **Description:** Create, read, update, delete tasks with title, notes, due date, priority, completion state.
- **User Benefit:** The core job of the app.
- **Acceptance Criteria:**
  - [ ] Add-task input is focused on load; Enter submits; empty/over-length titles rejected with inline error
  - [ ] Edits persist and show without full page reload
  - [ ] Delete requires confirm dialog (Topcoat UI dialog component)
  - [ ] A user can never read or modify another user's tasks (enforced in the data layer, tested)
- **Priority:** MVP
- **AI Component:** No

### Feature 3: Instant Completion Toggle (client-side reactivity)
- **Description:** Checkbox toggling updates the UI immediately via a `signal` and `$(...)` expression (compiled to JS), while the change is persisted via a shard/action; failure reverts the UI and shows an error.
- **User Benefit:** Snappy feel; the flagship demo of Topcoat reactivity without WASM.
- **Acceptance Criteria:**
  - [ ] Visual update in < 100 ms without waiting for the server
  - [ ] Persisted state matches UI after reload
  - [ ] Failure path tested (server error reverts state)
- **Priority:** MVP
- **AI Component:** No

### Feature 4: Filters, Lists and Live Search
- **Description:** Filter by status; assign tasks to lists; search-as-you-type implemented with a `#[shard]` that re-renders only the task list fragment.
- **User Benefit:** Findability; demonstrates partial re-rendering.
- **Acceptance Criteria:**
  - [ ] Filter and search state reflected in the URL (shareable, back-button safe)
  - [ ] Search debounced; only the list region is swapped
  - [ ] Counts ("3 active") update with changes
- **Priority:** Filters = MVP; Lists + Search = Should Have
- **AI Component:** No
- **Status (filters and search built and verified 2026-11-09; lists not built):**
  - Filter tabs All / Active / Completed with live counts (counts ignore the search text). The filter is a normal link (`?filter=active`), so it is shareable and back-button safe; an unknown value falls back to All.
  - Live search is a `#[shard]` that re-renders only the tabs and list as you type; Enter submits a plain GET form that also puts `q` in the URL, and a reload or shared link restores it. Tab links carry the current search. Matching is case-insensitive, every word must appear in the title or notes, and the text is treated as plain text (no pattern or SQL meaning).
  - Filtering is done in Rust over the user's todos (pure, unit-tested), not in SQL. Fine for a POC at hundreds or low thousands of todos; push it into the query if lists get large.
  - Completing a todo under the Active filter removes it from the list and updates the counts without a reload. Empty states distinguish "Nothing to do" from "No todos match" (with a clear link).
  - Verified hostile input: injection payloads in `q` (attribute and HTML-comment breakout attempts) create no elements and run no script; the query is cut to 100 chars server-side (a 300 KB query behaves like a 100-char one; 3 MB is rejected with 413); the shard endpoint redirects anonymous callers, returns nothing of another user's data, and refuses cross-origin requests.
  - **Limits:** no debounce (Topcoat's `$(...)` subset has no timer), so each keystroke is one shard request, fine locally but chatty over a slow network; the URL only updates on Enter, not while typing.
  - **Lists and sorting (built and verified 2026-11-09):**
    - **Lists:** each user has named lists (up to 50, names 1–80 chars, no control characters, unique ignoring case per user). A todo has an optional list; no list means Inbox. A `/lists` page creates, renames and deletes. **Deleting a list moves its todos to the Inbox**; it never deletes todos (verified: 4 todos survived deleting their list).
    - **Browsing:** list tabs (All lists / Inbox / each list) with live counts; the status tabs and counts are scoped to the chosen list; the add form defaults to the list being viewed; the edit page can move a todo between lists; "List: name" badges show only in the All-lists view.
    - **Sorting:** Oldest (default), Newest, Due date (soonest first, undated last), Priority (high first), Title (case-insensitive); ties always fall back to age. Verified with a mixed data set.
    - **URL state:** `?filter=…&list=…&sort=…&q=…` (defaults omitted). Every link and the search form carry all four, verified by clicking through list → sort → filter → search in the browser. Unknown or foreign list ids fall back to All lists.
    - **Security (IDOR):** assigning a todo to a list is checked against ownership in the data layer (`add` and `save`), in the edit route (visible `error=list`), and in the add procedure. Verified with two users: a foreign list id is refused via the edit form, the add procedure (six malformed/foreign values), `?list=` and shard arguments, with no leak of the other user's list name or todos; renaming or deleting another user's list returns "missing" and changes nothing. A list name containing `<img onerror>`/`<b>` renders as literal text on all four pages that show it. Mutation-checked: removing the ownership check fails exactly the two list-ownership tests.
    - **Migration:** `0003` adds a nullable `todos.list_id` and the `lists` table; verified over a populated table (existing rows land in the Inbox).
    - **Tests:** 71 total (up from 51), clippy clean.
    - **Known limits:** list-name uniqueness is checked in the application, not by a database constraint, so two simultaneous creates of the same name could both succeed; no list colors, ordering, or archiving; no drag-to-reorder or manual ordering; counts and filtering are computed in Rust over the user's whole todo set, which is fine for hundreds of todos but should move into SQL for large lists; deleting a list has no confirmation step (it is non-destructive to todos).

### Feature 5: Due Dates & Priority
- **Description:** Optional due date and Low/Medium/High priority; "Today" and "Overdue" views.
- **User Benefit:** Basic planning.
- **Acceptance Criteria:**
  - [ ] Overdue tasks visually distinct (not by color alone)
  - [ ] Dates stored in UTC, rendered in the user's configured/browser time zone
- **Priority:** Fields = MVP; Today/Overdue views = Post-MVP
- **AI Component:** No
- **Status (fields built and verified 2026-11-09):** Notes, priority and due date work in the add form, the list, and a separate edit page (`/todos/edit?id=…`).
  - Due dates are plain `YYYY-MM-DD` strings (a calendar date, no time or zone), not UTC datetimes as first planned: a due *day* doesn't shift with the viewer's time zone. "Today"/"Overdue" is computed in UTC, so far from UTC the switch happens at an odd local hour.
  - Priority and due state are shown as text badges ("Priority: High", "Overdue: …", "Due today"), not color alone. A done todo is never shown as overdue.
  - All input is validated on the server (title 1–200, notes ≤ 2000, priority low/medium/high, real calendar dates 1900–2200); tampered requests to the add procedure and edit form are rejected without changing data. Notes and titles render as escaped text (an injected `<script>` did not run).
  - Migration `0002` was hand-edited to give `priority` a `DEFAULT 'medium'`, because SQLite can't add a NOT NULL column without one to a table that has rows (verified against a populated table).
  - Known gap: migration `0001` (the earlier `user_id NOT NULL` column) has the same problem for databases that already hold pre-auth todos; only an issue for data created before auth existed.
  - **Not done:** the Today/Overdue views and sorting by due date or priority; the edit form doesn't keep unsaved input after a validation error (it reloads the saved values).

### Feature 6: Modern, Accessible UI
- **Description:** Tailwind-based components from `topcoat-ui`, responsive layout, dark mode, keyboard shortcuts, content-hashed assets via `asset!`, self-hosted font. **Visual design (built):** a teal-to-blue brand gradient with a Rust-orange accent; a drifting aurora and dot-grid backdrop fixed to the window; frosted-glass cards; inline-SVG illustrations (sign-in panel, empty states, dashboard banner); a three-column dashboard on wide screens (lists and sorting, the todo cards, a progress ring with counters and an "Up next" list) that collapses to one column on phones; and CSS-only motion (staggered entrances, a pop on completion, hover lift, an animated progress ring) that stops under `prefers-reduced-motion`. All of it respects the CSP: no inline styles, no third-party images or fonts.
- **User Benefit:** A polished demo; usable by everyone.
- **Acceptance Criteria:**
  - [ ] Works at 360 px width and up
  - [ ] Full keyboard operation; visible focus; WCAG 2.2 AA contrast
  - [ ] Respects `prefers-color-scheme` and `prefers-reduced-motion`
- **Priority:** MVP
- **AI Component:** No

---

## 7. AI Feature Design

**Not applicable for the POC.** No AI features are planned; the project's purpose is to showcase Rust and Topcoat.

If AI is added later (e.g., natural-language task entry such as "call dentist Friday 3pm", or auto-prioritization):

| Capability | Model Recommendation | Reasoning |
|------------|---------------------|-----------|
| Parse free-text into structured task | Small/cheap model (e.g., Claude Haiku) | Short, high-volume extraction |
| Weekly plan / task breakdown | Mid-tier model (e.g., Claude Sonnet) | Light reasoning, low volume |

Guidance if added: call the API server-side from a Topcoat action (key in env, never client-side), use prompt caching for the fixed system prompt, set spend limits and logging from day one, and keep a human confirmation step before saving AI-created tasks.

---

## 8. Technical Architecture

### Recommended Stack

| Layer | Technology | Rationale |
|-------|-----------|-----------|
| Language | Rust, stable channel (the repo's `rust-toolchain.toml` uses `stable`; confirm any crate `rust-version`) | Project goal |
| Tooling | `topcoat-cli` 0.12 (`cargo install topcoat-cli`): `topcoat new`, `topcoat dev` (required to run: it builds the asset bundle; plain `cargo run` panics), `topcoat fmt`, `topcoat ui` | Scaffold, hot-reload dev server, macro formatting, copy-in UI components |
| Web framework / UI | **Topcoat** + `topcoat-ui` (Tailwind) | Required; server-rendered, no WASM, reactivity via `signal` / `$(...)` / `#[shard]` |
| Server runtime | Tokio (via Topcoat) | Async foundation |
| Database | **Microsoft SQL Server 2022** in a Docker container (revised 2026-11-12; originally SQLite) | Requested; a real server database, run locally with one `docker compose` command |
| Data access | **`mssql-tds`** (Microsoft's `mssql-rs` driver) via `mssql-tiberius-bridge` for pooling and parameter binding; hand-written T-SQL (revised; originally the Toasty ORM, which has no SQL Server driver) | Microsoft's own driver; every value is a bound parameter |
| Migrations | Numbered T-SQL files in `migrations/`, embedded in the binary and applied at startup in one transaction (revised; originally the Toasty CLI) | Versioned schema, nothing to ship besides the executable |
| Auth | Topcoat's bring-your-own-storage session auth, with sessions stored in SQL Server; `argon2` crate for password hashing | Reuses framework cookie/rotation/expiry handling instead of hand-rolling it; shows `require_auth(cx)` pattern |
| Config / logging | `dotenvy`/env vars, `tracing` | Standard Rust practice |
| Testing | `cargo test`, integration tests against a real SQL Server (a throwaway database per test), Playwright for 2–3 e2e smoke flows | Confidence without heavy infra |
| Hosting | Local for the POC; Docker image + Fly.io/Railway (or a VPS) as stretch | Free/cheap, single binary |

### Alternative Stacks to Consider
- **Leptos or Dioxus (+ Axum, SQLx):** more mature community and larger ecosystem; full reactivity in Rust via WASM, at the cost of WASM bundle size/toolchain complexity. Pick these if the goal shifts from "showcase Topcoat" to "production Rust UI today."
- **Axum + Askama/Maud + HTMX + SQLx:** the proven, stable Rust server-rendered route; far more documentation, but you write hand-managed JS-attribute glue and lose Topcoat's type-checked client expressions.

### Architecture Diagram (ASCII)
```
[Browser]  <-- HTML + reactive instructions + generated JS -->  [Topcoat app (Tokio)]
   |                                                             |  pages / components / shards
   | $(...) client-side state (signals)                          |  require_auth(cx)
   |                                                             v
   |                                                       [Domain layer: Task, List, User]
   |                                                             |
   |                                                       [Toasty ORM]
   |                                                             v
   +--- static assets (content-hashed via asset!) ---       [SQLite file]
```

### Key Technical Decisions
- **Server-rendered, no WASM:** the central showcase; interactivity limited to Topcoat's supported Rust→JS expression subset. Anything beyond it is handled by shards (server round-trip).
- **Components fetch their own data** (Topcoat idiom) with `#[memoize]` to dedupe queries per request, avoiding N+1 patterns.
- **Authorization lives in the data layer**, not only the UI: every query is scoped by `user_id`.
- **Layered workspace:** `domain` (pure types/logic, unit-testable), `db` (persistence), `web` (Topcoat pages/components) — keeps business rules independent of the framework, which de-risks Topcoat's API churn.
- **SQLite to start:** simplest demo; schema kept portable to Postgres.
- **Pin versions** (exact Topcoat/Toasty versions) and record upgrade friction in the DX notes.
- **Start from the official scaffold and example:** generate with `topcoat new --recommended` and study `examples/toasty-todo` (note: that example uses an in-memory DB and form-post-then-redirect; this POC adds file-backed SQLite, auth, and richer interactivity on top).

---

## 9. API Design

Topcoat is page/shard oriented rather than REST-first, and a JSON API is **not** part of the MVP. Verified from the scaffold (Topcoat 0.12): `#[page]` functions render routes (module routing: `src/app/todos.rs` serves `/todos`); `#[shard]` functions are server-rendered fragments that re-render when a `Signal` they read changes (the scaffold bumps a `revision` signal after each mutation); `#[procedure]` functions are server functions called from browser `$(async |e| ...)` closures. Procedure arguments come from the browser and **must be validated and authorized server-side**.

| Kind | Rust item / route | Description | Auth Required |
|------|-------------------|-------------|---------------|
| Page | `#[page]` in `app/todos.rs` → `/todos` | Task list with filter/search state | Yes (redirects to login) |
| Page | `/login`, `/register` | Auth forms | No |
| Procedure | `login`, `register` | Create user/session | No |
| Procedure | `logout` | Revoke session | Yes |
| Shard | `todo_list(revision, filter, query)` | Re-render task list fragment | Yes |
| Procedure | `add(title, ...)` | Create task | Yes |
| Procedure | `toggle(id)` | Toggle completion | Yes |
| Procedure | `update(id, fields)` | Update task fields | Yes |
| Procedure | `remove(id)` | Delete task | Yes |
| Page | lists view (Should Have) | Tasks in a list | Yes |
| Ops | health endpoint (to confirm how to expose raw routes) | Liveness probe | No |

### Key Request/Response Shapes
```rust
// Server procedure called from the browser (shape per the scaffold's `add`)
#[procedure]
pub async fn add(cx: &Cx, title: String /* 1-200, trimmed */) -> Result<()>
// MVP+: extend with notes, due_at (UTC), priority, list_id; return validation errors to the UI.
// The user id comes from the authenticated session in `cx`, never from a browser argument.
```

---

## 10. Data Model

### Core Entities

**User**
| Field | Type | Description |
|-------|------|-------------|
| id | UUID | Primary key |
| email | String (unique, ≤320) | Login identifier, lower-cased |
| password_hash | String | Argon2id hash |
| created_at | DateTime (UTC) | Creation timestamp |

**Session**
| Field | Type | Description |
|-------|------|-------------|
| id | String (random, 256-bit) | Cookie value (store hashed if feasible) |
| user_id | UUID | FK → User |
| created_at / expires_at | DateTime (UTC) | Lifetime (e.g., 14 days) |
| revoked_at | DateTime? | Set on logout |

**List**
| Field | Type | Description |
|-------|------|-------------|
| id | UUID | Primary key |
| user_id | UUID | FK → User |
| name | String (≤80) | Display name |
| created_at | DateTime (UTC) | Creation timestamp |

**Task**
| Field | Type | Description |
|-------|------|-------------|
| id | UUID | Primary key |
| user_id | UUID | FK → User (owner; used for scoping) |
| list_id | UUID? | FK → List (null = Inbox) |
| title | String (1–200) | Required |
| notes | String? | Optional plain text |
| priority | Enum | low / medium / high |
| due_at | DateTime? (UTC) | Optional |
| completed_at | DateTime? | Null = active |
| created_at / updated_at | DateTime (UTC) | Audit timestamps |

### Relationships
- User has many Sessions, Lists, and Tasks.
- List belongs to User and has many Tasks.
- Task belongs to User and optionally to a List.
- Indexes: `Task(user_id, completed_at)`, `Task(user_id, due_at)`, `Session(user_id)`, `User(email)` unique.

---

## 11. Non-Functional Requirements

| Category | Requirement |
|----------|-------------|
| Performance | Page render p95 < 50 ms locally at 1,000 tasks; DB timeout ~3–5 s; no N+1 queries on the list page |
| Security | Argon2id, HttpOnly/SameSite cookies, CSRF tokens, server-side authorization on every query, input validation, parameterized queries, security headers (CSP etc.), no secrets in repo |
| Scalability | Single-node target (hundreds of users); stateless app, so Postgres + multiple instances is a post-MVP path |
| Availability | Best-effort for POC; `/healthz` endpoint; graceful shutdown |
| Accessibility | WCAG 2.2 AA; keyboard operable; screen-reader labels on toggles and dialogs |
| Maintainability | `cargo fmt`, `clippy -D warnings`, CI on every push; version-pinned dependencies |
| Compliance | Not applicable for POC; no PII beyond email; document data deletion path |
| Portability | Builds and runs on Windows, macOS, Linux |

---

## 12. Phased Delivery Plan

### Phase 1 — MVP
**Goal:** Working, demo-ready app showing the full Topcoat stack.
**Timeline:** ~2–3 weeks part-time
**Includes:**
- [ ] Scaffold with `topcoat new --recommended`, CI, pinned Topcoat/Toasty versions; short spike on the areas not covered by the official example (sessions on Toasty, CSRF, shard-based partial updates)
- [ ] Schema + migrations; domain and data layers with tests
- [ ] Register / login / logout with sessions and CSRF
- [ ] Task CRUD, instant completion toggle (signal + `$(...)`), status filters
- [ ] Responsive, accessible, dark-mode UI with `topcoat-ui`
- [x] README (setup < 5 min) and initial DX notes: done 2026-11-10 in `todo-app/README.md` and `todo-app/docs/DX-NOTES.md`. Measured: from a clean copy (no `target/`, no database) `topcoat dev` served the login page after **78 s** and sign-up worked (Cargo cache warm; add the one-off `cargo install topcoat-cli`, about a minute, on a fresh machine).
- [x] Deployment (built and verified 2026-11-10, Docker 29 on Windows):
  - **Packaging:** multi-stage `Dockerfile` (pinned `rust:1.99-bookworm` build; `topcoat asset bundle --release`; `debian:bookworm-slim` runtime, 138 MB), unprivileged user, read-only root filesystem, all capabilities dropped, `/data` volume for the SQLite file. `docker-compose.yml` adds a Caddy proxy for automatic HTTPS; the app's port is not published.
  - **Config (`src/config.rs`):** `DATABASE_URL`, `MIGRATE_ON_START`, `TRUSTED_PROXIES`, `TRUSTED_PROXY_HOPS`, validated at startup (typos fail with exit code 2 and a message naming the variable; a database URL is never echoed). Topcoat's own `TrustedProxies::networks` would panic on a bad address, so addresses are validated first.
  - **Migrations are embedded in the binary** and applied at startup (`toasty::embed_migrations!`), so the image needs no loose files. A test builds the schema *only* from the embedded migrations and exercises every model; mutation-checked: adding a model column without a migration fails it with `no such column`.
  - **Operations:** `GET /healthz` (checks the database) and a `todo-app healthcheck` subcommand (std only, so the image needs no `curl`); healthy in about 8 s; `docker stop` exits 0 in 0.77 s (graceful `SIGTERM`); data survives restart without re-running migrations.
  - **Verified through the real proxy:** HTTPS with the `Secure` `__Host-` cookie, HSTS and the security headers reaching the browser, HTTP redirects to HTTPS (308), and a spoofed `X-Forwarded-For` does **not** dodge the login rate limit (7 failures claiming 7 addresses were throttled at the 6th, and the correct password was refused).
  - **Backup and restore** (stop, copy `app.db`, start; restore with `chown 10001`) tried end to end: a user created after the backup disappeared and a user from before it could log in.
  - **CI:** `.github/workflows/ci.yml` (fmt, clippy `-D warnings`, tests, and an image build). Its commands pass locally and the file parses, but it has **not run on GitHub**. The code was `cargo fmt`-normalized so the format check passes.
  - **Docs:** `README.md`, `docs/DEPLOYMENT.md` (verified steps, go-live checklist, troubleshooting), `docs/DX-NOTES.md`, `.env.example`.
  - **Not done / untested:** an nginx or other proxy (only Caddy was run); behavior of a *failed* migration; multi-instance operation (by design unsupported); a real SMTP-over-TLS handshake against a provider.
  - **Environment note:** on the test machine Windows' own HTTP service held ports 8080 and 8081, so Docker Desktop silently skipped publishing them; the guide's troubleshooting table covers it.

### Phase 2 — Showcase Polish
**Goal:** Demonstrate the more advanced Topcoat features.
**Timeline:** ~1–2 weeks
**Includes:**
- [ ] Lists/projects, live search shard, Today/Overdue views
- [ ] `#[memoize]` demonstrated and benchmarked
- [ ] Playwright smoke tests; Lighthouse ≥ 90
- [ ] Walkthrough doc / short screencast; final DX write-up

### Phase 3 — Production-ish Extras (optional)
**Goal:** Show the path to deployment.
**Includes:**
- [ ] Postgres support and Docker image; deploy to a free tier
- [ ] Tags, drag-to-reorder, recurring tasks, export
- [ ] Optional AI natural-language task entry (see §7)

---

## 13. Open Questions & Risks

| # | Question / Risk | Owner | Status |
|---|----------------|-------|--------|
| 1 | Topcoat is experimental (v0.x); breaking changes likely. Mitigate by pinning versions and isolating domain/db layers | Dev | Open |
| 2 | Toasty + SQLite + migrations are confirmed in the official scaffold, but the roadmap lists "better Toasty integration" (form-driven create/update) as future work; expect some manual glue. Fallback: sqlx | Dev | Mitigated |
| 3 | Rust→JS expression subset may not cover needed client logic; fall back to shards or HTMX/Alpine | Dev | Open |
| 4 | Sparse docs and few third-party examples (the one tutorial found is thin and unverified) — rely on official docs and source | Dev | Open |
| 5 | Topcoat provides session primitives, but the roadmap lists full "Authentication" as future work; user registration, password hashing, CSRF and rate limiting are still ours to build and review | Dev | Open |
| 6 | Verify `cargo install topcoat-cli` and `topcoat dev` work cleanly on Windows | Dev | Open |
| 7 | Is multi-user auth required, or would a single-user, no-login POC be preferable? | Product | Open |
| 8 | Final app name and license | Product | Open |

---

## 14. Out of Scope (MVP)

- AI features of any kind
- Team sharing, collaboration, comments, assignments
- Native/mobile apps, offline mode/PWA sync
- Notifications, email, reminders (Topcoat email support is only planned)
- OAuth/social login, email verification
- Public JSON/REST API and third-party integrations
- Multi-region hosting, horizontal scaling, i18n

---

*This PRD was generated with Claude. Treat as a living document — update as decisions are made.*
