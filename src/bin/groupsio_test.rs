use anyhow::Result;
use reqwest::Client;
use serde_json::Value;

#[tokio::main]
async fn main() -> Result<()> {
    let api_key =
        std::env::var("GROUPSIO_API_KEY").expect("Set GROUPSIO_API_KEY environment variable");

    let client = Client::new();

    // First, get our subscriptions to find the SKCC group_id
    println!("=== Getting subscriptions ===");
    let response = client
        .get("https://groups.io/api/v1/getsubs")
        .header("Authorization", format!("Bearer {}", api_key))
        .query(&[("limit", "100")])
        .send()
        .await?;
    println!("Status: {}", response.status());
    let body: Value = response.json().await?;
    println!("{}", serde_json::to_string_pretty(&body)?);

    // Look for SKCC in the subscriptions and extract group_id
    if let Some(data) = body.get("data").and_then(|d| d.as_array()) {
        for sub in data {
            let group_name = sub.get("group_name").and_then(|n| n.as_str()).unwrap_or("");
            if group_name.to_uppercase().contains("SKCC") {
                println!("\n=== Found SKCC group ===");
                println!("{}", serde_json::to_string_pretty(&sub)?);

                if let Some(group_id) = sub.get("group_id").and_then(|id| id.as_i64()) {
                    println!("\n=== Trying getmembers with group_id {} ===", group_id);
                    let group_id_str = group_id.to_string();
                    let response = client
                        .get("https://groups.io/api/v1/getmembers")
                        .header("Authorization", format!("Bearer {}", api_key))
                        .query(&[("group_id", group_id_str.as_str()), ("limit", "5")])
                        .send()
                        .await?;
                    println!("Status: {}", response.status());
                    let body: Value = response.json().await?;
                    println!("{}", serde_json::to_string_pretty(&body)?);
                }
            }
        }
    }

    Ok(())
}
