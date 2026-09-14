use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    raemote::daemon::run().await
}
