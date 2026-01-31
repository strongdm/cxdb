# Multi-stage Dockerfile for cxdb
# Stage 1: Build Next.js frontend
# Stage 2: Build Rust binary
# Stage 3: Runtime with nginx + supervisord

# ============================================
# Stage 1: Build frontend
# ============================================
FROM node:20-alpine AS frontend

WORKDIR /app

# Install pnpm
RUN corepack enable && corepack prepare pnpm@latest --activate

# Copy package files
COPY frontend/package.json frontend/pnpm-lock.yaml* ./

# Install dependencies
RUN pnpm install --frozen-lockfile || pnpm install

# Copy source
COPY frontend/ ./

# Build static export
RUN pnpm build

# ============================================
# Stage 2: Build Rust binary
# ============================================
FROM rust:1.92-bookworm AS backend

WORKDIR /app

# Copy Cargo files first for dependency caching
COPY Cargo.toml Cargo.lock* ./
COPY server/Cargo.toml ./server/
COPY clients/rust/Cargo.toml ./clients/rust/

# Create dummy sources to build dependencies
RUN mkdir -p server/src clients/rust/src && \
    echo "fn main() {}" > server/src/main.rs && \
    echo "pub fn dummy() {}" > clients/rust/src/lib.rs && \
    cargo build --release --manifest-path server/Cargo.toml && \
    rm -rf server/src clients/rust/src

# Copy actual source and build
COPY server/ ./server/
COPY clients/ ./clients/
RUN touch server/src/main.rs && \
    cargo build --release --manifest-path server/Cargo.toml

# ============================================
# Stage 3: Runtime
# ============================================
FROM debian:bookworm-slim

# Install nginx and supervisor
RUN apt-get update && apt-get install -y --no-install-recommends \
    nginx \
    supervisor \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create directories
RUN mkdir -p /app /data /var/log/supervisor

# Copy binaries and static files
COPY --from=backend /app/target/release/cxdb-server /app/cxdb
COPY --from=frontend /app/out /usr/share/nginx/html

# Copy nginx config
COPY deploy/nginx.conf /etc/nginx/nginx.conf

# Copy supervisor config
COPY deploy/supervisord.conf /etc/supervisor/conf.d/supervisord.conf

# Environment
ENV CXDB_DATA_DIR=/data
ENV CXDB_BIND=0.0.0.0:9009
ENV CXDB_HTTP_BIND=127.0.0.1:9010

# Expose nginx port
EXPOSE 80

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD curl -f http://localhost/v1/contexts?limit=1 || exit 1

# Run supervisor
CMD ["/usr/bin/supervisord", "-c", "/etc/supervisor/conf.d/supervisord.conf"]
