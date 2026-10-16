// Requirement: FR-396 (Release zip packaging and Start Menu shortcut)
// Test ID: TC-405 (Automated verification of packaging contracts and installer safety)

use std::fs;
use std::path::Path;

#[test]
fn install_script_meets_safety_and_path_contracts() {
    let path = Path::new("packaging/windows/install.ps1");
    let content = fs::read_to_string(path).expect("packaging/windows/install.ps1 must exist");

    // Must not use Invoke-Expression or iex for security
    assert!(
        !content.contains("Invoke-Expression"),
        "install.ps1 must never use Invoke-Expression"
    );
    assert!(
        !content.contains("iex "),
        "install.ps1 must never use iex alias"
    );

    // Must install to LOCALAPPDATA\Programs\DmuxAI
    assert!(
        content.contains("LOCALAPPDATA") && content.contains(r"Programs\DmuxAI"),
        "install.ps1 must target per-user LOCALAPPDATA\\Programs\\DmuxAI directory"
    );

    // Must create Start Menu shortcut "DmuxAI.lnk"
    assert!(
        content.contains("DmuxAI.lnk"),
        "install.ps1 must define DmuxAI.lnk shortcut"
    );

    // Must target dmuxai-launcher.exe
    assert!(
        content.contains("dmuxai-launcher.exe"),
        "install.ps1 must reference dmuxai-launcher.exe"
    );

    // Must use -LiteralPath for safe path handling
    assert!(
        content.contains("-LiteralPath"),
        "install.ps1 must use -LiteralPath"
    );
}

#[test]
fn uninstall_script_meets_safety_and_removal_contracts() {
    let path = Path::new("packaging/windows/uninstall.ps1");
    let content = fs::read_to_string(path).expect("packaging/windows/uninstall.ps1 must exist");

    // Must not use Invoke-Expression or iex for security
    assert!(
        !content.contains("Invoke-Expression"),
        "uninstall.ps1 must never use Invoke-Expression"
    );
    assert!(
        !content.contains("iex "),
        "uninstall.ps1 must never use iex alias"
    );

    // Must reference the shortcut and install dir for cleanup
    assert!(
        content.contains("DmuxAI.lnk"),
        "uninstall.ps1 must remove DmuxAI.lnk"
    );
    assert!(
        content.contains("LOCALAPPDATA") && content.contains(r"Programs\DmuxAI"),
        "uninstall.ps1 must remove LOCALAPPDATA\\Programs\\DmuxAI directory"
    );

    // Must use -LiteralPath for safe path handling
    assert!(
        content.contains("-LiteralPath"),
        "uninstall.ps1 must use -LiteralPath"
    );
}

#[test]
fn release_workflow_includes_launcher_and_packaging_scripts() {
    let path = Path::new(".github/workflows/release.yml");
    let content = fs::read_to_string(path).expect(".github/workflows/release.yml must exist");

    assert!(
        content.contains("dmuxai-launcher.exe"),
        "release.yml must stage dmuxai-launcher.exe into release archive"
    );
    assert!(
        content.contains("install.ps1"),
        "release.yml must stage install.ps1 into release archive"
    );
    assert!(
        content.contains("uninstall.ps1"),
        "release.yml must stage uninstall.ps1 into release archive"
    );
    assert!(
        content.contains("README-windows.txt"),
        "release.yml must stage README-windows.txt into release archive"
    );
}

#[test]
fn package_windows_script_includes_launcher_and_packaging_scripts() {
    let path = Path::new("scripts/package-windows.sh");
    let content = fs::read_to_string(path).expect("scripts/package-windows.sh must exist");

    assert!(
        content.contains("dmuxai-launcher.exe"),
        "package-windows.sh must stage dmuxai-launcher.exe"
    );
    assert!(
        content.contains("install.ps1"),
        "package-windows.sh must stage install.ps1"
    );
    assert!(
        content.contains("uninstall.ps1"),
        "package-windows.sh must stage uninstall.ps1"
    );
    assert!(
        content.contains("README-windows.txt"),
        "package-windows.sh must stage README-windows.txt"
    );
    assert!(
        content.contains("gcc-mingw-w64-x86-64"),
        "package-windows.sh must check for gcc-mingw-w64-x86-64"
    );
    assert!(
        content.contains("x86_64-pc-windows-gnu"),
        "package-windows.sh must target x86_64-pc-windows-gnu"
    );
}
