#!/usr/bin/env python3
"""Probe independent libarchive encrypted fixtures using RiceExplorer's C API.

This is a backend compatibility probe, not a GUI or Rust integration test.
Run against the upstream libarchive/test directory (containing *.uu fixtures).
No fixtures, passwords, or extracted contents are saved beyond a temp directory.
"""

import argparse
import binascii
import ctypes
import pathlib
import tempfile


# Official libarchive test vectors; passwords are published with upstream tests.
SAMPLES = {
    "zip_winzip_aes128.zip": "password",
    "zip_winzip_aes256.zip": "password",
    "zip_winzip_aes256_stored.zip": "password",
    "zip_traditional_encryption_data.zip": "12345678",
    "zip_encryption_data.zip": "12345678",
    "zip_encryption_header.zip": "12345678",
    "7zip_encryption.7z": "12345678",
    "7zip_encryption_header.7z": "12345678",
    "7zip_encryption_partially.7z": "12345678",
    "rar_encryption_data.rar": "12345678",
    "rar_encryption_header.rar": "12345678",
    "rar_encryption_partially.rar": "12345678",
}
ZIP_REQUIRED = {
    "zip_winzip_aes128.zip",
    "zip_winzip_aes256.zip",
    "zip_winzip_aes256_stored.zip",
    "zip_traditional_encryption_data.zip",
}


def backend():
    lib = ctypes.CDLL("libarchive.so.13")
    lib.archive_version_string.restype = ctypes.c_char_p
    lib.archive_read_new.restype = ctypes.c_void_p
    for name in ("archive_read_support_filter_all", "archive_read_support_format_all"):
        getattr(lib, name).argtypes = [ctypes.c_void_p]
    lib.archive_read_add_passphrase.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
    lib.archive_read_open_fd.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_size_t]
    lib.archive_read_next_header.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p)]
    lib.archive_entry_size.argtypes = [ctypes.c_void_p]
    lib.archive_entry_size.restype = ctypes.c_longlong
    lib.archive_read_data.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_size_t]
    lib.archive_read_data.restype = ctypes.c_ssize_t
    lib.archive_error_string.argtypes = [ctypes.c_void_p]
    lib.archive_error_string.restype = ctypes.c_char_p
    lib.archive_read_free.argtypes = [ctypes.c_void_p]
    return lib


def decode_uu(source, output):
    lines = source.read_bytes().splitlines()
    if not lines or not lines[0].startswith(b"begin "):
        raise ValueError(f"Expected uuencoded fixture: {source}")
    with output.open("wb") as destination:
        for line in lines[1:]:
            if line == b"end":
                return
            destination.write(binascii.a2b_uu(line))
    raise ValueError(f"Missing uuencode end marker: {source}")


def try_read(lib, path, password):
    reader = lib.archive_read_new()
    if not reader:
        return False, "Unable to allocate archive reader"
    try:
        if lib.archive_read_support_filter_all(reader) != 0:
            return False, "Cannot configure filters"
        if lib.archive_read_support_format_all(reader) != 0:
            return False, "Cannot configure formats"
        if password and lib.archive_read_add_passphrase(reader, password.encode()) != 0:
            return False, "Passphrase rejected"
        with path.open("rb") as source:
            if lib.archive_read_open_fd(reader, source.fileno(), 65536) != 0:
                return False, "Unable to open archive"
            buffer = ctypes.create_string_buffer(65536)
            while True:
                entry = ctypes.c_void_p()
                code = lib.archive_read_next_header(reader, ctypes.byref(entry))
                if code == 1:  # ARCHIVE_EOF
                    return True, "all member payloads read"
                if code != 0:
                    break
                size = 0
                while True:
                    received = lib.archive_read_data(reader, buffer, len(buffer))
                    if received <= 0:
                        break
                    size += received
                if received < 0:
                    break
                if size != lib.archive_entry_size(entry):
                    return False, "Member size mismatch"
            message = lib.archive_error_string(reader)
            return False, message.decode(errors="replace") if message else f"Read error {code}"
    finally:
        lib.archive_read_free(reader)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture_dir", type=pathlib.Path, help="Upstream libarchive/libarchive/test directory")
    args = parser.parse_args()
    lib = backend()
    print("Backend:", lib.archive_version_string().decode())
    unexpected = []
    with tempfile.TemporaryDirectory(prefix="rice-archive-interop-") as tmp:
        for filename, password in SAMPLES.items():
            upstream = args.fixture_dir / f"test_read_format_{filename}.uu"
            if not upstream.is_file():
                raise FileNotFoundError(upstream)
            archive = pathlib.Path(tmp) / filename
            decode_uu(upstream, archive)
            good, detail = try_read(lib, archive, password)
            wrong, _ = try_read(lib, archive, "incorrect-password")
            missing, _ = try_read(lib, archive, None)
            print(f"{filename}: correct={good}, wrong={wrong}, missing={missing} — {detail}")
            if (filename in ZIP_REQUIRED and not good) or wrong or missing:
                unexpected.append(filename)
    if unexpected:
        raise SystemExit("Unexpected compatibility results: " + ", ".join(unexpected))
    print("Required ZIP checks passed; other encrypted formats are informational/backend-dependent.")


if __name__ == "__main__":
    main()
