/**
 * Theme color definitions for the CXDB frontend
 * All colors are hex strings that get applied to CSS variables
 */
export interface ThemeColors {
  // Core backgrounds (darkest to lightest)
  bg: string;           // Main background (darkest)
  bgSecondary: string;  // Secondary panels
  bgTertiary: string;   // Cards, elevated surfaces
  bgHover: string;      // Hover states
  card: string;         // Card backgrounds (with alpha)

  // Text colors (brightest to dimmest)
  text: string;         // Primary text
  textSecondary: string; // Secondary text
  textMuted: string;    // Muted text
  textDim: string;      // Dim/disabled text
  textFaint: string;    // Very faint text

  // Accent colors
  accent: string;
  accentDim: string;
  accentMuted: string;

  // Border colors
  border: string;
  borderDim: string;
  borderFaint: string;

  // Live/activity indicators
  liveGreen: string;
  liveGlow: string;
  liveGlowStrong: string;
  streamingAccent: string;
  activityFlash: string;

  // Warning
  warningYellow: string;

  // Gauge colors (dashboard)
  gaugeOk: string;
  gaugeWarn: string;
  gaugeHot: string;
  gaugeCritical: string;

  // Role colors
  roleUser: string;
  roleUserMuted: string;
  roleAssistant: string;
  roleAssistantMuted: string;
  roleSystem: string;
  roleSystemMuted: string;
  roleTool: string;
  roleToolMuted: string;

  // Tag colors
  tagDotrunner: string;
  tagDotrunnerBg: string;
  tagClaudeCode: string;
  tagClaudeCodeBg: string;
  tagCodex: string;
  tagCodexBg: string;
  tagGen: string;
  tagGenBg: string;
  tagTest: string;
  tagTestBg: string;
  tagDefault: string;
  tagDefaultBg: string;

  // State colors
  success: string;
  successMuted: string;
  error: string;
  errorMuted: string;
  warning: string;
  warningMuted: string;
  info: string;
  infoMuted: string;

  // Scrollbar
  scrollbarThumb: string;
  scrollbarThumbHover: string;

  // Selection
  selection: string;

  // Kbd element
  kbdBg: string;
  kbdBorder: string;
  kbdText: string;
}

export type ThemeId = 'sdm-brand' | 'original' | 'ember' | 'terminal' | 'forest' | 'coral' | 'obsidian';

export interface Theme {
  id: ThemeId;
  name: string;
  description: string;
  colors: ThemeColors;
}
