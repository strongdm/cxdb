// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

import { test, expect } from './fixtures';

async function createTaggedContext(
  serverHttpUrl: string,
  tag: string,
  title: string
): Promise<string> {
  const createResponse = await fetch(`${serverHttpUrl}/v1/contexts`, {
    method: 'POST',
  });
  expect(createResponse.ok).toBeTruthy();

  const createPayload = await createResponse.json() as { context_id: number };
  const contextId = String(createPayload.context_id);

  const appendResponse = await fetch(`${serverHttpUrl}/v1/contexts/${contextId}/turns`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({
      type_id: 'test.context-metadata',
      type_version: 1,
      data: {
        '30': {
          '1': tag,
          '2': title,
        },
      },
    }),
  });
  expect(appendResponse.ok).toBeTruthy();

  return contextId;
}

test.describe('Tag Filter', () => {
  test('clicking a context tag chip appends a CQL tag clause and filters the context list', async ({
    apiPage,
    serverHttpUrl,
  }) => {
    const claudeOneId = await createTaggedContext(serverHttpUrl, 'claude', 'Claude One');
    const dotrunnerId = await createTaggedContext(serverHttpUrl, 'dotrunner', 'Dotrunner One');
    const claudeTwoId = await createTaggedContext(serverHttpUrl, 'claude', 'Claude Two');

    await apiPage.goto('/');

    await expect(apiPage.locator(`[data-context-id="${claudeOneId}"]`)).toBeVisible();
    await expect(apiPage.locator(`[data-context-id="${dotrunnerId}"]`)).toBeVisible();
    await expect(apiPage.locator(`[data-context-id="${claudeTwoId}"]`)).toBeVisible();

    const searchInput = apiPage.locator('input[placeholder*="Search"]');
    await apiPage.locator('[data-context-tag-filter="claude"]').first().click();

    await expect(searchInput).toHaveValue('tag = "claude"');
    await expect(apiPage.getByRole('button', { name: 'All tags' })).toBeVisible();
    await expect(apiPage.locator('[data-context-id]')).toHaveCount(2);
    await expect(apiPage.locator(`[data-context-id="${claudeOneId}"]`)).toBeVisible();
    await expect(apiPage.locator(`[data-context-id="${claudeTwoId}"]`)).toBeVisible();
    await expect(apiPage.locator(`[data-context-id="${dotrunnerId}"]`)).toHaveCount(0);
  });

  test('clicking a context tag chip appends the tag clause to an existing search query', async ({
    apiPage,
    serverHttpUrl,
  }) => {
    const claudeId = await createTaggedContext(serverHttpUrl, 'claude', 'Claude Search Compose');
    const dotrunnerId = await createTaggedContext(serverHttpUrl, 'dotrunner', 'Dotrunner Search Compose');

    await apiPage.goto('/');

    const searchInput = apiPage.locator('input[placeholder*="Search"]');
    await searchInput.fill('title ^~= "Claude"');
    await apiPage.locator('[data-context-tag-filter="claude"]').first().click();

    await expect(searchInput).toHaveValue('title ^~= "Claude" AND tag = "claude"');
    await expect(apiPage.locator(`[data-context-id="${claudeId}"]`)).toBeVisible();
    await expect(apiPage.locator(`[data-context-id="${dotrunnerId}"]`)).toHaveCount(0);
  });
});
