use super::*;
use std::process::{Command, Stdio};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn powershell(script: &str, path: &Path) -> std::io::Result<()> {
    let status = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ])
        .env_remove("PSModulePath")
        .env("EUTHETO_TEST_PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(std::io::Error::other("Windows staging ACL fixture failed"));
    }
    Ok(())
}

#[test]
fn staged_report_does_not_inherit_shared_destination_permissions() -> TestResult {
    let directory = tempfile::tempdir()?;
    // A real Windows parent grants Builtin Users read access inherited by child files.
    // Setup may edit this disposable fixture's ACL; production never repairs its ACL.
    powershell(
        r"
$ErrorActionPreference = 'Stop'
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$users = [System.Security.Principal.SecurityIdentifier]::new('S-1-5-32-545')
$inheritance = [System.Security.AccessControl.InheritanceFlags]'ContainerInherit, ObjectInherit'
$acl = [System.Security.AccessControl.DirectorySecurity]::new()
$acl.SetAccessRuleProtection($true, $false)
$acl.SetOwner($sid)
$acl.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new(
  $sid, [System.Security.AccessControl.FileSystemRights]::FullControl, $inheritance,
  [System.Security.AccessControl.PropagationFlags]::None,
  [System.Security.AccessControl.AccessControlType]::Allow))
$acl.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new(
  $users, [System.Security.AccessControl.FileSystemRights]::ReadAndExecute, $inheritance,
  [System.Security.AccessControl.PropagationFlags]::None,
  [System.Security.AccessControl.AccessControlType]::Allow))
Set-Acl -LiteralPath $env:EUTHETO_TEST_PATH -AclObject $acl
$verified = Get-Acl -LiteralPath $env:EUTHETO_TEST_PATH
$rules = @($verified.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))
if (@($rules | Where-Object { $_.IdentityReference -eq $users }).Count -ne 1) {
  throw 'shared parent fixture was not established'
}
",
        directory.path(),
    )?;
    let destination = directory.path().join("rejected-report.json");
    let control = OperationControl::Cancellation(CancellationToken::new());
    let staged =
        prepare_json_atomic_controlled(&destination, &serde_json::json!({"rows": []}), &control)?;
    assert!(!destination.exists());
    // Inspect the actual staged file, not just the helper's directory DACL. This fails
    // for the old direct-in-shared-parent NamedTempFile implementation.
    powershell(
        r"
$ErrorActionPreference = 'Stop'
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$acl = Get-Acl -LiteralPath $env:EUTHETO_TEST_PATH
$rules = @($acl.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))
if ($acl.GetOwner([System.Security.Principal.SecurityIdentifier]) -ne $sid -or
    $rules.Count -ne 1 -or $rules[0].IdentityReference -ne $sid -or
    $rules[0].AccessControlType -ne [System.Security.AccessControl.AccessControlType]::Allow -or
    (($rules[0].FileSystemRights -band [System.Security.AccessControl.FileSystemRights]::FullControl) -ne
      [System.Security.AccessControl.FileSystemRights]::FullControl)) {
  throw 'staged bytes are not owner-private'
}
",
        staged.temporary.path(),
    )?;
    drop(staged);
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 0);
    Ok(())
}

#[test]
fn private_staging_rejects_existing_entries_without_cleanup_or_acl_repair() -> TestResult {
    let directory = tempfile::tempdir()?;
    let collision = directory.path().join("existing");
    std::fs::create_dir(&collision)?;
    let sentinel = collision.join("sentinel");
    std::fs::write(&sentinel, b"preserved")?;
    assert!(matches!(
        PrivatePublicationDirectory::create(collision),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists
    ));
    assert_eq!(std::fs::read(sentinel)?, b"preserved");
    Ok(())
}

#[test]
fn live_staging_directory_cannot_be_replaced_and_publish_releases_it() -> TestResult {
    let directory = tempfile::tempdir()?;
    let destination = directory.path().join("report.json");
    let control = OperationControl::Cancellation(CancellationToken::new());
    let value = serde_json::json!({"rows": []});
    let staged = prepare_json_atomic_controlled(&destination, &value, &control)?;
    let staging_directory = staged.private_directory.path.clone();
    assert!(std::fs::rename(&staging_directory, directory.path().join("redirected")).is_err());
    staged.publish_controlled(&control)?;
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&destination)?)?,
        value
    );
    assert!(!staging_directory.exists());
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
    Ok(())
}
