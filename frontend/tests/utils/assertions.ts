// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

import { Page, expect, Locator } from '@playwright/test';

/**
 * Assert that the debugger header shows the expected turn count.
 */
export async function expectTurnCount(page: Page, count: number): Promise<void> {
  const header = page.locator('[data-context-debugger]');
  await expect(header).toContainText(`${count} turns`);
}

/**
 * Assert that the timeline has the expected number of items.
 */
export async function expectTimelineItems(page: Page, count: number): Promise<void> {
  const eventList = page.locator('[data-debug-event-list] button');
  await expect(eventList).toHaveCount(count);
}

/**
 * Assert that the selected payload contains a key with an expected value.
 */
export async function expectSelectedPayload(
  page: Page,
  key: string,
  value: string | number
): Promise<void> {
  const payload = page.locator('[data-context-debugger] pre').first();
  const content = await payload.textContent();
  expect(content).toContain(`"${key}"`);
  if (typeof value === 'string') {
    expect(content).toContain(`"${value}"`);
  } else {
    expect(content).toContain(value.toString());
  }
}

/**
 * Assert that the server status indicator shows online.
 */
export async function expectServerOnline(page: Page): Promise<void> {
  await expect(page.getByText('Server online')).toBeVisible();
}

/**
 * Assert that the server status indicator shows offline.
 */
export async function expectServerOffline(page: Page): Promise<void> {
  await expect(page.getByText('Server offline')).toBeVisible();
}

/**
 * Wait for the debugger modal to be visible.
 */
export async function waitForDebugger(page: Page): Promise<Locator> {
  const debugger_ = page.locator('[data-context-debugger]');
  await expect(debugger_).toBeVisible();
  return debugger_;
}

/**
 * Wait for loading to complete in the debugger.
 */
export async function waitForDebuggerLoaded(page: Page): Promise<void> {
  // Wait for the loading spinner to disappear (use first() to handle multiple matches)
  const loadingLocator = page.locator('[data-context-debugger]').getByText('Loading...').first();
  await expect(loadingLocator).not.toBeVisible({ timeout: 10000 });
}

/**
 * Get the timeline event items.
 */
export function getTimelineItems(page: Page): Locator {
  return page.locator('[data-debug-event-list] button');
}

/**
 * Select a timeline item by index.
 */
export async function selectTimelineItem(page: Page, index: number): Promise<void> {
  const items = getTimelineItems(page);
  await items.nth(index).click();
}

/**
 * Get the search input in the debugger.
 */
export function getSearchInput(page: Page): Locator {
  return page.locator('[data-debug-search]');
}

/**
 * Assert that the debugger shows an error message.
 */
export async function expectDebuggerError(page: Page, errorText?: string): Promise<void> {
  const errorLocator = page.locator('[data-context-debugger]').locator('.text-red-400');
  await expect(errorLocator).toBeVisible();
  if (errorText) {
    await expect(errorLocator).toContainText(errorText);
  }
}

/**
 * Open a context by clicking on it in the context list.
 * This first refreshes the page to load the latest contexts.
 */
export async function addContext(page: Page, contextId: string | number): Promise<void> {
  // Reload the page to ensure the context list is fresh.
  // Use 'domcontentloaded' instead of 'networkidle' because SSE connections
  // (EventSource to /v1/events) keep the network active, preventing networkidle.
  await page.reload({ waitUntil: 'domcontentloaded' });

  // Wait for the context to appear in the list.
  // This polls until the element is visible, handling async React state updates.
  const contextItem = page.locator(`[data-context-id="${contextId}"]`);
  await expect(contextItem).toBeVisible({ timeout: 30000 });

  // Click on the context to open it
  await contextItem.click();
}

/**
 * Get the "Copy all" button in the debugger.
 */
export function getCopyAllButton(page: Page): Locator {
  return page.locator('[data-context-debugger] [data-copy-all]');
}

/**
 * Get the "Copy" button for the selected event.
 */
export function getCopyEventButton(page: Page): Locator {
  return page.locator('[data-context-debugger]').getByRole('button', { name: 'Copy', exact: true }).first();
}

/**
 * Check if the "Copied!" text is visible.
 */
export async function expectCopied(page: Page): Promise<void> {
  await expect(page.locator('[data-context-debugger]').getByText('Copied!')).toBeVisible();
}

/**
 * Expand the Raw Payload collapsible section if it's collapsed.
 */
export async function expandRawPayload(page: Page): Promise<void> {
  const rawPayloadSection = page.locator('[data-raw-payload-section]');
  // Check if already expanded by looking for the pre element
  const pre = rawPayloadSection.locator('pre');
  if (await pre.count() === 0) {
    // Not expanded, click to expand
    await rawPayloadSection.locator('button').click();
  }
}

/**
 * Get the raw payload content from the expanded section.
 */
export async function getRawPayload(page: Page): Promise<Locator> {
  await expandRawPayload(page);
  return page.locator('[data-raw-payload]');
}

/**
 * Expand the Turn Metadata collapsible section.
 */
export async function expandTurnMetadata(page: Page): Promise<void> {
  // Click the Turn Metadata button to expand it
  const metadataButton = page.locator('[data-context-debugger]').getByRole('button', { name: /Turn Metadata/i });
  await metadataButton.click();
}
