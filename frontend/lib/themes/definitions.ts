import type { Theme, ThemeColors } from './types';

/**
 * Original theme - matches current CXDB appearance exactly
 * Purple accent on blue-black background
 */
const originalColors: ThemeColors = {
  // Core backgrounds (slate-950 → slate-800)
  bg: '#080d16',           // slate-950 equivalent
  bgSecondary: '#0d1424',  // slate-925 equivalent
  bgTertiary: '#1e293b',   // slate-800
  bgHover: '#334155',      // slate-700
  card: 'rgba(15, 23, 42, 0.8)',

  // Text colors (slate-100 → slate-600)
  text: '#f1f5f9',         // slate-100
  textSecondary: '#cbd5e1', // slate-300
  textMuted: '#94a3b8',    // slate-400
  textDim: '#64748b',      // slate-500
  textFaint: '#475569',    // slate-600

  // Accent colors
  accent: '#a855f7',
  accentDim: '#7c3aed',
  accentMuted: 'rgba(168, 85, 247, 0.2)',

  // Border colors (subtle but visible)
  border: '#182430',      // visible but not harsh
  borderDim: '#121c26',   // subtle
  borderFaint: '#0c1418', // very dim

  // Live/activity indicators
  liveGreen: '#22c55e',
  liveGlow: 'rgba(34, 197, 94, 0.4)',
  liveGlowStrong: 'rgba(34, 197, 94, 0.6)',
  streamingAccent: 'rgba(168, 85, 247, 0.8)',
  activityFlash: 'rgba(168, 85, 247, 0.15)',

  // Warning
  warningYellow: '#eab308',

  // Gauge colors
  gaugeOk: '#22c55e',
  gaugeWarn: '#eab308',
  gaugeHot: '#f97316',
  gaugeCritical: '#ef4444',

  // Role colors
  roleUser: '#3b82f6',
  roleUserMuted: 'rgba(59, 130, 246, 0.2)',
  roleAssistant: '#a855f7',
  roleAssistantMuted: 'rgba(168, 85, 247, 0.2)',
  roleSystem: '#64748b',
  roleSystemMuted: 'rgba(100, 116, 139, 0.2)',
  roleTool: '#f59e0b',
  roleToolMuted: 'rgba(245, 158, 11, 0.2)',

  // Tag colors
  tagDotrunner: '#60a5fa',
  tagDotrunnerBg: 'rgba(37, 99, 235, 0.2)',
  tagClaudeCode: '#c084fc',
  tagClaudeCodeBg: 'rgba(147, 51, 234, 0.2)',
  tagCodex: '#22c55e',
  tagCodexBg: 'rgba(34, 197, 94, 0.2)',
  tagGen: '#34d399',
  tagGenBg: 'rgba(16, 185, 129, 0.2)',
  tagTest: '#fbbf24',
  tagTestBg: 'rgba(245, 158, 11, 0.2)',
  tagDefault: '#94a3b8',
  tagDefaultBg: 'rgba(71, 85, 105, 0.2)',

  // State colors
  success: '#22c55e',
  successMuted: 'rgba(34, 197, 94, 0.2)',
  error: '#ef4444',
  errorMuted: 'rgba(239, 68, 68, 0.2)',
  warning: '#eab308',
  warningMuted: 'rgba(234, 179, 8, 0.2)',
  info: '#3b82f6',
  infoMuted: 'rgba(59, 130, 246, 0.2)',

  // Scrollbar
  scrollbarThumb: '#475569',
  scrollbarThumbHover: '#64748b',

  // Selection
  selection: 'rgba(168, 85, 247, 0.3)',

  // Kbd element
  kbdBg: '#121c26',
  kbdBorder: '#182430',
  kbdText: '#cbd5e1',
};

/**
 * Ember theme - warm amber accents on warm black background
 * Cozy and inviting
 */
