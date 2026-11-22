fn main() {
    println!("cargo:rerun-if-changed=../scripts/check-native-frontend.mjs");
    println!("cargo:rerun-if-env-changed=TAURI_CONFIG");
    let frontend = std::process::Command::new("node")
        .arg("../scripts/check-native-frontend.mjs")
        .status()
        .expect("Node.js is required to verify embedded frontend assets");
    assert!(
        frontend.success(),
        "HomeBase frontend assets are not embedded safely"
    );
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=src/commands");
    println!("cargo:rerun-if-changed=src/model_capabilities.rs");
    println!("cargo:rerun-if-changed=src/model_capability_io.rs");
    println!("cargo:rerun-if-changed=src/wechat.rs");
    println!("cargo:rerun-if-changed=src/wechat_crypto.rs");
    println!("cargo:rerun-if-changed=src/assistant.rs");
    println!("cargo:rerun-if-changed=src/assistant_protocol.rs");
    println!("cargo:rerun-if-changed=src/control_dispatch.generated.rs");
    println!("cargo:rerun-if-changed=../control/policy.json");
    println!("cargo:rerun-if-changed=../control/commands.json");
    println!("cargo:rerun-if-changed=../scripts/generate-control-contract.mjs");
    println!("cargo:rerun-if-changed=../scripts/control-schemas.mjs");
    println!("cargo:rerun-if-changed=../scripts/control-interface-docs.mjs");
    println!("cargo:rerun-if-changed=../control/interface-guide.json");
    println!("cargo:rerun-if-changed=../control/reference.md");
    let status = std::process::Command::new("node")
        .arg("../scripts/generate-control-contract.mjs")
        .arg("--check")
        .status()
        .expect("Node.js is required to verify the control contract");
    assert!(
        status.success(),
        "Run npm run control:generate before building HomeBase"
    );
    tauri_build::build()
}
