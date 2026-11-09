# Remote upload recovery: manual, read-only first

This runbook applies to RiceField's **single-file WebDAV, SFTP and signed SMB
uploads** and to their disposable capability probes. It is for an interrupted
upload, lost server reply, occupied destination, failed private-stage cleanup
or interrupted probe. **It is not an automatic repair procedure.** Never run
these checks against an irreplaceable share when a disposable test share is
available. A network timeout is **not proof of failure**: the destination may
already contain the newly published file.

## 1. Preserve evidence before doing anything else

1. Stop new uploads to this destination. Do not retry the transfer, rename,
   overwrite, delete or move either remote object while the result is unknown.
   Do not run generic recursive cleanup on `.ricefield-*` entries.
2. In RiceField, open **Recovery → Uncertain remote transfers and safety
   probes**. Copy the **destination URI** and **private-stage URI** (or the
   safety-probe prefix). Keep the record visible until the checks below finish.
3. Save the exact error and timestamp privately. Never paste passwords or
   credential-bearing command lines into issue trackers.
4. Optional local-only read-only inspection: journal records reside under
   `${XDG_STATE_HOME:-$HOME/.local/state}/ricefield/webdav-journals/`.
   `.toml` records identify upload attempts; `.probe` records identify
   capability-probe prefixes. Damaged records or temporary files may generate
   warnings; the review UI still shows independent valid records. Do not edit
   or remove journal files yourself.

These journal entries describe **an intended operation**, not a confirmed
server state. The application does not contact the server during recovery
inspection. A corrupt or symlinked journal root blocks new uploads; do not
work around that guard by changing filesystem permissions or deleting files.

## 2. Read-only server inventory

Use an authenticated *read-only* file browser or native remote client to
independently inspect the recorded destination and stage paths. If possible,
download both candidates into a separate local review directory and compare
sizes and SHA-256 hashes against an independently retained original. **Never
download on top of the source** and never supply credentials on a shell
command line where process listings or shell history can expose them.

- **WebDAV:** a private `.ricefield-upload-<id>` object is a staged file.
  Conditional `MOVE` can remove that name and create the destination **before
  its success response reaches the client**. `404` on the stage alone says
  nothing about whether the final file was published.
- **OpenSSH-SFTP:** the private `.ricefield-upload-<id>/payload` is staged in
  a directory. Publication is an exclusive hardlink, so **both** the payload
  and final file may coexist after successful publication if cleanup fails.
  Comparing hashes alone does not authorize deleting one of those entries.
- **Signed SMB:** the same private-directory/payload layout and exclusive
  hardlink model apply, but NAS share ACLs can prevent listing or cleanup.
  Lack of access is not evidence of an absent file. Do not assume the ACLs of
  RiceField's permissive disposable Samba test share match a Synology NAS.
- **Safety probes:** `.ricefield-safety-<id>` prefixes (WebDAV may have
  numbered `-a`, `-b` and `-destination` objects) are separate from actual
  destination files. Their presence is not evidence of a user file upload.

## 3. Classify what you actually observed

| Review label | Independent observation | Safe next step |
| --- | --- | --- |
| `UNKNOWN / OFFLINE` | Connection failed, timed out, access denied, or one path could not be checked | Keep evidence; restore access and repeat read-only inventory. |
| `STAGE ONLY` | Private payload verified, intended destination definitely absent | Keep evidence; resolve intentionally with server administrator. Never auto-replay a timed-out publish. |
| `DESTINATION ONLY` | Destination independently verified; stage absent | Preserve verification notes; consider retiring **only the local record** after checking destination identity and the intended content. |
| `BOTH PRESENT` | Private stage and final entry independently verified | Publication may have succeeded while cleanup failed; keep both until a qualified owner explicitly chooses a server-side cleanup. |
| `NEITHER FOUND` | Both paths independently and authoritatively absent | Record the checks and any concurrent changes; absence does not prove the initial request never ran. |
| `CONFLICT / DIFFERENT BYTES` | Destination already belongs to another writer or content differs | Keep both records and server data; stop and investigate. Do not replace the winner. |
| `PROBE UNCERTAIN` | Capability-probe prefix or its objects may remain | Inspect separately; do not treat probes as user-upload stages. |

Use the most conservative applicable label. In particular, a timeout,
`404` from one path, a collision response or a surviving hardlink never
automatically transitions a record to `SUCCESS` or `FAILED`.

## 4. Close the review without modifying server data from RiceField

Only after checking the remote stage, final destination, expected bytes and
any competing writers may the owner choose **I reviewed the server; discard
record…** in RiceField. This removes **only local evidence** after a second
confirmation. It does not delete, retry, publish or move any server file.
If verification is incomplete, retain the evidence; leaving it in place is
safer than asserting a false success. Any later remote cleanup is a separate,
manual, explicitly authorized operation with its own backup and confirmation.

## Test coverage and limits

The deterministic fixtures cover occupied destinations, pre-publish process
kills, post-publish OpenSSH-SFTP/signed-SMB process kills, an OpenSSH-SFTP
server refusing cleanup after successful hardlink publication, and a WebDAV
server committing `MOVE` but dropping its response. They assert original
source bytes, remote destination/stage state and restart-visible local
records. These are **protocol exercises on disposable local fixtures**, not
physical Wi-Fi loss, unclean NAS power-off, arbitrary server implementations
or Synology ACL acceptance. Those remain separate owner checks.