const emberColors: ThemeColors = {
  // Core backgrounds (warm blacks)
  bg: '#0f0d0a',
  bgSecondary: '#1a1612',
  bgTertiary: '#292524',   // stone-800
  bgHover: '#44403c',      // stone-700
  card: 'rgba(26, 22, 18, 0.8)',

  // Text colors (warm whites)
  text: '#faf5f0',
  textSecondary: '#e7e5e4', // stone-200
  textMuted: '#a8a29e',    // stone-400
  textDim: '#78716c',      // stone-500
  textFaint: '#57534e',    // stone-600

  // Accent colors
  accent: '#f59e0b',
  accentDim: '#d97706',
  accentMuted: 'rgba(245, 158, 11, 0.2)',

  // Border colors (subtle but visible)
  border: '#2a241e',      // visible but not harsh
  borderDim: '#221c18',   // subtle
  borderFaint: '#1a1612', // very dim

  // Live/activity indicators
  liveGreen: '#22c55e',
  liveGlow: 'rgba(34, 197, 94, 0.4)',
  liveGlowStrong: 'rgba(34, 197, 94, 0.6)',
  streamingAccent: 'rgba(245, 158, 11, 0.8)',
  activityFlash: 'rgba(245, 158, 11, 0.15)',

  // Warning
  warningYellow: '#fbbf24',

  // Gauge colors
  gaugeOk: '#22c55e',
  gaugeWarn: '#fbbf24',
  gaugeHot: '#f97316',
  gaugeCritical: '#ef4444',

  // Role colors
  roleUser: '#38bdf8',
  roleUserMuted: 'rgba(56, 189, 248, 0.2)',
  roleAssistant: '#f59e0b',
  roleAssistantMuted: 'rgba(245, 158, 11, 0.2)',
  roleSystem: '#78716c',
  roleSystemMuted: 'rgba(120, 113, 108, 0.2)',
  roleTool: '#fb923c',
  roleToolMuted: 'rgba(251, 146, 60, 0.2)',

  // Tag colors
  tagDotrunner: '#38bdf8',
  tagDotrunnerBg: 'rgba(56, 189, 248, 0.2)',
  tagClaudeCode: '#f59e0b',
  tagClaudeCodeBg: 'rgba(245, 158, 11, 0.2)',
  tagCodex: '#fb7185',
  tagCodexBg: 'rgba(251, 113, 133, 0.2)',
  tagGen: '#34d399',
  tagGenBg: 'rgba(52, 211, 153, 0.2)',
  tagTest: '#fbbf24',
  tagTestBg: 'rgba(251, 191, 36, 0.2)',
  tagDefault: '#a8a29e',
  tagDefaultBg: 'rgba(120, 113, 108, 0.2)',

  // State colors
  success: '#22c55e',
  successMuted: 'rgba(34, 197, 94, 0.2)',
  error: '#ef4444',
  errorMuted: 'rgba(239, 68, 68, 0.2)',
  warning: '#fbbf24',
  warningMuted: 'rgba(251, 191, 36, 0.2)',
  info: '#38bdf8',
  infoMuted: 'rgba(56, 189, 248, 0.2)',

  // Scrollbar
  scrollbarThumb: '#57534e',
  scrollbarThumbHover: '#78716c',

  // Selection
  selection: 'rgba(245, 158, 11, 0.3)',

  // Kbd element
  kbdBg: '#221c18',
  kbdBorder: '#2a241e',
  kbdText: '#d6d3d1',
};

/**
 * Terminal theme - cyan accent on true black background
 * Retro tech / hacker aesthetic
 */
