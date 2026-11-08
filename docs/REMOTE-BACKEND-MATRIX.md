# Remote-backend publication safety matrix

This matrix describes **what RiceField itself currently proves and enables**.
It does not infer guarantees from a protocol name or the presence of a GVfs
mount. A successful generic `gio move` is not evidence of atomic no-replace
publication; see [the reproduced race](REMOTE-WRITE-SAFETY.md).

| Route | Browse / preview / download | RiceField remote write | Evidence / code gate | Owner hardware gate |
| --- | --- | --- | --- | --- |
| `smb://` (GVfs) | Read-only URI tab, bounded previews and downloads | **Disabled** | No verified server-specific atomic create-and-publish implementation | SMB NAS path, disconnect/reconnect, credentials, collisions |
| `sftp://` (GVfs) | Read-only URI tab, bounded previews and downloads | **Conditional single-file upload via OpenSSH client only** | Private `0700` staging directory; disposable four-round server-side `hardlink@openssh.com` test with independent clients; full staged GET byte comparison; atomic hardlink-to-final that refuses occupied names. Private synced evidence before probe/staging mutation. No rename fallback. | Real SFTP host, SSH host key and agent, hardlink support, permissions, latency/disconnect, storage consistency |
| `dav://`, `davs://` | Read-only URI tab plus separately authorized single-file upload | **Conditional single-file upload only** | Exclusive random stage PUT, byte-for-byte verification, `MOVE Overwrite: F`, sequential and concurrent two-client collision probes; separate private durable safety-probe evidence before any probe PUT and upload evidence before its own PUT. Reject a server that fails any probe. | Real NAS/WebDAV server, HTTP/TLS/auth, offline/crash, competing clients, manual journal cleanup |
| `mtp://` (GVfs) | Read-only browsing and supported download | **Disabled** | No verified atomic no-replace server-side commit or predictable recovery | Actual phone/handheld, unplug, permission and reconnect behavior |
| System-mounted NFS / SMB path | Navigated as a local filesystem | Only operations explicitly accepted by the guarded local engine | Kernel/mount filesystem primitives must actually support local no-overwrite and metadata checks; a network mount is not a blanket atomicity guarantee | NFS/SMB mount with real concurrent clients and disconnect |

## Mechanical test contract

1. Start from a disposable source and destination. Never test against a real
   user file. Both clients choose the **same** new destination, but distinct
   unpredictable staging names.
2. Prove that an existing destination is byte-for-byte unchanged after a
   collision. During a concurrent race, exactly one publisher may succeed;
   the other must receive a clear server-side precondition refusal.
3. Verify the complete staged content against the pinned local source before
   any final name appears. Refuse changed or unverifiable data, and keep the
   original. Test directory, symlink and other object-type collisions.
4. Interrupt each write phase, including after the remote server stored a
   stage but **before the client got its response**. An uncertain outcome must
   retain durable local evidence; it must not automatically retry or remove
   unknown server objects.
5. Decline operations on any backend where the contract cannot be independently
   demonstrated. Read-only browsing and explicit no-overwrite downloads remain
   available, never a hidden fall-through to a generic GIO writer.

## Automated coverage versus physical sign-off

The local Rust mock WebDAV server exercises existing-target collisions,
servers ignoring PUT/MOVE preconditions, independent curl request clients,
bounded staged byte comparison, cancellation, journaling failure and retained
evidence. A separate child-process regression kills the uploader after the
server receives the private stage PUT, confirming journal survival and no
publication to the final name.

Safety-probe evidence now records each random prefix before the first probe
PUT; failed/cancelled probes remain inspectable after restart, and a missing
or unwritable journal prevents the probe itself. The record names potential
round 0–3 `-a`, `-b` and `-destination` objects. It does not mean those
objects exist or authorize automatic server cleanup. Probes orphaned by older
releases have no retrospectively recoverable record.
A separate actual-process SIGKILL regression stops the first safety-probe PUT
after the mock server stores it but before the client receives its response;
the probe journal must already be durable and the requested upload must not
have started.

SFTP tests use **real OpenSSH SFTP protocol traffic**, not GVfs emulation:
`sftp -D /usr/lib/openssh/sftp-server` runs the isolated local server and
verifies successful publication, existing-file and symlink collisions, a
simultaneous two-client hardlink race, unsafe URI rejection and unavailable
server handling. The production path uses `sftp -b -` with strict host-key
checking, public-key / SSH-agent authentication and no password injection.
If OpenSSH hardlink support, remote creation, byte comparison or safety tests
fail, the upload fails closed; uncertain private probe/stage directories remain
in the local review journal without automatic server mutations.

SFTP upload paths and filenames currently accept only ASCII letters, digits,
spaces, dots, underscores and hyphens, without traversal or wildcard syntax.
IPv6 literal hosts and password-based SFTP upload are not supported. Browsing
and downloading via GVfs remain independent of these upload restrictions.

Mocks do **not** demonstrate a particular real NAS implementation's semantics,
storage durability, filesystem driver behavior or physical power-loss safety.
Those require the owner's disposable NAS/Mint tests before stable 1.0.

SMB and MTP remote writers remain disabled. The restricted SFTP writer is
**not** a blanket claim of safe writes on every SFTP server. Track further
backend acceptance under
[P0 issue #84](https://github.com/CopyPasteUser/Ricefield/issues/84).
