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
COPY migrations ./migrations

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

# OpenSSL and CA certificates, which the SQL Server driver uses for its encrypted
# connection, and an unprivileged user to run as.
RUN apt-get update \
 && apt-get install -y --no-install-recommends libssl3 ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home-dir /app --shell /usr/sbin/nologin app

WORKDIR /app
COPY --from=build /out/todo-app /app/todo-app
COPY --from=build /out/assets /app/assets

# The database is a separate SQL Server; give it to the app with DATABASE_URL (see
# docker-compose.yml). The app keeps nothing on its own disk.
ENV HOST=0.0.0.0 \
    PORT=3000

EXPOSE 3000
USER app

# The binary checks itself, so the image needs no curl.
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD ["/app/todo-app", "healthcheck"]

# Creates the database if needed, applies pending migrations, then serves. Stops cleanly on
# SIGTERM.
ENTRYPOINT ["/app/todo-app"]
