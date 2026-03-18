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
  test('clicking a context tag chip filters the context list to that tag', async ({
    apiPage,
    serverHttpUrl,
  }) => {
    const claudeOneId = await createTaggedContext(serverHttpUrl, 'claude-code', 'Claude One');
    const dotrunnerId = await createTaggedContext(serverHttpUrl, 'dotrunner', 'Dotrunner One');
    const claudeTwoId = await createTaggedContext(serverHttpUrl, 'claude-code', 'Claude Two');

    await apiPage.goto('/');

    await expect(apiPage.locator(`[data-context-id="${claudeOneId}"]`)).toBeVisible();
    await expect(apiPage.locator(`[data-context-id="${dotrunnerId}"]`)).toBeVisible();
    await expect(apiPage.locator(`[data-context-id="${claudeTwoId}"]`)).toBeVisible();

    await apiPage.locator('[data-context-tag-filter="claude-code"]').first().click();

    await expect(apiPage.locator('[data-context-id]')).toHaveCount(2);
    await expect(apiPage.locator(`[data-context-id="${claudeOneId}"]`)).toBeVisible();
    await expect(apiPage.locator(`[data-context-id="${claudeTwoId}"]`)).toBeVisible();
    await expect(apiPage.locator(`[data-context-id="${dotrunnerId}"]`)).toHaveCount(0);
    await expect(apiPage.getByRole('button', { name: 'claude-code', exact: true })).toBeVisible();
  });
});
