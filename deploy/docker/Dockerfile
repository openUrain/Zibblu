FROM rust:1.100.0-slim-bookworm@sha256:7c7d4eb4d05b7781187a327809466cfddf29753f6defdc3d19a0e291951b7a37 AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY apps/desktop/src-tauri/ apps/desktop/src-tauri/
COPY crates/core/ crates/core/
COPY services/online/ services/online/
RUN cargo build --locked --release -p zibblu-online

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /src/target/release/zibblu-online /usr/local/bin/zibblu-online
USER 65532:65532
EXPOSE 7841
ENTRYPOINT ["/usr/local/bin/zibblu-online"]
