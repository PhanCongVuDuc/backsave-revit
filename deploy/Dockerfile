# Build context: the repository root.
#   docker build -f deploy/Dockerfile -t backsave .

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
RUN cargo build --release --locked -p backsave-server -p engine-stub

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /app --shell /usr/sbin/nologin backsave \
    && mkdir -p /app /data \
    && chown backsave:backsave /data

COPY --from=rust /src/target/release/backsave-server /app/backsave-server
# The stub only copies files. Mount the real engine over /app/engine to convert for real.
COPY --from=rust /src/target/release/engine-stub /app/engine
COPY --from=web /src/web/dist /app/web

ENV BACKSAVE_ENGINE=/app/engine \
    BACKSAVE_DATA=/data \
    BACKSAVE_WEB_DIR=/app/web \
    BACKSAVE_LISTEN=0.0.0.0:8080 \
    BACKSAVE_TRUST_PROXY=true

USER backsave
WORKDIR /app
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["/app/backsave-server"]
