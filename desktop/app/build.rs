// The version for every Tethertone app lives in ../../VERSION.
fn main() {
    let path = "../../VERSION";
    println!("cargo:rerun-if-changed={path}");
    let version = std::fs::read_to_string(path).map(|v| v.trim().to_string()).unwrap_or_else(|_| "dev".into());
    println!("cargo:rustc-env=TETHERTONE_VERSION={version}");
}
