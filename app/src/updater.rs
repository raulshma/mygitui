//! In-app update installation: pick the release artifact for this
//! platform, download it, verify it against the release's `.sha256`
//! sidecar (dist emits one per artifact; our own AppImage/app-bundle jobs
//! emit the same format), and apply it per OS — AppImage self-replace,
//! silent MSI install, or `.app` bundle swap — then restart into the new
//! build.
//!
//! Off the repo op-queue by design (like forge/AI): pure HTTP + filesystem
//! work with nothing to do with git state. [`crate::update_check`] owns
//! the "is there a newer release?" question; this module owns "get it
//! onto disk and running".

use sha2::Digest;

/// GitHub repo slug releases are fetched from (shared with
/// [`crate::update_check`]).
const RELEASES_REPO: &str = "raulshma/mygitui";

/// One downloadable release asset (name + browser download URL + size).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// What installable package this build's platform consumes, and how it is
/// applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Package {
    /// Linux: replace the running `.AppImage` file in place, restart.
    AppImage,
    /// Windows: run the MSI silently detached; the app exits so msiexec
    /// can replace the binary.
    Msi,
    /// macOS: extract the `.app.tar.gz` over the running bundle, restart.
    AppBundle,
}

/// The package kind for the platform this build is running on.
pub fn platform_package() -> Result<Package, String> {
    match std::env::consts::OS {
        "linux" => Ok(Package::AppImage),
        "windows" => Ok(Package::Msi),
        "macos" => Ok(Package::AppBundle),
        other => Err(format!("no update package for platform `{other}`")),
    }
}

/// Suffix (including platform triple) the artifact for `pkg` ends with.
/// `std::env::consts::ARCH` tokens (`x86_64`, `aarch64`) match the ones in
/// our release artifact names verbatim.
fn artifact_suffix(pkg: Package) -> String {
    let arch = std::env::consts::ARCH;
    match pkg {
        Package::AppImage => format!("{arch}-unknown-linux-gnu.AppImage"),
        // Only an intel Windows build is shipped; arm Windows has no
        // artifact and pick_asset will report that as "no artifact".
        Package::Msi => "x86_64-pc-windows-msvc.msi".to_string(),
        Package::AppBundle => format!("{arch}-apple-darwin.app.tar.gz"),
    }
}

/// The release asset to install for `pkg` (the `.sha256` sidecar and any
/// unrelated artifacts are ignored).
pub fn pick_asset(assets: &[Asset], pkg: Package) -> Option<&Asset> {
    let suffix = artifact_suffix(pkg);
    assets
        .iter()
        .find(|asset| asset.name.ends_with(&suffix) && !asset.name.ends_with(".sha256"))
}

/// The hex digest from a `.sha256` sidecar body. dist's sidecars are
/// sha256sum-style (`"<hex>  <name>"`); anything else falls back to
/// whitespace tokenizing, so a bare-hex file also parses.
fn checksum_hex(sidecar_body: &str) -> &str {
    sidecar_body.split_whitespace().next().unwrap_or_default()
}

/// Verifies `payload` against the expected lowercase hex SHA-256.
pub fn verify(payload: &[u8], expected_hex: &str) -> Result<(), String> {
    let mut hasher = sha2::Sha256::new();
    hasher.update(payload);
    let actual = format!("{:x}", hasher.finalize());
    if actual == expected_hex.to_ascii_lowercase() {
        Ok(())
    } else {
        Err(format!(
            "downloaded artifact failed the SHA-256 check (expected {expected_hex}, got {actual})"
        ))
    }
}

/// Lists the published assets of one release (`tag` including its `v`).
pub async fn release_assets(tag: &str) -> Result<Vec<Asset>, String> {
    release_assets_at(
        &format!("https://api.github.com/repos/{RELEASES_REPO}"),
        tag,
    )
    .await
}

