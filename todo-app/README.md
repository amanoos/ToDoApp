# Topcoat To-Do

A to-do app written entirely in Rust on [Topcoat](https://github.com/tokio-rs/topcoat), the
full-stack web framework from the Tokio team. It is a proof of concept: small enough to read in
an afternoon, but complete enough to show a real app (accounts, a database, interactive UI,
security, deployment) without WebAssembly and without a line of hand-written JavaScript.

> Topcoat is early-stage (0.x) and expects breaking changes. This project pins exact versions;
> see [docs/DX-NOTES.md](docs/DX-NOTES.md) for an honest account of what worked and what didn't.

## What it does

- **Accounts:** register (or have sign-ups closed and create accounts yourself), log in, log out,
  change your password (which signs out every other device), delete your own account along with
  everything in it, and recover a forgotten password with a one-time link (emailed by the site
  when you configure email, or handed over by the operator). Passwords are hashed with Argon2id; sessions are server-side, revocable and extended
  while you are active; failed logins and wrong current-password guesses are rate limited.
- **Todos:** add, edit, complete, delete. Each has notes, a priority and a due date.
- **Lists and sorting:** group todos into lists (deleting a list moves its todos to the Inbox) and
  sort by age, due date, priority or title.
- **Filters and live search:** All / Active / Completed, with counts, and search-as-you-type that
  updates only the list. Everything you choose is kept in the URL, so views can be shared.
- **Private by construction:** every query is scoped to the signed-in user, with tests that
  prove one user cannot read or change another's todos or lists.
- **A designed interface:** a colorful, animated, full-window layout with a progress dashboard,
  illustrations and frosted-glass cards, all in CSS and inline SVG (no images to download, no
  JavaScript beyond Topcoat's own runtime).
- **Security headers** (CSP, anti-framing, no-store caching and more) and a **self-hosted**
  font, so the page makes no third-party requests.
- **Deployable:** a Docker image, a compose file with an HTTPS reverse proxy, health checks and
  migrations that run at startup.

## Quick start

You need a recent stable Rust toolchain and network access (the first build downloads the
Tailwind CLI and the font files).

```sh
cargo install topcoat-cli --version 0.12.0 --locked
topcoat dev
```

Open <http://127.0.0.1:3000> (if that port is busy, `topcoat dev` picks the next free one and
prints it) and create an account. The database (`data.db`) and its tables are created for you;
edits to the source rebuild and refresh the page automatically.

For scale: from a clean copy of the project (no `target/`, no database), `topcoat dev` was serving
the login page after 78 seconds and sign-up worked, on a machine whose Cargo download cache was
already warm. On a fresh machine add the one-off `cargo install topcoat-cli` (about a minute) and
your download time.

## Configuration

Everything is optional. Copy [`.env.example`](.env.example) for a documented template.

| Variable | Default | What it does |
| --- | --- | --- |
| `HOST`, `PORT` | `127.0.0.1`, `3000` | Where to listen. |
| `DATABASE_URL` | `sqlite:data.db` | The SQLite database file. |
| `MIGRATE_ON_START` | `true` | Apply pending database migrations at startup. |
| `REGISTRATION` | `open` | Who may sign up: `open`, `closed` (nobody; use `create-user`), or `first-user` (until the first account exists). The compose file defaults to `closed`. |
| `PUBLIC_URL` | none | The site's public address (no path), so `reset-link` can print complete links and emailed links point at your site. |
| `SMTP_URL`, `MAIL_FROM` | none | Turn on emailed password reset links. `SMTP_URL` must be encrypted (`smtps://...` or `smtp://...?tls=required`; plain only for `localhost`); `MAIL_FROM` and an `https://` `PUBLIC_URL` are required with it. |
| `TRUSTED_PROXIES` / `TRUSTED_PROXY_HOPS` | none / `0` | Reverse proxies to trust for the client's address (needed behind a proxy, or all users share one login rate limit). |
| `ENABLE_HSTS` | off | Send `Strict-Transport-Security` (only when served over HTTPS). |
| `CSP_REPORT_ONLY` | off | Report Content-Security-Policy violations without blocking. |

Mistakes in these settings stop the app at startup with a clear message instead of being
ignored (a typo like `REGISTRATION=colsed` will not leave sign-ups open). See
[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) for details.

### Closing sign-ups

With `REGISTRATION=closed` nobody can create an account on the website: the sign-up page says so
and the login page stops linking to it. Make accounts from the command line instead; the password
is read from standard input, never from an argument, so it does not appear in process listings or
shell history:

```sh
printf '%s\n' "a long password" | ./todo-app create-user you@example.com
```

(In Docker: `... | docker compose exec -T app /app/todo-app create-user you@example.com`.)

### Deleting accounts

People can delete their own account from the **Account** page: it asks for their password and for
their email address to be typed out, then permanently removes the account, its todos and lists,
and every session. The operator can do the same from the command line, which deletes nothing
unless you add `--yes` (without it, it only reports what would be deleted):

```sh
./todo-app delete-user someone@example.com          # preview only
./todo-app delete-user someone@example.com --yes    # delete for real
```

Deletion cannot be undone, and backups taken earlier still contain the data until they are
rotated away. See [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md#accounts-and-sign-ups).

### Forgotten passwords

With email configured (`SMTP_URL`, `MAIL_FROM`, `PUBLIC_URL`), the login page offers **Forgot your
password?**: a person types their address and is emailed the link below. The form answers every
address the same way and just as fast (the sending happens in the background), so it does not
reveal who has an account, and it is rate limited per address and per network. Check your mail
settings with `./todo-app send-test-email you@example.com`. Details and limits are in
[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md#forgotten-passwords).

Without email, the operator makes a one-time link and gives it to the person by whatever channel
they trust (this also works with email on):

```sh
./todo-app reset-link someone@example.com
# Password reset link for someone@example.com (works once, for 60 minutes; a newer link cancels it):
# https://todo.example.com/reset?token=...
```

The link works **once**, for **an hour**, and making a newer one cancels the older. Opening it
shows a form; choosing a new password there signs the account out on every device and does not
log anyone in. Only a hash of the token is stored, so a copy of the database yields no usable
links. Treat a link like a password: whoever holds it can take over the account until it is used
or expires.

## Working on it

```sh
cargo test                                   # 100+ unit tests, including database tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The database tests run against an in-memory SQLite database. One of them builds the schema
**only from the embedded migrations** and then uses every model, so it fails if you change a
model without generating a migration.

After changing a model:

```sh
cargo run -- toasty migration generate       # writes toasty/migrations/NNNN_migration.sql
```

Read the generated SQL. SQLite cannot add a `NOT NULL` column without a default to a table that
already has rows, so such a migration needs a hand-written `DEFAULT` (see
`toasty/migrations/0002_migration.sql`). Then rebuild: migrations are compiled into the
executable and applied at startup.

## Project layout

| Path | Contents |
| --- | --- |
| `src/app.rs`, `src/app/` | The route tree: router, layout, and one module per page (`todos`, `lists`, `login`, ...). `shell.rs` holds the page frames (top bar, split-screen sign-in). |
| `src/features/` | Everything a feature owns: `todo` (model, validation, sorting, search), `list`, `user` (accounts, sessions), `reset` (one-time links), `forgot` (emailing them), `throttle` (rate limiter). Pure logic is unit-tested here. |
| `src/components/` | Shared UI components (button, input, card, `glyph` for icons). |
| `src/security.rs` | The security-headers layer and the Content-Security-Policy. |
| `src/config.rs`, `src/db.rs`, `src/healthcheck.rs` | Settings, the database and embedded migrations, the container health check. |
| `styles.css` | The theme: color tokens, backdrop, SVG artwork and animation. Restyle the app here. |
| `toasty/` | Database migrations. Commit them with the code. |
| `Dockerfile`, `docker-compose.yml`, `deploy/` | Container image, compose stack, Caddy config. |
| `docs/` | Deployment guide and developer-experience notes. |

Pages use features and components; features never import from `app`.

## Deploying

```sh
docker compose up -d --build
```

starts the app behind Caddy with automatic HTTPS. Read
[docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) first: the login cookie requires HTTPS, backups and
upgrades have a procedure, and the app is designed for a single instance.

## Known limitations

- Single instance only: the login rate limiter is in memory and the database is SQLite.
- Email is used for one thing only: reset links, as a plain-text message, sent once with no queue
  or retries and no bounce handling (the app learns only that the mail server accepted it).
  There is no email verification, so an account's address is whatever was typed at sign-up, and
  no separate "log out other devices" button.
- Search filters in memory over a user's todos: fine for hundreds, not for very large lists.
- Topcoat's browser runtime needs `'unsafe-eval'` in the Content-Security-Policy, so the CSP is a
  second line of defence behind output escaping, not a replacement for it.

## Documentation

- [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md): running it for real.
- [docs/DX-NOTES.md](docs/DX-NOTES.md): what building with Rust and Topcoat was like.
- [Topcoat's own guides](https://github.com/tokio-rs/topcoat/tree/main/docs).
