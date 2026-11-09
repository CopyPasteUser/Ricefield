# RiceField 1.0 remote-write scope and acceptance contract

**Scope decision for the 1.0 candidate:** remote browsing, preview and guarded
download are supported where the respective GVfs reader works. Only the
backend-specific **single-file upload** routes described below may mutate a
remote server. Remote rename, delete, directory creation, arbitrary remote
copy/move, multi-file upload, MTP upload and transparent GIO writes are
**out of scope for 1.0 and must remain disabled**. They are not silently
implemented as a client-side `exists` check followed by a replace-capable
rename. Adding one later requires new protocol-specific capability, failure
and recovery tests plus a separate product decision.

This is a deliberately narrow functional scope, **not** proof that every
server speaking these protocols supports writing. Unsupported implementations
remain read-only. An application can still use local operations on an
explicitly system-mounted NFS/SMB path, but that path must independently
meet the local filesystem engine's atomicity rules.

## Advertised operations and server capabilities

| Backend | Read / bounded download | Direct remote upload | Upload admission | Still disabled in 1.0 |
| --- | --- | --- | --- | --- |
| WebDAV (`dav`/`davs`) | GVfs reader | Single file only | Random exclusive `PUT If-None-Match: *` stage, full GET comparison, four conditional `MOVE Overwrite: F` collision rounds including independent clients, synced recovery journal | Rename, delete, directory writes, multi-file and generic GIO write |
| OpenSSH-SFTP | GVfs reader | Single file only | Strict host-key and key-agent auth; private 0700 directory, full bounded GET comparison, four exclusive hardlink rounds, synced recovery journal | Password-upload fallback, rename, delete, remote directory mutation |
| Signed SMB2/3 | GVfs reader | Single file only | Signed, authenticated `smbclient`; random stage directory, full bounded GET comparison, four exclusive hardlink rounds, synced recovery journal; requires trusted share ACLs | Rename, delete, arbitrary GIO writes; no claim of stage ACL privacy |
| MTP | GVfs reader | **No** | No proven atomic no-replace publication | All writes |
| System-mounted network share | Local engine only | Subject to mounted-filesystem engine guards | Inode/identity and atomic publication checks of the local engine, as actually provided by the mount | Any unproven fallback or implicit remote-write authorization |

Every admitted upload takes a pinned private local snapshot, verifies server
bytes **before** publication, and issues exactly one no-replace publication
command. It must not retry an ambiguous publication, delete possible
remote leftovers or infer success from a timeout. Recovery uses durable local
records, which are inspected read-only; see
[the recovery runbook](REMOTE-RECOVERY-RUNBOOK.md).

## Disposable integration evidence (CI)

| Requirement | WebDAV | OpenSSH-SFTP | Signed SMB |
| --- | --- | --- | --- |
| Full uploader publishes exactly verified bytes | Real HTTP/curl against a local mock | Real `sftp` client with local OpenSSH server | Rust uploader against isolated authenticated Samba |
| Existing destination never replaced | HTTP 412 and unchanged bytes | Server refuses hardlink, file unchanged | Signed hardlink refused, file unchanged |
| Two **full uploaders** race for the same name | Distinct curl requests and private stages; one winner | Distinct OpenSSH sessions and private stages; one winner | Two authenticated signed SMB sessions and stages; one winner |
| Directory / symlink collision | Mock tests existing target; directory/symlink semantics require real-server acceptance | Real directory and symlink collision fixtures | Real directory collision fixture; symlink behavior is server/ACL-dependent |
| Unacknowledged publication / crash | Mock commits `MOVE` and drops response | Real child killed after hardlink | Real child killed after hardlink |
| Post-commit cleanup refusal | `MOVE` consumes private stage; no post-commit stage deletion | OpenSSH server rejects `rm`; journal and stage survive | Crash before cleanup; NAS ACL-dependent failure needs hardware acceptance |
| Retain original source and local recovery evidence | Yes | Yes | Yes |

Passing this matrix means **the disposable protocol implementation** passed
the stated checks, not that any arbitrary NAS is compliant. Full uploader
races additionally test their journal locking, pinned content, independently
authenticated clients and actual publish sequence; tests of the low-level
hardlink/MOVE primitive alone are insufficient to validate that composition.

## Synology DS214+ owner acceptance (still open)

The owner must create a **new disposable, empty, user-owned test share** on
the Synology with separate credentials. Never use the `medien` share or any
production library. Check beforehand that backups exist, the account cannot
write to unrelated shares, and RiceField has no privileged network mounts
that bypass this isolation.

1. **Capability:** connect over SMB with the test account; check that signed
   SMB2/3 works, and observe whether the server supports the exclusive
   hardlink probe. If not, verify RiceField refuses the upload while browsing
   and downloads still work.
2. **Stage ACL:** after a disposable upload or an intentionally interrupted
   test, verify the test account can inspect its stage, a separate untrusted
   account cannot read or alter it, and nobody changed the share policy.
   SMB's default ACL inheritance is not an application-enforced private 0700.
3. **Collisions:** prepare an unrelated existing file and directory in the
   isolated share. Try uploading with the same names. Confirm exact original
   bytes and directory entries survive. Repeat with two test clients writing
   different bytes to one vacant name; require exactly one successful publish.
4. **Disconnect:** interrupt Wi-Fi/network access before and after the stage
   upload and after publication, then reconnect. Independently inspect the
   stage, destination and local journal using the
   [manual runbook](REMOTE-RECOVERY-RUNBOOK.md). Never retry the uncertain
   request blindly or automatically clean the server.
5. **Restart/permissions:** restart the client with its journal intact;
   recover view-only evidence. Test denied permissions separately. Verify
   no sensitive password is exposed in process arguments, logs or journal.
6. Record NAS firmware, SMB signing/ACL settings, test-account scope,
   RiceField commit SHA, commands/visible outcomes and each failed case in
   [owner acceptance](OWNER-ACCEPTANCE-1.0.md). Restore only the disposable
   test share afterward; do not run bulk cleanup against real media.

**Exit gate:** automated evidence alone closes the software-specific race
and fail-closed checks, not the physical NAS, disconnect or ACL sign-off.
The exact 1.0 scope must be visible to the user before the final tag; no
release may silently claim unsupported remote rename/delete/MTP writes.