const terminalColors: ThemeColors = {
  // Core backgrounds (true black + cyan tints)
  bg: '#000000',
  bgSecondary: '#0a1214',
  bgTertiary: '#164e63',   // cyan-800
  bgHover: '#155e75',      // cyan-700
  card: 'rgba(10, 18, 20, 0.8)',

  // Text colors (cyan-tinted)
  text: '#e0f2fe',         // sky-100
  textSecondary: '#bae6fd', // sky-200
  textMuted: '#67e8f9',    // cyan-300
  textDim: '#22d3ee',      // cyan-400
  textFaint: '#06b6d4',    // cyan-500

  // Accent colors
  accent: '#06b6d4',
  accentDim: '#0891b2',
  accentMuted: 'rgba(6, 182, 212, 0.2)',

  // Border colors (subtle but visible)
  border: '#142028',      // visible but not harsh
  borderDim: '#0e181e',   // subtle
  borderFaint: '#0a1216', // very dim

  // Live/activity indicators
  liveGreen: '#22c55e',
  liveGlow: 'rgba(34, 197, 94, 0.4)',
  liveGlowStrong: 'rgba(34, 197, 94, 0.6)',
  streamingAccent: 'rgba(6, 182, 212, 0.8)',
  activityFlash: 'rgba(6, 182, 212, 0.15)',

  // Warning
  warningYellow: '#fbbf24',

  // Gauge colors
  gaugeOk: '#22c55e',
  gaugeWarn: '#fbbf24',
  gaugeHot: '#f97316',
  gaugeCritical: '#ef4444',

  // Role colors
  roleUser: '#38bdf8',
  roleUserMuted: 'rgba(56, 189, 248, 0.2)',
  roleAssistant: '#06b6d4',
  roleAssistantMuted: 'rgba(6, 182, 212, 0.2)',
  roleSystem: '#67e8f9',
  roleSystemMuted: 'rgba(103, 232, 249, 0.2)',
  roleTool: '#2dd4bf',
  roleToolMuted: 'rgba(45, 212, 191, 0.2)',

  // Tag colors
  tagDotrunner: '#38bdf8',
  tagDotrunnerBg: 'rgba(56, 189, 248, 0.2)',
  tagClaudeCode: '#06b6d4',
  tagClaudeCodeBg: 'rgba(6, 182, 212, 0.2)',
  tagCodex: '#4ade80',
  tagCodexBg: 'rgba(74, 222, 128, 0.2)',
  tagGen: '#2dd4bf',
  tagGenBg: 'rgba(45, 212, 191, 0.2)',
  tagTest: '#fbbf24',
  tagTestBg: 'rgba(251, 191, 36, 0.2)',
  tagDefault: '#67e8f9',
  tagDefaultBg: 'rgba(103, 232, 249, 0.2)',

  // State colors
  success: '#22c55e',
  successMuted: 'rgba(34, 197, 94, 0.2)',
  error: '#ef4444',
  errorMuted: 'rgba(239, 68, 68, 0.2)',
  warning: '#fbbf24',
  warningMuted: 'rgba(251, 191, 36, 0.2)',
  info: '#06b6d4',
  infoMuted: 'rgba(6, 182, 212, 0.2)',

  // Scrollbar
  scrollbarThumb: '#155e75',
  scrollbarThumbHover: '#0e7490',

  // Selection
  selection: 'rgba(6, 182, 212, 0.3)',

  // Kbd element
  kbdBg: '#0e181e',
  kbdBorder: '#142028',
  kbdText: '#a5f3fc',
};

/**
 * Forest theme - emerald accent on deep green background
 * Calm and natural
 */
