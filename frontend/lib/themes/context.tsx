'use client';

import {
  createContext,
  useContext,
  useState,
  useEffect,
  useCallback,
  useMemo,
  type ReactNode,
} from 'react';
import type { Theme, ThemeId, ThemeColors } from './types';
import { themes, themesById, defaultTheme } from './definitions';

const STORAGE_KEY = 'cxdb-theme';

interface ThemeContextValue {
  theme: Theme;
  themeId: ThemeId;
  setTheme: (id: ThemeId) => void;
  availableThemes: Theme[];
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

/**
 * Apply theme colors as CSS variables to the document root
 */
function applyThemeToDocument(colors: ThemeColors): void {
  if (typeof document === 'undefined') return;

  const root = document.documentElement;

  // Core backgrounds
  root.style.setProperty('--theme-bg', colors.bg);
  root.style.setProperty('--theme-bg-secondary', colors.bgSecondary);
  root.style.setProperty('--theme-bg-tertiary', colors.bgTertiary);
  root.style.setProperty('--theme-bg-hover', colors.bgHover);
  root.style.setProperty('--theme-card', colors.card);

  // Text colors
  root.style.setProperty('--theme-text', colors.text);
  root.style.setProperty('--theme-text-secondary', colors.textSecondary);
  root.style.setProperty('--theme-text-muted', colors.textMuted);
  root.style.setProperty('--theme-text-dim', colors.textDim);
  root.style.setProperty('--theme-text-faint', colors.textFaint);

  // Accent colors
  root.style.setProperty('--theme-accent', colors.accent);
  root.style.setProperty('--theme-accent-dim', colors.accentDim);
  root.style.setProperty('--theme-accent-muted', colors.accentMuted);

  // Border colors
  root.style.setProperty('--theme-border', colors.border);
  root.style.setProperty('--theme-border-dim', colors.borderDim);
  root.style.setProperty('--theme-border-faint', colors.borderFaint);

  // Live/activity indicators
  root.style.setProperty('--theme-live-green', colors.liveGreen);
  root.style.setProperty('--theme-live-glow', colors.liveGlow);
  root.style.setProperty('--theme-live-glow-strong', colors.liveGlowStrong);
  root.style.setProperty('--theme-streaming-accent', colors.streamingAccent);
  root.style.setProperty('--theme-activity-flash', colors.activityFlash);

  // Warning
  root.style.setProperty('--theme-warning-yellow', colors.warningYellow);

  // Gauge colors
  root.style.setProperty('--theme-gauge-ok', colors.gaugeOk);
  root.style.setProperty('--theme-gauge-warn', colors.gaugeWarn);
  root.style.setProperty('--theme-gauge-hot', colors.gaugeHot);
  root.style.setProperty('--theme-gauge-critical', colors.gaugeCritical);

  // Role colors
  root.style.setProperty('--theme-role-user', colors.roleUser);
  root.style.setProperty('--theme-role-user-muted', colors.roleUserMuted);
  root.style.setProperty('--theme-role-assistant', colors.roleAssistant);
  root.style.setProperty('--theme-role-assistant-muted', colors.roleAssistantMuted);
  root.style.setProperty('--theme-role-system', colors.roleSystem);
  root.style.setProperty('--theme-role-system-muted', colors.roleSystemMuted);
  root.style.setProperty('--theme-role-tool', colors.roleTool);
  root.style.setProperty('--theme-role-tool-muted', colors.roleToolMuted);

  // Tag colors
  root.style.setProperty('--theme-tag-dotrunner', colors.tagDotrunner);
  root.style.setProperty('--theme-tag-dotrunner-bg', colors.tagDotrunnerBg);
  root.style.setProperty('--theme-tag-claude-code', colors.tagClaudeCode);
  root.style.setProperty('--theme-tag-claude-code-bg', colors.tagClaudeCodeBg);
  root.style.setProperty('--theme-tag-codex', colors.tagCodex);
  root.style.setProperty('--theme-tag-codex-bg', colors.tagCodexBg);
  root.style.setProperty('--theme-tag-gen', colors.tagGen);
  root.style.setProperty('--theme-tag-gen-bg', colors.tagGenBg);
  root.style.setProperty('--theme-tag-test', colors.tagTest);
  root.style.setProperty('--theme-tag-test-bg', colors.tagTestBg);
  root.style.setProperty('--theme-tag-default', colors.tagDefault);
  root.style.setProperty('--theme-tag-default-bg', colors.tagDefaultBg);

  // State colors
  root.style.setProperty('--theme-success', colors.success);
  root.style.setProperty('--theme-success-muted', colors.successMuted);
  root.style.setProperty('--theme-error', colors.error);
  root.style.setProperty('--theme-error-muted', colors.errorMuted);
  root.style.setProperty('--theme-warning', colors.warning);
  root.style.setProperty('--theme-warning-muted', colors.warningMuted);
  root.style.setProperty('--theme-info', colors.info);
  root.style.setProperty('--theme-info-muted', colors.infoMuted);

  // Scrollbar
  root.style.setProperty('--theme-scrollbar-thumb', colors.scrollbarThumb);
  root.style.setProperty('--theme-scrollbar-thumb-hover', colors.scrollbarThumbHover);

  // Selection
  root.style.setProperty('--theme-selection', colors.selection);

  // Kbd element
  root.style.setProperty('--theme-kbd-bg', colors.kbdBg);
  root.style.setProperty('--theme-kbd-border', colors.kbdBorder);
  root.style.setProperty('--theme-kbd-text', colors.kbdText);

  // Also update the existing CSS variables for backwards compatibility
  root.style.setProperty('--bg', colors.bg);
  root.style.setProperty('--bg-secondary', colors.bgSecondary);
  root.style.setProperty('--card', colors.card);
  root.style.setProperty('--accent', colors.accent);
  root.style.setProperty('--accent-dim', colors.accentDim);
  root.style.setProperty('--text', colors.text);
  root.style.setProperty('--text-muted', colors.textMuted);
  root.style.setProperty('--border', colors.border);
  root.style.setProperty('--live-green', colors.liveGreen);
  root.style.setProperty('--live-glow', colors.liveGlow);
  root.style.setProperty('--live-glow-strong', colors.liveGlowStrong);
  root.style.setProperty('--activity-flash', colors.activityFlash);
  root.style.setProperty('--streaming-accent', colors.streamingAccent);
  root.style.setProperty('--warning-yellow', colors.warningYellow);
  root.style.setProperty('--gauge-ok', colors.gaugeOk);
  root.style.setProperty('--gauge-warn', colors.gaugeWarn);
  root.style.setProperty('--gauge-hot', colors.gaugeHot);
  root.style.setProperty('--gauge-critical', colors.gaugeCritical);
}

/**
 * Load theme from localStorage
 */
function loadSavedTheme(): Theme {
  if (typeof window === 'undefined') return defaultTheme;

  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved && themesById[saved]) {
      return themesById[saved];
    }
  } catch {
    // localStorage not available
  }

  return defaultTheme;
}

