// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

import { test, expect } from './fixtures';

const PROVIDER_CASES = [
  { tag: 'claude', titlePrefix: 'Claude' },
  { tag: 'codex', titlePrefix: 'Codex' },
] as const;

async function createTaggedContext(
  serverHttpUrl: string,
  tag: string,
  title: string,
  labels: string[] = []
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
          '3': labels,
        },
      },
    }),
  });
  expect(appendResponse.ok).toBeTruthy();

  return contextId;
}

test.describe('Tag Filter', () => {
  for (const { tag, titlePrefix } of PROVIDER_CASES) {
    test(`clicking a ${tag} context tag chip appends a CQL tag clause and filters the context list`, async ({
      apiPage,
      serverHttpUrl,
    }) => {
      const providerOneId = await createTaggedContext(serverHttpUrl, tag, `${titlePrefix} One`);
      const dotrunnerId = await createTaggedContext(serverHttpUrl, 'dotrunner', 'Dotrunner One');
      const providerTwoId = await createTaggedContext(serverHttpUrl, tag, `${titlePrefix} Two`);

      await apiPage.goto('/');

      await expect(apiPage.locator(`[data-context-id="${providerOneId}"]`)).toBeVisible();
      await expect(apiPage.locator(`[data-context-id="${dotrunnerId}"]`)).toBeVisible();
      await expect(apiPage.locator(`[data-context-id="${providerTwoId}"]`)).toBeVisible();

      const searchInput = apiPage.locator('input[placeholder*="Search"]');
      await apiPage.locator(`[data-context-tag-filter="${tag}"]`).first().click();

      await expect(searchInput).toHaveValue(`tag = "${tag}"`);
      await expect(apiPage.getByRole('button', { name: 'All tags' })).toBeVisible();
      await expect(apiPage.locator('[data-context-id]')).toHaveCount(2);
      await expect(apiPage.locator(`[data-context-id="${providerOneId}"]`)).toBeVisible();
      await expect(apiPage.locator(`[data-context-id="${providerTwoId}"]`)).toBeVisible();
      await expect(apiPage.locator(`[data-context-id="${dotrunnerId}"]`)).toHaveCount(0);
    });

    test(`clicking a ${tag} context tag chip appends the tag clause to an existing search query`, async ({
      apiPage,
      serverHttpUrl,
    }) => {
      const providerId = await createTaggedContext(serverHttpUrl, tag, `${titlePrefix} Search Compose`);
      const dotrunnerId = await createTaggedContext(serverHttpUrl, 'dotrunner', 'Dotrunner Search Compose');

      await apiPage.goto('/');

      const searchInput = apiPage.locator('input[placeholder*="Search"]');
      await searchInput.fill(`title ^~= "${titlePrefix}"`);
      await apiPage.locator(`[data-context-tag-filter="${tag}"]`).first().click();

      await expect(searchInput).toHaveValue(`title ^~= "${titlePrefix}" AND tag = "${tag}"`);
      await expect(apiPage.locator(`[data-context-id="${providerId}"]`)).toBeVisible();
      await expect(apiPage.locator(`[data-context-id="${dotrunnerId}"]`)).toHaveCount(0);
    });

    test(`clicking a label chip appends a CQL label clause and can compose with ${tag} tag clicks`, async ({
      apiPage,
      serverHttpUrl,
    }) => {
      const matchingId = await createTaggedContext(
        serverHttpUrl,
        tag,
        `${titlePrefix} Interactive`,
        ['interactive', 'shared']
      );
      const wrongLabelId = await createTaggedContext(
        serverHttpUrl,
        tag,
        `${titlePrefix} Batch`,
        ['batch']
      );
      const wrongTagId = await createTaggedContext(
        serverHttpUrl,
        'dotrunner',
        'Dotrunner Interactive',
        ['interactive']
      );

      await apiPage.goto('/');

      const searchInput = apiPage.locator('input[placeholder*="Search"]');
      await apiPage.locator('[data-context-label-filter="interactive"]').first().click();

      await expect(searchInput).toHaveValue('label = "interactive"');
      await expect(apiPage.locator(`[data-context-id="${matchingId}"]`)).toBeVisible();
      await expect(apiPage.locator(`[data-context-id="${wrongTagId}"]`)).toBeVisible();
      await expect(apiPage.locator(`[data-context-id="${wrongLabelId}"]`)).toHaveCount(0);

      await apiPage.locator(`[data-context-tag-filter="${tag}"]`).first().click();

      await expect(searchInput).toHaveValue(`label = "interactive" AND tag = "${tag}"`);
      await expect(apiPage.locator(`[data-context-id="${matchingId}"]`)).toBeVisible();
      await expect(apiPage.locator(`[data-context-id="${wrongLabelId}"]`)).toHaveCount(0);
      await expect(apiPage.locator(`[data-context-id="${wrongTagId}"]`)).toHaveCount(0);
    });
  }
});
