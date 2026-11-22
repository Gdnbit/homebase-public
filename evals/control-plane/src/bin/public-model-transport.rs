//! Explicit credential-free transport probe. No key store or model invocation.
use serde_json::json;
use std::{error::Error, time::Duration};
#[tokio::main]
async fn main() {
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).connect_timeout(Duration::from_secs(12)).timeout(Duration::from_secs(30)).build().unwrap();
    for (stage,request) in [
        ("public-models",client.get("https://openrouter.ai/api/v1/models")),
        ("unauthenticated-post",client.post("https://openrouter.ai/api/v1/chat/completions").json(&json!({}))),
    ] {
        match request.send().await {
            Ok(response)=>println!("{}",json!({"stage":stage,"http":response.status().as_u16()})),
            Err(error)=>{
                let mut causes=Vec::new();let mut current:Option<&(dyn Error+'static)>=Some(&error);
                while let Some(e)=current{causes.push(e.to_string());current=e.source();}
                println!("{}",json!({"stage":stage,"connect":error.is_connect(),"timeout":error.is_timeout(),"request":error.is_request(),"builder":error.is_builder(),"causes":causes}));
            }
        }
    }
}
