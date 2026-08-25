// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

import { test, expect } from './fixtures';
import {
  addContext,
  waitForDebugger,
  waitForDebuggerLoaded,
  selectTimelineItem,
  expandTurnMetadata,
  getRawPayload,
} from './utils/assertions';

test.describe('Turn DAG Structure', () => {
  test('sequential turns have correct parent linkage', async ({
    apiPage,
    goWriter,
    registry,
    serverHttpUrl,
  }) => {
    const ctx = goWriter.createContext();
    await registry.putBundle('test-bundle-v1');

    // Append 3 sequential turns
    const turn1 = goWriter.appendTurn(ctx.contextId, 'user', 'First message');
    const turn2 = goWriter.appendTurn(ctx.contextId, 'assistant', 'Response');
    const turn3 = goWriter.appendTurn(ctx.contextId, 'user', 'Follow-up');

    // Verify parent linkage via HTTP API
    const response = await fetch(
      `${serverHttpUrl}/v1/contexts/${ctx.contextId}/turns?view=typed&limit=10`
    );
    const data = await response.json();

    // Sort turns by turn_id
    const turns = data.turns.sort(
      (a: { turn_id: string }, b: { turn_id: string }) =>
        parseInt(a.turn_id) - parseInt(b.turn_id)
    );

    // Turn 1: parent = 0 (root)
    expect(parseInt(turns[0].parent_turn_id)).toBe(0);

    // Turn 2: parent = turn 1
    expect(parseInt(turns[1].parent_turn_id)).toBe(turn1.turnId);

    // Turn 3: parent = turn 2
    expect(parseInt(turns[2].parent_turn_id)).toBe(turn2.turnId);

    // Also verify the UI displays the metadata
    await apiPage.goto('/');
    await addContext(apiPage, ctx.contextId);
    await waitForDebugger(apiPage);
    await waitForDebuggerLoaded(apiPage);

    await selectTimelineItem(apiPage, 0);

    // Expand Turn Metadata section to see Parent and Depth
    await expandTurnMetadata(apiPage);

    await expect(
      apiPage.locator('[data-context-debugger]').getByText('Parent', { exact: true })
    ).toBeVisible();
    await expect(
      apiPage.locator('[data-context-debugger]').getByText('Depth', { exact: true })
    ).toBeVisible();
  });

  test('turns have incrementing depths', async ({ goWriter, registry }) => {
    const ctx = goWriter.createContext();
    await registry.putBundle('test-bundle-v1');

    // Append 3 sequential turns
    const turn1 = goWriter.appendTurn(ctx.contextId, 'user', 'Message 1');
    const turn2 = goWriter.appendTurn(ctx.contextId, 'user', 'Message 2');
    const turn3 = goWriter.appendTurn(ctx.contextId, 'user', 'Message 3');

    // Verify depths increase sequentially (relative to first turn)
    const baseDepth = turn1.depth;
    expect(turn2.depth).toBe(baseDepth + 1);
    expect(turn3.depth).toBe(baseDepth + 2);
  });

  test('parent turn IDs form a chain', async ({ goWriter, serverHttpUrl, registry }) => {
    const ctx = goWriter.createContext();
    await registry.putBundle('test-bundle-v1');

    // Append 3 sequential turns
    const turn1 = goWriter.appendTurn(ctx.contextId, 'user', 'Message 1');
    const turn2 = goWriter.appendTurn(ctx.contextId, 'user', 'Message 2');
    const turn3 = goWriter.appendTurn(ctx.contextId, 'user', 'Message 3');

    // Fetch turns via HTTP to check parent relationships
    const response = await fetch(
      `${serverHttpUrl}/v1/contexts/${ctx.contextId}/turns?view=typed&limit=10`
    );
    const data = await response.json();

    // Sort turns by turn_id
    const turns = data.turns.sort(
      (a: { turn_id: string }, b: { turn_id: string }) =>
        parseInt(a.turn_id) - parseInt(b.turn_id)
    );

    // Turn 1: parent = 0 (root)
    expect(parseInt(turns[0].parent_turn_id)).toBe(0);

    // Turn 2: parent = turn 1
    expect(parseInt(turns[1].parent_turn_id)).toBe(turn1.turnId);

    // Turn 3: parent = turn 2
    expect(parseInt(turns[2].parent_turn_id)).toBe(turn2.turnId);
  });

  test('turn metadata shows in debugger detail panel', async ({
    apiPage,
    goWriter,
    registry,
  }) => {
    const ctx = goWriter.createContext();
    await registry.putBundle('test-bundle-v1');

    const turn1 = goWriter.appendTurn(ctx.contextId, 'user', 'Test message');

    await apiPage.goto('/');
    await addContext(apiPage, ctx.contextId);
    await waitForDebugger(apiPage);
    await waitForDebuggerLoaded(apiPage);

    await selectTimelineItem(apiPage, 0);

    // Verify Turn Metadata section exists (collapsed header is visible)
    await expect(
      apiPage.locator('[data-context-debugger]').getByText('Turn Metadata')
    ).toBeVisible();

    // Expand Turn Metadata section
    await expandTurnMetadata(apiPage);

    // Verify Turn ID is displayed
    await expect(
      apiPage.locator('[data-context-debugger]').getByText('Turn ID', { exact: true })
    ).toBeVisible();

    // Verify Parent is displayed
    await expect(
      apiPage.locator('[data-context-debugger]').getByText('Parent', { exact: true })
    ).toBeVisible();

    // Verify Depth is displayed
    await expect(
      apiPage.locator('[data-context-debugger]').getByText('Depth', { exact: true })
    ).toBeVisible();
  });

  test('first turn has consistent depth and parent 0', async ({ goWriter, serverHttpUrl, registry }) => {
    // Register type bundle for proper turn retrieval
    await registry.putBundle('test-bundle-v1');

    const ctx = goWriter.createContext();

    // First turn in a context
    const turn = goWriter.appendTurn(ctx.contextId, 'user', 'First message');

    // First turn depth should be consistent (0 or 1 depending on implementation)
    expect(turn.depth).toBeGreaterThanOrEqual(0);

    // Verify parent is 0 via HTTP API
    const response = await fetch(
      `${serverHttpUrl}/v1/contexts/${ctx.contextId}/turns?view=typed&limit=10`
    );
    const data = await response.json();

    // Ensure we got turns
    expect(data.turns).toBeDefined();
    expect(data.turns.length).toBeGreaterThan(0);

    const firstTurn = data.turns[0];
    expect(parseInt(firstTurn.parent_turn_id)).toBe(0);
  });

  test('deep link hydrates a turn outside the first page', async ({
    apiPage,
    goWriter,
    registry,
    serverHttpUrl,
  }) => {
    const ctx = goWriter.createContext();
    await registry.putBundle('test-bundle-v1');
    const response = await fetch(
      `${serverHttpUrl}/v1/contexts/${ctx.contextId}/append-batch`,
      {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          turns: Array.from({ length: 105 }, (_, index) => ({
            type_id: 'com.yourorg.ai.MessageTurn',
            type_version: 1,
            data: {
              role: index % 2 === 0 ? 'user' : 'assistant',
              text: index === 0 ? 'deep-linked oldest turn' : `turn ${index}`,
            },
          })),
        }),
      }
    );
    expect(response.status).toBe(201);
    const result = await response.json();
    const linkedTurnId = String(result.turns[0].turn_id);

    await apiPage.goto(`/c/${ctx.contextId}/t/${linkedTurnId}`);
    await waitForDebugger(apiPage);
    await waitForDebuggerLoaded(apiPage);

    await expect(apiPage.locator(`[data-turn-id="${linkedTurnId}"]`)).toBeVisible();
    await expect(await getRawPayload(apiPage)).toContainText('deep-linked oldest turn');
  });
});
