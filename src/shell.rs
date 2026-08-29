//! Shell access (Pilar 9, ROADMAP Fase 4): `run_command` + `read_file`/`write_file`.
//!
//! Mekanik inti: `bash -lc "cd $cwd && <cmd>"` one-shot, cwd di-track per chat
//! (biar `cd` efektif antar panggilan), timeout + kill process group, output
//! panjang jadi file attachment, audit ke `command_logs`, secret masking.
//!
//! Keamanan: destructive pattern → confirmation gate inline keyboard (owner
//! tap approve/deny), tool menunggu via oneshot channel dengan timeout.

use crate::config::Config;
use anyhow::{bail, Context, Result};
use sqlx::PgPool;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use teloxide::prelude::*;
use teloxide::types::{ChatId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStderr, ChildStdout};
use tokio::sync::oneshot;

/// Tool result ke LLM: tail sebanyak ini (AGENTS.md Pilar 9 — truncation dengan tail).
const MAX_CTX_CHARS: usize = 2000;
/// Output melebihi ini → dikirim sebagai file attachment (send_document).
const FILE_THRESHOLD_CHARS: usize = 4000;
/// read_file: cap konten yang masuk context LLM.
const READ_FILE_MAX_CHARS: usize = 15_000;
/// write_file: batas isi per call.
const WRITE_FILE_MAX_BYTES: usize = 100_000;
/// Grace period setelah kill sebelum menyerah membaca sisa output.
const KILL_GRACE: Duration = Duration::from_secs(5);

pub struct ShellCtx {
    pub bot: Bot,
    pub pool: PgPool,
    /// Root workdir yang diizinkan utk read_file/write_file + cwd default run_command.
    pub roots: Vec<PathBuf>,
    /// Direktori skills (Pilar 11) — dipakai tool save_skill.
    pub skills_dir: PathBuf,
    pub cmd_timeout: Duration,
    pub confirm_timeout: Duration,
    /// cwd per chat_id — `cd` efektif antar panggilan.
    cwds: Mutex<HashMap<i64, PathBuf>>,
    /// Konfirmasi destructive command yang pending: id → sender verdict.
    pending: Mutex<HashMap<u32, oneshot::Sender<bool>>>,
    next_id: AtomicU32,
}

impl ShellCtx {
    pub fn new(cfg: &Config, bot: Bot, pool: PgPool) -> Arc<Self> {
        Arc::new(Self {
            bot,
            pool,
            roots: cfg.work_roots.clone(),
            skills_dir: PathBuf::from(&cfg.skills_dir),
            cmd_timeout: Duration::from_secs(cfg.run_cmd_timeout),
            confirm_timeout: Duration::from_secs(cfg.confirm_timeout),
            cwds: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU32::new(1),
        })
    }

    fn cwd_for(&self, chat_id: i64) -> PathBuf {
        self.cwds
            .lock()
            .unwrap()
            .get(&chat_id)
            .cloned()
            .unwrap_or_else(|| {
                self.roots
                    .first()
                    .cloned()
                    .unwrap_or_else(|| PathBuf::from("."))
            })
    }

    /// Kirim keyboard approve/deny, lalu tunggu verdict owner (oneshot + timeout).
    /// Return false kalau denied / timeout / channel drop.
    async fn request_confirmation(&self, chat_id: i64, cmd: &str) -> bool {
        let (tx, rx) = oneshot::channel();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.pending.lock().unwrap().insert(id, tx);

        let kb = InlineKeyboardMarkup::new([[
            InlineKeyboardButton::callback("✅ Approve", format!("hmc:{id}:ok")),
            InlineKeyboardButton::callback("❌ Deny", format!("hmc:{id}:no")),
        ]]);
        let sent = self
            .bot
            .send_message(
                ChatId(chat_id),
                format!(
                    "⚠️ Perintah destruktif — eksekusi sekarang?\n\n$ {}\n\n(approve dalam {} detik)",
                    cmd,
                    self.confirm_timeout.as_secs()
                ),
            )
            .reply_markup(kb)
            .await;

        let msg_id = sent.ok().map(|m| m.id);
        // timeout() -> Option<Recv>; Recv -> Result; Err (sender drop) dianggap deny.
        let verdict = tokio::time::timeout(self.confirm_timeout, rx)
            .await
            .ok()
            .and_then(|r| r.ok())
            .unwrap_or(false);

        // Masih ada di map = tidak pernah dijawab (timeout) → tandai kedaluwarsa
        if msg_id.is_some() && self.pending.lock().unwrap().remove(&id).is_some() {
            let _ = self
                .bot
                .edit_message_text(
                    ChatId(chat_id),
                    msg_id.unwrap(),
                    "⏰ Konfirmasi timeout — perintah dibatalkan.",
                )
                .await;
        }
        verdict
    }

