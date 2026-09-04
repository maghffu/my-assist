use crate::memory;
use crate::provider::ToolDef;
use crate::reminders;
use crate::shell::ShellCtx;
use crate::web::WebCtx;
use anyhow::{bail, Result};
use serde_json::{json, Value};

/// Definisi tools yang diekspos ke LLM (Fase 2: reminder + memory;
/// Fase 4: run_command/read_file/write_file; Fase 5: web_search/fetch_url/generate_image).
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
                langsung dieksekusi; command berisiko (rm -rf, dd, reboot, curl|sh, dst.) \
                otomatis diminta approval owner via tombol Telegram (✅ jalankan sekali / \
                🔁 sesi ini / ❌ tolak) — kalau approved, LANJUTKAN tugasnya sampai selesai; \
                kalau ditolak/timeout, laporkan dan JANGAN ulangi sendiri. Timeout eksekusi \
                120 detik; command interaktif (perlu input stdin) tidak didukung — pakai flag \
                non-interaktif (-y, DEBIAN_FRONTEND=noninteractive). Output panjang otomatis \
                dipotong — hanya bagian akhir (tail) yang terlihat; kalau hasil berpotensi \
                panjang (log, dump SQL, ls besar), pipe dari awal: `| head -n 50`, `| tail`, \
                `grep`, atau `LIMIT` di SQL — jangan dump semua lalu filter belakangan."
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
                dipotong (hanya awalnya yang terbaca) — untuk file besar minta baca bagian \
                spesifik saja. Lebih hemat & aman daripada cat via shell."
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
        ToolDef {
            name: "web_search".into(),
            description: "Cari informasi terkini di web (berita, versi library terbaru, harga, \
                error message, dokumentasi). WAJIB dipakai untuk pertanyaan yang butuh info lebih \
                baru dari pengetahuanmu — jangan menebak dari ingatan lama. Return daftar hasil \
                {judul, url, snippet} — sering sudah cukup untuk menjawab tanpa fetch halaman."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Query pencarian — spesifik, dalam bahasa yang paling relevan untuk topiknya"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Jumlah hasil (1-10, default 5)"
                    }
                },
                "required": ["query"]
            }),
        },
        ToolDef {
            name: "fetch_url".into(),
            description: "Ambil isi sebuah URL publik sebagai teks/markdown bersih. Gunakan \
                setelah web_search kalau perlu isi halaman penuh (dokumentasi, artikel, changelog). \
                Konten > 15 ribu karakter otomatis dipotong (hanya awalnya). Hanya URL publik \
                http/https — bukan IP internal/localhost."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "URL lengkap http/https" }
                },
                "required": ["url"]
            }),
        },
        ToolDef {
            name: "generate_image".into(),
            description: "Buat gambar dari deskripsi teks dan kirim langsung ke owner sebagai \
                foto (1024x1024, Pollinations). Prompt yang baik: subjek + gaya visual + mood + \
                lighting + detail penting. Kamu TIDAK bisa melihat hasilnya — kalau owner minta \
                revisi (misal 'birunya kurang'), panggil lagi dengan prompt yang dimodifikasi."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "Deskripsi gambar yang diinginkan, detail dan spesifik"
                    }
                },
                "required": ["prompt"]
            }),
        },
        ToolDef {
            name: "save_skill".into(),
            description: "Simpan prosedur yang BARU saja kamu kuasai/ketahui sebagai skill \
                (file markdown). Panggil HANYA setelah menyelesaikan masalah NON-TRIVIAL yang \
                mungkin dihadapi lagi: setup server, troubleshooting, konfigurasi, workaround \
                error, alur deploy. Isi: langkah bernomor, command yang TERBUKTI jalan (salin \
                persis), dan gotchas. JANGAN simpan: jawaban FAQ, one-liner, pengetahuan umum \
                yang bisa dicari ulang — itu bukan skill."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Nama pendek deskriptif, kebab-case, mis: renew-ssl-nginx"
                    },
                    "description": {
                        "type": "string",
                        "description": "Satu kalimat (max 160 char) menjelaskan KAPAN/untuk apa skill ini dipakai — dipakai sistem utk mencocokkan skill dengan topik pesan. Tulis topiknya eksplisit (mis. 'renew sertifikat SSL certbot, termasuk urusan port 80 yang dipakai apache'). Kosongkan utk derive otomatis dari baris pertama konten."
                    },
                    "content": {
                        "type": "string",
                        "description": "Isi skill: langkah, command, gotchas (markdown) — TANPA blok frontmatter (otomatis ditulis sistem)"
                    }
                },
                "required": ["name", "content"]
            }),
        },
    ]
}