/**
 * Save theme to localStorage
 */
function saveTheme(id: ThemeId): void {
  if (typeof window === 'undefined') return;

  try {
    localStorage.setItem(STORAGE_KEY, id);
  } catch {
    // localStorage not available
  }
}

interface ThemeProviderProps {
  children: ReactNode;
}

export function ThemeProvider({ children }: ThemeProviderProps) {
  const [theme, setThemeState] = useState<Theme>(defaultTheme);
  const [mounted, setMounted] = useState(false);

  // Load saved theme on mount
  useEffect(() => {
    const savedTheme = loadSavedTheme();
    setThemeState(savedTheme);
    applyThemeToDocument(savedTheme.colors);
    setMounted(true);
  }, []);

  // Apply theme colors when theme changes
  useEffect(() => {
    if (mounted) {
      applyThemeToDocument(theme.colors);
    }
  }, [theme, mounted]);

  const setTheme = useCallback((id: ThemeId) => {
    const newTheme = themesById[id];
    if (newTheme) {
      setThemeState(newTheme);
      saveTheme(id);
    }
  }, []);

  const value = useMemo<ThemeContextValue>(
    () => ({
      theme,
      themeId: theme.id,
      setTheme,
      availableThemes: themes,
    }),
    [theme, setTheme]
  );

  return (
    <ThemeContext.Provider value={value}>
      {children}
    </ThemeContext.Provider>
  );
}

export function useTheme(): ThemeContextValue {
  const context = useContext(ThemeContext);
  if (!context) {
    throw new Error('useTheme must be used within a ThemeProvider');
  }
  return context;
}
