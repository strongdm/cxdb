use clap::Parser;

#[tokio::main]
async fn main() {
    let cli = cxtx::cli::Cli::parse();
    match cxtx::run(cli).await {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("cxtx: {err:#}");
            std::process::exit(1);
        }
    }
}
