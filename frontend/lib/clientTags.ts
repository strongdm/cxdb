export const DEFAULT_TAG_COLOR = {
  bg: 'bg-theme-tag-default-bg',
  text: 'text-theme-tag-default',
  border: 'border-theme-tag-default/30',
};

const TAG_COLORS: Record<string, { bg: string; text: string; border: string }> = {
  dotrunner: {
    bg: 'bg-theme-tag-dotrunner-bg',
    text: 'text-theme-tag-dotrunner',
    border: 'border-theme-tag-dotrunner/30',
  },
  claude: {
    bg: 'bg-theme-tag-claude-code-bg',
    text: 'text-theme-tag-claude-code',
    border: 'border-theme-tag-claude-code/30',
  },
  'claude-code': {
    bg: 'bg-theme-tag-claude-code-bg',
    text: 'text-theme-tag-claude-code',
    border: 'border-theme-tag-claude-code/30',
  },
  codex: {
    bg: 'bg-theme-tag-codex-bg',
    text: 'text-theme-tag-codex',
    border: 'border-theme-tag-codex/30',
  },
  gen: {
    bg: 'bg-theme-tag-gen-bg',
    text: 'text-theme-tag-gen',
    border: 'border-theme-tag-gen/30',
  },
  test: {
    bg: 'bg-theme-tag-test-bg',
    text: 'text-theme-tag-test',
    border: 'border-theme-tag-test/30',
  },
};

export const MOCK_CLIENT_TAGS = ['claude', 'codex', 'dotrunner', 'test-harness', 'aider'] as const;

export function getTagColor(tag: string) {
  return TAG_COLORS[tag.toLowerCase()] || DEFAULT_TAG_COLOR;
}