/// [`release_assets`] against an explicit API base (tests point this at a
/// mock server).
async fn release_assets_at(base: &str, tag: &str) -> Result<Vec<Asset>, String> {
    #[derive(serde::Deserialize)]
    struct WireRelease {
        assets: Vec<WireAsset>,
    }
    #[derive(serde::Deserialize)]
    struct WireAsset {
        name: String,
        browser_download_url: String,
        #[serde(default)]
        size: u64,
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("mygitui")
        .build()
        .map_err(|e| format!("client build failed: {e}"))?;
    let release: WireRelease = client
        .get(format!("{base}/releases/tags/{tag}"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("release lookup failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("release lookup failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("release parse failed: {e}"))?;
    Ok(release
        .assets
        .into_iter()
        .map(|asset| Asset {
            name: asset.name,
            url: asset.browser_download_url,
            size: asset.size,
        })
        .collect())
}

/// Downloads one release asset into memory (installers are single-digit MB;
/// the archives tens of MB at most).
async fn download(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .user_agent("mygitui")
        .build()
        .map_err(|e| format!("client build failed: {e}"))?;
    let bytes = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("download failed: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    Ok(bytes.to_vec())
}

/// Fetches the `.sha256` sidecar for `asset_name` from the same release
/// and returns the expected hex digest.
async fn sidecar_hex(assets: &[Asset], asset_name: &str) -> Result<String, String> {
    let sidecar_name = format!("{asset_name}.sha256");
    let sidecar = assets
        .iter()
        .find(|asset| asset.name == sidecar_name)
        .ok_or_else(|| format!("release has no {sidecar_name} sidecar"))?;
    let body = download(&sidecar.url).await?;
    let text = String::from_utf8(body).map_err(|_| "checksum sidecar is not UTF-8".to_string())?;
    let hex = checksum_hex(&text);
    if hex.is_empty() {
        return Err(format!("{sidecar_name} sidecar is empty"));
    }
    Ok(hex.to_string())
}

/// One-shot: fetch the release's assets, pick, download, verify, apply.
/// On success the process has already exited (restart/installer launch
/// happens inside [`apply`]), so the `Ok` arm is effectively unreachable.
pub async fn install(tag: &str) -> Result<String, String> {
    let package = platform_package()?;
    let assets = release_assets(tag).await?;
    let asset = pick_asset(&assets, package)
        .ok_or_else(|| format!("release {tag} has no artifact for this platform"))?
        .clone();
    let expected = sidecar_hex(&assets, &asset.name).await?;
    let payload = download(&asset.url).await?;
    verify(&payload, &expected)?;
    apply(package, &payload)?;
    Ok(asset.name)
}

/// Applies a verified payload for `package` and exits the process on
/// success (the restart/installer-launch happens here), so it returns only
/// on failure.
pub fn apply(package: Package, payload: &[u8]) -> Result<(), String> {
    match package {
        Package::AppImage => {
            let exe = std::env::current_exe().map_err(|e| format!("cannot locate myself: {e}"))?;
            replace_appimage(&exe, payload)?;
            restart_and_exit(&exe)
        }
        Package::Msi => {
            let dir = std::env::temp_dir().join("mygitui-update");
            std::fs::create_dir_all(&dir).map_err(|e| format!("temp dir failed: {e}"))?;
            let msi = dir.join("mygitui-update.msi");
            std::fs::write(&msi, payload).map_err(|e| format!("write msi failed: {e}"))?;
            // Detached: it must outlive us. The app quits right after so
            // msiexec can swap the binary; the user re-launches normally.
            std::process::Command::new("msiexec")
                .args(["/i"])
                .arg(&msi)
                .args(["/qn", "/norestart"])
                .spawn()
                .map_err(|e| format!("launching msiexec failed: {e}"))?;
            std::thread::sleep(std::time::Duration::from_millis(500));
            std::process::exit(0)
        }
        #[cfg(target_os = "macos")]
        Package::AppBundle => {
            let exe = std::env::current_exe().map_err(|e| format!("cannot locate myself: {e}"))?;
            let bundle = bundle_root(&exe).ok_or_else(|| {
                "not running from a .app bundle; install the downloaded bundle manually".to_string()
            })?;
            let new_bundle = extract_app_bundle(payload)?;
            // Rename dance: keep the old bundle until the new one is in
            // place, then drop it.
            let old = bundle.with_extension("app.old");
            let _ = std::fs::remove_dir_all(&old);
            std::fs::rename(&bundle, &old)
                .map_err(|e| format!("backing up old bundle failed: {e}"))?;
            if let Err(err) = std::fs::rename(&new_bundle, &bundle) {
                let _ = std::fs::rename(&old, &bundle);
                return Err(format!("installing new bundle failed: {err}"));
            }
            let _ = std::fs::remove_dir_all(&old);
            restart_and_exit(&bundle.join("Contents/MacOS/mygitui"))
        }
        // Only macOS ships the tar crate; no other platform ever selects
        // the bundle package (platform_package gates it).
        #[cfg(not(target_os = "macos"))]
        Package::AppBundle => Err("app bundles are only installable on macOS".to_string()),
    }
}

/// Overwrites the AppImage at `exe_path` with `payload` (atomic-ish:
/// staged in the same directory, chmod'ed, renamed over the running
/// file). Exposed for tests.
fn replace_appimage(exe_path: &std::path::Path, payload: &[u8]) -> Result<(), String> {
    if !exe_path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with(".AppImage"))
    {
        return Err(
            "not running from an AppImage; download it from the release page instead".to_string(),
        );
    }
    let dir = exe_path
        .parent()
        .ok_or_else(|| "cannot resolve the AppImage directory".to_string())?;
    let staged = dir.join(format!(".mygitui-update-{}.AppImage", std::process::id()));
    std::fs::write(&staged, payload).map_err(|e| format!("staging update failed: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod update failed: {e}"))?;
    }
    std::fs::rename(&staged, exe_path).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!("replacing the AppImage failed: {e}")
    })?;
    Ok(())
}

