# Secret Store Key Hierarchy Design

> Tenet 4 — the genuine one-way door. Every secret ever stored is encrypted
> under whatever hierarchy we pick on day one. Changing it later means a
> migration that decrypts and re-encrypts every item for every user. Write it
> down before writing the crate.

## Hierarchy

```
Device OS keychain (or HSM on server)
  └─ MASTER KEY (Argon2id, per-user salt)
       └─ ACCOUNT KEY (derived, rotates on master password change)
            └─ ORG KEY (for team scope, wrapped to admin keypair)
                 └─ PER-ITEM KEY (random 256-bit, unique per secret)
                      └─ SECRET (AES-256-GCM, authenticated encryption)
```

### Master key derivation
- **KDF**: Argon2id (RFC 9106). Parameters: t=3, m=64 MiB, p=4, dklen=32.
- **Salt**: 32-byte random, stored per-user in the database plaintext
  (salt is not secret). Raises cost on DB compromise without revealing
  which users exist.
- **Password**: user-supplied master password (or OS auth on macOS/iOS,
  TPM attestation on Windows/Linux).
- **Storage**: Master key is derived, never stored. On success, the derived
  key decrypts the account key blob.
- **Raising**: Parameters are stored as versioned constants in the crate,
  not persisted per-user. Re-creation of a master key uses the latest
  parameters. Old parameter versions are kept in a `KDF_VERSION` table for
  migration, not silently upgraded.

### Wrapping chain
1. **Master key** → decrypts **account key** blob (stored encrypted).
   The account key blob is versioned; changing the master password re-encrypts
   only the account key, not all items.
2. **Account key** → decrypts **item keys** (stored encrypted, one blob per item).
   Item keys are 256-bit random, AES-256-GCM encrypted to the account key.
3. **Per-item key** (random 256-bit) → encrypts the actual **secret**.

Rotation of the account key only requires re-encrypting N item-key blobs (N =
items per account), not the items themselves — a fast batch.

### Sharing
- Each principal (user, agent, service) has an Ed25519 keypair (generated
  client-side, never leaves the device).
- An item shared into a scope: the item key is wrapped (AES-256-GCM) to each
  member's Curve25519 public key. The item key itself does not change.
- Scope boundaries come from `dock-ybc.11` OIDC scoping. Key assembly happens
  per-scope (resolve scope → list member public keys → unwrap item key), not
  via post-filtering.
- Org keys (for team scope) are wrapped to admin keypairs at enrollment time.

### Rotation (designed in from the start)
Three distinct operations, each safe to retry:
1. **Item key rotation**: Generate new item key, re-encrypt only that item's
   secret. Old item key blob deleted. Fast, local.
2. **Account key rotation**: Generate new account key, re-wrap each item key
   blob to the new account key. Batch operation, can be deferred to idle time.
3. **Org key rotation**: Generate new org key, re-wrap to new admin keypairs.
   Members must re-fetch their item key blobs (already wrapped to their
   personal key — unaffected).

### Recovery
- **Personal scope**: Forgot master password → lose the vault. This is
  correct for personal scope: the device-local key derivation means no
  server-side recovery is possible. Honest and correct.
- **Team scope**: Admin escrow. On enrollment, each member's public key is
  registered with the org. An admin can approve a recovery request, which
  re-wraps the member's item keys to the member's new keypair. The admin
  never sees plaintext secrets — only approves the key swap.
- **2FA for recovery**: Admin approval requires step-up auth (TOTP, passkey,
  or out-of-band confirmation). A single admin cannot silently recover.

## Threat Model

