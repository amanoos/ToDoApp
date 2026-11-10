# Developer-experience notes: building this with Rust and Topcoat

These are first-hand notes from building the app, written to be useful to someone deciding
whether Topcoat fits their next project. Versions: Topcoat 0.12, Toasty 0.11, Rust 1.99 on
Windows. Topcoat moved from 0.10 to 0.12 while this was being built, so treat anything here as
a snapshot.

## What worked well

- **One language, no WebAssembly.** Pages, data access, validation and the interactive bits are
  all Rust. A `$(...)` closure is ordinary, type-checked Rust that Topcoat runs in the browser,
  and the app contains no hand-written JavaScript.
- **A small, learnable model.** `#[page]` renders a route, `#[shard]` is a server-rendered
  fragment that re-renders when a signal it reads changes, and `#[procedure]` is a server
  function the browser can call. Live search, optimistic toggles and list updates are all just
  those three. There is no client-side state store to design.
- **The scaffold is genuinely useful.** `topcoat new --recommended` produced a working todo app
  on SQLite, and `topcoat dev` rebuilds and refreshes on every save. It also moves to the next
  free port instead of failing when 3000 is taken.
- **Authorization stays visible.** Topcoat has no global middleware; you call `require_user(cx)`
  where you need it. That made it easy to see (and test) that every query is scoped to the
  user. The session primitives are well designed: only a hash of the token is stored, the
  cookie is `__Host-`, `Secure`, `HttpOnly`, and `session::start` always issues a fresh token.
- **Safe defaults.** A built-in origin policy rejects cross-site state-changing requests
  (verified: a cross-origin POST gets 403), output is escaped by `view!` (an injected
  `<script>` rendered as text everywhere it was tried), and hashed assets are served with
  immutable caching.
- **Testability.** `Cx::new().with(db)` builds a real request context, so the data layer is
  tested against in-memory SQLite with no test framework of Topcoat's. `MigrationSet::apply`
  makes it easy to test that migrations and models agree.
- **Deployment pieces exist.** Embedded migrations, graceful shutdown on SIGTERM, a documented
  asset-bundling step and `host: Asset` for self-hosted fonts meant the production story needed
  very little custom code.
- **Windows was fine.** The toolchain, the dev server and `__Host-` cookies on `127.0.0.1` all
  worked without special handling.

## Rough edges

These cost real time. None is fatal; several are one-line fixes once you know.

0. **The stock `icon` component sets an inline `style` attribute**, which a strict
   Content-Security-Policy (`style-src 'self'`) blocks, logging one violation per icon. The icons still
   draw, but the console fills with errors. This app renders icons through its own
   `components/icon.rs`, the same SVG without the attribute. Likewise, anything dynamic that
   would normally use `style=` (the progress ring's percentage) is done with a fixed set of
   classes (`pct-0` to `pct-100`) and a registered CSS property, because Tailwind can only see
   class names that appear literally in the source.

1. **Generated method names collide with yours.** Toasty adds `create`, `update` and `delete` to
   every model, so a hand-written `Todo::update` is a compile error (E0592) pointing at the
   derive macro. Pick names like `add`, `save` and `remove`.
2. **Generated migrations can break populated databases.** Adding a `NOT NULL` column produces
   `ADD COLUMN ... NOT NULL` with no default, which SQLite rejects when the table has rows. It
   works on an empty development database and fails in production. Always read the SQL.
3. **No debounce in `$(...)`.** The browser-side expression language has no timers, so live
   search sends a request per keystroke.
4. **A Content-Security-Policy needs `'unsafe-eval'`.** The runtime builds handlers with
   `new Function`. Without it the page renders but interactions silently stop working after the
   first re-render, with no visible error. The framework has no CSP guidance or nonce support.
5. **Some responses skip your layers.** The origin check runs before layers, so its 403 cannot
   carry your security headers. A layer's headers on *errors* need `response_headers(cx)`
   rather than editing the response; this is documented, but easy to miss.
6. **Third-party requests by default.** Fontsource fonts load from a CDN at an unpinned
   `@latest` unless you pass `host: Asset`, and `topcoat dev`'s status indicator fetches its
   own font from a CDN. Both are one-line fixes once you notice them.
7. **Plain `cargo run` panics** (`no asset bundle at ...`) because assets are bundled by
   `topcoat dev` or `topcoat asset bundle`. Production builds must go through the CLI and ship
   the bundle next to the binary.
8. **The browser protocol is undocumented.** Shard and procedure calls are JSON with typed,
   dehydrated values (a `u64` is `{"t":"u64","bits":64,"v":"2"}`); invalid input gets a bare
   400. To test hostile input against those endpoints I had to capture a real request from the
   browser first.
9. **Validation and forms are yours.** Form parsing works, but there is no validation layer yet
   (it is on the roadmap). Errors travel back as a redirect with an error code in the URL.
10. **Thin docs for the data layer.** Toasty's generated query methods (`filter_by_*` only for
    indexed fields, `.first()`, `.update().field(...)`) were found mostly by reading the
    source and compiler errors. Once found, they were pleasant to use.
11. **Error plumbing.** `ok_or_redirect` returns its own error type, so you convert with `?`
    before returning a `topcoat::Result`.

## What I would want before using it for something long-lived

- A stable API (or at least a changelog written for upgraders).
- CSP support without `'unsafe-eval'`.
- Generated migrations that handle `NOT NULL` additions on existing data.
- Documented shard and procedure wire formats, plus a way to call them from tests.
- A validation story for forms.

## Verdict

For a small internal tool or a proof of concept, building full-stack in one language was a
real pleasure, and the safe defaults meant most of the security work was *verifying* things
rather than *building* them. For anything that must survive years of upgrades, wait for a 1.0
or budget for tracking the framework closely.
