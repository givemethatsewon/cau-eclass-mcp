#[tokio::main]
async fn main() -> anyhow::Result<()> {
    std::process::exit(eclass_core::cli::run().await?);
}