const forestColors: ThemeColors = {
  // Core backgrounds (deep greens)
  bg: '#030f0a',
  bgSecondary: '#0a1f15',
  bgTertiary: '#064e3b',   // emerald-800
  bgHover: '#047857',      // emerald-700
  card: 'rgba(10, 31, 21, 0.8)',

  // Text colors (green-tinted)
  text: '#ecfdf5',         // emerald-50
  textSecondary: '#d1fae5', // emerald-100
  textMuted: '#6ee7b7',    // emerald-300
  textDim: '#34d399',      // emerald-400
  textFaint: '#10b981',    // emerald-500

  // Accent colors
  accent: '#10b981',
  accentDim: '#059669',
  accentMuted: 'rgba(16, 185, 129, 0.2)',

  // Border colors (subtle but visible)
  border: '#143024',      // visible but not harsh
  borderDim: '#0e261c',   // subtle
  borderFaint: '#0a1c14', // very dim

  // Live/activity indicators (use blue for "live" since green is accent)
  liveGreen: '#3b82f6',
  liveGlow: 'rgba(59, 130, 246, 0.4)',
  liveGlowStrong: 'rgba(59, 130, 246, 0.6)',
  streamingAccent: 'rgba(16, 185, 129, 0.8)',
  activityFlash: 'rgba(16, 185, 129, 0.15)',

  // Warning
  warningYellow: '#fbbf24',

  // Gauge colors (use blue for OK since green is accent)
  gaugeOk: '#3b82f6',
  gaugeWarn: '#fbbf24',
  gaugeHot: '#f97316',
  gaugeCritical: '#ef4444',

  // Role colors
  roleUser: '#38bdf8',
  roleUserMuted: 'rgba(56, 189, 248, 0.2)',
  roleAssistant: '#10b981',
  roleAssistantMuted: 'rgba(16, 185, 129, 0.2)',
  roleSystem: '#6ee7b7',
  roleSystemMuted: 'rgba(110, 231, 183, 0.2)',
  roleTool: '#2dd4bf',
  roleToolMuted: 'rgba(45, 212, 191, 0.2)',

  // Tag colors
  tagDotrunner: '#38bdf8',
  tagDotrunnerBg: 'rgba(56, 189, 248, 0.2)',
  tagClaudeCode: '#10b981',
  tagClaudeCodeBg: 'rgba(16, 185, 129, 0.2)',
  tagCodex: '#f59e0b',
  tagCodexBg: 'rgba(245, 158, 11, 0.2)',
  tagGen: '#2dd4bf',
  tagGenBg: 'rgba(45, 212, 191, 0.2)',
  tagTest: '#fbbf24',
  tagTestBg: 'rgba(251, 191, 36, 0.2)',
  tagDefault: '#6ee7b7',
  tagDefaultBg: 'rgba(110, 231, 183, 0.2)',

  // State colors
  success: '#3b82f6',
  successMuted: 'rgba(59, 130, 246, 0.2)',
  error: '#ef4444',
  errorMuted: 'rgba(239, 68, 68, 0.2)',
  warning: '#fbbf24',
  warningMuted: 'rgba(251, 191, 36, 0.2)',
  info: '#10b981',
  infoMuted: 'rgba(16, 185, 129, 0.2)',

  // Scrollbar
  scrollbarThumb: '#047857',
  scrollbarThumbHover: '#059669',

  // Selection
  selection: 'rgba(16, 185, 129, 0.3)',

  // Kbd element
  kbdBg: '#0e261c',
  kbdBorder: '#143024',
  kbdText: '#a7f3d0',
};

/**
 * Coral theme - rose accent on warm gray background
 * Modern and approachable
 */
