use clap::{Parser, Subcommand};

use crate::provider::ProviderKind;

pub const DEFAULT_LOCAL_CXDB_URL: &str = "http://127.0.0.1:9010";

#[derive(Debug, Clone, Parser)]
#[command(
    name = "cxtx",
    about = "Wrap claude or codex, capture provider traffic, and upload canonical conversation context to CXDB",
    after_help = "Examples:\n  cxtx codex -- --model gpt-5\n  cxtx --local claude -- --print stream\n  cxtx --url http://127.0.0.1:9010 claude -- --print stream"
)]
pub struct Cli {
    #[arg(
        long,
        default_value = DEFAULT_LOCAL_CXDB_URL,
        help = "CXDB HTTP base URL used for registry publication, context creation, and turn append"
    )]
    pub url: String,

    #[arg(
        long,
        conflicts_with = "url",
        help = "Use the local CXDB HTTP endpoint at http://127.0.0.1:9010"
    )]
    pub local: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Launch the `claude` CLI through a local Anthropic-aware capture proxy.
    Claude {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Launch the `codex` CLI through a local OpenAI-aware capture proxy.
    Codex {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

impl Command {
    pub fn provider(&self) -> ProviderKind {
        match self {
            Self::Claude { .. } => ProviderKind::Claude,
            Self::Codex { .. } => ProviderKind::Codex,
        }
    }

    pub fn args(&self) -> &[String] {
        match self {
            Self::Claude { args } | Self::Codex { args } => args,
        }
    }
}

impl Cli {
    pub fn effective_url(&self) -> &str {
        if self.local {
            DEFAULT_LOCAL_CXDB_URL
        } else {
            &self.url
        }
    }

    pub fn for_tests(provider: ProviderKind, args: Vec<String>, url: &str) -> Self {
        let command = match provider {
            ProviderKind::Claude => Command::Claude { args },
            ProviderKind::Codex => Command::Codex { args },
        };
        Self {
            url: url.to_string(),
            local: false,
            command,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command, DEFAULT_LOCAL_CXDB_URL};
    use clap::Parser;

    #[test]
    fn defaults_to_local_cxdb_url() {
        let cli = Cli::parse_from(["cxtx", "codex"]);
        assert_eq!(cli.effective_url(), DEFAULT_LOCAL_CXDB_URL);
        assert!(!cli.local);
        assert!(matches!(cli.command, Command::Codex { .. }));
    }

    #[test]
    fn local_flag_switches_to_local_cxdb_url() {
        let cli = Cli::parse_from(["cxtx", "--local", "claude"]);
        assert_eq!(cli.effective_url(), DEFAULT_LOCAL_CXDB_URL);
        assert!(cli.local);
        assert!(matches!(cli.command, Command::Claude { .. }));
    }

    #[test]
    fn local_flag_conflicts_with_explicit_url() {
        let result =
            Cli::try_parse_from(["cxtx", "--local", "--url", "http://example.test", "codex"]);
        assert!(result.is_err());
    }
}
