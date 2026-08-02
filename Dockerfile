FROM rust:1.88-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --bin gaeb-web

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl poppler-utils tesseract-ocr tesseract-ocr-deu tesseract-ocr-eng \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/gaeb-web /usr/local/bin/gaeb-web
COPY web ./web
RUN useradd --system --uid 10001 --create-home gaeb \
    && mkdir -p /data/jobs \
    && chown -R gaeb:gaeb /data
USER gaeb
ENV BIND=0.0.0.0:8080 \
    DATA_DIR=/data \
    MAX_UPLOAD_BYTES=2097152 \
    PAID_MAX_UPLOAD_BYTES=26214400 \
    RETENTION_HOURS=24 \
    RUST_LOG=info
EXPOSE 8080
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["curl", "--fail", "--silent", "http://127.0.0.1:8080/health"]
ENTRYPOINT ["gaeb-web"]