const coralColors: ThemeColors = {
  // Core backgrounds (warm grays with rose tint)
  bg: '#0c0a0d',
  bgSecondary: '#18141a',
  bgTertiary: '#27272a',   // zinc-800
  bgHover: '#3f3f46',      // zinc-700
  card: 'rgba(24, 20, 26, 0.8)',

  // Text colors (rose-tinted whites)
  text: '#fdf2f4',         // rose-50
  textSecondary: '#fecdd3', // rose-200
  textMuted: '#a1a1aa',    // zinc-400
  textDim: '#71717a',      // zinc-500
  textFaint: '#52525b',    // zinc-600

  // Accent colors
  accent: '#f43f5e',
  accentDim: '#e11d48',
  accentMuted: 'rgba(244, 63, 94, 0.2)',

  // Border colors (subtle but visible)
  border: '#242028',      // visible but not harsh
  borderDim: '#1e1a20',   // subtle
  borderFaint: '#181418', // very dim

  // Live/activity indicators
  liveGreen: '#22c55e',
  liveGlow: 'rgba(34, 197, 94, 0.4)',
  liveGlowStrong: 'rgba(34, 197, 94, 0.6)',
  streamingAccent: 'rgba(244, 63, 94, 0.8)',
  activityFlash: 'rgba(244, 63, 94, 0.15)',

  // Warning
  warningYellow: '#fbbf24',

  // Gauge colors
  gaugeOk: '#22c55e',
  gaugeWarn: '#fbbf24',
  gaugeHot: '#f97316',
  gaugeCritical: '#ef4444',

  // Role colors
  roleUser: '#38bdf8',
  roleUserMuted: 'rgba(56, 189, 248, 0.2)',
  roleAssistant: '#f43f5e',
  roleAssistantMuted: 'rgba(244, 63, 94, 0.2)',
  roleSystem: '#a1a1aa',
  roleSystemMuted: 'rgba(161, 161, 170, 0.2)',
  roleTool: '#fb923c',
  roleToolMuted: 'rgba(251, 146, 60, 0.2)',

  // Tag colors
  tagDotrunner: '#38bdf8',
  tagDotrunnerBg: 'rgba(56, 189, 248, 0.2)',
  tagClaudeCode: '#f43f5e',
  tagClaudeCodeBg: 'rgba(244, 63, 94, 0.2)',
  tagCodex: '#fb923c',
  tagCodexBg: 'rgba(251, 146, 60, 0.2)',
  tagGen: '#34d399',
  tagGenBg: 'rgba(52, 211, 153, 0.2)',
  tagTest: '#fbbf24',
  tagTestBg: 'rgba(251, 191, 36, 0.2)',
  tagDefault: '#a1a1aa',
  tagDefaultBg: 'rgba(113, 113, 122, 0.2)',

  // State colors
  success: '#22c55e',
  successMuted: 'rgba(34, 197, 94, 0.2)',
  error: '#ef4444',
  errorMuted: 'rgba(239, 68, 68, 0.2)',
  warning: '#fbbf24',
  warningMuted: 'rgba(251, 191, 36, 0.2)',
  info: '#38bdf8',
  infoMuted: 'rgba(56, 189, 248, 0.2)',

  // Scrollbar
  scrollbarThumb: '#52525b',
  scrollbarThumbHover: '#71717a',

  // Selection
  selection: 'rgba(244, 63, 94, 0.3)',

  // Kbd element
  kbdBg: '#1e1a20',
  kbdBorder: '#242028',
  kbdText: '#d4d4d8',
};

/**
 * Obsidian theme - white accent on zinc background
 * Minimal, content-focused, relies on brightness contrast
 */
