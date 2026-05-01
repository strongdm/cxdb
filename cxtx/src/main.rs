use clap::Parser;

fn main() {
    // Build the Tokio runtime explicitly so we can:
    //  1. Hand its handle to `cxdb_otel::init` for background exporter tasks.
    //  2. Drop the `OtelGuard` before the runtime so shutdown's `block_on` can
    //     complete on a still-live runtime.
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("cxtx: failed to build tokio runtime: {err:#}");
            std::process::exit(1);
        }
    };

    let otel_cfg = cxdb_otel::OtelConfig::from_env();
    let handle = rt.handle().clone();
    let otel_guard = match cxdb_otel::init(&otel_cfg, &handle) {
        Ok(guard) => guard,
        Err(err) => {
            eprintln!("cxtx: otel init failed: {err}");
            std::process::exit(1);
        }
    };

    let cli = cxtx::cli::Cli::parse();
    let exit_code = rt.block_on(async {
        match cxtx::run(cli).await {
            Ok(code) => code,
            Err(err) => {
                eprintln!("cxtx: {err:#}");
                1
            }
        }
    });

    // Drop guard first (flushes), then runtime via scope end. Keep the
    // explicit drop for clarity.
    drop(otel_guard);
    drop(rt);
    std::process::exit(exit_code);
}