/// The `.app` bundle root for an executable inside one
/// (`…/X.app/Contents/MacOS/exe` → `…/X.app`). Only the macOS arm calls it
/// in production, but it is pure path logic so tests keep it compiled
/// everywhere.
#[cfg(any(target_os = "macos", test))]
fn bundle_root(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    // ancestors() yields the path itself first: exe → MacOS → Contents →
    // X.app.
    let bundle = exe.ancestors().nth(3)?;
    if bundle.extension().is_some_and(|ext| ext == "app") {
        Some(bundle.to_path_buf())
    } else {
        None
    }
}

/// Extracts an `.app.tar.gz` payload into a fresh temp dir and returns the
/// bundle path inside it.
#[cfg(target_os = "macos")]
fn extract_app_bundle(payload: &[u8]) -> Result<std::path::PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("mygitui-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("temp dir failed: {e}"))?;
    let gz = flate2::read::GzDecoder::new(payload);
    let mut archive = tar::Archive::new(gz);
    archive
        .unpack(&dir)
        .map_err(|e| format!("extracting app bundle failed: {e}"))?;
    // The tarball contains exactly one top-level `*.app` directory.
    let mut entries = std::fs::read_dir(&dir).map_err(|e| format!("temp dir read failed: {e}"))?;
    let bundle = entries
        .find_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension().is_some_and(|ext| ext == "app")).then_some(path)
        })
        .ok_or_else(|| "app bundle tarball has no .app directory".to_string())?;
    if !bundle.join("Contents/MacOS/mygitui").exists() {
        return Err("app bundle is missing Contents/MacOS/mygitui".to_string());
    }
    Ok(bundle)
}

