use clap::{Parser, Subcommand};

use crate::provider::ProviderKind;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "cxtx",
    about = "Wrap claude or codex, capture provider traffic, and upload canonical conversation context to CXDB",
    after_help = "Examples:\n  cxtx codex -- --model gpt-5\n  cxtx --url http://127.0.0.1:9010 claude -- --print stream"
)]
pub struct Cli {
    #[arg(
        long,
        default_value = "http://127.0.0.1:9010",
        help = "CXDB HTTP base URL used for registry publication, context creation, and turn append"
    )]
    pub url: String,

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
    pub fn for_tests(provider: ProviderKind, args: Vec<String>, url: &str) -> Self {
        let command = match provider {
            ProviderKind::Claude => Command::Claude { args },
            ProviderKind::Codex => Command::Codex { args },
        };
        Self {
            url: url.to_string(),
            command,
        }
    }
}
