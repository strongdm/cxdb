# syntax=docker/dockerfile:1.6
# NOTE: Build from repo root with: docker build --platform linux/amd64 -f gateway/Dockerfile .
# IMPORTANT: Target platform is linux/amd64 (EKS cluster). Always use --platform linux/amd64!

# ============================================
# Stage 1: Build frontend static assets
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
# Stage 2: Build Go binary with CGO for SQLite
# ============================================
FROM --platform=linux/amd64 golang:1.23-bookworm AS builder
WORKDIR /app

# Copy go mod files first for layer caching
COPY gateway/go.mod gateway/go.sum ./
RUN go mod download

# Copy source code
COPY gateway/ ./

# Copy frontend build output for embedding
COPY --from=frontend /app/.next-build/. ./pkg/proxy/web/

# Build with CGO enabled for SQLite
RUN --mount=type=cache,target=/root/.cache/go-build \
    CGO_ENABLED=1 go build -o /out/gateway ./cmd/server

# ============================================
# Stage 3: Minimal runtime image
# ============================================
FROM --platform=linux/amd64 debian:bookworm-slim

# Install curl for healthchecks and ca-certificates for HTTPS
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user and data directory
RUN useradd -r -u 65532 -g nogroup nonroot && \
    mkdir -p /data /app && \
    chown -R 65532:nogroup /data /app

USER nonroot

WORKDIR /app

ENV PORT=8080
ENV DATABASE_PATH=/data/sessions.db

# Copy binary (frontend is embedded inside)
COPY --from=builder --chown=65532:65532 /out/gateway /app/gateway

EXPOSE 8080
ENTRYPOINT ["/app/gateway"]