/// Spawns `exe` detached with this process's arguments and exits.
fn restart_and_exit(exe: &std::path::Path) -> ! {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let _ = std::process::Command::new(exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.to_string(),
            url: format!("https://example.com/{name}"),
            size: 0,
        }
    }

    fn release_assets_fixture() -> Vec<Asset> {
        vec![
            asset("mygitui-aarch64-apple-darwin.tar.xz"),
            asset("mygitui-aarch64-apple-darwin.app.tar.gz"),
            asset("mygitui-aarch64-apple-darwin.app.tar.gz.sha256"),
            asset("mygitui-x86_64-apple-darwin.tar.xz"),
            asset("mygitui-x86_64-apple-darwin.app.tar.gz"),
            asset("mygitui-x86_64-apple-darwin.app.tar.gz.sha256"),
            asset("mygitui-x86_64-pc-windows-msvc.msi"),
            asset("mygitui-x86_64-pc-windows-msvc.msi.sha256"),
            asset("mygitui-x86_64-pc-windows-msvc.zip"),
            asset("mygitui-x86_64-unknown-linux-gnu.tar.xz"),
            asset("mygitui-aarch64-unknown-linux-gnu.AppImage"),
            asset("mygitui-aarch64-unknown-linux-gnu.AppImage.sha256"),
            asset("mygitui-x86_64-unknown-linux-gnu.AppImage"),
            asset("mygitui-x86_64-unknown-linux-gnu.AppImage.sha256"),
            asset("mygitui-installer.sh"),
            asset("mygitui-installer.ps1"),
            asset("source.tar.gz"),
        ]
    }

    #[test]
    fn pick_matches_the_platform_artifact_only() {
        let assets = release_assets_fixture();
        let picked = |pkg| pick_asset(&assets, pkg).map(|asset| asset.name.as_str());
        // AppImages and app bundles are arch-specific; this build's arch
        // decides which one is picked (both exist in the fixture).
        let appimage = format!(
            "mygitui-{}-unknown-linux-gnu.AppImage",
            std::env::consts::ARCH
        );
        assert_eq!(picked(Package::AppImage), Some(appimage.as_str()));
        assert_eq!(
            picked(Package::Msi),
            Some("mygitui-x86_64-pc-windows-msvc.msi")
        );
        let picked_bundle = pick_asset(&assets, Package::AppBundle).unwrap();
        assert!(picked_bundle.name.ends_with(".app.tar.gz"));
        assert!(!picked_bundle.name.ends_with(".sha256"));
    }

    #[test]
    fn pick_reports_missing_artifacts() {
        // Only an intel Windows MSI is shipped; an arm64-only release has
        // no MSI for a non-x86_64 host and pick_asset must say so.
        let assets = vec![asset("mygitui-aarch64-apple-darwin.app.tar.gz")];
        assert_eq!(pick_asset(&assets, Package::Msi), None);
        assert_eq!(pick_asset(&[], Package::AppImage), None);
    }

    #[test]
    fn sidecar_hex_parses_both_shapes() {
        assert_eq!(checksum_hex("abc123  mygitui-x.AppImage\n"), "abc123");
        assert_eq!(checksum_hex("abc123\n"), "abc123");
        assert_eq!(checksum_hex(""), "");
    }

    #[test]
    fn verify_rejects_corrupted_payloads() {
        let abc = b"abc";
        let abc_hex = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(verify(abc, abc_hex), Ok(()));
        assert_eq!(verify(abc, &abc_hex.to_ascii_uppercase()), Ok(()));
        assert!(verify(b"abd", abc_hex).is_err());
        assert!(verify(abc, "deadbeef").is_err());
    }

    #[test]
    fn bundle_root_walks_out_of_contents() {
        use std::path::Path;
        assert_eq!(
            bundle_root(Path::new(
                "/Applications/MyGitUI.app/Contents/MacOS/mygitui"
            )),
            Some(std::path::PathBuf::from("/Applications/MyGitUI.app"))
        );
        // Not inside a bundle: no match.
        assert_eq!(bundle_root(Path::new("/usr/local/bin/mygitui")), None);
        assert_eq!(
            bundle_root(Path::new(
                "target/release/MyGitUI.app/Contents/MacOS/mygitui"
            )),
            Some(std::path::PathBuf::from("target/release/MyGitUI.app"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn appimage_replace_swaps_the_file_in_place() {
        let dir = std::env::temp_dir().join(format!("mygitui-updater-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("mygitui-x86_64-unknown-linux-gnu.AppImage");
        std::fs::write(&exe, b"old payload").unwrap();

        replace_appimage(&exe, b"new payload").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new payload");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&exe).unwrap().permissions().mode() & 0o111,
            0o111,
            "replaced AppImage must stay executable"
        );
        // No staging leftovers.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

        // A non-AppImage target refuses to be overwritten.
        let plain = dir.join("mygitui");
        std::fs::write(&plain, b"old").unwrap();
        assert!(replace_appimage(&plain, b"new").is_err());
        assert_eq!(std::fs::read(&plain).unwrap(), b"old");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn release_assets_parses_github_shape() {
        // GitHub releases/tags wire shape, served by the same mock server
        // the AI provider tests use.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let body = serde_json::json!({
            "tag_name": "v0.1.13",
            "assets": [
                {"name": "mygitui-x86_64-unknown-linux-gnu.AppImage",
                 "browser_download_url": "https://example.com/a.AppImage", "size": 42},
                {"name": "mygitui-x86_64-unknown-linux-gnu.AppImage.sha256",
                 "browser_download_url": "https://example.com/a.sha256", "size": 80}
            ]
        });
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/releases/tags/v0.1.13"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&server)
            .await;
        let base = server.uri();
        let assets = release_assets_at(&base, "v0.1.13").await.unwrap();
        assert_eq!(assets.len(), 2);
        assert_eq!(assets[0].size, 42);
        assert_eq!(assets[0].name, "mygitui-x86_64-unknown-linux-gnu.AppImage");
    }
}
