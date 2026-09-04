# syntax=docker/dockerfile:1.7

FROM rust:1.98-bookworm@sha256:82150a52ec202c1b14d7817e14516c392bb7f5cfebd88f1ed531cb37ebd39922 AS builder
WORKDIR /src

COPY .cargo .cargo
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
COPY migrations migrations
COPY docs/openapi.yaml docs/openapi.yaml
COPY docs/catalog/resources.json docs/catalog/resources.json

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo install cargo-auditable --version 0.7.5 --locked && \
    cargo auditable build --release --locked \
      --package x402-near-facilitator --bins && \
    mkdir -p /out && \
    install -m 0755 target/release/x402-near-facilitator /out/x402-near-facilitator && \
    install -m 0755 target/release/x402-near-admin /out/x402-near-admin

FROM debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818 AS runtime

ARG VERSION=0.0.0
ARG VCS_REF=unknown
LABEL org.opencontainers.image.title="FastNEAR x402 facilitator for NEAR and Base" \
      org.opencontainers.image.description="Rust x402 v2 exact Circle USDC facilitator for NEAR and Base" \
      org.opencontainers.image.source="https://github.com/fastnear/x402-facilitator" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.licenses="Apache-2.0"

RUN addgroup --system facilitator && \
    adduser --system --ingroup facilitator --no-create-home facilitator

COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /out/x402-near-facilitator /usr/local/bin/
COPY --from=builder /out/x402-near-admin /usr/local/bin/
COPY migrations /usr/share/x402-near-facilitator/migrations
COPY LICENSE NOTICE /usr/share/doc/x402-near-facilitator/

USER facilitator:facilitator
WORKDIR /var/empty
ENTRYPOINT ["/usr/local/bin/x402-near-facilitator"]
CMD ["--config", "/etc/x402-near-facilitator/config.json"]
