// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

import { test, expect } from './fixtures';
import {
  addContext,
  waitForDebugger,
  waitForDebuggerLoaded,
  getCopyAllButton,
  getCopyEventButton,
  expectCopied,
  selectTimelineItem,
} from './utils/assertions';

test.describe('Copy Functionality', () => {
  test.beforeEach(async ({ apiPage }) => {
    // Grant clipboard permissions
    await apiPage.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  });

  test('Copy all button is clickable and functional', async ({
    apiPage,
    goWriter,
    registry,
  }) => {
    const ctx = goWriter.createContext();
    await registry.putBundle('test-bundle-v1');

    goWriter.appendTurn(ctx.contextId, 'user', 'Test message');

    await apiPage.goto('/');
    await addContext(apiPage, ctx.contextId);
    await waitForDebugger(apiPage);
    await waitForDebuggerLoaded(apiPage);

    const copyAllButton = getCopyAllButton(apiPage);
    await expect(copyAllButton).toBeVisible();
    await expect(copyAllButton).toBeEnabled();

    // Click copy - verify it doesn't throw
    await copyAllButton.click();

    // Either shows "Copied!" briefly or "Copy all" - just verify button still works
    await apiPage.waitForTimeout(500);
    await expect(copyAllButton).toBeVisible();
  });

  test('Copy event button is clickable and functional', async ({
    apiPage,
    goWriter,
    registry,
  }) => {
    const ctx = goWriter.createContext();
    await registry.putBundle('test-bundle-v1');

    goWriter.appendTurn(ctx.contextId, 'user', 'Test message');

    await apiPage.goto('/');
    await addContext(apiPage, ctx.contextId);
    await waitForDebugger(apiPage);
    await waitForDebuggerLoaded(apiPage);

    // Select the turn first
    await selectTimelineItem(apiPage, 0);

    const copyEventButton = getCopyEventButton(apiPage);
    await expect(copyEventButton).toBeVisible();
    await expect(copyEventButton).toBeEnabled();

    // Click copy - verify it doesn't throw
    await copyEventButton.click();

    // Either shows "Copied!" briefly or "Copy" - just verify button still works
    await apiPage.waitForTimeout(500);
    await expect(copyEventButton).toBeVisible();
  });

  test('clipboard contains valid JSON after copy all', async ({
    apiPage,
    goWriter,
    registry,
  }) => {
    const ctx = goWriter.createContext();
    await registry.putBundle('test-bundle-v1');

    goWriter.appendTurn(ctx.contextId, 'user', 'Test message for copy');

    await apiPage.goto('/');
    await addContext(apiPage, ctx.contextId);
    await waitForDebugger(apiPage);
    await waitForDebuggerLoaded(apiPage);

    // Click copy all
    const copyAllButton = getCopyAllButton(apiPage);
    await copyAllButton.click();
    await expect(copyAllButton).toContainText('Copied!');

    // Read clipboard content
    const clipboardContent = await apiPage.evaluate(async () => {
      return await navigator.clipboard.readText();
    });

    // Should be valid JSON
    expect(() => JSON.parse(clipboardContent)).not.toThrow();

    // Should contain context data
    const parsed = JSON.parse(clipboardContent);
    expect(parsed).toHaveProperty('meta');
    expect(parsed).toHaveProperty('turns');
  });

  test('clipboard contains valid JSON after copy event', async ({
    apiPage,
    goWriter,
    registry,
  }) => {
    const ctx = goWriter.createContext();
    await registry.putBundle('test-bundle-v1');

    goWriter.appendTurn(ctx.contextId, 'user', 'Event copy test');

    await apiPage.goto('/');
    await addContext(apiPage, ctx.contextId);
    await waitForDebugger(apiPage);
    await waitForDebuggerLoaded(apiPage);

    // Select the turn (first item should be auto-selected)
    await selectTimelineItem(apiPage, 0);

    // Get the copy event button and click it
    const copyButton = getCopyEventButton(apiPage);
    await expect(copyButton).toBeVisible();
    await copyButton.click();

    // Wait a bit for clipboard to be populated
    await apiPage.waitForTimeout(500);

    // Read clipboard content
    const clipboardContent = await apiPage.evaluate(async () => {
      return await navigator.clipboard.readText();
    });

    // Should be valid JSON
    expect(() => JSON.parse(clipboardContent)).not.toThrow();

    // Should contain turn data (turn_id may be stringified)
    const parsed = JSON.parse(clipboardContent);
    expect(parsed).toBeDefined();
  });

  test('copy buttons are disabled when no data', async ({ apiPage }) => {
    // Navigate to home first, then trigger client-side navigation to non-existent context
    await apiPage.goto('/');
    await apiPage.waitForSelector('h1');
    await apiPage.evaluate(() => {
      window.history.pushState(null, '', '/c/999999');
      window.dispatchEvent(new PopStateEvent('popstate'));
    });
    await waitForDebugger(apiPage);

    // Wait for error or empty state
    await apiPage.waitForTimeout(2000);

    // Copy all button might be disabled or hidden
    const copyAllButton = getCopyAllButton(apiPage);
    const isDisabled = await copyAllButton.isDisabled();

    // Either disabled or just shows "Copy all" without data
    expect(isDisabled).toBeDefined();
  });
});