const obsidianColors: ThemeColors = {
  // Core backgrounds (pure zinc)
  bg: '#09090b',           // zinc-950
  bgSecondary: '#18181b',  // zinc-900
  bgTertiary: '#27272a',   // zinc-800
  bgHover: '#3f3f46',      // zinc-700
  card: 'rgba(24, 24, 27, 0.8)',

  // Text colors (white/zinc)
  text: '#fafafa',         // zinc-50
  textSecondary: '#e4e4e7', // zinc-200
  textMuted: '#a1a1aa',    // zinc-400
  textDim: '#71717a',      // zinc-500
  textFaint: '#52525b',    // zinc-600

  // Accent colors (white/near-white)
  accent: '#fafafa',
  accentDim: '#e4e4e7',
  accentMuted: 'rgba(250, 250, 250, 0.15)',

  // Border colors (subtle but visible)
  border: '#242426',      // visible but not harsh
  borderDim: '#1e1e20',   // subtle
  borderFaint: '#18181a', // very dim

  // Live/activity indicators
  liveGreen: '#22c55e',
  liveGlow: 'rgba(34, 197, 94, 0.4)',
  liveGlowStrong: 'rgba(34, 197, 94, 0.6)',
  streamingAccent: 'rgba(250, 250, 250, 0.8)',
  activityFlash: 'rgba(250, 250, 250, 0.1)',

  // Warning
  warningYellow: '#fbbf24',

  // Gauge colors
  gaugeOk: '#22c55e',
  gaugeWarn: '#fbbf24',
  gaugeHot: '#f97316',
  gaugeCritical: '#ef4444',

  // Role colors
  roleUser: '#60a5fa',
  roleUserMuted: 'rgba(96, 165, 250, 0.2)',
  roleAssistant: '#fafafa',
  roleAssistantMuted: 'rgba(250, 250, 250, 0.15)',
  roleSystem: '#71717a',
  roleSystemMuted: 'rgba(113, 113, 122, 0.2)',
  roleTool: '#a1a1aa',
  roleToolMuted: 'rgba(161, 161, 170, 0.2)',

  // Tag colors
  tagDotrunner: '#60a5fa',
  tagDotrunnerBg: 'rgba(96, 165, 250, 0.2)',
  tagClaudeCode: '#fafafa',
  tagClaudeCodeBg: 'rgba(250, 250, 250, 0.15)',
  tagCodex: '#4ade80',
  tagCodexBg: 'rgba(74, 222, 128, 0.15)',
  tagGen: '#a1a1aa',
  tagGenBg: 'rgba(161, 161, 170, 0.15)',
  tagTest: '#71717a',
  tagTestBg: 'rgba(113, 113, 122, 0.15)',
  tagDefault: '#71717a',
  tagDefaultBg: 'rgba(113, 113, 122, 0.15)',

  // State colors
  success: '#22c55e',
  successMuted: 'rgba(34, 197, 94, 0.2)',
  error: '#ef4444',
  errorMuted: 'rgba(239, 68, 68, 0.2)',
  warning: '#fbbf24',
  warningMuted: 'rgba(251, 191, 36, 0.2)',
  info: '#60a5fa',
  infoMuted: 'rgba(96, 165, 250, 0.2)',

  // Scrollbar
  scrollbarThumb: '#3f3f46',
  scrollbarThumbHover: '#52525b',

  // Selection
  selection: 'rgba(250, 250, 250, 0.2)',

  // Kbd element
  kbdBg: '#1e1e20',
  kbdBorder: '#242426',
  kbdText: '#d4d4d8',
};

/**
 * SDM Brand theme - teal primary on navy background
 * Based on strongDM brand guidelines
 */
