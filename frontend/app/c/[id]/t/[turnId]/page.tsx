// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

// For static export: no pages are pre-rendered at this route.
// In production, nginx rewrites all routes to /index.html.
// In development, this route handler serves the main page.
export function generateStaticParams() {
  return [];
}

// Re-export the main page - URL routing is handled client-side
export { default } from '@/app/page';
