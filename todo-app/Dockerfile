# syntax=docker/dockerfile:1.7

# ---- Build ---------------------------------------------------------------------------
# Needs network access: the build downloads the Tailwind CLI (build.rs) and the Geist font
# files (bundled into the app so visitors' browsers never contact a CDN).
FROM rust:1.99-bookworm AS build

# The Topcoat CLI bundles the app's assets (stylesheet, fonts, browser runtime) into an
# `assets/` directory next to the executable. Pin it to the version the app depends on.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo install topcoat-cli --version 0.12.0 --locked

WORKDIR /src
COPY Cargo.toml Cargo.lock build.rs styles.css components.toml ./
COPY src ./src
# The database migrations are compiled into the executable.
COPY toasty ./toasty

# One step builds the executable and its asset bundle, and copies both out of the cache
# mount; the two must always come from the same build.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    topcoat asset bundle --release \
 && mkdir -p /out \
 && cp target/release/todo-app /out/todo-app \
 && cp -r target/release/assets /out/assets

# ---- Run -----------------------------------------------------------------------------
FROM debian:bookworm-slim

# An unprivileged user, and a directory it owns for the SQLite database.
RUN useradd --system --uid 10001 --home-dir /app --shell /usr/sbin/nologin app \
 && mkdir -p /data \
 && chown app:app /data

WORKDIR /app
COPY --from=build /out/todo-app /app/todo-app
COPY --from=build /out/assets /app/assets

ENV HOST=0.0.0.0 \
    PORT=3000 \
    DATABASE_URL=sqlite:/data/app.db

# The database lives here; mount a volume to keep it.
VOLUME ["/data"]
EXPOSE 3000
USER app

# The binary checks itself, so the image needs no curl.
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD ["/app/todo-app", "healthcheck"]

# Applies pending database migrations, then serves. Stops cleanly on SIGTERM.
ENTRYPOINT ["/app/todo-app"]