/// Eksekusi tool call dari LLM. Return string hasil yang dikirim balik sebagai tool_result.
pub async fn execute(
    shell: &ShellCtx,
    web: &WebCtx,
    chat_id: i64,
    name: &str,
    input: &Value,
) -> Result<String> {
    let pool = &shell.pool;
    match name {
        "create_reminder" => {
            let message = input["message"].as_str().unwrap_or("").trim().to_string();
            let remind_at_raw = input["remind_at"].as_str().unwrap_or("").trim().to_string();
            if message.is_empty() || remind_at_raw.is_empty() {
                bail!("parameter 'message' dan 'remind_at' wajib diisi");
            }
            let mut remind_at = reminders::parse_remind_at(&remind_at_raw)?;
            // Recurring: kalau remind_at sudah lewat, roll forward ke kemunculan
            // berikutnya (anchor HH:MM tetap) — cegah fire langsung & drift.
            if input["recur"]
                .as_str()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
            {
                let now = chrono::Utc::now();
                while remind_at <= now {
                    match reminders::compute_next_run(
                        input["recur"].as_str().unwrap_or(""),
                        remind_at,
                        now,
                    ) {
                        Some(next) => remind_at = next,
                        None => break,
                    }
                }
            }
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
                    remind_at,
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
            // Memory v2 — save filter: tolak entri transien/trivial sebelum masuk DB.
            if fact_type == "inferred" && memory::is_transient_fact(&fact) {
                return Ok(format!(
                    "\u{2139}\u{fe0f} Entri dilewati save filter (transien/trivial): {fact}"
                ));
            }
            // Semantik-dedup: near-duplicate -> UPDATE entri lama, bukan INSERT baru.
            if let Some(id) = memory::find_near_duplicate(pool, chat_id, &fact).await? {
                memory::update_fact(pool, chat_id, id, &fact).await?;
                return Ok(format!(
                    "\u{2705} Memory diperbarui (duplikat digabung ke id {id}): {fact}"
                ));
            }
            let res = memory::save_fact(pool, chat_id, &fact, fact_type).await;
            // Warning save-gagal TIDAK boleh diam-diam: log + lempar error ke owner.
            if let Err(e) = &res {
                tracing::warn!(chat_id, "save_memory GAGAL: {e:#}");
            }
            res
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
        "web_search" => {
            let query = input["query"].as_str().unwrap_or("").trim().to_string();
            if query.is_empty() {
                bail!("parameter 'query' wajib diisi");
            }
            let max = input["max_results"].as_u64().unwrap_or(5) as usize;
            web.web_search(&query, max).await
        }
        "fetch_url" => {
            let url = input["url"].as_str().unwrap_or("").trim().to_string();
            if url.is_empty() {
                bail!("parameter 'url' wajib diisi");
            }
            web.fetch_url(chat_id, &url).await
        }
        "generate_image" => {
            let prompt = input["prompt"].as_str().unwrap_or("").trim().to_string();
            if prompt.is_empty() {
                bail!("parameter 'prompt' wajib diisi");
            }
            web.generate_image(chat_id, &prompt).await
        }
        "save_skill" => {
            let name = input["name"].as_str().unwrap_or("").trim().to_string();
            let content = input["content"].as_str().unwrap_or("").trim().to_string();
            let description = input["description"]
                .as_str()
                .map(str::trim)
                .filter(|d| !d.is_empty());
            if name.is_empty() || content.is_empty() {
                bail!("parameter 'name' dan 'content' wajib diisi");
            }
            crate::skills::save_skill(&shell.skills_dir, &name, description, &content)
        }
        other => bail!("tool tidak dikenal: {}", other),
    }
}
