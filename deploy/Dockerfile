# Build context: the repository root.
#   docker build -f deploy/Dockerfile -t revit-downgrade .

FROM node:24-bookworm-slim AS web
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM rust:1.99-bookworm AS rust
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY server ./server
COPY engine-stub ./engine-stub
RUN cargo build --release --locked -p revit-downgrade-server -p engine-stub

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /app --shell /usr/sbin/nologin revit-downgrade \
    && mkdir -p /app /data \
    && chown revit-downgrade:revit-downgrade /data

COPY --from=rust /src/target/release/revit-downgrade-server /app/revit-downgrade-server
# The stub only copies files. Mount the real engine over /app/engine to convert for real.
COPY --from=rust /src/target/release/engine-stub /app/engine
COPY --from=web /src/web/dist /app/web

ENV REVIT_DOWNGRADE_ENGINE=/app/engine \
    REVIT_DOWNGRADE_DATA=/data \
    REVIT_DOWNGRADE_WEB_DIR=/app/web \
    REVIT_DOWNGRADE_LISTEN=0.0.0.0:8080 \
    REVIT_DOWNGRADE_TRUST_PROXY=false

USER revit-downgrade
WORKDIR /app
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["/app/revit-downgrade-server"]
