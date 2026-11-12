# Deploying the to-do app

This guide covers running the app for real: a container, HTTPS, configuration, backups and
upgrades. Everything here was tried against the files in this repository (Docker 29, Windows
host); the few things that were not are marked **untested**.

## Read this first

- **The app needs HTTPS** (except on `localhost`). The login cookie is `__Host-`-prefixed and
  `Secure`, so a browser will not store or send it over plain HTTP from any other host, and
  logging in appears to do nothing. The compose setup below gets certificates automatically.
- **Run one app instance.** The login rate limiter lives in memory, so two copies would each have
  their own limits. (They could share the SQL Server database; only the limiter is per instance.)
- **The database is SQL Server, in its own container.** The compose file starts one, with a
  password you choose (`MSSQL_SA_PASSWORD`). SQL Server needs about 2 GB of memory and its image
  is about 2.2 GB on disk. The compose file accepts Microsoft's license terms for you (`ACCEPT_EULA=Y`;
  read them) and runs the free **Express** edition (10 GB database limit), which is allowed in
  production. The Developer edition used by `docker-compose.dev.yml` is **not licensed for
  production**; set `MSSQL_PID` in `.env` if you hold a license for another edition.
- **Sign-ups are a deployment decision.** The app itself defaults to `REGISTRATION=open`
  (anyone who can reach it can create an account), but the compose file sets `closed`: nobody
  can sign up on the website, and you create accounts from the command line (see "Accounts and
  sign-ups"). Pick the mode on purpose.
- **Email is optional and off by default.** People can change a password they know (Account
  page). A forgotten one is reset with a one-time link: either the site emails it (set
  `SMTP_URL`, `MAIL_FROM` and `PUBLIC_URL`; see "Forgotten passwords") or, with email off, *you*,
  the operator, make the link and hand it over.

## Quick start: Docker Compose with HTTPS

You need Docker with Compose, and for a real domain: DNS pointing at the server and ports 80
and 443 open (Caddy uses them to obtain certificates).

```sh
cp .env.example .env          # then set MSSQL_SA_PASSWORD and SITE_ADDRESS=todo.example.com
docker compose up -d --build
```

`MSSQL_SA_PASSWORD` is required (compose refuses to start without it) and SQL Server rejects weak
ones: at least 8 characters, from three of upper case, lower case, digits and symbols. Use letters
and digits if you can: the password is also placed in the app's `DATABASE_URL`, where special
characters would need percent-encoding.

Then create your account (sign-ups are closed by default; the password is read from standard
input, so it never appears in process listings or shell history):

```sh
printf '%s\n' "a long password, 12 to 128 characters" \
  | docker compose exec -T app /app/todo-app create-user you@example.com
```

For a local trial leave `SITE_ADDRESS=localhost` and open <https://localhost>; your browser will
warn about the certificate because Caddy issues it from its own local authority.

This starts three containers:

| Service | What it is |
| --- | --- |
| `db` | Microsoft SQL Server 2022 (`mcr.microsoft.com/mssql/server`), the app's database, with its data in the `db-data` volume. Its port is **not** published; only the app can reach it, on the compose network. |
| `app` | The Rust app, built from the [Dockerfile](../Dockerfile). Starts once the database is healthy, creates the `todoapp` database if it is missing, and applies migrations. Runs as an unprivileged user on a read-only filesystem with all Linux capabilities dropped, and keeps nothing on its own disk. Its port is **not** published; only the proxy can reach it. |
| `proxy` | [Caddy](https://caddyserver.com/): terminates HTTPS, redirects HTTP to HTTPS (308), compresses responses and forwards to the app. |

The first build takes a few minutes (it compiles in release mode and downloads the Tailwind CLI
and the font files). It needs network access. Later builds reuse the cache.

### What was verified

With SQL Server, on Docker 29 / Windows (the full compose stack, built from this repository):

- SQL Server became healthy about 40 seconds after `docker compose up`; the app, which waits for
  it, was healthy about 25 seconds after that. On its first start the app created the `todoapp`
  database and applied the one migration (`applied 1 database migration(s)`).
- Through the proxy over HTTPS: `/healthz` answers `ok`, and logging in redirects to `/todos`,
  which loads with the `__Host-` session cookie.
- `create-user`, `reset-link` and the `delete-user` preview work inside the app container.
- A backup taken while the app was running, and a restore of it, round-trip (below): a user
  created after the backup was gone after the restore, the others were intact, and the app was
  healthy again.
- The whole test suite (166 tests, most of them against a real SQL Server) passes, and CI runs it
  against a SQL Server service container.

Verified earlier, before the move to SQL Server (these do not touch the database, but were not
repeated afterwards): `docker stop` ends the app in under a second with exit code 0 (it handles
`SIGTERM`); the security headers (including HSTS) reach the browser; and a spoofed
`X-Forwarded-For` does not dodge the login rate limit (see "Behind a reverse proxy").

## Configuration

All settings are environment variables and all are optional. (The compose file additionally
requires `MSSQL_SA_PASSWORD`, which configures the database container.) Mistakes (a typo in a
flag, a bad network, a malformed database URL) stop the app at startup with exit code 2 and a message that
names the variable; the database URL is never echoed back, since it could hold a password.

| Variable | Default | Meaning |
| --- | --- | --- |
| `HOST` | `127.0.0.1` (image: `0.0.0.0`) | Address to listen on. |
| `PORT` | `3000` | Port to listen on. |
| `DATABASE_URL` | the local dev container: `sqlserver://sa:...@127.0.0.1:1433/todoapp?trust_server_certificate=true` (compose sets its own) | The SQL Server database: `sqlserver://user:password@host:1433/database`. Options: `trust_server_certificate=true` accepts a self-signed certificate (the SQL Server container has one), `encrypt=true` (the default), `false` or `strict`. Percent-encode special characters in the user, password and database name. If the database does not exist, the app creates it (the login needs permission to). Contains a password: never printed. Only SQL Server is supported. |
| `MIGRATE_ON_START` | `true` | Apply pending database migrations at startup. Set `false` only if you run migrations another way. |
| `REGISTRATION` | `open` (compose: `closed`) | Who may sign up on the website: `open`, `closed` or `first-user`. See "Accounts and sign-ups". |
| `PUBLIC_URL` | none | The site's public address, for example `https://todo.example.com` (scheme and host, no path). Used to write complete links: by `reset-link`, and in emailed reset links (it is the only source of the address in those, never the request's `Host` header). Anything with a path or odd characters stops the app at startup. |
| `SMTP_URL` | none | Turns on emailed password reset links. `smtps://user:pass@smtp.example.com:465` (implicit TLS) or `smtp://user:pass@smtp.example.com:587?tls=required` (STARTTLS). Percent-encode special characters in the user name and password. Plain `smtp://` without `tls=required` is accepted only for `localhost`, to try the app against a local mail catcher. Needs `MAIL_FROM` and `PUBLIC_URL`. The value holds a password and is never printed. |
| `MAIL_FROM` | none | The sender of those emails, `noreply@example.com` or `Todo <noreply@example.com>`. Required with `SMTP_URL`; setting it alone stops the app at startup. |
| `TRUSTED_PROXIES` | none | Comma-separated IP addresses or CIDR networks of reverse proxies, for example `10.0.0.0/8,172.16.0.0/12`. |
| `TRUSTED_PROXY_HOPS` | `0` | How many reverse proxies are in front, for when their addresses vary. The compose file sets `1`. |
| `ENABLE_HSTS` | off | Send `Strict-Transport-Security`. Turn on only when the site is served over HTTPS. The compose file does. |
| `CSP_REPORT_ONLY` | off | Send the Content-Security-Policy as report-only: browsers log violations but block nothing. For trying a policy change safely. |

`TOPCOAT_DEV_URL` is set by `topcoat dev` and lets the live-reload script run. It must not be set
in production.

### Behind a reverse proxy

The login rate limiter counts failures per client address. Behind a proxy, the address the app
sees is the proxy's, so without configuration **every user shares one limit**: a few failed
logins from anyone would lock everyone out. Tell the app which proxies to trust with
`TRUSTED_PROXIES` (preferred, when their addresses are known) or `TRUSTED_PROXY_HOPS`, and it
reads the real client from `X-Forwarded-For`.

This only works if the proxy sets that header to the address it actually saw and discards any
value the client sent; otherwise a client could invent its own address and escape the limit.
Caddy does this by default. It was tested: seven failed logins for one account, each claiming a
different `X-Forwarded-For`, were throttled at the sixth, and the correct password was refused
too.

`TRUSTED_PROXY_HOPS` trusts a *position*, so use it only when clients cannot reach the app
without passing through all the proxies. That is why the compose file does not publish the
app's port.

For another proxy such as nginx, the requirements are the same: terminate HTTPS, forward the
`Host` header, and set `X-Forwarded-For` to the connecting address, for example
`proxy_set_header X-Forwarded-For $remote_addr;` (which replaces rather than appends).
**Untested** with this app.

### Security headers

Every response carries a Content-Security-Policy, `X-Content-Type-Options`, `X-Frame-Options`,
`Referrer-Policy`, `Permissions-Policy` and the `Cross-Origin-*` policies, and everything except
hashed assets is `Cache-Control: no-store`. One response is the exception: Topcoat rejects a
cross-site state-changing request with a plain 403 *before* any application code runs, so that
reply has none of these headers. It has no page to attack, but if you want it covered, add the
same headers in your proxy.

The Content-Security-Policy allows `'unsafe-eval'` because Topcoat's browser runtime requires it;
see [DX-NOTES.md](DX-NOTES.md).

## Accounts and sign-ups

`REGISTRATION` controls the website's sign-up page:

| Value | Behavior | Use it when |
| --- | --- | --- |
| `open` | Anyone who can reach the site can create an account. | A public service, or a private network you trust. |
| `closed` | Nobody can. The sign-up page says so, the login page stops linking to it, and a POST to it is refused before anything is validated or stored. | Almost every private deployment. **The compose default.** |
| `first-user` | Anyone, until the first account exists; then nobody. | You will register immediately after deploying. |

Be careful with `first-user`: until that first account exists, whoever reaches the site first
owns it. With automatic HTTPS a new site is reachable within seconds, so on a public server prefer
`closed` and create the account yourself. (If two people submit the form at the same moment, only
the earliest account is kept; the other is removed and told sign-ups are closed.)

**Creating accounts when sign-ups are closed.** `todo-app create-user <email>` writes the account
straight to the database, whatever `REGISTRATION` says. It reads the password from standard
input, so pipe it in rather than typing it as an argument:

```sh
printf '%s\n' "a long password" | docker compose exec -T app /app/todo-app create-user you@example.com
```

It applies the same rules as the sign-up page (a plausible email, a password of 12 to 128
characters, an email not already used, case-insensitively), prints `created account <email>`,
and never echoes the password. It exits 0 on success, 1 if the account was refused (the message
says why) and 2 if it was used wrongly (no password on standard input, wrong arguments). Only the
line ending is removed from the password, so spaces at the end are part of it. If it is run in a
terminal it says the typed password will be visible. It works against a brand-new database too.

Closing sign-ups does not affect logging in, changing a password, or anyone's existing account.
A typo in the setting (for example `REGISTRATION=colsed`) stops the app at startup rather than
leaving sign-ups open.

### Deleting accounts

**By the person.** The Account page has a "Delete account" section. It asks for the current
password and for the account's email address to be typed out, and then deletes, in one
transaction, the user's sessions, todos and lists, and then the account itself. Either everything
is removed or nothing is: a failure part-way rolls the whole deletion back. The browser is sent to
the login page and its session cookie is cleared; every other device is signed out too, because
its session no longer exists. The password check uses the same rate limit as changing the
password (5 wrong answers per 15 minutes per account, shared between the two forms), so a stolen
session cannot be used to guess the password through either form. A mistyped confirmation is
reported without counting as a guess. Afterwards the same email address can sign up again, and
starts with nothing.

**By the operator** (for example on a site with sign-ups closed). `todo-app delete-user <email>`
deletes nothing by default; it prints what the account owns and exits with code 2. Add `--yes` to
delete:

```sh
docker compose exec -T app /app/todo-app delete-user someone@example.com          # preview
docker compose exec -T app /app/todo-app delete-user someone@example.com --yes    # delete
```

It exits 0 when it deleted the account, 1 when there is no such account (or it failed, in which
case nothing was changed), and 2 when it was used wrongly or was only a preview. The email is
matched ignoring case. It worked against a running server in testing.

Things to know:

- **Backups still contain the data.** Deleting an account removes it from the live database only.
  Older backups keep it until you rotate them away, so decide how long you keep them if you have
  a duty to erase data promptly.
- **In `first-user` mode, deleting the only account reopens sign-ups**, because there is then no
  first account. Use `closed` if that matters.
- **Deleted is deleted.** There is no recycle bin, undo or grace period.
- Only the database is touched. The app stores a user's todos, lists and sessions nowhere else
  (it has no uploads); anything outside the app, such as proxy logs or copies of the database
  file, is yours to deal with.

## Forgotten passwords

There are two ways to get a reset link to someone. Both use the same link, with the same rules
(below). Emailing is optional; the operator route always works.

### Emailed links ("Forgot your password?")

Set `SMTP_URL`, `MAIL_FROM` and `PUBLIC_URL` (all three; the app refuses to start with only some).
The login page then links to `/forgot`, where a person types their address and the site emails
them a link. Check the settings first, without waiting for anyone to forget anything:

```sh
docker compose exec -T app /app/todo-app send-test-email you@example.com
```

That sends one message that contains no link and exits 0 if the mail server accepted it, 1 if not,
2 if email is not configured. Use a mail account meant for sending (a transactional email service
or your provider's SMTP submission account), and set up SPF/DKIM for the sending domain, or the
messages will land in spam. That part is outside this app.

How it behaves (each point is covered by a test or was tried against a local SMTP server):

- **Same answer for every address.** The form says "If that address belongs to an account, a link
  ... is on its way" whether or not it does, and replies in the same few milliseconds: the lookup,
  the token and the mail-server conversation happen in the background after the reply. In the live
  check the mail server took 2 seconds per message and both kinds of request still answered in
  about 3 ms. The site therefore cannot be used to find out who has an account.
- **Rate limited.** 3 requests per address per hour, and 10 per network per 15 minutes (the
  network is the client's address as the proxy reports it, so set `TRUSTED_PROXY_HOPS` or
  `TRUSTED_PROXIES` as described above). Requests for unknown addresses count too, so the limit
  looks the same for every address. A network over its limit is told so; an address over its limit
  is silently ignored.
- **The link is built from `PUBLIC_URL`**, never from the request's `Host` header, which a visitor
  controls. `PUBLIC_URL` must be `https://` when email is on (`http://` only for `localhost`).
- **The connection to the mail server must be encrypted** (see `SMTP_URL`), because the message
  carries a login-equivalent secret.
- **Failures are logged without details.** If the mail server is down, the person still sees the
  normal reply (they cannot be told), and the app logs `could not email a password reset link:
  could not deliver the mail`. The address and the link are not logged. Nothing is retried: the
  person asks again.
- **Someone who knows an address can cancel its owner's latest link** by asking for a new one
  (a newer link cancels the older). They cannot read the link or take the account, and the rate
  limit stops them doing it more than three times an hour.
- **The mailbox becomes the key to the account.** Whoever can read the account's email can reset
  its password, as on most sites. Using a link signs every session out, so the real owner notices.

Not covered: bounces and delivery reports (the app only knows the mail server *accepted* the
message), HTML emails and translations (the message is plain text), and mail-server failures longer
than one request (there is no queue).

### Links made by the operator

The operator makes a link and passes it to the person (by a channel they already trust: in person,
a chat message, a phone call). This works with or without email:

```sh
docker compose exec -T app /app/todo-app reset-link someone@example.com
```

```
Password reset link for someone@example.com (works once, for 60 minutes; a newer link cancels it):
https://todo.example.com/reset?token=<64 hex characters>
```

Set `PUBLIC_URL` so the command prints the complete address; without it only the path is printed
(`/reset?token=...`) and you put the site's address in front. The command exits 0 when it made a
link, 1 when there is no such account, and 2 when it was used wrongly.

**What the person does:** opens the link, which shows a form and changes nothing by itself,
chooses a new password twice, and is sent to the login page. Nobody is logged in by the reset, and
**every session of the account is signed out**, including a thief's.

**Security properties** (each is covered by a test):

- The token is 256 random bits, 64 hex characters. Only its SHA-256 hash is stored, so reading the
  database (or a backup) yields no usable link.
- A link works **once** and for **one hour**. Making a new link for the same account cancels the
  old one, so an account has at most one live link.
- A wrong, malformed, expired, used or cancelled link all look the same: "not valid". Nothing in
  the page reveals whether an account exists. (The emailed route answers every request alike; see
  above.)
- Bad links are limited to 20 per 15 minutes per network (counted separately from logins). Past
  that, even a real link is refused until the window passes.
- A password the form refuses (too short, a typo in the confirmation) does not use up the link.
- Deleting an account deletes its pending link. Expired links are swept out hourly.

**Handle a link like a password.** Whoever holds it can take over the account until it is used or
expires. It travels in a URL, so it can appear in the operator's terminal history, in chat logs,
and in a reverse proxy's access logs if the proxy logs query strings; the app itself does not log
it, and the page is served with `Referrer-Policy: same-origin` and `Cache-Control: no-store` so it
does not leak onward. If you suspect a link was seen by the wrong person, make a new one, which
cancels it.

## Health checks

`GET /healthz` returns `200 ok` when the app can reach its database and needs no login. The
image also contains `todo-app healthcheck`, which asks the local server the same question using
only the standard library (the image has no `curl`), and exits 0 or 1. Docker runs it every 30
seconds. When a container is unhealthy:

```sh
docker compose logs app
```

## Data and backups

Everything the app stores is in the `todoapp` database, which lives in the `db-data` volume of
the `db` container (named `<project>_db-data`, for example `todo-app_db-data`). Do not back it up
by copying the volume's files while SQL Server is running; use SQL Server's own `BACKUP DATABASE`,
which is consistent and **works while the app keeps running**.

The commands below send the SQL to `sqlcmd` on standard input, and the container's own shell
supplies the password (`$MSSQL_SA_PASSWORD`), so nothing secret appears in your command line. They
were run exactly like this against the compose stack. On Windows with Git Bash, put
`MSYS_NO_PATHCONV=1` in front of them (or export it), or Git Bash rewrites the `/var/...` paths.

**Back up:**

```sh
mkdir -p backup
echo "BACKUP DATABASE [todoapp] TO DISK = N'/var/opt/mssql/data/todoapp.bak' WITH INIT, COMPRESSION, CHECKSUM" \
  | docker compose exec -T db sh -c '/opt/mssql-tools18/bin/sqlcmd -C -S localhost -U sa -P "$MSSQL_SA_PASSWORD" -b'
docker compose cp db:/var/opt/mssql/data/todoapp.bak backup/todoapp-$(date +%F).bak
```

The first command writes the backup inside the container (`WITH INIT` makes each backup replace
the last one there) and prints `BACKUP DATABASE successfully processed ...`; the second copies it
out to your machine. `CHECKSUM` makes SQL Server verify page checksums as it reads them. Check a
copy later with `RESTORE VERIFYONLY FROM DISK = ...`.

**Restore** (this replaces the current database with the backup's contents, so everything
changed since is lost; stop the app first so nothing is using it):

```sh
docker compose stop app
docker compose cp backup/todoapp-2026-11-12.bak db:/var/opt/mssql/data/restore.bak
echo "ALTER DATABASE [todoapp] SET SINGLE_USER WITH ROLLBACK IMMEDIATE; RESTORE DATABASE [todoapp] FROM DISK = N'/var/opt/mssql/data/restore.bak' WITH REPLACE; ALTER DATABASE [todoapp] SET MULTI_USER;" \
  | docker compose exec -T db sh -c '/opt/mssql-tools18/bin/sqlcmd -C -S localhost -U sa -P "$MSSQL_SA_PASSWORD" -b'
docker compose start app
```

Tried end to end: after restoring, a user created *after* the backup was gone, the users created
before it were back, and the app was healthy again. Sessions are stored in the database too, so a
restore also restores who was logged in at the time of the backup. The restored copy
(`restore.bak`) stays in the container's data folder until you delete it.

Schedule the backup commands with cron or your platform's scheduler, and copy the files off the
machine. Anything that is not copied elsewhere is not a backup.

## Upgrading

```sh
# 1. Back up (above).
git pull
docker compose up -d --build
```

The new container applies any new migrations when it starts, and `docker compose logs app` shows
`applied N database migration(s)`. Migrations only go forward, so to roll back, restore the
backup **and** run the previous version of the code.

If a migration fails the app exits with `database migration failed` and the error. All pending
migrations run in one transaction under a lock, so SQL Server undoes a failed one rather than
leaving it half applied, and the lock is there so that two app copies starting together cannot
both apply the same one. (The rollback of a failed *transaction* is covered by tests; a
deliberately broken migration and two simultaneous starts were not tried.) Keep the backup you took before upgrading regardless.

## Building without Docker

The release executable needs its asset bundle next to it, and the two must come from the same
build:

```sh
cargo install topcoat-cli --version 0.12.0 --locked
topcoat asset bundle --release        # builds target/release/todo-app and target/release/assets
DATABASE_URL='sqlserver://user:password@db.example.com:1433/todoapp' HOST=0.0.0.0 ./target/release/todo-app
```

This needs a SQL Server the app can reach (your own, or a container as in the compose file). On
Linux the binary uses the system's OpenSSL (`libssl3`) for the database connection; the Docker
image installs it.

Running a plain `cargo build` binary without the bundle panics at startup with
`no asset bundle at ...`.

For an on-premises Windows Server (a service, HTTPS, backups), see
[DEPLOYMENT-WINDOWS.md](DEPLOYMENT-WINDOWS.md).

## Go-live checklist

- [ ] The site is served over HTTPS on its real domain and HTTP redirects to it.
- [ ] `ENABLE_HSTS=1` (the compose file already sets it).
- [ ] `TRUSTED_PROXIES` or `TRUSTED_PROXY_HOPS` matches your proxy setup, and the app's port is
      not reachable except through the proxy.
- [ ] A backup is scheduled, copied off the machine, and a restore has been tried. You know how
      long you keep old backups, since deleted accounts live on in them.
- [ ] `.env` is not committed (it is in `.gitignore`). It holds the SQL Server `sa` password
      (`MSSQL_SA_PASSWORD`), which you chose yourself and is not the placeholder from
      `.env.example`; the `db` container's port is not published (the compose file does not).
- [ ] SQL Server's edition suits your license (`MSSQL_PID`; the compose default is Express).
- [ ] `REGISTRATION` is set the way you intend (the compose file defaults to `closed`), and you
      have created your own account with `create-user`.
- [ ] If you turned on email: `send-test-email` succeeded, a real reset email arrived (not in
      spam), and `PUBLIC_URL` is your real `https://` address.
- [ ] Base images are rebuilt periodically for security updates (`docker compose build --pull`).

## Troubleshooting

| Symptom | Likely cause and fix |
| --- | --- |
| Logging in just returns to the login page | The site is not on HTTPS (or `localhost`). The session cookie is `Secure`, so the browser drops it. Use HTTPS. |
| Everyone gets "Too many attempts" at once | The app does not know about the proxy, so all users look like one client. Set `TRUSTED_PROXY_HOPS=1` or `TRUSTED_PROXIES`. |
| `configuration error: ...` and exit code 2 | A setting is invalid; the message names the variable. |
| "Forgot your password?" never delivers | `docker compose logs app` for `could not email a password reset link`; run `send-test-email` to check the mail server settings; look in spam; check SPF/DKIM. An address asked about more than 3 times in an hour is ignored until the hour passes. |
| The app stops with `SMTP_URL: would send mail unencrypted` | Use `smtps://...` or `smtp://...?tls=required`. Plain `smtp://` is for `localhost` only. |
| A reset link says "not valid" | It was already used, is older than an hour, was cancelled by a newer link for the same account, or was cut off when copied (it is 64 characters after `token=`). Make a new one with `reset-link`. If many bad links were tried from one network recently, wait 15 minutes. |
| The sign-up page says "Sign-ups are closed" | `REGISTRATION` is `closed` (the compose default), or `first-user` and an account already exists. Create the account with `create-user`, or set `REGISTRATION=open`. |
| `no asset bundle at ...` on start | The executable was built without `topcoat asset bundle`, or the `assets/` folder is not next to it. |
| Page loads but is unstyled or buttons do nothing | Check the browser console for Content-Security-Policy messages. To experiment safely, set `CSP_REPORT_ONLY=1`. |
| `docker compose up` shows no mapping for port 80 or 443 | Another process already holds that host port. On Windows even built-in services can (`netstat -ano`). Set `HTTP_PORT` and `HTTPS_PORT` in `.env`. Note the HTTP-to-HTTPS redirect omits a non-standard HTTPS port. |
| The build fails downloading something | The build needs network access for the Tailwind CLI and the font files. |
| App container is `unhealthy` or exits | `docker compose logs app`; usually it cannot reach or sign in to SQL Server (`cannot open the database: ...`): check `docker compose ps` that `db` is healthy, and that `MSSQL_SA_PASSWORD` in `.env` is the one the database was first created with (changing it later does not change an existing database's password). |
| `db` never becomes healthy | `docker compose logs db`. SQL Server needs about 2 GB of memory and refuses a weak or missing `MSSQL_SA_PASSWORD`. |
