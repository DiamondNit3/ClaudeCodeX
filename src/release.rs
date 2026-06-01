use anyhow::Result;

pub fn print_release_check() -> Result<()> {
    println!("release target: {}", std::env::consts::OS);
    println!("arch: {}", std::env::consts::ARCH);
    println!("binary: ccx");
    println!("required checks:");
    println!("  cargo fmt --all -- --check");
    println!("  cargo test");
    println!("  cargo build --release");
    println!("  cargo build --features tui");
    Ok(())
}
