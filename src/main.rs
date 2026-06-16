use portfolio_rs::cli;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    cli::run().await
}
