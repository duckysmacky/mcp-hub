ARG PYTHON_VERSION=3.13.18

FROM oven/bun:1.4.4-slim AS frontend
WORKDIR /build
COPY frontend/package.json frontend/bun.lock ./
RUN bun install --frozen-lockfile
COPY frontend/ .
RUN bun run build

FROM rust:1-slim-trixie AS chef
WORKDIR /build
RUN cargo install cargo-chef --locked

FROM chef AS planner
COPY backend/Cargo.toml backend/Cargo.lock ./
COPY backend/src ./src
COPY backend/tests/fixtures/fake_mcp ./tests/fixtures/fake_mcp
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS backend
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json
COPY backend/Cargo.toml backend/Cargo.lock ./
COPY backend/src ./src
COPY backend/tests/fixtures/fake_mcp ./tests/fixtures/fake_mcp
COPY backend/migrations ./migrations
COPY backend/.sqlx ./.sqlx
ENV SQLX_OFFLINE=true
RUN cargo build --release --locked --bin mcp-hub

# uv's own build pins the exact Python version, which Debian's package doesn't.
FROM ghcr.io/astral-sh/uv:0.13.7 AS uv

FROM debian:trixie-slim AS python
ARG PYTHON_VERSION
COPY --from=uv /uv /usr/local/bin/uv
RUN uv python install "$PYTHON_VERSION" --install-dir /opt/python

FROM debian:trixie-slim AS runtime
ARG PYTHON_VERSION
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tini \
    && rm -rf /var/lib/apt/lists/*

# The runtimes must sit in /usr/local/bin: the only PATH children get includes it.
COPY --from=oven/bun:1.4.4-slim /usr/local/bin/bun /usr/local/bin/bun
COPY --from=node:24-trixie-slim /usr/local/bin/node /usr/local/bin/node
COPY --from=uv /uv /usr/local/bin/uv
COPY --from=python /opt/python /opt/python
COPY --from=backend /build/target/release/mcp-hub /usr/local/bin/mcp-hub
COPY --from=frontend /build/build /usr/share/mcp-hub/static

# A new named volume takes the ownership of /data/packages from here.
RUN ln -s /opt/python/cpython-"$PYTHON_VERSION"-*/bin/python3 /usr/local/bin/python3 \
    && useradd --system --uid 10001 --create-home --shell /usr/sbin/nologin mcphub \
    && mkdir -p /data/packages /etc/mcp-hub \
    && chown mcphub:mcphub /data/packages

USER mcphub
EXPOSE 8080
# tini reaps orphaned grandchildren of MCP servers.
ENTRYPOINT ["tini", "--", "mcp-hub"]
# The server only starts with `run`; operator commands replace this (`docker compose run ... catalog lock`).
CMD ["run"]
