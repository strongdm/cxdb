// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

'use client';

import { useTheme } from '@/lib/themes/context';
import { cn } from '@/lib/utils';

export function ThemeSelector() {
  const { themeId, setTheme, availableThemes } = useTheme();

  return (
    <div className="flex items-center gap-1 p-0.5 bg-theme-bg-tertiary/50 rounded-lg">
      {availableThemes.map((t) => (
        <button
          key={t.id}
          onClick={() => setTheme(t.id)}
          className={cn(
            'px-2.5 py-1 text-xs font-medium rounded-md transition-all',
            themeId === t.id
              ? 'bg-theme-bg-secondary text-theme-text-secondary shadow-sm'
              : 'text-theme-text-dim hover:text-theme-text-muted hover:bg-theme-bg-tertiary/50'
          )}
          title={t.name}
        >
          {t.name}
        </button>
      ))}
    </div>
  );
}
