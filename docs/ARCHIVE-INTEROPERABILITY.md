# Encrypted archive interoperability (Linux backend)

Recorded 2026-11-05. These are **independent backend compatibility checks**,
not a completed Linux Mint GUI acceptance or a stable 0.5 sign-off.

RiceExplorer dynamically loads `libarchive.so.13` and passes passwords to its
archive reader. On a Linux test runner with **libarchive 3.7.4**, the
`scripts/archive-interoperability.py` probe read every member's full payload
using libarchive's C API (`archive_read_open_fd` / `archive_read_add_passphrase`).
It tested the **correct**, **incorrect** and **absent** passwords against
12 independently authored upstream test fixtures.

Source: [`libarchive/libarchive`](https://github.com/libarchive/libarchive),
upstream commit `2774b74cb7269c9b5fa89e7d876b11d5a729c421`,
`libarchive/test/test_read_format_*.uu` and accompanying `.c` test definitions.
The fixture passwords below are public upstream test passwords, **not user
credentials**. Binary fixtures are not vendored into this repository.

| Upstream encrypted sample | Correct password | Wrong / absent password | Interpretation |
| --- | --- | --- | --- |
| `zip_winzip_aes128.zip` | Full read | Rejected / rejected | Supported |
| `zip_winzip_aes256.zip` | Full read | Rejected / rejected | Supported |
| `zip_winzip_aes256_stored.zip` | Full read | Rejected / rejected | Supported |
| `zip_traditional_encryption_data.zip` | Full read | Rejected / rejected | Supported (legacy ZIP) |
| `zip_encryption_data.zip` | Rejected | Rejected / rejected | Unsupported encryption flag in this backend |
| `zip_encryption_header.zip` | Rejected | Rejected / rejected | Encrypted-header fixture not supported |
| `7zip_encryption.7z` | Rejected | Rejected / rejected | Encrypted 7z payload unsupported |
| `7zip_encryption_header.7z` | Rejected | Rejected / rejected | Encrypted 7z header unsupported |
| `7zip_encryption_partially.7z` | Rejected | Rejected / rejected | Encrypted 7z payload unsupported |
| `rar_encryption_data.rar` | Rejected | Rejected / rejected | Encrypted RAR payload unsupported |
| `rar_encryption_header.rar` | Rejected | Rejected / rejected | Encrypted RAR header unsupported |
| `rar_encryption_partially.rar` | Rejected | Rejected / rejected | Encrypted RAR payload unsupported |

All four supported ZIP vectors read fully with the correct passphrase; none
read with the wrong or absent passphrase. **Do not advertise encrypted 7z or
RAR as compatible** on libarchive 3.7.4, and do not assume every possible ZIP
encryption extension is supported. Other libarchive builds may differ;
retest per distribution. Ordinary unencrypted 7z/RAR support is separate.

## Reproduce without copying archive payloads into the project

```bash
git clone --depth 1 --filter=blob:none --sparse https://github.com/libarchive/libarchive.git /tmp/rice-libarchive-fixtures
git -C /tmp/rice-libarchive-fixtures sparse-checkout set --no-cone 'libarchive/test/test_read_format_7zip_encryption*' 'libarchive/test/test_read_format_rar_encryption*' 'libarchive/test/test_read_format_zip_encryption*' 'libarchive/test/test_read_format_zip_traditional_encryption*' 'libarchive/test/test_read_format_zip_winzip_aes*'
python3 scripts/archive-interoperability.py /tmp/rice-libarchive-fixtures/libarchive/test
```

The script uses only Python's standard library and the system
`libarchive.so.13`, prints backend-specific outcomes and fails if any of the
four required ZIP cases regress or an incorrect/missing password succeeds.
It reports other formats without treating unsupported encryption as a
successful integration. It extracts no files and never handles real secrets.

**Still open:** Rust application's full end-to-end UX on a physical Mint
device, archive creation compatibility against an independent extractor,
more encrypted format variants, low-RAM runs and GUI error wording. Keyring
storage and real-device/mount-disconnect acceptance remain explicitly deferred
by the project owner; no remote writes or automatic credential persistence.
