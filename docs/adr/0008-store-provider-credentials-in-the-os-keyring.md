# ADR-0008: Store provider credentials in the OS keyring

HelixNotes stores provider credentials in the current user's native OS credential store,
keyed by the application identifier and a secret-free account name. `config.json` retains
provider addresses and usernames but is a redacted projection: it never contains AI keys,
WebDAV passwords, or future integration tokens.

Per-vault credentials use the stable identity in `.helixnotes/vault_id`, not the vault path
or bookmark. Moving a vault therefore keeps its keyring entries addressable. The identifier
itself is safe to keep with the vault and in machine-local configuration; it is not a secret.

Existing plaintext credentials migrate transactionally at startup. The app removes them from
`config.json` only after the credential-store writes succeed; if the store is unavailable or
locked, the running configuration is cleared of credentials while the file is left intact for
a later retry. While migration is blocked, non-secret settings may still be saved. Such a save
copies only the plaintext recovery values already present on disk into the next disk projection;
those values never enter the running configuration or act as a provider fallback. Settings writes
that change credentials update the credential store before the redacted config and roll back the
first write if the second fails. This fail-closed boundary avoids both plaintext fallback and
credential loss, while still allowing a headless process without a credential store to start with
provider features unconfigured.

The alternative of encrypting credentials with an application-managed key was rejected because
it only moves the secret needed for decryption. Storing secrets in a vault was rejected because
vaults are user-controlled, portable Markdown collections and may themselves be synchronized.
