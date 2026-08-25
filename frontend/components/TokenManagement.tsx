// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

'use client';

import { useEffect, useState } from 'react';
import type { APITokenMetadata } from '@/types';
import { AlertCircle, Check, Copy, Loader2, Lock, Plus, Trash2, X } from '@/components/icons';
import { createAPIToken, fetchAPITokens, fetchCurrentUser, revokeAPIToken } from '@/lib/api';
import { cn } from '@/lib/utils';

interface TokenManagementProps {
  isOpen: boolean;
  onClose: () => void;
}

function formatDate(value?: string | null): string {
  if (!value) return 'Never';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return 'Unknown';
  return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(date);
}

function isExpired(value: string): boolean {
  const date = new Date(value);
  return !Number.isNaN(date.getTime()) && date.getTime() <= Date.now();
}

export function TokenManagement({ isOpen, onClose }: TokenManagementProps) {
  const [tokens, setTokens] = useState<APITokenMetadata[]>([]);
  const [csrfToken, setCsrfToken] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [includeWrite, setIncludeWrite] = useState(false);
  const [expiresAt, setExpiresAt] = useState('');
  const [newPlaintext, setNewPlaintext] = useState<string | null>(null);
  const [copyStatus, setCopyStatus] = useState<'idle' | 'copied' | 'failed'>('idle');
  const [revokingId, setRevokingId] = useState<string | null>(null);
  const [confirmingId, setConfirmingId] = useState<string | null>(null);

  useEffect(() => {
    if (!isOpen) {
      setNewPlaintext(null);
      setCopyStatus('idle');
      setConfirmingId(null);
      return;
    }

    let cancelled = false;
    setLoading(true);
    setError(null);
    const load = async () => {
      try {
        const user = await fetchCurrentUser();
        const listedTokens = await fetchAPITokens();
        if (!cancelled) {
          setCsrfToken(user.csrf_token);
          setTokens(listedTokens);
        }
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : 'Unable to load API tokens.');
      } finally {
        if (!cancelled) setLoading(false);
      }
    };
    void load();
    return () => { cancelled = true; };
  }, [isOpen]);

  if (!isOpen) return null;

  const closePanel = () => {
    setNewPlaintext(null);
    setCopyStatus('idle');
    onClose();
  };

  const handleCreate = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const trimmedName = name.trim();
    if (!trimmedName) {
      setError('Enter a name for this token.');
      return;
    }
    if (!csrfToken) {
      setError('Your browser session is not ready. Reload and try again.');
      return;
    }

    let expires: string | undefined;
    if (expiresAt) {
      const date = new Date(expiresAt);
      if (Number.isNaN(date.getTime()) || date.getTime() <= Date.now()) {
        setError('Expiry must be a future date.');
        return;
      }
      expires = date.toISOString();
    }

    setCreating(true);
    setError(null);
    try {
      const result = await createAPIToken(csrfToken, {
        name: trimmedName,
        scopes: includeWrite ? ['cxdb:read', 'cxdb:write'] : ['cxdb:read'],
        ...(expires ? { expires_at: expires } : {}),
      });
      setTokens(previous => [result.token, ...previous.filter(token => token.id !== result.token.id)]);
      setNewPlaintext(result.plaintext);
      setCopyStatus('idle');
      setName('');
      setIncludeWrite(false);
      setExpiresAt('');
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Unable to create API token.');
    } finally {
      setCreating(false);
    }
  };

  const handleCopy = async () => {
    if (!newPlaintext) return;
    try {
      await navigator.clipboard.writeText(newPlaintext);
      setCopyStatus('copied');
    } catch {
      setCopyStatus('failed');
    }
  };

  const handleRevoke = async (tokenId: string) => {
    if (!csrfToken) {
      setError('Your browser session is not ready. Reload and try again.');
      return;
    }
    setRevokingId(tokenId);
    setError(null);
    try {
      await revokeAPIToken(csrfToken, tokenId);
      setTokens(previous => previous.map(token => token.id === tokenId
        ? { ...token, revoked_at: new Date().toISOString() }
        : token));
      setConfirmingId(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Unable to revoke API token.');
    } finally {
      setRevokingId(null);
    }
  };

  return (
    <div data-token-management className="fixed inset-0 z-50 flex items-start justify-center overflow-hidden bg-black/60 p-0 sm:p-8" role="dialog" aria-modal="true" aria-labelledby="token-management-title">
      <div className="flex h-[100dvh] w-full max-w-3xl flex-col overflow-hidden border border-theme-border bg-theme-bg-secondary shadow-2xl sm:h-auto sm:max-h-[calc(100dvh-4rem)] sm:rounded-xl">
        <div className="flex shrink-0 items-center justify-between border-b border-theme-border-dim px-4 py-3 sm:px-5 sm:py-4">
          <div className="flex items-center gap-3">
            <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-theme-accent-muted text-theme-accent"><Lock className="h-4 w-4" /></div>
            <div>
              <h2 id="token-management-title" className="text-base font-semibold text-theme-text">API tokens</h2>
              <p className="text-xs text-theme-text-dim">Manage personal access for tools and scripts.</p>
            </div>
          </div>
          <button type="button" onClick={closePanel} className="rounded-md p-2 text-theme-text-dim hover:bg-theme-bg-tertiary hover:text-theme-text" aria-label="Close API token management"><X className="h-4 w-4" /></button>
        </div>

        <div className="flex-1 space-y-5 overflow-y-auto p-3 pb-6 sm:p-5">
          {error && <div role="alert" className="flex items-start gap-2 rounded-lg border border-red-500/30 bg-red-500/10 px-3 py-2.5 text-sm text-red-300"><AlertCircle className="mt-0.5 h-4 w-4 shrink-0" /><span>{error}</span></div>}

          {newPlaintext && (
            <section className="rounded-lg border border-amber-500/40 bg-amber-500/10 p-4" aria-labelledby="new-token-title">
              <h3 id="new-token-title" className="text-sm font-semibold text-amber-200">Token created</h3>
              <p className="mt-1 text-xs leading-relaxed text-amber-100/80">This secret is shown only once. Copy it now. It will not be available again.</p>
              <div className="mt-3 flex flex-col gap-2 sm:flex-row">
                <input value={newPlaintext} readOnly type="text" aria-label="New API token secret" className="min-w-0 flex-1 rounded-md border border-amber-500/40 bg-theme-bg px-3 py-2 font-mono text-xs text-theme-text" />
                <button type="button" onClick={() => void handleCopy()} className="flex min-h-10 shrink-0 items-center justify-center gap-1.5 rounded-md bg-amber-500 px-3 py-2 text-xs font-semibold text-black hover:bg-amber-400">
                  {copyStatus === 'copied' ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}{copyStatus === 'copied' ? 'Copied' : 'Copy'}
                </button>
              </div>
              {copyStatus === 'failed' && <p className="mt-2 text-xs text-red-300">Copy failed. Select the secret and copy it manually.</p>}
            </section>
          )}

          <section>
            <h3 className="mb-3 flex items-center gap-2 text-sm font-semibold text-theme-text"><Plus className="h-4 w-4 text-theme-accent" />Create token</h3>
            <form onSubmit={handleCreate} className="grid gap-3 rounded-lg border border-theme-border-dim bg-theme-bg/40 p-3 sm:grid-cols-2 sm:p-4">
              <label className="sm:col-span-2"><span className="mb-1 block text-xs text-theme-text-muted">Name</span><input value={name} onChange={event => setName(event.target.value)} placeholder="My laptop" required maxLength={120} className="w-full rounded-md border border-theme-border bg-theme-bg-secondary px-3 py-2 text-sm text-theme-text placeholder:text-theme-text-faint focus:outline-none focus:ring-2 focus:ring-theme-accent/30" /></label>
              <fieldset className="sm:col-span-2"><legend className="mb-1.5 block text-xs text-theme-text-muted">Scopes</legend>
                <label className="flex items-center gap-2 text-sm text-theme-text-secondary"><input type="checkbox" checked disabled className="accent-theme-accent" />Read context data</label>
                <label className="mt-2 flex items-center gap-2 text-sm text-theme-text-secondary"><input type="checkbox" checked={includeWrite} onChange={event => setIncludeWrite(event.target.checked)} className="accent-theme-accent" />Write context data</label>
              </fieldset>
              <label><span className="mb-1 block text-xs text-theme-text-muted">Expiry (optional)</span><input type="datetime-local" value={expiresAt} onChange={event => setExpiresAt(event.target.value)} className="w-full rounded-md border border-theme-border bg-theme-bg-secondary px-3 py-2 text-sm text-theme-text focus:outline-none focus:ring-2 focus:ring-theme-accent/30" /></label>
              <div className="flex items-end justify-stretch sm:justify-end"><button type="submit" disabled={creating || loading || !csrfToken} className="flex min-h-11 w-full items-center justify-center gap-2 rounded-md bg-theme-accent px-3 py-2 text-sm font-semibold text-white transition-colors hover:bg-theme-accent-dim disabled:cursor-not-allowed disabled:opacity-50 sm:min-h-0 sm:w-auto">{creating && <Loader2 className="h-4 w-4 animate-spin" />}{creating ? 'Creating...' : 'Create token'}</button></div>
            </form>
          </section>

          <section>
            <h3 className="mb-3 text-sm font-semibold text-theme-text">Your tokens</h3>
            {loading ? <div className="flex items-center justify-center gap-2 rounded-lg border border-theme-border-dim py-10 text-sm text-theme-text-muted"><Loader2 className="h-4 w-4 animate-spin" />Loading tokens...</div> : tokens.length === 0 ? <div className="rounded-lg border border-dashed border-theme-border-dim py-8 text-center text-sm text-theme-text-dim">No API tokens yet.</div> : (
              <div className="space-y-2">
                {tokens.map(token => {
                  const revoked = Boolean(token.revoked_at);
                  const expired = isExpired(token.expires_at);
                  return <article key={token.id} className={cn('rounded-lg border border-theme-border-dim bg-theme-bg/40 p-3', revoked && 'opacity-60')}>
                    <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between"><div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-2"><h4 className="font-medium text-theme-text">{token.name}</h4>{revoked && <span className="rounded bg-red-500/15 px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-red-300">Revoked</span>}{!revoked && expired && <span className="rounded bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-amber-300">Expired</span>}</div>
                      <p className="mt-1 break-all font-mono text-xs text-theme-text-dim" title={token.id}>{token.prefix}</p>
                      <div className="mt-2 flex flex-wrap gap-1.5">{token.scopes.map(scope => <span key={scope} className="rounded bg-theme-accent-muted px-1.5 py-0.5 text-[10px] text-theme-accent">{scope}</span>)}</div>
                      <dl className="mt-3 grid grid-cols-1 gap-x-5 gap-y-1 text-xs text-theme-text-dim sm:grid-cols-3"><div><dt className="inline">Created: </dt><dd className="inline text-theme-text-muted">{formatDate(token.created_at)}</dd></div><div><dt className="inline">Expires: </dt><dd className="inline text-theme-text-muted">{formatDate(token.expires_at)}</dd></div><div><dt className="inline">Last used: </dt><dd className="inline text-theme-text-muted">{formatDate(token.last_used_at)}</dd></div></dl>
                    </div>
                    {!revoked && (confirmingId === token.id ? <div className="flex shrink-0 flex-wrap items-center gap-2 text-xs"><span className="text-theme-text-muted">Revoke this token?</span><button type="button" onClick={() => setConfirmingId(null)} className="rounded px-2 py-1 text-theme-text-muted hover:bg-theme-bg-tertiary">Cancel</button><button type="button" onClick={() => void handleRevoke(token.id)} disabled={revokingId === token.id} className="flex items-center gap-1 rounded bg-red-600/80 px-2 py-1 font-medium text-white hover:bg-red-600 disabled:opacity-50">{revokingId === token.id && <Loader2 className="h-3 w-3 animate-spin" />}Revoke</button></div> : <button type="button" onClick={() => setConfirmingId(token.id)} className="flex shrink-0 items-center gap-1.5 rounded-md border border-red-500/30 px-2.5 py-1.5 text-xs text-red-300 hover:bg-red-500/10"><Trash2 className="h-3.5 w-3.5" />Revoke</button>)}
                    </div>
                  </article>;
                })}
              </div>
            )}
          </section>
        </div>
      </div>
    </div>
  );
}
