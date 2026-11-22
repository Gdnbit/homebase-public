use homebase_control_tests::model_capability_io::reliability::{self,Failure,Kind};

#[tokio::test]
async fn closed_loopback_port_is_a_bounded_pre_submission_failure_without_private_data(){
    let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let address=listener.local_addr().unwrap();drop(listener);
    let client=reqwest::Client::builder().no_proxy().build().unwrap();
    let error=reliability::send(client.post(format!("http://{address}/?key=synthetic-private-value")).body("synthetic-private-body"),true).await.err().unwrap();
    let failure=Failure::decode(&error).unwrap();assert_eq!(failure.kind,Kind::Connect);assert!(failure.safe_to_resume());assert_eq!(failure.attempts,3);assert_eq!(failure.previous.len(),2);assert!(!error.contains("synthetic-private"));assert!(!error.contains(&address.to_string()));
}
