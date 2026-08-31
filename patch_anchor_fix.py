#!/usr/bin/env python3
"""Fix call site compute_next_run di tools/mod.rs — signature baru 3 arg.
Pemeliharaan perubahan WIP owner (roll-forward recurring saat create)."""
import sys, pathlib

f = pathlib.Path("/root/my-assist/src/tools/mod.rs")
src = f.read_text()

def rep(text, old, new, count):
    n = text.count(old)
    assert n == count, f"expect {count}x, found {n}x: {old[:60]!r}"
    return text.replace(old, new)

# 1. roll-forward loop: 1x call dengan now — fungsi baru sudah catch-up sendiri
src = rep(src, """                    match reminders::compute_next_run(
                        input["recur"].as_str().unwrap_or(""),
                        remind_at,
                    ) {""",
"""                    match reminders::compute_next_run(
                        input["recur"].as_str().unwrap_or(""),
                        remind_at,
                        now,
                    ) {""", 1)

# 2. validasi recur di pesan sukses: now = remind_at (cuma cek Some/None)
src = rep(src, """                match reminders::compute_next_run(
                    input["recur"].as_str().unwrap_or(""),
                    remind_at
                ) {""",
"""                match reminders::compute_next_run(
                    input["recur"].as_str().unwrap_or(""),
                    remind_at,
                    remind_at
                ) {""", 1)

f.write_text(src)
print("PATCH OK: tools/mod.rs compute_next_run x2")