    /// Dipakai callback handler gateway: ambil sender konfirmasi by id.
    pub fn take_pending(&self, id: u32) -> Option<oneshot::Sender<bool>> {
        self.pending.lock().unwrap().remove(&id)
    }

    /// Eksekusi shell command (Pilar 9 mekanik inti).
    pub async fn run_command(&self, chat_id: i64, raw_cmd: &str) -> Result<String> {
        let cmd = raw_cmd.trim().trim_end_matches(';').trim();
        if cmd.is_empty() {
            bail!("parameter 'command' kosong");
        }

        // Confirmation gate destructive pattern (keamanan #3)
        if is_destructive(cmd) {
            tracing::warn!(chat_id, cmd, "destructive command — menunggu approval owner");
            if !self.request_confirmation(chat_id, cmd).await {
                self.audit(chat_id, &format!("[DIBATALKAN] {cmd}"), None, 0)
                    .await;
                return Ok("❌ Dibatalkan — owner tidak menyetujui (atau konfirmasi timeout).".into());
            }
        }

        let cwd = self.cwd_for(chat_id);
        // Wrapper: cd ke cwd chat → jalankan cmd → cetak marker cwd baru → exit rc asli.
        // Marker diparse dari stdout untuk update cwd map (cd efektif antar panggilan).
        let script = format!(
            "cd {} && {{ {}; }}\n__hermes_rc=$?\nprintf '\\n__HERMES_CWD__%s\\n' \"$PWD\"\nexit $__hermes_rc\n",
            sh_quote(&cwd),
            cmd
        );

        let started = Instant::now();
        let mut bcmd = tokio::process::Command::new("bash");
        bcmd.arg("-l")
            .arg("-c")
            .arg(&script)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // process group sendiri → saat timeout bisa killpg tanpa proses yatim
            bcmd.as_std_mut().process_group(0);
        }

        let mut child = bcmd
            .spawn()
            .with_context(|| format!("gagal spawn bash untuk: {cmd}"))?;
        let pid = child.id();
        let mut out_pipe = child.stdout.take().expect("stdout piped");
        let mut err_pipe = child.stderr.take().expect("stderr piped");
        let mut out_buf: Vec<u8> = Vec::new();
        let mut err_buf: Vec<u8> = Vec::new();

        let (status, timed_out) = match tokio::time::timeout(
            self.cmd_timeout,
            drain(&mut child, &mut out_pipe, &mut err_pipe, &mut out_buf, &mut err_buf),
        )
        .await
        {
            Ok(res) => (Some(res.context("bash exit")?), false),
            Err(_) => {
                kill_process_group(pid);
                let s = match tokio::time::timeout(
                    KILL_GRACE,
                    drain(&mut child, &mut out_pipe, &mut err_pipe, &mut out_buf, &mut err_buf),
                )
                .await
                {
                    Ok(Ok(s)) => Some(s),
                    _ => None,
                };
                (s, true)
            }
        };

        let duration_ms = started.elapsed().as_millis() as i64;
        let exit_code = status.as_ref().and_then(|s| s.code());

        let mut stdout = String::from_utf8_lossy(&out_buf).to_string();
        let stderr = String::from_utf8_lossy(&err_buf).to_string();
        if let Some(new_cwd) = extract_cwd(&mut stdout) {
            self.cwds.lock().unwrap().insert(chat_id, new_cwd);
        }

