// Copyright 2025 StrongDM Inc
// SPDX-License-Identifier: Apache-2.0

'use client';

import { useState, useRef, useEffect } from 'react';
import { useTheme, type Theme, type ThemeId } from '@/lib/themes';
import { cn } from '@/lib/utils';
import { ChevronDown, Palette } from './icons';

interface ThemeSelectorProps {
  className?: string;
}

function ThemeSwatch({ theme, size = 'sm' }: { theme: Theme; size?: 'sm' | 'md' }) {
  const sizeClasses = size === 'sm' ? 'w-4 h-4' : 'w-5 h-5';
  return (
    <div
      className={cn(
        sizeClasses,
        'rounded-full border-2 flex-shrink-0'
      )}
      style={{
        backgroundColor: theme.colors.bg,
        borderColor: theme.colors.accent,
      }}
    />
  );
}

export function ThemeSelector({ className }: ThemeSelectorProps) {
  const { theme, setTheme, availableThemes } = useTheme();
  const [isOpen, setIsOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);

  // Close dropdown when clicking outside
  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      if (dropdownRef.current && !dropdownRef.current.contains(event.target as Node)) {
        setIsOpen(false);
      }
    }

    if (isOpen) {
      document.addEventListener('mousedown', handleClickOutside);
      return () => document.removeEventListener('mousedown', handleClickOutside);
    }
  }, [isOpen]);

  // Close dropdown on escape
  useEffect(() => {
    function handleEscape(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        setIsOpen(false);
      }
    }

    if (isOpen) {
      document.addEventListener('keydown', handleEscape);
      return () => document.removeEventListener('keydown', handleEscape);
    }
  }, [isOpen]);

  const handleSelect = (id: ThemeId) => {
    setTheme(id);
    setIsOpen(false);
  };

  return (
    <div ref={dropdownRef} className={cn('relative', className)}>
      <button
        onClick={() => setIsOpen(!isOpen)}
        className={cn(
          'flex items-center gap-2 px-2.5 py-1 rounded-full text-xs transition-colors',
          'bg-theme-bg-hover/50 text-theme-text-muted border border-theme-border hover:text-theme-text-secondary hover:border-theme-text-dim'
        )}
        aria-expanded={isOpen}
        aria-haspopup="listbox"
        aria-label={`Theme: ${theme.name}`}
      >
        <Palette className="w-3 h-3" />
        <ThemeSwatch theme={theme} />
        <span className="hidden sm:inline">{theme.name}</span>
        <ChevronDown
          className={cn(
            'w-3 h-3 transition-transform',
            isOpen && 'rotate-180'
          )}
        />
      </button>

      {isOpen && (
        <div
          className={cn(
            'absolute left-0 top-full z-50 mt-1 sm:left-auto sm:right-0',
            'bg-theme-bg-secondary border border-theme-border rounded-lg shadow-xl',
            'py-1 min-w-[180px]',
            'animate-fade-in'
          )}
          role="listbox"
        >
          {availableThemes.map((t) => (
            <button
              key={t.id}
              onClick={() => handleSelect(t.id)}
              className={cn(
                'w-full flex items-center gap-3 px-3 py-2 text-left text-sm transition-colors',
                'hover:bg-theme-bg-tertiary',
                t.id === theme.id
                  ? 'text-theme-text bg-theme-bg-tertiary/50'
                  : 'text-theme-text-muted'
              )}
              role="option"
              aria-selected={t.id === theme.id}
            >
              <ThemeSwatch theme={t} size="md" />
              <div className="flex-1 min-w-0">
                <div className={cn(
                  'font-medium',
                  t.id === theme.id ? 'text-theme-text' : 'text-theme-text-secondary'
                )}>
                  {t.name}
                </div>
                <div className="text-xs text-theme-text-dim truncate">
                  {t.description}
                </div>
              </div>
              {t.id === theme.id && (
                <span className="text-theme-accent text-lg">*</span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

export default ThemeSelector;
