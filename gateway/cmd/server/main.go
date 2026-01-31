// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

package main

import (
	"context"
	"io/fs"
	"log/slog"
	"os"
	"os/signal"
	"strings"
	"syscall"

	"github.com/strongdm/cxdb/gateway/internal/config"
	"github.com/strongdm/cxdb/gateway/pkg/auth"
	"github.com/strongdm/cxdb/gateway/pkg/proxy"
)

// Entry point for the cxdb Gateway server.
// This gateway provides Google OAuth authentication for reads while
// forwarding writes directly to the cxdb backend.
func main() {
	logger := slog.New(slog.NewJSONHandler(os.Stdout, &slog.HandlerOptions{
		Level:     slog.LevelInfo,
		AddSource: true,
	}))

	cfg, err := config.Load()
	if err != nil {
		logger.Error("config load failed", "err", err)
		os.Exit(1)
	}

	cookieSecure := strings.HasPrefix(cfg.PublicBaseURL, "https://")

	sessionStore, err := auth.NewSessionStore(
		cfg.DatabasePath,
		cfg.CookieName,
		cfg.SessionTTL,
		cfg.CookieDomain,
		cookieSecure,
		cfg.SessionSecret,
	)
	if err != nil {
		logger.Error("session store init failed", "err", err)
		os.Exit(1)
	}
	defer func() { _ = sessionStore.Close() }()

	googleAuth := auth.NewGoogleAuth(
		cfg.PublicBaseURL,
		cfg.GoogleClientID,
		cfg.GoogleClientSecret,
		cfg.GoogleAllowedDomain,
		cfg.PublicAllowedHosts,
		sessionStore,
	)

	reverseProxy, err := proxy.NewReverseProxy(cfg.CXDBBackendURL, logger)
	if err != nil {
		logger.Error("reverse proxy init failed", "err", err)
		os.Exit(1)
	}

	// Extract embedded static assets for the React frontend
	staticAssets, err := fs.Sub(proxy.EmbeddedStatic, "web")
	if err != nil {
		logger.Error("embed static assets failed", "err", err)
		os.Exit(1)
	}

	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()

	server, err := proxy.New(cfg, sessionStore, googleAuth, reverseProxy, staticAssets, logger)
	if err != nil {
		logger.Error("server init failed", "err", err)
		os.Exit(1)
	}

	logger.Info("cxdb gateway starting",
		"port", cfg.Port,
		"backend", cfg.CXDBBackendURL,
		"dev_mode", cfg.DevMode,
	)

	if err := server.ListenAndServe(ctx); err != nil {
		logger.Error("server exited", "err", err)
		os.Exit(1)
	}
}
