FROM rust:1.96-slim-bookworm AS chef
WORKDIR /src
RUN cargo install cargo-chef --locked

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN sed -i \
    -e '/crates\/backend/d' \
    -e '/crates\/bridge/d' \
    -e '/crates\/build-config/d' \
    -e '/crates\/frontend/d' \
    -e '/crates\/i18n/d' \
    -e '/crates\/launcher/d' \
    Cargo.toml
RUN cargo chef prepare --recipe-path recipe.json --bin instance-builder

FROM chef AS instance-builder
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json --bin instance-builder
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN sed -i \
    -e '/crates\/backend/d' \
    -e '/crates\/bridge/d' \
    -e '/crates\/build-config/d' \
    -e '/crates\/frontend/d' \
    -e '/crates\/i18n/d' \
    -e '/crates\/launcher/d' \
    Cargo.toml
RUN cargo build --release --bin instance-builder


FROM golang:1.26-bookworm AS backend-builder
WORKDIR /src/backend

COPY backend/go.mod backend/go.sum ./
RUN go mod download

COPY backend/ .
RUN CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -o /out/server ./cmd/server


FROM debian:bookworm-slim AS runtime
ENV SPEC_FILE=/data/internal/spec.json \
    GENERATED_DIR=/data/generated \
    WORKDIR_DIR=/data/workdir \
    UPLOADED_INSTANCES_DIR=/data/internal/uploaded-instances \
    TEMP_DIR=/tmp/potato \
    HOST=0.0.0.0 \
    PORT=8000

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=backend-builder /out/server /usr/local/bin/server
COPY --from=instance-builder /src/target/release/instance-builder /usr/local/bin/instance-builder

RUN mkdir -p /data/internal/uploaded-instances /data/generated /data/workdir

EXPOSE 8000
ENTRYPOINT ["/usr/local/bin/server"]
