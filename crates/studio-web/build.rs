fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Pinned and verified: the build downloads Tailwind's standalone CLI, and
    // only for platforms whose checksum we know.
    let checksum = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => {
            "sha256:cdf646702987a743464dff4d9c60fd4480d1c1e73dd819a9a67f1078815dce9d"
        }
        ("macos", "x86_64") => {
            "sha256:7922e0953f2110c05976e3bf58f14e643d90427575e766b7d433f5f80cbee7e1"
        }
        ("linux", "x86_64") => {
            "sha256:dc61b3ac6b8c9ca874c0cc4c57b2409791a64c5540404ca5f5367360babc313a"
        }
        ("linux", "aarch64") => {
            "sha256:55fd0b241214eff3de1e8ee4f22796662f2d2e7a49bcfca7477cfd0bac398195"
        }
        (os, arch) => return Err(format!("no pinned Tailwind CLI for {os}/{arch}").into()),
    };
    topcoat::tailwind::BuildConfig::new()
        .input("src/app.css")
        .version_checksum(TAILWIND, checksum)
        .render()?;
    Ok(())
}

const TAILWIND: &str = "4.3.3";
