# Deploying to an on-premises Windows Server

This guide runs the app natively on Windows Server, without Docker: the release executable as
a Windows service, with Caddy in front of it for HTTPS. For everything that is the same on any
platform (settings, sign-ups, password resets, security headers) read
[DEPLOYMENT.md](DEPLOYMENT.md); this page only covers what is different on Windows.

> **Out of date: written before the move to SQL Server.** The app no longer uses SQLite. It
> stores its data in Microsoft SQL Server (see [DEPLOYMENT.md](DEPLOYMENT.md)), and a
> `DATABASE_URL=sqlite:...` setting now stops it at startup. Wherever this page mentions the
> SQLite file, its data folder, or backing up and restoring a file, that no longer applies: the
> app needs a SQL Server it can reach (`DATABASE_URL=sqlserver://user:password@host:1433/todoapp`),
> backed up with SQL Server's own `BACKUP DATABASE` as described in DEPLOYMENT.md. The service,
> Caddy and HTTPS parts are unaffected. This page has not been revised for SQL Server.

> **Untested.** Unlike [DEPLOYMENT.md](DEPLOYMENT.md), the steps below were **not** run on a
> Windows Server. The app itself builds and runs on Windows (it is developed there), and the
> commands use standard Windows tooling, but treat this as a checked-by-reading recipe and try
> it on a test server first. Things most worth confirming are marked **check**.

## What you end up with

```
browser --HTTPS:443--> Caddy (service) --HTTP:3000, localhost only--> todo-app.exe (service)
                                                                          |
                                                                      app.db (SQLite)
```

- `todo-app.exe` listens on `127.0.0.1:3000`. Nothing but the proxy on the same machine can
  reach it.
