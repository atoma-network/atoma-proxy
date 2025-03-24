use std::path::Path;

fn main() {
    let migrations_path = Path::new("src/migrations");

    if migrations_path.exists() {
        println!("cargo:rerun-if-changed=src/migrations");
    }
}
