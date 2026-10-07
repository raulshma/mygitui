//! OS-keyring-backed secret storage.
//!
//! Thin wrapper over the `keyring` crate used for credentials the app must
//! persist securely but never write to disk in plaintext: AI provider API
//! keys (e.g. OpenRouter) and opencode server credentials.
//!
//! Every secret lives under the single service name [`SERVICE`] (`"mygitui"`);
//! the caller-supplied `key` (e.g. `"openrouter-api-key"`) is the entry's
//! username. Backends: Windows Credential Manager, macOS Keychain, Secret
//! Service on Linux.
//!
//! Secret values are never logged and never included in error strings.

use keyring::Entry;

/// Service name under which every mygitui secret is stored.
const SERVICE: &str = "mygitui";

/// Open the keyring entry for `key`.
///
/// Errors are generic (operation + platform cause): keyring error `Display`
/// strings describe storage failures and never contain the secret value.
fn entry(key: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, key).map_err(|e| format!("failed to open secret store: {e}"))
}

/// Read a secret from the OS keyring.
///
/// Returns `Ok(None)` when no entry exists for `key` (never set, or deleted).
#[tauri::command]
pub fn secrets_get(key: String) -> Result<Option<String>, String> {
    match entry(&key)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("failed to read secret: {e}")),
    }
}

/// Store (create or overwrite) a secret in the OS keyring.
#[tauri::command]
pub fn secrets_set(key: String, value: String) -> Result<(), String> {
    entry(&key)?
        .set_password(&value)
        .map_err(|e| format!("failed to store secret: {e}"))
}

/// Delete a secret from the OS keyring.
///
/// Idempotent: deleting a key that does not exist is `Ok(())`.
#[tauri::command]
pub fn secrets_delete(key: String) -> Result<(), String> {
    match entry(&key)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("failed to delete secret: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reserved throwaway key used to probe backend availability.
    const PROBE_KEY: &str = "test/__backend_probe__";

    /// Whether this environment has a usable OS keyring backend.
    ///
    /// Windows Credential Manager and macOS Keychain are always present.
    /// On Linux the Secret Service backend needs a session D-Bus plus a
    /// provider (gnome-keyring / KWallet), which headless CI images usually
    /// lack -- probe with a real roundtrip there and fall back to skipping.
    fn backend_available() -> bool {
        if cfg!(any(target_os = "windows", target_os = "macos")) {
            return true;
        }
        let Ok(probe) = Entry::new(SERVICE, PROBE_KEY) else {
            return false;
        };
        let ok = probe.set_password("probe").is_ok();
        let _ = probe.delete_credential();
        ok
    }

    /// Skip guard for tests that need a real backend: prints a note and
    /// returns `true` when the environment has no usable keyring.
    fn skip_without_backend() -> bool {
        if backend_available() {
            false
        } else {
            eprintln!("skipping: no OS keyring backend available in this environment");
            true
        }
    }

    #[test]
    fn set_get_roundtrip() {
        if skip_without_backend() {
            return;
        }
        let key = "test/roundtrip".to_string();
        secrets_set(key.clone(), "s3cret-valu3".to_string()).expect("set should succeed");
        assert_eq!(
            secrets_get(key.clone()).expect("get after set should succeed"),
            Some("s3cret-valu3".to_string())
        );
        secrets_delete(key).expect("cleanup delete should succeed");
    }

    #[test]
    fn get_missing_returns_none() {
        if skip_without_backend() {
            return;
        }
        let key = "test/never_set".to_string();
        // Ensure it is absent even if a previous run left it behind.
        secrets_delete(key.clone()).expect("pre-clean delete should succeed");
        assert_eq!(
            secrets_get(key).expect("get of missing secret should succeed"),
            None
        );
    }

    #[test]
    fn delete_then_get_is_none() {
        if skip_without_backend() {
            return;
        }
        let key = "test/delete".to_string();
        secrets_set(key.clone(), "doomed".to_string()).expect("set should succeed");
        secrets_delete(key.clone()).expect("delete should succeed");
        assert_eq!(
            secrets_get(key.clone()).expect("get after delete should succeed"),
            None
        );
        // Deleting again is idempotent, not an error.
        secrets_delete(key).expect("delete of missing secret should succeed");
    }
}