- Caddy terminates HTTPS and forwards to it. Only port 443 (and 80, for the redirect) is open.
- Both run as services under [NSSM](https://nssm.cc), so they start at boot and restart if they
  crash. [WinSW](https://github.com/winsw/winsw) works the same way if your organization
  prefers it; the executable is not itself a Windows service, so it needs a wrapper.

Run **one** instance, as everywhere (see "Read this first" in [DEPLOYMENT.md](DEPLOYMENT.md)).

## 1. Build the release bundle

Build on any Windows machine with Rust (a developer PC or a CI runner). The server needs no
Rust, Node or Tailwind. The build downloads the Tailwind CLI and font files, so it needs network
access.

```powershell
cargo install topcoat-cli --version 0.12.0 --locked
cd todo-app
topcoat asset bundle --release
```

This produces two things that **must be deployed together and come from the same build**:

- `target\release\todo-app.exe`
- `target\release\assets\` (the stylesheet, fonts and browser runtime)

The database migrations are compiled into the executable. Running a plain `cargo build` binary
without the bundle stops at startup with `no asset bundle at ...`.

## 2. Lay out the server

Pick locations and keep the program separate from the data, so upgrades never touch the
database:

| Path | What | Who may write |
| --- | --- | --- |
| `C:\Apps\todo-app\` | `todo-app.exe` and the `assets\` folder next to it | Administrators only |
| `D:\TodoData\` | `app.db`, and later backups | The service account |
| `C:\Apps\caddy\` | `caddy.exe` and `Caddyfile` | Administrators only |
| `C:\Apps\nssm\` | `nssm.exe` | Administrators only |

Copy the executable and `assets\` to `C:\Apps\todo-app\`. Download Caddy for Windows from
<https://caddyserver.com/download> and NSSM from <https://nssm.cc/download>.

Run the service under a low-privilege account. `NT AUTHORITY\LocalService` needs no password:

```powershell
New-Item -ItemType Directory -Force D:\TodoData | Out-Null
icacls D:\TodoData /grant "NT AUTHORITY\LocalService:(OI)(CI)M"
icacls C:\Apps\todo-app /grant "NT AUTHORITY\LocalService:(OI)(CI)RX"
```

The service account needs to read the program folder, and to create and modify files in the data
folder (SQLite creates the database file there, so give it the folder, not just a file).

## 3. Install the app as a service

Open PowerShell **as Administrator**. Adjust the host name, paths and settings; the variables are
the ones in [DEPLOYMENT.md](DEPLOYMENT.md#configuration).

```powershell
$nssm = "C:\Apps\nssm\nssm.exe"

& $nssm install TodoApp "C:\Apps\todo-app\todo-app.exe"
& $nssm set TodoApp AppDirectory "C:\Apps\todo-app"
& $nssm set TodoApp ObjectName "NT AUTHORITY\LocalService"
& $nssm set TodoApp Start SERVICE_AUTO_START

# One KEY=value per argument. The database path uses forward slashes.  (check)
& $nssm set TodoApp AppEnvironmentExtra `
  "HOST=127.0.0.1" `
  "PORT=3000" `
  "DATABASE_URL=sqlite:D:/TodoData/app.db" `
  "REGISTRATION=closed" `
  "PUBLIC_URL=https://todo.corp.example.com" `
  "ENABLE_HSTS=1" `
  "TRUSTED_PROXY_HOPS=1"

# Logs, rotated, and restart on failure.
New-Item -ItemType Directory -Force C:\Apps\todo-app\logs | Out-Null
icacls C:\Apps\todo-app\logs /grant "NT AUTHORITY\LocalService:(OI)(CI)M"
& $nssm set TodoApp AppStdout "C:\Apps\todo-app\logs\app.log"
& $nssm set TodoApp AppStderr "C:\Apps\todo-app\logs\app.log"
& $nssm set TodoApp AppRotateFiles 1
& $nssm set TodoApp AppRotateBytes 10485760
& $nssm set TodoApp AppExit Default Restart
& $nssm set TodoApp AppRestartDelay 5000

Start-Service TodoApp
```

Why those settings:

- **`HOST=127.0.0.1`** keeps the app off the network. The Docker image listens on `0.0.0.0`
  because the container network is private; here the proxy is on the same machine, so loopback
  is right.
- **`TRUSTED_PROXY_HOPS=1`** tells the login rate limiter that exactly one proxy (Caddy) sits in
  front, so it reads the real client address instead of treating everyone as `127.0.0.1`. Without
  it, a few failed logins from anyone lock everyone out. This is only safe because nothing but
  Caddy can reach the app.
- **`REGISTRATION=closed`** means you create accounts yourself (step 5).
- **`PUBLIC_URL`** is the address people use, `https://` and no path.

Check it came up. The app applies its database migrations on first start:

```powershell
Get-Content C:\Apps\todo-app\logs\app.log -Tail 20
& "C:\Apps\todo-app\todo-app.exe" healthcheck; $LASTEXITCODE     # 0 means healthy
```

`healthcheck` asks the server at the configured `HOST` and `PORT`, so run it from a session that
has the same `PORT` (the default, 3000, matches). **check**: if it reports unhealthy while the log
looks fine, open <http://127.0.0.1:3000/healthz> in a browser on the server; it should say `ok`.

## 4. HTTPS with Caddy

The login cookie is `Secure`, so a browser will not keep it over plain HTTP from any host except
`localhost`. HTTPS is required, not optional.

Create `C:\Apps\caddy\Caddyfile`:

```caddyfile
todo.corp.example.com {
	encode zstd gzip

	# Caddy sets X-Forwarded-For to the address it actually saw and discards any value a
	# client sent, which is what TRUSTED_PROXY_HOPS=1 relies on.
	reverse_proxy 127.0.0.1:3000
}
```

This is the repository's [deploy/Caddyfile](../deploy/Caddyfile) with the container name
(`app:3000`) replaced by the loopback address. Which certificate Caddy uses depends on the
server:

| Situation | Add inside the site block | Notes |
| --- | --- | --- |
| Public DNS name, server reachable from the internet on 80 and 443 | nothing | Caddy gets and renews a Let's Encrypt certificate itself. |
| Internal name, you run an internal CA | `tls C:\Apps\caddy\todo.crt C:\Apps\caddy\todo.key` | Issue the certificate from your CA; clients already trust it through Group Policy. Restart Caddy to load a renewed one. |
| Internal name, no CA | `tls internal` | Caddy issues from its own local CA. Every client must trust Caddy's root certificate (`root.crt` under Caddy's data folder), or browsers show a warning. Distribute it by Group Policy. |

Install Caddy as a service the same way:

```powershell
& $nssm install Caddy "C:\Apps\caddy\caddy.exe" "run --config C:\Apps\caddy\Caddyfile"
& $nssm set Caddy AppDirectory "C:\Apps\caddy"
& $nssm set Caddy Start SERVICE_AUTO_START
& $nssm set Caddy AppExit Default Restart
New-Item -ItemType Directory -Force C:\Apps\caddy\logs | Out-Null
& $nssm set Caddy AppStdout "C:\Apps\caddy\logs\caddy.log"
& $nssm set Caddy AppStderr "C:\Apps\caddy\logs\caddy.log"
Start-Service Caddy
```

Caddy binds ports 80 and 443. If something else already holds them (IIS and `http.sys` often do
on Windows Server), find it with `netstat -ano | findstr ":443"` and either stop that site or put
the app behind it instead (below).

### Using IIS instead of Caddy

If IIS already serves HTTPS on the machine and you would rather use it, install **URL Rewrite**
and **Application Request Routing (ARR)**, enable the proxy in ARR, and add a rewrite rule that
forwards to `http://127.0.0.1:3000`. Two things must be right, or the app misbehaves:

- Keep the original `Host` header (turn off "Reverse rewrite host in response headers"), and
- make IIS **replace** `X-Forwarded-For` with the connecting address rather than appending to it.
  If a client-supplied value is passed through, a visitor can invent an address and escape the
  login rate limit. `TRUSTED_PROXY_HOPS=1` is only correct when the proxy does this.

This route is **untested** with this app. Caddy is the recommended one because the repository's
guide verified its `X-Forwarded-For` behavior.

## 5. Open the firewall and create your account

Allow only the proxy's ports. The app's own port stays closed, because it listens on loopback
only:

```powershell
New-NetFirewallRule -DisplayName "To-Do HTTPS" -Direction Inbound -Protocol TCP -LocalPort 443 -Action Allow
New-NetFirewallRule -DisplayName "To-Do HTTP redirect" -Direction Inbound -Protocol TCP -LocalPort 80 -Action Allow
```

Add `-RemoteAddress 10.0.0.0/8` (your network) to limit who can connect if it is internal-only.

Then create the first account. The password is read from standard input so it never appears in a
process list or shell history. The command needs the same `DATABASE_URL` the service uses, because
it writes straight to the database:

```powershell
$env:DATABASE_URL = "sqlite:D:/TodoData/app.db"
"a long password, 12 to 128 characters" | & "C:\Apps\todo-app\todo-app.exe" create-user you@corp.example.com
```

Your administrator account needs write access to `D:\TodoData\app.db`. Either grant it, or run the
command from a shell that runs as `LocalService` (for example with PsExec,
`psexec -u "NT AUTHORITY\LocalService"`). **check**: if the database ends up owned by you and the
service cannot write to it afterwards, re-run the `icacls` command from step 2 on `D:\TodoData`.

Now browse to <https://todo.corp.example.com> from another machine, log in, and add a todo. The
other operator commands work the same way from this shell:

```powershell
& "C:\Apps\todo-app\todo-app.exe" reset-link someone@corp.example.com
& "C:\Apps\todo-app\todo-app.exe" delete-user someone@corp.example.com --yes
& "C:\Apps\todo-app\todo-app.exe" send-test-email you@corp.example.com
```

Set `DATABASE_URL` (and, for `reset-link`, `PUBLIC_URL`) in the shell first, as above.

### Email for password resets

Optional. To turn on "Forgot your password?", add three variables to the service and restart it:

```powershell
& $nssm set TodoApp AppEnvironmentExtra `
  "HOST=127.0.0.1" "PORT=3000" "DATABASE_URL=sqlite:D:/TodoData/app.db" `
  "REGISTRATION=closed" "PUBLIC_URL=https://todo.corp.example.com" `
  "ENABLE_HSTS=1" "TRUSTED_PROXY_HOPS=1" `
  "SMTP_URL=smtp://relay.corp.example.com:587?tls=required" `
  "MAIL_FROM=To-Do <noreply@corp.example.com>"
Restart-Service TodoApp
```

`AppEnvironmentExtra` **replaces the whole list**, so repeat every variable, not just the new
ones. The connection to the mail server must be encrypted (`smtps://` or `?tls=required`), and the
server needs outbound access to your relay. Details and limits are in
[DEPLOYMENT.md](DEPLOYMENT.md#forgotten-passwords).

## 6. Backups

The database is one SQLite file. Back it up with the service stopped for a moment, so the copy is
consistent (a copy taken while the app is writing is not guaranteed to be). Save as
`C:\Apps\todo-app\backup.ps1`:

```powershell
$ErrorActionPreference = "Stop"
$src  = "D:\TodoData\app.db"
$dest = "\\fileserver\backups\todo"            # a location off this machine
$keep = 30                                     # days of backups to keep

Stop-Service TodoApp
try {
    Copy-Item $src (Join-Path $dest ("app-{0:yyyy-MM-dd}.db" -f (Get-Date)))
} finally {
    Start-Service TodoApp
}
Get-ChildItem $dest -Filter "app-*.db" |
    Where-Object LastWriteTime -lt (Get-Date).AddDays(-$keep) |
    Remove-Item
```

Schedule it nightly from Task Scheduler, running as an account that can control the service and
write to the destination:

```powershell
$action  = New-ScheduledTaskAction -Execute "powershell.exe" -Argument "-NoProfile -File C:\Apps\todo-app\backup.ps1"
$trigger = New-ScheduledTaskTrigger -Daily -At 2am
Register-ScheduledTask -TaskName "Backup To-Do" -Action $action -Trigger $trigger -User "SYSTEM" -RunLevel Highest
```

(`SYSTEM` also needs write access to the share; use a service account with a stored credential
instead if the share requires it.)

**Restore:**

```powershell
Stop-Service TodoApp
Copy-Item "\\fileserver\backups\todo\app-2026-11-09.db" D:\TodoData\app.db -Force
icacls D:\TodoData\app.db /grant "NT AUTHORITY\LocalService:M"
Start-Service TodoApp
```

Sessions live in the database, so a restore also restores who was logged in at the time of the
backup. Try a restore once before you depend on it; "Data and backups" in
[DEPLOYMENT.md](DEPLOYMENT.md#data-and-backups) says what was verified for the Docker version.
Also decide how long you keep old backups: a deleted account lives on in them until they rotate
away.

## 7. Upgrading

1. Back up (above).
2. Build the new bundle (step 1) and copy it over the old one:

```powershell
Stop-Service TodoApp
Copy-Item .\todo-app.exe C:\Apps\todo-app\todo-app.exe -Force
Remove-Item C:\Apps\todo-app\assets -Recurse -Force
Copy-Item .\assets C:\Apps\todo-app\assets -Recurse
Start-Service TodoApp
Get-Content C:\Apps\todo-app\logs\app.log -Tail 20     # look for "applied N database migration(s)"
```

Replace `assets\` completely rather than copying over it, so no stale files are left from the
old build. The new executable applies any new migrations when it starts. Migrations only go
forward, so to roll back, restore the backup **and** put the previous executable and `assets\`
back. If a migration fails the app exits with `database migration failed`; restore the backup
rather than assuming it rolled back cleanly.

## Go-live checklist

Everything in the [main checklist](DEPLOYMENT.md#go-live-checklist) applies, with the Windows
equivalents:

- [ ] The site is served over HTTPS on its real name; clients trust the certificate (no browser
      warning), and HTTP redirects to it.
- [ ] `ENABLE_HSTS=1` and `TRUSTED_PROXY_HOPS=1` are set on the `TodoApp` service, and port 3000
      is not reachable from other machines (try it from one).
- [ ] Both services (`TodoApp`, `Caddy`) are set to start automatically; reboot the server once
      and confirm the site comes back by itself.
- [ ] The backup task runs, the files land off the machine, and a restore has been tried.
- [ ] `REGISTRATION` is what you intend, and you have created your own account.
- [ ] If email is on: `send-test-email` succeeded and a real reset email arrived.
- [ ] Windows Update restarts are planned for, or the services are verified to return after them.

## Troubleshooting

| Symptom | Likely cause and fix |
| --- | --- |
| `TodoApp` starts then stops at once | Read `C:\Apps\todo-app\logs\app.log`. `configuration error: ...` names the bad variable; `no asset bundle at ...` means `assets\` is missing next to the exe; a database error usually means the service account cannot write `D:\TodoData`. |
| Logging in just returns to the login page | The page is not on HTTPS, so the browser drops the `Secure` cookie. Use the HTTPS address. |
| Everyone gets "Too many attempts" at once | The app sees only the proxy's address. Make sure `TRUSTED_PROXY_HOPS=1` is set on the service and restart it. |
| Caddy will not start: address already in use | Another program has port 80 or 443 (often IIS or `http.sys`). `netstat -ano \| findstr ":443"` shows the process id. |
| Browser warns the certificate is untrusted | The certificate comes from a CA the clients do not trust. Install your internal CA, or Caddy's root for `tls internal`, on clients. |
| Settings changed but nothing happened | Service environment variables are read at start: `Restart-Service TodoApp`. And `AppEnvironmentExtra` replaces the whole list. |
| `create-user` says it cannot open the database | `DATABASE_URL` is not set in your shell, or your account cannot write `D:\TodoData`. |
