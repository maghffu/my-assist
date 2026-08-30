# markdown.new — ekstraksi URL → Markdown (pengetahuan tahan lama, migrasi dari hermes-agent 29 Agu 2026)

Layanan gratis Cloudflare untuk mengubah halaman web jadi Markdown bersih. Di hermes-lite ini tier-2 di fallback chain `fetch_url` (Pilar 10) — dokumen ini catatan perilaku layanan yang terverifikasi langsung di lapangan.

## Perilaku layanan (terverifikasi 20 Agu 2026)

- `GET https://markdown.new/<full-url>` → HTTP 200, Markdown bersih (~80% lebih hemat token dari HTML mentah)
- Response diawali preamble `Title: <judul>` / `URL Source: <url>`, lalu `Markdown Content:` dan body — **strip preamble** sebelum pakai
- Latensi ~4 detik untuk halaman blog (hasil 20KB markdown) — set timeout request ≥30 detik
- Tanpa API key, tanpa SDK — cukup HTTP GET biasa
- Halaman JS-heavy: layanan otomatis fallback ke Cloudflare Browser Rendering di belakang layar
- Rate limit eksplisit: 500 request/hari/IP + header `x-rate-limit-remaining` untuk tracking — pantau kalau dipakai massal

## Gotchas operasional

- Kalau hasil fetch terasa aneh/kosong, cek dulu apakah response masih format preamble-nya (Title/URL Source) — parser yang lupa strip preamble akan bocor metadata ke context.
- Alternatif fallback kalau markdown.new down/limit: `r.jina.ai` keyless (rate limit ketat), atau direct fetch + header `Accept: text/markdown` (content negotiation Cloudflare "Markdown for Agents").
- Layanan ini melewati infra pihak ketiga — JANGAN fetch URL yang butuh auth/berisi data sensitif (aturan hygiene Pilar 10).

## Riwayat

Sebelumnya dipakai sebagai custom web-extract provider di hermes-agent (`web.extract_backend: markdownnew`, plugin Python ~100 baris dengan relative-import quirks). Plugin itu pensiun bersama hermes-agent — hermes-lite mengimplementasikan chain fetch native di Rust. Dokumen plugin asli ada di backup pre-migration.
