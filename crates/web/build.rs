use std::{env, fs, path::PathBuf};

use not_news_store::LegacyGraphReader;

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let database = manifest_dir.join("../../backend/data/graph.sqlite");
    println!("cargo:rerun-if-changed={}", database.display());

    let graph = LegacyGraphReader::new(&database)
        .load()
        .unwrap_or_else(|error| panic!("failed to load {}: {error}", database.display()));
    assert_eq!(
        graph.events.len(),
        71,
        "the browser performance corpus must contain the same 71 events as the native baseline"
    );
    let encoded = serde_json::to_vec(&graph).expect("the validated graph snapshot must serialize");
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("build output dir")).join("graph.json");
    fs::write(&output, encoded)
        .unwrap_or_else(|error| panic!("failed to write {}: {error}", output.display()));
}