        let mut combined = stdout.trim_end().to_string();
        if !stderr.trim().is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str("[stderr]\n");
            combined.push_str(stderr.trim_end());
        }
        if timed_out {
            combined.push_str(&format!(
                "\n⏱️ Dibatalkan: melebihi timeout {} detik (process group di-kill).",
                self.cmd_timeout.as_secs()
            ));
        }

        self.audit(chat_id, cmd, exit_code, duration_ms).await;
        tracing::info!(chat_id, cmd, exit_code, duration_ms, timed_out, "run_command");

        // Output handling (Pilar 9): > threshold → file attachment; context LLM dapat tail
        let display = mask_secrets(&combined);
        let total = display.chars().count();
        if total > FILE_THRESHOLD_CHARS {
            let fname = format!("hermes-output-{}.txt", started.elapsed().as_millis());
            let doc = InputFile::memory(display.clone().into_bytes()).file_name(fname);
            if let Err(e) = self
                .bot
                .send_document(ChatId(chat_id), doc)
                .caption(format!(
                    "📋 Output penuh `{}` ({} char)",
                    truncate_chars(cmd, 80),
                    total
                ))
                .await
            {
                tracing::error!(chat_id, "gagal kirim output sebagai file: {e:#}");
            }
        }

        Ok(mask_secrets(&format!(
            "$ {}\nexit={} duration={}ms{}\n{}",
            cmd,
            exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "killed(signal)".into()),
            duration_ms,
            if timed_out { " TIMEOUT" } else { "" },
            tail_chars(&display, MAX_CTX_CHARS),
        )))
    }

    /// read_file dengan path guard (tool pendamping Pilar 9).
    pub async fn read_file(&self, chat_id: i64, path: &str) -> Result<String> {
        let canon = self.guard_path(path)?;
        let bytes = tokio::fs::read(&canon)
            .await
            .with_context(|| format!("gagal baca {}", canon.display()))?;
        let text = String::from_utf8_lossy(&bytes).to_string();
        let total = text.chars().count();

        if total > FILE_THRESHOLD_CHARS {
            let fname = canon
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "file.txt".into());
            let doc = InputFile::memory(text.clone().into_bytes()).file_name(fname);
            let _ = self
                .bot
                .send_document(ChatId(chat_id), doc)
                .caption(format!("📄 {} ({} char)", canon.display(), total))
                .await;
        }

        Ok(format!(
            "📄 {} ({} char):\n{}",
            canon.display(),
            total,
            head_chars(&text, READ_FILE_MAX_CHARS)
        ))
    }

    /// write_file dengan path guard + auto-create direktori.
    pub async fn write_file(&self, chat_id: i64, path: &str, content: &str) -> Result<String> {
        if content.len() > WRITE_FILE_MAX_BYTES {
            bail!(
                "konten {} byte > batas {} byte — pecah jadi beberapa write",
                content.len(),
                WRITE_FILE_MAX_BYTES
            );
        }
        let canon = self.guard_path(path)?;
        if let Some(parent) = canon.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let n = content.len();
        tokio::fs::write(&canon, content)
            .await
            .with_context(|| format!("gagal tulis {}", canon.display()))?;
        tracing::info!(chat_id, path = %canon.display(), bytes = n, "write_file");
        Ok(format!("✅ Ditulis {} byte → {}", n, canon.display()))
    }

    /// Path guard: resolve path (relatif ke root utama), naik ke ancestor yang ada,
    /// canonicalize (symlink ikut ter-resolve), lalu pastikan masih di bawah roots.
    fn guard_path(&self, raw: &str) -> Result<PathBuf> {
        let raw = raw.trim();
        if raw.is_empty() {
            bail!("path kosong");
        }
        let p = Path::new(raw);
        let joined = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.roots
                .first()
                .cloned()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(p)
        };

        // Naik sampai bagian yang benar-benar ada di disk, simpan sisanya
        let mut probe = joined.clone();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        let mut steps = 0;
        while !probe.exists() && steps < 64 {
            match probe.file_name() {
                Some(f) => {
                    tail.push(f.to_os_string());
                    match probe.parent() {
                        Some(par) => probe = par.to_path_buf(),
                        None => break,
                    }
                }
                None => break,
            }
            steps += 1;
        }
        let mut canon = std::fs::canonicalize(&probe)
            .with_context(|| format!("path tidak ditemukan: {}", probe.display()))?;
        for part in tail.iter().rev() {
            canon.push(part);
        }

        if !self.roots.iter().any(|r| canon.starts_with(r)) {
            bail!(
                "❌ akses di luar workdir yang diizinkan: [{}] — path: {}",
                self.roots
                    .iter()
                    .map(|r| r.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
                canon.display()
            );
        }
        Ok(canon)
    }

    /// Audit trail ke tabel command_logs (keamanan #4).
    async fn audit(&self, chat_id: i64, command: &str, exit_code: Option<i32>, duration_ms: i64) {
        if let Err(e) = sqlx::query(
            "INSERT INTO command_logs (chat_id, command, exit_code, duration_ms)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(chat_id)
        .bind(command)
        .bind(exit_code)
        .bind(duration_ms)
        .execute(&self.pool)
        .await
        {
            tracing::error!("audit command_logs gagal: {e:#}");
        }
    }
}

