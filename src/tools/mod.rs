use crate::memory;
use crate::provider::ToolDef;
use crate::reminders;
use crate::shell::ShellCtx;
use anyhow::{bail, Result};
use serde_json::{json, Value};

/// Definisi tools yang diekspos ke LLM (Fase 2: reminder + memory;
/// Fase 4+ menambah run_command/read_file/write_file, web, skills, image — lihat ROADMAP).
pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "create_reminder".into(),
            description: "Buat pengingat atau tugas terjadwal. Gunakan setiap kali owner minta \
                diingatkan sesuatu, atau minta rutinitas berulang (briefing harian, laporan \
                mingguan, cek backup). LLM yang memutuskan kapan memanggil tool ini — \
                jangan minta owner menulis format khusus."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "Isi reminder (untuk dikirim ulang ke owner), ATAU instruksi lengkap untuk job yang harus dikerjakan agent"
                    },
                    "remind_at": {
                        "type": "string",
                        "description": "Waktu trigger, format RFC 3339 dengan timezone, contoh: 2025-06-01T15:00:00+07:00. Owner di Asia/Jakarta (UTC+7)."
                    },
                    "kind": {
                        "type": "string",
                        "enum": ["static", "job"],
                        "description": "static = kirim teks apa adanya saat waktunya tiba. job = agent mengeksekusi instruksi (misal: 'cari berita teknologi hari ini lalu rangkum 5 poin'). Default: static."
                    },
                    "recur": {
                        "type": "string",
                        "enum": ["daily", "weekly"],
                        "description": "Opsional — kosongkan untuk one-shot."
                    }
                },
                "required": ["message", "remind_at"]
            }),
        },
        ToolDef {
            name: "save_memory".into(),
            description: "Simpan fakta stabil tentang owner yang berguna lintas sesi \
                (preferensi, jadwal rutin, proyek yang dikerjakan, kebiasaan). \
                Panggil proaktif saat owner menyebut info penting — tidak perlu minta izin. \
                JANGAN simpan info basi/saat ini juga (cuaca, topik obrolan biasa)."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "fact": {
                        "type": "string",
                        "description": "Satu fakta ringkas, ditulis dari sudut pandang 'owner ...'"
                    },
                    "fact_type": {
                        "type": "string",
                        "enum": ["explicit", "inferred"],
                        "description": "explicit = owner menyebutnya langsung. inferred = disimpulkan dari konteks (tandai jujur sebagai dugaan). Default: explicit."
                    }
                },
                "required": ["fact"]
            }),
        },
        ToolDef {
            name: "run_command".into(),
            description: "Jalankan shell command di VPS server ini (bash). Working directory \
                diingat antar panggilan — `cd` efektif untuk command berikutnya. Command biasa \
                langsung dieksekusi; command destruktif (rm -rf, dd, reboot, curl|sh, dst.) \
                otomatis minta approval owner via tombol Telegram. Timeout eksekusi 120 detik; \
                command interaktif (perlu input stdin) tidak didukung. Output panjang otomatis \
                dikirim sebagai file ke owner."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Command bash lengkap (bisa satu baris dengan && atau ;)"
                    }
                },
                "required": ["command"]
            }),
        },
        ToolDef {
            name: "read_file".into(),
            description: "Baca isi file dari disk. Path absolut, atau relatif terhadap \
                workdir utama. Hanya di dalam direktori yang diizinkan. File besar otomatis \
                dikirim sebagai file ke owner. Lebih hemat & aman daripada cat via shell."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path file (absolut atau relatif workdir)" }
                },
                "required": ["path"]
            }),
        },
        ToolDef {
            name: "write_file".into(),
            description: "Tulis (overwrite penuh) isi file. Direktori dibuat otomatis kalau \
                belum ada. Hanya di dalam direktori yang diizinkan. Maksimum ~100KB per call — \
                untuk perubahan besar, pecah jadi beberapa call. Lebih aman daripada echo/heredoc \
                via shell."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path file tujuan" },
                    "content": { "type": "string", "description": "Isi file lengkap" }
                },
                "required": ["path", "content"]
            }),
        },
    ]
}

/// Eksekusi tool call dari LLM. Return string hasil yang dikirim balik sebagai tool_result.
pub async fn execute(shell: &ShellCtx, chat_id: i64, name: &str, input: &Value) -> Result<String> {
    let pool = &shell.pool;
    match name {
        "create_reminder" => {
            let message = input["message"].as_str().unwrap_or("").trim().to_string();
            let remind_at_raw = input["remind_at"].as_str().unwrap_or("").trim().to_string();
            if message.is_empty() || remind_at_raw.is_empty() {
                bail!("parameter 'message' dan 'remind_at' wajib diisi");
            }
            let remind_at = reminders::parse_remind_at(&remind_at_raw)?;
            let kind = input["kind"].as_str().unwrap_or("static").to_string();
            let recur = input["recur"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());

            let id =
                reminders::create(pool, chat_id, &message, remind_at, &kind, recur).await?;
            Ok(format!(
                "✅ Reminder #{} dibuat: \"{}\" — trigger {} [{}{}]",
                id,
                message,
                reminders::fmt_jakarta(remind_at),
                kind,
                match reminders::compute_next_run(
                    input["recur"].as_str().unwrap_or(""),
                    remind_at
                ) {
                    Some(_) => ", berulang".to_string(),
                    None => String::new(),
                }
            ))
        }
        "save_memory" => {
            let fact = input["fact"].as_str().unwrap_or("").trim().to_string();
            if fact.is_empty() {
                bail!("parameter 'fact' wajib diisi");
            }
            let fact_type = match input["fact_type"].as_str() {
                Some("inferred") => "inferred",
                _ => "explicit",
            };
            memory::save_fact(pool, chat_id, &fact, fact_type).await
        }
        "run_command" => {
            let command = input["command"].as_str().unwrap_or("").trim().to_string();
            if command.is_empty() {
                bail!("parameter 'command' wajib diisi");
            }
            shell.run_command(chat_id, &command).await
        }
        "read_file" => {
            let path = input["path"].as_str().unwrap_or("").trim().to_string();
            if path.is_empty() {
                bail!("parameter 'path' wajib diisi");
            }
            shell.read_file(chat_id, &path).await
        }
        "write_file" => {
            let path = input["path"].as_str().unwrap_or("").trim().to_string();
            let content = input["content"].as_str().unwrap_or("").to_string();
            if path.is_empty() {
                bail!("parameter 'path' wajib diisi");
            }
            shell.write_file(chat_id, &path, &content).await
        }
        other => bail!("tool tidak dikenal: {}", other),
    }
}
