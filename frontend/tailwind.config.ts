import type { Config } from 'tailwindcss';

const config: Config = {
  content: [
    './pages/**/*.{js,ts,jsx,tsx,mdx}',
    './components/**/*.{js,ts,jsx,tsx,mdx}',
    './app/**/*.{js,ts,jsx,tsx,mdx}',
  ],
  theme: {
    extend: {
      borderColor: {
        DEFAULT: 'var(--theme-border)',
      },
      colors: {
        slate: {
          850: '#172033',
          925: '#0d1424',
          950: '#080d16',
        },
        theme: {
          // Core backgrounds
          bg: 'var(--theme-bg)',
          'bg-secondary': 'var(--theme-bg-secondary)',
          'bg-tertiary': 'var(--theme-bg-tertiary)',
          'bg-hover': 'var(--theme-bg-hover)',
          card: 'var(--theme-card)',

          // Text colors
          text: 'var(--theme-text)',
          'text-secondary': 'var(--theme-text-secondary)',
          'text-muted': 'var(--theme-text-muted)',
          'text-dim': 'var(--theme-text-dim)',
          'text-faint': 'var(--theme-text-faint)',

          // Accent colors
          accent: 'var(--theme-accent)',
          'accent-dim': 'var(--theme-accent-dim)',
          'accent-muted': 'var(--theme-accent-muted)',

          // Border colors
          border: 'var(--theme-border)',
          'border-dim': 'var(--theme-border-dim)',
          'border-faint': 'var(--theme-border-faint)',

          // Live/activity indicators
          'live-green': 'var(--theme-live-green)',
          'live-glow': 'var(--theme-live-glow)',
          'live-glow-strong': 'var(--theme-live-glow-strong)',
          'streaming-accent': 'var(--theme-streaming-accent)',
          'activity-flash': 'var(--theme-activity-flash)',

          // Warning
          'warning-yellow': 'var(--theme-warning-yellow)',

          // Gauge colors
          'gauge-ok': 'var(--theme-gauge-ok)',
          'gauge-warn': 'var(--theme-gauge-warn)',
          'gauge-hot': 'var(--theme-gauge-hot)',
          'gauge-critical': 'var(--theme-gauge-critical)',

          // Role colors
          'role-user': 'var(--theme-role-user)',
          'role-user-muted': 'var(--theme-role-user-muted)',
          'role-assistant': 'var(--theme-role-assistant)',
          'role-assistant-muted': 'var(--theme-role-assistant-muted)',
          'role-system': 'var(--theme-role-system)',
          'role-system-muted': 'var(--theme-role-system-muted)',
          'role-tool': 'var(--theme-role-tool)',
          'role-tool-muted': 'var(--theme-role-tool-muted)',

          // Tag colors
          'tag-dotrunner': 'var(--theme-tag-dotrunner)',
          'tag-dotrunner-bg': 'var(--theme-tag-dotrunner-bg)',
          'tag-claude-code': 'var(--theme-tag-claude-code)',
          'tag-claude-code-bg': 'var(--theme-tag-claude-code-bg)',
          'tag-codex': 'var(--theme-tag-codex)',
          'tag-codex-bg': 'var(--theme-tag-codex-bg)',
          'tag-gen': 'var(--theme-tag-gen)',
          'tag-gen-bg': 'var(--theme-tag-gen-bg)',
          'tag-test': 'var(--theme-tag-test)',
          'tag-test-bg': 'var(--theme-tag-test-bg)',
          'tag-default': 'var(--theme-tag-default)',
          'tag-default-bg': 'var(--theme-tag-default-bg)',

          // State colors
          success: 'var(--theme-success)',
          'success-muted': 'var(--theme-success-muted)',
          error: 'var(--theme-error)',
          'error-muted': 'var(--theme-error-muted)',
          warning: 'var(--theme-warning)',
          'warning-muted': 'var(--theme-warning-muted)',
          info: 'var(--theme-info)',
          'info-muted': 'var(--theme-info-muted)',

          // Scrollbar
          'scrollbar-thumb': 'var(--theme-scrollbar-thumb)',
          'scrollbar-thumb-hover': 'var(--theme-scrollbar-thumb-hover)',

          // Selection
          selection: 'var(--theme-selection)',

          // Kbd element
          'kbd-bg': 'var(--theme-kbd-bg)',
          'kbd-border': 'var(--theme-kbd-border)',
          'kbd-text': 'var(--theme-kbd-text)',
        },
      },
      fontFamily: {
        sans: ['Inter', 'system-ui', '-apple-system', 'sans-serif'],
        mono: ['JetBrains Mono', 'Consolas', 'monospace'],
      },
      animation: {
        'pulse-slow': 'pulse 3s cubic-bezier(0.4, 0, 0.6, 1) infinite',
        'fade-in': 'fadeIn 0.2s ease-out',
        // Live observer animations
        'breathe': 'breathe 3s ease-in-out infinite',
        'slide-in-left': 'slideInLeft 0.3s ease-out forwards',
        'slide-up': 'slideUp 0.4s cubic-bezier(0.34, 1.56, 0.64, 1) forwards',
        'highlight-fade': 'highlightFade 3s ease-out forwards',
        'pulse-once': 'pulseOnce 1s ease-out forwards',
        'activity-flash': 'activityFlash 1s ease-out forwards',
        'cursor-blink': 'cursorBlink 1s step-end infinite',
        'spin-slow': 'spin 2s linear infinite',
        'progress-indeterminate': 'progressIndeterminate 1.5s ease-in-out infinite',
        'glow-pulse': 'glowPulse 2s ease-in-out infinite',
      },
      keyframes: {
        fadeIn: {
          '0%': { opacity: '0', transform: 'translateY(-4px)' },
          '100%': { opacity: '1', transform: 'translateY(0)' },
        },
        breathe: {
          '0%, 100%': { opacity: '0.7', transform: 'scale(1)' },
          '50%': { opacity: '1', transform: 'scale(1.15)' },
        },
        slideInLeft: {
          '0%': { opacity: '0', transform: 'translateX(-16px)' },
          '100%': { opacity: '1', transform: 'translateX(0)' },
        },
        slideUp: {
          '0%': { opacity: '0', transform: 'translateY(20px) scale(0.98)' },
          '60%': { opacity: '1', transform: 'translateY(-4px) scale(1.01)' },
          '100%': { opacity: '1', transform: 'translateY(0) scale(1)' },
        },
        highlightFade: {
          '0%': {
            boxShadow: 'inset 3px 0 0 #a855f7, 0 0 20px -5px #a855f7',
            background: 'rgba(168, 85, 247, 0.08)',
          },
          '100%': {
            boxShadow: 'inset 3px 0 0 transparent, 0 0 0 0 transparent',
            background: 'transparent',
          },
        },
        pulseOnce: {
          '0%': { boxShadow: '0 0 0 0 rgba(168, 85, 247, 0.4)' },
          '70%': { boxShadow: '0 0 0 10px rgba(168, 85, 247, 0)' },
          '100%': { boxShadow: '0 0 0 0 rgba(168, 85, 247, 0)' },
        },
        activityFlash: {
          '0%': { background: 'transparent' },
          '20%': { background: 'rgba(168, 85, 247, 0.15)' },
          '100%': { background: 'transparent' },
        },
        cursorBlink: {
          '0%, 50%': { opacity: '1' },
          '51%, 100%': { opacity: '0' },
        },
        progressIndeterminate: {
          '0%': { transform: 'translateX(-100%)' },
          '100%': { transform: 'translateX(300%)' },
        },
        glowPulse: {
          '0%, 100%': { boxShadow: '0 0 0 1px rgba(34, 197, 94, 0.3), 0 0 20px -10px rgba(34, 197, 94, 0.2)' },
          '50%': { boxShadow: '0 0 0 1px rgba(34, 197, 94, 0.5), 0 0 30px -5px rgba(34, 197, 94, 0.3)' },
        },
      },
    },
  },
  plugins: [],
};

export default config;
