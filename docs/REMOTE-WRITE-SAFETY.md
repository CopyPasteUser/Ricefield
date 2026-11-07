# Why generic GIO network writes are still disabled

**Status: P0, updated 2026-11-07.** RiceField must not overwrite an unrelated file,
even when two clients race to publish the same destination name. A file may
appear under its final name only after its complete contents are verified.
These are requirements for turning on network writes, not optional UX policy.

## Reproduced failure

During development of [PR #71](https://github.com/CopyPasteUser/Ricefield/pull/71),
both of these approaches violated the no-overwrite contract on the local GIO
backend:

1. `gio move -T -C` replaced an already existing destination in a sequential
   test, even though no overwrite option was provided.
2. Native `g_file_move` with `G_FILE_COPY_NO_FALLBACK_FOR_MOVE` and without
   `G_FILE_COPY_OVERWRITE` passed sequential collision checks, but a concurrent
   test intermittently returned **success for both publishers** targeting the
   same final filename. The test failed in 13 of 30 repeated trials in this
   development environment. This is consistent with a check-then-rename race;
   the API flags alone do **not** provide a portable atomic no-replace commit.

The draft was closed **without merging**. A green GitHub CI run does not negate
an intermittent destructive race found by additional stress tests. None of
its upload controls, native bindings or remote write code entered `main`.

## Required design before re-enabling uploads

- Select a **backend-specific, server-enforced atomic conditional publish**
  (or fail closed when the backend cannot provide one). A client-side
  `exists`/`info` check, a per-process lock or a global RiceField mutex cannot
  prevent overwrites by independent server clients.
- Write only to exclusively created unpredictable staging objects; retain the
  local source. Verify full staged bytes against an unchanged, pinned local
  source. Publish the finished stage only once.
- Verify guarantees in concurrent and adversarial tests per supported backend:
  SMB, SFTP, WebDAV and MTP where write support is actually promised. Include
  existing files, directories, dangling symlinks, collisions created during
  transfer, concurrent clients, cancellations, unmounts, offline transitions,
  partial files and recovery after abrupt termination.
- Treat remote filesystem capability claims as *unverified* until the actual
  server and mount implementation pass the protocol-specific test matrix.
  Some backends may have to remain read-only.

## Safe behavior today

GIO URI tabs support **read-only navigation, previews and explicit downloads**.
An isolated WebDAV-only single-file upload path now attempts conditional
`PUT If-None-Match: *` to an unpredictable private stage, verifies the full
staged bytes against a private local snapshot and requests server-side
`MOVE` with `Overwrite: F`. Before each upload, four disposable checks
(including three concurrent two-client collisions) require exactly one
publisher to succeed and the other to receive HTTP 412. Unsupported or
nonconforming servers fail closed, with no upload. This provides a narrowly
testable backend, **not acceptance of every WebDAV implementation**; real
NAS and adversarial process-crash cases still require user testing. Failed
uploads may leave hidden server stages requiring manual inspection.
When an upload could have created a stage and later fails, RiceField reports
the exact private stage URI and warns that timeout or publication results may
be ambiguous. It does not automatically delete or retry the remote object.
Preserve that URI privately until the final destination and stage have been
checked on the server. This is not durable remote-upload journaling.

SMB, SFTP and MTP upload, and all remote rename and delete actions remain
disabled until their own atomic backends pass the same gate. A system-mounted share can be
browsed as a local path and only supports the local file engine's guarded
operations where its mount exposes the necessary filesystem primitives.
Do not bypass a rejected atomic rename with a non-atomic fallback.

This blocker is separate from the owner's later Linux Mint/NAS hardware
acceptance: it is an unresolved **code/architecture requirement**, so the
project must not claim that only manual testing stands between it and 1.0.