/// Parse callback data keyboard konfirmasi: "hmc:<id>:ok|no".
pub fn parse_confirm(data: &str) -> Option<(u32, bool)> {
    let rest = data.strip_prefix("hmc:")?;
    let (id, verdict) = rest.split_once(':')?;
    let id = id.parse().ok()?;
    let ok = match verdict {
        "ok" => true,
        "no" => false,
        _ => return None,
    };
    Some((id, ok))
}

/// Baca kedua pipe sampai EOF lalu tunggu exit — dipakai bersama timeout.
/// Pipe dibaca paralel supaya tidak deadlock kalau output besar (pipe buffer 64KB).
async fn drain(
    child: &mut Child,
    out: &mut ChildStdout,
    err: &mut ChildStderr,
    out_buf: &mut Vec<u8>,
    err_buf: &mut Vec<u8>,
) -> std::io::Result<std::process::ExitStatus> {
    let (o, e) = tokio::join!(out.read_to_end(out_buf), err.read_to_end(err_buf));
    o?;
    e?;
    child.wait().await
}

/// Kill process group (SIGKILL) — unix only; fallback non-unix via kill_on_drop.
#[cfg(unix)]
fn kill_process_group(pid: Option<u32>) {
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;
    if let Some(pid) = pid {
        let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_process_group(pid: Option<u32>) {
    let _ = pid; // best-effort: kill_on_drop(true) yang menangani
}

/// Quote path untuk disisipkan aman ke script bash (single-quote escape).
fn sh_quote(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"))
}

/// Ambil marker `__HERMES_CWD__<path>` terakhir dari stdout, strip dari output.
/// Return None kalau tidak ada / path tidak valid.
fn extract_cwd(stdout: &mut String) -> Option<PathBuf> {
    const MARKER: &str = "__HERMES_CWD__";
    let idx = stdout.rfind(MARKER)?;
    let after = &stdout[idx + MARKER.len()..];
    let end = after.find('\n').unwrap_or(after.len());
    let path = after[..end].trim().to_string(); // owned dulu — stdout akan di-truncate
    let valid = path.starts_with('/') && !path.contains(char::is_whitespace);

    let cut = if idx > 0 { idx - 1 } else { idx }; // buang newline sebelum marker juga
    stdout.truncate(cut);

    if valid {
        Some(PathBuf::from(path))
    } else {
        None
    }
}

fn tail_chars(s: &str, n: usize) -> String {
    let total = s.chars().count();
    if total <= n {
        return s.to_string();
    }
    let start = s
        .char_indices()
        .nth(total - n)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!(
        "…(dipotong — {total} char total, tampilkan {n} terakhir; versi penuh dikirim sebagai file)\n{}",
        &s[start..]
    )
}

fn head_chars(s: &str, n: usize) -> String {
    let total = s.chars().count();
    if total <= n {
        return s.to_string();
    }
    let end = s
        .char_indices()
        .nth(n)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}…(dipotong — {total} char total; versi penuh dikirim sebagai file)", &s[..end])
}

fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Deteksi command destruktif → butuh confirmation gate (keamanan #3).
/// Heuristik token-based (tanpa dependency regex) — err ke sisi aman.
pub fn is_destructive(cmd: &str) -> bool {
    let toks: Vec<&str> = cmd
        .split_whitespace()
        .filter(|t| *t != "sudo" && *t != "&&")
        .map(|t| t.trim_end_matches(';'))
        .collect();

    // rm rekursif + force (rm -rf, rm -fr, rm -r -f, --recursive --force)
    if let Some(i) = toks.iter().position(|t| *t == "rm") {
        let flags: Vec<&str> = toks[i + 1..]
            .iter()
            .take_while(|t| t.starts_with('-'))
            .map(|t| t.trim_start_matches('-'))
            .collect();
        let has_r = flags.iter().any(|f| f.contains('r') || *f == "recursive");
        let has_f = flags.iter().any(|f| f.contains('f') || *f == "force");
        if has_r && has_f {
            return true;
        }
    }

    // chmod/chown rekursif (-R huruf kapital — beda dengan -r yang bukan recursive)
    for name in ["chmod", "chown"] {
        if let Some(i) = toks.iter().position(|t| *t == name) {
            let flags: Vec<&str> = toks[i + 1..]
                .iter()
                .take_while(|t| t.starts_with('-'))
                .map(|t| t.trim_start_matches('-'))
                .collect();
            if flags.iter().any(|f| f.contains('R')) {
                return true;
            }
        }
    }

    // Command berbahaya di posisi mana pun (setelah && / ; / sebagai argumen lain)
    let hard = |t: &str| {
        matches!(
            t,
            "dd" | "mkfs" | "fdisk" | "sfdisk" | "parted" | "wipefs" | "blkdiscard" | "shred"
                | "reboot" | "shutdown" | "halt" | "poweroff"
        ) || t.starts_with("mkfs.")
    };
    if toks.iter().any(|t| hard(t)) {
        return true;
    }

    // init 0 / init 6
    if let Some(i) = toks.iter().position(|t| *t == "init") {
        if matches!(toks.get(i + 1), Some(&"0") | Some(&"6")) {
            return true;
        }
    }

    // Tulis langsung ke block device
    let joined = toks.join(" ");
    if ["/dev/sd", "/dev/nvme", "/dev/vd", "/dev/mmcblk"].iter().any(|d| {
        joined.contains(&format!("> {d}")) || joined.contains(&format!("of={d}"))
    }) {
        return true;
    }

    // curl/wget di-pipe ke shell
    let downloads = toks.iter().any(|t| *t == "curl" || *t == "wget");
    let pipe_shell = toks
        .windows(2)
        .any(|w| w[0] == "|" && matches!(w[1], "sh" | "bash" | "zsh" | "dash" | "ksh"));
    downloads && pipe_shell
}

/// Secret masking (keamanan #5): nilai env/credential tidak boleh bocor ke Telegram.
/// KEY=VALUE / KEY:VALUE + token dikenal (sk-ant-, ghp_, AKIA, pola 32hex.16alnum).
pub fn mask_secrets(s: &str) -> String {
    s.lines().map(mask_line).collect::<Vec<_>>().join("\n")
}

fn mask_line(line: &str) -> String {
    line.split(' ')
        .map(mask_token)
        .collect::<Vec<_>>()
        .join(" ")
}

fn mask_token(tok: &str) -> String {
    // KEY=VALUE
    if let Some(eq) = tok.find('=') {
        let (l, _) = tok.split_at(eq);
        let key = l.trim_end_matches(':').to_ascii_lowercase();
        if is_secret_key(&key) && tok.len() - eq > 4 {
            return format!("{l}=***");
        }
    }
    // KEY:VALUE satu token (mis. "password:rahasia")
    if let Some(col) = tok.find(':') {
        let (l, _) = tok.split_at(col);
        let key = l.to_ascii_lowercase();
        if is_secret_key(&key) && tok.len() - col > 5 {
            return format!("{l}:***");
        }
    }
    // Token rahasia berdiri sendiri
    if is_secret_token(tok) {
        return "***".into();
    }
    tok.to_string()
}

fn is_secret_key(k: &str) -> bool {
    const KEYS: &[&str] = &[
        "api_key",
        "apikey",
        "key",
        "token",
        "secret",
        "client_secret",
        "password",
        "passwd",
        "passphrase",
        "credential",
        "credentials",
        "auth",
        "authorization",
        "bearer",
        "private_key",
    ];
    KEYS.contains(&k)
}

fn is_secret_token(t: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "sk-ant-", "sk-proj-", "sk-", "xoxb-", "xoxp-", "ghp_", "gho_", "github_pat_",
        "glpat-", "AKIA", "AIza",
    ];
    if t.len() >= 16 && PREFIXES.iter().any(|p| t.starts_with(p)) {
        return true;
    }
    // Pola GLM/zhipu: 32 hex + '.' + >= 8 alnum
    if let Some(d) = t.find('.') {
        let (head, _) = t.split_at(d);
        let rest = &t[d + 1..];
        if head.len() == 32
            && head.bytes().all(|c| c.is_ascii_hexdigit())
            && rest.len() >= 8
            && rest.bytes().all(|c| c.is_ascii_alphanumeric())
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destructive_detection() {
        assert!(is_destructive("rm -rf /tmp/x"));
        assert!(is_destructive("sudo rm -fr /tmp/x"));
        assert!(is_destructive("rm -r -f /tmp/x"));
        assert!(is_destructive("rm --recursive --force /tmp/x"));
        assert!(is_destructive("docker exec c rm -rf /data"));
        assert!(is_destructive("dd if=/dev/zero of=/dev/sda"));
        assert!(is_destructive("mkfs.ext4 /dev/sdb"));
        assert!(is_destructive("apt update && reboot"));
        assert!(is_destructive("chmod -R 777 /var/www"));
        assert!(is_destructive("sudo chown -R www-data: /var"));
        assert!(is_destructive("curl -fsSL https://x.sh | sh"));
        assert!(is_destructive("wget -qO- https://x | bash"));
        assert!(is_destructive("echo hi > /dev/sda"));
        assert!(is_destructive("systemctl reboot"));
        assert!(is_destructive("init 6"));

        assert!(!is_destructive("rm file.txt"));
        assert!(!is_destructive("rm -r build/"));
        assert!(!is_destructive("chmod +x script.sh"));
        assert!(!is_destructive("curl -s https://api.example.com/data"));
        assert!(!is_destructive("ls -la; df -h"));
        assert!(!is_destructive("grep -r pattern ."));
        assert!(!is_destructive("docker rm -f webapp"));
        assert!(!is_destructive("systemctl restart nginx"));
    }

    #[test]
    fn masking() {
        let out = mask_secrets(
            "API_KEY=abc123XYZ done\nAuthorization: Bearer sk-ant-1234567890abcdefgh\n\
             key zhipu: f35c75275dda4caf880ceb824c23779b.xGHJOL8VhSORhm0Y\nnormal line",
        );
        assert!(out.contains("API_KEY=***"));
        assert!(!out.contains("abc123XYZ"));
        assert!(!out.contains("sk-ant-1234"));
        assert!(!out.contains("f35c75275dda4caf880ceb824c23779b"));
        assert!(out.contains("normal line"));
    }

    #[test]
    fn cwd_marker_extraction() {
        let mut s = "line1\nline2\n__HERMES_CWD__/etc/nginx\n".to_string();
        let cwd = extract_cwd(&mut s);
        assert_eq!(cwd, Some(PathBuf::from("/etc/nginx")));
        assert_eq!(s, "line1\nline2");
        assert_eq!(extract_cwd(&mut "no marker".to_string()), None);
    }

    #[test]
    fn confirm_data_parsing() {
        assert_eq!(parse_confirm("hmc:42:ok"), Some((42, true)));
        assert_eq!(parse_confirm("hmc:7:no"), Some((7, false)));
        assert_eq!(parse_confirm("hmc:x:ok"), None);
        assert_eq!(parse_confirm("other:1:ok"), None);
    }
}