const sdmBrandColors: ThemeColors = {
  // Core backgrounds (Navy palette - dark end)
  bg: '#00101b',           // navy-950 (darkest)
  bgSecondary: '#081825',  // navy-900
  bgTertiary: '#102535',   // navy-800
  bgHover: '#1a3548',      // navy-700
  card: 'rgba(8, 24, 37, 0.8)', // navy-900 with alpha

  // Text colors (light with slight cool tint)
  text: '#f0f6fa',         // navy-25 (lightest)
  textSecondary: '#d3e2ec', // lighter navy
  textMuted: '#a4bacb',    // mid navy-light
  textDim: '#7096b1',      // navy-500
  textFaint: '#4a7090',    // navy-600

  // Accent colors (Teal - primary brand, hue ~193)
  accent: '#22a6b8',       // Teal primary
  accentDim: '#1a8a99',    // Darker teal
  accentMuted: 'rgba(34, 166, 184, 0.2)',

  // Border colors (subtle but visible)
  border: '#122838',      // visible but not harsh
  borderDim: '#0c2030',   // subtle
  borderFaint: '#081828', // very dim

  // Live/activity indicators (green for live)
  liveGreen: '#28a745',    // green-600
  liveGlow: 'rgba(40, 167, 69, 0.4)',
  liveGlowStrong: 'rgba(40, 167, 69, 0.6)',
  streamingAccent: 'rgba(34, 166, 184, 0.8)',
  activityFlash: 'rgba(34, 166, 184, 0.15)',

  // Warning (yellow, hue ~45)
  warningYellow: '#d4a017', // yellow-600

  // Gauge colors (semantic mapping)
  gaugeOk: '#28a745',      // green-600 (positive)
  gaugeWarn: '#d4a017',    // yellow-600 (warn)
  gaugeHot: '#fd7e14',     // orange (accent)
  gaugeCritical: '#dc3545', // red-600 (negative)

  // Role colors
  roleUser: '#7096b1',     // navy-500 (secondary brand)
  roleUserMuted: 'rgba(112, 150, 177, 0.2)',
  roleAssistant: '#22a6b8', // teal (primary brand)
  roleAssistantMuted: 'rgba(34, 166, 184, 0.2)',
  roleSystem: '#7d868e',   // gray-500
  roleSystemMuted: 'rgba(125, 134, 142, 0.2)',
  roleTool: '#fd7e14',     // orange (accent, hue 35)
  roleToolMuted: 'rgba(253, 126, 20, 0.2)',

  // Tag colors
  tagDotrunner: '#7096b1', // navy-500
  tagDotrunnerBg: 'rgba(112, 150, 177, 0.2)',
  tagClaudeCode: '#22a6b8', // teal
  tagClaudeCodeBg: 'rgba(34, 166, 184, 0.2)',
  tagCodex: '#9ad04b',      // green-400
  tagCodexBg: 'rgba(154, 208, 75, 0.2)',
  tagGen: '#28a745',       // green-600
  tagGenBg: 'rgba(40, 167, 69, 0.2)',
  tagTest: '#d4a017',      // yellow-600
  tagTestBg: 'rgba(212, 160, 23, 0.2)',
  tagDefault: '#a4acb3',   // gray-400
  tagDefaultBg: 'rgba(125, 134, 142, 0.2)',

  // State colors (semantic mapping from spec)
  success: '#28a745',      // green-600 (positive initial)
  successMuted: 'rgba(40, 167, 69, 0.2)',
  error: '#dc3545',        // red-600 (negative initial)
  errorMuted: 'rgba(220, 53, 69, 0.2)',
  warning: '#d4a017',      // yellow-600 (warn initial)
  warningMuted: 'rgba(212, 160, 23, 0.2)',
  info: '#4a90d9',         // blue-600 (info, hue ~218)
  infoMuted: 'rgba(74, 144, 217, 0.2)',

  // Scrollbar (navy tinted)
  scrollbarThumb: '#1a3548',  // navy-700
  scrollbarThumbHover: '#2a4560', // navy-600

  // Selection
  selection: 'rgba(34, 166, 184, 0.3)',

  // Kbd element (navy tinted)
  kbdBg: '#0c2030',        // matches borderDim
  kbdBorder: '#122838',    // matches border
  kbdText: '#d3e2ec',      // light navy
};

export const themes: Theme[] = [
  {
    id: 'sdm-brand',
    name: 'Default',
    description: 'Teal and navy on cool gray',
    colors: sdmBrandColors,
  },
  {
    id: 'original',
    name: 'Trope',
    description: 'Purple accent on blue-black background',
    colors: originalColors,
  },
  {
    id: 'ember',
    name: 'Ember',
    description: 'Warm amber on dark background',
    colors: emberColors,
  },
  {
    id: 'terminal',
    name: 'Terminal',
    description: 'Cyan accent on true black',
    colors: terminalColors,
  },
  {
    id: 'forest',
    name: 'Forest',
    description: 'Emerald accent on deep green',
    colors: forestColors,
  },
  {
    id: 'coral',
    name: 'Coral',
    description: 'Rose accent on warm gray',
    colors: coralColors,
  },
  {
    id: 'obsidian',
    name: 'Obsidian',
    description: 'Minimal white on zinc',
    colors: obsidianColors,
  },
];

export const themesById: Record<string, Theme> = Object.fromEntries(
  themes.map((theme) => [theme.id, theme])
);

export const defaultTheme = themes[0];
