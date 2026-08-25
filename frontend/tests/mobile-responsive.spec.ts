// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

import type { Page } from '@playwright/test';
import { test, expect } from './fixtures';
import {
  addContext,
  getRawPayload,
  waitForDebugger,
  waitForDebuggerLoaded,
} from './utils/assertions';

const phoneViewport = { width: 390, height: 844 };

async function expectNoHorizontalOverflow(page: Page) {
  const dimensions = await page.evaluate(() => ({
    viewport: window.innerWidth,
    document: document.documentElement.scrollWidth,
    body: document.body.scrollWidth,
  }));
  expect(dimensions.document).toBeLessThanOrEqual(dimensions.viewport + 1);
  expect(dimensions.body).toBeLessThanOrEqual(dimensions.viewport + 1);
}

test.describe('Mobile responsive layout', () => {
  test.beforeEach(async ({ apiPage }) => {
    await apiPage.setViewportSize(phoneViewport);
  });

  test('dashboard controls and token management fit a phone viewport', async ({ apiPage }) => {
    await apiPage.route('**/api/v1/me', route => route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({
        email: 'mobile@example.com', issuer: 'test', subject: 'mobile',
        scopes: ['cxdb:read', 'cxdb:write'], csrf_token: 'csrf-mobile',
      }),
    }));
    await apiPage.route('**/api/v1/tokens**', route => route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ tokens: [] }),
    }));

    await apiPage.goto('/');
    await expect(apiPage.locator('header')).toBeVisible();
    await expect(apiPage.locator('aside')).toBeVisible();
    await expect(apiPage.locator('main')).toBeVisible();
    await expectNoHorizontalOverflow(apiPage);

    const asideBox = await apiPage.locator('aside').boundingBox();
    const mainBox = await apiPage.locator('main').boundingBox();
    expect(asideBox).not.toBeNull();
    expect(mainBox).not.toBeNull();
    expect(mainBox!.y).toBeGreaterThanOrEqual(asideBox!.y + asideBox!.height - 1);

    await apiPage.getByRole('button', { name: /Theme/ }).click();
    const themeMenuBox = await apiPage.getByRole('listbox').boundingBox();
    expect(themeMenuBox).not.toBeNull();
    expect(themeMenuBox!.x).toBeGreaterThanOrEqual(0);
    expect(themeMenuBox!.x + themeMenuBox!.width).toBeLessThanOrEqual(phoneViewport.width);

    // The menu must be hit-testable on a touch viewport. A parent with
    // overflow-x-auto also clips overflow in the block direction, so the
    // menu can exist in the DOM while taps reach the search controls below.
    const firstAlternative = apiPage.getByRole('option').nth(1);
    const optionBox = await firstAlternative.boundingBox();
    expect(optionBox).not.toBeNull();
    const optionReceivesPointer = await apiPage.evaluate(({ x, y }) => (
      document.elementFromPoint(x, y)?.closest('[role="option"]') !== null
    ), {
      x: optionBox!.x + optionBox!.width / 2,
      y: optionBox!.y + optionBox!.height / 2,
    });
    expect(optionReceivesPointer).toBe(true);
    await firstAlternative.click();
    await expect(apiPage.getByRole('button', { name: 'Theme: Trope' })).toBeVisible();

    // One control starts and stops the demo; there is no separate mode toggle.
    await apiPage.getByRole('button', { name: 'Start demo' }).click();
    await expect(apiPage.getByRole('button', { name: 'Stop demo' })).toBeVisible();
    await expect(apiPage.getByRole('status', { name: 'Server online' })).toBeVisible();
    await expect(apiPage.getByRole('status')).toHaveCount(1);
    const serverStatusBox = await apiPage.getByRole('status').boundingBox();
    expect(serverStatusBox).not.toBeNull();
    expect(serverStatusBox!.width).toBeLessThan(30);
    await apiPage.getByRole('button', { name: 'Stop demo' }).click();
    await expect(apiPage.getByRole('button', { name: 'Start demo' })).toBeVisible();

    await apiPage.getByRole('button', { name: 'Manage API tokens' }).click();
    const tokenPanel = apiPage.locator('[data-token-management] > div');
    await expect(tokenPanel).toBeVisible();
    const panelBox = await tokenPanel.boundingBox();
    expect(panelBox).not.toBeNull();
    expect(panelBox!.x).toBeGreaterThanOrEqual(0);
    expect(panelBox!.width).toBeLessThanOrEqual(phoneViewport.width);
    expect(panelBox!.height).toBeLessThanOrEqual(phoneViewport.height);
    await expect(apiPage.getByRole('button', { name: 'Create token' })).toBeVisible();
    await expectNoHorizontalOverflow(apiPage);

    const nameFontSize = await apiPage.getByLabel('Name').evaluate(element => (
      window.getComputedStyle(element).fontSize
    ));
    expect(nameFontSize).toBe('16px');
  });

  test('context debugger stacks the timeline above turn details', async ({
    apiPage,
    goWriter,
    registry,
  }) => {
    const context = goWriter.createContext();
    await registry.putBundle('mobile-layout-v1');
    goWriter.appendTurn(context.contextId, 'user', 'mobile detail content', {
      typeId: 'com.yourorg.ai.MessageTurn',
      typeVersion: 1,
    });

    await apiPage.goto('/');
    await addContext(apiPage, context.contextId);
    await waitForDebugger(apiPage);
    await waitForDebuggerLoaded(apiPage);
    await expectNoHorizontalOverflow(apiPage);

    const timeline = apiPage.locator('[data-debug-event-list]');
    const detailTab = apiPage.getByRole('button', { name: 'Turn', exact: true });
    const timelineBox = await timeline.boundingBox();
    const detailBox = await detailTab.boundingBox();
    expect(timelineBox).not.toBeNull();
    expect(detailBox).not.toBeNull();
    expect(detailBox!.y).toBeGreaterThan(timelineBox!.y);
    expect(timelineBox!.width).toBeLessThanOrEqual(phoneViewport.width);
    await expect(apiPage.getByRole('button', { name: 'Close context debugger' })).toBeVisible();
    await expect(await getRawPayload(apiPage)).toContainText('mobile detail content');

    await apiPage.setViewportSize({ width: 844, height: 390 });
    await expectNoHorizontalOverflow(apiPage);
    await expect(timeline).toBeVisible();
    await expect(detailTab).toBeVisible();
  });
});
