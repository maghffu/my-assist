import sys

p = "/root/my-assist/src/shell.rs"
src = open(p).read()

if "system prune" in src:
    print("ALREADY PATCHED — batal")
    sys.exit(1)

# 1) Tambah 3 kategori pattern baru sebelum blok curl|sh
anchor1 = "    // curl/wget di-pipe ke shell"
new_checks = '''    // Docker prune — hapus resource docker tak terpakai (container/image/volume/cache)
    if toks.iter().any(|t| *t == "prune")
        || joined.contains("system prune")
        || joined.contains("builder prune")
        || joined.contains("container prune")
        || joined.contains("volume prune")
        || joined.contains("image prune")
    {
        return Some("docker prune (hapus resource docker tak terpakai)");
    }

    // journalctl vacuum — potong log journald
    if toks.iter().any(|t| *t == "journalctl") && joined.contains("vacuum") {
        return Some("potong log journald (journalctl --vacuum)");
    }

    // truncate — kosongkan file (umumnya log)
    if toks.iter().any(|t| *t == "truncate") {
        return Some("truncate file (kosongkan isi file)");
    }

'''
assert anchor1 in src, "anchor1 tidak ketemu"
src = src.replace(anchor1, new_checks + anchor1, 1)

# 2) Tambah test asserts
anchor2 = '''        assert_eq!(
            destructive_reason("chmod -R 777 /var/www"),
            Some("ubah izin rekursif (chmod -R)")
        );'''
new_tests = anchor2 + '''
        assert_eq!(
            destructive_reason("docker system prune -af --volumes"),
            Some("docker prune (hapus resource docker tak terpakai)")
        );
        assert_eq!(
            destructive_reason("journalctl --vacuum-size=200M"),
            Some("potong log journald (journalctl --vacuum)")
        );
        assert_eq!(
            destructive_reason("truncate -s 0 /var/log/messages"),
            Some("truncate file (kosongkan isi file)")
        );
        assert_eq!(destructive_reason("docker ps"), None);'''
assert anchor2 in src, "anchor2 tidak ketemu"
src = src.replace(anchor2, new_tests, 1)

open(p, "w").write(src)
print("PATCHED OK (+docker prune, journalctl vacuum, truncate + 4 asserts)")
