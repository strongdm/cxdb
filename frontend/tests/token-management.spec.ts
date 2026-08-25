// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

import { test, expect } from './fixtures';
import type { APITokenMetadata } from '@/types';

test.describe('Personal API token management', () => {
  test('loads metadata, creates with CSRF, reveals once, and revokes with confirmation', async ({ apiPage }) => {
    const csrfToken = 'csrf-test-token';
    const plaintext = 'cxpat_test.secret-value';
    const requests: Array<{ method: string; csrf: string | undefined; body?: string }> = [];
    let token: APITokenMetadata = {
      id: 'cxpat_test',
      prefix: 'cxpat_test',
      name: 'Existing token',
      issuer: 'issuer',
      subject: 'user-1',
      scopes: ['cxdb:read'],
      created_at: '2026-08-20T12:00:00Z',
      expires_at: '2026-09-20T12:00:00Z',
      revoked_at: null,
      last_used_at: null,
    };

    await apiPage.route('**/api/v1/me', async route => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          email: 'user@example.com',
          issuer: 'issuer',
          subject: 'user-1',
          scopes: [],
          csrf_token: csrfToken,
        }),
      });
    });
    await apiPage.route('**/api/v1/tokens**', async route => {
      const request = route.request();
      requests.push({ method: request.method(), csrf: request.headers()['x-csrf-token'], body: request.postData() ?? undefined });
      if (request.method() === 'GET') {
        await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ tokens: [token] }) });
        return;
      }
      if (request.method() === 'POST') {
        expect(request.headers()['x-csrf-token']).toBe(csrfToken);
        token = {
          ...token,
          name: 'Laptop',
          scopes: ['cxdb:read', 'cxdb:write'],
        };
        await route.fulfill({ status: 201, contentType: 'application/json', body: JSON.stringify({ token, plaintext }) });
        return;
      }
      expect(request.method()).toBe('DELETE');
      expect(request.headers()['x-csrf-token']).toBe(csrfToken);
      token = { ...token, revoked_at: '2026-08-25T12:00:00Z' };
      await route.fulfill({ status: 204, body: '' });
    });

    await apiPage.goto('/');
    await apiPage.getByRole('button', { name: 'Manage API tokens' }).click();
    await expect(apiPage.getByRole('dialog')).toContainText('Existing token');
    await expect(apiPage.getByRole('dialog')).toContainText('cxdb:read');

    await apiPage.getByLabel('Name').fill('Laptop');
    await apiPage.getByLabel('Write context data').check();
    await apiPage.getByRole('button', { name: 'Create token' }).click();
    await expect(apiPage.getByRole('dialog')).toContainText('This secret is shown only once');
    await expect(apiPage.getByLabel('New API token secret')).toHaveValue(plaintext);

    const createRequest = requests.find(request => request.method === 'POST');
    expect(createRequest?.body).toContain('cxdb:write');
    expect(createRequest?.csrf).toBe(csrfToken);

    await apiPage.getByRole('button', { name: 'Close API token management' }).click();
    await expect(apiPage.locator('[data-token-management]')).toHaveCount(0);
    await apiPage.getByRole('button', { name: 'Manage API tokens' }).click();
    await expect(apiPage.getByLabel('New API token secret')).toHaveCount(0);

    const article = apiPage.getByRole('article').filter({ hasText: 'Laptop' });
    await article.getByRole('button', { name: 'Revoke' }).click();
    await expect(article).toContainText('Revoke this token?');
    await article.getByRole('button', { name: 'Revoke' }).click();
    await expect(article).toContainText('Revoked');
    expect(requests.some(request => request.method === 'DELETE' && request.csrf === csrfToken)).toBe(true);
  });
});