| Attacker holds | What they learn | Mitigation |
|---|---|---|
| **The database** (full dump) | Nothing. Item keys and secrets are AES-256-GCM encrypted. Master key is Argon2id-derived from password (not stored). Account key is encrypted to master key. | Argon2id with per-user salt; 64 MiB memory makes brute-force expensive. |
| **One device** (compromised laptop) | Master password entry → account key + all item keys + all secrets for that user. | Device-level threat; mitigated by OS auth (macOS keychain, Windows Hello). Other users unaffected. |
| **The host process** (Canvas binary compromised) | Can observe decrypted data only while the user is authenticated (in-memory secrets). | Secrets are decrypted on-demand in isolated scopes; never persisted to disk unprotected. |
| **A valid session token** | Can make API calls within the token's scope until expiry. | Tokens are short-lived (15 min); scope is OIDC-derived per `dock-ybc.11`. |
| **A valid admin** | Can approve recovery for team members; can add/remove members. | Recovery requires 2FA step-up. Admin cannot decrypt items directly (only approves key rewraps). Logs all recovery operations. |
| **A valid user, wrong scope** | Cannot access items outside their OIDC scope. | Key assembly happens per-scope; item keys wrapped only to scope members' public keys. |

## Design Decisions

1. **No server-side master key**: The master key is always derived client-side
   from the user's password. The server stores only encrypted blobs. This is
   zero-knowledge.

2. **Per-item keys (not per-user encryption)**: Each secret gets its own random
   256-bit key. Sharing = re-wrapping the item key to new recipients. Rotation
   = re-encrypting one item. This makes sharing and rotation composable.

3. **Versioned KDF parameters**: Parameters are crate-level constants with a
   version tag. Old versions retained for migration. No per-user parameter
   storage that could drift.

4. **Curve25519 for key wrapping, Ed25519 for signing**: Separate key pairs for
   confidentiality (wrapping) and authenticity (signing). Both generated
   client-side.

5. **Personal vs. team recovery are separate**: Personal = lose password → lose
   vault. Team = admin escrow with 2FA step-up. Mixed-scope items use the
   team path.

6. **AES-256-GCM**: Authenticated encryption. The GCM tag catches tampering
   even if an attacker modifies encrypted blobs.

## Metadata Encryption Boundary (dock-ybc.12.2)

> LastPass 2022: vault URLs were not encrypted, so attackers knew who banked
> where and could phish precisely during brute-force. The cipher was fine;
> the metadata boundary was the vulnerability.

Every field is classified: **E** (encrypted at rest) or **P** (plaintext,
safe to expose).

| Field | Classification | Rationale |
|---|---|---|
| Secret content (passwords, tokens, keys, notes) | **E** | The actual secret. Always encrypted. |
| Item title/label | **E** | Reveals the service name (bank, email). Encrypt. |
| URL | **E** | Reveals browsing patterns. Encrypt (LastPass lesson). |
| Username/account | **E** | Often the email address — PII. Encrypt. |
| TOTP secret | **E** | Effectively a password. Encrypt. |
| Creation/modified timestamps | **P** | Needed for sync; not sensitive. |
| Item ID (UUID) | **P** | Needed for lookup; reveals nothing. |
| Item type (password, TOTP, note, key) | **E** | Knowing "has TOTP" leaks MFA adoption. |
| Sharing scope (personal/team/project) | **P** | Required for routing; not sensitive. |
| Member list (team scope) | **P** | Needed for access control; members consent to visibility. |
| Owner identity | **P** | Derived from OIDC session; already known to fpl-auth. |
| Expiration/TTL | **P** | Needed for cleanup. |

**Server-visible set**: item ID, sharing scope, member list, owner identity,
creation/modified timestamps, expiration. Everything else is encrypted
client-side before storage. The server can route, sync, and grant access —
but cannot enumerate which services a user uses, which accounts they hold,
or what their passwords look like.

This classification is immutable: once adopted, fields cannot be migrated
between E and P without re-encrypting or decrypting all data (the one-way
door from Tenet 4).

## Depends on / Blocks

- Depends on: nothing (this is the foundation).
- Blocks: `dock-ybc.12` (secret store implementation),
  `dock-ybc.12.2` (metadata encryption boundary).
- Relates to: `dock-ybc.11` (OIDC scoping), `dock-ybc.10` (agent-scoped sessions).
