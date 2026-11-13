use clap::Parser;

/// Deterministic HTTP test server with fault injection.
#[derive(Parser)]
struct Args {
    /// Port on 127.0.0.1 (0 picks a free one).
    #[arg(long, default_value_t = 8080)]
    port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", args.port)).await?;
    println!(
        "faultbox listening on http://{}/file?size=N&seed=S",
        listener.local_addr()?
    );
    axum::serve(listener, faultbox::router()).await?;
    Ok(())
}
