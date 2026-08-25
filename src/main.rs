#![forbid(unsafe_code)]

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::json;
use shared_auth_client::SharedAuthClient;

#[derive(Debug, Parser)]
#[command(name = "happy-wakey", version, about)]
struct Cli {
    /// Shared Auth customer-realm base URL. Plain HTTP is rejected except by
    /// the official client for its explicit loopback development profile.
    #[arg(
        long,
        env = "HAPPY_WAKEY_SHARED_AUTH_BASE",
        default_value = "https://auth.oresoftware.dev"
    )]
    shared_auth_base: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Report the non-secret Shared Auth capability surface.
    Capabilities,
    /// Verify HAPPY_WAKEY_ACCESS_TOKEN without printing or logging it.
    Verify,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client =
        SharedAuthClient::try_new(cli.shared_auth_base).context("invalid Shared Auth authority")?;

    match cli.command {
        Command::Capabilities => {
            let capabilities = client
                .capabilities()
                .await
                .context("Shared Auth capability request failed")?;
            println!(
                "{}",
                json!({
                    "mfa_enabled": capabilities.mfa_enabled,
                    "methods": capabilities.methods,
                    "threefa_import_scheme": capabilities.threefa_import_scheme,
                    "biometric_model": capabilities.biometric_model,
                })
            );
        }
        Command::Verify => {
            let token = std::env::var("HAPPY_WAKEY_ACCESS_TOKEN")
                .context("HAPPY_WAKEY_ACCESS_TOKEN is required")?;
            if token.trim() != token || token.is_empty() || token.len() > 16_384 {
                bail!("HAPPY_WAKEY_ACCESS_TOKEN is malformed");
            }
            let valid = client
                .verify(&token)
                .await
                .context("Shared Auth verification request failed")?;
            println!("{}", json!({ "valid": valid }));
            if !valid {
                bail!("credential is not valid for the configured Shared Auth realm");
            }
        }
    }

    Ok(())
}
